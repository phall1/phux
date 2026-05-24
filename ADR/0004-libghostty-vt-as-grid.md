# 0004 — libghostty-vt is the canonical grid

Status: Accepted
Date: 2026-05-24

## Context

Every pane has a terminal screen state — the grid, plus scrollback,
plus modes, plus cursor. The server must maintain this state because
it is the source of truth for diffs (ADR-0002).

The implementation can be:

- A hand-written grid (tmux: `grid.c`, ~1600 LOC, plus `screen-write.c`,
  ~2500 LOC, plus `input.c` for VT parsing).
- `libghostty_vt::Terminal` from the safe Rust crate over libghostty-vt.

## Decision

Per-pane state is a `libghostty_vt::Terminal`. The server feeds PTY
output into it, queries the resulting grid via `RenderState`, and
emits diffs from there.

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
  exposes a row/cell iterator API that maps cleanly to our cell-level
  diff format.

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
