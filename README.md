# session-tui

A terminal multiplexer for coding-agent sessions. The left pane lists
your Claude Code and Codex sessions (newest first); the right pane is an
embedded terminal where sessions run. Multiple sessions can run at once —
switching selection swaps which live terminal is shown.

```
┌ Sessions ─────────┐┌ Terminal ──────────────────────────────────┐
│▶● fix login bug   ││ ...live agent session...                   │
│ ○ refactor parser ││                                            │
│ ● add dark mode   ││                                            │
└───────────────────┘└────────────────────────────────────────────┘
```

- `●` Claude Code · `○` Codex · `▶` running

## Usage

```sh
cargo run --release
```

### Keys

| Key | Where | Action |
|---|---|---|
| `j`/`k`/arrows | list | move selection |
| `Enter` | list | resume selected session (or attach if running) |
| `n` | list | new session: pick agent (Tab) + directory, or type a path |
| `Ctrl+K` | list | kill the selected running session (confirms) |
| `q` | list | quit (confirms if sessions are running) |
| `Ctrl+\` | anywhere | toggle focus between list and terminal |
| `PgUp`/`PgDn` | terminal | browse scrollback; any other key snaps back live |

In terminal focus every other key (including `Esc`, `Ctrl+C`, paste)
goes to the embedded agent verbatim.

## How it works

- **Discovery**: scans `~/.claude/projects/*/​*.jsonl` and
  `~/.codex/sessions/**/rollout-*.jsonl` directly; subagent/sidechain
  transcripts are skipped, titles come from the first human message.
  A file watcher (500 ms debounce) keeps the list fresh.
- **Resume**: `claude --resume <id>` / `codex resume <id>` in the
  session's original cwd, inside a fresh PTY.
- **Terminal**: portable-pty + vt100 emulation rendered through
  tui-term, 10k lines scrollback per session.
- **Quitting** kills child processes (after confirmation); sessions
  remain resumable from their transcripts on disk.

## Development

```sh
cargo test          # unit + integration tests (real PTYs included)
cargo clippy --all-targets
```

Architecture: `sessions` (JSONL scanning) · `term` (PTY + emulator) ·
`app` (pure state machine emitting effects) · `ui` (ratatui rendering) ·
`main` (event loop, effect execution, fs watcher).
