---
audience: consumers, contributors, agents
stability: stable
last-reviewed: 2026-05-28
---

# Appendix B — Reserved ranges

**TL;DR.** The reserved-discriminant ranges for future protocol
extensions: which message-ID slots are earmarked for which categories
(lifecycle, hot path, control plane, events, L1 lifecycle), and the
enum-allocation discipline for `PhysicalKey` and `ErrorCode`.
Implementers extending the protocol pick from these ranges via PR.

---

## 1. Reserved message-ID ranges

For implementers extending the protocol:

- Message IDs `0x04..=0x0F` and `0x83..=0x8F`: reserved for connection-
  lifecycle messages.
- Message IDs `0x14..=0x1F` and `0x93..=0x9F`: reserved for hot-path
  messages.
- Message IDs `0x24..=0x2F` and `0xA3..=0xAF`: reserved for further L1
  Terminal lifecycle / per-pane control frames (phux-4li.10 allocated
  `0x22..=0x23` C→S and `0xA1..=0xA2` S→C from these ranges).
- Message IDs `0x31..=0x3F` and `0xC2..=0xCF`: reserved for control
  plane.
- Message IDs `0x41..=0x4F` and `0xB3..=0xBF`: reserved for events
  (phux-y2t allocated `SUBSCRIBE_EVENTS = 0x41` C→S and `EVENT = 0xB3`
  S→C from these ranges; `0x42..=0x4F` and `0xB4..=0xBF` remain open).

## 2. Reserved enum ranges

`PhysicalKey` enum values and `ErrorCode` enum values are allocated
sequentially. Implementers proposing new values open a PR against
this document.

(Earlier drafts of the SPEC reserved a `DiffOp` tag range here; per
[ADR-0013](../../ADR/0013-libghostty-bytes-on-wire.md), Terminal
content is now a VT byte stream and `DiffOp` no
longer exists as a wire concept.)
