---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-30
---

# 0023 — The wire protocol owns its input atoms

**TL;DR.** [ADR-0008] reused libghostty-vt's `Key`/`Action`/`Mods` types
directly as the wire's input atoms ("no parallel universe of types"). That
couples `phux-protocol`'s wire to libghostty-vt, which is FFI-backed (a zig
static lib) and does not build for `wasm32` — so the codec can't compile for a
browser consumer (`phux-web`, ADR-0017's wire-consumer thesis). This ADR amends
ADR-0008, scoped to the wire: **`phux-protocol` defines its own
wire-representation input atoms** (`PhysicalKey` as a `u32` newtype over the
libghostty key discriminant; `KeyAction`/`ModSet`/focus/mouse as small native
types). The libghostty engine boundary is where atoms convert — `From`/`TryFrom`
impls compiled only under the `server` feature. The wire stays byte-identical;
no protocol/version change.

Status: Accepted
Date: 2026-05-30

## Context

`phux-protocol` is the one crate meant for external consumption and is the wire
contract every consumer speaks (ADR-0017: the TUI is not protocol-privileged; an
agent SDK and a browser client are peers). Per ADR-0008, its `input` module did
`pub use libghostty_vt::key::{Action, Key, Mods}` — the wire atoms *were*
libghostty's types. Both `pub mod input` and `pub mod wire` were gated behind
the `server` feature, which depends on `libghostty-vt`.

That gate means the default (libghostty-free) build of `phux-protocol` has **no
codec at all** — only `caps`/`ids`. A wasm consumer needs the `FrameKind` codec
(to speak `HELLO`/`ATTACH` and decode `TERMINAL_OUTPUT`/`TERMINAL_SNAPSHOT`), but
turning on `server` pulls `libghostty-vt`, whose `build.rs` builds/links a zig
static lib and `panic!`s on non-native targets. The wire is therefore unbuildable
on `wasm32`, blocking the browser client.

The atoms enter the wire only through five small re-exports
(`input/key.rs`, `mouse.rs`, `focus.rs`). On the wire they are already plain
integers (`key` is a `u32` discriminant, `mods` a `u16` bitset). The server is
the only place that needs the *libghostty* form — its per-terminal encoders feed
events to libghostty's key/mouse encoders to produce PTY bytes.

## Decision

1. `phux-protocol` defines its own wire input atoms, libghostty-free:
   `PhysicalKey(u32)` (transparent newtype over the W3C key discriminant),
   `KeyAction` (Press/Release/Repeat), `ModSet` (`bitflags`, `u16`), and the
   focus/mouse atoms. The wire encode/decode is unchanged (same bytes).
2. `pub mod input` and `pub mod wire` are no longer gated behind `server`; the
   codec builds with default (libghostty-free) features and so compiles for
   `wasm32`.
3. `libghostty-vt` becomes an optional dependency, enabled by `server`. Under
   `server`, `phux-protocol` provides `From`/`TryFrom` conversions between its
   atoms and libghostty's, so the server's encoders convert at their boundary.

## Rationale

A wire protocol should own its wire-format types; depending on a specific
engine's Rust type layout for the *protocol* is a layering inversion. The
conversion belongs exactly where bytes meet the engine — the server's encoders —
not in the shared contract. This unblocks every non-native consumer (browser,
and future no-FFI SDKs) without a protocol change, and keeps libghostty as the
canonical *engine* (ADR-0008's real intent) while it is no longer the canonical
*wire vocabulary*.

## Tradeoffs

- Two representations of the key discriminant now exist (phux `u32` newtype +
  libghostty enum), kept in lockstep by the conversion impls + a round-trip test.
  We accept this seam to decouple the wire from the engine.
- `PhysicalKey` is a `u32` newtype, not a 170-variant enum, so it is less
  self-documenting than libghostty's `Key`. Named consts cover the common keys;
  exhaustiveness lives in libghostty (consulted via the `server` conversion).

## Alternatives considered

- **Keep ADR-0008, make libghostty-vt build on wasm.** Would preserve a single
  type universe, but requires either compiling the zig FFI into the wasm binary
  (the rust+zig shared-linear-memory problem) or a types-only build mode of
  libghostty-vt — upstream work with real risk, for a worse layering.
- **Mirror libghostty's `Key` as a 170-variant enum in phux-protocol.** Faithful
  but a maintenance burden that silently drifts; the `u32` newtype + conversion
  is leaner and the drift is caught by the round-trip test.

[ADR-0008]: 0008-use-libghostty-types-directly.md
[ADR-0017]: 0017-tui-not-protocol-privileged.md
