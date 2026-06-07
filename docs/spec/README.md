---
audience: consumers, contributors, agents
stability: stable
last-reviewed: 2026-06-06
---

# docs/spec/

**TL;DR.** A map of the normative phux wire protocol, split by tier.
Each file is the source of truth for one part of the wire that
`phux-protocol` and downstream consumers implement. RFC-2119
SHALL / SHOULD / MAY language is used only inside this directory.

---

## Files

| File | Owns |
|---|---|
| [TUTORIAL.md](./TUTORIAL.md) | **Start here:** a complete session walkthrough (HELLO → attach → output → input → detach) |
| [proto.md](./proto.md) | Framing, version negotiation, capabilities, flow control, transport |
| [L1.md](./L1.md) | Terminal substrate — the REQUIRED conformance tier |
| [L2.md](./L2.md) | Reserved, unused — no collection tier (dissolved per ADR-0030) |
| [L3.md](./L3.md) | Metadata storage — OPTIONAL |
| [input.md](./input.md) | INPUT_KEY / INPUT_MOUSE / INPUT_FOCUS / INPUT_PASTE / INPUT_RAW |
| [appendix-encoding.md](./appendix-encoding.md) | Encoding primitives and the normative payload shape (positional, big-endian, length-prefixed) |
| [appendix-reserved.md](./appendix-reserved.md) | Reserved discriminant ranges |
| [CHANGELOG.md](./CHANGELOG.md) | Wire-format change log, version-stamped |

## Version

The current protocol version lives in `crates/phux-protocol/src/`
(grep `PROTOCOL_VERSION`). The top entry in [CHANGELOG.md](./CHANGELOG.md)
must match it; CI gate `spec-version-sync` enforces this.
