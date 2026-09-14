# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`tuiterminals` is a Rust terminal multiplexer: a TUI (ratatui) that hosts multiple
pseudo-terminals, each running a shell, with a VT100 parser (alacritty_terminal)
rendering each pane. The name is "TUI" + "terminals".

**Current state:** a working v1. The app opens a grid of panes (1–4), each running its
own shell — or a configured agent — in a PTY, and renders each pane's alacritty cell
grid. Global keybindings manage the panes (new / kill / quit / cycle), and panes can
share context with each other — see [Context sharing](#context-sharing). A TOML config
file makes the shell, agents, keybindings, border colors, and pane cap configurable — see
[Configuration](#configuration).

## Commands

Standard cargo workflow (no custom scripts, no lint config yet):

```bash
cargo build        # debug build
cargo run          # run the app (uses the default config path)
cargo run -- --config <path>   # run with an explicit config file
cargo test         # run the test suite (config.rs + app.rs)
cargo clippy       # lints (no clippy config; defaults apply)
cargo build --release
```

Tests live in four modules, all run by `cargo test`:
- `src/config.rs` — key-combo parsing, color parsing, and `Config::load` against temp
  files (fast, no I/O beyond temp files).
- `src/app.rs` — the `apply_cd`/`resolve_path` cwd-tracking logic, the
  `screen_text`/`paste_wrap`/`relayable` context-sharing helpers, plus
  `pump_unblocks_conpty_roundtrip`, a **real integration test** that spawns a live shell
  in a PTY and pumps it for up to 5s until output reaches the grid. It is the regression
  guard for the ConPTY stall (see [Windows ConPTY quirk](#windows-conpty-quirk)) and is
  the slow test in the suite.
- `src/ui.rs` — every modal transition (share/isolate, agent picker, send-target) and
  the prompt-line strings.
- `src/resources.rs` — the `nvidia-smi` VRAM output parser.

No CI and no formatter config yet. Run a single test with `cargo test <name>` (e.g.
`cargo test apply_cd`).

`examples/pty_probe.rs` is a standalone diagnostic (`cargo run --example pty_probe`) that
opens a PTY exactly like the app and probes the ConPTY I/O round-trip in phases, bypassing
ratatui/crossterm — useful for isolating whether a stall is in the PTY/shell layer or the
rendering layer.

## Architecture

Six modules, one per concern. The data flow that ties them together:
`portable-pty` → byte stream (mpsc channel from a per-pane reader thread) →
`alacritty_terminal` (parse into a cell grid) → `ratatui` (draw cells into a pane).
`crossterm` events drive the main loop and forward keystrokes back into the active PTY.

- **`src/main.rs`** — entry point and the event loop. Sets up raw mode + alternate
  screen, installs a panic hook that restores the outer terminal, then loops:
  `app.pump()` (drain PTY output into each term) → `app.check_children()` (reap
  exited shells) → poll crossterm → route the event → draw. `reflow()` recomputes the
  pane layout and resizes every pane's grid + PTY on terminal resize. While a modal is
  open (see `src/ui.rs`) every key press routes to it instead of the global bindings or
  the panes; a completed modal action spawns a pane or sends a relay.
- **`src/app.rs`** — `App` (the pane list, active index, PTY system, `max_panes`, the
  configured `shell`, the configured `agents`, and the `launch_dir` where `CONTEXT.md`
  lives) and `Pane` (an alacritty `Term`, a `vte` `Processor`, the reader channel, the
  PTY writer/master, the child handle, plus `shared` — is the pane on the context bus —
  and `agent_name` for the border title). `TermSize` implements alacritty's
  `Dimensions`. v1 has no scrollback (`scrolling_history: 0`), so the grid is exactly
  the visible screen.
  - **cwd tracking** — each `Pane` keeps a shadow `cwd` and a per-keystroke `line`
    buffer. `Pane::observe_key` (called from `main.rs` before forwarding a key to the
    active pane) mirrors the shell's line editor: on Enter, if the line is a `cd`/`pushd`
    command, `apply_cd`/`resolve_path` update `cwd` (handling relative paths, `~`/`~user`,
    absolute paths, bare `cd` → home, and `cd /d <path>`). A new pane is then spawned in
    the active pane's `cwd`, so a pane opened right after `cd`-ing lands in the same
    directory. It is best-effort: it tracks directly-typed commands and degrades to the
    last known dir on arrow-key edits or history recall.
  - **`PtyWriter`** — the alacritty `EventListener` for a pane. It relays
    `Event::PtyWrite` back into the PTY so the terminal's replies to escape-sequence
    queries reach the child. This is what unblocks the Windows ConPTY stall — see below.
  - **Context sharing** — `Pane::screen_text` extracts the visible grid (the
    `renderable_content`/`display_iter` pattern from `render.rs`); `paste_wrap` wraps a
    payload in bracketed-paste markers; `App::send_to`/`broadcast` write the wrapped
    screen text into other shared, alive panes' `writer`; `dump_context` appends it to
    `CONTEXT.md` in the launch dir; `note_shared_context` types a pointer line into a
    newly spawned shared pane. The `relayable` predicate keeps the target filter
    unit-testable.
- **`src/input.rs`** — `handle_global` matches a key against the configured
  `Keybindings` (first match wins) to decide app-level actions; `key_to_bytes` encodes
  any other key press into the bytes a terminal would send to the shell (Ctrl+letter →
  ASCII control codes, arrows/F-keys → escape sequences, Alt → ESC prefix).
- **`src/ui.rs`** — the modal state machine: `Modal` (the share/isolate question, the
  agent picker, the send-target picker), `ModalAction`, `handle_key` (routes a key
  press to the open modal — Esc always cancels, unknown keys are swallowed), and
  `prompt` (the overlay line(s) drawn at the bottom of the frame). Pure and fully
  unit-tested.
- **`src/render.rs`** — `layout_panes` (1 fills, 2 splits, 3–4 form a 2×2 grid),
  `draw_panes` (borders with active/inactive colors + each pane's content), the top
  resource bar, the bottom modal prompt overlay, and the alacritty→ratatui
  color/flag translation.
- **`src/resources.rs`** — the top resource bar's sampler: `sysinfo` for CPU/RAM and an
  off-main-thread `nvidia-smi` probe for VRAM (1s gate, non-blocking).
- **`src/config.rs`** — the TOML config subsystem (below).

## Windows ConPTY quirk

On Windows the ConPTY console host sends a lone `\x1b[6n` (cursor position report request)
on startup and **stalls until it gets a `\x1b[row;colR` reply**. If that reply never
reaches the shell, the whole I/O round-trip deadlocks and every pane renders as an empty
black screen. alacritty computes the CPR reply in its `device_status` handler and emits it
as `Event::PtyWrite`; the `PtyWriter` listener in `app.rs` is what relays it back into the
PTY. If you touch the event-listener or the `pump` loop, keep this path intact — the
`pump_unblocks_conpty_roundtrip` test exists specifically to catch a regression here.

## Configuration

Config is loaded from `--config <path>` if given, else the platform default
(`~/.config/tuiterminals/config.toml` on Unix, `%APPDATA%\tuiterminals\config.toml`
on Windows, via the `dirs` crate). Loading happens **before** the terminal is touched,
so a malformed file errors out cleanly without corrupting the outer terminal.

Every field is optional: a missing file yields `Config::default()`, and a partial file
fills the absent fields with the same defaults — so no config file means the app
behaves exactly as it did before config existed. A present-but-malformed file is a
hard error.

The five knobs:

- **`[shell]`** — `program` + `args`; each pane spawns this. Absent → the platform
  default shell (`CommandBuilder::new_default_prog()`).
- **`[agents]`** — a list of named agent programs (`name`, `program`, `args`), written
  as repeated `[[agents]]` tables. Each is spawnable into a pane from the agent picker
  (see [Context sharing](#context-sharing)). Absent → the picker offers a plain shell
  only.
- **`[keybindings]`** — the nine global actions (`new_pane`, `kill_pane`, `quit`,
  `prev_pane`, `next_pane`, `new_agent`, `send_context`, `broadcast_context`,
  `dump_context`), each a key-combo string like `"ctrl+n"` or `"ctrl+shift+s"`. Parsed
  by `parse_combo`; a binding matches only when the key code and all four modifier bits
  match exactly (so `ctrl+n` does not also fire on `shift+ctrl+n`).
- **`[colors]`** — `active_border` / `inactive_border`, each a `"#rrggbb"` string.
  These retheme the pane chrome only; a program's own cell colors still come from the
  alacritty palette.
- **`max_panes`** — the pane cap (default 4).

Example:

```toml
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
args = []

[keybindings]
new_pane          = "ctrl+n"
kill_pane         = "ctrl+k"
quit              = "ctrl+q"
prev_pane         = "ctrl+left"
next_pane         = "ctrl+right"
new_agent         = "ctrl+shift+n"
send_context      = "ctrl+shift+s"
broadcast_context = "ctrl+shift+b"
dump_context      = "ctrl+shift+c"

[colors]
active_border   = "#00ffff"
inactive_border = "#444444"
```

## Context sharing

Panes can share context with each other through a "context bus". Every pane is either
**shared** (on the bus) or **isolated** (off it). The border title shows membership at a
glance: an isolated pane's title carries `(private)`, and an agent pane carries
`· <agent name>`. The **first pane is always shared** — it is the root of the bus.
Isolated panes neither send nor receive; they are invisible to the relay.

**Share/isolate choice at every spawn.** Both `new_pane` and `new_agent` open a modal
that first asks whether the new pane shares context (`[y] share   [n] isolate   (esc)
cancel`). The `new_agent` binding goes through a two-step flow: first a picker lists the
configured `[[agents]]` by number (plus `[s]` for a plain shell, which defers to the
share question), then the chosen agent gets the same y/n share question.

**The modal.** While a modal is open, *every* key press routes to it — nothing reaches
the global bindings or the panes. Esc always cancels; any other key that doesn't complete
the flow is swallowed. The prompt line is drawn in a black bar pinned to the bottom of
the frame (see `src/ui.rs`).

**Relaying context.** A shared pane's visible screen text can be sent to other panes:
- **Targeted** — `send_context` opens a picker of the other shared, alive panes (numbered
  in that filtered order); the active pane's screen text is written into the chosen
  pane's PTY.
- **Broadcast** — `broadcast_context` sends to every other shared, alive pane at once.
- **Dump** — `dump_context` appends the active pane's screen to `CONTEXT.md` in the
  launch directory (the directory the app was started from), under a
  `## <timestamp> — pane <n> (<name>)` header.

Relayed text is wrapped in **bracketed-paste markers** (`\x1b[200~ … \x1b[201~`) so agent
TUIs treat the dump as a single paste, and **no trailing Enter is sent** — the text lands
at the target's prompt and the user decides whether to send it. When a *shared* pane is
spawned, a pointer line (`shared context: <launch_dir>/CONTEXT.md`) is typed into it so
the agent knows where the shared file lives.

## Build profile

`[profile.release]` is tuned for a small single-binary TUI: `opt-level = 2`,
`lto = "thin"`, `codegen-units = 1`, `debug = false`. Don't change these casually —
they are a deliberate size/speed tradeoff for a binary that ships as one file.
