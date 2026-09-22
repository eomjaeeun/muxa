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
//! Schedules are in-memory only. A muxad restart drops them, same as it
//! would drop the shell loop this replaces; nothing here is durable.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

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
    paused: Arc<AtomicBool>,
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
}

impl KeepaliveStore {
    #[must_use]
    pub fn in_memory() -> Arc<Self> {
        Arc::new(Self {
            entries: RwLock::new(HashMap::new()),
        })
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
        let paused = Arc::new(AtomicBool::new(false));
        let started_at = OffsetDateTime::now_utc();
        let task_pane = pane.clone();
        let task_paused = paused.clone();
        let store = self.clone();
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
            // The first tick fires immediately; a keepalive loop's whole
            // point is "not yet, wait first" — burn that one silently.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if !task_paused.load(Ordering::SeqCst) {
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
                paused,
                task,
            },
        );
    }

    /// Stop and remove `pane`'s schedule. `true` if one existed.
    pub async fn stop(&self, pane: &str) -> bool {
        self.entries.write().await.remove(pane).is_some()
    }

    /// Pause `pane`'s schedule in place — the schedule stays listed, just
    /// skips ticks. `true` if a schedule exists for `pane` (whether or not
    /// it was already paused).
    pub async fn pause(&self, pane: &str) -> bool {
        let entries = self.entries.read().await;
        let Some(entry) = entries.get(pane) else {
            return false;
        };
        entry.paused.store(true, Ordering::SeqCst);
        true
    }

    /// Lift every pause. Called once when `watch` starts: the operator is
    /// back at the console, so anything held for their benefit resumes.
    pub async fn resume_all(&self) {
        let entries = self.entries.read().await;
        for entry in entries.values() {
            entry.paused.store(false, Ordering::SeqCst);
        }
    }

    pub async fn list(&self) -> Vec<KeepaliveInfo> {
        let entries = self.entries.read().await;
        let mut list: Vec<KeepaliveInfo> = entries
            .iter()
            .map(|(pane, entry)| KeepaliveInfo {
                pane: pane.clone(),
                interval_secs: entry.interval_secs,
                paused: entry.paused.load(Ordering::SeqCst),
                started_at: entry.started_at,
            })
            .collect();
        list.sort_by_key(|entry| entry.started_at);
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[tokio::test(start_paused = true)]
    async fn ticks_the_configured_number_of_times_and_skips_while_paused() {
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

        assert!(store.pause("%1").await);
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 1, "paused tick must not fire");

        store.resume_all().await;
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

    #[tokio::test]
    async fn pause_on_an_unknown_pane_reports_false() {
        let store = KeepaliveStore::in_memory();
        assert!(!store.pause("%nope").await);
    }
}
