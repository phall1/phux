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
