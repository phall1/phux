---
audience: contributors, agents
stability: stable
last-reviewed: 2026-06-06
---

# Architecture Decision Records

**TL;DR.** Index of every decision that has closed off a design space
in phux. Format and `Status:` vocabulary defined in
[`../docs/CONVENTIONS.md`](../docs/CONVENTIONS.md). Read these when
you need to know *why* something is the way it is — the architecture
docs describe *what* the code is.

We write down decisions so future contributors (including future-us) can
understand why the system is the way it is. Format follows [Michael
Nygard's template][nygard].

[nygard]: https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions

## Index

<!--
This table should be generated from the ADR/0*.md files (each file's first
`# ` heading is the title, its `Status:` line is the status) rather than
hand-maintained. It drifted before — the table stopped at 0028 while 0029
and 0030 existed, and 0022 was missing entirely. Regenerate it instead of
editing rows by hand, or the same drift recurs. The relationship
annotations in the Status column (supersedes / refines / builds on /
amends / extends) are hand-curated from each ADR's body.
-->

| # | Decision | Status |
|---|----------|--------|
| [0001](./0001-language-rust.md) | Use Rust | Accepted |
| [0002](./0002-diff-based-protocol.md) | Diff-based wire protocol, not VT byte replay | Superseded by [0013](./0013-libghostty-bytes-on-wire.md) |
| [0003](./0003-server-process-model.md) | Single server, many sessions | Accepted |
| [0004](./0004-libghostty-vt-as-grid.md) | libghostty-vt is the canonical grid | Accepted |
| [0005](./0005-relationship-to-zmx-and-zmosh.md) | Relationship to zmx and zmosh | Accepted |
| [0006](./0006-input-mirrors-libghostty.md) | Input event types re-export libghostty-vt's atoms | Accepted (amended by [0024](./0024-wire-owns-input-atoms.md)) |
| [0007](./0007-mosh-class-transport-and-satellites.md) | Mosh-class transport semantics and satellite forward-compat | Accepted (forward-compat) |
| [0008](./0008-use-libghostty-types-directly.md) | Use libghostty-vt's types directly; stop reimplementing them | Accepted (amended by [0024](./0024-wire-owns-input-atoms.md)) |
| [0009](./0009-phux-vs-mux-positioning.md) | phux vs coder/mux: positioning | Accepted |
| [0010](./0010-frontend-agnostic-tmux-cc-reserved.md) | phux is TUI-first, non-TUI not precluded; tmux control mode reserved as compat option | Accepted (forward-compat) |
| [0011](./0011-protocol-core-independence.md) | `phux-protocol` and `phux-core` are independent; `IdBridge` is their only meeting point | Accepted |
| [0012](./0012-binary-split-tree-layout.md) | Window layout is a binary split tree, not n-ary | Accepted |
| [0013](./0013-libghostty-bytes-on-wire.md) | Libghostty bytes on the wire; structured input remains | Accepted (supersedes [0002](./0002-diff-based-protocol.md)) |
| [0014](./0014-server-terminal-pane-actor.md) | Server-side `Terminal` placement: per-pane PaneActor on a `LocalSet` | Accepted |
| [0015](./0015-protocol-layering.md) | Protocol layering: L1 substrate, L2 collections, L3 metadata | Accepted (L2 tier dissolved by [0030](./0030-engine-delegated-wire-and-projection-consumers.md)) |
| [0016](./0016-terminal-id-as-wire-primary.md) | `TerminalId` as the wire primary; `PaneId` is a consumer-side alias | Accepted |
| [0017](./0017-tui-not-protocol-privileged.md) | The reference TUI is not protocol-privileged | Accepted (refines [0010](./0010-frontend-agnostic-tmux-cc-reserved.md)) |
| [0018](./0018-lazy-state-synchronization.md) | Lazy state synchronization is the wire's long-arc shape | Accepted (builds on [0013](./0013-libghostty-bytes-on-wire.md)) |
| [0019](./0019-tui-multi-pane-rendering.md) | Multi-pane TUI rendering: layout persistence, wire shape, and chrome | Accepted |
| [0020](./0020-layered-render.md) | Layered render: ratatui chrome over libghostty pane interiors | Accepted |
| [0021](./0021-control-plane-commands.md) | Control-plane commands and client-side selector resolution | Accepted (builds on [0017](./0017-tui-not-protocol-privileged.md)) |
| [0022](./0022-tool-for-agents.md) | phux as a tool for agents | Accepted |
| [0023](./0023-config-ux-philosophy.md) | Config UX: pure-config, defaults as a live base layer | Accepted (TUI-local, builds on [0017](./0017-tui-not-protocol-privileged.md)) |
| [0024](./0024-wire-owns-input-atoms.md) | The wire protocol owns its input atoms | Accepted (amends [0006](./0006-input-mirrors-libghostty.md), [0008](./0008-use-libghostty-types-directly.md)) |
| [0025](./0025-browser-web-client.md) | Browser web client over a WebSocket transport | Accepted (builds on [0017](./0017-tui-not-protocol-privileged.md), [0024](./0024-wire-owns-input-atoms.md)) |
| [0026](./0026-overlays-theme-stack-single-dispatch.md) | Overlays: one theme, a real stack, and a single dispatch path | Accepted (builds on [0020](./0020-layered-render.md)) |
| [0027](./0027-terminal-references-and-l3-links.md) | Terminals are referenced, not owned: views, links, and L3 tags | Accepted (builds on [0017](./0017-tui-not-protocol-privileged.md), [0015](./0015-protocol-layering.md)) |
| [0028](./0028-runtime-log-control.md) | Runtime log control | Accepted (forward-compat, builds on [0024](./0024-wire-owns-input-atoms.md)) |
| [0029](./0029-one-cursor-authority-and-repaint-scheduler.md) | One cursor authority and a repaint scheduler | Accepted (forward-compat, extends [0020](./0020-layered-render.md)) |
| [0030](./0030-engine-delegated-wire-and-projection-consumers.md) | Engine-delegated wire and projection consumers | Accepted (supersedes the L2 tier of [0015](./0015-protocol-layering.md)) |
| [0031](./0031-remote-consumer-auth-and-encryption.md) | Remote-consumer authentication and encryption (no SSH tunnel) | Proposed |
| [0032](./0032-graceful-server-upgrade.md) | Graceful server upgrade (sessions survive a binary update) | Accepted |
| [0033](./0033-input-authority-and-process-signals.md) | Input authority leases and process signals ("take the wheel + kill") | Accepted |
| [0034](./0034-kitty-graphics-image-passthrough.md) | Kitty graphics / image passthrough through the cell renderer | Proposed |

## When to write an ADR

- Picking between viable approaches with long-term consequences.
- Closing off a design space (deciding *against* something).
- Anything you'd want to explain to a new contributor on day one.

## When NOT to write an ADR

- Bug fixes.
- Refactors that don't change behavior.
- Anything purely internal to a single function.

## Template

```
# NNNN — Short title

Status: Proposed | Accepted | Deprecated | Superseded by ADR-NNNN
Date: YYYY-MM-DD

## Context
What is the situation that calls for a decision?

## Decision
What was decided.

## Rationale
Why this and not the alternatives.

## Tradeoffs
What we give up.

## Alternatives considered
Brief sketch of the other candidates and why they lost.
```
