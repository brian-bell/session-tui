# session-tui — agent context

A Rust TUI that lists Claude Code and Codex sessions in a left pane and
runs them in embedded terminals in a right pane. Multiple sessions run
concurrently; selection swaps which live terminal is shown. The split
follows what the user is doing: 80/20 while browsing (no terminal
attached), 25/75 once one is attached, and by default the list
auto-hides entirely while the terminal has focus; `h` in list focus
toggles auto-hide.

> **Note:** this repo maintains `CLAUDE.md` and `AGENTS.md` as separate
> files (not a symlink) to support Beads integration — `bd` manages
> tool-specific blocks in each file independently. Shared project
> context lives here in `AGENTS.md`; `CLAUDE.md` refers back to this
> file.

## Build, test, run

```sh
cargo build                  # debug build
cargo run --release          # run the TUI
cargo test                   # full suite (spawns real PTYs; unix only)
cargo clippy --all-targets   # kept at zero warnings
cargo run --release --example scan   # scan real session stores, print timing
python3 scripts/e2e.py               # end-to-end PTY driver (builds release first)
```

There is no Makefile. CI (`.github/workflows/ci.yml`) runs `cargo test`
and `cargo clippy --all-targets -- -D warnings` on pushes to main and on
PRs. Tests use `tempfile` fixtures and real `/bin/sh` children; they
require a Unix host (CI uses ubuntu-latest).

`scripts/e2e.py` (Python 3 stdlib, unix only) is the end-to-end check:
it runs the release binary in a real PTY with a hermetic `$HOME` (fixture
transcript) and a fake `claude` shim on `PATH`, then drives
list → picker → launch → input passthrough → auto-hide resize → focus
toggle → auto-hide toggle → kill → quit,
asserting on rendered frames. It covers what unit tests can't — crossterm
key encoding quirks, PTY lifecycle, real rendering — and its assertions
are whitespace-insensitive because ratatui's frame diffing draws blank
cells as cursor moves. Set `SESSION_TUI_BIN` to test a prebuilt binary.
Exit code 0 means all checks passed.

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
  missing store roots with 0700. `Scanner` makes rescans incremental:
  it caches parse results per path keyed on (mtime, len) — including
  parsed-and-rejected files — so a watcher rescan only reparses
  changed/new transcripts and drops entries for deleted ones; the
  scanner thread holds one instance for its lifetime.
- `term.rs` — one live session. `PtySession` spawns a child via
  portable-pty, drains output on a reader thread into a `vt100::Parser`
  (10k scrollback), and exposes input/resize/kill/status. `CommandSpec`
  builds `claude --resume <id>` / `codex resume <id>` / bare launches.
  `SessionStatus` maps output recency to Busy/Idle (2s threshold).
  `TermModes` samples the child's DEC private modes (DECCKM, bracketed
  paste) from the emulator as one value.
- `input.rs` — user input → child bytes. `encode_key` translates
  crossterm key events into the ANSI sequences a terminal would send
  (honoring `TermModes`), `encode_paste` wraps pastes for bracketed
  paste, and `is_focus_toggle` recognizes the reserved focus chord.
  The state machine holds no byte-level knowledge.
- `picker.rs` — the new-session dialog state. Mutations (`edit`,
  `backspace`, `paste`, `move_highlight`, `toggle_agent`) enforce the
  picker's invariant internally: editing resets the highlight and
  cancels explicit navigation. `chosen_dir` resolves typed-path vs
  navigated-match precedence; fields are read via accessors.
- `roster.rs` — the session list and its lifecycle. `Roster` owns the
  `Row`s (scanned transcripts plus provisional launches), each row's
  live run, and selection-by-identity. `launch`/`resume_selected`/
  `mark_exited`/`absorb_scan` are the lifecycle; adoption of a
  provisional row by its scanned transcript happens in `absorb_scan`
  under the unambiguity rules below. Never touches the filesystem or a
  PTY. Domain terms are defined in `CONTEXT.md`.
- `app.rs` — pure Elm-style state machine. `handle_key`/`handle_paste`
  return `Effect`s (`Spawn`, `WriteTerminal`, `Kill`, `Quit`); it never
  touches a PTY. Owns focus (List/Terminal), overlays (confirm
  quit/kill, launch picker), scrollback offset, notices, the auto-hide
  flag (`list_hidden` is derived from it and focus, never stored), and
  the attached run; delegates session-list state to `roster.rs`, input
  encoding to `input.rs`, and picker state to `picker.rs`. `handle_key`/`handle_paste` take the child's
  `TermModes` as a parameter — App holds no synced mode state.
- `ui.rs` — ratatui rendering: session rows, tui-term pane, overlays,
  help/notice bar. Three layouts — 80/20 with nothing attached, 25/75
  with a terminal attached, terminal-only when auto-hide hides the
  list. `panes` computes the frame geometry once from the frame size
  and app state; `render` and `terminal_pane_size` (the single source
  of PTY dimensions) both consume it, so the drawn pane and the PTY
  size cannot disagree.
- `main.rs` — composition root: crossterm event loop, effect execution,
  child reaping, notify-based store watcher (500ms debounce), and
  `TerminalGuard` (restores the terminal on every exit path).

Tests live in `tests/` (integration-style, public API only) with shared
fixtures in `tests/fixtures/mod.rs`.

## Invariants and gotchas

- **App stays pure.** All side effects go through `Effect`; the main
  loop is the only place PTYs are created, written, or killed. Keep new
  behavior unit-testable through `handle_key`.
- **Provisional sessions**: a fresh launch inserts a provisional `Row`
  (`Row::is_provisional`) with no transcript. On rescan it is *adopted*
  by a scanned transcript only when unambiguous: same agent + cwd,
  mtime >= launch, transcript id not in the launch snapshot, exactly
  one candidate, and no competing waiter (another provisional launch
  or a pending-fork resume) in the same agent+cwd.
  Provisional rows are dropped when their process exits (nothing to
  resume); a running transcript row that vanishes from a scan is kept
  so its PTY stays reattachable.
- **Fork adoption**: `claude --resume` forks a NEW transcript id, so
  resuming a forking agent (`Agent::forks_on_resume`) carries a pending
  fork (snapshot of known transcript ids + resume time) and
  `absorb_scan` hands its run to the fork under the same unambiguity
  rules; the original row becomes historical again. codex appends in
  place and never carries one. No candidate means the row keeps its
  run, and an append to the resumed transcript itself clears the
  pending fork so the row stops competing with launches in its cwd.
  Observed ambiguity is terminal: a scan with two plausible candidates
  means the waiter (pending fork or provisional) can never adopt —
  later scans carry no information mtimes didn't carry then, so
  shrinking the set again would be a guess. The waiter keeps blocking
  sibling adoption in its agent+cwd while its process lives, though:
  any new transcript there could still be its late write, and a wrong
  binding is worse than a stranded placeholder. The block dies with
  the process. Domain terms in `CONTEXT.md`.
- **cwd handling**: picker cwds are canonicalized (transcripts record
  the child's resolved getcwd — `/tmp` is a symlink on macOS). Missing
  cwds are refused on both launch and resume because portable-pty
  silently falls back to `$HOME`.
- **Key encoding** (`input.rs`): honors DECCKM (application cursor)
  and legacy quirks — crossterm reports Ctrl+\ as Ctrl+4 (0x1C) and
  0x1D–0x1F as Ctrl+5..7. Ctrl+\/Ctrl+4 is the reserved focus toggle
  and must never reach the PTY. The main loop samples `TermModes` from
  the attached PTY per input event and passes it into
  `handle_key`/`handle_paste`.
- **Paste**: bracketed only when the child enabled DECSET 2004
  (`TermModes::bracketed_paste`, tracked by the emulator); picker-open
  pastes fill the path field.
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
- **PTY sizing**: pane geometry depends on the frame size *and* app
  state (auto-hide, whether a terminal is attached). The main loop
  reconciles every PTY against `terminal_pane_size(area, &app)` before
  each draw — that reconciliation is the only place PTYs are resized
  after spawn.

## Conventions

- TDD: red-green-refactor, one behavior per test, tests exercise public
  interfaces only (see `tests/*.rs` for the style).
- Clippy stays at zero warnings.
- Comments state invariants the code can't show, not narration.

## Known limitations / v2 backlog

Fuzzy list filter, TOML config (keybinds, scan roots, scrollback size,
auto-hide persistence),
copy mode with clipboard, full path browser in the picker, detach
daemon (sessions currently die with the app — transcripts keep them
resumable), mouse support.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:970c3bf2 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.

## Agent Context Profiles

The managed Beads block is task-tracking guidance, not permission to override repository, user, or orchestrator instructions.

- **Conservative (default)**: Use `bd` for task tracking. Do not run git commits, git pushes, or Dolt remote sync unless explicitly asked. At handoff, report changed files, validation, and suggested next commands.
- **Minimal**: Keep tool instruction files as pointers to `bd prime`; use the same conservative git policy unless active instructions say otherwise.
- **Team-maintainer**: Only when the repository explicitly opts in, agents may close beads, run quality gates, commit, and push as part of session close. A current "do not commit" or "do not push" instruction still wins.

## Session Completion

This protocol applies when ending a Beads implementation workflow. It is subordinate to explicit user, repository, and orchestrator instructions.

1. **File issues for remaining work** - Create beads for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Handle git/sync by active profile**:
   ```bash
   # Conservative/minimal/default: report status and proposed commands; wait for approval.
   git status

   # Team-maintainer opt-in only, unless current instructions forbid it:
   git pull --rebase
   bd dolt push
   git push
   git status
   ```
5. **Hand off** - Summarize changes, validation, issue status, and any blocked sync/commit/push step

**Critical rules:**
- Explicit user or orchestrator instructions override this Beads block.
- Do not commit or push without clear authority from the active profile or the current user request.
- If a required sync or push is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->

<!-- BEGIN BEADS CODEX SETUP: generated by bd setup codex -->
## Beads Issue Tracker

Use Beads (`bd`) for durable task tracking in repositories that include it. Use the `beads` skill at `.agents/skills/beads/SKILL.md` (project install) or `~/.agents/skills/beads/SKILL.md` (global install) for Beads workflow guidance, then use the `bd` CLI for issue operations.

### Quick Reference

```bash
bd ready                # Find available work
bd show <id>            # View issue details
bd update <id> --claim  # Claim work
bd close <id>           # Complete work
bd prime                # Refresh Beads context
```

### Rules

- Use `bd` for all task tracking; do not create markdown TODO lists.
- Run `bd prime` when Beads context is missing or stale. Codex 0.129.0+ can load Beads context automatically through native hooks; use `/hooks` to inspect or toggle them.
- Keep persistent project memory in Beads via `bd remember`; do not create ad hoc memory files.

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.
<!-- END BEADS CODEX SETUP -->
