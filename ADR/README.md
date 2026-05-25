# Architecture Decision Records

We write down decisions so future contributors (including future-us) can
understand why the system is the way it is. Format follows [Michael
Nygard's template][nygard].

[nygard]: https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions

## Index

| # | Decision | Status |
|---|----------|--------|
| [0001](./0001-language-rust.md) | Use Rust | Accepted |
| [0002](./0002-diff-based-protocol.md) | Diff-based wire protocol, not VT byte replay | Accepted |
| [0003](./0003-server-process-model.md) | Single server, many sessions | Accepted |
| [0004](./0004-libghostty-vt-as-grid.md) | libghostty-vt is the canonical grid | Accepted |
| [0005](./0005-relationship-to-zmx-and-zmosh.md) | Greenfield relative to zmx / zmosh | Accepted |
| [0006](./0006-input-mirrors-libghostty.md) | Input event types mirror libghostty's API | Accepted |
| [0007](./0007-mosh-class-transport-and-satellites.md) | Mosh-class transport semantics and satellite forward-compat | Accepted (forward-compat); impl deferred to v0.2+ |
| [0008](./0008-use-libghostty-types-directly.md) | Re-export libghostty-vt's input/style types directly | Accepted |
| [0009](./0009-phux-vs-mux-positioning.md) | phux is a protocol substrate; Mux is a product (no overlap) | Accepted |
| [0010](./0010-frontend-agnostic-tmux-cc-reserved.md) | Frontend-agnostic server; tmux control mode reserved as compat | Accepted (forward-compat); CC adapter not on the roadmap |
| [0011](./0011-protocol-core-independence.md) | `phux-protocol` and `phux-core` are independent; `IdBridge` is their only meeting point | Accepted |

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
