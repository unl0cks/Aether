use std::sync::atomic::{AtomicU64, Ordering};

use crate::texture_pool_policy::PoolMaintenanceReport;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TexturePoolKind {
    General,
    Offscreen,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PoolSnapshot {
    pub requests: u64,
    pub reuses: u64,
    pub allocations: u64,
    pub allocated_bytes_estimate: u64,
    pub unknown_size_allocations: u64,
    pub resets: u64,
    pub discarded_available_entries: u64,
    pub maintenance_passes: u64,
    pub available_entries_after_maintenance: u64,
    pub available_bytes_after_maintenance: u64,
    pub age_evicted_entries: u64,
    pub age_evicted_bytes_estimate: u64,
    pub budget_evicted_entries: u64,
    pub budget_evicted_bytes_estimate: u64,
    pub unknown_size_retention_rejections: u64,
    pub globals_available_after_maintenance: u64,
    pub globals_age_evictions: u64,
    pub globals_budget_evictions: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WgpuMetricsSnapshot {
    pub general: PoolSnapshot,
    pub offscreen: PoolSnapshot,
}

struct PoolCounters {
    requests: AtomicU64,
    reuses: AtomicU64,
    allocations: AtomicU64,
    allocated_bytes_estimate: AtomicU64,
    unknown_size_allocations: AtomicU64,
    resets: AtomicU64,
    discarded_available_entries: AtomicU64,
    maintenance_passes: AtomicU64,
    available_entries_after_maintenance: AtomicU64,
    available_bytes_after_maintenance: AtomicU64,
    age_evicted_entries: AtomicU64,
    age_evicted_bytes_estimate: AtomicU64,
    budget_evicted_entries: AtomicU64,
    budget_evicted_bytes_estimate: AtomicU64,
    unknown_size_retention_rejections: AtomicU64,
    globals_available_after_maintenance: AtomicU64,
    globals_age_evictions: AtomicU64,
    globals_budget_evictions: AtomicU64,
}

impl PoolCounters {
    const fn new() -> Self {
        Self {
            requests: AtomicU64::new(0),
            reuses: AtomicU64::new(0),
            allocations: AtomicU64::new(0),
            allocated_bytes_estimate: AtomicU64::new(0),
            unknown_size_allocations: AtomicU64::new(0),
            resets: AtomicU64::new(0),
            discarded_available_entries: AtomicU64::new(0),
            maintenance_passes: AtomicU64::new(0),
            available_entries_after_maintenance: AtomicU64::new(0),
            available_bytes_after_maintenance: AtomicU64::new(0),
            age_evicted_entries: AtomicU64::new(0),
            age_evicted_bytes_estimate: AtomicU64::new(0),
            budget_evicted_entries: AtomicU64::new(0),
            budget_evicted_bytes_estimate: AtomicU64::new(0),
            unknown_size_retention_rejections: AtomicU64::new(0),
            globals_available_after_maintenance: AtomicU64::new(0),
            globals_age_evictions: AtomicU64::new(0),
            globals_budget_evictions: AtomicU64::new(0),
        }
    }

    fn take(&self) -> PoolSnapshot {
        PoolSnapshot {
            requests: self.requests.swap(0, Ordering::Relaxed),
            reuses: self.reuses.swap(0, Ordering::Relaxed),
            allocations: self.allocations.swap(0, Ordering::Relaxed),
            allocated_bytes_estimate: self.allocated_bytes_estimate.swap(0, Ordering::Relaxed),
            unknown_size_allocations: self.unknown_size_allocations.swap(0, Ordering::Relaxed),
            resets: self.resets.swap(0, Ordering::Relaxed),
            discarded_available_entries: self
                .discarded_available_entries
                .swap(0, Ordering::Relaxed),
            maintenance_passes: self.maintenance_passes.swap(0, Ordering::Relaxed),
            available_entries_after_maintenance: self
                .available_entries_after_maintenance
                .load(Ordering::Relaxed),
            available_bytes_after_maintenance: self
                .available_bytes_after_maintenance
                .load(Ordering::Relaxed),
            age_evicted_entries: self.age_evicted_entries.swap(0, Ordering::Relaxed),
            age_evicted_bytes_estimate: self.age_evicted_bytes_estimate.swap(0, Ordering::Relaxed),
            budget_evicted_entries: self.budget_evicted_entries.swap(0, Ordering::Relaxed),
            budget_evicted_bytes_estimate: self
                .budget_evicted_bytes_estimate
                .swap(0, Ordering::Relaxed),
            unknown_size_retention_rejections: self
                .unknown_size_retention_rejections
                .swap(0, Ordering::Relaxed),
            globals_available_after_maintenance: self
                .globals_available_after_maintenance
                .load(Ordering::Relaxed),
            globals_age_evictions: self.globals_age_evictions.swap(0, Ordering::Relaxed),
            globals_budget_evictions: self.globals_budget_evictions.swap(0, Ordering::Relaxed),
        }
    }
}

static GENERAL: PoolCounters = PoolCounters::new();
static OFFSCREEN: PoolCounters = PoolCounters::new();

fn counters(kind: TexturePoolKind) -> &'static PoolCounters {
    match kind {
        TexturePoolKind::General => &GENERAL,
        TexturePoolKind::Offscreen => &OFFSCREEN,
    }
}

fn saturating_atomic_add(counter: &AtomicU64, value: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(value))
    });
}

pub fn record_texture_request(kind: TexturePoolKind, reused: bool, allocated_bytes: Option<u64>) {
    let counters = counters(kind);
    saturating_atomic_add(&counters.requests, 1);
    if reused {
        saturating_atomic_add(&counters.reuses, 1);
    } else {
        saturating_atomic_add(&counters.allocations, 1);
        if let Some(bytes) = allocated_bytes {
            saturating_atomic_add(&counters.allocated_bytes_estimate, bytes);
        } else {
            saturating_atomic_add(&counters.unknown_size_allocations, 1);
        }
    }
}

pub fn record_pool_reset(kind: TexturePoolKind, discarded_available_entries: usize) {
    let counters = counters(kind);
    saturating_atomic_add(&counters.resets, 1);
    saturating_atomic_add(
        &counters.discarded_available_entries,
        u64::try_from(discarded_available_entries).unwrap_or(u64::MAX),
    );
    counters
        .available_entries_after_maintenance
        .store(0, Ordering::Relaxed);
    counters
        .available_bytes_after_maintenance
        .store(0, Ordering::Relaxed);
    counters
        .globals_available_after_maintenance
        .store(0, Ordering::Relaxed);
}

pub(crate) fn record_pool_maintenance(kind: TexturePoolKind, report: PoolMaintenanceReport) {
    let counters = counters(kind);
    saturating_atomic_add(&counters.maintenance_passes, 1);
    counters
        .available_entries_after_maintenance
        .store(report.available_entries, Ordering::Relaxed);
    counters
        .available_bytes_after_maintenance
        .store(report.available_bytes, Ordering::Relaxed);
    saturating_atomic_add(&counters.age_evicted_entries, report.age_evicted_entries);
    saturating_atomic_add(
        &counters.age_evicted_bytes_estimate,
        report.age_evicted_bytes,
    );
    saturating_atomic_add(
        &counters.budget_evicted_entries,
        report.budget_evicted_entries,
    );
    saturating_atomic_add(
        &counters.budget_evicted_bytes_estimate,
        report.budget_evicted_bytes,
    );
    saturating_atomic_add(
        &counters.unknown_size_retention_rejections,
        report.unknown_size_evicted_entries,
    );
    counters
        .globals_available_after_maintenance
        .store(report.globals_available_entries, Ordering::Relaxed);
    saturating_atomic_add(
        &counters.globals_age_evictions,
        report.globals_age_evictions,
    );
    saturating_atomic_add(
        &counters.globals_budget_evictions,
        report.globals_budget_evictions,
    );
}

pub fn take_snapshot() -> WgpuMetricsSnapshot {
    WgpuMetricsSnapshot {
        general: GENERAL.take(),
        offscreen: OFFSCREEN.take(),
    }
}

pub fn estimate_texture_bytes(
    size: wgpu::Extent3d,
    format: wgpu::TextureFormat,
    mip_level_count: u32,
    sample_count: u32,
) -> Option<u64> {
    crate::texture_pool_policy::estimate_texture_bytes(size, format, mip_level_count, sample_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture_pool_policy::PoolMaintenanceReport;
    use std::sync::Mutex;

    static METRICS_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn texture_estimator_handles_blocks_mips_layers_and_samples() {
        assert_eq!(
            estimate_texture_bytes(
                wgpu::Extent3d {
                    width: 4,
                    height: 2,
                    depth_or_array_layers: 1,
                },
                wgpu::TextureFormat::Rgba8Unorm,
                1,
                1,
            ),
            Some(32),
        );
        assert_eq!(
            estimate_texture_bytes(
                wgpu::Extent3d {
                    width: 7,
                    height: 5,
                    depth_or_array_layers: 1,
                },
                wgpu::TextureFormat::Bc1RgbaUnorm,
                1,
                1,
            ),
            Some(32),
        );
        assert_eq!(
            estimate_texture_bytes(
                wgpu::Extent3d {
                    width: 8,
                    height: 8,
                    depth_or_array_layers: 1,
                },
                wgpu::TextureFormat::Rgba8Unorm,
                4,
                1,
            ),
            Some(340),
        );
        assert_eq!(
            estimate_texture_bytes(
                wgpu::Extent3d {
                    width: 4,
                    height: 4,
                    depth_or_array_layers: 3,
                },
                wgpu::TextureFormat::Rgba8Unorm,
                1,
                4,
            ),
            Some(768),
        );
        assert_eq!(
            estimate_texture_bytes(
                wgpu::Extent3d {
                    width: 4,
                    height: 4,
                    depth_or_array_layers: 1,
                },
                wgpu::TextureFormat::Depth24Plus,
                1,
                1,
            ),
            None,
        );
    }

    #[test]
    fn pool_snapshot_separates_general_and_offscreen_and_resets() {
        let _guard = METRICS_TEST_LOCK.lock().unwrap();
        let _ = take_snapshot();
        record_texture_request(TexturePoolKind::General, true, Some(64));
        record_texture_request(TexturePoolKind::General, false, Some(128));
        record_texture_request(TexturePoolKind::Offscreen, false, None);
        record_pool_reset(TexturePoolKind::Offscreen, 3);

        let snapshot = take_snapshot();
        assert_eq!(snapshot.general.requests, 2);
        assert_eq!(snapshot.general.reuses, 1);
        assert_eq!(snapshot.general.allocations, 1);
        assert_eq!(snapshot.general.allocated_bytes_estimate, 128);
        assert_eq!(snapshot.offscreen.unknown_size_allocations, 1);
        assert_eq!(snapshot.offscreen.resets, 1);
        assert_eq!(snapshot.offscreen.discarded_available_entries, 3);
        assert_eq!(take_snapshot(), WgpuMetricsSnapshot::default());
    }

    #[test]
    fn pool_maintenance_keeps_gauges_and_resets_counters() {
        let _guard = METRICS_TEST_LOCK.lock().unwrap();
        record_pool_reset(TexturePoolKind::Offscreen, 0);
        let _ = take_snapshot();

        record_pool_maintenance(
            TexturePoolKind::Offscreen,
            PoolMaintenanceReport {
                available_entries: 7,
                available_bytes: 11_000,
                age_evicted_entries: 2,
                age_evicted_bytes: 3_000,
                budget_evicted_entries: 4,
                budget_evicted_bytes: 5_000,
                unknown_size_evicted_entries: 6,
                globals_available_entries: 8,
                globals_age_evictions: 9,
                globals_budget_evictions: 10,
            },
        );

        let first = take_snapshot().offscreen;
        assert_eq!(first.maintenance_passes, 1);
        assert_eq!(first.available_entries_after_maintenance, 7);
        assert_eq!(first.available_bytes_after_maintenance, 11_000);
        assert_eq!(first.age_evicted_entries, 2);
        assert_eq!(first.age_evicted_bytes_estimate, 3_000);
        assert_eq!(first.budget_evicted_entries, 4);
        assert_eq!(first.budget_evicted_bytes_estimate, 5_000);
        assert_eq!(first.unknown_size_retention_rejections, 6);
        assert_eq!(first.globals_available_after_maintenance, 8);
        assert_eq!(first.globals_age_evictions, 9);
        assert_eq!(first.globals_budget_evictions, 10);

        let second = take_snapshot().offscreen;
        assert_eq!(second.maintenance_passes, 0);
        assert_eq!(second.age_evicted_entries, 0);
        assert_eq!(second.available_entries_after_maintenance, 7);
        assert_eq!(second.available_bytes_after_maintenance, 11_000);
        assert_eq!(second.globals_available_after_maintenance, 8);

        record_pool_reset(TexturePoolKind::Offscreen, 0);
        let reset = take_snapshot().offscreen;
        assert_eq!(reset.available_entries_after_maintenance, 0);
        assert_eq!(reset.available_bytes_after_maintenance, 0);
        assert_eq!(reset.globals_available_after_maintenance, 0);
    }
}
