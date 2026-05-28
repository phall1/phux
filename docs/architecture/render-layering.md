---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-27
---

# Render layering: ratatui chrome over libghostty pane interiors

**TL;DR.** phux-client uses two renderers on disjoint screen regions.
libghostty paints pane interiors on the hot path so kitty graphics,
sixel, OSC 8, and the Kitty key protocol pass through unchanged.
ratatui paints the chrome (status bar, dividers, modals); the layers
composite rather than interleave. CI grep-guards the boundary.

---

Under epic `phux-5ke` (and TBD `ADR-0020`) phux-client uses two
renderers for disjoint screen regions. libghostty paints pane interiors
on the hot path — kitty graphics, sixel, OSC 8 hyperlinks, and the
Kitty key protocol all pass through unchanged. `ratatui` paints the
chrome: status bar, pane dividers, borders, modals, future tab bar.
The layers composite rather than interleave; chrome carves skip-cell
rectangles for pane rects so libghostty owns those cells exclusively.

The `ratatui` dependency is scoped to `crates/phux-client/src/render/`
(submodules `chrome` and `overlay`). Attach loop, pane mirror, predict
layer, and layout math stay ratatui-free. A CI grep guard,
`scripts/check-ratatui-boundary.sh`, runs from `just ci` and fails the
build if a `ratatui` import appears outside `render/`.
