//! Session persistence: remember what each pane was running (its directory and
//! its launch — a plain shell or a named agent) so a restart can bring the
//! layout back. tmux-style: every pane is respawned *fresh* in its last
//! directory; nothing about the live terminal contents is captured.
//!
//! The file is a small TOML document at the platform config dir
//! (`~/.config/tuiterminals/session.toml` on Unix,
//! `%APPDATA%\tuiterminals\session.toml` on Windows). Agents are stored **by
//! name** (`agent:<name>`) rather than by index, so reordering the
//! `[[agents]]` list in the config never mis-resolves a saved pane.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app::{App, Launch};
use crate::config::Config;

/// The session file path: the platform config dir, mirroring the config file's
/// own location (see `config_path_from_args` in `main.rs`).
pub fn session_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("tuiterminals").join("session.toml"))
        .unwrap_or_else(|| PathBuf::from("session.toml"))
}

/// A saved session: one entry per pane, in pane order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub panes: Vec<SessionPane>,
}

/// One saved pane. `launch` is `"shell"` or `"agent:<name>"`; `cwd` is the
/// directory the pane's shell was believed to be in; `shared` records whether
/// the pane was on the context bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPane {
    pub cwd: String,
    pub launch: String,
    pub shared: bool,
}

impl Session {
    /// Read and parse the session file. A missing file is the caller's concern
    /// (handled with `.ok()`); a *present-but-malformed* file is an error the
    /// caller swallows, falling back to the single-shell-pane default.
    pub fn load(path: &Path) -> anyhow::Result<Session> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        let session: Session = toml::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?;
        Ok(session)
    }

    /// Serialize the current app state to the session file, creating the
    /// parent directory if needed. One `SessionPane` per live pane: its
    /// tracked `cwd`, its launch (resolved back to a name), and whether it is
    /// on the context bus. Pane 0 is always written `shared = true` — the
    /// bus-root invariant — regardless of what the pane's own flag says.
    pub fn save(path: &Path, app: &App, config: &Config) -> anyhow::Result<()> {
        let mut panes = Vec::with_capacity(app.panes.len());
        for (i, pane) in app.panes.iter().enumerate() {
            let launch = match pane.launch {
                Launch::Shell => "shell".to_string(),
                Launch::Agent(idx) => config
                    .agents
                    .get(idx)
                    .map(|a| format!("agent:{}", a.name))
                    .unwrap_or_else(|| "shell".to_string()),
                Launch::Tool(idx) => config
                    .tools
                    .get(idx)
                    .map(|t| format!("tool:{}", t.name))
                    .unwrap_or_else(|| "shell".to_string()),
            };
            panes.push(SessionPane {
                cwd: pane.cwd.to_string_lossy().into_owned(),
                launch,
                shared: i == 0 || pane.shared,
            });
        }
        let session = Session { panes };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("creating {}: {e}", parent.display()))?;
        }
        let raw = toml::to_string_pretty(&session)
            .map_err(|e| anyhow::anyhow!("serializing session: {e}"))?;
        std::fs::write(path, raw)
            .map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
        Ok(())
    }
}
