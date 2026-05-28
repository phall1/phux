---
audience: agents, contributors
stability: stable
last-reviewed: 2026-05-28
---

# Project Instructions for AI Agents

**TL;DR.** Tracker integration (`bd`) and session-close protocol are
auto-injected below. Doc system: read [`docs/CONVENTIONS.md`](./docs/CONVENTIONS.md)
before adding or moving docs — it defines frontmatter, TL;DR, ADR
template, and `just docs-check` gates. Build & test: `nix develop`,
then `just ci`. Conceptual model in [`docs/CONCEPTS.md`](./docs/CONCEPTS.md).

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:7510c1e2 -->
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

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->


## Build & Test

The dev shell is Nix-pinned (`flake.nix`): Rust 1.90, `zig_0_15` for
libghostty-vt's build, plus `nextest`, `deny`, `watch`, `insta`,
`mutants`, `just`.

```bash
nix develop      # or direnv allow once
just ci          # everything CI must pass: fmt-check + lint + test + deny
just check       # quick type-check
just test        # cargo nextest run --workspace --all-features
```

`just ci` is the bar in CONTRIBUTING.md. Do not push without it green.

## Architecture Overview

phux is a **libghostty-backed terminal control plane**. The wire is
asymmetric: server→client *terminal content* is **VT bytes** forwarded
from the PTY ([ADR-0013](./ADR/0013-libghostty-bytes-on-wire.md));
client→server *input* is **structured key, mouse, focus, and paste
events** built from libghostty's atoms (ADR-0006, ADR-0008). The
protocol is layered as L1 Terminal substrate + L2 Collection + L3
Metadata ([ADR-0015](./ADR/0015-protocol-layering.md)). One server per
user ([ADR-0003](./ADR/0003-server-process-model.md)); one tokio
current-thread runtime; UDS transport with a QUIC future
([ADR-0007](./ADR/0007-mosh-class-transport-and-satellites.md)).

Authoritative docs, in order of priority:

- [`docs/CONCEPTS.md`](./docs/CONCEPTS.md) — canonical mental model.
  Read this first if you haven't.
- [`docs/spec/`](./docs/spec/) — normative wire protocol. Code conforms
  to it, not vice versa.
- [`docs/architecture/`](./docs/architecture/) — internal structure
  (process model, crate graph, data model, threading, transport).
- [`docs/consumers/tui.md`](./docs/consumers/tui.md) — TUI consumer
  surface (CLI, config, keybindings, status bar, hooks).
- [`docs/operations.md`](./docs/operations.md) — error model, logging,
  security boundaries.
- [`docs/vision.md`](./docs/vision.md) — the long arc.
- [`ADR/`](./ADR/) — decisions, with rationale and tradeoffs.

Crates: `phux-protocol` (wire), `phux-core` (domain), `phux-server`
(daemon), `phux-client` (renderer + ratatui chrome), `phux-config`
(TOML + widgets), `phux` (binary). `phux-protocol` is publishable; the
rest are `publish = false`.

## Conventions & Patterns

- **Doc system is layered.** Read [`docs/CONVENTIONS.md`](./docs/CONVENTIONS.md)
  before adding or moving docs. Every `.md` outside `README.md` carries
  YAML frontmatter (audience/stability/last-reviewed) and a `**TL;DR.**`
  block. `just docs-check` enforces the gates in CI.
- **No emojis in committed files.** Plain prose only.
- **Conventional commits.** `feat(scope): ...`, `fix(scope): ...`,
  `docs(scope): ...`, `chore(scope): ...`.
- **`docs/spec/` is normative.** Wire changes update the relevant
  `docs/spec/*.md` + add an entry to `docs/spec/CHANGELOG.md` + bump
  version (see CONTRIBUTING.md). Wire bytes are owned by
  `phux-protocol`; `phux-server` and `phux-client` consume them.
- **ADR for any decision that closes off a design space.** Strict
  template per `docs/CONVENTIONS.md` — controlled `Status:` vocabulary,
  ~150-line cap. Bug fixes don't need an ADR; "should this be in `core`
  or `server`?" does.
- **`unsafe` requires a `// SAFETY:` comment.** Library crates default
  to `forbid(unsafe_code)`.
- **No new deps without a paragraph of justification in the PR.**
- **Linear history on `main`.** Rebase, ff-only merges; no `--no-ff`.
- **Multi-agent fan-out uses self-managed worktrees** — see
  CONTRIBUTING.md §"Multi-agent fan-out" for the wave-1 race that
  motivated this.
