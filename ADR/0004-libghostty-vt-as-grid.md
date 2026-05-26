# 0004 — libghostty-vt is the canonical grid

> **Post-ADR-0013 note (2026-05-25):** ADR-0013 supersedes ADR-0002
> (bytes-on-wire replaces structured cell diffs). This ADR is
> unaffected in substance — libghostty-vt is still the canonical
> server-side grid — but the *use* the server makes of it changed:
> the server now forwards PTY bytes directly on `PANE_OUTPUT` and
> only consults the libghostty `Terminal` to synthesize `PANE_SNAPSHOT`
> VT replay bytes on attach (mosh-style: walk `grid_ref()`, emit
> SGR runs + cells). It no longer "emits diffs from there." Inline
> wording below has been touched up; the decision itself is intact.

Status: Accepted
Date: 2026-05-24

> **Update 2026-05-26:** [ADR-0013](./0013-libghostty-bytes-on-wire.md)
> supersedes the diff-based wire; libghostty-vt is now the grid
> representation on BOTH ends (server and client). The "diff" wording
> in passages below and in the post-ADR-0013 note above refers to the
> deleted architecture. [ADR-0016](./0016-terminal-id-as-wire-primary.md)
> renamed `PaneId → TerminalId` and `PANE_* → TERMINAL_*` at the wire
> level (commit `9f4bb2e`); references to `PANE_OUTPUT` /
> `PANE_SNAPSHOT` below should be read as `TERMINAL_OUTPUT` /
> `TERMINAL_SNAPSHOT`. The "every pane has a terminal screen state"
> framing remains correct in spirit — under ADR-0015's L1/L2/L3
> layering, "pane" is a TUI-consumer concept and the wire entity is a
> Terminal.

## Context

Every pane has a terminal screen state — the grid, plus scrollback,
plus modes, plus cursor. The server must maintain this state because
it is the source of truth that snapshots are synthesized from when a
client attaches, and (originally) the source of truth that wire-side
diffs were computed from (ADR-0002, now superseded by ADR-0013 —
content flows as VT bytes on the wire; the server-side grid still
backs `PANE_SNAPSHOT` synthesis).

The implementation can be:

- A hand-written grid (tmux: `grid.c`, ~1600 LOC, plus `screen-write.c`,
  ~2500 LOC, plus `input.c` for VT parsing).
- `libghostty_vt::Terminal` from the safe Rust crate over libghostty-vt.

## Decision

Per-pane state is a `libghostty_vt::Terminal`. The server feeds PTY
output into it and forwards those same bytes to attached clients via
`PANE_OUTPUT` (ADR-0013); on attach, the server walks the resulting
grid via `RenderState` / `grid_ref()` to synthesize the
`PANE_SNAPSHOT` VT replay bytes that catch a new client up.

## Rationale

- **Correctness.** Terminal emulation is significantly harder than it
  looks: VT sequences, SGR, OSC, DEC private modes, modes within
  modes, sixel and kitty graphics, character sets, scrolling regions,
  unicode width and grapheme clustering. libghostty's terminal core
  is the most standards-compliant implementation available, and it
  has been hammered on by real users running real workloads.
- **Free upgrades.** Bugs fixed upstream become bugs fixed in phux.
  Newly supported VT features arrive automatically.
- **Reduces phux's surface.** We don't ship a VT parser. Our scope is
  multiplexing, layout, IPC, and rendering — not terminal emulation.
  This is the largest single way phux is smaller than tmux.
- **Aligned with the protocol shape.** libghostty-vt's `RenderState`
  exposes a row/cell iterator API that drives `PANE_SNAPSHOT` byte
  synthesis on attach (ADR-0013) and provides local dirty tracking
  on the server's own grid. (Pre-ADR-0013 this same iterator drove
  cell-level diff emission; the iterator is just as well-suited to
  the byte-synthesis path.)

## Tradeoffs

- **Coupling to libghostty's release cadence.** We pin a specific
  commit and upgrade deliberately. libghostty-rs's vendored build
  mode makes the build hermetic per pin.
- **API churn risk.** Mitigated by libghostty-vt's stable C ABI and
  the safe Rust crate's semver discipline.

## Alternatives considered

- **Hand-written grid (tmux's approach).** Reimplements work that
  exists, less standards-compliant in the wild, and ties the project's
  fate to our ability to keep up with terminal emulation as it
  continues to evolve (kitty kbd, OSC 133, image protocols, …).
  Rejected.
- **vte/alacritty/wezterm terminal cores.** All viable alternatives,
  but none are a first-class Rust library with the same combination
  of completeness, standards compliance, and ongoing investment. We
  bet on libghostty.
