//! Application state: the pane list, PTY lifecycle, and per-tick updates.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtyPair, PtySize, PtySystem};

use crate::config::{Agent, Config, Shell};

/// How long a pane may be quiet before it reads as "idle" rather than "working".
/// A shell sitting at a prompt is idle; a worker streaming output is working.
pub const IDLE_AFTER: Duration = Duration::from_secs(2);

/// A pane's activity state, derived from how recently its PTY produced output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneStatus {
    /// The pane's PTY produced output within [`IDLE_AFTER`].
    Working,
    /// The pane is alive but has been quiet for at least [`IDLE_AFTER`].
    Idle,
    /// The pane's child process has exited.
    Dead,
}

/// Dimensions handed to `Term::new` / `Term::resize`.
///
/// v1 has no scrollback: `total_lines == screen_lines`, so the grid's
/// `display_offset` is always 0 and `display_iter` covers exactly the
/// visible screen.
#[derive(Copy, Clone, Debug)]
pub struct TermSize {
    pub rows: u16,
    pub cols: u16,
}

impl TermSize {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }
}

/// What a pane runs: a plain shell, or one of the configured agents by index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    Shell,
    Agent(usize),
}

impl App {
    /// Create the app with a single pane filling `size`. The first pane is
    /// **shared** — it is the root of the context bus — and starts in the
    /// directory the app was launched from (where `CONTEXT.md` will live).
    pub fn new(
        pty_system: Box<dyn PtySystem + Send>,
        size: TermSize,
        config: &Config,
    ) -> anyhow::Result<Self> {
        let launch_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let command = Self::shell_command(&config.shell);
        let pane = Pane::new(&*pty_system, size, &command, None, &launch_dir, true)?;
        Ok(Self {
            panes: vec![pane],
            active: 0,
            pty_system,
            max_panes: config.max_panes,
            shell: config.shell.clone(),
            agents: config.agents.clone(),
            launch_dir,
        })
    }

    /// The `CommandBuilder` for the configured shell (or the platform default).
    fn shell_command(shell: &Option<Shell>) -> CommandBuilder {
        match shell {
            Some(s) => {
                let mut cmd = CommandBuilder::new(&s.program);
                for arg in &s.args {
                    cmd.arg(arg);
                }
                cmd
            }
            None => CommandBuilder::new_default_prog(),
        }
    }

    /// Resolve `launch` to the command to spawn plus the agent's display name
    /// (`None` for a plain shell). An out-of-range agent index falls back to
    /// the shell so a stale picker pick can never crash a spawn.
    fn command_for(&self, launch: &Launch) -> (CommandBuilder, Option<String>) {
        match launch {
            Launch::Shell => (Self::shell_command(&self.shell), None),
            Launch::Agent(i) => match self.agents.get(*i) {
                Some(agent) => {
                    let mut cmd = CommandBuilder::new(&agent.program);
                    for arg in &agent.args {
                        cmd.arg(arg);
                    }
                    (cmd, Some(agent.name.clone()))
                }
                None => (Self::shell_command(&self.shell), None),
            },
        }
    }

    /// Feed all pending PTY output into each pane's VT processor, then write
    /// back any PTY bytes the terminal requested (e.g. CPR responses).
    pub fn pump(&mut self) {
        for pane in &mut self.panes {
            while let Ok(bytes) = pane.bytes.try_recv() {
                // Any output means the pane is doing something right now.
                pane.last_activity = Instant::now();
                pane.processor.advance(&mut pane.term, &bytes);
            }
            while let Ok(text) = pane.pty_rx.try_recv() {
                let _ = pane.writer.write_all(text.as_bytes());
            }
        }
    }

    /// Check child processes; mark panes whose shell has exited.
    pub fn check_children(&mut self) {
        for pane in &mut self.panes {
            if !pane.alive {
                continue;
            }
            if let Ok(Some(status)) = pane.child.try_wait() {
                pane.alive = false;
                pane.exit_status = Some(status);
            }
        }
    }

    /// Resize every pane's term grid and PTY to the given per-pane sizes.
    pub fn resize_panes(&mut self, sizes: &[TermSize]) {
        for (pane, size) in self.panes.iter_mut().zip(sizes) {
            pane.term.resize(*size);
            let _ = pane.master.resize(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    /// Add a pane (up to `max_panes`) sized `size` running `launch`; `shared`
    /// decides whether it joins the context bus. Returns the new index (or the
    /// active index when the cap is reached). The new pane starts in the
    /// active pane's current directory, so a pane opened right after `cd`-ing
    /// in the active pane lands in the same place.
    pub fn add_pane(
        &mut self,
        size: TermSize,
        launch: &Launch,
        shared: bool,
    ) -> anyhow::Result<usize> {
        if self.panes.len() >= self.max_panes {
            return Ok(self.active);
        }
        let cwd = self
            .panes
            .get(self.active)
            .map(|p| p.cwd.clone())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let (command, agent_name) = self.command_for(launch);
        self.panes.push(Pane::new(
            &*self.pty_system,
            size,
            &command,
            agent_name.as_deref(),
            &cwd,
            shared,
        )?);
        Ok(self.panes.len() - 1)
    }

    /// Kill the active pane's child process.
    pub fn kill_active(&mut self) {
        if let Some(pane) = self.panes.get_mut(self.active) {
            if pane.alive {
                let _ = pane.child.kill();
            }
        }
    }

    /// Move the active pane left/right, wrapping around.
    pub fn cycle(&mut self, forward: bool) {
        if self.panes.len() < 2 {
            return;
        }
        let len = self.panes.len();
        self.active = if forward {
            (self.active + 1) % len
        } else {
            (self.active + len - 1) % len
        };
    }

    pub fn all_dead(&self) -> bool {
        self.panes.iter().all(|p| !p.alive)
    }

    /// Send the active pane's visible screen into pane `target`. No-op unless
    /// both sides are alive and on the context bus. The payload is wrapped in
    /// bracketed-paste markers so the target's shell/agent takes it as one
    /// paste, and no Enter is sent — the text lands at the target's prompt and
    /// the user decides whether to send it.
    pub fn send_to(&mut self, target: usize) {
        let source = self.active;
        let payload = match self.panes.get(source) {
            Some(p) if relayable(p.shared, p.alive, source, target) => {
                let name = p.agent_name.clone().unwrap_or_else(|| "shell".to_string());
                format!(
                    "--- from pane {} ({}) ---\n{}\n--- end ---\n",
                    source + 1,
                    name,
                    p.screen_text()
                )
            }
            _ => return,
        };
        // Re-check the target now that the source borrow is over.
        if !self
            .panes
            .get(target)
            .is_some_and(|t| relayable(t.shared, t.alive, target, source))
        {
            return;
        }
        if let Some(t) = self.panes.get_mut(target) {
            let _ = t.writer.write_all(&paste_wrap(&payload));
        }
    }

    /// Send the active pane's visible screen to **every** other shared, alive
    /// pane at once. No-op unless the active pane itself is alive and shared.
    pub fn broadcast(&mut self) {
        let source = self.active;
        let payload = match self.panes.get(source) {
            Some(p) if p.shared && p.alive => {
                let name = p.agent_name.clone().unwrap_or_else(|| "shell".to_string());
                format!(
                    "--- from pane {} ({}) ---\n{}\n--- end ---\n",
                    source + 1,
                    name,
                    p.screen_text()
                )
            }
            _ => return,
        };
        for (i, pane) in self.panes.iter_mut().enumerate() {
            if relayable(pane.shared, pane.alive, i, source) {
                let _ = pane.writer.write_all(&paste_wrap(&payload));
            }
        }
    }

    /// Append the active pane's visible screen to `CONTEXT.md` in the launch
    /// directory, under a `## <timestamp> — pane <i> (<name>)` header. No-op
    /// unless the active pane is alive and shared.
    pub fn dump_context(&mut self) {
        let (text, name) = match self.panes.get(self.active) {
            Some(p) if p.shared && p.alive => (
                p.screen_text(),
                p.agent_name.clone().unwrap_or_else(|| "shell".to_string()),
            ),
            _ => return,
        };
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = self.launch_dir.join("CONTEXT.md");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(file, "\n## {} — pane {} ({})\n", ts, self.active + 1, name);
            let _ = writeln!(file, "{}", text);
        }
    }

    /// Type the shared-context pointer line into pane `index` (no Enter).
    /// Shared panes only — an isolated pane is off the bus and gets nothing.
    pub fn note_shared_context(&mut self, index: usize) {
        if let Some(pane) = self.panes.get_mut(index) {
            if pane.shared {
                let note = format!(
                    "shared context: {}\n",
                    self.launch_dir.join("CONTEXT.md").display()
                );
                let _ = pane.writer.write_all(note.as_bytes());
            }
        }
    }
}

pub struct App {
    pub panes: Vec<Pane>,
    pub active: usize,
    pty_system: Box<dyn PtySystem + Send>,
    max_panes: usize,
    shell: Option<Shell>,
    /// Configured agents (the `[[agents]]` config section), by picker index.
    pub agents: Vec<Agent>,
    /// The directory the app was launched from; `CONTEXT.md` lives here.
    launch_dir: PathBuf,
}

/// Alacritty's event listener for a pane. Relays `Event::PtyWrite` back into
/// the PTY so the terminal's responses to escape-sequence queries reach the
/// child. On Windows the ConPTY console host sends a lone `\x1b[6n` (cursor
/// position report request) on startup and stalls until it gets a
/// `\x1b[row;colR` reply; alacritty computes that reply in its `device_status`
/// handler and emits it here, which is what unblocks the whole I/O round-trip.
///
/// The other event variants (title, bell, clipboard, ...) are ignored, matching
/// the old `VoidListener` behavior.
pub struct PtyWriter {
    tx: mpsc::Sender<String>,
}

impl EventListener for PtyWriter {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event {
            let _ = self.tx.send(text);
        }
    }
}

pub struct Pane {
    /// All mutation happens on the main thread.
    pub term: Term<PtyWriter>,
    /// Drives `term` (a `vte::ansi::Handler`) from raw PTY bytes.
    pub processor: vte::ansi::Processor,
    bytes: mpsc::Receiver<Vec<u8>>,
    /// PTY writes alacritty requested (e.g. CPR responses), drained in `pump`.
    pty_rx: mpsc::Receiver<String>,
    /// From `master.take_writer()` — taken once, dropped on exit (sends EOF).
    pub writer: Box<dyn Write + Send>,
    /// Kept for `resize`.
    pub master: Box<dyn MasterPty + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    pub alive: bool,
    pub exit_status: Option<ExitStatus>,
    /// Whether this pane is on the context bus: it may send/receive screen
    /// relays and its screen may be dumped to `CONTEXT.md`. The first pane is
    /// shared; later panes are chosen at spawn time.
    pub shared: bool,
    /// The configured agent's display name, for the border title. `None`
    /// for a plain shell.
    pub agent_name: Option<String>,
    /// The directory this pane's shell is believed to be in. Seeded with the
    /// launch dir and updated as the user types `cd` commands (see
    /// [`Pane::observe_key`]). A new pane starts here, so panes "follow" the
    /// directory of the pane they were opened from.
    pub cwd: PathBuf,
    /// The command line the user is currently typing, one char at a time.
    /// Cleared on Enter (after any `cd` is applied) and on Ctrl/Alt keys,
    /// which reset the shell's line editor.
    line: String,
    /// When the pane's PTY last delivered output bytes (stamped in
    /// [`App::pump`]). A fresh pane is seeded with this far enough in the
    /// past that it reads [`PaneStatus::Idle`] immediately — a shell at a
    /// prompt *is* idle — and only flips to Working once it emits bytes.
    last_activity: Instant,
}

impl Pane {
    /// Open a PTY of `size`, spawn `command` on it, and start a reader thread
    /// that ships raw bytes over an mpsc channel. `agent_name` is the
    /// configured agent's display name (for the border title) or `None` for a
    /// plain shell; `shared` puts the pane on the context bus.
    fn new(
        pty_system: &dyn PtySystem,
        size: TermSize,
        command: &CommandBuilder,
        agent_name: Option<&str>,
        cwd: &Path,
        shared: bool,
    ) -> anyhow::Result<Self> {
        let PtyPair { slave, master } = pty_system.openpty(PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Start the shell in `cwd`. On Windows the default shell would
        // otherwise fall back to %USERPROFILE% (see portable-pty's
        // `CommandBuilder::current_directory`), so an explicit cwd is what
        // pins a pane to a specific folder.
        let mut command = command.clone();
        command.cwd(cwd);
        let child = slave.spawn_command(command)?;
        let reader = master.try_clone_reader()?;
        let writer = master.take_writer()?;

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let term_config = TermConfig {
            scrolling_history: 0,
            ..Default::default()
        };
        let (pty_tx, pty_rx) = mpsc::channel::<String>();
        let term = Term::new(term_config, &size, PtyWriter { tx: pty_tx });

        Ok(Self {
            term,
            processor: vte::ansi::Processor::new(),
            bytes: rx,
            pty_rx,
            writer,
            master,
            child,
            alive: true,
            exit_status: None,
            shared,
            agent_name: agent_name.map(str::to_string),
            cwd: cwd.to_path_buf(),
            line: String::new(),
            last_activity: Instant::now() - IDLE_AFTER,
        })
    }

    /// This pane's current activity state (see [`PaneStatus`]).
    pub fn status(&self) -> PaneStatus {
        classify(self.alive, self.last_activity.elapsed())
    }

    /// The pane's visible screen as plain text: one line per grid row,
    /// trailing spaces trimmed, trailing blank lines dropped. This is what a
    /// context relay / `CONTEXT.md` dump carries.
    pub fn screen_text(&self) -> String {
        screen_text(&self.term)
    }

    /// Feed a keystroke into this pane's command-line tracker. On Enter, if
    /// the line is a `cd`/`pushd` command, [`Pane::cwd`] is updated to where it
    /// points. This is a best-effort shadow of the shell's own line editor:
    /// it tracks directly-typed commands, so a pane's `cwd` stays honest for
    /// the common "type `cd <path>` and hit Enter" flow, and degrades
    /// gracefully (last known dir) when the user edits the line with arrow
    /// keys or history recall.
    pub fn observe_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Char(c) => {
                if key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                    // Ctrl/Alt+letter resets the shell's line editor, so the
                    // shadow line is no longer what the shell sees.
                    self.line.clear();
                } else {
                    self.line.push(c);
                }
            }
            KeyCode::Backspace => {
                self.line.pop();
            }
            KeyCode::Enter if !self.line.is_empty() => {
                apply_cd(&mut self.cwd, &self.line);
                self.line.clear();
            }
            _ => {}
        }
    }
}

/// Extract the visible grid of `term` as plain text. Kept as a free
/// function (rather than only a `Pane` method) so it is unit-testable against
/// a bare `Term` with no live PTY. v1 has no scrollback, so the grid is
/// exactly the visible screen: bucket each cell's char by its row, skip
/// hidden/wide-spacer cells, trim trailing spaces per line, and drop trailing
/// blank lines.
fn screen_text(term: &Term<PtyWriter>) -> String {
    let content = term.renderable_content();
    let mut lines: Vec<String> = Vec::new();
    for indexed in content.display_iter {
        if indexed
            .cell
            .flags
            .contains(Flags::HIDDEN | Flags::WIDE_CHAR_SPACER)
        {
            continue;
        }
        let row = indexed.point.line.0 as usize;
        while lines.len() <= row {
            lines.push(String::new());
        }
        lines[row].push(indexed.cell.c);
    }
    while lines.last().is_some_and(|l| l.trim_end().is_empty()) {
        lines.pop();
    }
    lines.iter()
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wrap a relayed payload in bracketed-paste markers so the target's shell or
/// agent TUI takes the whole dump as one paste rather than a burst of
/// keystrokes. No trailing Enter is added — the text lands at the target's
/// prompt and the user decides whether to send it.
fn paste_wrap(payload: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(payload.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(payload.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

/// Whether pane `index` may take part in a relay whose source is pane
/// `source`: it must be on the context bus, alive, and not the source itself.
/// Kept as a pure predicate so the filter is unit-testable.
pub(crate) fn relayable(shared: bool, alive: bool, index: usize, source: usize) -> bool {
    shared && alive && index != source
}

/// Classify a pane's activity from whether it is alive and how long since its
/// PTY last produced output. Kept as a pure function so the boundary is
/// unit-testable without a live PTY; [`Pane::status`] delegates here.
fn classify(alive: bool, elapsed: Duration) -> PaneStatus {
    if !alive {
        PaneStatus::Dead
    } else if elapsed < IDLE_AFTER {
        PaneStatus::Working
    } else {
        PaneStatus::Idle
    }
}

/// If `line` is a `cd`/`pushd` command, set `cwd` to the directory it points
/// to and return `true`; otherwise leave `cwd` untouched and return `false`.
/// Handles relative paths (joined onto the current `cwd`), `~`/`~user`
/// (expanded against the home dir), absolute paths, and bare `cd` (home).
fn apply_cd(cwd: &mut PathBuf, line: &str) -> bool {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let cmd = match tokens.first() {
        Some(c) => c.to_lowercase(),
        None => return false,
    };
    if cmd != "cd" && cmd != "pushd" {
        return false;
    }
    // Bare `cd` → home.
    if tokens.len() < 2 {
        if let Some(home) = dirs::home_dir() {
            *cwd = home;
            return true;
        }
        return false;
    }
    // `cd /d <path>` (cmd.exe) — drop the flag, keep the path.
    let rest = &tokens[1..];
    let path = if matches!(rest.first().copied(), Some("/d") | Some("-d")) {
        rest[1..].join(" ")
    } else {
        rest.join(" ")
    };
    if path.is_empty() {
        return false;
    }
    *cwd = resolve_path(cwd, &path);
    true
}

/// Resolve `p` against `base`: `~`/`~user` expands to the home dir, an
/// absolute path is used as-is, and a relative path is joined onto `base`.
fn resolve_path(base: &Path, p: &str) -> PathBuf {
    let p = p.trim().trim_matches('"').trim();
    if let Some(rest) = p.strip_prefix('~') {
        let home = dirs::home_dir().unwrap_or_else(|| base.to_path_buf());
        return home.join(rest.trim_start_matches('/').trim_start_matches('\\'));
    }
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use portable_pty::native_pty_system;

    /// The Windows ConPTY host emits a lone `\x1b[6n` (cursor position report
    /// request) on startup and stalls until it receives a `\x1b[row;colR`
    /// reply. This drives the real `App`/`Pane` pump loop and asserts the
    /// shell's output actually reaches the term grid — i.e. the I/O round-trip
    /// is unblocked. Regression test for the "empty black screen" bug: before
    /// the `PtyWriter` listener relayed alacritty's CPR response, the grid
    /// stayed blank forever.
    #[test]
    fn pump_unblocks_conpty_roundtrip() {
        let size = TermSize::new(24, 80);
        let mut app = App::new(native_pty_system(), size, &Config::default()).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut saw_content = false;
        while !saw_content && std::time::Instant::now() < deadline {
            app.pump();
            app.check_children();
            saw_content = app.panes.iter().any(|p| {
                let mut content = p.term.renderable_content();
                content.display_iter.any(|c| c.cell.c != ' ')
            });
            if !saw_content {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }

        // Kill the shell before asserting so a failure doesn't orphan it.
        for p in &mut app.panes {
            if p.alive {
                let _ = p.child.kill();
            }
        }

        assert!(
            saw_content,
            "shell output never reached the term grid — ConPTY round-trip stalled"
        );
    }

    // --- context sharing (screen_text / paste_wrap / relayable) ---

    #[test]
    fn screen_text_extracts_visible_rows() {
        let (tx, _rx) = mpsc::channel::<String>();
        let size = TermSize::new(5, 10);
        let mut term = Term::new(
            TermConfig {
                scrolling_history: 0,
                ..Default::default()
            },
            &size,
            PtyWriter { tx },
        );
        let mut processor: vte::ansi::Processor = vte::ansi::Processor::new();
        processor.advance(&mut term, b"hello\r\nworld");
        assert_eq!(screen_text(&term), "hello\nworld");
    }

    #[test]
    fn paste_wrap_brackets_the_payload() {
        let bytes = paste_wrap("payload text");
        assert!(bytes.starts_with(b"\x1b[200~"));
        assert!(bytes.ends_with(b"\x1b[201~"));
        assert_eq!(&bytes[6..bytes.len() - 6], b"payload text");
    }

    #[test]
    fn relayable_requires_shared_alive_and_not_source() {
        assert!(relayable(true, true, 1, 0));
        assert!(!relayable(false, true, 1, 0));
        assert!(!relayable(true, false, 1, 0));
        assert!(!relayable(true, true, 0, 0));
    }

    // --- activity classification (classify) ---

    #[test]
    fn classify_alive_and_fresh_is_working() {
        assert_eq!(
            classify(true, Duration::from_millis(100)),
            PaneStatus::Working
        );
    }

    #[test]
    fn classify_alive_and_stale_is_idle() {
        assert_eq!(
            classify(true, IDLE_AFTER + Duration::from_millis(1)),
            PaneStatus::Idle
        );
    }

    #[test]
    fn classify_dead_overrides_recency() {
        // A dead pane reads Dead regardless of how fresh its last output was.
        assert_eq!(classify(false, Duration::from_millis(0)), PaneStatus::Dead);
    }

    // --- cwd tracking (apply_cd / resolve_path) ---

    #[test]
    fn apply_cd_relative_joins_onto_cwd() {
        let base = std::env::temp_dir();
        let mut cwd = base.clone();
        assert!(apply_cd(&mut cwd, "cd sub/dir"));
        assert_eq!(cwd, base.join("sub/dir"));
    }

    #[test]
    fn apply_cd_ignores_non_cd_commands() {
        let mut cwd = PathBuf::from("/somewhere");
        assert!(!apply_cd(&mut cwd, "ls -la"));
        assert!(!apply_cd(&mut cwd, "git status"));
        assert_eq!(cwd, PathBuf::from("/somewhere"));
    }

    #[test]
    fn apply_cd_no_arg_goes_home() {
        let mut cwd = PathBuf::from("/somewhere");
        assert!(apply_cd(&mut cwd, "cd"));
        // Home is an absolute path, so the result must not be joined onto cwd.
        assert!(cwd.is_absolute());
    }

    #[test]
    fn apply_cd_tilde_expands_to_home() {
        let mut cwd = PathBuf::from("/somewhere");
        assert!(apply_cd(&mut cwd, "cd ~"));
        assert!(cwd.is_absolute());
        assert!(cwd != *"/somewhere");
    }

    #[test]
    fn apply_cd_absolute_replaces_cwd() {
        let abs = if cfg!(windows) { "C:\\abs\\path" } else { "/abs/path" };
        let mut cwd = PathBuf::from("/somewhere");
        assert!(apply_cd(&mut cwd, &format!("cd {abs}")));
        assert!(cwd.is_absolute());
        assert_eq!(cwd, Path::new(abs));
    }

    #[test]
    fn apply_cd_pushd_and_quoted_path() {
        let base = std::env::temp_dir();
        let mut cwd = base.clone();
        assert!(apply_cd(&mut cwd, "pushd \"my folder\""));
        assert_eq!(cwd, base.join("my folder"));
    }

    #[test]
    fn resolve_path_relative_and_absolute() {
        let base = std::env::temp_dir();
        assert_eq!(resolve_path(&base, "a/b"), base.join("a/b"));
        let abs = if cfg!(windows) { "C:\\x" } else { "/x" };
        assert_eq!(resolve_path(&base, abs), Path::new(abs).to_path_buf());
    }
}
