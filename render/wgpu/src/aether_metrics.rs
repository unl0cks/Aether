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
    /// Multisample resolves, and the pixels they covered.
    ///
    /// A resolve reads every sample of a target and writes one texel per pixel, so its cost is the
    /// pixel count times the sample count and has nothing to do with what was drawn. Attached to
    /// every pass it tracked `render_passes`; deferred to the reads that need it, the gap between
    /// these two counters is the work that stopped happening.
    pub msaa_resolves: u64,
    pub msaa_resolve_pixels: u64,
    /// Resolves the region test avoided. Against `msaa_resolves`, what it bought.
    pub msaa_resolves_skipped: u64,
    /// How many times a frame's commands were handed over part-way through. Zero means every frame
    /// reached the driver in one piece, which is the state the device was lost in.
    pub submission_splits: u64,
    /// Why runs of blends are not sharing passes, and how many passes a perfect batcher would use.
    ///
    /// `blend_chunks` alone cannot say whether batching is failing or simply has nothing to batch.
    /// These do: `blend_full_surface` says the regions are unusable, `blend_adjacent` says whether
    /// compatible blends ever sit next to each other, and `blend_ideal_batches` is the composite
    /// pass count a batcher that could reorder freely would reach. The gap between that and
    /// `blend_chunks` is the entire prize, and it is worth knowing before building anything.
    pub blend_full_surface: u64,
    pub blend_adjacent: u64,
    pub blend_ideal_batches: u64,
    pub blend_break_not_blend: u64,
    pub blend_break_mode: u64,
    pub blend_break_overlap: u64,
    pub draw_chunks: u64,
    /// Draw chunks whose extent could not be worked out, so no blend may be reordered past them.
    /// If this is large the reordering is being blocked by missing bounds rather than by overlap.
    pub draw_unbounded: u64,
    /// CPU time spent building the per-blend bind groups counted by `bind_groups`.
    ///
    /// Frame time is now known to be CPU encoding rather than GPU work, and one bind group is
    /// built per blend and never reused, so this says whether that is where it goes.
    pub bind_group_nanos: u64,
    /// How many blends the reorder actually relocated, and how many draw chunks ran out of boxes.
    ///
    /// These separate the two ways the reordering can come to nothing. If `moves` is near zero the
    /// rule still cannot fire and the constraint is elsewhere; if `moves` is large but blend passes
    /// have not fallen, the moves are not producing batches. And if `at_box_capacity` is large the
    /// eight boxes are saturating and re-merging into the same screen-wide union that made the
    /// first attempt useless.
    pub blend_reorder_moves: u64,
    pub draw_chunks_at_box_capacity: u64,
    /// Bitmap-cache entries rebuilt this interval, split by whether they carry a filter.
    ///
    /// The two cost completely different things and only one of them can be avoided. A filterless
    /// cache that is dirty every frame is pure loss -- its subtree could have been drawn straight
    /// into the parent -- while a filtered one has to render to a texture whatever we do, because
    /// the filter reads it back.
    pub cache_entries_filtered: u64,
    pub cache_entries_filterless: u64,
    pub cache_entry_filters: u64,
    /// Cache-entry encoding time split by whether the entry carries a filter. Only the filterless
    /// half is reclaimable by a caching decision; the filtered half belongs to the blur.
    pub cache_filtered_nanos: u64,
    pub cache_filterless_nanos: u64,
    /// How the frame's blur-family filters would group under an atlas.
    ///
    /// `filter_applications / filter_groups` is the batch size atlasing would reach. At 1.0 the
    /// objects share no blur parameters and atlasing is worth nothing, which is the whole go/no-go.
    pub filter_applications: u64,
    pub filter_groups: u64,
    pub filter_largest_group: u64,
    /// Groups that really were blurred together, and both sides of the trade. `members / groups` is
    /// the passes bought; `atlas_pixels / member_pixels` is the padding paid for them.
    /// `cacheAsBitmap` surfaces served from the pool against those that reached the driver.
    pub cache_texture_pool_hits: u64,
    pub cache_texture_pool_misses: u64,
    pub filter_atlas_groups: u64,
    pub filter_atlas_members: u64,
    pub filter_atlas_pixels: u64,
    pub filter_atlas_member_pixels: u64,
    /// Cache groups whose CONTENTS drew in one shared pass, and how many entries that
    /// covered. `members / groups` is the content passes each shared pass replaced.
    pub cache_content_atlas_groups: u64,
    pub cache_content_atlas_members: u64,
    /// The same, for complex-blend children sharing a pass instead of taking one each.
    pub blend_child_atlas_groups: u64,
    pub blend_child_atlas_members: u64,
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
static SUBMISSION_SPLITS: AtomicU64 = AtomicU64::new(0);
static DRAW_COMMANDS: AtomicU64 = AtomicU64::new(0);
static BLEND_CHUNKS: AtomicU64 = AtomicU64::new(0);
static BIND_GROUPS: AtomicU64 = AtomicU64::new(0);
static MSAA_RESOLVES: AtomicU64 = AtomicU64::new(0);
static MSAA_RESOLVE_PIXELS: AtomicU64 = AtomicU64::new(0);

/// Record one multisample resolve over `pixels` pixels of a target.
static MSAA_RESOLVES_SKIPPED: AtomicU64 = AtomicU64::new(0);

/// A blend read back a region nothing had drawn into since the last resolve, so the whole
/// resolve was skipped. Against `msaa_resolves`, this is what the region test is buying.
pub fn record_msaa_resolve_skipped() {
    saturating_atomic_add(&MSAA_RESOLVES_SKIPPED, 1);
}

pub fn record_msaa_resolve(pixels: u64) {
    saturating_atomic_add(&MSAA_RESOLVES, 1);
    saturating_atomic_add(&MSAA_RESOLVE_PIXELS, pixels);
}

/// Record one chunk of encoded work: a render pass, and either the draws it carries or the fact
/// that it is a blend needing its own target.
pub fn record_encoded_chunk(draw_commands: u64, is_blend: bool) {
    saturating_atomic_add(&RENDER_PASSES, 1);
    saturating_atomic_add(&DRAW_COMMANDS, draw_commands);
    if is_blend {
        saturating_atomic_add(&BLEND_CHUNKS, 1);
    } else {
        saturating_atomic_add(&DRAW_CHUNKS, 1);
    }
}

static BLEND_FULL_SURFACE: AtomicU64 = AtomicU64::new(0);
static BLEND_ADJACENT: AtomicU64 = AtomicU64::new(0);
static BLEND_IDEAL_BATCHES: AtomicU64 = AtomicU64::new(0);
static BLEND_BREAK_NOT_BLEND: AtomicU64 = AtomicU64::new(0);
static BLEND_BREAK_MODE: AtomicU64 = AtomicU64::new(0);
static BLEND_BREAK_OVERLAP: AtomicU64 = AtomicU64::new(0);
static DRAW_CHUNKS: AtomicU64 = AtomicU64::new(0);
static DRAW_UNBOUNDED: AtomicU64 = AtomicU64::new(0);
static BLEND_REORDER_MOVES: AtomicU64 = AtomicU64::new(0);
static DRAW_CHUNKS_AT_BOX_CAPACITY: AtomicU64 = AtomicU64::new(0);

/// Record the blends one target's reorder relocated, and how many of its draw chunks were full.
pub fn record_blend_reorder(moves: u64, chunks_at_capacity: u64) {
    saturating_atomic_add(&BLEND_REORDER_MOVES, moves);
    saturating_atomic_add(&DRAW_CHUNKS_AT_BOX_CAPACITY, chunks_at_capacity);
}

static CACHE_ENTRIES_FILTERED: AtomicU64 = AtomicU64::new(0);
static CACHE_ENTRIES_FILTERLESS: AtomicU64 = AtomicU64::new(0);
static CACHE_ENTRY_FILTERS: AtomicU64 = AtomicU64::new(0);

/// Record one rebuilt bitmap-cache entry and how many filters it carries.
pub fn record_cache_entry(filters: u64) {
    if filters == 0 {
        saturating_atomic_add(&CACHE_ENTRIES_FILTERLESS, 1);
    } else {
        saturating_atomic_add(&CACHE_ENTRIES_FILTERED, 1);
        saturating_atomic_add(&CACHE_ENTRY_FILTERS, filters);
    }
}

static CACHE_FILTERED_NANOS: AtomicU64 = AtomicU64::new(0);
static CACHE_FILTERLESS_NANOS: AtomicU64 = AtomicU64::new(0);

/// Split the cache-entry encoding cost by whether the entry carries a filter.
///
/// The two are not the same problem and only one of them is reclaimable. A filterless entry that is
/// dirty every frame is pure loss, because its subtree could have been drawn straight into the
/// parent; a filtered one has to render to a texture no matter what, because the filter reads it
/// back. `cache N ms over M entries` cannot tell those apart, and it is the largest single item in
/// a crowded frame, so which half it is decides what to build.
pub fn record_cache_entry_time(filtered: bool, elapsed: std::time::Duration) {
    let nanos = elapsed.as_nanos() as u64;
    if filtered {
        saturating_atomic_add(&CACHE_FILTERED_NANOS, nanos);
    } else {
        saturating_atomic_add(&CACHE_FILTERLESS_NANOS, nanos);
    }
}

static FILTER_APPLICATIONS: AtomicU64 = AtomicU64::new(0);
static FILTER_GROUPS: AtomicU64 = AtomicU64::new(0);
static FILTER_LARGEST_GROUP: AtomicU64 = AtomicU64::new(0);

/// The grouping key for a filter whose cost is a separable blur, or `None` if it has no such cost.
///
/// This exists to decide one question: would atlasing the frame's filter work pay? Atlasing packs
/// several objects into one padded texture and runs a single horizontal and vertical pass over the
/// group, so it only helps objects that share blur parameters exactly. Two glows of radius 8 can
/// share; a radius-8 and a radius-17 cannot share anything.
///
/// Keyed on the raw fixed-point radii and the pass count rather than anything derived, so two
/// filters group only when they would genuinely run the same kernel.
pub fn atlasable_filter_signature(filter: &ruffle_render::filters::Filter) -> Option<u64> {
    use ruffle_render::filters::Filter;

    // A distinct tag per variant, so two filters with equal radii but different maths never share
    // a group.
    let (tag, blur_x, blur_y, passes) = match filter {
        Filter::BlurFilter(f) => (0u64, f.blur_x.get(), f.blur_y.get(), f.num_passes()),
        Filter::GlowFilter(f) => (1, f.blur_x.get(), f.blur_y.get(), f.num_passes()),
        Filter::DropShadowFilter(f) => (2, f.blur_x.get(), f.blur_y.get(), f.num_passes()),
        Filter::BevelFilter(f) => (3, f.blur_x.get(), f.blur_y.get(), f.num_passes()),
        Filter::GradientGlowFilter(f) => (4, f.blur_x.get(), f.blur_y.get(), f.num_passes()),
        Filter::GradientBevelFilter(f) => (5, f.blur_x.get(), f.blur_y.get(), f.num_passes()),
        // No blur, so no separable passes to share.
        Filter::ColorMatrixFilter(_)
        | Filter::ConvolutionFilter(_)
        | Filter::DisplacementMapFilter(_)
        | Filter::ShaderFilter(_) => return None,
    };

    Some(
        (tag << 56)
            ^ ((blur_x as u32 as u64) << 24)
            ^ ((blur_y as u32 as u64) << 8)
            ^ passes as u64,
    )
}

/// Record how the frame's blur-family filters would group under an atlas.
///
/// `applications / groups` is the batch size an atlas would achieve. At 1.0 every filter has its
/// own parameters and atlasing is worth exactly nothing; the lever is only worth building if this
/// is comfortably above 1.
pub fn record_filter_groups(signatures: &mut Vec<u64>) {
    if signatures.is_empty() {
        return;
    }
    let (groups, largest) = count_signature_groups(signatures);

    saturating_atomic_add(&FILTER_APPLICATIONS, signatures.len() as u64);
    saturating_atomic_add(&FILTER_GROUPS, groups);
    saturating_atomic_add(&FILTER_LARGEST_GROUP, largest);
}

/// How many distinct groups a frame's signatures form, and the size of the biggest.
///
/// Split out from the recording so the arithmetic can be tested without a GPU or a frame. Sorts in
/// place and counts runs; the caller's vector order is not meaningful afterwards.
fn count_signature_groups(signatures: &mut [u64]) -> (u64, u64) {
    signatures.sort_unstable();

    let mut groups = 0u64;
    let mut largest = 0u64;
    let mut run = 0u64;
    let mut previous = None;
    for &signature in signatures.iter() {
        if Some(signature) == previous {
            run += 1;
        } else {
            groups += 1;
            // The run that just ended, not the one starting, so the final run needs the same
            // comparison again after the loop.
            largest = largest.max(run);
            run = 1;
            previous = Some(signature);
        }
    }

    (groups, largest.max(run))
}

static CACHE_TEXTURE_POOL_HITS: AtomicU64 = AtomicU64::new(0);
static CACHE_TEXTURE_POOL_MISSES: AtomicU64 = AtomicU64::new(0);

/// Record whether a `cacheAsBitmap` surface came from the pool or from the driver.
///
/// A miss is a `device.create_texture` on the render thread, and a burst of them is the measured
/// cause of frame-time spikes: 108 allocations in the worst seconds against 12 in the rest, with
/// the render baseline unchanged. The hit rate is how that gets confirmed as fixed rather than
/// assumed, and the offscreen pool it copies reaches 99.6%.
pub fn record_cache_texture_pool(hit: bool) {
    if hit {
        saturating_atomic_add(&CACHE_TEXTURE_POOL_HITS, 1);
    } else {
        saturating_atomic_add(&CACHE_TEXTURE_POOL_MISSES, 1);
    }
}

static BLEND_CHILD_ATLAS_GROUPS: AtomicU64 = AtomicU64::new(0);
static BLEND_CHILD_ATLAS_MEMBERS: AtomicU64 = AtomicU64::new(0);

/// Record a batch of complex-blend children that drew in one shared pass.
pub fn record_blend_child_atlas(members: u64) {
    saturating_atomic_add(&BLEND_CHILD_ATLAS_GROUPS, 1);
    saturating_atomic_add(&BLEND_CHILD_ATLAS_MEMBERS, members);
}

static CACHE_CONTENT_ATLAS_GROUPS: AtomicU64 = AtomicU64::new(0);
static CACHE_CONTENT_ATLAS_MEMBERS: AtomicU64 = AtomicU64::new(0);

/// Record a cache group whose contents drew in one shared pass instead of one pass each.
pub fn record_cache_content_atlas(members: u64) {
    saturating_atomic_add(&CACHE_CONTENT_ATLAS_GROUPS, 1);
    saturating_atomic_add(&CACHE_CONTENT_ATLAS_MEMBERS, members);
}

static FILTER_ATLAS_GROUPS: AtomicU64 = AtomicU64::new(0);
static FILTER_ATLAS_MEMBERS: AtomicU64 = AtomicU64::new(0);
static FILTER_ATLAS_PIXELS: AtomicU64 = AtomicU64::new(0);
static FILTER_ATLAS_MEMBER_PIXELS: AtomicU64 = AtomicU64::new(0);

/// Record one group that was actually blurred together, and both sides of the trade it makes.
///
/// Atlasing buys passes and pays pixels. `members / groups` is what it bought: a group of six runs
/// one set of blur passes where six would have run their own. `pixels` against `member_pixels` is
/// what it cost: every source is padded by the blur's reach on all four sides, so a small object
/// with a wide kernel can easily take three times its own area in the atlas.
///
/// Both are needed to judge it. Frame time was measured as CPU encoding rather than GPU work, which
/// is why trading pixels for passes should win -- but "should" is why this is counted rather than
/// assumed, and a scene of small objects with wide glows is exactly where it could lose.
pub fn record_filter_atlas(members: u64, atlas_pixels: u64, member_pixels: u64) {
    saturating_atomic_add(&FILTER_ATLAS_GROUPS, 1);
    saturating_atomic_add(&FILTER_ATLAS_MEMBERS, members);
    saturating_atomic_add(&FILTER_ATLAS_PIXELS, atlas_pixels);
    saturating_atomic_add(&FILTER_ATLAS_MEMBER_PIXELS, member_pixels);
}

/// What one target's chunk list says about why its blends are not sharing passes.
#[derive(Default, Clone, Copy)]
pub struct BlendBatchCensus {
    pub full_surface: u64,
    pub adjacent: u64,
    pub ideal_batches: u64,
    pub break_not_blend: u64,
    pub break_mode: u64,
    pub break_overlap: u64,
    pub draw_unbounded: u64,
}

pub fn record_blend_batch_census(census: BlendBatchCensus) {
    saturating_atomic_add(&BLEND_FULL_SURFACE, census.full_surface);
    saturating_atomic_add(&BLEND_ADJACENT, census.adjacent);
    saturating_atomic_add(&BLEND_IDEAL_BATCHES, census.ideal_batches);
    saturating_atomic_add(&BLEND_BREAK_NOT_BLEND, census.break_not_blend);
    saturating_atomic_add(&BLEND_BREAK_MODE, census.break_mode);
    saturating_atomic_add(&BLEND_BREAK_OVERLAP, census.break_overlap);
    saturating_atomic_add(&DRAW_UNBOUNDED, census.draw_unbounded);
}

/// A frame's commands were handed to the driver part-way through rather than all at once.
pub fn record_submission_split() {
    saturating_atomic_add(&SUBMISSION_SPLITS, 1);
}

/// Record a bind group built during encoding rather than served from a cache.
pub fn record_bind_group_created() {
    saturating_atomic_add(&BIND_GROUPS, 1);
}

static BIND_GROUP_NANOS: AtomicU64 = AtomicU64::new(0);

/// CPU time spent inside `create_bind_group` for complex blends.
///
/// Frame time is now known to be CPU encoding rather than GPU work -- a crowded Yulgar frame
/// spends 54 of its 57 ms in submit while the queue blocks for 2 -- and that comes to ~67 us per
/// render pass, which is a great deal for describing one. One bind group is built per blend and
/// never reused, so this says whether that is where the time goes before anything is built to
/// cache them.
pub fn record_bind_group_nanos(elapsed: std::time::Duration) {
    saturating_atomic_add(&BIND_GROUP_NANOS, nanos(elapsed));
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
        msaa_resolves: MSAA_RESOLVES.swap(0, Ordering::Relaxed),
        msaa_resolve_pixels: MSAA_RESOLVE_PIXELS.swap(0, Ordering::Relaxed),
        msaa_resolves_skipped: MSAA_RESOLVES_SKIPPED.swap(0, Ordering::Relaxed),
        render_passes: RENDER_PASSES.swap(0, Ordering::Relaxed),
        submission_splits: SUBMISSION_SPLITS.swap(0, Ordering::Relaxed),
        complex_blends: std::array::from_fn(|i| COMPLEX_BLENDS[i].swap(0, Ordering::Relaxed)),
        draw_commands: DRAW_COMMANDS.swap(0, Ordering::Relaxed),
        blend_chunks: BLEND_CHUNKS.swap(0, Ordering::Relaxed),
        bind_groups: BIND_GROUPS.swap(0, Ordering::Relaxed),
        blend_full_surface: BLEND_FULL_SURFACE.swap(0, Ordering::Relaxed),
        blend_adjacent: BLEND_ADJACENT.swap(0, Ordering::Relaxed),
        blend_ideal_batches: BLEND_IDEAL_BATCHES.swap(0, Ordering::Relaxed),
        blend_break_not_blend: BLEND_BREAK_NOT_BLEND.swap(0, Ordering::Relaxed),
        blend_break_mode: BLEND_BREAK_MODE.swap(0, Ordering::Relaxed),
        blend_break_overlap: BLEND_BREAK_OVERLAP.swap(0, Ordering::Relaxed),
        draw_chunks: DRAW_CHUNKS.swap(0, Ordering::Relaxed),
        draw_unbounded: DRAW_UNBOUNDED.swap(0, Ordering::Relaxed),
        bind_group_nanos: BIND_GROUP_NANOS.swap(0, Ordering::Relaxed),
        blend_reorder_moves: BLEND_REORDER_MOVES.swap(0, Ordering::Relaxed),
        draw_chunks_at_box_capacity: DRAW_CHUNKS_AT_BOX_CAPACITY.swap(0, Ordering::Relaxed),
        cache_entries_filtered: CACHE_ENTRIES_FILTERED.swap(0, Ordering::Relaxed),
        cache_entries_filterless: CACHE_ENTRIES_FILTERLESS.swap(0, Ordering::Relaxed),
        cache_entry_filters: CACHE_ENTRY_FILTERS.swap(0, Ordering::Relaxed),
        cache_filtered_nanos: CACHE_FILTERED_NANOS.swap(0, Ordering::Relaxed),
        cache_filterless_nanos: CACHE_FILTERLESS_NANOS.swap(0, Ordering::Relaxed),
        filter_applications: FILTER_APPLICATIONS.swap(0, Ordering::Relaxed),
        filter_groups: FILTER_GROUPS.swap(0, Ordering::Relaxed),
        filter_largest_group: FILTER_LARGEST_GROUP.swap(0, Ordering::Relaxed),
        cache_texture_pool_hits: CACHE_TEXTURE_POOL_HITS.swap(0, Ordering::Relaxed),
        cache_texture_pool_misses: CACHE_TEXTURE_POOL_MISSES.swap(0, Ordering::Relaxed),
        filter_atlas_groups: FILTER_ATLAS_GROUPS.swap(0, Ordering::Relaxed),
        filter_atlas_members: FILTER_ATLAS_MEMBERS.swap(0, Ordering::Relaxed),
        filter_atlas_pixels: FILTER_ATLAS_PIXELS.swap(0, Ordering::Relaxed),
        filter_atlas_member_pixels: FILTER_ATLAS_MEMBER_PIXELS.swap(0, Ordering::Relaxed),
        cache_content_atlas_groups: CACHE_CONTENT_ATLAS_GROUPS.swap(0, Ordering::Relaxed),
        cache_content_atlas_members: CACHE_CONTENT_ATLAS_MEMBERS.swap(0, Ordering::Relaxed),
        blend_child_atlas_groups: BLEND_CHILD_ATLAS_GROUPS.swap(0, Ordering::Relaxed),
        blend_child_atlas_members: BLEND_CHILD_ATLAS_MEMBERS.swap(0, Ordering::Relaxed),
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

/// Serialises the tests that touch the process-global census.
///
/// The counters and the recent-texture ring are one shared set of statics, so any two tests that
/// write them are reading each other's work when cargo runs them on different threads. That is not
/// hypothetical: the ring test records textures, which lands in the same census the pool and
/// residency tests assert exact totals against, and those four failed in parallel while passing
/// under `--test-threads=1`.
///
/// It lives out here rather than inside one test module because both modules need it.
#[cfg(test)]
static METRICS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod recent_texture_tests {
    use super::*;

    /// A device lost with 15 GB free is not explained by any total, so the crash report has to be
    /// able to name the request that failed. The newest entry is the one that matters, which means
    /// the ring must not report it first or bury it once it wraps.
    #[test]
    fn the_ring_reports_recent_creations_oldest_first_and_survives_wrapping() {
        let _guard = METRICS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ = recent_textures();
        for index in 0..(RECENT_TEXTURE_SLOTS as u32 + 3) {
            record_texture_created(TextureOrigin::Pool, 100 + index, 200 + index, 4, 64);
        }

        let recent = recent_textures();
        assert_eq!(recent.len(), RECENT_TEXTURE_SLOTS);

        let newest = recent.last().unwrap();
        let last_index = RECENT_TEXTURE_SLOTS as u32 + 2;
        assert_eq!(newest.width, 100 + last_index);
        assert_eq!(newest.height, 200 + last_index);
        assert_eq!(newest.samples, 4);
        assert_eq!(newest.origin, "pool");

        // Oldest surviving is three in, since three were overwritten.
        assert_eq!(recent[0].width, 103);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture_pool_policy::PoolMaintenanceReport;

    /// The go/no-go arithmetic for atlasing filters, including the case that decides it.
    #[test]
    fn signature_groups_count_runs_including_the_last_one() {
        // Nothing shares parameters: one group each, biggest group of one. This is the reading that
        // would mean an atlas buys nothing, so it has to be unambiguous.
        let (groups, largest) = count_signature_groups(&mut [1, 2, 3, 4]);
        assert_eq!((groups, largest), (4, 1));

        // All identical: one group covering everything.
        let (groups, largest) = count_signature_groups(&mut [7, 7, 7, 7]);
        assert_eq!((groups, largest), (1, 4));

        // Unsorted input, and the largest run sits at the END -- the case a run-counter that only
        // compares on change silently loses.
        let (groups, largest) = count_signature_groups(&mut [5, 9, 5, 9, 9, 9]);
        assert_eq!((groups, largest), (2, 4));

        let (groups, largest) = count_signature_groups(&mut [3]);
        assert_eq!((groups, largest), (1, 1));
    }

    /// Filters with no blur have no separable passes, so they must not be counted as atlasable.
    #[test]
    fn only_blur_family_filters_get_a_signature() {
        use ruffle_render::filters::Filter;

        let blur = Filter::BlurFilter(swf::BlurFilter {
            blur_x: swf::Fixed16::from_f32(8.0),
            blur_y: swf::Fixed16::from_f32(8.0),
            flags: swf::BlurFilterFlags::from_passes(2),
        });
        let same = Filter::BlurFilter(swf::BlurFilter {
            blur_x: swf::Fixed16::from_f32(8.0),
            blur_y: swf::Fixed16::from_f32(8.0),
            flags: swf::BlurFilterFlags::from_passes(2),
        });
        let wider = Filter::BlurFilter(swf::BlurFilter {
            blur_x: swf::Fixed16::from_f32(17.0),
            blur_y: swf::Fixed16::from_f32(8.0),
            flags: swf::BlurFilterFlags::from_passes(2),
        });

        assert_eq!(
            atlasable_filter_signature(&blur),
            atlasable_filter_signature(&same),
            "equal parameters must share a group, or atlasing looks worthless when it is not"
        );
        assert_ne!(
            atlasable_filter_signature(&blur),
            atlasable_filter_signature(&wider),
            "a different radius runs a different kernel and cannot share a pass"
        );
        assert!(
            atlasable_filter_signature(&Filter::ColorMatrixFilter(Default::default())).is_none(),
            "a colour matrix has no blur to batch"
        );
    }

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
        let _guard = METRICS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = METRICS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = METRICS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        // Derived, not spelled out. This read `513` from when the table held 512 buckets, so
        // growing it to 4096 turned a working guard into a failing one and the assertion stopped
        // saying anything about the overflow it exists to check.
        assert!(
            report.contains(&format!("{} allocations", TEXTURE_BUCKETS + 1)),
            "the header total must include the overflow: {report}"
        );
    }

    #[test]
    fn texture_census_names_gradient_atlas_allocations() {
        let _guard = METRICS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = METRICS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = METRICS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
const TEXTURE_BUCKETS: usize = 4096;
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

/// The most recent texture creations, newest last.
///
/// When the device is lost to "Out of memory" the log carries no indication of what failed: wgpu
/// reports the loss and nothing else. Totals cannot answer it either, since the card had 15 GB free
/// at the time. What is missing is the request itself, so the last few are kept and printed with the
/// census. The one at the end is the closest thing to the allocation that killed the device.
const RECENT_TEXTURE_SLOTS: usize = 16;
static RECENT_TEXTURES: [AtomicU64; RECENT_TEXTURE_SLOTS] =
    [const { AtomicU64::new(0) }; RECENT_TEXTURE_SLOTS];
static RECENT_TEXTURE_CURSOR: AtomicU64 = AtomicU64::new(0);

/// One recent creation, decoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecentTexture {
    pub origin: &'static str,
    pub width: u32,
    pub height: u32,
    pub samples: u32,
}

/// The recent creations, oldest first. Slots never written are skipped.
pub fn recent_textures() -> Vec<RecentTexture> {
    let cursor = RECENT_TEXTURE_CURSOR.load(Ordering::Relaxed);
    let mut out = Vec::with_capacity(RECENT_TEXTURE_SLOTS);
    for step in 0..RECENT_TEXTURE_SLOTS as u64 {
        // Start one past the newest so the oldest surviving entry comes first.
        let index = ((cursor + step) % RECENT_TEXTURE_SLOTS as u64) as usize;
        let key = RECENT_TEXTURES[index].load(Ordering::Relaxed);
        if key == 0 {
            continue;
        }
        out.push(RecentTexture {
            // The encoding is 1-based on purpose, so that an untouched slot reads as zero.
            origin: match key >> 62 {
                1 => TextureOrigin::Pool,
                2 => TextureOrigin::Bitmap,
                _ => TextureOrigin::GradientAtlas,
            }
            .name(),
            width: ((key >> 29) & 0x1FFF_FFFF) as u32,
            height: (key & 0x1FFF_FFFF) as u32,
            samples: ((key >> 58) & 0xF) as u32,
        });
    }
    out
}

pub fn record_texture_created(
    origin: TextureOrigin,
    width: u32,
    height: u32,
    samples: u32,
    bytes: u64,
) {
    let key = texture_key(origin, width, height, samples);
    // Wrapping ring, written before the census bookkeeping below so an allocation that is about to
    // fail is still recorded.
    let slot =
        RECENT_TEXTURE_CURSOR.fetch_add(1, Ordering::Relaxed) as usize % RECENT_TEXTURE_SLOTS;
    RECENT_TEXTURES[slot].store(key, Ordering::Relaxed);
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

    // The requests themselves, newest last. Every device loss so far has arrived with no indication
    // of what was being allocated, on a card with most of its memory free, so the totals above have
    // never been able to explain one. The final entry is the closest thing to the failing request.
    let recent = recent_textures();
    if !recent.is_empty() {
        // Say "most recent", not "before the fault": this report also prints from the routine
        // once-a-minute census, and the old wording read as an allocation failure in an ordinary
        // healthy session, which cost a real investigation a wrong turn on 2026-08-21.
        out.push(format!(
            "most recent {} texture requests (newest last):",
            recent.len()
        ));
        for texture in recent {
            out.push(format!(
                "  {:>6} {:>5}x{:<5} x{}",
                texture.origin, texture.width, texture.height, texture.samples,
            ));
        }
    }
    out
}
