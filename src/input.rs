//! Keyboard input: global keybindings and key-to-PTY-byte encoding.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::Keybindings;

/// A keybinding handled by the app rather than forwarded to a pane.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GlobalKey {
    NewPane,
    KillPane,
    Quit,
    PrevPane,
    NextPane,
    NewAgent,
    RunTool,
    SendContext,
    BroadcastContext,
    DumpContext,
    ScrollUp,
    ScrollDown,
    ScrollTop,
    ScrollBottom,
}

/// Interpret a key press as a global binding, if one matches.
///
/// The fourteen bindings are checked in a fixed order (first match wins), so a
/// key that matches two configured bindings resolves to the earlier one.
pub fn handle_global(key: &KeyEvent, kb: &Keybindings) -> Option<GlobalKey> {
    if kb.new_pane.matches(key) {
        return Some(GlobalKey::NewPane);
    }
    if kb.kill_pane.matches(key) {
        return Some(GlobalKey::KillPane);
    }
    if kb.quit.matches(key) {
        return Some(GlobalKey::Quit);
    }
    if kb.prev_pane.matches(key) {
        return Some(GlobalKey::PrevPane);
    }
    if kb.next_pane.matches(key) {
        return Some(GlobalKey::NextPane);
    }
    if kb.new_agent.matches(key) {
        return Some(GlobalKey::NewAgent);
    }
    if kb.run_tool.matches(key) {
        return Some(GlobalKey::RunTool);
    }
    if kb.send_context.matches(key) {
        return Some(GlobalKey::SendContext);
    }
    if kb.broadcast_context.matches(key) {
        return Some(GlobalKey::BroadcastContext);
    }
    if kb.dump_context.matches(key) {
        return Some(GlobalKey::DumpContext);
    }
    if kb.scroll_up.matches(key) {
        return Some(GlobalKey::ScrollUp);
    }
    if kb.scroll_down.matches(key) {
        return Some(GlobalKey::ScrollDown);
    }
    if kb.scroll_top.matches(key) {
        return Some(GlobalKey::ScrollTop);
    }
    if kb.scroll_bottom.matches(key) {
        return Some(GlobalKey::ScrollBottom);
    }
    None
}

/// Encode a key press as the bytes a terminal would send to the shell.
///
/// Returns an empty vec for keys with no byte representation (e.g. F13+).
pub fn key_to_bytes(key: &KeyEvent) -> Vec<u8> {
    let mut bytes = match key.code {
        KeyCode::Char(c) => match c {
            // Ctrl+letter is reported as the base letter with the CONTROL
            // modifier; map it to the ASCII control code.
            'a'..='z' if key.modifiers.contains(KeyModifiers::CONTROL) => {
                vec![c as u8 - b'a' + 1]
            }
            _ => c.to_string().into_bytes(),
        },
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Backspace => b"\x7f".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(1) => b"\x1bOP".to_vec(),
        KeyCode::F(2) => b"\x1bOQ".to_vec(),
        KeyCode::F(3) => b"\x1bOR".to_vec(),
        KeyCode::F(4) => b"\x1bOS".to_vec(),
        KeyCode::F(5) => b"\x1b[15~".to_vec(),
        KeyCode::F(6) => b"\x1b[17~".to_vec(),
        KeyCode::F(7) => b"\x1b[18~".to_vec(),
        KeyCode::F(8) => b"\x1b[19~".to_vec(),
        _ => Vec::new(),
    };

    if key.modifiers.contains(KeyModifiers::ALT) && !bytes.is_empty() {
        bytes.insert(0, 0x1b);
    }

    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn scroll_defaults_route_to_scroll_actions() {
        let kb = Keybindings::default();
        let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(
            handle_global(&key(KeyCode::Up, ctrl_shift), &kb),
            Some(GlobalKey::ScrollUp)
        );
        assert_eq!(
            handle_global(&key(KeyCode::Down, ctrl_shift), &kb),
            Some(GlobalKey::ScrollDown)
        );
        assert_eq!(
            handle_global(&key(KeyCode::Home, ctrl_shift), &kb),
            Some(GlobalKey::ScrollTop)
        );
        assert_eq!(
            handle_global(&key(KeyCode::End, ctrl_shift), &kb),
            Some(GlobalKey::ScrollBottom)
        );
    }

    #[test]
    fn scroll_bindings_require_exact_modifiers() {
        let kb = Keybindings::default();
        // A bare arrow key (no ctrl+shift) is not a scroll binding.
        assert_eq!(handle_global(&key(KeyCode::Up, KeyModifiers::NONE), &kb), None);
    }

    #[test]
    fn run_tool_default_routes_to_run_tool() {
        let kb = Keybindings::default();
        let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(
            handle_global(&key(KeyCode::Char('t'), ctrl_shift), &kb),
            Some(GlobalKey::RunTool)
        );
        // A bare `t` (no modifiers) is not a binding.
        assert_eq!(handle_global(&key(KeyCode::Char('t'), KeyModifiers::NONE), &kb), None);
    }
}
