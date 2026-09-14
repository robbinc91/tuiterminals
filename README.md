# tuiterminals

A terminal multiplexer that lives *inside* your terminal. `tuiterminals` is a
Rust TUI that hosts multiple pseudo-terminals side by side — each running its own
shell, or a configured agent — and renders every pane's live output. The name is
**TUI** + **terminals**.

Instead of a separate window manager, you get a grid of real PTYs in one screen:
split it into 1–4 (or more) panes, run a shell in each, and let them share context
with one another.

```
 SYS   CPU  12%   RAM 3.2/16.0 GB   VRAM 1.1/8.0 GB
 APP   CPU   2%   RAM 0.05 GB   VRAM n/a
┌────────────────────────────────────────┬────────────────────────────────────────┐
│ ● working                               │ ● idle · claude                         │
│                                        │                                          │
│ PS C:\dev> git status                   │ > build the VT parser                   │
│ On branch main                         │   thinking…                             │
│ nothing to commit, working tree clean  │                                          │
│                                        │                                          │
└────────────────────────────────────────┴────────────────────────────────────────┘
```

*(Illustrative mockup. Each pane's border carries a status dot — `● working` in
green, `● idle` in yellow, `✕ dead` in gray — plus the agent's name and a
`(private)` marker when the pane is off the context bus.)*

## Features

- **A grid of live panes.** 1 pane fills the screen, 2 split it, 3–4 form a 2×2
  grid. Each pane is a real pseudo-terminal running its own process.
- **Shells or agents.** Every pane runs your configured shell — or a named agent
  (e.g. `claude`, `codex`) launched from the agent picker.
- **A context bus.** Panes can *share* or stay *isolated*. A shared pane's visible
  screen can be sent to another pane, broadcast to all of them, or dumped to a
  `CONTEXT.md` file — wrapped in bracketed-paste markers so agent TUIs treat it as
  one paste.
- **cwd tracking.** `cd`/`pushd` you type in a pane is tracked, so a pane you open
  right after a `cd` lands in the same directory.
- **A resource bar.** A two-row monitor at the top shows whole-machine (`SYS`) and
  this-process (`APP`) CPU / RAM / VRAM.
- **Configurable.** A TOML file makes the shell, agents, keybindings, border colors,
  and pane cap all yours. No file → sensible defaults.

## Requirements

- A Rust toolchain (`cargo`).
- A real terminal to run it in.
- **Windows 10 (October 2018 build 1809) or newer** — the app uses the Windows
  ConPTY pseudo-console. (On Unix it uses a regular PTY.)

## Getting started

```bash
cargo run                 # build and run with the default config
cargo run -- --config my.toml   # run with an explicit config file
```

Other common commands:

```bash
cargo build               # debug build
cargo build --release     # optimized single-binary build
cargo test                # run the test suite
cargo clippy              # lints
```

`examples/pty_probe.rs` is a standalone diagnostic that opens a PTY exactly like the
app and probes the I/O round-trip in phases — useful for telling whether a stall is in
the PTY/shell layer or the rendering layer:

```bash
cargo run --example pty_probe
```

## Configuration

Config is a TOML file, loaded **before** the terminal is touched (so a malformed file
errors out cleanly rather than corrupting your outer terminal). It is read from
`--config <path>` if given, otherwise the platform default:

- Unix: `~/.config/tuiterminals/config.toml`
- Windows: `%APPDATA%\tuiterminals\config.toml`

**Every field is optional.** A missing file yields the built-in defaults, and a partial
file fills the absent fields with the same defaults — so no config file means the app
behaves exactly as it would with one. A present-but-malformed file is a hard error.

The five knobs:

- **`[shell]`** — `program` + `args`; each new shell pane spawns this. Absent → the
  platform default shell.
- **`[[agents]]`** — a list of named agent programs (`name`, `program`, `args`), written
  as repeated tables. Each is launchable into a pane from the agent picker. Absent →
  the picker offers a plain shell only.
- **`[keybindings]`** — the nine global actions, each a key-combo string.
- **`[colors]`** — `active_border` / `inactive_border`, each a `"#rrggbb"` string. These
  retheme the pane chrome only; a program's own cell colors still come from the
  alacritty palette.
- **`max_panes`** — the pane cap (default `4`).

### Example

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
inactive_border = "#808080"
```

### Keybindings

A binding matches only when the key code **and** all four modifier bits (shift, ctrl,
alt, super) match exactly — so `ctrl+n` does not also fire on `shift+ctrl+n`.

| Action              | Default          | What it does                                              |
| ------------------- | ---------------- | --------------------------------------------------------- |
| `new_pane`          | `ctrl+n`         | Open a new shell pane (asks share / isolate first)        |
| `kill_pane`         | `ctrl+k`         | Kill the active pane                                      |
| `quit`              | `ctrl+q`         | Quit the app                                              |
| `prev_pane`         | `ctrl+left`      | Focus the previous pane                                    |
| `next_pane`         | `ctrl+right`     | Focus the next pane                                        |
| `new_agent`         | `ctrl+shift+n`   | Open the agent picker (then the share/isolate question)   |
| `send_context`      | `ctrl+shift+s`   | Send the active pane's screen to a chosen shared pane     |
| `broadcast_context` | `ctrl+shift+b`   | Send the active pane's screen to every other shared pane  |
| `dump_context`      | `ctrl+shift+c`   | Append the active pane's screen to `CONTEXT.md`          |

Key-combo strings are `+`-separated: a modifier list (`ctrl`/`control`, `alt`/`option`,
`shift`, `super`/`meta`/`cmd`/`win`) plus exactly one key. Named keys include `left`,
`right`, `up`, `down`, `enter`, `tab`, `backspace`, `esc`/`escape`, `home`, `end`,
`pageup`/`pgup`, `pagedown`/`pgdn`, `delete`/`del`, `insert`, and `f1`–`f12`; any other
single character is matched literally.

### Colors

Defaults: `active_border = "#00ffff"` (cyan), `inactive_border = "#808080"` (dark
gray). These color the pane borders only — the active pane's border uses
`active_border`, the rest use `inactive_border`.

## Context sharing

Panes share context through a **context bus**. Every pane is either **shared** (on the
bus) or **isolated** (off it). The border title shows membership at a glance: an
isolated pane's title carries `(private)`, and an agent pane carries `· <agent name>`.
The **first pane is always shared** — it is the root of the bus. Isolated panes neither
send nor receive; they are invisible to the relay.

**You choose at every spawn.** Both `new_pane` and `new_agent` open a modal that first
asks whether the new pane shares context:

```
[y] share   [n] isolate   (esc) cancel
```

`new_agent` is a two-step flow: first a picker lists the configured `[[agents]]` by
number (plus `[s]` for a plain shell, which defers to the share question), then the
chosen agent gets the same y/n share question.

**The modal.** While one is open, *every* key press routes to it — nothing reaches the
global bindings or the panes. Esc always cancels; any other key that doesn't complete the
flow is swallowed. The prompt is drawn in a black bar pinned to the bottom of the frame.

**Relaying context.** A shared pane's visible screen text can be sent to other panes:

- **Targeted** — `send_context` opens a picker of the other shared, alive panes (numbered
  in that filtered order); the active pane's screen text is written into the chosen pane's
  PTY.
- **Broadcast** — `broadcast_context` sends to every other shared, alive pane at once.
- **Dump** — `dump_context` appends the active pane's screen to `CONTEXT.md` in the launch
  directory (the directory the app was started from), under a
  `## <timestamp> — pane <n> (<name>)` header.

Relayed text is wrapped in **bracketed-paste markers** (`\x1b[200~ … \x1b[201~`) so agent
TUIs treat the dump as a single paste, and **no trailing Enter is sent** — the text lands
at the target's prompt and you decide whether to send it. When a *shared* pane is
spawned, a pointer line (`shared context: <launch_dir>/CONTEXT.md`) is typed into it so
the agent knows where the shared file lives.

## How it works

Six modules, one per concern. The data flow that ties them together:

```
portable-pty ──▶ byte stream (mpsc channel from a per-pane reader thread)
                ──▶ alacritty_terminal (parse into a cell grid)
                ──▶ ratatui (draw cells into a pane)
```

`crossterm` events drive the main loop and forward keystrokes back into the active PTY.

| Module           | Responsibility                                                                 |
| ---------------- | ------------------------------------------------------------------------------ |
| `src/main.rs`    | Entry point and event loop: raw mode + alternate screen, a panic hook that restores the outer terminal, then `pump` → `check_children` → poll crossterm → route → draw. `reflow()` recomputes the layout on resize. |
| `src/app.rs`     | `App` (pane list, active index, PTY system, `max_panes`, configured shell/agents, launch dir) and `Pane` (an alacritty `Term`, a `vte` `Processor`, the reader channel, the PTY writer, the child handle, `shared`, `agent_name`). Holds the cwd tracker and the context-sharing helpers. |
| `src/input.rs`   | `handle_global` (match a key against the configured bindings) and `key_to_bytes` (encode any other key press into the bytes a terminal would send). |
| `src/ui.rs`      | The modal state machine: the share/isolate question, the agent picker, the send-target picker, and the prompt-line strings. Pure and fully unit-tested. |
| `src/render.rs`  | Pane layout (1 fills, 2 splits, 3–4 form a 2×2 grid), drawing borders + each pane's grid, the top resource bar, the bottom modal prompt, and the alacritty→ratatui color/flag translation. |
| `src/resources.rs` | The top resource bar's sampler: `sysinfo` for CPU/RAM and an off-main-thread `nvidia-smi` probe for VRAM. |
| `src/config.rs`  | The TOML config subsystem: loading, key-combo parsing, and color parsing. |

### The Windows ConPTY quirk

On Windows the ConPTY console host sends a lone `\x1b[6n` (cursor position report
request) on startup and **stalls until it gets a `\x1b[row;colR` reply**. If that reply
never reaches the shell, the whole I/O round-trip deadlocks and every pane renders as an
empty black screen. alacritty computes the CPR reply in its `device_status` handler and
emits it as `Event::PtyWrite`; the `PtyWriter` event listener in `app.rs` relays it back
into the PTY. If you touch the event listener or the `pump` loop, keep this path intact —
the `pump_unblocks_conpty_roundtrip` test exists specifically to catch a regression here.

## Testing

The suite runs with `cargo test` and lives in four modules:

- **`src/config.rs`** — key-combo parsing, color parsing, and `Config::load` against temp
  files (fast, no I/O beyond temp files).
- **`src/app.rs`** — the `apply_cd`/`resolve_path` cwd-tracking logic, the
  `screen_text`/`paste_wrap`/`relayable` context-sharing helpers, plus
  `pump_unblocks_conpty_roundtrip` — a **real integration test** that spawns a live shell
  in a PTY and pumps it for up to 5s until output reaches the grid. It is the regression
  guard for the ConPTY stall and the slow test in the suite.
- **`src/ui.rs`** — every modal transition (share/isolate, agent picker, send-target) and
  the prompt-line strings.
- **`src/resources.rs`** — the `nvidia-smi` VRAM output parser.

Run a single test with `cargo test <name>` (e.g. `cargo test apply_cd`).

## Build profile

`[profile.release]` is tuned for a small single-binary TUI: `opt-level = 2`,
`lto = "thin"`, `codegen-units = 1`, `debug = false`. These are a deliberate size/speed
tradeoff for a binary that ships as one file — don't change them casually.
