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
    /// CPU time spent inside `Device::create_texture` since the last snapshot.
    ///
    /// Frame time is reported as the wall time of the render call, which is CPU work: building
    /// command buffers and creating the GPU objects they reference. A texture creation is a real
    /// driver allocation on that thread, so if the pools are missing often enough this shows up
    /// directly in the frame budget rather than on the GPU.
    pub texture_create_nanos: u64,
    pub texture_creations: u64,
    /// CPU time inside `submit_frame`, and the portion of it spent rebuilding `cacheAsBitmap`
    /// surfaces. Frame time minus this is the core's own display-list traversal and command
    /// building, which is otherwise invisible.
    pub submit_nanos: u64,
    pub cache_entry_nanos: u64,
    pub cache_entries: u64,
    /// Time handing the finished frame to the driver. Encoding is CPU work, but a queue submit
    /// blocks once the GPU falls behind, so this separates "we are slow to describe the frame"
    /// from "the GPU cannot keep up with the frame we described".
    pub queue_nanos: u64,
    /// What one frame's encoding actually consists of. Nearly all frame time is the stage draw,
    /// and these say whether that is the number of things drawn or the number of times the
    /// renderer has to stop and set up a new target to draw them into.
    pub render_passes: u64,
    /// Complex blends this interval, per mode, indexed by [`crate::blend::ComplexBlend`]. Blends
    /// are what create render passes, and frame time tracks passes, so which modes AQW actually
    /// uses decides what can be done about them.
    pub complex_blends: [u64; COMPLEX_BLEND_MODES],
    pub draw_commands: u64,
    pub blend_chunks: u64,
    pub bind_groups: u64,
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
    let census = pool_census(kind);
    census.requests.fetch_add(1, Ordering::Relaxed);
    if reused {
        census.reuses.fetch_add(1, Ordering::Relaxed);
    }

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
    let census = pool_census(kind);
    census.buckets.store(report.bucket_count, Ordering::Relaxed);
    census
        .available_entries
        .store(report.available_entries, Ordering::Relaxed);
    census
        .available_bytes
        .store(report.available_bytes, Ordering::Relaxed);
    census
        .budget_evicted_entries
        .fetch_add(report.budget_evicted_entries, Ordering::Relaxed);
    census
        .age_evicted_entries
        .fetch_add(report.age_evicted_entries, Ordering::Relaxed);

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

static TEXTURE_CREATE_NANOS: AtomicU64 = AtomicU64::new(0);
static TEXTURE_CREATIONS: AtomicU64 = AtomicU64::new(0);

/// Record one `Device::create_texture` call and what it cost on the calling thread.
pub fn record_texture_creation(elapsed: std::time::Duration) {
    saturating_atomic_add(
        &TEXTURE_CREATE_NANOS,
        elapsed.as_nanos().min(u64::MAX as u128) as u64,
    );
    saturating_atomic_add(&TEXTURE_CREATIONS, 1);
}

static SUBMIT_NANOS: AtomicU64 = AtomicU64::new(0);
static CACHE_ENTRY_NANOS: AtomicU64 = AtomicU64::new(0);
static CACHE_ENTRIES: AtomicU64 = AtomicU64::new(0);

/// Record one `submit_frame`: its total cost, and the cost and count of the cache surfaces it
/// rebuilt before drawing the stage.
pub fn record_submit_frame(
    total: std::time::Duration,
    cache: std::time::Duration,
    entries: u64,
    queue: std::time::Duration,
) {
    saturating_atomic_add(&SUBMIT_NANOS, nanos(total));
    saturating_atomic_add(&CACHE_ENTRY_NANOS, nanos(cache));
    saturating_atomic_add(&CACHE_ENTRIES, entries);
    saturating_atomic_add(&QUEUE_NANOS, nanos(queue));
}

static QUEUE_NANOS: AtomicU64 = AtomicU64::new(0);

fn nanos(duration: std::time::Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

/// Number of variants in [`crate::blend::ComplexBlend`].
pub const COMPLEX_BLEND_MODES: usize = 9;

pub const COMPLEX_BLEND_NAMES: [&str; COMPLEX_BLEND_MODES] = [
    "multiply",
    "lighten",
    "darken",
    "difference",
    "invert",
    "alpha",
    "erase",
    "overlay",
    "hardlight",
];

static COMPLEX_BLENDS: [AtomicU64; COMPLEX_BLEND_MODES] =
    [const { AtomicU64::new(0) }; COMPLEX_BLEND_MODES];

/// Record one complex blend by its mode index.
pub fn record_complex_blend(mode: usize) {
    if let Some(counter) = COMPLEX_BLENDS.get(mode) {
        saturating_atomic_add(counter, 1);
    }
}

static RENDER_PASSES: AtomicU64 = AtomicU64::new(0);
static DRAW_COMMANDS: AtomicU64 = AtomicU64::new(0);
static BLEND_CHUNKS: AtomicU64 = AtomicU64::new(0);
static BIND_GROUPS: AtomicU64 = AtomicU64::new(0);

/// Record one chunk of encoded work: a render pass, and either the draws it carries or the fact
/// that it is a blend needing its own target.
pub fn record_encoded_chunk(draw_commands: u64, is_blend: bool) {
    saturating_atomic_add(&RENDER_PASSES, 1);
    saturating_atomic_add(&DRAW_COMMANDS, draw_commands);
    if is_blend {
        saturating_atomic_add(&BLEND_CHUNKS, 1);
    }
}

/// Record a bind group built during encoding rather than served from a cache.
pub fn record_bind_group_created() {
    saturating_atomic_add(&BIND_GROUPS, 1);
}

pub fn take_snapshot() -> WgpuMetricsSnapshot {
    WgpuMetricsSnapshot {
        general: GENERAL.take(),
        offscreen: OFFSCREEN.take(),
        texture_create_nanos: TEXTURE_CREATE_NANOS.swap(0, Ordering::Relaxed),
        texture_creations: TEXTURE_CREATIONS.swap(0, Ordering::Relaxed),
        submit_nanos: SUBMIT_NANOS.swap(0, Ordering::Relaxed),
        cache_entry_nanos: CACHE_ENTRY_NANOS.swap(0, Ordering::Relaxed),
        cache_entries: CACHE_ENTRIES.swap(0, Ordering::Relaxed),
        queue_nanos: QUEUE_NANOS.swap(0, Ordering::Relaxed),
        render_passes: RENDER_PASSES.swap(0, Ordering::Relaxed),
        complex_blends: std::array::from_fn(|i| COMPLEX_BLENDS[i].swap(0, Ordering::Relaxed)),
        draw_commands: DRAW_COMMANDS.swap(0, Ordering::Relaxed),
        blend_chunks: BLEND_CHUNKS.swap(0, Ordering::Relaxed),
        bind_groups: BIND_GROUPS.swap(0, Ordering::Relaxed),
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
                bucket_count: 3,
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

    #[test]
    fn saturated_census_still_attributes_the_overflow_and_counts_its_bytes() {
        // The table fills within a minute of real play, and once it does every later allocation
        // lands in the overflow. A session reported 83,252 of 140,258 allocations there, excluded
        // from the totals and unattributed -- so the visible rows looked like the whole story.
        let _guard = METRICS_TEST_LOCK.lock().unwrap();
        reset_texture_census();
        for height in 0..TEXTURE_BUCKETS as u32 {
            record_texture_created(TextureOrigin::Pool, 1, height + 1, 1, 10);
        }

        record_texture_created(TextureOrigin::Bitmap, 4096, 4096, 1, 67_108_864);

        let report = texture_census_report(4).join("\n");
        assert!(
            report.contains("(table full)"),
            "a saturated table must say so rather than report 512 as a finding: {report}"
        );
        assert!(
            report.contains("bitmap (sizes past the table)"),
            "overflow must still name which origin allocated it: {report}"
        );
        assert!(
            report.contains("67.1 MB"),
            "overflow must carry its bytes: {report}"
        );
        assert!(
            report.contains("513 allocations"),
            "the header total must include the overflow: {report}"
        );
    }

    #[test]
    fn texture_census_names_gradient_atlas_allocations() {
        let _guard = METRICS_TEST_LOCK.lock().unwrap();
        reset_texture_census();
        record_texture_created(TextureOrigin::GradientAtlas, 256, 4_096, 1, 4_194_304);

        let report = texture_census_report(4).join("\n");
        assert!(
            report.contains("gradient-atlas"),
            "the atlas must be distinguishable from decoded bitmaps: {report}"
        );
        assert!(
            report.contains("256x4096"),
            "atlas dimensions missing: {report}"
        );
    }

    #[test]
    fn census_reports_pool_reuse_and_how_thinly_the_entry_cap_is_spread() {
        // Whether the pool reuses anything is the question the entry cap was raised to answer, and
        // it cannot be read off allocation counts alone -- a request that reuses and a request that
        // allocates look identical in the size table.
        let _guard = METRICS_TEST_LOCK.lock().unwrap();
        reset_texture_census();
        record_texture_request(TexturePoolKind::General, true, Some(64));
        record_texture_request(TexturePoolKind::General, false, Some(64));
        record_texture_request(TexturePoolKind::General, false, Some(64));
        record_texture_request(TexturePoolKind::General, false, Some(64));
        record_pool_maintenance(
            TexturePoolKind::General,
            PoolMaintenanceReport {
                bucket_count: 412,
                available_entries: 96,
                available_bytes: 310_000_000,
                budget_evicted_entries: 7,
                age_evicted_entries: 3,
                ..Default::default()
            },
        );

        let report = texture_census_report(1).join("\n");
        assert!(
            report.contains("general pool: 1/4 reused (25.0%)"),
            "reuse ratio must be reported: {report}"
        );
        assert!(
            report.contains("412 buckets"),
            "bucket count says how thinly the entry cap is divided: {report}"
        );
        assert!(
            report.contains("evicted 7 for budget / 3 for age"),
            "eviction cause separates a byte limit from an idle limit: {report}"
        );
        assert!(
            !report.contains("offscreen pool:"),
            "a pool with no requests must not print a line: {report}"
        );

        // Put the per-interval counters back as they were found. Their post-maintenance figures are
        // gauges rather than counters, so `take_snapshot` reads without clearing them and the state
        // this test just wrote would otherwise surface in whichever test runs next.
        reset_texture_census();
        record_pool_reset(TexturePoolKind::General, 0);
        let _ = take_snapshot();
    }

    #[test]
    fn census_reports_live_residency_so_churn_is_distinguishable_from_retention() {
        let _guard = METRICS_TEST_LOCK.lock().unwrap();
        reset_texture_census();
        record_gpu_residency(39_600_000_000, 1185, 812);

        let report = texture_census_report(1).join("\n");
        assert!(
            report.contains("1185 textures"),
            "live texture count must appear: {report}"
        );
        assert!(
            report.contains("39.60 GB resident"),
            "resident bytes are the figure that explains a device loss: {report}"
        );
    }
}

// ---- texture allocation census -------------------------------------------------------------
//
// Everything above counts pool activity, which turned out to describe only a small share of what
// the GPU actually holds: a 198-second session reported 39.6 GB resident across 1,185 live
// textures — an average of 33 MB each — while the general pool accounted for 854 stage-sized
// allocations, about one per frame. The bytes are being created somewhere these counters never
// looked, and every attempt to reason about WHERE from aggregate totals has been wrong.
//
// So: record every texture as it is created, bucketed by size and by the call site that asked for
// it, and print the biggest buckets when the device dies. Not a sampling profile and not another
// metrics session — a dozen lines that name the allocator.

/// Which part of the renderer asked for a texture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureOrigin {
    /// Render targets and filter intermediates, via the texture pools.
    Pool,
    /// Decoded SWF images and BitmapData surfaces, via `register_bitmap`.
    Bitmap,
    /// Packed vector-gradient color ramps shared by every registered shape.
    GradientAtlas,
}

impl TextureOrigin {
    fn name(self) -> &'static str {
        match self {
            TextureOrigin::Pool => "pool",
            TextureOrigin::Bitmap => "bitmap",
            TextureOrigin::GradientAtlas => "gradient-atlas",
        }
    }
}

struct TextureBucketCounters {
    /// Packed key: origin << 62 | samples << 58 | width << 29 | height.
    key: AtomicU64,
    count: AtomicU64,
    bytes: AtomicU64,
}

impl TextureBucketCounters {
    const fn new() -> Self {
        Self {
            key: AtomicU64::new(0),
            count: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        }
    }
}

/// Fixed table, probed linearly. A texture allocation is already an expensive operation, so the
/// cost of a short scan here is irrelevant, and a fixed table means no allocation on this path and
/// no lock to contend.
const TEXTURE_BUCKETS: usize = 512;
static TEXTURE_CENSUS: [TextureBucketCounters; TEXTURE_BUCKETS] =
    [const { TextureBucketCounters::new() }; TEXTURE_BUCKETS];

/// Allocations that arrived after the table filled, split by origin and carrying their bytes.
///
/// A single untyped counter was not enough. AQW loads hundreds of distinctly-sized avatar parts in
/// the first minute, so all 512 slots fill early and everything afterwards lands here: a real
/// session reported 83,252 of its 140,258 allocations as overflow. Reporting 59% of the census as
/// one anonymous number, and leaving those bytes out of the totals, made the visible rows look far
/// more conclusive than they were.
struct OverflowCounters {
    count: AtomicU64,
    bytes: AtomicU64,
}

impl OverflowCounters {
    const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        }
    }
}

static TEXTURE_CENSUS_OVERFLOW: [OverflowCounters; 3] = [const { OverflowCounters::new() }; 3];

/// Last sampled live-resource figures from wgpu-hal. The census above counts textures as they are
/// *created*, which cannot distinguish per-frame churn that is freed again from memory that is
/// still held -- and only the second kind loses the device. Recorded from the render loop rather
/// than read in the device-lost callback, which must not reach back into the dying device.
static LIVE_TEXTURE_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_TEXTURE_COUNT: AtomicU64 = AtomicU64::new(0);
static LIVE_MEMORY_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static LIVE_SAMPLES: AtomicU64 = AtomicU64::new(0);
/// High-water marks. The per-frame sample alone only says what was resident on the last frame
/// before the fault, which cannot distinguish "never grew" from "grew and was freed again".
static PEAK_TEXTURE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_TEXTURE_COUNT: AtomicU64 = AtomicU64::new(0);

/// Sample of live GPU resources, taken once per frame from `Device::get_internal_counters`.
pub fn record_gpu_residency(texture_bytes: u64, textures: u64, memory_allocations: u64) {
    LIVE_TEXTURE_BYTES.store(texture_bytes, Ordering::Relaxed);
    LIVE_TEXTURE_COUNT.store(textures, Ordering::Relaxed);
    LIVE_MEMORY_ALLOCATIONS.store(memory_allocations, Ordering::Relaxed);
    LIVE_SAMPLES.fetch_add(1, Ordering::Relaxed);
    PEAK_TEXTURE_BYTES.fetch_max(texture_bytes, Ordering::Relaxed);
    PEAK_TEXTURE_COUNT.fetch_max(textures, Ordering::Relaxed);
}

fn overflow_index(origin: TextureOrigin) -> usize {
    match origin {
        TextureOrigin::Pool => 0,
        TextureOrigin::Bitmap => 1,
        TextureOrigin::GradientAtlas => 2,
    }
}

fn overflow_origin(index: usize) -> TextureOrigin {
    match index {
        0 => TextureOrigin::Pool,
        1 => TextureOrigin::Bitmap,
        _ => TextureOrigin::GradientAtlas,
    }
}

/// Cumulative pool activity for the crash census.
///
/// Deliberately separate from `PoolCounters` above, which the periodic metrics consumer *drains*
/// every second: a census printed after a fault needs the whole session, not whatever happened to
/// accrue since the last drain. The reuse ratio is the number that says whether the pool is doing
/// its job at all, and the bucket count is what says whether the entry cap can hold anything
/// useful once it is divided across every size a frame touches.
struct PoolCensusCounters {
    requests: AtomicU64,
    reuses: AtomicU64,
    buckets: AtomicU64,
    available_entries: AtomicU64,
    available_bytes: AtomicU64,
    budget_evicted_entries: AtomicU64,
    age_evicted_entries: AtomicU64,
}

impl PoolCensusCounters {
    const fn new() -> Self {
        Self {
            requests: AtomicU64::new(0),
            reuses: AtomicU64::new(0),
            buckets: AtomicU64::new(0),
            available_entries: AtomicU64::new(0),
            available_bytes: AtomicU64::new(0),
            budget_evicted_entries: AtomicU64::new(0),
            age_evicted_entries: AtomicU64::new(0),
        }
    }
}

static POOL_CENSUS: [PoolCensusCounters; 2] = [const { PoolCensusCounters::new() }; 2];

fn pool_census(kind: TexturePoolKind) -> &'static PoolCensusCounters {
    match kind {
        TexturePoolKind::General => &POOL_CENSUS[0],
        TexturePoolKind::Offscreen => &POOL_CENSUS[1],
    }
}

fn texture_key(origin: TextureOrigin, width: u32, height: u32, samples: u32) -> u64 {
    let origin = match origin {
        TextureOrigin::Pool => 1_u64,
        TextureOrigin::Bitmap => 2_u64,
        TextureOrigin::GradientAtlas => 3_u64,
    };
    // Never zero, so an untouched slot is distinguishable from a real bucket.
    (origin << 62)
        | ((samples.min(15) as u64) << 58)
        | ((width.min(0x1FFF_FFFF) as u64) << 29)
        | (height.min(0x1FFF_FFFF) as u64)
}

pub fn record_texture_created(
    origin: TextureOrigin,
    width: u32,
    height: u32,
    samples: u32,
    bytes: u64,
) {
    let key = texture_key(origin, width, height, samples);
    let start = (key % TEXTURE_BUCKETS as u64) as usize;
    for probe in 0..TEXTURE_BUCKETS {
        let slot = &TEXTURE_CENSUS[(start + probe) % TEXTURE_BUCKETS];
        let existing = slot.key.load(Ordering::Relaxed);
        if existing == key
            || (existing == 0
                && slot
                    .key
                    .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok())
        {
            slot.count.fetch_add(1, Ordering::Relaxed);
            slot.bytes.fetch_add(bytes, Ordering::Relaxed);
            return;
        }
    }
    let overflow = &TEXTURE_CENSUS_OVERFLOW[overflow_index(origin)];
    overflow.count.fetch_add(1, Ordering::Relaxed);
    overflow.bytes.fetch_add(bytes, Ordering::Relaxed);
}

#[cfg(test)]
fn reset_texture_census() {
    for slot in &TEXTURE_CENSUS {
        slot.key.store(0, Ordering::Relaxed);
        slot.count.store(0, Ordering::Relaxed);
        slot.bytes.store(0, Ordering::Relaxed);
    }
    for slot in &TEXTURE_CENSUS_OVERFLOW {
        slot.count.store(0, Ordering::Relaxed);
        slot.bytes.store(0, Ordering::Relaxed);
    }
    LIVE_TEXTURE_BYTES.store(0, Ordering::Relaxed);
    LIVE_TEXTURE_COUNT.store(0, Ordering::Relaxed);
    LIVE_MEMORY_ALLOCATIONS.store(0, Ordering::Relaxed);
    LIVE_SAMPLES.store(0, Ordering::Relaxed);
    PEAK_TEXTURE_BYTES.store(0, Ordering::Relaxed);
    PEAK_TEXTURE_COUNT.store(0, Ordering::Relaxed);
    for census in &POOL_CENSUS {
        census.requests.store(0, Ordering::Relaxed);
        census.reuses.store(0, Ordering::Relaxed);
        census.buckets.store(0, Ordering::Relaxed);
        census.available_entries.store(0, Ordering::Relaxed);
        census.available_bytes.store(0, Ordering::Relaxed);
        census.budget_evicted_entries.store(0, Ordering::Relaxed);
        census.age_evicted_entries.store(0, Ordering::Relaxed);
    }
}

/// One line per size bucket, biggest total bytes first. Safe to call from a device-loss handler.
pub fn texture_census_report(limit: usize) -> Vec<String> {
    let mut rows: Vec<(u64, u64, String)> = TEXTURE_CENSUS
        .iter()
        .filter_map(|slot| {
            let key = slot.key.load(Ordering::Relaxed);
            if key == 0 {
                return None;
            }
            let count = slot.count.load(Ordering::Relaxed);
            let bytes = slot.bytes.load(Ordering::Relaxed);
            let origin = match key >> 62 {
                1 => TextureOrigin::Pool,
                2 => TextureOrigin::Bitmap,
                _ => TextureOrigin::GradientAtlas,
            };
            let samples = (key >> 58) & 0xF;
            let width = (key >> 29) & 0x1FFF_FFFF;
            let height = key & 0x1FFF_FFFF;
            Some((
                bytes,
                count,
                format!(
                    "{:>6} {:>5}x{:<5} x{}  {:>7} allocs  {:>9.1} MB",
                    origin.name(),
                    width,
                    height,
                    samples,
                    count,
                    bytes as f64 / 1e6,
                ),
            ))
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));

    let overflow_count: u64 = TEXTURE_CENSUS_OVERFLOW
        .iter()
        .map(|slot| slot.count.load(Ordering::Relaxed))
        .sum();
    let overflow_bytes: u64 = TEXTURE_CENSUS_OVERFLOW
        .iter()
        .map(|slot| slot.bytes.load(Ordering::Relaxed))
        .sum();
    let total_bytes: u64 = rows
        .iter()
        .map(|r| r.0)
        .sum::<u64>()
        .saturating_add(overflow_bytes);
    let total_count: u64 = rows
        .iter()
        .map(|r| r.1)
        .sum::<u64>()
        .saturating_add(overflow_count);
    let used_buckets = rows.len();
    let mut out = vec![format!(
        "texture census: {} allocations, {:.1} GB created in total, {}/{} size buckets used{}",
        total_count,
        total_bytes as f64 / 1e9,
        used_buckets,
        TEXTURE_BUCKETS,
        if used_buckets >= TEXTURE_BUCKETS {
            " (table full)"
        } else {
            ""
        },
    )];

    // Created-vs-resident is the whole question: churn that is freed again costs frame time, but
    // only memory still held loses the device. Report it first, above the size breakdown.
    if LIVE_SAMPLES.load(Ordering::Relaxed) > 0 {
        out.push(format!(
            "  live at fault: {} textures, {:.2} GB resident, {} device allocations",
            LIVE_TEXTURE_COUNT.load(Ordering::Relaxed),
            LIVE_TEXTURE_BYTES.load(Ordering::Relaxed) as f64 / 1e9,
            LIVE_MEMORY_ALLOCATIONS.load(Ordering::Relaxed),
        ));
        out.push(format!(
            "  peak live:     {} textures, {:.2} GB resident",
            PEAK_TEXTURE_COUNT.load(Ordering::Relaxed),
            PEAK_TEXTURE_BYTES.load(Ordering::Relaxed) as f64 / 1e9,
        ));
    }

    for (census, name) in POOL_CENSUS.iter().zip(["general", "offscreen"]) {
        let requests = census.requests.load(Ordering::Relaxed);
        if requests == 0 {
            continue;
        }
        let reuses = census.reuses.load(Ordering::Relaxed);
        out.push(format!(
            "  {name} pool: {reuses}/{requests} reused ({:.1}%), {} buckets, \
             {} entries / {:.2} GB retained, evicted {} for budget / {} for age",
            reuses as f64 * 100.0 / requests as f64,
            census.buckets.load(Ordering::Relaxed),
            census.available_entries.load(Ordering::Relaxed),
            census.available_bytes.load(Ordering::Relaxed) as f64 / 1e9,
            census.budget_evicted_entries.load(Ordering::Relaxed),
            census.age_evicted_entries.load(Ordering::Relaxed),
        ));
    }

    out.extend(rows.into_iter().take(limit).map(|r| r.2));

    for (index, slot) in TEXTURE_CENSUS_OVERFLOW.iter().enumerate() {
        let count = slot.count.load(Ordering::Relaxed);
        if count == 0 {
            continue;
        }
        out.push(format!(
            "{:>6} (sizes past the table)  {:>7} allocs  {:>9.1} MB",
            overflow_origin(index).name(),
            count,
            slot.bytes.load(Ordering::Relaxed) as f64 / 1e6,
        ));
    }
    out
}
