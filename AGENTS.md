# session-tui — agent context

A Rust TUI that lists Claude Code and Codex sessions in a left pane and
runs them in embedded terminals in a right pane (25/75 split). Multiple
sessions run concurrently; selection swaps which live terminal is shown.

## Build, test, run

```sh
cargo build                  # debug build
cargo run --release          # run the TUI
cargo test                   # full suite (spawns real PTYs; unix only)
cargo clippy --all-targets   # kept at zero warnings
cargo run --release --example scan   # scan real session stores, print timing
```

There is no Makefile or CI config yet. Tests use `tempfile` fixtures and
real `/bin/sh` children; they require a Unix host.

## Architecture

One binary crate plus a library, `src/`:

- `sessions.rs` — discovery. Scans `~/.claude/projects/*/​*.jsonl` and
  `~/.codex/sessions/**/rollout-*.jsonl` into `SessionMeta` (id, agent,
  cwd, title, mtime timestamp), merged newest-first. Skips Claude
  sidechain transcripts (`isSidechain`) and Codex subagent rollouts
  (`thread_source == "subagent"`). Titles: first human message; known
  synthetic wrappers (`<command-*>`, `<local-command-*>`) are skipped,
  and slash-command sessions are titled `/<command-name>`. Control
  characters are stripped at parse time. `ensure_store_roots` creates
  missing store roots with 0700.
- `term.rs` — one live session. `PtySession` spawns a child via
  portable-pty, drains output on a reader thread into a `vt100::Parser`
  (10k scrollback), and exposes input/resize/kill/status. `CommandSpec`
  builds `claude --resume <id>` / `codex resume <id>` / bare launches.
  `SessionStatus` maps output recency to Busy/Idle (2s threshold).
- `app.rs` — pure Elm-style state machine. `handle_key`/`handle_paste`
  return `Effect`s (`Spawn`, `WriteTerminal`, `Kill`, `Quit`); it never
  touches a PTY. Owns focus (List/Terminal), selection, overlays
  (confirm quit/kill, launch picker), scrollback offset, notices, and
  the provisional-session ("live-*") lifecycle.
- `ui.rs` — ratatui rendering: 25/75 layout, session rows, tui-term
  pane, overlays, help/notice bar. `terminal_pane_size` is the single
  source of PTY dimensions.
- `main.rs` — composition root: crossterm event loop, effect execution,
  child reaping, notify-based store watcher (500ms debounce), and
  `TerminalGuard` (restores the terminal on every exit path).

Tests live in `tests/` (integration-style, public API only) with shared
fixtures in `tests/fixtures/mod.rs`.

## Invariants and gotchas

- **App stays pure.** All side effects go through `Effect`; the main
  loop is the only place PTYs are created, written, or killed. Keep new
  behavior unit-testable through `handle_key`.
- **Provisional sessions**: a fresh launch inserts a `live-<run_id>`
  row with no transcript. On rescan it is *adopted* by a scanned
  transcript only when unambiguous: same agent + cwd, mtime >= launch,
  transcript id not in the launch snapshot, exactly one candidate, and
  no sibling placeholder in the same agent+cwd. Rows are dropped when
  their process exits (nothing to resume).
- **cwd handling**: picker cwds are canonicalized (transcripts record
  the child's resolved getcwd — `/tmp` is a symlink on macOS). Missing
  cwds are refused on both launch and resume because portable-pty
  silently falls back to `$HOME`.
- **Key encoding** (`encode_key`): honors DECCKM (application cursor)
  and legacy quirks — crossterm reports Ctrl+\ as Ctrl+4 (0x1C) and
  0x1D–0x1F as Ctrl+5..7. Ctrl+\/Ctrl+4 is the reserved focus toggle
  and must never reach the PTY.
- **Paste**: bracketed only when the child enabled DECSET 2004 (the
  emulator tracks it); picker-open pastes fill the path field.
- **Child lifecycle**: `PtySession::kill` reaps (kill + wait); `Drop`
  kills only if `try_wait` says the child still runs — portable-pty's
  unix `kill()` SIGHUPs the stored pid unconditionally, and a reaped
  pid may be reused.
- **Untrusted transcript data**: every transcript-derived string
  (titles, cwd/project names, notices, picker rows) is stripped of
  control characters at the render boundary (`ui::sanitize`), in
  addition to parse-time title stripping. Keep that invariant for any
  new rendering of transcript data.
- **y/N dialogs**: only `y`/`Y` confirms; Enter cancels.

## Conventions

- TDD: red-green-refactor, one behavior per test, tests exercise public
  interfaces only (see `tests/*.rs` for the style).
- Clippy stays at zero warnings.
- Comments state invariants the code can't show, not narration.

## Known limitations / v2 backlog

Fuzzy list filter, TOML config (keybinds, scan roots, scrollback size),
copy mode with clipboard, full path browser in the picker, detach
daemon (sessions currently die with the app — transcripts keep them
resumable), mouse support.
