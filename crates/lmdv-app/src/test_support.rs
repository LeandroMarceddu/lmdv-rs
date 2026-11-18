//! Test-only constructors shared by the crate's unit tests.

use std::rc::Rc;

use lmdv_io::AppDirs;
use tempfile::TempDir;

use crate::shared::AppShared;

/// An `AppDirs` rooted entirely inside `temp`.
pub fn dirs(temp: &TempDir) -> AppDirs {
    let root = temp.path().join("lmdv-state");
    AppDirs {
        config: root.join("config"),
        data: root.clone(),
        cache: root.join("cache"),
        state: root.join("state"),
        logs: root.join("state").join("logs"),
        portable: false,
        instance_id: "test".to_owned(),
    }
}

/// A shared state with its own store writer and empty recents.
pub fn shared(temp: &TempDir) -> Rc<AppShared> {
    AppShared::start(dirs(temp))
}
