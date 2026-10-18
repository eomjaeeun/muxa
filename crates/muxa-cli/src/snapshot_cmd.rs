//! `muxa snapshot` — save and restore tmux layouts and tracked agents.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: SnapshotCommand,
}

#[derive(Debug, clap::Subcommand)]
enum SnapshotCommand {
    /// Save the current tmux session/window/pane layout and tracked agents to a JSON file.
    Save { file: PathBuf },
    /// Recreate tmux sessions/windows/panes and resume agents from a saved snapshot.
    Restore { file: PathBuf },
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Snapshot {
    sessions: Vec<SnapshotSession>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SnapshotSession {
    name: String,
    windows: Vec<SnapshotWindow>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SnapshotWindow {
    name: String,
    layout: String,
    panes: Vec<SnapshotPane>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SnapshotPane {
    index: u32,
    session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "SnapshotCwd::is_missing")]
    cwd: SnapshotCwd,
    container: Option<String>,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
#[serde(untagged)]
enum SnapshotCwd {
    #[default]
    Missing,
    Value(String),
    Null,
}

impl<'de> Deserialize<'de> for SnapshotCwd {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Option::<String>::deserialize(deserializer)?.map_or(Self::Null, Self::Value))
    }
}

impl SnapshotCwd {
    fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    fn as_deref(&self) -> Option<&str> {
        match self {
            Self::Value(value) => Some(value),
            Self::Missing | Self::Null => None,
        }
    }
}

pub(crate) async fn run(args: Args, socket: PathBuf) -> Result<()> {
    match args.action {
        SnapshotCommand::Save { file } => save(&file, socket).await,
        SnapshotCommand::Restore { file } => restore(&file).await,
    }
}

async fn save(path: &Path, socket: PathBuf) -> Result<()> {
    let panes = muxa::tmux::list_panes().context("listing tmux panes")?;
    let layouts = read_layouts()?;
    let agents = muxa::ipc::Client::new(socket)
        .snapshot()
        .await
        .context("reading tracked agents from muxad")?;
    let snapshot = assemble_snapshot(&panes, &agents, &layouts);

    let json = serde_json::to_string_pretty(&snapshot).context("serializing snapshot")?;
    std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
    let pane_count = snapshot
        .sessions
        .iter()
        .flat_map(|s| &s.windows)
        .map(|w| w.panes.len())
        .sum::<usize>();
    let agent_count = snapshot
        .sessions
        .iter()
        .flat_map(|s| &s.windows)
        .flat_map(|w| &w.panes)
        .filter(|p| p.session_id.is_some())
        .count();
    println!(
        "saved {pane_count} pane(s) ({agent_count} with an agent) across {} session(s) -> {}",
        snapshot.sessions.len(),
        path.display()
    );
    Ok(())
}

fn assemble_snapshot(
    panes: &[muxa::tmux::PaneInfo],
    agents: &[muxa::state::Agent],
    layouts: &HashMap<(String, String), String>,
) -> Snapshot {
    let agents_by_pane: HashMap<&str, &muxa::state::Agent> = agents
        .iter()
        .filter_map(|agent| agent.pane.as_deref().map(|pane| (pane, agent)))
        .collect();

    let mut session_order = Vec::new();
    let mut grouped: HashMap<String, HashMap<String, Vec<&muxa::tmux::PaneInfo>>> = HashMap::new();
    for pane in panes {
        if !grouped.contains_key(&pane.session) {
            session_order.push(pane.session.clone());
        }
        grouped
            .entry(pane.session.clone())
            .or_default()
            .entry(pane.window_index.clone())
            .or_default()
            .push(pane);
    }

    let mut snapshot = Snapshot {
        sessions: Vec::new(),
    };
    for session_name in session_order {
        let windows = grouped.remove(&session_name).unwrap_or_default();
        let mut window_indices: Vec<String> = windows.keys().cloned().collect();
        window_indices.sort_by_key(|index| index.parse::<u32>().unwrap_or(u32::MAX));
        let mut snapshot_windows = Vec::new();
        for window_index in window_indices {
            let mut window_panes = windows.get(&window_index).cloned().unwrap_or_default();
            window_panes.sort_by_key(|pane| pane.pane_index.parse::<u32>().unwrap_or(u32::MAX));
            let name = window_panes
                .first()
                .map_or_else(String::new, |pane| pane.window_name.clone());
            let panes = window_panes
                .into_iter()
                .map(|pane| {
                    let index = pane.pane_index.parse::<u32>().unwrap_or_default();
                    let container = muxa::process_tree::docker_container_for_pid(pane.pane_pid);
                    match agents_by_pane
                        .get(pane.pane_id.as_str())
                        .filter(|agent| !agent.session_id.is_empty())
                    {
                        Some(agent) => SnapshotPane {
                            index,
                            session_id: Some(agent.session_id.clone()),
                            kind: Some(agent.kind.to_string()),
                            title: Some(pane.title.replace('✳', "").trim().to_string()),
                            cwd: if container.is_some() {
                                SnapshotCwd::Null
                            } else {
                                SnapshotCwd::Value(pane.current_path.clone())
                            },
                            container,
                        },
                        None => SnapshotPane {
                            index,
                            session_id: None,
                            kind: None,
                            title: None,
                            cwd: SnapshotCwd::Missing,
                            container,
                        },
                    }
                })
                .collect();
            snapshot_windows.push(SnapshotWindow {
                name,
                layout: layouts
                    .get(&(session_name.clone(), window_index))
                    .cloned()
                    .unwrap_or_default(),
                panes,
            });
        }
        snapshot.sessions.push(SnapshotSession {
            name: session_name,
            windows: snapshot_windows,
        });
    }

    snapshot
}

fn read_layouts() -> Result<HashMap<(String, String), String>> {
    let output = muxa::tmux::capture_control_on(
        None,
        &[
            "list-windows",
            "-a",
            "-F",
            "#{session_name}\t#{window_index}\t#{window_layout}",
        ],
    )
    .context("listing tmux window layouts")?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            Some((
                (fields.next()?.into(), fields.next()?.into()),
                fields.next()?.into(),
            ))
        })
        .collect())
}

async fn restore(path: &Path) -> Result<()> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let snapshot: Snapshot =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let existing: HashSet<String> = muxa::tmux::list_sessions()
        .context("listing tmux sessions")?
        .into_iter()
        .map(|s| s.name)
        .collect();

    for session in &snapshot.sessions {
        let reused = existing.contains(&session.name);
        if reused {
            println!("session '{}' already exists — reusing it", session.name);
        } else {
            control(&["new-session", "-d", "-s", &session.name])?;
        }
        for (window_number, window) in session.windows.iter().enumerate() {
            if window_number == 0 && !reused {
                control(&["rename-window", "-t", &session.name, &window.name])?;
            } else {
                control(&["new-window", "-t", &session.name, "-n", &window.name])?;
            }
            let target = format!("{}:{}", session.name, window.name);
            for _ in 0..window.panes.len().saturating_sub(1) {
                control(&["split-window", "-h", "-t", &target])?;
            }
            if !window.layout.is_empty() {
                control(&["select-layout", "-t", &target, &window.layout])?;
            }
            let output = muxa::tmux::capture_control_on(
                None,
                &[
                    "list-panes",
                    "-t",
                    &target,
                    "-F",
                    "#{pane_index} #{pane_id}",
                ],
            )?;
            let pane_ids: HashMap<u32, &str> = output
                .lines()
                .filter_map(|line| {
                    let (index, id) = line.split_once(' ')?;
                    Some((index.parse().ok()?, id))
                })
                .collect();
            let mut panes: Vec<&SnapshotPane> = window.panes.iter().collect();
            panes.sort_by_key(|pane| pane.index);
            for pane in panes {
                if let Some(pane_id) = pane_ids.get(&pane.index) {
                    populate_pane(pane_id, pane).await?;
                }
            }
        }
    }
    Ok(())
}

fn control(args: &[&str]) -> Result<()> {
    muxa::tmux::run_control_on(None, args).with_context(|| format!("tmux {}", args.join(" ")))
}

fn send(pane_id: &str, text: &str) -> Result<()> {
    if !muxa::tmux::send_text_on(None, pane_id, text)
        || !muxa::tmux::send_text_on(None, pane_id, "\r")
    {
        bail!("failed to send command to tmux pane {pane_id}");
    }
    Ok(())
}

async fn populate_pane(pane_id: &str, pane: &SnapshotPane) -> Result<()> {
    // These heuristic waits preserve the proven startup sequencing of the old script.
    if let Some(container) = &pane.container {
        let entry = match container.as_str() {
            "je-dev" => "jedev".to_string(),
            "je-new" => "jenew".to_string(),
            _ => format!("docker exec -it {container} zsh"),
        };
        send(pane_id, &entry)?;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    if let Some(cwd) = pane.cwd.as_deref() {
        send(pane_id, &format!("cd {cwd}"))?;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let Some(session_id) = pane.session_id.as_deref() else {
        return Ok(());
    };
    send(pane_id, &format!("claude --resume {session_id}"))?;
    tokio::time::sleep(Duration::from_secs(4)).await;
    let captured = muxa::tmux::capture_pane_on(None, pane_id)?;
    if captured.contains("trust this folder") || captured.contains("Yes, I trust") {
        control(&["send-keys", "-t", pane_id, "Down"])?;
        control(&["send-keys", "-t", pane_id, "Enter"])?;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let short = |length| session_id.chars().take(length).collect::<String>();
    let label = pane
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .map_or_else(|| short(8), str::to_string);
    println!("  {pane_id}: resumed {label} ({})", short(12));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_json_round_trip_preserves_python_schema() {
        let snapshot = Snapshot {
            sessions: vec![SnapshotSession {
                name: "work".into(),
                windows: vec![SnapshotWindow {
                    name: "main".into(),
                    layout: "layout".into(),
                    panes: vec![
                        SnapshotPane {
                            index: 0,
                            session_id: Some("abc".into()),
                            kind: Some("claude_code".into()),
                            title: Some("agent".into()),
                            cwd: SnapshotCwd::Value("/work".into()),
                            container: None,
                        },
                        SnapshotPane {
                            index: 1,
                            session_id: None,
                            kind: None,
                            title: None,
                            cwd: SnapshotCwd::Missing,
                            container: None,
                        },
                        SnapshotPane {
                            index: 2,
                            session_id: Some("def".into()),
                            kind: Some("claude_code".into()),
                            title: Some("container".into()),
                            cwd: SnapshotCwd::Null,
                            container: Some("je-dev".into()),
                        },
                    ],
                }],
            }],
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        let panes = value["sessions"][0]["windows"][0]["panes"]
            .as_array()
            .unwrap();
        assert_eq!(panes[0]["cwd"], "/work");
        assert!(!panes[1].as_object().unwrap().contains_key("kind"));
        assert!(!panes[1].as_object().unwrap().contains_key("title"));
        assert!(!panes[1].as_object().unwrap().contains_key("cwd"));
        assert!(panes[2].as_object().unwrap().contains_key("cwd"));
        assert!(panes[2]["cwd"].is_null());
        assert_eq!(serde_json::from_value::<Snapshot>(value).unwrap(), snapshot);
    }
}
