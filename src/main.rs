//! tuiterminals — a TUI hosting a grid of terminals, each running its own
//! shell in a PTY, rendered through alacritty_terminal's cell grid.

mod app;
mod config;
mod input;
mod render;
mod resources;
mod ui;

use std::io::stdout;
use std::time::Duration;

use app::{App, Launch, PaneStatus, TermSize};
use config::Config;
use ui::{Modal, ModalAction, NewAgentStep};
use resources::Sampler;
use crossterm::event;
use crossterm::event::{Event, KeyEventKind};
use crossterm::terminal;
use crossterm::terminal::SetTitle;
use crossterm::{cursor, ExecutableCommand};
use portable_pty::native_pty_system;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

type AppTerminal = Terminal<CrosstermBackend<std::io::Stdout>>;

/// The config file path: `--config <path>` if given, else the platform
/// default (`~/.config/tuiterminals/config.toml` on Unix,
/// `%APPDATA%\tuiterminals\config.toml` on Windows).
fn config_path_from_args() -> anyhow::Result<std::path::PathBuf> {
    let default = dirs::config_dir()
        .map(|d| d.join("tuiterminals").join("config.toml"))
        .unwrap_or_else(|| std::path::PathBuf::from("config.toml"));

    let mut args = std::env::args();
    let mut path = default;
    while let Some(arg) = args.next() {
        if arg == "--config" {
            path = args
                .next()
                .map(std::path::PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("--config requires a path argument"))?;
        }
    }
    Ok(path)
}

fn main() -> anyhow::Result<()> {
    // Load config before touching the terminal so a malformed file fails
    // cleanly without corrupting the outer terminal.
    let config = Config::load(&config_path_from_args()?)?;

    terminal::enable_raw_mode()?;
    let mut term = CrosstermBackend::new(stdout());
    term.execute(terminal::EnterAlternateScreen)?;
    term.execute(cursor::Hide)?;

    let mut terminal = Terminal::new(term)?;

    // Restore the outer terminal even if we panic mid-frame.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut out = std::io::stdout();
        let _ = out.execute(terminal::LeaveAlternateScreen);
        let _ = out.execute(cursor::Show);
        let _ = out.execute(SetTitle(""));
        let _ = terminal::disable_raw_mode();
        default_hook(info);
    }));

    let result = run(&mut terminal, &config);

    let mut out = std::io::stdout();
    let _ = out.execute(terminal::LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    let _ = out.execute(SetTitle(""));
    let _ = terminal::disable_raw_mode();

    result
}

fn run(terminal: &mut AppTerminal, config: &Config) -> anyhow::Result<()> {
    let pty_system = native_pty_system();

    let size = terminal.size()?;
    let outer = render::pane_area(Rect::new(0, 0, size.width, size.height));
    let first_size = TermSize::new(
        render::inner_area(outer).height,
        render::inner_area(outer).width,
    );
    let mut app = App::new(pty_system, first_size, config)?;

    let mut sampler = Sampler::new();
    // The OS window/tab title doubles as an out-of-window nag: how many panes
    // are idle. Tracked so we only re-emit it when the count actually changes.
    let mut last_title = String::new();

    // The open modal prompt, if any. While one is open, every key press
    // routes to it (Esc cancels); nothing reaches the global bindings or the
    // panes.
    let mut modal = Modal::None;

    loop {
        app.pump();
        app.check_children();

        if event::poll(Duration::from_millis(100)).unwrap_or(false) {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if modal != Modal::None {
                        // A modal is open: every key routes to it. A key that
                        // doesn't complete the flow is swallowed.
                        let targets = ui::send_targets(&app, app.active);
                        if let Some(action) = modal.handle_key(&key, &config.agents, &targets) {
                            match action {
                                ModalAction::SpawnShell { shared } => {
                                    if let Some(index) = spawn_pane(
                                        terminal,
                                        &mut app,
                                        config.max_panes,
                                        &Launch::Shell,
                                        shared,
                                    )? {
                                        if shared {
                                            app.note_shared_context(index);
                                        }
                                    }
                                }
                                ModalAction::SpawnAgent { index, shared } => {
                                    if let Some(new) = spawn_pane(
                                        terminal,
                                        &mut app,
                                        config.max_panes,
                                        &Launch::Agent(index),
                                        shared,
                                    )? {
                                        if shared {
                                            app.note_shared_context(new);
                                        }
                                    }
                                }
                                ModalAction::Send { target } => app.send_to(target),
                                ModalAction::Cancel => {}
                            }
                            modal = Modal::None;
                        }
                    } else {
                        match input::handle_global(&key, &config.keybindings) {
                            Some(input::GlobalKey::NewPane) => {
                                if app.panes.len() < config.max_panes {
                                    modal = Modal::NewPaneShare;
                                }
                            }
                            Some(input::GlobalKey::KillPane) => app.kill_active(),
                            Some(input::GlobalKey::Quit) => break,
                            Some(input::GlobalKey::PrevPane) => app.cycle(false),
                            Some(input::GlobalKey::NextPane) => app.cycle(true),
                            Some(input::GlobalKey::NewAgent) => {
                                if app.panes.len() < config.max_panes {
                                    modal = Modal::NewAgent {
                                        step: NewAgentStep::Pick,
                                        chosen: None,
                                    };
                                }
                            }
                            Some(input::GlobalKey::SendContext) => {
                                if app
                                    .panes
                                    .get(app.active)
                                    .is_some_and(|p| p.shared && p.alive)
                                    && !ui::send_targets(&app, app.active).is_empty()
                                {
                                    modal = Modal::SendTarget { source: app.active };
                                }
                            }
                            Some(input::GlobalKey::BroadcastContext) => {
                                if app
                                    .panes
                                    .get(app.active)
                                    .is_some_and(|p| p.shared && p.alive)
                                {
                                    app.broadcast();
                                }
                            }
                            Some(input::GlobalKey::DumpContext) => {
                                if app
                                    .panes
                                    .get(app.active)
                                    .is_some_and(|p| p.shared && p.alive)
                                {
                                    app.dump_context();
                                }
                            }
                            None => {
                                if let Some(pane) = app.panes.get_mut(app.active) {
                                    if pane.alive {
                                        // Track the keystroke so a later new pane can
                                        // start in this pane's directory (see Pane::cwd).
                                        pane.observe_key(&key);
                                        let bytes = input::key_to_bytes(&key);
                                        if !bytes.is_empty() {
                                            let _ = pane.writer.write_all(&bytes);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Event::Resize(_, _) => reflow(terminal, &mut app)?,
                _ => {}
            }
        }

        if app.all_dead() {
            break;
        }

        let res = sampler.tick();

        // Update the window title to nag about idle panes, only on change.
        // Best-effort: a terminal that ignores OSC 0 must not kill the loop.
        let n_idle = app
            .panes
            .iter()
            .filter(|p| p.status() == PaneStatus::Idle)
            .count();
        let title = if n_idle > 0 {
            format!("tuiterminals · {n_idle} idle")
        } else {
            "tuiterminals".to_string()
        };
        if title != last_title {
            let _ = terminal.backend_mut().execute(SetTitle(&title));
            last_title = title;
        }

        terminal.draw(|frame| render::draw_panes(frame, &app, &config.colors, res, &modal))?;
    }

    Ok(())
}

/// Spawn a new pane running `launch` in the last slot of the (len+1) layout
/// and make it active. Returns the new index, or `None` when the pane cap is
/// reached (in which case nothing is spawned).
fn spawn_pane(
    terminal: &mut AppTerminal,
    app: &mut App,
    max_panes: usize,
    launch: &Launch,
    shared: bool,
) -> anyhow::Result<Option<usize>> {
    if app.panes.len() >= max_panes {
        return Ok(None);
    }
    let size = terminal.size()?;
    let rects = render::layout_panes(
        render::pane_area(Rect::new(0, 0, size.width, size.height)),
        app.panes.len() + 1,
    );
    let inner = render::inner_area(*rects.last().unwrap());
    let index = app.add_pane(TermSize::new(inner.height, inner.width), launch, shared)?;
    app.active = index;
    reflow(terminal, app)?;
    Ok(Some(index))
}

/// Recompute the pane layout for the current terminal size and resize every
/// pane's term grid and PTY to match.
fn reflow(terminal: &mut AppTerminal, app: &mut App) -> anyhow::Result<()> {
    let size = terminal.size()?;
    let rects = render::layout_panes(
        render::pane_area(Rect::new(0, 0, size.width, size.height)),
        app.panes.len(),
    );
    let sizes: Vec<TermSize> = rects
        .iter()
        .map(|r| {
            let inner = render::inner_area(*r);
            TermSize::new(inner.height, inner.width)
        })
        .collect();
    app.resize_panes(&sizes);
    Ok(())
}
