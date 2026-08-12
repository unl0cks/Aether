//! Why an allocation failed on a card with plenty of memory left.
//!
//! The 0.5.2 texture-grid change cut the bytes created across a session from 7.3 GB to 2.9 GB, and
//! the device was still lost to "Out of memory" holding 0.51 GB across 46 device allocations on a
//! 16 GB card. Totals cannot explain that, so they are the wrong thing to be reading.
//!
//! What can explain it is shape. wgpu suballocates textures out of a handful of large blocks, and a
//! block with 40 MB free spread across a thousand small gaps cannot satisfy a request for one
//! contiguous megabyte. The allocator then asks the driver for a fresh block, and it is that request
//! which fails. Summing free bytes hides this exactly; the number that matters is the largest
//! contiguous run, which this reports alongside the totals so the two can be compared.

use wgpu::AllocatorReport;

/// The shape of the allocator's memory at one moment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AllocatorFragmentation {
    pub blocks: usize,
    pub live_allocations: usize,
    /// Total the allocator has reserved from the driver.
    pub reserved_bytes: u64,
    /// The part of it actually holding resources.
    pub allocated_bytes: u64,
    /// Reserved but unused. Large here and small in `largest_free_run_bytes` means fragmentation.
    pub free_bytes: u64,
    /// The biggest single contiguous free run in any block.
    ///
    /// This is the real ceiling on the next allocation that can be served without asking the driver
    /// for more, which makes it the number to watch when allocations fail with memory to spare.
    pub largest_free_run_bytes: u64,
}

impl AllocatorFragmentation {
    /// How much of the free space is unusable for an allocation the size of the largest free run.
    ///
    /// Reported as a percentage so it can be compared between sessions and cards. A session that
    /// ends at 95% is one where the allocator was full of holes it could not use.
    pub fn wasted_free_percent(&self) -> f64 {
        if self.free_bytes == 0 {
            return 0.0;
        }
        let usable = self.largest_free_run_bytes as f64;
        (1.0 - usable / self.free_bytes as f64) * 100.0
    }
}

/// Reduce a wgpu allocator report to the shape of its free space.
pub fn summarise(report: &AllocatorReport) -> AllocatorFragmentation {
    let mut summary = AllocatorFragmentation {
        blocks: report.blocks.len(),
        live_allocations: report.allocations.len(),
        reserved_bytes: report.total_reserved_bytes,
        allocated_bytes: report.total_allocated_bytes,
        ..Default::default()
    };

    for block in &report.blocks {
        // gpu-allocator hands back the allocations for a block as a range into one shared list.
        // A range that does not address this report is skipped rather than trusted.
        let Some(allocations) = report.allocations.get(block.allocations.clone()) else {
            continue;
        };

        let mut offsets: Vec<_> = allocations.iter().collect();
        // Nothing documents these as ordered, and walking them unsorted would invent overlaps and
        // report a fragmented block as a healthy one.
        offsets.sort_unstable_by_key(|allocation| allocation.offset);

        let mut cursor = 0_u64;
        for allocation in offsets {
            summary.largest_free_run_bytes = summary
                .largest_free_run_bytes
                .max(allocation.offset.saturating_sub(cursor));
            cursor = cursor.max(allocation.offset.saturating_add(allocation.size));
        }
        summary.largest_free_run_bytes = summary
            .largest_free_run_bytes
            .max(block.size.saturating_sub(cursor));
    }

    summary.free_bytes = summary
        .reserved_bytes
        .saturating_sub(summary.allocated_bytes);
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;
    use wgpu_types::{AllocationReport, MemoryBlockReport};

    fn allocation(offset: u64, size: u64) -> AllocationReport {
        AllocationReport {
            name: String::new(),
            offset,
            size,
        }
    }

    fn report(
        blocks: Vec<(u64, Range<usize>)>,
        allocations: Vec<AllocationReport>,
    ) -> AllocatorReport {
        let allocated_bytes = allocations.iter().map(|entry| entry.size).sum();
        AllocatorReport {
            total_reserved_bytes: blocks.iter().map(|(size, _)| *size).sum(),
            total_allocated_bytes: allocated_bytes,
            blocks: blocks
                .into_iter()
                .map(|(size, allocations)| MemoryBlockReport { size, allocations })
                .collect(),
            allocations,
        }
    }

    #[test]
    fn a_gap_between_two_allocations_is_measured_as_free_space() {
        let summary = summarise(&report(
            vec![(4_000, 0..2)],
            vec![allocation(0, 1_000), allocation(3_000, 1_000)],
        ));

        assert_eq!(summary.blocks, 1);
        assert_eq!(summary.live_allocations, 2);
        assert_eq!(summary.allocated_bytes, 2_000);
        assert_eq!(summary.free_bytes, 2_000);
        assert_eq!(summary.largest_free_run_bytes, 2_000);
    }

    #[test]
    fn the_tail_of_a_block_past_the_last_allocation_counts_as_free() {
        let summary = summarise(&report(vec![(4_000, 0..1)], vec![allocation(0, 1_000)]));

        assert_eq!(summary.largest_free_run_bytes, 3_000);
    }

    /// The case this module exists for: plenty free, none of it usable.
    ///
    /// A card reporting hundreds of megabytes free while a one-megabyte texture fails to allocate is
    /// not out of memory, it is out of contiguous memory, and only the free run says so.
    #[test]
    fn free_space_shattered_into_small_gaps_reports_a_small_largest_run() {
        // 1,000 allocations of 3 KB each, spaced 4 KB apart: 1 KB of free space between every pair.
        let allocations: Vec<AllocationReport> = (0..1_000)
            .map(|index| allocation(index * 4_000, 3_000))
            .collect();
        let summary = summarise(&report(vec![(4_000_000, 0..1_000)], allocations));

        assert_eq!(summary.free_bytes, 1_000_000);
        assert_eq!(summary.largest_free_run_bytes, 1_000);
        assert!(summary.wasted_free_percent() > 99.0);
    }

    /// Nothing promises the allocations arrive in offset order, and walking them unsorted would
    /// invent overlaps and report a fragmented block as a healthy one.
    #[test]
    fn allocations_out_of_offset_order_still_measure_the_same_gaps() {
        let ordered = summarise(&report(
            vec![(9_000, 0..3)],
            vec![
                allocation(0, 1_000),
                allocation(2_000, 1_000),
                allocation(8_000, 1_000),
            ],
        ));
        let shuffled = summarise(&report(
            vec![(9_000, 0..3)],
            vec![
                allocation(8_000, 1_000),
                allocation(0, 1_000),
                allocation(2_000, 1_000),
            ],
        ));

        assert_eq!(ordered, shuffled);
        assert_eq!(ordered.largest_free_run_bytes, 5_000);
    }

    #[test]
    fn an_empty_report_summarises_to_zeroes_rather_than_dividing_by_zero() {
        let summary = summarise(&report(vec![], vec![]));

        assert_eq!(summary, AllocatorFragmentation::default());
        assert_eq!(summary.wasted_free_percent(), 0.0);
    }

    /// A block whose range does not address the report must not panic the crash handler. This runs
    /// while the device is already being lost, which is the worst possible moment to slice a vector
    /// out of bounds.
    #[test]
    fn a_block_range_past_the_allocation_list_is_skipped_rather_than_panicking() {
        let summary = summarise(&report(vec![(4_000, 0..99)], vec![allocation(0, 1_000)]));

        assert_eq!(summary.blocks, 1);
        assert_eq!(summary.largest_free_run_bytes, 0);
    }
}
