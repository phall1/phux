---
audience: contributors, agents
stability: stable
last-reviewed: 2026-06-03
---

# docs/

**TL;DR.** The doc tree, and the order to read it in. Concepts and
consumer guides are single files; the normative wire spec and the
architecture description are split per concept under their own
subdirectories. New here? [`CONCEPTS.md`](./CONCEPTS.md) first, then pick
a lane below. Adding or moving docs? [`CONVENTIONS.md`](./CONVENTIONS.md)
first — it's the law (frontmatter, TL;DR rule, ADR template, CI gates).

---

## Read in this order

If you're just trying to understand phux, top to bottom:

1. [`CONCEPTS.md`](./CONCEPTS.md) — what phux actually is. The terminal
   as the unit; human and agent as peer consumers; the layered wire.
   Read this even if you read nothing else.
2. [`QUICKSTART.md`](./QUICKSTART.md) — get a session running and poke at it.
3. [`vision.md`](./vision.md) — where it's headed, and why the v0.1 wire
   is already shaped for it.

## Pick a lane

| You are | Go to |
|---|---|
| Installing it | [`INSTALL.md`](./INSTALL.md) |
| Running it for the first time | [`QUICKSTART.md`](./QUICKSTART.md) |
| Configuring keys / status bar | [`CONFIG.md`](./CONFIG.md) |
| Driving it from an agent | [`consumers/agents.md`](./consumers/agents.md) (CLI) · [`consumers/mcp.md`](./consumers/mcp.md) (MCP) |
| Writing a different consumer | [`consumers/tui.md`](./consumers/tui.md) · [`consumers/web.md`](./consumers/web.md) |
| Implementing against the wire | [`spec/`](./spec/) — normative, versioned with `phux-protocol` |
| Reading how it's built | [`architecture/`](./architecture/) — process model, threading, transport, crate graph |
| Operating it | [`operations.md`](./operations.md) — errors, logging, telemetry, security boundaries |
| Understanding a past decision | [`../ADR/README.md`](../ADR/README.md) |
| Touching the docs themselves | [`CONVENTIONS.md`](./CONVENTIONS.md) |

[`../ADR/`](../ADR/) holds decision records — one decision per file,
Nygard template, strict `Status:` vocabulary. Start at
[`../ADR/README.md`](../ADR/README.md) for the index.

## What's deliberately not here

- **Code-level docs** live in `crates/*/src/` as rustdoc.
  `cargo doc --workspace --all-features` renders them.
- **Scratch research** lives in [`../research/`](../research/) at
  `stability: scratch`. Once a finding is ratified it graduates into an
  ADR or one of the reference docs above — it doesn't linger here as
  half-truth.
