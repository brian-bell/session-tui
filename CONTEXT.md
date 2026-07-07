# session-tui — domain language

Terms used consistently across code, docs, and reviews. When a concept
gets a name here, code should use that name.

- **Session** — one Claude Code or Codex conversation, identified by
  its transcript on disk. Shown as one row in the list.
- **Transcript** — the JSONL file an agent CLI writes for a session
  (`~/.claude/projects/**` or `~/.codex/sessions/**`). The durable
  identity of a session; what makes it resumable.
- **Roster** (`roster.rs`) — the session list and every lifecycle rule
  around it: launching, resuming, adoption, exit. Owns the rows,
  selection-by-identity, and each row's live run. Never touches the
  filesystem or a PTY.
- **Row** — one entry in the roster: a scanned transcript or a
  provisional launch, plus its live run if one is attached.
- **Provisional session** — a row for a process launched here whose
  transcript hasn't been discovered yet. It has a run but no
  transcript id, and disappears with its process.
- **Adoption** — a rescan discovering the transcript a provisional
  session's process wrote, and merging the two rows into one. Only
  happens when the match is unambiguous (see AGENTS.md invariants).
- **Launch snapshot** — the transcript ids that existed when a
  provisional session was launched. Those belong to other sessions, so
  only a transcript outside the snapshot may be adopted.
- **Run** — one live PTY, identified by a `RunId` for the lifetime of
  the app. A session may gain and lose runs; the transcript persists.
