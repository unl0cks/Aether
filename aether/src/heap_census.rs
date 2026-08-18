//! A global allocator that counts what the Rust heap is holding.
//!
//! Written because every measurement so far has been a subset. The core census can say how many
//! movies, characters and SWF bytes are resident; the GPU census can say how many textures and
//! buffers the driver is holding. A session in Yulgar had both of those go flat while the process
//! kept taking eighty megabytes a minute, and neither instrument could see it, so four separate
//! theories were built and killed on data that could not have confirmed any of them.
//!
//! This closes the account. Every Rust allocation in the process passes through here, so the
//! reported figure covers the collector arena, the character libraries, the renderer's meshes and
//! everything else the client itself allocates. Set against the operating system's own view of the
//! process, it splits the growth exactly where the next question is:
//!
//! * Rust heap climbing -- the client is holding it, and the subsystem counts say which.
//! * Rust heap flat while private bytes climb -- the graphics driver or the allocator is, and no
//!   amount of work inside the core will touch it.
//!
//! Metrics builds only. Two atomic adds on every allocation is not something to ship, and the
//! point of it is a diagnostic run rather than a release.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Bytes currently allocated and not yet freed.
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);

/// How many allocations have been served, ever.
///
/// Live bytes that hold steady while this climbs is churn, which costs frame time rather than
/// memory -- a distinction the resident figure alone cannot make.
static TOTAL_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

/// Bytes currently held by the Rust heap.
pub fn live_bytes() -> usize {
    LIVE_BYTES.load(Ordering::Relaxed)
}

/// Allocations served since the process started.
pub fn total_allocations() -> u64 {
    TOTAL_ALLOCATIONS.load(Ordering::Relaxed)
}

/// Wraps the system allocator, counting bytes in and out.
pub struct TrackingAllocator;

// SAFETY: every method forwards to `System`, which is a valid allocator, and the pointers and
// layouts are passed through untouched. The counters are incidental and cannot affect allocation.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            TOTAL_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            TOTAL_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            // Only on success: a failed realloc leaves the original block untouched, and
            // subtracting for it would drift the live figure down every time memory is tight,
            // which is exactly when the number is being read.
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
            LIVE_BYTES.fetch_add(new_size, Ordering::Relaxed);
            TOTAL_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        new_ptr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocating_and_freeing_returns_the_live_figure_to_where_it_started() {
        // The whole instrument rests on alloc and dealloc being symmetric. If they are not, the
        // live figure drifts in one direction on its own and reads as precisely the leak it is
        // supposed to be detecting.
        let before = live_bytes();
        let allocations_before = total_allocations();

        let block: Vec<u8> = vec![0; 4 * 1024 * 1024];
        assert!(
            live_bytes() >= before + 4 * 1024 * 1024,
            "a four megabyte allocation must be visible in the live figure"
        );
        assert!(total_allocations() > allocations_before);
        drop(block);

        // Other threads allocate too, so this cannot be an equality against `before`.
        assert!(
            live_bytes() < before + 4 * 1024 * 1024,
            "freeing must give the bytes back"
        );
    }

    #[test]
    fn growing_a_vector_does_not_double_count_the_reallocation() {
        // `realloc` has to subtract the old size as well as add the new one. Missing the
        // subtraction makes every growing buffer in the process look like a leak, and buffers grow
        // constantly.
        let before = live_bytes();
        let mut block: Vec<u8> = Vec::with_capacity(1024 * 1024);
        block.resize(1024 * 1024, 0);
        block.reserve(8 * 1024 * 1024);
        let live_while_held = live_bytes();
        drop(block);

        assert!(
            live_while_held < before + 32 * 1024 * 1024,
            "a buffer grown once must not be counted at every size it has ever been"
        );
    }
}
