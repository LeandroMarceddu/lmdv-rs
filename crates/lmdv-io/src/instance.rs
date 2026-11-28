//! The single-instance lock and transport.
//!
//! One process owns the instance and every later process hands it its launch.
//!
//! * Unix: the first process takes a `flock` on `<socket>.lock`, binds a stream
//!   socket (mode 0600 in a directory 0700) and forwards each launch to a
//!   callback. A later process connects and waits for an ack. `SO_PEERCRED`
//!   must match the effective uid.
//! * Windows: the first process owns a named mutex (`Local\lmdv-<id>.lock`) and
//!   serves a named pipe (`\\.\pipe\lmdv-<id>-<session>`). The mutex namespace
//!   is per session, and the pipe name carries the session id because the pipe
//!   namespace is machine-wide: a second session of the same account runs its
//!   own instance instead of connecting to a window on another desktop. The
//!   pipe instance is created with the process token's default DACL, so only
//!   the same user (and SYSTEM or an administrator) may connect, and remote
//!   clients are rejected.
//!
//! Both transports carry the same frames: a big-endian `u32` length followed by
//! UTF-8 JSON, capped at 1 MiB. The listener only forwards a
//! [`LaunchRequest`]; it never opens files.
//!
//! A failure to claim or forward is not fatal: the app logs it and starts
//! without single-instance routing.

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
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use fs2::FileExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING, PIPE_ACCESS_DUPLEX};
#[cfg(windows)]
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    PeekNamedPipe, WaitNamedPipeW,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::CreateMutexW;

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

/// The transport objects used by one instance id.
///
/// On Unix `dir` is the runtime folder, `socket` the socket path and `lock`
/// the `flock` file. On Windows the same fields carry Win32 object names
/// instead: `socket` is the named-pipe name, `lock` the named-mutex name, and
/// `dir` the app data folder (diagnostics only — no file is created there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstancePaths {
    /// The directory holding the Unix socket (mode 0700); the data folder on
    /// Windows.
    pub dir: PathBuf,
    /// `$XDG_RUNTIME_DIR/lmdv-<id>.sock`, a cache/tmp fallback, or
    /// `\\.\pipe\lmdv-<id>-<session>` on Windows.
    pub socket: PathBuf,
    /// The `flock` file, never removed; the named-mutex name on Windows.
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

    /// Resolves the named mutex and named pipe for an instance id.
    ///
    /// Nothing is created here: [`InstanceServer::claim`] creates the mutex and
    /// the first pipe instance, and [`forward`] opens the pipe.
    #[cfg(windows)]
    pub fn discover(dirs: &AppDirs) -> Result<Self, InstanceError> {
        Ok(Self {
            dir: dirs.data.clone(),
            socket: PathBuf::from(windows_pipe_name(&dirs.instance_id, session_id())),
            lock: PathBuf::from(windows_mutex_name(&dirs.instance_id)),
        })
    }

    /// Builds paths in an explicit directory. Used by tests and advanced
    /// callers; [`InstanceServer::claim`] still creates the directory 0700.
    ///
    /// This is the Unix layout; Windows callers use [`Self::discover`], whose
    /// fields are Win32 object names rather than paths.
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
    #[cfg(unix)]
    _lock: File,
    #[cfg(windows)]
    _mutex: OwnedHandle,
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

    /// Claims the single-instance role on Windows.
    ///
    /// A free named mutex makes this process primary: it creates the first
    /// pipe instance (so a name conflict is reported here, not on the listener
    /// thread) and serves launches from it. A mutex that already exists means
    /// another process owns the instance, and the caller forwards.
    #[cfg(windows)]
    pub fn claim<F>(paths: InstancePaths, on_launch: F) -> Result<Claim, InstanceError>
    where
        F: FnMut(LaunchRequest) -> LaunchAck + Send + 'static,
    {
        let mutex_name = wide(&paths.lock.to_string_lossy());
        // `CreateMutexW` reports “the object existed” through the thread's last
        // error, so read it immediately; failure is a null handle.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 1, mutex_name.as_ptr()) };
        let last_error = unsafe { GetLastError() };
        if handle.is_null() {
            return Err(InstanceError::Io(io::Error::from_raw_os_error(last_error as i32)));
        }
        // SAFETY: the handle came from `CreateMutexW` and is owned from here on.
        let mutex = unsafe { OwnedHandle::from_raw_handle(handle) };
        if last_error == ERROR_ALREADY_EXISTS {
            return Ok(Claim::Secondary(paths));
        }

        let pipe = paths.socket.to_string_lossy().into_owned();
        let first = create_pipe_instance(&pipe)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_name = format!("lmdv-instance-{pipe}");
        let thread = match thread::Builder::new()
            .name(thread_name)
            .spawn(move || serve_pipe(&pipe, first, thread_stop, on_launch))
        {
            Ok(thread) => thread,
            Err(error) => return Err(InstanceError::Io(error)),
        };

        Ok(Claim::Primary(Self { paths, _mutex: mutex, stop, thread: Some(thread) }))
    }

    /// The transport objects (socket and lock paths, or Win32 object names).
    pub fn paths(&self) -> &InstancePaths {
        &self.paths
    }
}

impl Drop for InstanceServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // A pipe server blocks in `ConnectNamedPipe`; a throwaway client
        // connection ends that wait so the listener can see the stop flag.
        #[cfg(windows)]
        if let Some(thread) = &self.thread {
            wake_pipe_server(&self.paths.socket, thread);
        }
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

/// Forwards a launch over the named pipe and waits [`ACK_TIMEOUT`] for an ack.
///
/// A primary that is still starting is waited for up to [`CONNECT_TIMEOUT`]:
/// a missing pipe is retried, and `ERROR_PIPE_BUSY` waits with
/// `WaitNamedPipeW` rather than spinning.
#[cfg(windows)]
pub fn forward(paths: &InstancePaths, request: &LaunchRequest) -> Result<LaunchAck, InstanceError> {
    let name = paths.socket.to_string_lossy();
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let file = loop {
        match open_pipe_client(&name) {
            Ok(file) => break file,
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                let now = Instant::now();
                if now >= deadline {
                    return Err(InstanceError::Timeout);
                }
                let wait = (deadline - now).as_millis().min(50) as u32;
                wait_named_pipe(&name, wait.max(1));
            }
            Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {
                if Instant::now() >= deadline {
                    return Err(InstanceError::Timeout);
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(InstanceError::Io(error)),
        }
    };

    let mut writer = &file;
    write_message(&mut writer, request).map_err(map_ack_timeout)?;
    let mut reader = PipeReader { file: &file, deadline: Instant::now() + ACK_TIMEOUT };
    let ack: LaunchAck = read_message(&mut reader).map_err(map_ack_timeout)?;
    if ack.v != PROTOCOL_VERSION {
        return Err(InstanceError::Version(ack.v));
    }
    Ok(ack)
}

/// A pipe buffer big enough for the launches lmdv sends (the protocol cap is
/// still [`MAX_MESSAGE_BYTES`]): one write or ack never blocks on a reader that
/// is a few microseconds behind.
#[cfg(windows)]
const PIPE_BUFFER_BYTES: u32 = 64 * 1024;

/// The NUL-terminated UTF-16 form the Win32 names take.
#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Creates one byte-mode duplex pipe instance with the process's default DACL.
#[cfg(windows)]
fn create_pipe_instance(name: &str) -> io::Result<File> {
    let name = wide(name);
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            0,
            std::ptr::null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a valid pipe handle is owned by the returned `File` from here on.
    Ok(unsafe { File::from_raw_handle(handle) })
}

/// Opens the primary's pipe; `ERROR_PIPE_BUSY` means an instance exists but is
/// serving another client.
#[cfg(windows)]
fn open_pipe_client(name: &str) -> io::Result<File> {
    let name = wide(name);
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a valid pipe handle is owned by the returned `File` from here on.
    Ok(unsafe { File::from_raw_handle(handle) })
}

#[cfg(windows)]
fn wait_named_pipe(name: &str, millis: u32) {
    let name = wide(name);
    unsafe { WaitNamedPipeW(name.as_ptr(), millis) };
}

/// Serves launches until `stop` is set.
///
/// The instance is created before the blocking `ConnectNamedPipe`, so a client
/// that arrives early queues instead of failing. Every iteration uses a fresh
/// instance: one client, one request, one ack.
#[cfg(windows)]
fn serve_pipe<F>(name: &str, first: File, stop: Arc<AtomicBool>, mut on_launch: F)
where
    F: FnMut(LaunchRequest) -> LaunchAck,
{
    let mut next = Some(first);
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let pipe = match next.take().map_or_else(|| create_pipe_instance(name), Ok) {
            Ok(pipe) => pipe,
            Err(_) => {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        let connected = unsafe { ConnectNamedPipe(pipe.as_raw_handle() as HANDLE, std::ptr::null_mut()) };
        if connected == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
                continue;
            }
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        handle_pipe_client(pipe, &mut on_launch);
    }
}

#[cfg(windows)]
fn handle_pipe_client<F>(pipe: File, on_launch: &mut F)
where
    F: FnMut(LaunchRequest) -> LaunchAck,
{
    let mut reader = PipeReader { file: &pipe, deadline: Instant::now() + ACK_TIMEOUT };
    let request = match read_message::<_, LaunchRequest>(&mut reader) {
        Ok(request) => request,
        Err(_) => return,
    };
    let mut writer = &pipe;
    if request.v != PROTOCOL_VERSION {
        let _ = write_message(&mut writer, &LaunchAck::failed(format!("unsupported version {}", request.v)));
        return;
    }
    let ack = on_launch(request);
    let _ = write_message(&mut writer, &ack);
}

/// A pipe reader that never blocks longer than its deadline.
///
/// `PeekNamedPipe` says whether anything is available, so a client that
/// connects and then stalls cannot pin the listener thread (the Unix socket
/// uses a read timeout for the same reason). Reads only happen when bytes are
/// already there, and a byte-mode pipe returns what it has.
#[cfg(windows)]
struct PipeReader<'a> {
    file: &'a File,
    deadline: Instant,
}

#[cfg(windows)]
impl Read for PipeReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut available = 0u32;
            let ok = unsafe {
                PeekNamedPipe(
                    self.file.as_raw_handle() as HANDLE,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let error = io::Error::last_os_error();
                return match error.raw_os_error() {
                    // A closed client is a clean end of stream, not an error.
                    Some(code)
                        if code == ERROR_BROKEN_PIPE as i32
                            || code == ERROR_PIPE_NOT_CONNECTED as i32
                            || code == ERROR_NO_DATA as i32 =>
                    {
                        Ok(0)
                    }
                    _ => Err(error),
                };
            }
            if available > 0 {
                let mut file = self.file;
                return file.read(out);
            }
            if Instant::now() >= self.deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the instance pipe did not answer"));
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
}

/// Connects once to end the server's `ConnectNamedPipe` wait on shutdown.
#[cfg(windows)]
fn wake_pipe_server(name: &std::path::Path, thread: &std::thread::JoinHandle<()>) {
    let name = name.to_string_lossy();
    let deadline = Instant::now() + Duration::from_millis(500);
    while !thread.is_finished() && Instant::now() < deadline {
        match open_pipe_client(&name) {
            Ok(file) => {
                drop(file);
                return;
            }
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                wait_named_pipe(&name, 10);
            }
            Err(_) => thread::sleep(Duration::from_millis(5)),
        }
    }
}

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

/// The sanitized instance id shared by every transport object name.
fn instance_stem(instance_id: &str) -> String {
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
    clean
}

fn socket_name(instance_id: &str) -> String {
    format!("lmdv-{}.sock", instance_stem(instance_id))
}

/// The named-pipe name a Windows primary serves.
///
/// The pipe namespace is machine-wide, so the current session id separates two
/// sessions of the same account; the mutex's `Local\` namespace already does
/// that for the claim itself.
#[cfg(any(windows, test))]
fn windows_pipe_name(instance_id: &str, session: u32) -> String {
    format!(r"\\.\pipe\lmdv-{}-{session}", instance_stem(instance_id))
}

/// The named-mutex name a Windows primary owns; `Local\` keeps it per session,
/// which matches the desktop app's scope.
#[cfg(any(windows, test))]
fn windows_mutex_name(instance_id: &str) -> String {
    format!(r"Local\lmdv-{}.lock", instance_stem(instance_id))
}

/// This process's terminal-services session id; 0 when Windows does not answer.
#[cfg(windows)]
fn session_id() -> u32 {
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let mut session = 0u32;
    // SAFETY: the process id is this process's and `session` is a valid out
    // pointer; the call has no other side effects.
    let answered = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    if answered == 0 { 0 } else { session }
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
    #[cfg(any(unix, windows))]
    use std::sync::mpsc;
    #[cfg(any(unix, windows))]
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn the_message_codec_is_length_prefixed() {
        let request = LaunchRequest::new(Launch {
            path: Some(PathBuf::from("/tmp/a.md")),
            line: Some(4),
            new_window: true,
            cwd: PathBuf::from("/tmp"),
            ..Launch::default()
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
            ..Launch::default()
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

    #[cfg(windows)]
    #[test]
    fn a_named_pipe_primary_forwards_and_acks() {
        let temp = TempDir::new().unwrap();
        let dirs = AppDirs {
            config: temp.path().join("config"),
            data: temp.path().join("data"),
            cache: temp.path().join("cache"),
            state: temp.path().join("state"),
            logs: temp.path().join("state/logs"),
            portable: false,
            instance_id: format!("test-{}", std::process::id()),
        };
        let paths = InstancePaths::discover(&dirs).unwrap();
        assert!(paths.socket.to_string_lossy().starts_with(r"\\.\pipe\lmdv-test-"));
        assert!(paths.lock.to_string_lossy().starts_with(r"Local\lmdv-test-"));

        let (tx, rx) = mpsc::channel();
        let claim = InstanceServer::claim(paths.clone(), move |request| {
            tx.send(request).unwrap();
            LaunchAck::ok()
        })
        .unwrap();
        let Claim::Primary(server) = claim else {
            panic!("the first claim must be primary");
        };

        let launch = Launch {
            path: Some(PathBuf::from(r"C:\notes\a.md")),
            line: Some(7),
            new_window: true,
            cwd: PathBuf::from(r"C:\notes"),
            ..Launch::default()
        };
        let ack = forward(&paths, &LaunchRequest::new(launch.clone())).unwrap();
        assert!(ack.ok);
        let received = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(received.launch, launch);

        let secondary = InstanceServer::claim(paths.clone(), |_| LaunchAck::ok());
        assert!(matches!(secondary, Ok(Claim::Secondary(_))), "a held mutex means secondary");

        drop(server);
    }

    #[test]
    fn instance_ids_are_sanitized_for_the_socket_name() {
        assert_eq!(socket_name("main"), "lmdv-main.sock");
        assert_eq!(socket_name("a/b c"), "lmdv-a-b-c.sock");
        assert_eq!(socket_name(""), "lmdv-main.sock");
        assert!(socket_name(&"x".repeat(200)).len() <= "lmdv-".len() + 64 + ".sock".len());
    }

    #[test]
    fn the_windows_objects_are_named_per_instance_and_sanitized() {
        assert_eq!(windows_pipe_name("main", 1), r"\\.\pipe\lmdv-main-1");
        assert_eq!(windows_mutex_name("main"), r"Local\lmdv-main.lock");
        assert_eq!(windows_pipe_name("a/b c", 2), r"\\.\pipe\lmdv-a-b-c-2");
        assert_eq!(windows_mutex_name("a/b c"), r"Local\lmdv-a-b-c.lock");
        assert_eq!(windows_pipe_name("", 0), r"\\.\pipe\lmdv-main-0", "an empty id is the main instance");
        assert_ne!(windows_pipe_name("portable-0123456789ab", 1), windows_pipe_name("main", 1));
        assert_ne!(
            windows_pipe_name("main", 1),
            windows_pipe_name("main", 2),
            "a second session must not reach the first session's window"
        );
        assert!(
            windows_pipe_name(&"x".repeat(200), 4294967295).len() <= r"\\.\pipe\lmdv-".len() + 64 + 11,
            "the sanitized id and the session id are bounded"
        );
        assert!(
            windows_mutex_name(&"x".repeat(200)).len() <= r"Local\lmdv-".len() + 64 + ".lock".len(),
            "the sanitized id is capped"
        );
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
