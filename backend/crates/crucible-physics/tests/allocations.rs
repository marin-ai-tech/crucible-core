//! The physics step makes no allocations after warm-up.
//!
//! This test binary uses a counting global allocator. The `GlobalAlloc` trait and calls to
//! the system allocator are unsafe, so this file opts into `unsafe` for the test.
//! The allocator forwards each call to `System` and counts calls on the current thread
//! while counting is enabled. It does not change the library's allocator.

#![allow(unsafe_code)]

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use common::{SCENARIOS, Trajectory};
use crucible_physics::PhysicsSnapshot;

struct CountingAllocator;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

fn note_allocation() {
    // `try_with` skips counting while thread-local storage is being destroyed.
    let _ = COUNTING.try_with(|on| {
        if on.get() {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        }
    });
}

// SAFETY: Calls are forwarded unchanged to the system allocator. Counting allocates no
// memory because the thread-local counter is initialized with a constant.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        // SAFETY: The caller upholds the allocator contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        // SAFETY: The caller upholds the allocator contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation();
        // SAFETY: The caller upholds the allocator contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: The caller upholds the allocator contract.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Counts allocations on the current thread while `f` runs.
fn count_allocations(f: impl FnOnce()) -> u64 {
    COUNT.with(|c| c.set(0));
    COUNTING.with(|on| on.set(true));
    f();
    COUNTING.with(|on| on.set(false));
    COUNT.with(|c| c.get())
}

/// Every recorded fixture must allocate zero times per tick after warm-up. Run each scenario
/// once to grow the buffers, restore its initial snapshot, and count allocations on replay.
/// Each tick includes input, the physics step, and snapshot capture as in a prediction loop.
#[test]
fn zero_allocations_per_tick_after_warmup() {
    for name in SCENARIOS {
        let traj = Trajectory::load(name);
        let mut scene = traj.build();
        let mut initial = PhysicsSnapshot::default();
        let mut per_tick = PhysicsSnapshot::default();
        scene.world.save_state(&mut initial);

        // The first run grows the buffers to the required size.
        let mut first_run = 0u64;
        for k in 0..traj.tick_count() {
            let n = count_allocations(|| {
                scene.step_reference_tick(&traj, k);
                scene.world.save_state(&mut per_tick);
            });
            if k > 0 {
                first_run += n;
            }
        }

        scene.restore(&initial, crucible_physics::MoveButtons::NONE);
        let mut total = 0u64;
        for k in 0..traj.tick_count() {
            let n = count_allocations(|| {
                scene.step_reference_tick(&traj, k);
                scene.world.save_state(&mut per_tick);
            });
            assert_eq!(n, 0, "{name}: {n} allocations on tick {k} after warm-up");
            total += n;
        }
        println!(
            "ALLOCATIONS {name}: first run after tick 1: {first_run}; after warm-up: {total} in {} ticks",
            traj.tick_count()
        );
    }
}
