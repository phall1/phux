---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-27
---

# State replay & crash recovery

**TL;DR.** The intended journal-on-disk shape that turns crash recovery
into mechanical replay: per-pane append-only PTY byte logs, capped, fed
back into fresh libghostty Terminals on `--recover`. Not yet
implemented; the bytes-on-wire choice (ADR-0013) makes the eventual
implementation routine.

---

> **Status:** Design intent. Not yet implemented as of 2026-05-26.
> Nothing in the server currently writes to disk; `server.pid`,
> per-pane journals, and `--recover` do not exist. The bytes-on-wire
> shape (ADR-0013) makes the implementation mechanical when its turn
> comes — the PTY byte stream we forward to clients is also exactly
> what a journal would record.

The intended shape: the server journals raw PTY output to disk, per
pane, in `journal/<pane_id>.log`. Journals are append-only, fsync'd
on close, and capped (default: 10 MB ring per pane).

On startup, if `server.pid` is stale, the server can be invoked with
`--recover`. It reads each journal, replays it into a fresh
`libghostty_vt::Terminal`, and reconstitutes sessions from a metadata
file alongside the journals.

Crash recovery is therefore a property of the design, not an
add-on. tmux loses everything on a daemon crash; phux will not.
