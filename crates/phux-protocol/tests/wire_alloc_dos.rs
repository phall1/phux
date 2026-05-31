//! Repro + regression: attacker-controlled count prefixes must not drive
//! pre-allocation disproportionate to the bytes actually present in the frame.
//!
//! A tiny frame (single-digit bytes) that declares a 4-billion-element list
//! pre-`fix` calls `Vec::with_capacity(4e9)`. Depending on the platform that
//! either aborts the process (allocator returns null -> `handle_alloc_error`)
//! or silently reserves tens of GiB of address space. Either way it is a
//! decode-path denial of service: the decoder must reject the frame having
//! reserved no more
//! than the remaining input could justify.
//!
//! The test installs a recording global allocator that captures the single
//! largest allocation request made on this thread while decoding, so the
//! assertion is deterministic across platforms (it does not depend on whether
//! the OS overcommits).

#![allow(clippy::cast_possible_truncation, clippy::unwrap_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use phux_protocol::wire::frame::FrameKind;

static MAX_ALLOC: AtomicUsize = AtomicUsize::new(0);
static RECORDING: AtomicUsize = AtomicUsize::new(0);

struct RecordingAlloc;

// SAFETY: forwards every call straight to `System`; the only added behaviour is
// reading `layout.size()` and updating two atomics, neither of which touches
// the returned pointer or violates the `GlobalAlloc` contract.
unsafe impl GlobalAlloc for RecordingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if RECORDING.load(Ordering::Relaxed) == 1 {
            MAX_ALLOC.fetch_max(layout.size(), Ordering::Relaxed);
        }
        // SAFETY: same layout precondition the caller already upholds.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr`/`layout` pairing is the caller's responsibility.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static A: RecordingAlloc = RecordingAlloc;

fn largest_alloc_during(decode_input: &[u8]) -> usize {
    MAX_ALLOC.store(0, Ordering::Relaxed);
    RECORDING.store(1, Ordering::Relaxed);
    let _ = FrameKind::decode(decode_input);
    RECORDING.store(0, Ordering::Relaxed);
    MAX_ALLOC.load(Ordering::Relaxed)
}

/// Build a frame: 4-byte length header, then body bytes.
fn framed(body: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
    buf.extend_from_slice(body);
    buf
}

#[test]
fn metadata_keys_huge_count_does_not_over_reserve() {
    // type 0xD2 + request_id(4) + count(4) = u32::MAX.
    let mut body = vec![0xD2];
    body.extend_from_slice(&0u32.to_be_bytes());
    body.extend_from_slice(&u32::MAX.to_be_bytes());
    let frame = framed(&body);
    let max = largest_alloc_during(&frame);
    assert!(FrameKind::decode(&frame).is_err());
    // Body is 9 bytes; a sane decoder reserves on the order of the input,
    // never gigabytes. 1 MiB is a generous ceiling.
    assert!(
        max < 1 << 20,
        "decoder reserved {max} bytes for a {}-byte frame",
        frame.len()
    );
}

#[test]
fn spawn_terminal_huge_command_list_does_not_over_reserve() {
    // SPAWN_TERMINAL (0x22): request_id(4), collection(4), command Option list.
    // tag=1 (Some) then count u32::MAX.
    let mut body = vec![0x22];
    body.extend_from_slice(&0u32.to_be_bytes()); // request_id
    body.extend_from_slice(&1u32.to_be_bytes()); // collection
    body.push(1); // command = Some
    body.extend_from_slice(&u32::MAX.to_be_bytes()); // list count
    let frame = framed(&body);
    let max = largest_alloc_during(&frame);
    assert!(FrameKind::decode(&frame).is_err());
    assert!(max < 1 << 20, "command-list reserved {max} bytes");
}

#[test]
fn spawn_terminal_huge_env_list_does_not_over_reserve() {
    // SPAWN_TERMINAL env: command None(0), cwd None(0), env Some(1) + count.
    let mut body = vec![0x22];
    body.extend_from_slice(&0u32.to_be_bytes());
    body.extend_from_slice(&1u32.to_be_bytes());
    body.push(0); // command None
    body.push(0); // cwd None
    body.push(1); // env Some
    body.extend_from_slice(&u32::MAX.to_be_bytes());
    let frame = framed(&body);
    let max = largest_alloc_during(&frame);
    assert!(FrameKind::decode(&frame).is_err());
    assert!(max < 1 << 20, "env-list reserved {max} bytes");
}

#[test]
fn attached_snapshot_huge_sessions_list_does_not_over_reserve() {
    // ATTACHED (0x81): SessionSnapshot starts with a u32 sessions count.
    let mut body = vec![0x81];
    body.extend_from_slice(&u32::MAX.to_be_bytes()); // sessions count
    let frame = framed(&body);
    let max = largest_alloc_during(&frame);
    assert!(FrameKind::decode(&frame).is_err());
    assert!(max < 1 << 20, "snapshot sessions reserved {max} bytes");
}
