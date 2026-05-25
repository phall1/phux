# Project Instructions for AI Agents

This file provides instructions and context for AI coding agents working on this project.

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

phux is a terminal multiplexer built on `libghostty-vt`. The wire
protocol carries **cell-level diffs** and **structured input events**,
not VT bytes (ADR-0002, ADR-0006, ADR-0008). One server per user
(ADR-0003); one tokio current-thread runtime; UDS transport with a
QUIC future (ADR-0007).

Authoritative docs, in order of priority:

- [`SPEC.md`](./SPEC.md) — normative wire protocol. Code conforms to
  it, not vice versa.
- [`ARCHITECTURE.md`](./ARCHITECTURE.md) — internal structure (process
  model, crate graph, data model, modules).
- [`DESIGN.md`](./DESIGN.md) — user-facing surface (CLI, config,
  keybindings, status bar, hooks).
- [`ADR/`](./ADR/) — decisions, with rationale and tradeoffs.

Crates: `phux-protocol` (wire), `phux-core` (domain), `phux-server`
(daemon), `phux-client` (renderer + diff mirror), `phux-config` (TOML
+ widgets), `phux` (binary). `phux-protocol` is publishable; the rest
are `publish = false`.

## Conventions & Patterns

- **No emojis in committed files.** Plain prose only.
- **Conventional commits.** `feat(scope): ...`, `fix(scope): ...`,
  `docs(scope): ...`, `chore(scope): ...`.
- **SPEC.md is normative.** Wire changes update SPEC + bump version
  (see CONTRIBUTING.md). Wire bytes are owned by `phux-protocol`;
  `phux-server` and `phux-client` consume them.
- **ADR for any decision that closes off a design space.** Bug fixes
  don't need one; "should this be in `core` or `server`?" does.
- **`unsafe` requires a `// SAFETY:` comment.** Library crates default
  to `forbid(unsafe_code)`.
- **No new deps without a paragraph of justification in the PR.**
- **Linear history on `main`.** Rebase, ff-only merges; no `--no-ff`.
- **Multi-agent fan-out uses self-managed worktrees** — see
  CONTRIBUTING.md §"Multi-agent fan-out" for the wave-1 race that
  motivated this.
