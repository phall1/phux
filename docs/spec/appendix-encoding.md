---
audience: consumers, contributors, agents
stability: stable
last-reviewed: 2026-06-06
---

# Appendix A — Encoding primitives

**TL;DR.** The normative encoding for every phux wire payload: positional,
big-endian, length-prefixed fields. Fields appear in a fixed order; a
decoder reads them by position and applies defaults for missing trailing
fields. A migration to field-tagged (TLV) encoding is designed but not
built; this appendix records both the current shape and that deferred
direction.

---

## 1. Encoding shape (current, normative)

Every payload SHALL be encoded as positional, big-endian,
length-prefixed fields. A message is a sequence of fields written in a
fixed, documented order; the decoder reads each field by its position,
not by a tag. This is the encoding every shipping consumer speaks today.

Field order is part of each message's definition. New fields are appended
after the existing ones. A decoder SHALL accept every prefix of a
message's field sequence and apply the documented default for any
trailing field that is absent, so that a peer encoding an older,
shorter body remains readable by a newer decoder. This is the
extensibility mechanism in the current shape: append-only trailing
fields with defaults, not skip-by-length over tagged fields.

Primitive widths and the `varint` / `bytes` / `str` / `bool` /
`optional<T>` conventions are defined in [proto.md §Conventions](./proto.md).
Multi-byte integers are big-endian on the wire, chosen for hex-dump
readability and network feel; fixed widths exist where natural (for
example timestamps and color channels) so the wire matches the
conceptual width.

A canonical hex dump of a `HELLO_OK` selecting the full tier set with an
opaque `server_id` is committed at
`crates/phux-protocol/tests/snapshots/frame_wire_snapshots__snap_hello_ok.snap`
and pinned by the `snap_hello_ok` snapshot test; any wire-format change
surfaces there as a reviewable diff.

---

## 2. Field-tagged (TLV) encoding — deferred

A field-tagged encoding — each field carrying a `field_id` and a
`wire_type` so a decoder can skip an unknown field by length and match
fields by id rather than position — is designed but not part of the
current wire. It would replace the positional shape's append-only
extensibility with id-matched, order-independent fields. The migration
from positional to field-tagged encoding is tracked future work
(bead phux-ktte, relates phux-i58).

The intended TLV layout, recorded here so the direction is concrete and
not re-derived later, is a field of `{ field_id: varint, wire_type: u8,
value: ... }` over these wire types:

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

Until that migration lands, this table describes a target, not the wire.
Implementations SHALL encode and decode the positional shape of §1.
