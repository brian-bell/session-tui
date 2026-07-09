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
- **Fork adoption** — `claude --resume` writes the continuation to a
  NEW transcript id; a rescan hands the resumed row's run to that fork
  under the same snapshot + unambiguity rules as launch adoption. An
  agent that instead appends in place (codex) produces no candidate and
  simply keeps its run.
- **Pending fork** — the wait state a forking agent's resume
  (`Agent::forks_on_resume`) carries until its fork is attributed: a
  snapshot of the transcript ids known at resume time, plus the resume
  timestamp. Never set for in-place agents (codex). Cleared when the
  fork is adopted, when the resumed transcript itself advances (proof
  of in-place appending), or when the process exits. Marked ambiguous
  — never able to adopt, run squatting on the origin row — when a scan
  observes two plausible forks; once ambiguous, an in-place append no
  longer clears it (contradictory evidence), only process exit does.
- **Waiter** — any process that could plausibly write a new transcript
  in an agent + cwd: a provisional launch or a pending-fork resume.
  Two waiters in the same agent + cwd block all adoption there — a new
  transcript can't be attributed to either.
- **Terminal ambiguity** — a single scan showing two or more plausible
  transcripts for one waiter. No later scan can tell them apart, so
  the waiter can never adopt: a pending fork leaves its run on the
  origin row for good, a provisional launch becomes permanently
  unadoptable. The waiter still blocks sibling adoption in its
  agent+cwd while its process lives — any new transcript there could
  be its late write — and the block dies with the process.
- **Run** — one live PTY, identified by a `RunId` for the lifetime of
  the app. A session may gain and lose runs; the transcript persists.
