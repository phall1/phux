---
audience: agents, contributors
stability: stable
last-reviewed: 2026-05-28
---

# phux Project Instructions for Agents

See [`AGENTS.md`](./AGENTS.md) for universal agent instructions
(shell hygiene, session completion protocol). This file adds
phux-project-specific guidance: build, test, architecture, and conventions.

---

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
- [`docs/CONVENTIONS.md`](./docs/CONVENTIONS.md) — doc system,
  frontmatter, TL;DR rule, ADR template.
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
