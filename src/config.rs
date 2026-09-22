//! TOML configuration: shell, agents, tools, keybindings, colors, and the
//! max-panes cap.
//!
//! The file is loaded from `--config <path>`, or from the platform config
//! directory (`~/.config/tuiterminals/config.toml`) when no flag is given.
//! Every field is optional: a missing file yields [`Config::default`], and a
//! partial file fills the absent fields with the same defaults. A present but
//! malformed file is a hard error.
//!
//! ```toml
//! max_panes = 6
//!
//! [shell]
//! program = "powershell"
//! args = ["-NoProfile"]
//!
//! [[agents]]
//! name = "claude"
//! program = "claude"
//! args = []
//!
//! [[tools]]
//! name = "review"
//! kind = "run"              # spawn a program in a new pane
//! program = "claude"
//! args = ["-p", "Review the git diff in {cwd}"]
//!
//! [[tools]]
//! name = "explain"
//! kind = "prompt"           # type a template into the active pane
//! text = "Explain the code on screen. Cwd: {cwd}"
//!
//! [keybindings]
//! new_pane  = "ctrl+n"
//! kill_pane = "ctrl+k"
//! quit      = "ctrl+q"
//! prev_pane = "ctrl+left"
//! next_pane = "ctrl+right"
//! new_agent         = "ctrl+shift+n"
//! send_context      = "ctrl+shift+s"
//! broadcast_context = "ctrl+shift+b"
//! dump_context      = "ctrl+shift+c"
//! run_tool          = "ctrl+shift+t"
//! scroll_up         = "ctrl+shift+up"
//! scroll_down       = "ctrl+shift+down"
//! scroll_top        = "ctrl+shift+home"
//! scroll_bottom     = "ctrl+shift+end"
//!
//! [colors]
//! active_border   = "#00ffff"
//! inactive_border = "#444444"
//! ```

use std::path::Path;

use anyhow::Context;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color as RatColor;
use serde::de::{self, Deserializer};
use serde::Deserialize;

/// Top-level configuration.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Config {
    /// Shell each pane spawns. `None` → the platform default shell.
    #[serde(default)]
    pub shell: Option<Shell>,
    /// Named agent programs the agent picker offers (see `[[agents]]`).
    #[serde(default)]
    pub agents: Vec<Agent>,
    /// Named one-shot tools the `run_tool` picker offers (see `[[tools]]`).
    /// Unlike the other sections, a present-but-incomplete `[[tools]]` entry
    /// is a hard error (see [`Tool`]).
    #[serde(default, deserialize_with = "deserialize_tools")]
    pub tools: Vec<Tool>,
    /// Global (app-level) keybindings.
    #[serde(default)]
    pub keybindings: Keybindings,
    /// Pane border colors.
    #[serde(default)]
    pub colors: Colors,
    /// Maximum number of panes in the grid.
    #[serde(default = "default_max_panes")]
    pub max_panes: usize,
    /// Lines of scrollback history each pane's grid keeps above the visible
    /// screen.
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: usize,
    /// Whether to restore the previous session's panes on startup (and save
    /// them on exit). A `false` here means the app always opens with a single
    /// fresh shell pane and writes no session file.
    #[serde(default = "default_restore_sessions")]
    pub restore_sessions: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: None,
            agents: vec![],
            tools: vec![],
            keybindings: Keybindings::default(),
            colors: Colors::default(),
            max_panes: default_max_panes(),
            scrollback_lines: default_scrollback_lines(),
            restore_sessions: default_restore_sessions(),
        }
    }
}

impl Config {
    /// Load config from `path`. A missing file yields [`Config::default`]; a
    /// present but unparseable file is an error.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {}", path.display()))?;
        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("parsing config file {}", path.display()))?;
        Ok(config)
    }
}

fn default_max_panes() -> usize {
    4
}

fn default_scrollback_lines() -> usize {
    10000
}

fn default_restore_sessions() -> bool {
    true
}

/// The program each pane runs, plus its arguments.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Shell {
    pub program: String,
    pub args: Vec<String>,
}

/// A named agent program offered by the agent picker (`[[agents]]`).
/// `name` is the label shown in the picker and in the pane's border title;
/// `program`/`args` are what the pane actually spawns.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Agent {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
}

/// What a tool does when fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// Spawn `program`/`args` in a new pane (like an agent, but one-shot).
    Run,
    /// Type the expanded `text` template into the active pane.
    Prompt,
}

/// A named one-shot command or prompt template (`[[tools]]`).
///
/// Two kinds: a **run** tool spawns `program`/`args` in a new pane (the pane
/// then goes through the usual share/isolate question), and a **prompt** tool
/// types its `text` — with `{cwd}` and `{screen}` placeholders expanded — into
/// the active pane. Unlike the other config sections, a present-but-incomplete
/// entry is a **hard error**: a `run` tool needs `program` (and `kind = "run"`),
/// a `prompt` tool needs `text` (and `kind = "prompt"`).
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub kind: ToolKind,
    /// Program to spawn, for `run` tools.
    pub program: Option<String>,
    /// Arguments for the spawned program, for `run` tools.
    pub args: Vec<String>,
    /// The prompt template, for `prompt` tools.
    pub text: Option<String>,
}

/// The raw, unvalidated shape of a `[[tools]]` entry. Deserialized per entry
/// so a malformed one produces a precise error (see [`tool_from_raw`]).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RawTool {
    name: String,
    kind: Option<String>,
    program: Option<String>,
    args: Vec<String>,
    text: Option<String>,
}

/// Validate a raw tool entry: `kind` must be `"run"` or `"prompt"`; a `run`
/// tool must carry a non-empty `program`; a `prompt` tool must carry a
/// non-empty `text`.
fn tool_from_raw(raw: RawTool) -> Result<Tool, String> {
    let kind = match raw.kind.as_deref() {
        Some("run") => ToolKind::Run,
        Some("prompt") => ToolKind::Prompt,
        Some(other) => {
            return Err(format!(
                "tool {}: unknown kind {other:?} (expected \"run\" or \"prompt\")",
                raw.name
            ))
        }
        None => {
            return Err(format!(
                "tool {}: missing kind (expected \"run\" or \"prompt\")",
                raw.name
            ))
        }
    };
    match kind {
        ToolKind::Run => {
            let program = match raw.program.filter(|p| !p.trim().is_empty()) {
                Some(p) => p,
                None => {
                    return Err(format!(
                        "tool {}: a run tool needs a non-empty program",
                        raw.name
                    ))
                }
            };
            Ok(Tool {
                name: raw.name,
                kind,
                program: Some(program),
                args: raw.args,
                text: None,
            })
        }
        ToolKind::Prompt => {
            let text = match raw.text.filter(|t| !t.trim().is_empty()) {
                Some(t) => t,
                None => {
                    return Err(format!(
                        "tool {}: a prompt tool needs a non-empty text",
                        raw.name
                    ))
                }
            };
            Ok(Tool {
                name: raw.name,
                kind,
                program: None,
                args: vec![],
                text: Some(text),
            })
        }
    }
}

/// Deserialize the `[[tools]]` list, validating each entry.
fn deserialize_tools<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Tool>, D::Error> {
    let raws: Vec<RawTool> = serde::Deserialize::deserialize(d)?;
    raws.into_iter()
        .map(tool_from_raw)
        .collect::<Result<Vec<_>, _>>()
        .map_err(serde::de::Error::custom)
}

/// The global actions and their key combos.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Keybindings {
    #[serde(
        default = "KeyCombo::new_pane_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub new_pane: KeyCombo,
    #[serde(
        default = "KeyCombo::kill_pane_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub kill_pane: KeyCombo,
    #[serde(
        default = "KeyCombo::quit_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub quit: KeyCombo,
    #[serde(
        default = "KeyCombo::prev_pane_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub prev_pane: KeyCombo,
    #[serde(
        default = "KeyCombo::next_pane_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub next_pane: KeyCombo,
    #[serde(
        default = "KeyCombo::new_agent_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub new_agent: KeyCombo,
    #[serde(
        default = "KeyCombo::send_context_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub send_context: KeyCombo,
    #[serde(
        default = "KeyCombo::broadcast_context_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub broadcast_context: KeyCombo,
    #[serde(
        default = "KeyCombo::dump_context_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub dump_context: KeyCombo,
    #[serde(
        default = "KeyCombo::run_tool_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub run_tool: KeyCombo,
    #[serde(
        default = "KeyCombo::scroll_up_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub scroll_up: KeyCombo,
    #[serde(
        default = "KeyCombo::scroll_down_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub scroll_down: KeyCombo,
    #[serde(
        default = "KeyCombo::scroll_top_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub scroll_top: KeyCombo,
    #[serde(
        default = "KeyCombo::scroll_bottom_default",
        deserialize_with = "deserialize_key_combo"
    )]
    pub scroll_bottom: KeyCombo,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            new_pane: KeyCombo::new_pane_default(),
            kill_pane: KeyCombo::kill_pane_default(),
            quit: KeyCombo::quit_default(),
            prev_pane: KeyCombo::prev_pane_default(),
            next_pane: KeyCombo::next_pane_default(),
            new_agent: KeyCombo::new_agent_default(),
            send_context: KeyCombo::send_context_default(),
            broadcast_context: KeyCombo::broadcast_context_default(),
            dump_context: KeyCombo::dump_context_default(),
            run_tool: KeyCombo::run_tool_default(),
            scroll_up: KeyCombo::scroll_up_default(),
            scroll_down: KeyCombo::scroll_down_default(),
            scroll_top: KeyCombo::scroll_top_default(),
            scroll_bottom: KeyCombo::scroll_bottom_default(),
        }
    }
}

/// A single key binding: a key code plus the modifiers that must be held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub mods: KeyModifiers,
    pub code: KeyCode,
}

impl KeyCombo {
    fn new_pane_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL,
            code: KeyCode::Char('n'),
        }
    }
    fn kill_pane_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL,
            code: KeyCode::Char('k'),
        }
    }
    fn quit_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL,
            code: KeyCode::Char('q'),
        }
    }
    fn prev_pane_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL,
            code: KeyCode::Left,
        }
    }
    fn next_pane_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL,
            code: KeyCode::Right,
        }
    }
    fn new_agent_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            code: KeyCode::Char('n'),
        }
    }
    fn send_context_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            code: KeyCode::Char('s'),
        }
    }
    fn broadcast_context_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            code: KeyCode::Char('b'),
        }
    }
    fn dump_context_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            code: KeyCode::Char('c'),
        }
    }
    fn run_tool_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            code: KeyCode::Char('t'),
        }
    }
    fn scroll_up_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            code: KeyCode::Up,
        }
    }
    fn scroll_down_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            code: KeyCode::Down,
        }
    }
    fn scroll_top_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            code: KeyCode::Home,
        }
    }
    fn scroll_bottom_default() -> Self {
        Self {
            mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            code: KeyCode::End,
        }
    }

    /// True when `key` has this combo's code and exactly this combo's
    /// modifier bits (so `ctrl+n` does not also match `shift+ctrl+n`).
    ///
    /// The key's letter is compared in lowercase: terminals commonly encode
    /// `Shift+letter` as the uppercase letter, so a binding written
    /// `ctrl+shift+n` arrives as `Char('N')` and would otherwise never match.
    pub fn matches(&self, key: &KeyEvent) -> bool {
        let key_code = match key.code {
            KeyCode::Char(c @ 'A'..='Z') => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        if key_code != self.code {
            return false;
        }
        for m in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SHIFT,
            KeyModifiers::SUPER,
        ] {
            if key.modifiers.contains(m) != self.mods.contains(m) {
                return false;
            }
        }
        true
    }
}

/// Parse a binding string like `"ctrl+left"` or `"shift+enter"` into a
/// [`KeyCombo`]. Modifier tokens may appear in any order; exactly one key
/// token is required.
fn parse_combo(s: &str) -> Result<KeyCombo, String> {
    let mut mods = KeyModifiers::NONE;
    let mut code: Option<KeyCode> = None;

    for token in s.split('+').map(str::trim) {
        if token.is_empty() {
            continue;
        }
        match token.to_lowercase().as_str() {
            "ctrl" | "control" => mods |= KeyModifiers::CONTROL,
            "alt" | "option" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            "super" | "meta" | "cmd" | "win" => mods |= KeyModifiers::SUPER,
            other => {
                let parsed = parse_key(other)
                    .ok_or_else(|| format!("unknown key or modifier {other:?} in {s:?}"))?;
                if code.is_some() {
                    return Err(format!("more than one key in {s:?}"));
                }
                code = Some(parsed);
            }
        }
    }

    let code = code.ok_or_else(|| format!("no key in {s:?}"))?;
    Ok(KeyCombo { mods, code })
}

/// Map a single non-modifier token to a [`KeyCode`].
fn parse_key(token: &str) -> Option<KeyCode> {
    if token.chars().count() == 1 {
        return token.chars().next().map(KeyCode::Char);
    }
    Some(match token {
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "enter" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "esc" | "escape" => KeyCode::Esc,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        "delete" | "del" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "f1" => KeyCode::F(1),
        "f2" => KeyCode::F(2),
        "f3" => KeyCode::F(3),
        "f4" => KeyCode::F(4),
        "f5" => KeyCode::F(5),
        "f6" => KeyCode::F(6),
        "f7" => KeyCode::F(7),
        "f8" => KeyCode::F(8),
        "f9" => KeyCode::F(9),
        "f10" => KeyCode::F(10),
        "f11" => KeyCode::F(11),
        "f12" => KeyCode::F(12),
        _ => return None,
    })
}

/// Serde wrapper: read a binding string and parse it.
fn deserialize_key_combo<'de, D: Deserializer<'de>>(d: D) -> Result<KeyCombo, D::Error> {
    let s = String::deserialize(d)?;
    parse_combo(&s).map_err(de::Error::custom)
}

/// Pane border colors.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Colors {
    #[serde(default, deserialize_with = "deserialize_color")]
    pub active_border: RgbColor,
    #[serde(default, deserialize_with = "deserialize_color")]
    pub inactive_border: RgbColor,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            active_border: RgbColor {
                r: 0x00,
                g: 0xff,
                b: 0xff,
            }, // cyan
            inactive_border: RgbColor {
                r: 0x80,
                g: 0x80,
                b: 0x80,
            }, // dark gray
        }
    }
}

/// An RGB color, written in TOML as a `"#rrggbb"` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Default for RgbColor {
    fn default() -> Self {
        Self {
            r: 0xff,
            g: 0xff,
            b: 0xff,
        }
    }
}

impl RgbColor {
    /// The ratatui color for this RGB value.
    pub fn to_ratatui(self) -> RatColor {
        RatColor::Rgb(self.r, self.g, self.b)
    }
}

/// Parse a `"#rrggbb"` (or `"rrggbb"`) string into an [`RgbColor`].
fn parse_color(s: &str) -> Result<RgbColor, String> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return Err(format!("expected #rrggbb, got {s:?}"));
    }
    let byte = |i: usize| -> Result<u8, String> {
        u8::from_str_radix(&hex[i..i + 2], 16)
            .map_err(|e| format!("bad hex in {s:?}: {e}"))
    };
    Ok(RgbColor {
        r: byte(0)?,
        g: byte(2)?,
        b: byte(4)?,
    })
}

/// Serde wrapper: read a color string and parse it.
fn deserialize_color<'de, D: Deserializer<'de>>(d: D) -> Result<RgbColor, D::Error> {
    let s = String::deserialize(d)?;
    parse_color(&s).map_err(de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn parse_combo_char_with_ctrl() {
        let c = parse_combo("ctrl+n").unwrap();
        assert_eq!(c.code, KeyCode::Char('n'));
        assert!(c.mods.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn parse_combo_named_key() {
        let c = parse_combo("ctrl+left").unwrap();
        assert_eq!(c.code, KeyCode::Left);
        assert!(c.mods.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn parse_combo_multi_modifier_any_order() {
        let c = parse_combo("shift+enter").unwrap();
        assert_eq!(c.code, KeyCode::Enter);
        assert!(c.mods.contains(KeyModifiers::SHIFT));
        assert!(!c.mods.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn parse_combo_ctrl_shift() {
        let c = parse_combo("ctrl+shift+n").unwrap();
        assert_eq!(c.code, KeyCode::Char('n'));
        assert!(c.mods.contains(KeyModifiers::CONTROL));
        assert!(c.mods.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn parse_combo_rejects_unknown() {
        assert!(parse_combo("ctrl+warp").is_err());
        assert!(parse_combo("ctrl").is_err()); // no key
        assert!(parse_combo("ctrl+n+m").is_err()); // two keys
    }

    #[test]
    fn combo_matches_exact_modifiers() {
        let c = parse_combo("ctrl+n").unwrap();
        assert!(c.matches(&key(KeyCode::Char('n'), KeyModifiers::CONTROL)));
        // Extra modifier must not match.
        assert!(!c.matches(&key(
            KeyCode::Char('n'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
        // Wrong code must not match.
        assert!(!c.matches(&key(KeyCode::Char('k'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn combo_matches_uppercase_letter_as_lowercase() {
        // A terminal that encodes Shift+N as the uppercase 'N' must still
        // match a binding written with the lowercase letter.
        let c = parse_combo("ctrl+shift+n").unwrap();
        assert!(c.matches(&key(
            KeyCode::Char('N'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
        // And a plain ctrl+n binding matches an uppercase 'N' with ctrl only.
        let p = parse_combo("ctrl+n").unwrap();
        assert!(p.matches(&key(KeyCode::Char('N'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn uppercase_normalization_keeps_exact_modifiers() {
        // ctrl+n must NOT match ctrl+shift+n even when the letter is uppercase.
        let p = parse_combo("ctrl+n").unwrap();
        assert!(!p.matches(&key(
            KeyCode::Char('N'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
    }

    #[test]
    fn parse_color_hex() {
        let c = parse_color("#00ffff").unwrap();
        assert_eq!((c.r, c.g, c.b), (0, 255, 255));
        assert_eq!(parse_color("444444").unwrap(), RgbColor { r: 0x44, g: 0x44, b: 0x44 });
    }

    #[test]
    fn parse_color_rejects_bad() {
        assert!(parse_color("#12").is_err());
        assert!(parse_color("zzzzzz").is_err());
    }

    #[test]
    fn defaults_match_previous_hardcoded_behavior() {
        let kb = Keybindings::default();
        assert_eq!(kb.new_pane, parse_combo("ctrl+n").unwrap());
        assert_eq!(kb.kill_pane, parse_combo("ctrl+k").unwrap());
        assert_eq!(kb.quit, parse_combo("ctrl+q").unwrap());
        assert_eq!(kb.prev_pane, parse_combo("ctrl+left").unwrap());
        assert_eq!(kb.next_pane, parse_combo("ctrl+right").unwrap());
        assert_eq!(kb.new_agent, parse_combo("ctrl+shift+n").unwrap());
        assert_eq!(kb.send_context, parse_combo("ctrl+shift+s").unwrap());
        assert_eq!(kb.broadcast_context, parse_combo("ctrl+shift+b").unwrap());
        assert_eq!(kb.dump_context, parse_combo("ctrl+shift+c").unwrap());
        assert_eq!(kb.run_tool, parse_combo("ctrl+shift+t").unwrap());
        assert_eq!(kb.scroll_up, parse_combo("ctrl+shift+up").unwrap());
        assert_eq!(kb.scroll_down, parse_combo("ctrl+shift+down").unwrap());
        assert_eq!(kb.scroll_top, parse_combo("ctrl+shift+home").unwrap());
        assert_eq!(kb.scroll_bottom, parse_combo("ctrl+shift+end").unwrap());
        assert_eq!(Config::default().max_panes, 4);
        assert_eq!(Config::default().scrollback_lines, 10000);
    }

    #[test]
    fn load_absent_file_gives_defaults() {
        let path = std::path::Path::new("/nonexistent/tuiterminals/config.toml");
        let config = Config::load(path).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn load_partial_file_fills_defaults() {
        let dir = std::env::temp_dir().join("tuiterminals-test-partial");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("partial.toml");
        std::fs::write(&path, "max_panes = 8\n").unwrap();

        let config = Config::load(&path).unwrap();
        assert_eq!(config.max_panes, 8);
        assert!(config.shell.is_none());
        assert!(config.agents.is_empty());
        assert!(config.tools.is_empty());
        assert_eq!(config.keybindings, Keybindings::default());
        // An absent scrollback knob falls back to the default.
        assert_eq!(config.scrollback_lines, 10000);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_partial_file_parses_scrollback_lines() {
        let dir = std::env::temp_dir().join("tuiterminals-test-scrollback");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scrollback.toml");
        std::fs::write(&path, "scrollback_lines = 500\n").unwrap();

        let config = Config::load(&path).unwrap();
        assert_eq!(config.scrollback_lines, 500);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_full_file_parses_all_knobs() {
        let dir = std::env::temp_dir().join("tuiterminals-test-full");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("full.toml");
        std::fs::write(
            &path,
            r##"
max_panes = 6

[shell]
program = "powershell"
args = ["-NoProfile"]

[[agents]]
name = "claude"
program = "claude"
args = []

[[agents]]
name = "codex"
program = "codex"
args = ["--yolo"]

[[tools]]
name = "review"
kind = "run"
program = "claude"
args = ["-p", "Review the git diff in {cwd}"]

[[tools]]
name = "explain"
kind = "prompt"
text = "Explain the code on screen. Cwd: {cwd}"

[keybindings]
new_pane  = "ctrl+n"
kill_pane = "ctrl+k"
quit      = "ctrl+q"
prev_pane = "ctrl+left"
next_pane = "ctrl+right"
new_agent         = "ctrl+shift+n"
send_context      = "ctrl+shift+s"
broadcast_context = "ctrl+shift+b"
dump_context      = "ctrl+shift+c"
run_tool          = "ctrl+shift+t"
scroll_up         = "ctrl+shift+up"
scroll_down       = "ctrl+shift+down"
scroll_top        = "ctrl+shift+home"
scroll_bottom     = "ctrl+shift+end"

[colors]
active_border   = "#00ffff"
inactive_border = "#444444"
"##,
        )
        .unwrap();

        let config = Config::load(&path).unwrap();
        assert_eq!(
            config.shell,
            Some(Shell {
                program: "powershell".to_string(),
                args: vec!["-NoProfile".to_string()],
            })
        );
        assert_eq!(
            config.agents,
            vec![
                Agent {
                    name: "claude".to_string(),
                    program: "claude".to_string(),
                    args: vec![],
                },
                Agent {
                    name: "codex".to_string(),
                    program: "codex".to_string(),
                    args: vec!["--yolo".to_string()],
                }
            ]
        );
        assert_eq!(config.max_panes, 6);
        assert_eq!(
            config.tools,
            vec![
                Tool {
                    name: "review".to_string(),
                    kind: ToolKind::Run,
                    program: Some("claude".to_string()),
                    args: vec!["-p".to_string(), "Review the git diff in {cwd}".to_string()],
                    text: None,
                },
                Tool {
                    name: "explain".to_string(),
                    kind: ToolKind::Prompt,
                    program: None,
                    args: vec![],
                    text: Some("Explain the code on screen. Cwd: {cwd}".to_string()),
                }
            ]
        );
        assert_eq!(
            config.colors.active_border,
            RgbColor {
                r: 0x00,
                g: 0xff,
                b: 0xff
            }
        );
        assert_eq!(
            config.colors.inactive_border,
            RgbColor {
                r: 0x44,
                g: 0x44,
                b: 0x44
            }
        );
        // The example uses the documented defaults, so it should equal them.
        assert_eq!(config.keybindings, Keybindings::default());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_prompt_tool_missing_text_errors() {
        let dir = std::env::temp_dir().join("tuiterminals-test-tool-text");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tool.toml");
        std::fs::write(
            &path,
            r##"
[[tools]]
name = "explain"
kind = "prompt"
"##,
        )
        .unwrap();

        assert!(Config::load(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_run_tool_missing_program_errors() {
        let dir = std::env::temp_dir().join("tuiterminals-test-tool-program");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tool.toml");
        std::fs::write(
            &path,
            r##"
[[tools]]
name = "review"
kind = "run"
"##,
        )
        .unwrap();

        assert!(Config::load(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_tool_missing_kind_errors() {
        let dir = std::env::temp_dir().join("tuiterminals-test-tool-kind");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tool.toml");
        std::fs::write(
            &path,
            r##"
[[tools]]
name = "mystery"
program = "claude"
"##,
        )
        .unwrap();

        assert!(Config::load(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_tool_unknown_kind_errors() {
        let dir = std::env::temp_dir().join("tuiterminals-test-tool-unknown-kind");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tool.toml");
        std::fs::write(
            &path,
            r##"
[[tools]]
name = "weird"
kind = "teleport"
program = "claude"
"##,
        )
        .unwrap();

        assert!(Config::load(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_malformed_file_errors() {
        let dir = std::env::temp_dir().join("tuiterminals-test-malformed");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.toml");
        std::fs::write(&path, "this is not [valid toml").unwrap();

        assert!(Config::load(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
