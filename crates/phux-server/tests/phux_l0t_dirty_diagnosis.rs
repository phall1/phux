//! phux-l0t — diagnose `Snapshot::dirty()` returning `Error::InvalidValue`
//! on the second `RenderState::update` call.
//!
//! Background: the wave-2 regression test (cf. commits 460faeb, 6946ccd)
//! was dropped because calling `render()` twice in a row caused
//! `snapshot.dirty()` to fail with `InvalidValue`. The client renderer
//! (`crates/phux-client/src/attach/render.rs`) defends with
//! `.unwrap_or(Dirty::Full)` as a workaround; `SnapshotSynthesizer`
//! (`crates/phux-server/src/grid.rs`) does the same in
//! `synthesize_incremental`.
//!
//! **Outcome: upstream FFI bug in `libghostty-vt` (rev `31d1f70`).**
//!
//! The precondition is **inverted** from the original hypothesis:
//!
//! - Calling `RenderState::update` repeatedly **without** `Snapshot::set_dirty`
//!   in between always returns a valid `Dirty` value.
//! - Calling `RenderState::update` after **any** `Snapshot::set_dirty(...)`
//!   call (Clean, Partial, or Full — value-agnostic) makes the next
//!   `dirty()` return `Error::InvalidValue`.
//! - Per-row `row.set_dirty(false)` alone does **not** trigger the bug.
//!
//! Root cause is in `libghostty-vt/src/render.rs::Snapshot::set` (line 343):
//!
//! ```ignore
//! fn set<T>(&self, tag: ffi::RenderStateOption::Type, value: &T) -> Result<()> {
//!     let result = unsafe {
//!         ffi::ghostty_render_state_set(
//!             self.0.0.as_raw(),
//!             tag,
//!             std::ptr::from_ref(&value).cast(), // BUG: pointer to the &T, not to T
//!         )
//!     };
//!     ...
//! }
//! ```
//!
//! `std::ptr::from_ref(&value)` takes a pointer to the **reference**
//! (`*const &T`), not to the value (`*const T`). The C side reads the
//! low bits of the pointer's address as the dirty enum, stores garbage,
//! and the next `update` either propagates or fails to overwrite that
//! garbage. `dirty()` then `try_into`s an out-of-band `u32` and returns
//! `InvalidValue`.
//!
//! The fix upstream is one character: `std::ptr::from_ref(value).cast()`.
//! Until that lands, phux must NOT depend on `Snapshot::set_dirty(...)`.
//!
//! ## Implication for ADR-0018 / phux-q0e.3
//!
//! `SnapshotSynthesizer::mark_synced` (`grid.rs`) calls
//! `snapshot.set_dirty(Dirty::Clean)`. Any tick driver that invokes
//! `mark_synced` on `FRAME_ACK` permanently poisons the per-consumer
//! `RenderState`: every subsequent `synthesize_incremental` call will
//! see `dirty() == Err(InvalidValue)` and fall back to the `Dirty::Full`
//! path (full reset + paint). Functionally correct (over-emission, not
//! under-emission) and the loss-tolerance invariant still holds, but
//! the `Dirty::Clean` and `Dirty::Partial` optimization paths are dead
//! code until upstream lands the fix.
//!
//! This file pins the contract so we get a loud assertion failure when
//! upstream behavior changes.

#![allow(clippy::expect_used, reason = "diagnostic tests")]

use libghostty_vt::{RenderState, Terminal, TerminalOptions, render::Dirty};

fn fresh(cols: u16, rows: u16) -> Terminal<'static, 'static> {
    Terminal::new(TerminalOptions {
        cols,
        rows,
        max_scrollback: 100,
    })
    .expect("Terminal::new")
}

/// Baseline: a single `update` + `dirty()` always works. Establishes that
/// the FFI isn't broken on the first call.
#[test]
fn baseline_single_update_dirty_works() {
    let terminal = fresh(20, 5);
    let mut rs = RenderState::new().expect("RenderState::new");
    let snap = rs.update(&terminal).expect("update 1");
    let _ = snap.dirty().expect("dirty 1");
}

/// Repeated `update` calls **without** calling `set_dirty` in between all
/// succeed. This is the contract the workaround relies on.
#[test]
fn repeated_updates_without_set_dirty_are_stable() {
    let terminal = fresh(20, 5);
    let mut rs = RenderState::new().expect("RenderState::new");

    for i in 0..10 {
        let snap = rs.update(&terminal).expect("update");
        let d = snap.dirty().expect("dirty must succeed without set_dirty");
        // libghostty reports Full on every update we never acknowledge;
        // the dirty bits never clear on their own. That's exactly the
        // shape q0e.3 wants — over-emission is safe, under-emission is not.
        assert_eq!(d, Dirty::Full, "iter {i}: expected Full without ack");
    }
}

/// Repeated `update` calls with `vt_write` in between, still without
/// `set_dirty`. Models the realistic production tick.
#[test]
fn repeated_updates_with_writes_no_set_dirty_are_stable() {
    let mut terminal = fresh(20, 5);
    let mut rs = RenderState::new().expect("RenderState::new");

    for _ in 0..5 {
        {
            let snap = rs.update(&terminal).expect("update");
            let _ = snap.dirty().expect("dirty must succeed");
        }
        terminal.vt_write(b"x");
    }
}

/// **Demonstrates the upstream FFI bug.** Calling `Snapshot::set_dirty`
/// between updates poisons the next `dirty()` read regardless of the value
/// passed (Clean, Partial, or Full).
///
/// This test asserts the bug **exists** so we get a loud signal when
/// upstream fixes it (the assertion will start failing, prompting us to
/// retire the workarounds in `phux-client/src/attach/render.rs` and
/// `phux-server/src/grid.rs`).
#[test]
fn upstream_bug_set_dirty_clean_poisons_next_dirty() {
    let terminal = fresh(20, 5);
    let mut rs = RenderState::new().expect("RenderState::new");

    {
        let snap1 = rs.update(&terminal).expect("update 1");
        let _ = snap1.dirty().expect("dirty 1");
        snap1
            .set_dirty(Dirty::Clean)
            .expect("set_dirty Clean reports success");
    }

    let snap2 = rs.update(&terminal).expect("update 2");
    let d2 = snap2.dirty();

    assert!(
        d2.is_err(),
        "phux-l0t: upstream bug appears fixed (dirty() now returns {d2:?}). \
         Retire the .unwrap_or(Dirty::Full) workaround in \
         crates/phux-client/src/attach/render.rs and \
         crates/phux-server/src/grid.rs::synthesize_incremental."
    );
    assert!(
        matches!(d2, Err(libghostty_vt::Error::InvalidValue)),
        "phux-l0t: bug still present but with a different error variant; \
         got {d2:?}, expected Err(InvalidValue)."
    );
}

/// The bug is value-agnostic: `set_dirty(Full)` poisons too. (If the bug
/// were value-specific, only certain enum values would corrupt.)
#[test]
fn upstream_bug_is_value_agnostic_full() {
    let terminal = fresh(20, 5);
    let mut rs = RenderState::new().expect("RenderState::new");

    {
        let snap1 = rs.update(&terminal).expect("update 1");
        let _ = snap1.dirty().expect("dirty 1");
        snap1.set_dirty(Dirty::Full).expect("set_dirty Full");
    }

    let snap2 = rs.update(&terminal).expect("update 2");
    let d2 = snap2.dirty();
    assert!(
        matches!(d2, Err(libghostty_vt::Error::InvalidValue)),
        "phux-l0t: set_dirty(Full) should also poison; got {d2:?}"
    );
}

/// Does `row.dirty()` (the per-row read) work as a signal source for the
/// state-sync tick driver, given that `Snapshot::set_dirty` is poisoned?
///
/// **Result: no.** Every row reads `dirty=true` on every update even after
/// `row.set_dirty(false)` ack on the prior tick. The per-row clear is
/// silently ineffective — most likely because libghostty's `update` path
/// re-marks every row dirty whenever the global dirty bit is non-Clean,
/// and we can't clear the global without poisoning `dirty()` (see the
/// upstream bug above). The entire dirty-signal surface is effectively
/// unusable until the upstream fix lands. Documented here so q0e.3's
/// design accounts for it.
#[test]
fn per_row_dirty_read_works_across_updates_with_per_row_clear() {
    use libghostty_vt::render::{CellIterator, RowIterator};

    let mut terminal = fresh(20, 5);
    let mut rs = RenderState::new().expect("RenderState::new");
    let mut rows = RowIterator::new().expect("RowIterator::new");
    let mut cells = CellIterator::new().expect("CellIterator::new");

    // Tick 1: every row should be dirty (initial state). Clear per-row.
    {
        let snap = rs.update(&terminal).expect("update 1");
        let rows_total = snap.rows().expect("rows");
        let mut row_iter = rows.update(&snap).expect("row update");
        let mut i: u16 = 0;
        while let Some(row) = row_iter.next() {
            if i >= rows_total {
                break;
            }
            assert!(
                row.dirty().expect("row.dirty 1"),
                "tick 1 row {i} should be dirty"
            );
            // Drain cells then ack.
            let mut cell_iter = cells.update(row).expect("cell update");
            while let Some(_c) = cell_iter.next() {}
            row.set_dirty(false).expect("row ack 1");
            i += 1;
        }
    }

    // Tick 2 (no writes): if per-row clear worked, every row would now
    // read `dirty=false`. We assert the BROKEN behavior so we get a loud
    // signal when upstream lands the fix.
    {
        let snap = rs.update(&terminal).expect("update 2");
        let rows_total = snap.rows().expect("rows");
        let mut row_iter = rows.update(&snap).expect("row update");
        let mut i: u16 = 0;
        let mut all_dirty = true;
        while let Some(row) = row_iter.next() {
            if i >= rows_total {
                break;
            }
            let d = row.dirty().expect("row.dirty 2 must not be poisoned");
            if !d {
                all_dirty = false;
            }
            i += 1;
        }
        assert!(
            all_dirty,
            "phux-l0t: per-row signal appears to now work (some rows reported \
             clean after per-row ack with no writes). Upstream may have fixed \
             the bug — revisit q0e.3 design to consume per-row dirty."
        );
    }

    // Tick 3 (write): at least one row reports dirty (vacuously true
    // today since all rows always report dirty, but kept as a forward-
    // compatibility check for when upstream is fixed).
    terminal.vt_write(b"hello");
    {
        let snap = rs.update(&terminal).expect("update 3");
        let rows_total = snap.rows().expect("rows");
        let mut row_iter = rows.update(&snap).expect("row update");
        let mut i: u16 = 0;
        let mut any_dirty = false;
        while let Some(row) = row_iter.next() {
            if i >= rows_total {
                break;
            }
            if row.dirty().expect("row.dirty 3") {
                any_dirty = true;
            }
            i += 1;
        }
        assert!(
            any_dirty,
            "phux-l0t: after vt_write at least one row must report dirty"
        );
    }
}
