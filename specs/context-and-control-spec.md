# Spec: control & shared-context improvements

Five improvements to tuiterminals, in build order:

1. Context size limitation (shared context)
2. Context compression and range control
3. Tool definitions (`[[tools]]`)
4. tmux-style prefix key
5. Per-pane pause/resume

Each section: goal → design → file changes → config → tests → acceptance.
Items 1–2 share one relay choke point; item 3 builds on it. Items 4–5 are
independent. Nothing here touches the PTY/ConPTY round-trip path
(`PtyWriter`, `pump`) — the `pump_unblocks_conpty_roundtrip` test must keep
passing throughout.

---

## 1. Context size limitation

**Goal.** Bound how much screen text a relay can carry. Today
`Pane::screen_text` (app.rs) captures the whole visible grid and
`send_to` / `broadcast` / `dump_context` relay it unbounded — a wide pane
is ~10 KB per relay, and `CONTEXT.md` grows without limit.

**Design.**

- New pure function in `app.rs`:
  `truncate_context(text: &str, max_lines: usize, max_bytes: usize) -> String`.
  - Keeps the **tail** of the text (most recent output is what matters).
  - When truncating, prepends one marker line:
    `… (elided N earlier lines) …` where `N` is the number of lines dropped.
  - Applies the byte cap after the line cap; the marker accounts for its own
    bytes (never let the result exceed `max_bytes`).
  - `max_lines == 0` or `max_bytes == 0` means "no cap" for that dimension.
- All three relays pass their payload through it, in this order:
  `screen_text()` → (item 2) `compress_screen` → `truncate_context` →
  `paste_wrap`. `dump_context` uses the same pipeline for the file write.
- `dump_context` additionally enforces a file cap: after appending a section,
  if `CONTEXT.md` exceeds `max_context_file_kb`, trim the file to the newest
  `## ` sections that fit under the cap (never cut a section in half).
- When `confirm_before_send` is set, the send flow gains a final confirmation
  step (see UI below) so the user sees the size before anything is written.

**UI (ui.rs).**

- `Modal::SendTarget` gains a follow-up state:
  `Modal::ConfirmSend { target: usize, lines: usize, bytes: usize }`.
  Prompt: `send N lines (M KB) to pane <t>?  [y] yes   [n] no`.
  `y` → `ModalAction::Send { target }`; `n`/`esc` → `Cancel`.
- `ConfirmSend` is only inserted when `confirm_before_send` is true; with it
  false the picker completes exactly as today.
- `broadcast` does **not** get a confirm step (it is one keypress by design);
  it is bounded by the caps alone.

**Config (config.rs).** New optional `[context]` section, all fields defaulted:

```toml
[context]
max_lines            = 200     # 0 = no cap
max_bytes            = 16384   # 0 = no cap
confirm_before_send  = true
max_context_file_kb  = 256     # 0 = no cap
```

`Config` gains `context: Context` with `#[serde(default)]`; a missing
`[context]` section yields the defaults above.

**Tests.**

- `app.rs`: `truncate_context` — no-op under caps; tail kept on line cap;
  marker present with correct `N`; byte cap respected including marker size;
  zero = unbounded.
- `config.rs`: `[context]` parses; partial section fills defaults; missing
  section = defaults.
- `ui.rs`: `SendTarget` → `ConfirmSend` → `Send`/`Cancel` transitions, and
  the no-confirm path when the flag is off.

**Acceptance.** A 50-row × 200-col pane relayed with the default caps lands
≤ 16 KB at the target; `CONTEXT.md` never exceeds its cap; a relay of an
uncapped-size screen is visibly marked as elided.

---

## 2. Context compression and range control

**Goal.** Make relayed text denser (less noise) and let the user choose
*which part* of the pane to send, not just how much.

**Design.**

- New pure function in `app.rs`: `compress_screen(text: &str) -> String`.
  Passes, in order:
  1. **trim** — strip trailing whitespace from every line;
  2. **blanks** — collapse ≥ 2 consecutive blank lines to one;
  3. **repeat** — collapse ≥ 3 consecutive *identical* lines to one copy plus
     a trailing `  (×N)` marker on that line (repeated prompts, spinner lines).
  Idempotent: `compress_screen(compress_screen(x)) == compress_screen(x)`.
- Applied in the relay pipeline (item 1) when `context.compression` allows:
  `"none"` → skip; `"trim"` → passes 1–2; `"compact"` → all three.
  Default: `"trim"`.
- **Range control** — the send flow gains a range step *before* the target
  picker:
  - `[a]ll` — the visible screen (today's behavior, compressed + capped);
  - `[t]ail` — last `context.tail_lines` lines of the visible screen
    (default 50; implemented as `truncate_context` with `max_lines = N` and
    no elision marker);
  - `[h]istory` — the pane's scrollback: the most recent
    `context.max_lines` lines of the *full* grid (visible + history),
    via the existing `display_iter` negative-`Line` path that
    `screen_text` already walks;
  - `esc` — cancel.
- `screen_text` gains a sibling, `screen_text_range(&self, from_history: bool,
  max_lines: usize) -> String`, so the visible-only path and the history path
  share the grid-walking code.

**Config.** Inside `[context]`:

```toml
compression  = "trim"   # "none" | "trim" | "compact"
tail_lines   = 50
```

**Tests.**

- `app.rs`: `compress_screen` — each pass individually; idempotence; the `×N`
  marker count is correct; a screen already clean is unchanged.
- `app.rs`: `screen_text_range` — history rows are mapped back into the
  visible range the same way `screen_text` does (display_offset handling).
- `ui.rs`: range step → each of the three ranges reaches the target picker
  (or confirm step) with the right payload size; `esc` cancels.

**Acceptance.** A pane showing a repeated spinner line relays one copy plus
`(×N)`; choosing `tail` on a full screen sends ~50 lines, not the whole
grid; choosing `history` returns scrollback lines older than the visible
screen.

---

## 3. Tool definitions

**Goal.** A `[[tools]]` config section: named one-shot commands or prompt
templates the user can fire at a pane — the "tool definitions" of this
multiplexer. `[[agents]]` covers long-running programs; tools cover the
rest.

**Design.**

- `config.rs`:

  ```toml
  [[tools]]
  name = "review"
  kind = "run"              # spawn a program in a new pane
  program = "claude"
  args = ["-p", "Review the git diff in {cwd}"]

  [[tools]]
  name = "explain"
  kind = "prompt"           # type a template into the active pane
  text = "Explain the code on screen. Cwd: {cwd}"
  ```

  `Tool { name, kind, program, args, text }` — `program`/`args` required for
  `run`, `text` required for `prompt`; a present-but-incomplete `[[tools]]`
  entry is a hard config error (same policy as a malformed file).

- `app.rs`: `App.tools: Vec<Tool>` (from config, like `agents`) and
  `App::run_tool(index)`:
  - **`run`** — spawn a new pane running `program`/`args` with `{cwd}`
    expanded to the active pane's tracked `cwd`; the pane goes through the
    same share/isolate question as `new_agent`; the border title shows
    `· <tool name>` (reuses the `agent_name` display slot).
  - **`prompt`** — expand `{cwd}` and `{screen}` (active pane's screen text,
    through the item 1–2 pipeline) in `text`, then write
    `paste_wrap(expanded)` into the **active** pane's writer. No trailing
    Enter — the relay convention: the text lands at the prompt and the user
    decides whether to send it. Only shared, alive panes may be the target
    (checked with `relayable` against a virtual source, i.e. the app itself —
    in practice: target must be `shared && alive`).
- `ui.rs`: `Modal::ToolPicker` (numbered list, like the `NewAgent` picker)
  and `ModalAction::RunTool { index }`. The picker is opened by the new
  `run_tool` binding; with zero configured tools the binding is a no-op that
  flashes `no tools configured` in the prompt bar.
- `input.rs` / `config.rs`: 15th action, `run_tool` (default
  `ctrl+shift+t`).
- `session.rs`: a `run` tool pane is a pane running a program, not a
  configured agent — save it as `launch = "tool:<name>"` and resolve it back
  on restore; an unresolvable name degrades to a plain shell (same fallback
  policy as a renamed agent).

**Tests.**

- `config.rs`: both kinds parse; missing `text` on `prompt` / missing
  `program` on `run` errors; absent section → empty list.
- `app.rs`: placeholder expansion (`{cwd}`, `{screen}`; unknown `{x}` left
  verbatim); `run_tool` on a `prompt` tool writes paste-wrapped bytes with no
  trailing `\n`.
- `ui.rs`: picker → `RunTool` for a digit; `esc` cancels; out-of-range digit
  swallowed.

**Acceptance.** With the two example tools configured, `run_tool` lists them;
picking `explain` types the expanded template at the active pane's prompt;
picking `review` opens the share question and then a pane titled
`· review`.

---

## 4. tmux-style prefix key

**Goal.** Stop the global bindings from fighting the hosted programs. With a
prefix configured, every multiplexer action requires `prefix + key`, and the
prefix itself never reaches a pane.

**Design.**

- `config.rs`: `Keybindings.prefix: Option<KeyCombo>`
  (`#[serde(default)]`; `None` = today's behavior, direct bindings).
- `main.rs`: one flag, `waiting_for_prefix: bool`.
  - Prefix configured and key == prefix → set the flag, swallow the key.
  - Flag set → clear it; route the key through `handle_global` (which sees
    the *second* key only). No match → the key is swallowed (it was a
    failed prefix attempt, not pane input).
  - `prefix + prefix` → forward the prefix bytes to the active pane
    (send-the-prefix escape hatch), regardless of bindings.
  - While a modal is open the prefix is ignored (modals already swallow
    everything).
  - The flag auto-clears after one key, so there is no timeout state to
    manage.
- With `prefix = None`, behavior is byte-for-byte today's: every binding
  fires directly and no key is ever consumed by the prefix logic.

**Config example.**

```toml
[keybindings]
prefix    = "ctrl+b"
new_pane  = "n"
kill_pane = "k"
# ... second keys may be bare letters now
```

**Tests.**

- `input.rs`: with a prefix configured, the *second* key alone matches a
  binding (the prefix half is handled in main, so `handle_global` is
  unchanged — test that a bare `n` with no prefix state does **not** match
  when a prefix is configured, by exercising the new
  `prefix_gate(prefix, first, second) -> bool` helper kept pure for this).
- `config.rs`: `prefix` parses; absent → `None`.

**Acceptance.** With `prefix = "ctrl+b"`, pressing `ctrl+n` alone goes to
the shell; `ctrl+b` then `n` opens a pane; `ctrl+b` `ctrl+b` types `ctrl+b`
into the shell; with no `prefix` in the config, nothing changes.

---

## 5. Per-pane pause/resume

**Goal.** Freeze a runaway pane's input without killing it. Today the only
control over a running pane is `kill_pane`.

**Design.**

- `app.rs`: `Pane.paused: bool` (default `false`) and
  `App::toggle_pause(index)`.
- `main.rs`: before forwarding a keystroke to the active pane, if
  `paused` → swallow it. Output keeps pumping into the grid — the user still
  sees the pane working; only input is gated (flow control, not a kill).
  `observe_key` is also skipped while paused, so the shadow `cwd`/`line`
  tracker stays consistent (it mirrors what the shell actually receives).
- `render.rs`: the border title gains `(paused)` alongside the existing
  `(private)` / `· <agent>` markers.
- `config.rs` / `input.rs`: 16th action, `pause_pane` (default
  `ctrl+shift+p`; with a prefix configured, `prefix + p` works too since it
  is just another binding).
- `session.rs`: no change — a paused pane saves/restores as usual (it comes
  back unpaused; pause is a live-session state, not a session fact).

**Tests.**

- `app.rs`: `toggle_pause` flips the flag; a paused pane's writer is not
  called by the forward path (test the pure gate predicate
  `input_gated(paused) -> bool` if the check lives in main).
- `ui.rs`-style prompt test: the bottom bar shows the paused state when a
  non-active pane is paused (border title, covered by the render test if one
  exists for titles).

**Acceptance.** `pause_pane` on a pane running a busy command: keystrokes no
longer reach it, its output still renders, the border reads `(paused)`;
pressing again resumes input.

---

## Build order & global constraints

1 → 2 → 3 → 4 → 5, for the reasons in the proposal: 1 and 2 share the relay
choke point; 3 reuses it for `{screen}`; 4 settles the keybinding schema
before 5 adds another action.

Global rules for all five:

- **No PTY-path changes.** `PtyWriter`, `pump`, and the ConPTY CPR relay are
  off-limits; `pump_unblocks_conpty_roundtrip` must pass after every step.
- **Config is additive.** Every new field/section is `#[serde(default)]`;
  a config file written before this spec loads and behaves unchanged
  (except where a feature is explicitly opt-in, e.g. the prefix).
- **Pure helpers get unit tests** in the module that owns them
  (`app.rs` / `config.rs` / `input.rs` / `ui.rs`), matching the existing
  test layout in CLAUDE.md.
- **CLAUDE.md updated last**, per feature: the six-knobs list, the
  keybinding count (13 → 15/16), and a new "Context pipeline" section
  describing compress → truncate → wrap.
