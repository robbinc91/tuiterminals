//! Modal prompts: the small state machine behind the bottom-of-screen overlays
//! for spawning panes/agents and relaying context between panes.
//!
//! While a modal is open, *all* key presses route here (Esc always cancels);
//! nothing reaches the global keybindings or the panes. Each state either
//! returns a [`ModalAction`] for `main.rs` to execute, transitions to another
//! state (returning `None`), or swallows the key (returning `None` with the
//! state unchanged).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::Line;

use crate::app::{App, relayable};
use crate::config::Agent;

/// The modal state machine. `None` = no modal is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    /// No modal is open.
    None,
    /// "share context? y/n" — asked when a plain shell pane is being spawned.
    NewPaneShare,
    /// Agent spawn flow. In [`NewAgentStep::Pick`] the user chooses an agent
    /// by digit (or `s` for a plain shell, which defers to
    /// [`Modal::NewPaneShare`]); in [`NewAgentStep::Share`] the chosen agent's
    /// index sits in `chosen` and the y/n share question is pending.
    NewAgent {
        step: NewAgentStep,
        chosen: Option<usize>,
    },
    /// "send the active pane's screen to which other shared pane?" — `source`
    /// is the active pane's index.
    SendTarget { source: usize },
}

/// Which step of the agent-spawn flow we're in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewAgentStep {
    /// Choosing which agent (or a plain shell) to spawn.
    Pick,
    /// An agent is chosen; asking whether it shares context.
    Share,
}

/// What `main.rs` should do once a modal produces an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalAction {
    /// Spawn a plain shell pane, shared or isolated.
    SpawnShell { shared: bool },
    /// Spawn the configured agent at `index`, shared or isolated.
    SpawnAgent { index: usize, shared: bool },
    /// Send the source pane's screen text to the shared pane at `target`.
    Send { target: usize },
    /// Cancel the modal; nothing else happens.
    Cancel,
}

impl Modal {
    /// Route a key press to the open modal.
    ///
    /// `agents` is the configured agent list (drives the picker); `targets` is
    /// the already-filtered list of pane indices that can receive a relay from
    /// the active pane — shared, alive, and not the source (see
    /// [`send_targets`]). Returns `Some(action)` when the key completes the
    /// flow, `None` when it moves to another state or is swallowed.
    pub fn handle_key(
        &mut self,
        key: &KeyEvent,
        agents: &[Agent],
        targets: &[usize],
    ) -> Option<ModalAction> {
        match self {
            Modal::None => None,
            Modal::NewPaneShare => match key.code {
                KeyCode::Char('y') => Some(ModalAction::SpawnShell { shared: true }),
                KeyCode::Char('n') => Some(ModalAction::SpawnShell { shared: false }),
                KeyCode::Esc => Some(ModalAction::Cancel),
                _ => None,
            },
            Modal::NewAgent { step, chosen } => match (*step, key.code) {
                (NewAgentStep::Pick, KeyCode::Char(c @ '1'..='9')) => {
                    let i = (c as usize - '0' as usize) - 1;
                    if i < agents.len() {
                        *self = Modal::NewAgent {
                            step: NewAgentStep::Share,
                            chosen: Some(i),
                        };
                        None
                    } else {
                        None
                    }
                }
                (NewAgentStep::Pick, KeyCode::Char('s')) => {
                    *self = Modal::NewPaneShare;
                    None
                }
                (NewAgentStep::Share, KeyCode::Char('y')) if chosen.is_some() => {
                    Some(ModalAction::SpawnAgent {
                        index: chosen.unwrap(),
                        shared: true,
                    })
                }
                (NewAgentStep::Share, KeyCode::Char('n')) if chosen.is_some() => {
                    Some(ModalAction::SpawnAgent {
                        index: chosen.unwrap(),
                        shared: false,
                    })
                }
                (_, KeyCode::Esc) => Some(ModalAction::Cancel),
                _ => None,
            },
            Modal::SendTarget { .. } => match key.code {
                KeyCode::Char(c @ '1'..='9') => {
                    let i = (c as usize - '0' as usize) - 1;
                    targets.get(i).map(|&target| ModalAction::Send { target })
                }
                KeyCode::Esc => Some(ModalAction::Cancel),
                _ => None,
            },
        }
    }

    /// The overlay line(s) for the current modal state, rendered at the
    /// bottom of the frame. Empty when no modal is open.
    pub fn prompt(&self, app: &App, agents: &[Agent]) -> Vec<Line<'_>> {
        match self {
            Modal::None => Vec::new(),
            Modal::NewPaneShare => vec![Line::from(
                " new pane — share context with other panes?  [y] share   [n] isolate   (esc) cancel",
            )],
            Modal::NewAgent {
                step: NewAgentStep::Pick,
                ..
            } => vec![Line::from(new_agent_pick_line(agents))],
            Modal::NewAgent {
                step: NewAgentStep::Share,
                chosen,
            } => {
                let name = chosen
                    .and_then(|i| agents.get(i))
                    .map(|a| a.name.as_str())
                    .unwrap_or("agent");
                vec![Line::from(format!(
                    " new agent ({name}) — share context with other panes?  [y] share   [n] isolate   (esc) cancel"
                ))]
            }
            Modal::SendTarget { source } => {
                let src_name = app
                    .panes
                    .get(*source)
                    .and_then(|p| p.agent_name.as_deref())
                    .unwrap_or("shell");
                let targets = send_targets(app, *source);
                let pairs: Vec<(usize, &str)> = targets
                    .iter()
                    .map(|&t| {
                        (
                            t,
                            app.panes
                                .get(t)
                                .and_then(|p| p.agent_name.as_deref())
                                .unwrap_or("shell"),
                        )
                    })
                    .collect();
                vec![send_target_line(*source, src_name, &pairs)]
            }
        }
    }
}

/// The single overlay line for the agent picker: one `[n] name` entry per
/// configured agent, then the plain-shell option.
fn new_agent_pick_line(agents: &[Agent]) -> String {
    let mut s = String::from(" new agent — pick:  ");
    for (i, agent) in agents.iter().enumerate() {
        s.push_str(&format!("[{}] {}   ", i + 1, agent.name));
    }
    s.push_str("[s] plain shell   (esc) cancel");
    s
}

/// The single overlay line for a send-target prompt, given the source pane's
/// display name and the `(index, name)` pairs of its relay targets.
fn send_target_line(source: usize, src_name: &str, targets: &[(usize, &str)]) -> Line<'static> {
    let mut s = format!(" send context from pane {} ({}) to:  ", source + 1, src_name);
    for (i, (t, name)) in targets.iter().enumerate() {
        s.push_str(&format!("[{}] pane {} ({})   ", i + 1, t + 1, name));
    }
    s.push_str("(esc) cancel");
    Line::from(s)
}

/// The indices of panes that can receive a relay from `source`: on the
/// context bus, alive, and not the source itself.
pub(crate) fn send_targets(app: &App, source: usize) -> Vec<usize> {
    app.panes
        .iter()
        .enumerate()
        .filter(|(i, p)| relayable(p.shared, p.alive, *i, source))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    fn agents() -> Vec<Agent> {
        vec![
            Agent {
                name: "claude".into(),
                program: "claude".into(),
                args: vec![],
            },
            Agent {
                name: "codex".into(),
                program: "codex".into(),
                args: vec![],
            },
        ]
    }

    // --- NewPaneShare ---

    #[test]
    fn new_pane_share_y_spawns_shared() {
        let mut m = Modal::NewPaneShare;
        assert_eq!(
            m.handle_key(&key('y'), &[], &[]),
            Some(ModalAction::SpawnShell { shared: true })
        );
    }

    #[test]
    fn new_pane_share_n_spawns_isolated() {
        let mut m = Modal::NewPaneShare;
        assert_eq!(
            m.handle_key(&key('n'), &[], &[]),
            Some(ModalAction::SpawnShell { shared: false })
        );
    }

    #[test]
    fn new_pane_share_esc_cancels() {
        let mut m = Modal::NewPaneShare;
        assert_eq!(m.handle_key(&esc(), &[], &[]), Some(ModalAction::Cancel));
    }

    #[test]
    fn new_pane_share_unknown_is_swallowed() {
        let mut m = Modal::NewPaneShare;
        assert_eq!(m.handle_key(&key('x'), &[], &[]), None);
        assert_eq!(m, Modal::NewPaneShare);
    }

    // --- NewAgent::Pick ---

    #[test]
    fn agent_pick_digit_moves_to_share_step() {
        let mut m = Modal::NewAgent {
            step: NewAgentStep::Pick,
            chosen: None,
        };
        assert_eq!(m.handle_key(&key('2'), &agents(), &[]), None);
        assert_eq!(
            m,
            Modal::NewAgent {
                step: NewAgentStep::Share,
                chosen: Some(1)
            }
        );
    }

    #[test]
    fn agent_pick_digit_out_of_range_is_swallowed() {
        let mut m = Modal::NewAgent {
            step: NewAgentStep::Pick,
            chosen: None,
        };
        assert_eq!(m.handle_key(&key('3'), &agents(), &[]), None);
        assert_eq!(
            m,
            Modal::NewAgent {
                step: NewAgentStep::Pick,
                chosen: None
            }
        );
    }

    #[test]
    fn agent_pick_s_defers_to_shell_share_question() {
        let mut m = Modal::NewAgent {
            step: NewAgentStep::Pick,
            chosen: None,
        };
        assert_eq!(m.handle_key(&key('s'), &agents(), &[]), None);
        assert_eq!(m, Modal::NewPaneShare);
    }

    #[test]
    fn agent_pick_esc_cancels() {
        let mut m = Modal::NewAgent {
            step: NewAgentStep::Pick,
            chosen: None,
        };
        assert_eq!(
            m.handle_key(&esc(), &agents(), &[]),
            Some(ModalAction::Cancel)
        );
    }

    // --- NewAgent::Share ---

    #[test]
    fn agent_share_y_spawns_shared_agent() {
        let mut m = Modal::NewAgent {
            step: NewAgentStep::Share,
            chosen: Some(0),
        };
        assert_eq!(
            m.handle_key(&key('y'), &agents(), &[]),
            Some(ModalAction::SpawnAgent {
                index: 0,
                shared: true
            })
        );
    }

    #[test]
    fn agent_share_n_spawns_isolated_agent() {
        let mut m = Modal::NewAgent {
            step: NewAgentStep::Share,
            chosen: Some(1),
        };
        assert_eq!(
            m.handle_key(&key('n'), &agents(), &[]),
            Some(ModalAction::SpawnAgent {
                index: 1,
                shared: false
            })
        );
    }

    #[test]
    fn agent_share_esc_cancels() {
        let mut m = Modal::NewAgent {
            step: NewAgentStep::Share,
            chosen: Some(0),
        };
        assert_eq!(
            m.handle_key(&esc(), &agents(), &[]),
            Some(ModalAction::Cancel)
        );
    }

    // --- SendTarget ---

    #[test]
    fn send_target_digit_maps_into_filtered_list() {
        let mut m = Modal::SendTarget { source: 0 };
        // Filtered targets are panes 1 and 3; digit '2' → pane 3.
        assert_eq!(
            m.handle_key(&key('2'), &[], &[1, 3]),
            Some(ModalAction::Send { target: 3 })
        );
    }

    #[test]
    fn send_target_digit_out_of_range_is_swallowed() {
        let mut m = Modal::SendTarget { source: 0 };
        assert_eq!(m.handle_key(&key('3'), &[], &[1]), None);
        assert_eq!(m, Modal::SendTarget { source: 0 });
    }

    #[test]
    fn send_target_esc_cancels() {
        let mut m = Modal::SendTarget { source: 0 };
        assert_eq!(
            m.handle_key(&esc(), &[], &[1]),
            Some(ModalAction::Cancel)
        );
    }

    // --- None ---

    #[test]
    fn none_swallows_everything() {
        let mut m = Modal::None;
        assert_eq!(m.handle_key(&key('y'), &[], &[]), None);
        assert_eq!(m.handle_key(&esc(), &[], &[]), None);
        assert_eq!(m, Modal::None);
    }

    // --- prompt lines ---

    #[test]
    fn agent_pick_line_lists_agents_then_shell() {
        let line = new_agent_pick_line(&agents());
        assert_eq!(
            line,
            " new agent — pick:  [1] claude   [2] codex   [s] plain shell   (esc) cancel"
        );
    }

    #[test]
    fn agent_pick_line_with_no_agents_offers_shell_only() {
        let line = new_agent_pick_line(&[]);
        assert_eq!(
            line,
            " new agent — pick:  [s] plain shell   (esc) cancel"
        );
    }

    #[test]
    fn send_target_line_numbers_the_filtered_targets() {
        let line = send_target_line(0, "claude", &[(1, "shell"), (3, "codex")]);
        assert_eq!(
            line,
            Line::from(" send context from pane 1 (claude) to:  [1] pane 2 (shell)   [2] pane 4 (codex)   (esc) cancel")
        );
    }

    #[test]
    fn send_target_line_with_no_targets_still_offers_cancel() {
        let line = send_target_line(0, "shell", &[]);
        assert_eq!(line, Line::from(" send context from pane 1 (shell) to:  (esc) cancel"));
    }
}
