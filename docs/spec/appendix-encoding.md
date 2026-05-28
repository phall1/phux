---
audience: consumers, contributors, agents
stability: stable
last-reviewed: 2026-05-28
---

# Appendix A — Encoding primitives

**TL;DR.** The self-delimiting field encoding used by every phux wire
payload: field-tagged TLVs, eight wire types (varint, signed varint,
fixed-32, fixed-64, bytes, message, list, tagged), and the
extensibility rules that let decoders skip unknown fields by length.
Big-endian throughout, protobuf-shaped but tailored to phux's tagged
unions.

---

## 1. Field encoding

Every payload is encoded as a sequence of fields. Fields are
self-delimiting: a decoder can skip an unknown field without knowing its
semantics.

A field is `{ field_id: varint, wire_type: u8, value: ... }`, where
`wire_type` determines how `value` is encoded:

| wire_type | Name       | Encoding                                          |
|-----------|------------|---------------------------------------------------|
| 0         | `VARINT`   | LEB128 unsigned integer                           |
| 1         | `SVARINT`  | LEB128 zig-zag signed integer                     |
| 2         | `FIXED32`  | 4 bytes, big-endian                               |
| 3         | `FIXED64`  | 8 bytes, big-endian                               |
| 4         | `BYTES`    | `varint length || bytes`                          |
| 5         | `MESSAGE`  | `varint length || nested encoded fields`          |
| 6         | `LIST`     | `varint length || elements with type prefix`      |
| 7         | `TAGGED`   | `varint tag || nested encoded fields`             |

Messages and tagged unions are encoded as a sequence of fields, each
prefixed with its `field_id` and `wire_type`. Decoders match by
`field_id` (not by position) and skip unknown `field_id`s by reading
their declared `wire_type`.

This format is intentionally similar in spirit to Protocol Buffers'
wire format, but designed for the specific concerns of this protocol:

- Big-endian for hex-dump readability and "network feel".
- No `varint`-only restriction on integers; fixed widths exist where
  natural (e.g. timestamps, color channels) so the wire matches the
  conceptual width.
- A first-class `TAGGED` wire type for tagged unions, so they don't have
  to be reified as `oneof`-style hacks.

A canonical hex dump of a `HELLO_OK` selecting version `0.1.0` is
included in `crates/phux-protocol/tests/snapshots/hello_ok_v0_1_0.snap`
once the codec exists.
