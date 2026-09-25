//! XDG-aware default paths.

use std::path::PathBuf;

pub const SOCKET_FILENAME: &str = "muxa.sock";
pub const CONFIG_DIRNAME: &str = "muxa";
pub const CONFIG_FILENAME: &str = "config.toml";
pub const HISTORY_FILENAME: &str = "prompts.ndjson";
pub const ACTIVITY_FILENAME: &str = "activity.ndjson";
pub const STATE_FILENAME: &str = "state.json";
pub const SESSION_ACTIVITY_FILENAME: &str = "session-activity.json";
pub const COLLABORATION_FILENAME: &str = "collaboration.json";
pub const COLLABORATION_AUDIT_FILENAME: &str = "collaboration-audit.ndjson";
pub const ASK_FILENAME: &str = "ask.json";
pub const NODE_ID_FILENAME: &str = "host-id";
pub const DASHBOARD_WORK_FILENAME: &str = "dashboard-work.json";
pub const PIPELINE_RUN_FILENAME: &str = "pipeline-runs.json";
pub const WATCH_READ_FILENAME: &str = "watch-read.json";
pub const WATCH_TAB_CHOICE_FILENAME: &str = "watch-tab-choice.json";
pub const WATCH_MEMO_FILENAME: &str = "watch-memo.json";
pub const WATCH_MEMO_PANEL_FILENAME: &str = "watch-memo-panel.json";

/// Default daemon socket path. Prefers `$XDG_RUNTIME_DIR/muxa.sock`; falls
/// back to `/tmp/muxa-<uid>.sock` when the runtime dir is unset.
pub fn default_socket() -> PathBuf {
    if let Some(dir) = dirs::runtime_dir() {
        return dir.join(SOCKET_FILENAME);
    }
    PathBuf::from(format!("/tmp/muxa-{}.sock", posix_uid()))
}

/// Default config file path: `$XDG_CONFIG_HOME/muxa/config.toml`, falling
/// back to `$HOME/.config/muxa/config.toml`.
pub fn default_config_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(CONFIG_DIRNAME).join(CONFIG_FILENAME))
}

/// Default prompt-history file: `$XDG_DATA_HOME/muxa/prompts.ndjson`,
/// falling back to `$HOME/.local/share/muxa/prompts.ndjson`. Lives under
/// the data dir (not state/config) because prompts are user content the
/// operator may want to back up or grep through.
pub fn default_history_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(HISTORY_FILENAME))
}

/// Default activity ledger file: `$XDG_DATA_HOME/muxa/activity.ndjson`.
/// Stores closed duration intervals for agent states and tmux session
/// foreground time.
pub fn default_activity_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(ACTIVITY_FILENAME))
}

/// Default agent-registry snapshot file: `$XDG_DATA_HOME/muxa/state.json`,
/// falling back to `$HOME/.local/share/muxa/state.json`. Co-located with
/// the prompt history so a single backup or rotation policy covers both.
pub fn default_state_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(STATE_FILENAME))
}

/// Where `muxa watch` remembers which agents the operator has already read:
/// `$XDG_DATA_HOME/muxa/watch-read.json`.
///
/// Data, not config: it is a per-operator reading position that changes many
/// times an hour, so it must not churn `config.toml` the way the persisted
/// sort and split do. It is also deliberately not daemon state — two people
/// watching the same host have different unread sets.
pub fn default_watch_read_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(WATCH_READ_FILENAME))
}

/// Where `muxa watch` remembers which pane each window row's `Tab` points
/// at: `$XDG_DATA_HOME/muxa/watch-tab-choice.json`.
///
/// Has to survive a restart, not just a refresh: jumping into any pane
/// (`Enter`/`p`/`m`) quits `watch` outright, so an in-memory-only map would
/// reset every window's choice back to its default the moment the operator
/// checked on any one of them — not just the one they jumped to.
pub fn default_watch_tab_choice_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(WATCH_TAB_CHOICE_FILENAME))
}

/// Where `muxa watch`'s `Ctrl-N` scratch memo lives:
/// `$XDG_DATA_HOME/muxa/watch-memo.json`.
///
/// Data, not config, same reasoning as `default_watch_read_file`: it's a
/// per-operator scratchpad that changes on every keystroke, not something
/// that belongs in `config.toml`.
pub fn default_watch_memo_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(WATCH_MEMO_FILENAME))
}

/// Where `muxa watch` remembers whether the scratch memo panel was open
/// (and focused) when the operator last left it:
/// `$XDG_DATA_HOME/muxa/watch-memo-panel.json`.
///
/// Has to survive a restart for the same reason `default_watch_tab_choice_file`
/// does: jumping into any pane quits `watch` outright, so an in-memory-only
/// flag would reset the panel to closed every time the operator came back.
pub fn default_watch_memo_panel_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(WATCH_MEMO_PANEL_FILENAME))
}

/// Stable physical-node identity used by Muxa Fleet. It intentionally lives
/// in the data directory rather than config: SSH aliases, host names, labels,
/// and even the config file can change without creating a different node.
pub fn default_node_id_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(NODE_ID_FILENAME))
}

/// Default tmux session activity file: `$XDG_DATA_HOME/muxa/session-activity.json`.
/// Co-located with the daemon's other user-state files so backups and
/// cleanup policies stay simple.
pub fn default_session_activity_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(SESSION_ACTIVITY_FILENAME))
}

/// Default durable collaboration mailbox snapshot.
/// Ask history: `$XDG_DATA_HOME/muxa/ask.json`, beside the
/// collaboration mailbox it mirrors.
pub fn default_ask_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(ASK_FILENAME))
}

pub fn default_collaboration_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(COLLABORATION_FILENAME))
}

/// Default append-only collaboration caller audit ledger.
pub fn default_collaboration_audit_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(COLLABORATION_AUDIT_FILENAME))
}

/// Durable logical Work records. Execution bindings remain owned by pane
/// backends; this file stores operator metadata and optional external issue
/// references keyed by `{workspace_id, work_id}`.
pub fn default_dashboard_work_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(DASHBOARD_WORK_FILENAME))
}

/// Durable desired graph and generation-aware state for Work pipeline Runs.
pub fn default_pipeline_run_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_DIRNAME).join(PIPELINE_RUN_FILENAME))
}

fn posix_uid() -> u32 {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}
