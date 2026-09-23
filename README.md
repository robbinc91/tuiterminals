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
- **One-shot tools.** Named `[[tools]]` are fired from the tool picker: a `run` tool
  spawns its program in a new pane (then the usual share/isolate question), and a
  `prompt` tool types a template — with `{cwd}` and `{screen}` placeholders expanded —
  into the active pane.
- **A context bus.** Panes can *share* or stay *isolated*. A shared pane's visible
  screen can be sent to another pane, broadcast to all of them, or dumped to a
  `CONTEXT.md` file — wrapped in bracketed-paste markers so agent TUIs treat it as
  one paste.
- **cwd tracking.** `cd`/`pushd` you type in a pane is tracked, so a pane you open
  right after a `cd` lands in the same directory.
- **Scrollback history.** Each pane keeps a scrollback of prior output (default 10000
  rows); scroll up into it with the mouse wheel or the `scroll_*` bindings, and the
  live screen is restored when you scroll back down.
- **Session restore.** On exit the app saves each pane's directory and launch, and on
  the next start offers to bring them back — each respawned fresh in the directory it
  was last in.
- **A resource bar.** A two-row monitor at the top shows whole-machine (`SYS`) and
  this-process (`APP`) CPU / RAM / VRAM.
- **Configurable.** A TOML file makes the shell, agents, tools, keybindings, border
  colors, pane cap, scrollback depth, and session restore all yours. No file →
  sensible defaults.

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

The eight knobs:

- **`[shell]`** — `program` + `args`; each new shell pane spawns this. Absent → the
  platform default shell.
- **`[[agents]]`** — a list of named agent programs (`name`, `program`, `args`), written
  as repeated tables. Each is launchable into a pane from the agent picker. Absent →
  the picker offers a plain shell only.
- **`[[tools]]`** — a list of named one-shot tools (`name`, `kind`, and either
  `program`/`args` for a `run` tool or `text` for a `prompt` tool), written as repeated
  tables. A `run` tool spawns its program in a new pane (then the usual share/isolate
  question); a `prompt` tool types its `text` — with `{cwd}` and `{screen}` placeholders
  expanded — into the active pane. Unlike the other sections, a present-but-incomplete
  entry is a hard error. Absent → no tool picker.
- **`[keybindings]`** — the fourteen global actions, each a key-combo string.
- **`[colors]`** — `active_border` / `inactive_border`, each a `"#rrggbb"` string. These
  retheme the pane chrome only; a program's own cell colors still come from the
  alacritty palette.
- **`max_panes`** — the pane cap (default `4`).
- **`scrollback_lines`** — per-pane scrollback history in rows (default `10000`).
- **`restore_sessions`** — whether the app saves its panes on exit and offers to restore
  them on the next start (default `true`). See [Session persistence](#session-persistence).

### Example

```toml
max_panes = 6
scrollback_lines = 10000
restore_sessions = true

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
new_pane          = "ctrl+n"
kill_pane         = "ctrl+k"
quit              = "ctrl+q"
prev_pane         = "ctrl+left"
next_pane         = "ctrl+right"
new_agent         = "ctrl+shift+n"
run_tool          = "ctrl+shift+t"
send_context      = "ctrl+shift+s"
broadcast_context = "ctrl+shift+b"
dump_context      = "ctrl+shift+c"
scroll_up         = "ctrl+shift+up"
scroll_down       = "ctrl+shift+down"
scroll_top        = "ctrl+shift+home"
scroll_bottom     = "ctrl+shift+end"

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
| `run_tool`          | `ctrl+shift+t`   | Open the tool picker (a `run` tool spawns a pane; a `prompt` tool types into the active pane) |
| `send_context`      | `ctrl+shift+s`   | Send the active pane's screen to a chosen shared pane     |
| `broadcast_context` | `ctrl+shift+b`   | Send the active pane's screen to every other shared pane  |
| `dump_context`      | `ctrl+shift+c`   | Append the active pane's screen to `CONTEXT.md`          |
| `scroll_up`         | `ctrl+shift+up`  | Scroll the active pane up 3 lines (the mouse wheel does the same) |
| `scroll_down`       | `ctrl+shift+down`| Scroll the active pane down 3 lines                      |
| `scroll_top`        | `ctrl+shift+home`| Scroll the active pane to the top of its history           |
| `scroll_bottom`     | `ctrl+shift+end` | Scroll the active pane back to the live screen            |

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

## Session persistence

tmux-style: on exit the app saves its pane layout, and on the next start it offers
to bring it back. **Nothing about the live terminal contents is captured** — every
pane is respawned *fresh* in the directory it was last in. The file is a small TOML
document at the platform config dir (`~/.config/tuiterminals/session.toml` on Unix,
`%APPDATA%\tuiterminals\session.toml` on Windows), written by `src/session.rs`.

- **What's saved** — one entry per pane, in pane order: its tracked `cwd`, its launch
  (a plain `shell`, or a named agent/tool), and whether it was on the context bus.
  Agents and tools are stored **by name** (`agent:<name>`, `tool:<name>`) rather than
  by index, so reordering the `[[agents]]`/`[[tools]]` lists in the config never
  mis-resolves a saved pane. Pane 0 is always written `shared = true` — the bus-root
  invariant — regardless of its own flag.
- **Restoring** — `App::from_session` (in `src/app.rs`) rebuilds one fresh pane per
  saved pane (capped at `max_panes`), each re-launched in its last directory. A saved
  agent or tool that no longer exists in the config falls back to a plain shell, so a
  stale session never fails a spawn; the first pane is always shared. `main.rs` reflows
  afterwards so each pane shrinks from the full area to its layout slot.
- **Gating** — the `restore_sessions` config knob (default `true`) controls both ends:
  when off, the app neither reads a session on start nor writes one on exit. A missing
  or malformed session file is swallowed (falls through to the single-shell-pane
  default) rather than erroring.

## How it works

Eight modules, one per concern. The data flow that ties them together:

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
| `src/session.rs` | tmux-style session persistence: save each pane's `cwd` + launch on exit, and rebuild them fresh on the next start (see [Session persistence](#session-persistence)). |

### The Windows ConPTY quirk

On Windows the ConPTY console host sends a lone `\x1b[6n` (cursor position report
request) on startup and **stalls until it gets a `\x1b[row;colR` reply**. If that reply
never reaches the shell, the whole I/O round-trip deadlocks and every pane renders as an
empty black screen. alacritty computes the CPR reply in its `device_status` handler and
emits it as `Event::PtyWrite`; the `PtyWriter` event listener in `app.rs` relays it back
into the PTY. If you touch the event listener or the `pump` loop, keep this path intact —
the `pump_unblocks_conpty_roundtrip` test exists specifically to catch a regression here.

## Testing

The suite runs with `cargo test` and lives in five modules:

- **`src/config.rs`** — key-combo parsing, color parsing, and `Config::load` against temp
  files (fast, no I/O beyond temp files).
- **`src/app.rs`** — the `apply_cd`/`resolve_path` cwd-tracking logic, the
  `screen_text`/`paste_wrap`/`relayable` context-sharing helpers, plus
  `pump_unblocks_conpty_roundtrip` — a **real integration test** that spawns a live shell
  in a PTY and pumps it for up to 5s until output reaches the grid. It is the regression
  guard for the ConPTY stall and the slow test in the suite.
- **`src/input.rs`** — the scroll-binding routing: the four scroll defaults resolve to the
  scroll actions, and a bare arrow key (no modifiers) does not.
- **`src/ui.rs`** — every modal transition (share/isolate, agent picker, send-target) and
  the prompt-line strings.
- **`src/resources.rs`** — the `nvidia-smi` VRAM output parser.

Run a single test with `cargo test <name>` (e.g. `cargo test apply_cd`).

## Build profile

`[profile.release]` is tuned for a small single-binary TUI: `opt-level = 2`,
`lto = "thin"`, `codegen-units = 1`, `debug = false`. These are a deliberate size/speed
tradeoff for a binary that ships as one file — don't change them casually.
