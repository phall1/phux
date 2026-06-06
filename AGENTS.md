---
audience: agents, contributors
stability: stable
last-reviewed: 2026-05-28
---

# Agent Instructions

**TL;DR.** Universal agent substrate for this repo: non-interactive
shell hygiene (always pass `-f` / `-y` flags), the beads-tracker
integration block (auto-maintained), and the session-close protocol.
For project-specific guidance see [`CLAUDE.md`](./CLAUDE.md); for the
doc system see [`docs/CONVENTIONS.md`](./docs/CONVENTIONS.md).

Tracker integration and command reference live in the auto-injected
`<!-- BEGIN BEADS INTEGRATION -->` block below — it is the canonical
source and stays in sync with the `bd` tool. This file's hand-maintained
sections cover the rest: shell-command hygiene, project conventions, and
anything the auto-block doesn't.

## Non-Interactive Shell Commands

**ALWAYS use non-interactive flags** with file operations to avoid hanging on confirmation prompts.

Shell commands like `cp`, `mv`, and `rm` may be aliased to include `-i` (interactive) mode on some systems, causing the agent to hang indefinitely waiting for y/n input.

**Use these forms instead:**
```bash
# Force overwrite without prompting
cp -f source dest           # NOT: cp source dest
mv -f source dest           # NOT: mv source dest
rm -f file                  # NOT: rm file

# For recursive operations
rm -rf directory            # NOT: rm -r directory
cp -rf source dest          # NOT: cp -r source dest
```

**Other commands that may prompt:**
- `scp` - use `-o BatchMode=yes` for non-interactive
- `ssh` - use `-o BatchMode=yes` to fail instead of prompting
- `apt-get` - use `-y` flag
- `brew` - use `HOMEBREW_NO_AUTO_UPDATE=1` env var

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:7510c1e2 -->
## Beads Issue Tracker

This project uses **bd (beads)** for *public phux framework work* only. Run `bd prime` to see full workflow context and commands.

Tradecraft, offensive-security, and GHOST-specific planning lives in the private repo at **`../phux-tradecraft/.beads/`**. Do NOT file tradecraft issues in this public tracker.

### Quick Reference

```bash
bd ready              # Find available public phux work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` in this repo ONLY for public phux framework work (protocol, server, client, rendering, docs, tests)
- File tradecraft / offensive-capability issues in `../phux-tradecraft` — never in this public tracker
- Do NOT use TodoWrite, TaskCreate, or markdown TODO lists in either repo
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
