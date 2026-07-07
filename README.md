# session-tui

A terminal multiplexer for coding-agent sessions. The left pane lists
your Claude Code and Codex sessions (newest first); the right pane is an
embedded terminal where sessions run. Multiple sessions can run at once —
switching selection swaps which live terminal is shown.

```
┌ Sessions ─────────┐┌ Terminal ──────────────────────────────────┐
│⚡● fix login bug   ││ ...live agent session...                   │
│ ○ /code-review    ││                                            │
│▶● add dark mode   ││                                            │
└───────────────────┘└────────────────────────────────────────────┘
```

- `●` Claude Code · `○` Codex
- `⚡` running and actively producing output · `▶` running, waiting

## Requirements

- Unix (macOS/Linux); tested on macOS
- Rust toolchain (2021 edition or later)
- `claude` and/or `codex` CLIs on `PATH` for launching sessions

## Usage

```sh
cargo run --release
```

Sessions are discovered from `~/.claude/projects` and
`~/.codex/sessions` and kept fresh by a file watcher — sessions you
start outside the app appear automatically.

### Keys

| Key | Where | Action |
|---|---|---|
| `j`/`k`/arrows | list | move selection |
| `Enter` | list | resume selected session (or attach if running) |
| `n` | list | new session: `Tab` picks agent, arrows pick a known project dir, or type/paste a path |
| `Ctrl+K` | list | kill the selected running session (`y` confirms) |
| `q` | list | quit (`y` confirms if sessions are running) |
| `Ctrl+\` | anywhere | toggle focus between list and terminal |
| `PgUp`/`PgDn` | terminal | browse scrollback; any other key snaps back live |

In terminal focus every other key — including `Esc`, `Ctrl+C`,
Alt/meta combinations, function keys, and paste — goes to the embedded
agent verbatim. Confirmation prompts default to No: only `y` confirms.

## How it works

- **Discovery**: scans `~/.claude/projects/*/​*.jsonl` and
  `~/.codex/sessions/**/rollout-*.jsonl` directly; subagent/sidechain
  transcripts are skipped. Titles come from the first human message;
  sessions started with a slash command are titled `/<command>`.
  A file watcher (500 ms debounce) keeps the list fresh.
- **Resume**: `claude --resume <id>` / `codex resume <id>` in the
  session's original cwd, inside a fresh PTY. Sessions whose directory
  no longer exists are refused with a notice rather than silently
  resumed from `$HOME`.
- **Terminal**: portable-pty + vt100 emulation rendered through
  tui-term — 10k lines scrollback per session, application-cursor and
  bracketed-paste modes honored.
- **Quitting** kills child processes (after confirmation); sessions
  remain resumable from their transcripts on disk.

## Development

```sh
cargo test                    # unit + integration tests (real PTYs)
cargo clippy --all-targets    # kept at zero warnings
cargo run --release --example scan   # scan your real stores, print timing
```

Architecture: `sessions` (JSONL scanning) · `term` (PTY + emulator) ·
`app` (pure state machine emitting effects) · `ui` (ratatui rendering) ·
`main` (event loop, effect execution, fs watcher). See
[AGENTS.md](AGENTS.md) for the full agent/contributor context.
