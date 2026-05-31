---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-31
---

# Render layering: ratatui chrome over libghostty pane interiors

**TL;DR.** phux-client uses two renderers on disjoint screen regions.
libghostty paints pane interiors on the hot path so kitty graphics,
sixel, OSC 8, and the Kitty key protocol pass through unchanged.
ratatui paints the chrome (status bar, dividers, modals); the layers
composite rather than interleave. A crate split makes the boundary
compiler-enforced.

---

Under epic `phux-5ke` (and `ADR-0020`) phux-client uses two
renderers for disjoint screen regions. libghostty paints pane interiors
on the hot path — kitty graphics, sixel, OSC 8 hyperlinks, and the
Kitty key protocol all pass through unchanged. `ratatui` paints the
chrome: status bar, pane dividers, borders, modals, future tab bar.
The layers composite rather than interleave; chrome carves skip-cell
rectangles for pane rects so libghostty owns those cells exclusively.

The `ratatui` dependency is scoped to a single crate, `phux-client`
(under `src/render/`, submodules `chrome` and `overlay`). The
pane-interior substrate — pane mirror, predict layer, layout math, and
multi-pane composition — lives in a separate crate, `phux-client-core`,
which carries **no `ratatui` dependency**. The boundary is therefore
enforced by the compiler: a `use ratatui` in the substrate fails to
build because the crate cannot name it. This replaced the original
`scripts/check-ratatui-boundary.sh` grep guard with phux-0fv. The attach
loop stays in `phux-client` (it composites chrome over panes, so it
legitimately depends on both the chrome and the substrate).
