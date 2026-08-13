//! Debounced file watching (§9.4). inotify watches the inode and editors
//! save atomically — write a temp file, rename over the target — so a watch
//! on the file itself goes permanently deaf after the first save. Watch the
//! containing directory, filter by filename, debounce (one save emits 3–5
//! inotify events). The same trap applies to the Omarchy theme directory.

use notify_debouncer_full::notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub type DirWatcher = notify_debouncer_full::Debouncer<
    notify_debouncer_full::notify::RecommendedWatcher,
    notify_debouncer_full::RecommendedCache,
>;

/// Watch `dir` (non-recursive, debounced 300 ms); affected paths stream to
/// the receiver. Dropping the returned debouncer stops the watch and closes
/// the channel — keep it alive for as long as the watch should run.
pub fn watch_dir(dir: &Path) -> Option<(DirWatcher, async_channel::Receiver<Vec<PathBuf>>)> {
    let (tx, rx) = async_channel::unbounded();
    let mut debouncer = new_debouncer(
        Duration::from_millis(300),
        None,
        move |result: DebounceEventResult| {
            if let Ok(events) = result {
                let paths: Vec<PathBuf> =
                    events.into_iter().flat_map(|e| e.event.paths.clone()).collect();
                if !paths.is_empty() {
                    let _ = tx.send_blocking(paths);
                }
            }
        },
    )
    .ok()?;
    debouncer.watch(dir, RecursiveMode::NonRecursive).ok()?;
    Some((debouncer, rx))
}
