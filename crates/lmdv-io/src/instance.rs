//! The single-instance lock and Unix-socket protocol.
//!
//! The first process takes a `flock` on `<socket>.lock`, binds a stream socket
//! (mode 0600 in a directory 0700) and forwards each launch to a callback. A
//! later process connects and waits for an ack. Messages are a big-endian
//! `u32` length followed by UTF-8 JSON, capped at 1 MiB. `SO_PEERCRED` must
//! match the effective uid.
//!
//! The listener only forwards a [`LaunchRequest`]; it never opens files.
//!
//! Windows has no Unix-domain socket path in `std` and no peer credentials, so
//! the transport is a compile-time stub there: the message types and codec are
//! shared, and [`InstancePaths::discover`], [`InstanceServer::claim`] and
//! [`forward`] return [`InstanceError::Unsupported`]. The app degrades to
//! starting without single-instance routing, exactly as it does when a Unix
//! claim fails.

use std::fs::File;
#[cfg(unix)]
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::thread;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
use fs2::FileExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::dirs::AppDirs;
use crate::launch::Launch;

/// The socket protocol version.
pub const PROTOCOL_VERSION: u32 = 1;
/// Maximum JSON payload: 1 MiB.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// How long a client waits for a starting primary to bind.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a client waits for the UI to acknowledge a launch.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(2);

/// One launch forwarded to the running instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchRequest {
    pub v: u32,
    pub launch: Launch,
    /// `XDG_ACTIVATION_TOKEN`, when the launcher provided one.
    pub activation_token: Option<String>,
}

impl Default for LaunchRequest {
    fn default() -> Self {
        Self { v: PROTOCOL_VERSION, launch: Launch::default(), activation_token: None }
    }
}

impl LaunchRequest {
    /// Wraps a parsed CLI launch with the current protocol version.
    pub fn new(launch: Launch) -> Self {
        Self { launch, ..Self::default() }
    }
}

/// The primary's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchAck {
    pub v: u32,
    pub ok: bool,
    pub error: Option<String>,
}

impl Default for LaunchAck {
    fn default() -> Self {
        Self { v: PROTOCOL_VERSION, ok: false, error: None }
    }
}

impl LaunchAck {
    /// The launch was handed to the UI.
    pub fn ok() -> Self {
        Self { ok: true, ..Self::default() }
    }

    /// The launch could not be routed.
    pub fn failed(message: impl Into<String>) -> Self {
        Self { error: Some(message.into()), ..Self::default() }
    }
}

/// The socket and lock file used by one instance id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstancePaths {
    /// The directory holding the socket; mode 0700.
    pub dir: PathBuf,
    /// `$XDG_RUNTIME_DIR/lmdv-<id>.sock`, or a cache/tmp fallback.
    pub socket: PathBuf,
    /// The `flock` file, never removed.
    pub lock: PathBuf,
}

impl InstancePaths {
    /// Resolves the runtime directory with the XDG ownership/mode checks and
    /// cache/tmp fallbacks.
    #[cfg(unix)]
    pub fn discover(dirs: &AppDirs) -> Result<Self, InstanceError> {
        let uid = effective_uid();
        let name = socket_name(&dirs.instance_id);

        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
            && runtime.is_absolute()
            && private_dir_ok(&runtime, uid)
        {
            let socket = runtime.join(&name);
            if fits(&socket) {
                return Ok(Self::build(runtime, socket));
            }
        }

        let cache = dirs.cache.clone();
        if create_private_dir(&cache, uid).is_ok() {
            let socket = cache.join(&name);
            if fits(&socket) {
                return Ok(Self::build(cache, socket));
            }
        }

        let fallback = PathBuf::from(format!("/tmp/lmdv-{uid}"));
        create_private_dir(&fallback, uid)?;
        let socket = fallback.join(&name);
        if !fits(&socket) {
            return Err(InstanceError::PathTooLong(socket));
        }
        Ok(Self::build(fallback, socket))
    }

    /// Windows has no single-instance transport here yet.
    #[cfg(windows)]
    pub fn discover(_dirs: &AppDirs) -> Result<Self, InstanceError> {
        Err(InstanceError::Unsupported)
    }

    /// Builds paths in an explicit directory. Used by tests and advanced
    /// callers; [`InstanceServer::claim`] still creates the directory 0700.
    pub fn in_dir(dir: impl Into<PathBuf>, instance_id: &str) -> Self {
        let dir = dir.into();
        let socket = dir.join(socket_name(instance_id));
        Self::build(dir, socket)
    }

    fn build(dir: PathBuf, socket: PathBuf) -> Self {
        let mut lock = socket.as_os_str().to_owned();
        lock.push(".lock");
        Self { dir, socket, lock: PathBuf::from(lock) }
    }
}

/// The result of claiming the instance role.
pub enum Claim {
    /// This process owns the lock and listens.
    Primary(InstanceServer),
    /// Another process owns the lock; forward launches to [`InstancePaths`].
    Secondary(InstancePaths),
}

/// The listener thread and its held lock.
pub struct InstanceServer {
    paths: InstancePaths,
    _lock: File,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl InstanceServer {
    /// Claims the single-instance role.
    ///
    /// A free lock makes this process primary: any leftover socket is removed
    /// only when it is a socket owned by the effective uid and no live process
    /// answers on it. A held lock returns [`Claim::Secondary`]. The callback
    /// runs on the listener thread and must not block on the UI thread; it
    /// returns the ack written to the client.
    #[cfg(unix)]
    pub fn claim<F>(paths: InstancePaths, on_launch: F) -> Result<Claim, InstanceError>
    where
        F: FnMut(LaunchRequest) -> LaunchAck + Send + 'static,
    {
        if !fits(&paths.socket) {
            return Err(InstanceError::PathTooLong(paths.socket));
        }
        let uid = effective_uid();
        create_private_dir(&paths.dir, uid)?;
        let lock = open_lock(&paths.lock)?;

        match lock.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if is_would_block(&error) => return Ok(Claim::Secondary(paths)),
            Err(error) => return Err(InstanceError::Lock(error)),
        }

        match fs::symlink_metadata(&paths.socket) {
            Ok(metadata) => {
                if metadata.uid() != uid || !metadata.file_type().is_socket() {
                    return Err(InstanceError::UnsafeSocket(paths.socket));
                }
                match UnixStream::connect(&paths.socket) {
                    // A live process answers even though the lock is free: do
                    // not unlink its socket; forward instead.
                    Ok(_) => return Ok(Claim::Secondary(paths)),
                    Err(error)
                        if matches!(error.kind(), io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound) =>
                    {
                        fs::remove_file(&paths.socket)?;
                    }
                    Err(error) => return Err(InstanceError::Io(error)),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(InstanceError::Io(error)),
        }

        let listener = UnixListener::bind(&paths.socket)?;
        fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let socket = paths.socket.clone();
        let thread = match thread::Builder::new()
            .name(format!("lmdv-instance-{}", socket.display()))
            .spawn(move || serve(listener, thread_stop, on_launch))
        {
            Ok(thread) => thread,
            Err(error) => {
                let _ = fs::remove_file(&paths.socket);
                return Err(InstanceError::Io(error));
            }
        };

        Ok(Claim::Primary(Self { paths, _lock: lock, stop, thread: Some(thread) }))
    }

    /// Windows has no single-instance transport here yet. The app treats this
    /// exactly like any other claim failure and starts without routing.
    #[cfg(windows)]
    pub fn claim<F>(_paths: InstancePaths, _on_launch: F) -> Result<Claim, InstanceError>
    where
        F: FnMut(LaunchRequest) -> LaunchAck + Send + 'static,
    {
        Err(InstanceError::Unsupported)
    }

    /// The socket and lock paths.
    pub fn paths(&self) -> &InstancePaths {
        &self.paths
    }
}

impl Drop for InstanceServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        #[cfg(unix)]
        {
            if let Ok(metadata) = fs::symlink_metadata(&self.paths.socket)
                && metadata.uid() == effective_uid()
                && metadata.file_type().is_socket()
            {
                let _ = fs::remove_file(&self.paths.socket);
            }
        }
    }
}

/// Forwards a launch to the primary and waits [`ACK_TIMEOUT`] for its answer.
///
/// Connection is retried for [`CONNECT_TIMEOUT`] so a primary that is still
/// starting can bind; there is no busy loop and no UI work here.
#[cfg(unix)]
pub fn forward(paths: &InstancePaths, request: &LaunchRequest) -> Result<LaunchAck, InstanceError> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let stream = loop {
        match UnixStream::connect(&paths.socket) {
            Ok(stream) => break stream,
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
                if Instant::now() >= deadline {
                    return Err(InstanceError::Timeout);
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(InstanceError::Io(error)),
        }
    };

    let mut stream = stream;
    stream.set_read_timeout(Some(ACK_TIMEOUT))?;
    stream.set_write_timeout(Some(ACK_TIMEOUT))?;
    write_message(&mut stream, request).map_err(map_ack_timeout)?;
    let ack: LaunchAck = read_message(&mut stream).map_err(map_ack_timeout)?;
    if ack.v != PROTOCOL_VERSION {
        return Err(InstanceError::Version(ack.v));
    }
    Ok(ack)
}

/// Windows has no single-instance transport here yet.
#[cfg(windows)]
pub fn forward(_paths: &InstancePaths, _request: &LaunchRequest) -> Result<LaunchAck, InstanceError> {
    Err(InstanceError::Unsupported)
}

#[cfg(unix)]
fn map_ack_timeout(error: InstanceError) -> InstanceError {
    match &error {
        InstanceError::Io(source) if matches!(source.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
            InstanceError::Timeout
        }
        _ => error,
    }
}

/// Writes one length-prefixed JSON message.
pub fn write_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> Result<(), InstanceError> {
    let bytes = serde_json::to_vec(message).map_err(|error| InstanceError::Protocol(error.to_string()))?;
    if bytes.is_empty() || bytes.len() > MAX_MESSAGE_BYTES {
        return Err(InstanceError::Protocol(format!("message is {} bytes", bytes.len())));
    }
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

/// Reads one length-prefixed JSON message.
pub fn read_message<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, InstanceError> {
    let mut header = [0u8; 4];
    reader.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    if len == 0 || len > MAX_MESSAGE_BYTES {
        return Err(InstanceError::Protocol(format!("invalid message length {len}")));
    }
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|error| InstanceError::Protocol(error.to_string()))
}

/// Why the instance service failed.
#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    #[error("single-instance I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("invalid single-instance message: {0}")]
    Protocol(String),
    #[error("the socket path is too long: {0}")]
    PathTooLong(PathBuf),
    #[error("refusing to touch an unsafe socket: {0}")]
    UnsafeSocket(PathBuf),
    #[error("another lmdv instance did not answer")]
    Timeout,
    #[error("unsupported protocol version {0}")]
    Version(u32),
    #[error("cannot lock the instance file: {0}")]
    Lock(#[source] io::Error),
    #[error("single-instance routing is not supported on this platform")]
    Unsupported,
}

#[cfg(unix)]
fn serve<F>(listener: UnixListener, stop: Arc<AtomicBool>, mut on_launch: F)
where
    F: FnMut(LaunchRequest) -> LaunchAck,
{
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => handle_client(stream, &mut on_launch),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(20)),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[cfg(unix)]
fn handle_client<F>(mut stream: UnixStream, on_launch: &mut F)
where
    F: FnMut(LaunchRequest) -> LaunchAck,
{
    let _ = stream.set_read_timeout(Some(ACK_TIMEOUT));
    let _ = stream.set_write_timeout(Some(ACK_TIMEOUT));
    match peer_uid(&stream) {
        Ok(uid) if uid == effective_uid() => {}
        _ => return,
    }

    let request = match read_message::<_, LaunchRequest>(&mut stream) {
        Ok(request) => request,
        Err(_) => return,
    };
    if request.v != PROTOCOL_VERSION {
        let _ = write_message(&mut stream, &LaunchAck::failed(format!("unsupported version {}", request.v)));
        return;
    }
    let ack = on_launch(request);
    let _ = write_message(&mut stream, &ack);
}

#[cfg(unix)]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { credentials.assume_init() }.uid)
}

#[cfg(unix)]
fn open_lock(path: &Path) -> io::Result<File> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && !metadata.file_type().is_file()
    {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the instance lock is not a regular file"));
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(unix)]
fn is_would_block(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        || error.raw_os_error() == Some(libc::EWOULDBLOCK)
        || error.raw_os_error() == Some(libc::EAGAIN)
}

fn socket_name(instance_id: &str) -> String {
    let mut clean: String = instance_id
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => character,
            _ => '-',
        })
        .collect();
    clean.truncate(64);
    if clean.is_empty() {
        clean.push_str("main");
    }
    format!("lmdv-{clean}.sock")
}

#[cfg(unix)]
fn fits(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().len() < 108
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn private_dir_ok(path: &Path, uid: u32) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            metadata.file_type().is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == uid
                && metadata.permissions().mode() & 0o077 == 0
        }
        Err(_) => false,
    }
}

#[cfg(unix)]
fn create_private_dir(path: &Path, uid: u32) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() || metadata.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the instance directory is not owned by this user",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    #[cfg(unix)]
    use std::sync::mpsc;
    #[cfg(unix)]
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn the_message_codec_is_length_prefixed() {
        let request = LaunchRequest::new(Launch {
            path: Some(PathBuf::from("/tmp/a.md")),
            line: Some(4),
            new_window: true,
            cwd: PathBuf::from("/tmp"),
        });
        let mut bytes = Vec::new();
        write_message(&mut bytes, &request).unwrap();
        assert_eq!(u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize, bytes.len() - 4);
        let decoded: LaunchRequest = read_message(&mut Cursor::new(&bytes)).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn the_codec_rejects_bad_lengths_and_json() {
        let mut too_long = Vec::new();
        too_long.extend_from_slice(&((MAX_MESSAGE_BYTES + 1) as u32).to_be_bytes());
        assert!(matches!(
            read_message::<_, LaunchRequest>(&mut Cursor::new(&too_long)),
            Err(InstanceError::Protocol(_))
        ));

        let mut zero = Vec::new();
        zero.extend_from_slice(&0u32.to_be_bytes());
        assert!(matches!(read_message::<_, LaunchRequest>(&mut Cursor::new(&zero)), Err(InstanceError::Protocol(_))));

        let bad = b"not json";
        let mut invalid = Vec::new();
        invalid.extend_from_slice(&(bad.len() as u32).to_be_bytes());
        invalid.extend_from_slice(bad);
        assert!(matches!(
            read_message::<_, LaunchRequest>(&mut Cursor::new(&invalid)),
            Err(InstanceError::Protocol(_))
        ));

        let huge = vec![b'x'; MAX_MESSAGE_BYTES + 1];
        assert!(matches!(write_message(&mut Vec::new(), &huge), Err(InstanceError::Protocol(_))));
    }

    #[cfg(unix)]
    #[test]
    fn a_local_primary_forwards_and_acks() {
        let temp = TempDir::new().unwrap();
        let paths = InstancePaths::in_dir(temp.path().join("runtime"), "test");
        let (tx, rx) = mpsc::channel();
        let claim = InstanceServer::claim(paths.clone(), move |request| {
            tx.send(request).unwrap();
            LaunchAck::ok()
        })
        .unwrap();
        let Claim::Primary(server) = claim else {
            panic!("the first claim must be primary");
        };

        assert_eq!(fs::metadata(&paths.dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&paths.socket).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(fs::metadata(&paths.socket).unwrap().file_type().is_socket());

        let launch = Launch {
            path: Some(PathBuf::from("/tmp/a.md")),
            line: Some(7),
            new_window: true,
            cwd: PathBuf::from("/tmp"),
        };
        let ack = forward(&paths, &LaunchRequest::new(launch.clone())).unwrap();
        assert!(ack.ok);
        let received = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(received.launch, launch);

        let secondary = InstanceServer::claim(paths.clone(), |_| LaunchAck::ok());
        assert!(matches!(secondary, Ok(Claim::Secondary(_))), "a held lock means secondary");

        drop(server);
        assert!(!paths.socket.exists(), "the primary removes its socket");
        assert!(InstancePaths::in_dir(temp.path().join("runtime"), "test").lock.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_stale_socket_is_reclaimed_but_a_live_one_is_not() {
        let temp = TempDir::new().unwrap();
        let paths = InstancePaths::in_dir(temp.path().join("runtime"), "stale");
        fs::create_dir_all(&paths.dir).unwrap();
        fs::set_permissions(&paths.dir, fs::Permissions::from_mode(0o700)).unwrap();
        {
            let listener = UnixListener::bind(&paths.socket).unwrap();
            drop(listener);
        }
        assert!(paths.socket.exists());

        let claim = InstanceServer::claim(paths.clone(), |_| LaunchAck::ok()).unwrap();
        let Claim::Primary(server) = claim else {
            panic!("a stale socket without a lock owner must be reclaimed");
        };
        drop(server);
        assert!(!paths.socket.exists());

        // Recreate a live primary and make sure a second claim never unlinks it.
        let claim = InstanceServer::claim(paths.clone(), |_| LaunchAck::ok()).unwrap();
        let Claim::Primary(server) = claim else {
            panic!("a free lock must be primary");
        };
        let secondary = InstanceServer::claim(paths.clone(), |_| LaunchAck::ok());
        assert!(matches!(secondary, Ok(Claim::Secondary(_))));
        assert!(paths.socket.exists());
        drop(server);
    }

    #[cfg(unix)]
    #[test]
    fn a_foreign_object_at_the_socket_path_is_refused() {
        let temp = TempDir::new().unwrap();
        let paths = InstancePaths::in_dir(temp.path().join("runtime"), "unsafe");
        fs::create_dir_all(&paths.dir).unwrap();
        fs::set_permissions(&paths.dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&paths.socket, b"not a socket").unwrap();

        let claim = InstanceServer::claim(paths.clone(), |_| LaunchAck::ok());
        assert!(matches!(claim, Err(InstanceError::UnsafeSocket(_))));
        assert!(paths.socket.exists(), "the refused object is left alone");
    }

    #[test]
    fn instance_ids_are_sanitized_for_the_socket_name() {
        assert_eq!(socket_name("main"), "lmdv-main.sock");
        assert_eq!(socket_name("a/b c"), "lmdv-a-b-c.sock");
        assert_eq!(socket_name(""), "lmdv-main.sock");
        assert!(socket_name(&"x".repeat(200)).len() <= "lmdv-".len() + 64 + ".sock".len());
    }

    #[test]
    fn the_message_carries_the_client_cwd_and_activation_token() {
        let request = LaunchRequest {
            launch: Launch { path: Some(PathBuf::from("/tmp/a.md")), ..Launch::default() },
            activation_token: Some("token".into()),
            ..LaunchRequest::default()
        };
        let mut bytes = Vec::new();
        write_message(&mut bytes, &request).unwrap();
        let decoded: LaunchRequest = read_message(&mut Cursor::new(&bytes)).unwrap();
        assert_eq!(decoded.activation_token.as_deref(), Some("token"));
    }
}
