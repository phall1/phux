//! The monotone repaint accumulator (ADR-0029 §2, phux agent-detector work).
//!
//! ADR-0029 was Accepted with a `RepaintLevel` accumulator specified and never
//! implemented: the driver's repaint triggers each painted inline, so two in
//! one `select!` iteration double-painted. That was tolerable while the
//! triggers were rare. It stops being tolerable the moment a server-side agent
//! detector starts writing `phux.agent/v1` records, because every
//! `METADATA_CHANGED` broadcast routes to `paint_full_frame` — an `ESC[2J`
//! full-screen clear plus a forced re-render of every pane. A burst of twenty
//! coalesced metadata frames would strobe the whole screen twenty times.
//!
//! The fix has two halves. This module is the scheduler half: triggers RAISE a
//! level during the iteration and the loop DRAINS it exactly once, so N
//! triggers collapse into one paint at the highest requested level. The other
//! half is [`super::paint::paint_chrome_in_place`], the cheap level's painter —
//! sidebar strip + status bar, no `ED2`, no pane-interior re-render.
//!
//! [`RepaintLevel`] derives `Ord` in DECLARATION order, which is what makes
//! [`RepaintAccumulator::raise_chrome`] / [`RepaintAccumulator::raise_full`] a
//! monotone `max`: idempotent, order-independent, and impossible to lower.

/// How much of the frame a drained repaint must redraw. Ordered least- to
/// most-expensive; `Ord` follows declaration order so a raise is a `max`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum RepaintLevel {
    /// Nothing to do — no trigger fired this iteration.
    #[default]
    None,
    /// In-place chrome only: the sidebar strip and the status bar. No `ED2`,
    /// no pane-interior render, and the painters' own content caches make an
    /// unchanged strip/bar a zero-byte no-op.
    Chrome,
    /// The full viewport: `ED2` + every pane + dividers + chrome. Required
    /// whenever the layout (and therefore the pane rects) moved under us.
    Full,
}

/// One iteration's accumulated repaint intent (ADR-0029 §2).
///
/// Triggers raise; the loop drains once. Because the level is a `max` and the
/// drain is a single site, "two triggers in one iteration paint twice" is not
/// representable rather than merely fixed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct RepaintAccumulator {
    level: RepaintLevel,
    /// `true` once a [`RepaintLevel::Full`] was raised: that path physically
    /// clears the viewport (`ED2`), so a painter's content cache must be
    /// bypassed for the cells it wiped. Reported out of [`Self::drain`] so the
    /// caller can pass the force-full flag on.
    viewport_was_cleared: bool,
}

impl RepaintAccumulator {
    /// Request an in-place chrome repaint (sidebar strip + status bar).
    ///
    /// The cheap level: it never clears the viewport and never touches a pane
    /// interior, so it is safe to raise on every agent-state change — which,
    /// with a live detector, is the highest-frequency chrome trigger there is.
    pub(super) const fn raise_chrome(&mut self) {
        if matches!(self.level, RepaintLevel::None) {
            self.level = RepaintLevel::Chrome;
        }
    }

    /// Request a full-viewport repaint (`ED2` + every pane + chrome).
    ///
    /// Monotone: this always wins over a same-iteration `raise_chrome`, and
    /// records that the viewport was cleared.
    pub(super) const fn raise_full(&mut self) {
        self.level = RepaintLevel::Full;
        self.viewport_was_cleared = true;
    }

    /// The accumulated level plus "was the viewport cleared", resetting to
    /// [`Default`]. Called EXACTLY ONCE per loop iteration.
    pub(super) fn drain(&mut self) -> (RepaintLevel, bool) {
        let taken = std::mem::take(self);
        (taken.level, taken.viewport_was_cleared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Declaration order IS the cost order, and that is what makes `raise` a
    /// monotone max.
    #[test]
    fn levels_order_none_below_chrome_below_full() {
        assert!(RepaintLevel::None < RepaintLevel::Chrome);
        assert!(RepaintLevel::Chrome < RepaintLevel::Full);
        assert_eq!(RepaintLevel::default(), RepaintLevel::None);
    }

    /// A raise never lowers the level, and is order-independent: `full` then
    /// `chrome` and `chrome` then `full` both drain as `Full`.
    #[test]
    fn raise_is_monotone_and_order_independent() {
        let mut a = RepaintAccumulator::default();
        a.raise_full();
        a.raise_chrome();

        let mut b = RepaintAccumulator::default();
        b.raise_chrome();
        b.raise_full();

        assert_eq!(a, b);
        assert_eq!(a.drain(), (RepaintLevel::Full, true));
        assert_eq!(b.drain(), (RepaintLevel::Full, true));
    }

    /// Raising is idempotent: twenty `MetadataChanged` frames in one coalesced
    /// batch collapse into ONE `Chrome` paint. This is the whole point of the
    /// accumulator with a live agent detector upstream.
    #[test]
    fn twenty_chrome_raises_collapse_into_one_chrome_drain() {
        let mut accum = RepaintAccumulator::default();
        for _ in 0..20 {
            accum.raise_chrome();
        }
        assert_eq!(accum.drain(), (RepaintLevel::Chrome, false));
        // Drained: the next iteration starts clean, so an idle loop pass
        // paints nothing at all.
        assert_eq!(accum.drain(), (RepaintLevel::None, false));
    }

    /// A chrome-only iteration must NOT report the viewport as cleared — only
    /// the full path emits `ED2`, and only it may bypass a painter's cache.
    #[test]
    fn chrome_never_reports_a_cleared_viewport() {
        let mut accum = RepaintAccumulator::default();
        accum.raise_chrome();
        let (level, cleared) = accum.drain();
        assert_eq!(level, RepaintLevel::Chrome);
        assert!(!cleared, "chrome paints in place; it clears nothing");
    }

    /// The default (no trigger) iteration drains to `None` and paints nothing.
    #[test]
    fn untouched_accumulator_drains_to_none() {
        let mut accum = RepaintAccumulator::default();
        assert_eq!(accum.drain(), (RepaintLevel::None, false));
    }
}
