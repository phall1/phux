---
audience: humans, agents, consumers, contributors
stability: stable
last-reviewed: 2026-06-06
---

# docs/

**TL;DR.** The doc tree and the order to read it in. Concepts and consumer
guides are single files; the normative wire spec and the architecture
description split per concept under their own subdirectories. Each subtree
has its own README index. Editing docs is governed by CONVENTIONS, which is
the law on frontmatter, the TL;DR rule, the ADR template, and the CI gates.

---

## Read in this order

To understand phux, top to bottom:

1. [`CONCEPTS.md`](./CONCEPTS.md) — what phux is, the terminal as the unit,
   human and agent as peer consumers, and the current maturity status
   (phux is pre-1.0 and still spec-led). Read this even if you read nothing
   else.
2. [`QUICKSTART.md`](./QUICKSTART.md) — the nix dev-shell and `just ci`
   setup, then get a session running and poke at the TUI, agent, MCP, plugin,
   and workspace surfaces that work today.
3. [`spec/`](./spec/) — the normative wire surface, versioned with
   `phux-protocol`. Start at [`spec/README.md`](./spec/README.md).
4. [`consumers/`](./consumers/) — how the CLI, agents, MCP, TUI, and web
   client drive the wire. Start at
   [`consumers/README.md`](./consumers/README.md).
5. [`architecture/`](./architecture/) — how it is built: process model,
   threading, transport, rendering, verification. Start at
   [`architecture/README.md`](./architecture/README.md).
6. [`../ADR/README.md`](../ADR/README.md) — the decision index. Read
   [`../ADR/0030-engine-delegated-wire-and-projection-consumers.md`](../ADR/0030-engine-delegated-wire-and-projection-consumers.md)
   to understand why structured views are consumer-side projections, not a
   wire tier.

## Pick a lane

| You are | Go to |
|---|---|
| Understanding phux | [`CONCEPTS.md`](./CONCEPTS.md) |
| Installing it | [`INSTALL.md`](./INSTALL.md) |
| Setting up the dev shell / running it | [`QUICKSTART.md`](./QUICKSTART.md) |
| Configuring keys / status bar | [`CONFIG.md`](./CONFIG.md) |
| Driving it from an agent | [`consumers/agents.md`](./consumers/agents.md) (CLI) · [`consumers/mcp.md`](./consumers/mcp.md) (MCP) |
| Composing an agent bench | [`CONFIG.md`](./CONFIG.md#plugins) · [`../examples/plugins/agent-tools/README.md`](../examples/plugins/agent-tools/README.md) |
| Writing a different consumer | [`consumers/tui.md`](./consumers/tui.md) · [`consumers/web.md`](./consumers/web.md) |
| Implementing against the wire | [`spec/README.md`](./spec/README.md) — normative, versioned with `phux-protocol` |
| Reading how it is built | [`architecture/README.md`](./architecture/README.md) — process model, threading, transport, rendering, verification |
| Operating it | [`operations.md`](./operations.md) — errors, logging, telemetry, security boundaries |
| Shipping a release | [`RELEASING.md`](./RELEASING.md) — preflight, GitHub Actions release button, Homebrew, crates.io |
| Understanding a past decision | [`../ADR/README.md`](../ADR/README.md) |
| Recording the README demo | [`demo.md`](./demo.md) |
| Touching the docs themselves | [`CONVENTIONS.md`](./CONVENTIONS.md) |

## What is deliberately not here

- **Code-level docs** live in `crates/*/src/` as rustdoc.
  `cargo doc --workspace --all-features` renders them.
- **Scratch research** lives in [`../research/`](../research/) at
  `stability: scratch`. Once a finding is ratified it graduates into an ADR
  or one of the reference docs above; it does not linger here as half-truth.
</content>
</invoke>
