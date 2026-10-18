//! `muxa keepalive` — periodic Enter keystrokes into one pane, started and
//! stopped by the operator rather than by any state transition.
//!
//! This is the daemon-owned form of `while true; do tmux send-keys -t
//! <pane> "" Enter; sleep N; done`. It has to live in muxad rather than
//! inside `muxa watch` because jumping into a pane quits `watch`'s own
//! process outright (see `watch.rs`'s `AttachTopologyPane`/`AttachPane`) —
//! a loop owned by the TUI would die with it. muxad outlives every
//! `watch` launch, so a schedule keeps running (or stays correctly
//! paused) across them.
//!
//! Deliberately not `automation`: automation is event-driven with no
//! notion of a plain interval, and its guardrails (cooldown, hourly caps,
//! one-firing-per-episode) exist to bound a rule nobody is watching fire.
//! A keepalive schedule is the opposite — the operator started it on
//! purpose, for exactly this pane, and stops it on purpose too.
//!
//! Daemon-owned schedules are persisted to the XDG data directory and
//! restored when muxad starts. Focus state remains live tmux-derived state
//! and is deliberately never persisted.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

const FOCUS_POLL_INTERVAL: Duration = Duration::from_secs(1);
const PERSIST_DEBOUNCE: Duration = Duration::from_millis(200);
pub const KEEPALIVE_SCHEMA_VERSION: u32 = 1;

#[cfg(unix)]
const KEEPALIVE_FILE_MODE: u32 = 0o600;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedKeepalive {
    pub pane: String,
    pub interval_secs: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeepaliveFile {
    #[serde(default)]
    v: u32,
    #[serde(with = "time::serde::rfc3339")]
    saved_at: OffsetDateTime,
    schedules: Vec<PersistedKeepalive>,
}

pub async fn load(path: &Path) -> Vec<PersistedKeepalive> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            debug!(path = %path.display(), "no keepalive file on disk; starting empty");
            return Vec::new();
        }
        Err(error) => {
            warn!(%error, path = %path.display(), "could not read keepalive file");
            return Vec::new();
        }
    };
    let parsed: KeepaliveFile = match serde_json::from_slice(&bytes) {
        Ok(parsed) => parsed,
        Err(error) => {
            warn!(%error, path = %path.display(), "keepalive file is corrupt; starting empty");
            return Vec::new();
        }
    };
    if parsed.v != KEEPALIVE_SCHEMA_VERSION {
        warn!(file_version = parsed.v, expected = KEEPALIVE_SCHEMA_VERSION,
            path = %path.display(), "keepalive file has unknown schema version; starting empty");
        return Vec::new();
    }
    parsed.schedules
}

pub async fn save(path: &Path, records: &[PersistedKeepalive]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
    })?;
    if !parent.as_os_str().is_empty() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let basename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("keepalive.json");
    let tmp = parent.join(format!(".{basename}.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec(&KeepaliveFile {
        v: KEEPALIVE_SCHEMA_VERSION,
        saved_at: OffsetDateTime::now_utc(),
        schedules: records.to_vec(),
    })
    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    {
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        options.mode(KEEPALIVE_FILE_MODE);
        let mut file = options.open(&tmp).await?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
    }
    tokio::fs::rename(&tmp, path).await?;
    if !parent.as_os_str().is_empty() {
        if let Ok(dir) = OpenOptions::new().read(true).open(parent).await {
            let _ = dir.sync_all().await;
        }
    }
    Ok(())
}

/// One schedule as reported to a client — the live fields, not the task
/// handle backing it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeepaliveInfo {
    pub pane: String,
    pub interval_secs: u64,
    pub paused: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
}

struct Entry {
    interval_secs: u64,
    started_at: OffsetDateTime,
    task: JoinHandle<()>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Live keepalive schedules, keyed by pane id. One entry per pane: starting
/// a second schedule on a pane already running one replaces it outright
/// rather than stacking two loops on the same target.
pub struct KeepaliveStore {
    entries: RwLock<HashMap<String, Entry>>,
    focused_panes: Arc<RwLock<HashSet<String>>>,
    focus_task: Option<JoinHandle<()>>,
    persistence_path: Option<PathBuf>,
    dirty: Arc<Notify>,
    persistence_task: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Drop for KeepaliveStore {
    fn drop(&mut self) {
        if let Some(task) = &self.focus_task {
            task.abort();
        }
        if let Ok(Some(task)) = self.persistence_task.get_mut() {
            task.abort();
        }
    }
}

impl KeepaliveStore {
    #[must_use]
    pub fn in_memory() -> Arc<Self> {
        Arc::new(Self {
            entries: RwLock::new(HashMap::new()),
            focused_panes: Arc::new(RwLock::new(HashSet::new())),
            focus_task: None,
            persistence_path: None,
            dirty: Arc::new(Notify::new()),
            persistence_task: std::sync::Mutex::new(None),
        })
    }

    /// Create the daemon store and continuously track panes displayed by real
    /// (non-control-mode) tmux clients.
    #[must_use]
    pub fn with_tmux_focus_tracking(persistence_path: Option<PathBuf>) -> Arc<Self> {
        let focused_panes = Arc::new(RwLock::new(HashSet::new()));
        let task_focused_panes = Arc::clone(&focused_panes);
        let focus_task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(FOCUS_POLL_INTERVAL);
            loop {
                ticker.tick().await;
                match tokio::task::spawn_blocking(crate::tmux::focused_panes).await {
                    Ok(Ok(sample)) => *task_focused_panes.write().await = sample,
                    Ok(Err(error)) => warn!(%error, "could not poll tmux client focus"),
                    Err(error) => warn!(%error, "tmux focus poll task failed"),
                }
            }
        });
        let store = Arc::new(Self {
            entries: RwLock::new(HashMap::new()),
            focused_panes,
            focus_task: Some(focus_task),
            persistence_path: persistence_path.clone(),
            dirty: Arc::new(Notify::new()),
            persistence_task: std::sync::Mutex::new(None),
        });
        if let Some(path) = persistence_path {
            let weak_store = Arc::downgrade(&store);
            let dirty = Arc::clone(&store.dirty);
            let task = tokio::spawn(async move {
                loop {
                    dirty.notified().await;
                    tokio::time::sleep(PERSIST_DEBOUNCE).await;
                    let Some(store) = weak_store.upgrade() else {
                        return;
                    };
                    let records = store.persisted_records().await;
                    if let Err(error) = save(&path, &records).await {
                        warn!(%error, path = %path.display(), "could not save keepalive schedules");
                    }
                }
            });
            *store
                .persistence_task
                .lock()
                .expect("keepalive persistence task lock") = Some(task);
        }
        store
    }

    /// Start a schedule for `pane`, replacing any schedule already running
    /// on it. `tick` fires once per `interval`, skipped while paused; it is
    /// the caller's job to make `tick` actually inject the keystroke —
    /// this store only owns the timer and the pause flag; it does not
    /// know how to reach a pane's backend.
    pub async fn start<F, Fut>(self: &Arc<Self>, pane: String, interval: Duration, tick: F)
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let interval_secs = interval.as_secs().max(1);
        let started_at = OffsetDateTime::now_utc();
        let task_pane = pane.clone();
        let store = self.clone();
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
            // The first tick fires immediately; a keepalive loop's whole
            // point is "not yet, wait first" — burn that one silently.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if !store.focused_panes.read().await.contains(&task_pane) {
                    tick(task_pane.clone()).await;
                }
                // The entry (and with it, this task's own membership) may
                // have been removed by `stop` between ticks. Nothing left
                // to check against — the abort in `Drop` handles the
                // ordinary shutdown path; this only guards the rare race
                // where a tick was already in flight.
                if !store.entries.read().await.contains_key(&task_pane) {
                    return;
                }
            }
        });
        let mut entries = self.entries.write().await;
        entries.insert(
            pane,
            Entry {
                interval_secs,
                started_at,
                task,
            },
        );
        drop(entries);
        self.mark_dirty();
    }

    /// Stop and remove `pane`'s schedule. `true` if one existed.
    pub async fn stop(&self, pane: &str) -> bool {
        let removed = self.entries.write().await.remove(pane).is_some();
        if removed {
            self.mark_dirty();
        }
        removed
    }

    /// Immediately treat `panes` as focused. The next tmux focus sample
    /// replaces this seed with the current attached-client state.
    pub async fn mark_focused(&self, panes: impl IntoIterator<Item = String>) {
        self.focused_panes.write().await.extend(panes);
    }

    pub async fn list(&self) -> Vec<KeepaliveInfo> {
        let entries = self.entries.read().await;
        let focused_panes = self.focused_panes.read().await;
        let mut list: Vec<KeepaliveInfo> = entries
            .iter()
            .map(|(pane, entry)| KeepaliveInfo {
                pane: pane.clone(),
                interval_secs: entry.interval_secs,
                paused: focused_panes.contains(pane),
                started_at: entry.started_at,
            })
            .collect();
        list.sort_by_key(|entry| entry.started_at);
        list
    }

    fn mark_dirty(&self) {
        if self.persistence_path.is_some() {
            self.dirty.notify_one();
        }
    }

    async fn persisted_records(&self) -> Vec<PersistedKeepalive> {
        self.entries
            .read()
            .await
            .iter()
            .map(|(pane, entry)| PersistedKeepalive {
                pane: pane.clone(),
                interval_secs: entry.interval_secs,
                started_at: entry.started_at,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn record(pane: &str, interval_secs: u64) -> PersistedKeepalive {
        PersistedKeepalive {
            pane: pane.into(),
            interval_secs,
            started_at: OffsetDateTime::now_utc(),
        }
    }

    #[tokio::test]
    async fn persistence_round_trips_and_bad_inputs_start_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keepalive.json");
        let expected = record("%7", 42);
        save(&path, std::slice::from_ref(&expected)).await.unwrap();
        let loaded = load(&path).await;
        assert_eq!(loaded[0].pane, expected.pane);
        assert_eq!(loaded[0].interval_secs, expected.interval_secs);

        let missing = dir.path().join("missing.json");
        assert!(load(&missing).await.is_empty());
        tokio::fs::write(&path, b"not json").await.unwrap();
        assert!(load(&path).await.is_empty());
        tokio::fs::write(
            &path,
            br#"{"v":999,"saved_at":"2025-01-01T00:00:00Z","schedules":[]}"#,
        )
        .await
        .unwrap();
        assert!(load(&path).await.is_empty());
    }

    #[tokio::test]
    async fn store_start_is_persisted_after_debounce() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keepalive.json");
        let store = KeepaliveStore::with_tmux_focus_tracking(Some(path.clone()));
        store
            .start("%8".into(), Duration::from_secs(17), |_| async {})
            .await;
        tokio::time::sleep(Duration::from_millis(350)).await;
        let loaded = load(&path).await;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].pane, "%8");
        assert_eq!(loaded[0].interval_secs, 17);
    }

    #[tokio::test(start_paused = true)]
    async fn ticks_the_configured_number_of_times_and_skips_while_focused() {
        let store = KeepaliveStore::in_memory();
        let count = Arc::new(AtomicU32::new(0));
        let counting = count.clone();
        store
            .start("%1".into(), Duration::from_secs(5), move |_pane| {
                let counting = counting.clone();
                async move {
                    counting.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;

        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 0, "first tick is the wait");

        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 1);

        store.mark_focused(["%1".into()]).await;
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 1, "paused tick must not fire");

        store.focused_panes.write().await.clear();
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn stopping_removes_it_from_the_list_and_starting_replaces_not_stacks() {
        let store = KeepaliveStore::in_memory();
        store
            .start("%1".into(), Duration::from_secs(5), |_| async {})
            .await;
        assert_eq!(store.list().await.len(), 1);

        // Starting again on the same pane replaces, not adds a second loop.
        store
            .start("%1".into(), Duration::from_secs(10), |_| async {})
            .await;
        let list = store.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].interval_secs, 10);

        assert!(store.stop("%1").await);
        assert!(store.list().await.is_empty());
        assert!(!store.stop("%1").await, "already gone");
    }
}
