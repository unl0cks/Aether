use ruffle_core::aether_diagnostics::{CacheOffenderSummary, EnterFrameHandlerReport};
use ruffle_core::aether_metrics::{
    Avm2BroadcastSnapshot, Avm2NestedGotoSnapshot, InputSnapshot, RenderSnapshot, TickSnapshot,
    TimingSnapshot,
};
use ruffle_render_wgpu::aether_metrics::{PoolSnapshot, WgpuMetricsSnapshot};
use serde::Serialize;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_SAMPLES: usize = 2_048;
const REPORT_INTERVAL: Duration = Duration::from_secs(1);
const MAX_CACHE_OFFENDERS_PER_REPORT: usize = 20;
const MAX_ENTER_FRAME_HANDLERS_PER_REPORT: usize = 20;

#[derive(Debug, Default, Serialize)]
struct CounterTotals {
    display_objects_entered: u64,
    display_objects_mask_skipped: u64,
    container_children_visited: u64,
    container_children_invisible: u64,
    container_masks_rendered: u64,
    bitmap_cache_checks: u64,
    bitmap_cache_hits: u64,
    bitmap_cache_fast_hits: u64,
    bitmap_cache_full_evaluations: u64,
    bitmap_cache_rebuilds: u64,
    bitmap_cache_oversize: u64,
    offscreen_cache_draws: u64,
    bitmap_cache_texture_allocations: u64,
    bitmap_cache_texture_allocated_bytes_estimate: u64,
    bitmap_cache_texture_reuses: u64,
    bitmap_cache_texture_resize_allocations: u64,
}

impl CounterTotals {
    fn add(&mut self, sample: RenderSnapshot) {
        self.display_objects_entered = self
            .display_objects_entered
            .saturating_add(sample.display_objects_entered);
        self.display_objects_mask_skipped = self
            .display_objects_mask_skipped
            .saturating_add(sample.display_objects_mask_skipped);
        self.container_children_visited = self
            .container_children_visited
            .saturating_add(sample.container_children_visited);
        self.container_children_invisible = self
            .container_children_invisible
            .saturating_add(sample.container_children_invisible);
        self.container_masks_rendered = self
            .container_masks_rendered
            .saturating_add(sample.container_masks_rendered);
        self.bitmap_cache_checks = self
            .bitmap_cache_checks
            .saturating_add(sample.bitmap_cache_checks);
        self.bitmap_cache_hits = self
            .bitmap_cache_hits
            .saturating_add(sample.bitmap_cache_hits);
        self.bitmap_cache_fast_hits = self
            .bitmap_cache_fast_hits
            .saturating_add(sample.bitmap_cache_fast_hits);
        self.bitmap_cache_full_evaluations = self
            .bitmap_cache_full_evaluations
            .saturating_add(sample.bitmap_cache_full_evaluations);
        self.bitmap_cache_rebuilds = self
            .bitmap_cache_rebuilds
            .saturating_add(sample.bitmap_cache_rebuilds);
        self.bitmap_cache_oversize = self
            .bitmap_cache_oversize
            .saturating_add(sample.bitmap_cache_oversize);
        self.offscreen_cache_draws = self
            .offscreen_cache_draws
            .saturating_add(sample.offscreen_cache_draws);
        self.bitmap_cache_texture_allocations = self
            .bitmap_cache_texture_allocations
            .saturating_add(sample.bitmap_cache_texture_allocations);
        self.bitmap_cache_texture_allocated_bytes_estimate = self
            .bitmap_cache_texture_allocated_bytes_estimate
            .saturating_add(sample.bitmap_cache_texture_allocated_bytes_estimate);
        self.bitmap_cache_texture_reuses = self
            .bitmap_cache_texture_reuses
            .saturating_add(sample.bitmap_cache_texture_reuses);
        self.bitmap_cache_texture_resize_allocations = self
            .bitmap_cache_texture_resize_allocations
            .saturating_add(sample.bitmap_cache_texture_resize_allocations);
    }

    fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}

#[derive(Debug, Default, Serialize)]
struct Avm2BroadcastTotals {
    targets_examined: u64,
    targets_dispatched: u64,
    stale_registrations_pruned: u64,
    handler_snapshot_spills: u64,
}

impl Avm2BroadcastTotals {
    fn add(&mut self, sample: Avm2BroadcastSnapshot) {
        self.targets_examined = self
            .targets_examined
            .saturating_add(sample.targets_examined);
        self.targets_dispatched = self
            .targets_dispatched
            .saturating_add(sample.targets_dispatched);
        self.stale_registrations_pruned = self
            .stale_registrations_pruned
            .saturating_add(sample.stale_registrations_pruned);
        self.handler_snapshot_spills = self
            .handler_snapshot_spills
            .saturating_add(sample.handler_snapshot_spills);
    }

    fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}

#[derive(Debug, Default)]
struct Avm2NestedGotoTotals {
    same_frame: u64,
    changed_frame: u64,
    total: TimingTotals,
    construct: TimingTotals,
    frame_scripts: TimingTotals,
    exit_broadcast: TimingTotals,
}

impl Avm2NestedGotoTotals {
    fn add(&mut self, sample: Avm2NestedGotoSnapshot) {
        self.same_frame = self.same_frame.saturating_add(sample.same_frame);
        self.changed_frame = self.changed_frame.saturating_add(sample.changed_frame);
        self.total.add(sample.total);
        self.construct.add(sample.construct);
        self.frame_scripts.add(sample.frame_scripts);
        self.exit_broadcast.add(sample.exit_broadcast);
    }

    fn take_report(&mut self) -> Avm2NestedGotoReport {
        let totals = std::mem::take(self);
        Avm2NestedGotoReport {
            same_frame: totals.same_frame,
            changed_frame: totals.changed_frame,
            total_scope: "outermost_lifecycle_chain",
            phase_scope: "exclusive_all_depths",
            total: totals.total.into_report(),
            construct: totals.construct.into_report(),
            frame_scripts: totals.frame_scripts.into_report(),
            exit_broadcast: totals.exit_broadcast.into_report(),
        }
    }
}

#[derive(Debug, Serialize)]
struct Avm2NestedGotoReport {
    same_frame: u64,
    changed_frame: u64,
    total_scope: &'static str,
    phase_scope: &'static str,
    total: TimingReport,
    construct: TimingReport,
    frame_scripts: TimingReport,
    exit_broadcast: TimingReport,
}

#[derive(Debug, Default)]
struct TimingTotals {
    total_ns: u64,
    invocations: u64,
}

impl TimingTotals {
    fn add(&mut self, sample: TimingSnapshot) {
        self.total_ns = self.total_ns.saturating_add(sample.total_ns);
        self.invocations = self.invocations.saturating_add(sample.invocations);
    }

    fn into_report(self) -> TimingReport {
        TimingReport {
            total_us: self.total_ns / 1_000,
            invocations: self.invocations,
        }
    }
}

#[derive(Debug, Default, Serialize)]
struct TimingReport {
    total_us: u64,
    invocations: u64,
}

#[derive(Debug, Default)]
struct TickPhaseTotals {
    total_tick_ns: u64,
    tick_invocations: u64,
    preload: TimingTotals,
    avm2_enter: TimingTotals,
    avm2_enter_orphans: TimingTotals,
    avm2_enter_stage: TimingTotals,
    avm2_enter_broadcast: TimingTotals,
    avm2_construct: TimingTotals,
    avm2_frame_scripts: TimingTotals,
    avm2_exit: TimingTotals,
    avm2_post: TimingTotals,
    avm1_frame: TimingTotals,
    sound_update: TimingTotals,
    local_connections: TimingTotals,
    post_frame_callbacks: TimingTotals,
    audio_skew: TimingTotals,
    sockets: TimingTotals,
    net_connections: TimingTotals,
    timers: TimingTotals,
    streams: TimingTotals,
    audio_tick: TimingTotals,
}

impl TickPhaseTotals {
    fn add(&mut self, duration: Duration, sample: TickSnapshot) {
        self.total_tick_ns = self
            .total_tick_ns
            .saturating_add(u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX));
        self.tick_invocations = self.tick_invocations.saturating_add(1);
        self.preload.add(sample.preload);
        self.avm2_enter.add(sample.avm2_enter);
        self.avm2_enter_orphans.add(sample.avm2_enter_orphans);
        self.avm2_enter_stage.add(sample.avm2_enter_stage);
        self.avm2_enter_broadcast.add(sample.avm2_enter_broadcast);
        self.avm2_construct.add(sample.avm2_construct);
        self.avm2_frame_scripts.add(sample.avm2_frame_scripts);
        self.avm2_exit.add(sample.avm2_exit);
        self.avm2_post.add(sample.avm2_post);
        self.avm1_frame.add(sample.avm1_frame);
        self.sound_update.add(sample.sound_update);
        self.local_connections.add(sample.local_connections);
        self.post_frame_callbacks.add(sample.post_frame_callbacks);
        self.audio_skew.add(sample.audio_skew);
        self.sockets.add(sample.sockets);
        self.net_connections.add(sample.net_connections);
        self.timers.add(sample.timers);
        self.streams.add(sample.streams);
        self.audio_tick.add(sample.audio_tick);
    }

    fn take_report(&mut self) -> TickPhaseReport {
        let totals = std::mem::take(self);
        let measured_ns = [
            totals.preload.total_ns,
            totals.avm2_enter.total_ns,
            totals.avm2_construct.total_ns,
            totals.avm2_frame_scripts.total_ns,
            totals.avm2_exit.total_ns,
            totals.avm2_post.total_ns,
            totals.avm1_frame.total_ns,
            totals.sound_update.total_ns,
            totals.local_connections.total_ns,
            totals.post_frame_callbacks.total_ns,
            totals.audio_skew.total_ns,
            totals.sockets.total_ns,
            totals.net_connections.total_ns,
            totals.timers.total_ns,
            totals.streams.total_ns,
            totals.audio_tick.total_ns,
        ]
        .into_iter()
        .fold(0_u64, u64::saturating_add);

        TickPhaseReport {
            preload: totals.preload.into_report(),
            avm2_enter: totals.avm2_enter.into_report(),
            avm2_enter_orphans: totals.avm2_enter_orphans.into_report(),
            avm2_enter_stage: totals.avm2_enter_stage.into_report(),
            avm2_enter_broadcast: totals.avm2_enter_broadcast.into_report(),
            avm2_construct: totals.avm2_construct.into_report(),
            avm2_frame_scripts: totals.avm2_frame_scripts.into_report(),
            avm2_exit: totals.avm2_exit.into_report(),
            avm2_post: totals.avm2_post.into_report(),
            avm1_frame: totals.avm1_frame.into_report(),
            sound_update: totals.sound_update.into_report(),
            local_connections: totals.local_connections.into_report(),
            post_frame_callbacks: totals.post_frame_callbacks.into_report(),
            audio_skew: totals.audio_skew.into_report(),
            sockets: totals.sockets.into_report(),
            net_connections: totals.net_connections.into_report(),
            timers: totals.timers.into_report(),
            streams: totals.streams.into_report(),
            audio_tick: totals.audio_tick.into_report(),
            other: TimingReport {
                total_us: totals.total_tick_ns.saturating_sub(measured_ns) / 1_000,
                invocations: totals.tick_invocations,
            },
        }
    }
}

#[derive(Debug, Default, Serialize)]
struct TickPhaseReport {
    preload: TimingReport,
    avm2_enter: TimingReport,
    avm2_enter_orphans: TimingReport,
    avm2_enter_stage: TimingReport,
    avm2_enter_broadcast: TimingReport,
    avm2_construct: TimingReport,
    avm2_frame_scripts: TimingReport,
    avm2_exit: TimingReport,
    avm2_post: TimingReport,
    avm1_frame: TimingReport,
    sound_update: TimingReport,
    local_connections: TimingReport,
    post_frame_callbacks: TimingReport,
    audio_skew: TimingReport,
    sockets: TimingReport,
    net_connections: TimingReport,
    timers: TimingReport,
    streams: TimingReport,
    audio_tick: TimingReport,
    other: TimingReport,
}

#[derive(Debug, Default, Serialize)]
struct WgpuPoolTotals {
    requests: u64,
    reuses: u64,
    allocations: u64,
    allocated_bytes_estimate: u64,
    unknown_size_allocations: u64,
    resets: u64,
    discarded_available_entries: u64,
    maintenance_passes: u64,
    available_entries_after_maintenance: u64,
    available_bytes_after_maintenance: u64,
    age_evicted_entries: u64,
    age_evicted_bytes_estimate: u64,
    budget_evicted_entries: u64,
    budget_evicted_bytes_estimate: u64,
    unknown_size_retention_rejections: u64,
    globals_available_after_maintenance: u64,
    globals_age_evictions: u64,
    globals_budget_evictions: u64,
}

impl WgpuPoolTotals {
    fn add(&mut self, sample: PoolSnapshot) {
        self.requests = self.requests.saturating_add(sample.requests);
        self.reuses = self.reuses.saturating_add(sample.reuses);
        self.allocations = self.allocations.saturating_add(sample.allocations);
        self.allocated_bytes_estimate = self
            .allocated_bytes_estimate
            .saturating_add(sample.allocated_bytes_estimate);
        self.unknown_size_allocations = self
            .unknown_size_allocations
            .saturating_add(sample.unknown_size_allocations);
        self.resets = self.resets.saturating_add(sample.resets);
        self.discarded_available_entries = self
            .discarded_available_entries
            .saturating_add(sample.discarded_available_entries);
        self.maintenance_passes = self
            .maintenance_passes
            .saturating_add(sample.maintenance_passes);
        self.available_entries_after_maintenance = sample.available_entries_after_maintenance;
        self.available_bytes_after_maintenance = sample.available_bytes_after_maintenance;
        self.age_evicted_entries = self
            .age_evicted_entries
            .saturating_add(sample.age_evicted_entries);
        self.age_evicted_bytes_estimate = self
            .age_evicted_bytes_estimate
            .saturating_add(sample.age_evicted_bytes_estimate);
        self.budget_evicted_entries = self
            .budget_evicted_entries
            .saturating_add(sample.budget_evicted_entries);
        self.budget_evicted_bytes_estimate = self
            .budget_evicted_bytes_estimate
            .saturating_add(sample.budget_evicted_bytes_estimate);
        self.unknown_size_retention_rejections = self
            .unknown_size_retention_rejections
            .saturating_add(sample.unknown_size_retention_rejections);
        self.globals_available_after_maintenance = sample.globals_available_after_maintenance;
        self.globals_age_evictions = self
            .globals_age_evictions
            .saturating_add(sample.globals_age_evictions);
        self.globals_budget_evictions = self
            .globals_budget_evictions
            .saturating_add(sample.globals_budget_evictions);
    }
}

#[derive(Debug, Default, Serialize)]
struct WgpuTexturePoolTotals {
    general: WgpuPoolTotals,
    offscreen: WgpuPoolTotals,
    /// CPU time the interval spent creating textures, and how many it created. This lands inside
    /// the render time reported below, so the two can be compared directly.
    texture_create_nanos: u64,
    texture_creations: u64,
    submit_nanos: u64,
    cache_entry_nanos: u64,
    cache_entries: u64,
    queue_nanos: u64,
    render_passes: u64,
    complex_blends: [u64; ruffle_render_wgpu::aether_metrics::COMPLEX_BLEND_MODES],
    draw_commands: u64,
    blend_chunks: u64,
    bind_groups: u64,
}

impl WgpuTexturePoolTotals {
    fn add(&mut self, sample: WgpuMetricsSnapshot) {
        self.general.add(sample.general);
        self.offscreen.add(sample.offscreen);
        self.texture_create_nanos = self
            .texture_create_nanos
            .saturating_add(sample.texture_create_nanos);
        self.texture_creations = self
            .texture_creations
            .saturating_add(sample.texture_creations);
        self.submit_nanos = self.submit_nanos.saturating_add(sample.submit_nanos);
        self.cache_entry_nanos = self
            .cache_entry_nanos
            .saturating_add(sample.cache_entry_nanos);
        self.cache_entries = self.cache_entries.saturating_add(sample.cache_entries);
        self.queue_nanos = self.queue_nanos.saturating_add(sample.queue_nanos);
        self.render_passes = self.render_passes.saturating_add(sample.render_passes);
        for (total, sample) in self.complex_blends.iter_mut().zip(sample.complex_blends) {
            *total = total.saturating_add(sample);
        }
        self.draw_commands = self.draw_commands.saturating_add(sample.draw_commands);
        self.blend_chunks = self.blend_chunks.saturating_add(sample.blend_chunks);
        self.bind_groups = self.bind_groups.saturating_add(sample.bind_groups);
    }

    fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}

#[derive(Debug, Default, Serialize)]
struct CadenceTotals {
    host_tick_calls: u64,
    authored_frames_executed: u64,
    authored_catch_up_resets: u64,
    redraw_events: u64,
    native_movie_renders: u64,
    surface_presentations: u64,
    surface_unavailable: u64,
}

impl CadenceTotals {
    fn record_tick(&mut self, sample: TickSnapshot) {
        self.host_tick_calls = self.host_tick_calls.saturating_add(1);
        self.authored_frames_executed = self
            .authored_frames_executed
            .saturating_add(sample.authored_frames_executed);
        self.authored_catch_up_resets = self
            .authored_catch_up_resets
            .saturating_add(sample.authored_catch_up_resets);
    }

    fn record_render(&mut self, player_rendered: bool, surface_presented: bool) {
        self.redraw_events = self.redraw_events.saturating_add(1);
        if player_rendered {
            self.native_movie_renders = self.native_movie_renders.saturating_add(1);
        }
        if surface_presented {
            self.surface_presentations = self.surface_presentations.saturating_add(1);
        } else {
            self.surface_unavailable = self.surface_unavailable.saturating_add(1);
        }
    }

    fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}

#[derive(Debug, Default)]
struct InputTotals {
    host_mouse_moves_received: u64,
    host_mouse_moves_gui_consumed: u64,
    host_mouse_moves_duplicate: u64,
    host_mouse_moves_coalesced: u64,
    host_mouse_moves_dispatched: u64,
    core_mouse_moves: TimingTotals,
    mouse_picks: TimingTotals,
}

#[derive(Debug, Default, Serialize)]
struct InputReport {
    host_mouse_moves_received: u64,
    host_mouse_moves_gui_consumed: u64,
    host_mouse_moves_duplicate: u64,
    host_mouse_moves_coalesced: u64,
    host_mouse_moves_dispatched: u64,
    core_mouse_moves: TimingReport,
    mouse_picks: TimingReport,
}

impl InputTotals {
    fn host_received(&mut self) {
        self.host_mouse_moves_received = self.host_mouse_moves_received.saturating_add(1);
    }

    fn host_gui_consumed(&mut self) {
        self.host_mouse_moves_gui_consumed = self.host_mouse_moves_gui_consumed.saturating_add(1);
    }

    fn host_duplicate(&mut self) {
        self.host_mouse_moves_duplicate = self.host_mouse_moves_duplicate.saturating_add(1);
    }

    fn host_coalesced(&mut self) {
        self.host_mouse_moves_coalesced = self.host_mouse_moves_coalesced.saturating_add(1);
    }

    fn host_dispatched(&mut self) {
        self.host_mouse_moves_dispatched = self.host_mouse_moves_dispatched.saturating_add(1);
    }

    fn add_core(&mut self, snapshot: InputSnapshot) {
        self.core_mouse_moves.add(snapshot.mouse_moves);
        self.mouse_picks.add(snapshot.mouse_picks);
    }

    fn take_report(&mut self) -> InputReport {
        let totals = std::mem::take(self);
        InputReport {
            host_mouse_moves_received: totals.host_mouse_moves_received,
            host_mouse_moves_gui_consumed: totals.host_mouse_moves_gui_consumed,
            host_mouse_moves_duplicate: totals.host_mouse_moves_duplicate,
            host_mouse_moves_coalesced: totals.host_mouse_moves_coalesced,
            host_mouse_moves_dispatched: totals.host_mouse_moves_dispatched,
            core_mouse_moves: totals.core_mouse_moves.into_report(),
            mouse_picks: totals.mouse_picks.into_report(),
        }
    }
}

#[derive(Debug, Serialize)]
struct Distribution {
    samples: usize,
    mean_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

/// How many distinct allocation labels a census reports before collapsing the rest.
const MAX_ALLOCATION_BUCKETS: usize = 12;

/// The label wgpu allocations carry when the renderer did not name them.
const UNLABELLED_ALLOCATION: &str = "<unlabelled>";

/// Replace every run of digits with `#`, so per-object labels collapse to their class.
///
/// Ruffle names resources after the object that owns them ("Shape 4821 vertices"), which
/// makes every allocation its own group and attributes nothing.
fn collapse_identifiers(label: &str) -> String {
    let mut collapsed = String::with_capacity(label.len());
    let mut in_digits = false;
    for character in label.chars() {
        if character.is_ascii_digit() {
            if !in_digits {
                collapsed.push('#');
                in_digits = true;
            }
        } else {
            collapsed.push(character);
            in_digits = false;
        }
    }
    collapsed
}

/// A named group of live GPU allocations.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AllocationBucket {
    name: String,
    count: usize,
    bytes: u64,
}

/// Group live GPU allocations by their label, largest total first.
///
/// The driver reports every allocation individually and AQW produces tens of thousands of
/// them, so the useful output is which label owns the bytes rather than the raw blocks.
fn summarize_allocations(
    allocations: impl IntoIterator<Item = (String, u64)>,
    limit: usize,
) -> Vec<AllocationBucket> {
    let mut totals: std::collections::HashMap<String, (usize, u64)> =
        std::collections::HashMap::new();
    for (name, size) in allocations {
        let name = if name.is_empty() {
            UNLABELLED_ALLOCATION.to_string()
        } else {
            collapse_identifiers(&name)
        };
        let entry = totals.entry(name).or_default();
        entry.0 = entry.0.saturating_add(1);
        entry.1 = entry.1.saturating_add(size);
    }

    let mut buckets: Vec<AllocationBucket> = totals
        .into_iter()
        .map(|(name, (count, bytes))| AllocationBucket { name, count, bytes })
        .collect();
    // Break ties on name so the output is stable across runs and diffable.
    buckets.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    buckets.truncate(limit);
    buckets
}

/// Live wgpu resource counts and byte totals, straight from the driver's own registries.
///
/// Counts alone were misleading: 4,700 textures and 37,900 buffers are indistinguishable from
/// 200 MB and 9.5 GB, and the fix is completely different in each case. The byte fields come
/// from wgpu-hal's own accounting, and `total_allocated_bytes` is the device allocator's
/// ground truth to check the attribution against.
#[derive(Clone, Debug, Default, Serialize)]
pub struct WgpuResourceCensus {
    textures: usize,
    texture_views: usize,
    buffers: usize,
    bind_groups: usize,
    samplers: usize,
    render_pipelines: usize,
    shader_modules: usize,
    /// Resources the user has released but which the driver has not reclaimed yet.
    textures_kept_from_user: usize,
    buffers_kept_from_user: usize,
    texture_views_kept_from_user: usize,
    /// GPU memory wgpu-hal attributes to buffers, in bytes. Requires wgpu's `counters`.
    buffer_memory_bytes: u64,
    /// GPU memory wgpu-hal attributes to textures, in bytes. Requires wgpu's `counters`.
    texture_memory_bytes: u64,
    memory_allocations: u64,
    /// Sum of live allocations, per the device allocator. Zero on backends without one.
    total_allocated_bytes: u64,
    /// Memory reserved by the allocator's blocks, including unallocated slack. The gap
    /// between this and `total_allocated_bytes` is fragmentation we are paying for.
    total_reserved_bytes: u64,
    largest_allocations: Vec<AllocationBucket>,
}

/// Take a census of live GPU resources. Walks every wgpu registry and the device allocator,
/// so call it at most once per reporting interval rather than per frame.
pub fn wgpu_resource_census(
    instance: &wgpu::Instance,
    device: &wgpu::Device,
) -> Option<WgpuResourceCensus> {
    let report = instance.generate_report()?;
    let hub = report.hub;
    let hal = device.get_internal_counters().hal;
    let allocator = device.generate_allocator_report();

    Some(WgpuResourceCensus {
        textures: hub.textures.num_allocated,
        texture_views: hub.texture_views.num_allocated,
        buffers: hub.buffers.num_allocated,
        bind_groups: hub.bind_groups.num_allocated,
        samplers: hub.samplers.num_allocated,
        render_pipelines: hub.render_pipelines.num_allocated,
        shader_modules: hub.shader_modules.num_allocated,
        textures_kept_from_user: hub.textures.num_kept_from_user,
        buffers_kept_from_user: hub.buffers.num_kept_from_user,
        texture_views_kept_from_user: hub.texture_views.num_kept_from_user,
        buffer_memory_bytes: hal.buffer_memory.read().max(0) as u64,
        texture_memory_bytes: hal.texture_memory.read().max(0) as u64,
        memory_allocations: hal.memory_allocations.read().max(0) as u64,
        total_allocated_bytes: allocator
            .as_ref()
            .map_or(0, |report| report.total_allocated_bytes),
        total_reserved_bytes: allocator
            .as_ref()
            .map_or(0, |report| report.total_reserved_bytes),
        largest_allocations: allocator.map_or_else(Vec::new, |report| {
            summarize_allocations(
                report
                    .allocations
                    .into_iter()
                    .map(|allocation| (allocation.name, allocation.size)),
                MAX_ALLOCATION_BUCKETS,
            )
        }),
    })
}

#[derive(Debug, Serialize)]
struct MetricsLine {
    unix_time_ms: u128,
    elapsed_seconds: f64,
    interval_us: u64,
    tick: Distribution,
    player_render: Distribution,
    gui_present: Distribution,
    render_frames: usize,
    cadence: CadenceTotals,
    input: InputReport,
    counters: CounterTotals,
    avm2_broadcast: Avm2BroadcastTotals,
    avm2_nested_goto: Avm2NestedGotoReport,
    enter_frame_handlers: EnterFrameHandlerReport,
    tick_phases: TickPhaseReport,
    wgpu_texture_pools: WgpuTexturePoolTotals,
    wgpu_resources: Option<WgpuResourceCensus>,
    /// Total failed AQW class lookups this interval, including repeats the log suppresses.
    aqw_definition_lookup_misses: u64,
    cache_offenders: Vec<CacheOffenderSummary>,
}

/// Low-overhead one-second summaries for Aether's baseline and optimization comparisons.
pub struct AetherMetrics {
    enabled: bool,
    writer: Option<BufWriter<File>>,
    started_at: Instant,
    last_report: Instant,
    tick_ms: VecDeque<f64>,
    player_render_ms: VecDeque<f64>,
    gui_present_ms: VecDeque<f64>,
    render_frames: usize,
    cadence: CadenceTotals,
    input: InputTotals,
    counters: CounterTotals,
    avm2_broadcast: Avm2BroadcastTotals,
    avm2_nested_goto: Avm2NestedGotoTotals,
    tick_phases: TickPhaseTotals,
    wgpu_texture_pools: WgpuTexturePoolTotals,
    pending_census: Option<WgpuResourceCensus>,
}

impl AetherMetrics {
    pub fn new(enabled: bool, output_path: PathBuf) -> Self {
        let writer = if enabled {
            open_writer(&output_path)
        } else {
            None
        };
        let enabled = enabled && writer.is_some();
        ruffle_core::aether_diagnostics::set_cache_offender_enabled(enabled);
        ruffle_core::aether_diagnostics::set_enter_frame_handler_attribution_enabled(enabled);
        ruffle_core::aether_metrics::set_avm2_nested_goto_metrics_enabled(enabled);

        let now = Instant::now();
        Self {
            enabled,
            writer,
            started_at: now,
            last_report: now,
            tick_ms: VecDeque::with_capacity(MAX_SAMPLES.min(512)),
            player_render_ms: VecDeque::with_capacity(MAX_SAMPLES.min(512)),
            gui_present_ms: VecDeque::with_capacity(MAX_SAMPLES.min(512)),
            render_frames: 0,
            cadence: CadenceTotals::default(),
            input: InputTotals::default(),
            counters: CounterTotals::default(),
            avm2_broadcast: Avm2BroadcastTotals::default(),
            avm2_nested_goto: Avm2NestedGotoTotals::default(),
            tick_phases: TickPhaseTotals::default(),
            wgpu_texture_pools: WgpuTexturePoolTotals::default(),
            pending_census: None,
        }
    }

    /// Whether a new report is about to be written, so the caller should take a GPU
    /// resource census. Walking the wgpu registries is too costly to do every frame.
    pub fn census_due(&self) -> bool {
        self.enabled
            && self.pending_census.is_none()
            && self.last_report.elapsed() >= REPORT_INTERVAL
    }

    pub fn set_resource_census(&mut self, census: Option<WgpuResourceCensus>) {
        if self.enabled {
            self.pending_census = census;
        }
    }

    pub fn record_tick(&mut self, duration: Duration, phases: TickSnapshot) {
        if !self.enabled {
            return;
        }
        push_bounded(&mut self.tick_ms, duration.as_secs_f64() * 1_000.0);
        self.avm2_broadcast.add(phases.avm2_broadcast);
        self.avm2_nested_goto.add(phases.avm2_nested_goto);
        self.cadence.record_tick(phases);
        self.tick_phases.add(duration, phases);
        self.flush_if_due();
    }

    pub fn record_render(
        &mut self,
        player_render: Duration,
        gui_present: Duration,
        player_rendered: bool,
        surface_presented: bool,
        counters: RenderSnapshot,
        wgpu: WgpuMetricsSnapshot,
    ) {
        if !self.enabled {
            return;
        }
        push_bounded(
            &mut self.player_render_ms,
            player_render.as_secs_f64() * 1_000.0,
        );
        push_bounded(
            &mut self.gui_present_ms,
            gui_present.as_secs_f64() * 1_000.0,
        );
        self.render_frames = self.render_frames.saturating_add(1);
        self.cadence
            .record_render(player_rendered, surface_presented);
        self.counters.add(counters);
        self.wgpu_texture_pools.add(wgpu);
        self.flush_if_due();
    }

    pub fn record_host_mouse_received(&mut self) {
        if self.enabled {
            self.input.host_received();
        }
    }

    pub fn record_host_mouse_gui_consumed(&mut self) {
        if self.enabled {
            self.input.host_gui_consumed();
        }
    }

    pub fn record_host_mouse_duplicate(&mut self) {
        if self.enabled {
            self.input.host_duplicate();
        }
    }

    pub fn record_host_mouse_coalesced(&mut self) {
        if self.enabled {
            self.input.host_coalesced();
        }
    }

    pub fn record_host_mouse_dispatched(&mut self) {
        if self.enabled {
            self.input.host_dispatched();
        }
    }

    fn flush_if_due(&mut self) {
        if self.last_report.elapsed() < REPORT_INTERVAL {
            return;
        }
        self.flush_summary();
        self.last_report = Instant::now();
    }

    fn flush_summary(&mut self) {
        let interval_us = u64::try_from(self.last_report.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.input
            .add_core(ruffle_core::aether_metrics::take_input_snapshot());
        let line = MetricsLine {
            unix_time_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or_default(),
            elapsed_seconds: self.started_at.elapsed().as_secs_f64(),
            interval_us,
            tick: distribution(&self.tick_ms),
            player_render: distribution(&self.player_render_ms),
            gui_present: distribution(&self.gui_present_ms),
            render_frames: self.render_frames,
            cadence: self.cadence.take(),
            input: self.input.take_report(),
            counters: self.counters.take(),
            avm2_broadcast: self.avm2_broadcast.take(),
            avm2_nested_goto: self.avm2_nested_goto.take_report(),
            enter_frame_handlers: ruffle_core::aether_diagnostics::take_enter_frame_handler_report(
                MAX_ENTER_FRAME_HANDLERS_PER_REPORT,
            ),
            tick_phases: self.tick_phases.take_report(),
            wgpu_texture_pools: self.wgpu_texture_pools.take(),
            wgpu_resources: self.pending_census.take(),
            aqw_definition_lookup_misses:
                ruffle_core::aether_compatibility::take_aqw_definition_lookup_miss_count(),
            cache_offenders: ruffle_core::aether_diagnostics::take_cache_offender_summaries(
                MAX_CACHE_OFFENDERS_PER_REPORT,
            ),
        };

        // The JSONL carries far more than this, but the first question about any frame-rate
        // problem is only ever "CPU or GPU", and answering it should not require parsing a file.
        tracing::info!(
            target: "aether::perf",
            "{}",
            perf_summary_line(
                line.interval_us,
                line.cadence.authored_frames_executed,
                line.render_frames,
                &line.tick,
                &line.player_render,
                &line.gui_present,
                line.wgpu_texture_pools.texture_create_nanos,
                line.wgpu_texture_pools.texture_creations,
                line.wgpu_texture_pools.submit_nanos,
                line.wgpu_texture_pools.cache_entry_nanos,
                line.wgpu_texture_pools.cache_entries,
                &line.wgpu_texture_pools,
            )
        );

        self.tick_ms.clear();
        self.player_render_ms.clear();
        self.gui_present_ms.clear();
        self.render_frames = 0;

        let Some(writer) = self.writer.as_mut() else {
            return;
        };

        match serde_json::to_writer(&mut *writer, &line) {
            Ok(()) => {
                if let Err(error) = writer.write_all(b"\n").and_then(|_| writer.flush()) {
                    tracing::warn!("Failed to flush Aether metrics: {error}");
                }
            }
            Err(error) => tracing::warn!("Failed to serialize Aether metrics: {error}"),
        }
    }
}

/// Name the blend modes a frame actually used, loudest first.
///
/// Frame time tracks render passes, and complex blends are what create them, so this says which
/// of the nine modes is worth doing something about. Silent when nothing blended.
fn complex_blend_breakdown(
    counts: &[u64; ruffle_render_wgpu::aether_metrics::COMPLEX_BLEND_MODES],
    rendered_frames: f64,
) -> String {
    let names = ruffle_render_wgpu::aether_metrics::COMPLEX_BLEND_NAMES;
    let mut used: Vec<_> = counts
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .collect();
    if used.is_empty() {
        return String::new();
    }

    used.sort_by(|a, b| b.1.cmp(a.1));
    let listed: Vec<String> = used
        .iter()
        .map(|(mode, count)| format!("{} {:.0}", names[*mode], **count as f64 / rendered_frames))
        .collect();
    format!(" | blends: {}", listed.join(", "))
}

/// Condense one reporting interval into a single line.
///
/// `tick` is the player's own work for the interval: advancing timelines and running ActionScript.
/// `render` is building and submitting the frame. `present` is handing it to the compositor. A
/// scene that is CPU-bound shows a large tick and a small render; one that is GPU-bound shows the
/// reverse. Separating the SWF's executed frame rate from the host's render rate matters too,
/// because the two only agree while the player is keeping up with its own frame budget.
#[allow(clippy::too_many_arguments)]
fn perf_summary_line(
    interval_us: u64,
    authored_frames_executed: u64,
    render_frames: usize,
    tick: &Distribution,
    player_render: &Distribution,
    gui_present: &Distribution,
    texture_create_nanos: u64,
    texture_creations: u64,
    submit_nanos: u64,
    cache_entry_nanos: u64,
    cache_entries: u64,
    encoding: &WgpuTexturePoolTotals,
) -> String {
    // Clamping to a microsecond rather than to MIN_POSITIVE: dividing by the latter overflows to
    // infinity, which is not an improvement on dividing by zero.
    let seconds = interval_us.max(1) as f64 / 1_000_000.0;
    let rendered_frames = render_frames.max(1) as f64;
    format!(
        "swf {:.1}/s, render {:.1} fps | tick {:.1}/{:.1} ms | render {:.1}/{:.1} ms | \
         present {:.1}/{:.1} ms (avg/max) | submit {:.1} ms (cache {:.1} ms over {:.0} entries, \
         newtex {:.1} ms over {:.0}) per frame | passes {:.0} draws {:.0} blends {:.0}          binds {:.0} queue {:.1} ms per frame{}",
        authored_frames_executed as f64 / seconds,
        render_frames as f64 / seconds,
        tick.mean_ms,
        tick.max_ms,
        player_render.mean_ms,
        player_render.max_ms,
        gui_present.mean_ms,
        gui_present.max_ms,
        // All four are shares of `render` above. `render` minus `submit` is the core's own
        // display-list traversal and command building, which nothing else reports.
        submit_nanos as f64 / 1_000_000.0 / rendered_frames,
        cache_entry_nanos as f64 / 1_000_000.0 / rendered_frames,
        cache_entries as f64 / rendered_frames,
        texture_create_nanos as f64 / 1_000_000.0 / rendered_frames,
        texture_creations as f64 / rendered_frames,
        encoding.render_passes as f64 / rendered_frames,
        encoding.draw_commands as f64 / rendered_frames,
        encoding.blend_chunks as f64 / rendered_frames,
        encoding.bind_groups as f64 / rendered_frames,
        encoding.queue_nanos as f64 / 1_000_000.0 / rendered_frames,
        complex_blend_breakdown(&encoding.complex_blends, rendered_frames),
    )
}

impl Drop for AetherMetrics {
    fn drop(&mut self) {
        if self.enabled
            && (!self.tick_ms.is_empty()
                || !self.player_render_ms.is_empty()
                || !self.gui_present_ms.is_empty())
        {
            self.flush_summary();
        }
        ruffle_core::aether_diagnostics::set_cache_offender_enabled(false);
        ruffle_core::aether_diagnostics::set_enter_frame_handler_attribution_enabled(false);
        ruffle_core::aether_metrics::set_avm2_nested_goto_metrics_enabled(false);
    }
}

fn open_writer(path: &Path) -> Option<BufWriter<File>> {
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        tracing::warn!("Failed to create Aether metrics directory {parent:?}: {error}");
        return None;
    }

    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => Some(BufWriter::new(file)),
        Err(error) => {
            tracing::warn!("Failed to open Aether metrics file {path:?}: {error}");
            None
        }
    }
}

fn push_bounded(samples: &mut VecDeque<f64>, value: f64) {
    if samples.len() == MAX_SAMPLES {
        samples.pop_front();
    }
    samples.push_back(value);
}

fn distribution(samples: &VecDeque<f64>) -> Distribution {
    if samples.is_empty() {
        return Distribution {
            samples: 0,
            mean_ms: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
            max_ms: 0.0,
        };
    }

    let mut sorted: Vec<f64> = samples.iter().copied().collect();
    sorted.sort_by(f64::total_cmp);
    let sum: f64 = sorted.iter().sum();
    let max_ms = sorted.last().copied().unwrap_or_default();

    Distribution {
        samples: sorted.len(),
        mean_ms: sum / sorted.len() as f64,
        p50_ms: percentile(&sorted, 0.50),
        p95_ms: percentile(&sorted, 0.95),
        p99_ms: percentile(&sorted, 0.99),
        max_ms,
    }
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let last_index = sorted.len().saturating_sub(1);
    let position = (last_index as f64 * percentile).round() as usize;
    sorted
        .get(position.min(last_index))
        .copied()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruffle_core::aether_diagnostics::{
        EnterFrameHandlerDescriptor, EnterFrameHandlerReport, EnterFrameHandlerSummary,
    };
    use ruffle_core::aether_metrics::{
        Avm2NestedGotoSnapshot, InputSnapshot, TickSnapshot, TimingSnapshot,
    };
    use ruffle_render_wgpu::aether_metrics::{PoolSnapshot, WgpuMetricsSnapshot};

    #[test]
    fn cadence_totals_keep_render_and_presentation_semantics_separate() {
        let mut totals = CadenceTotals::default();
        totals.record_tick(TickSnapshot {
            authored_frames_executed: 2,
            authored_catch_up_resets: 1,
            ..Default::default()
        });
        totals.record_render(true, true);
        totals.record_render(true, false);
        totals.record_render(false, true);

        let report = totals.take();
        assert_eq!(report.host_tick_calls, 1);
        assert_eq!(report.authored_frames_executed, 2);
        assert_eq!(report.authored_catch_up_resets, 1);
        assert_eq!(report.redraw_events, 3);
        assert_eq!(report.native_movie_renders, 2);
        assert_eq!(report.surface_presentations, 2);
        assert_eq!(report.surface_unavailable, 1);
    }

    #[test]
    fn cadence_report_serializes_integer_counts() {
        let report = CadenceTotals {
            host_tick_calls: 3,
            native_movie_renders: 4,
            surface_presentations: 5,
            ..Default::default()
        };
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["host_tick_calls"], 3);
        assert_eq!(value["native_movie_renders"], 4);
        assert_eq!(value["surface_presentations"], 5);
    }

    #[test]
    fn avm2_broadcast_totals_accumulate_and_serialize_as_integer_counts() {
        let mut totals = Avm2BroadcastTotals::default();
        totals.add(Avm2BroadcastSnapshot {
            targets_examined: 8,
            targets_dispatched: 5,
            stale_registrations_pruned: 2,
            handler_snapshot_spills: 1,
        });
        totals.add(Avm2BroadcastSnapshot {
            targets_examined: 3,
            targets_dispatched: 2,
            stale_registrations_pruned: 1,
            handler_snapshot_spills: 0,
        });

        let value = serde_json::to_value(totals.take()).unwrap();
        assert_eq!(value["targets_examined"], 11);
        assert_eq!(value["targets_dispatched"], 7);
        assert_eq!(value["stale_registrations_pruned"], 3);
        assert_eq!(value["handler_snapshot_spills"], 1);
    }

    #[test]
    fn avm2_nested_goto_totals_accumulate_serialize_and_reset() {
        let mut totals = Avm2NestedGotoTotals::default();
        totals.add(Avm2NestedGotoSnapshot {
            same_frame: 2,
            changed_frame: 3,
            total: TimingSnapshot {
                total_ns: 9_000,
                invocations: 5,
            },
            construct: TimingSnapshot {
                total_ns: 2_000,
                invocations: 5,
            },
            frame_scripts: TimingSnapshot {
                total_ns: 3_000,
                invocations: 5,
            },
            exit_broadcast: TimingSnapshot {
                total_ns: 1_000,
                invocations: 5,
            },
        });
        totals.add(Avm2NestedGotoSnapshot {
            same_frame: 1,
            changed_frame: 1,
            total: TimingSnapshot {
                total_ns: 6_000,
                invocations: 2,
            },
            construct: TimingSnapshot {
                total_ns: 1_000,
                invocations: 2,
            },
            frame_scripts: TimingSnapshot {
                total_ns: 2_000,
                invocations: 2,
            },
            exit_broadcast: TimingSnapshot {
                total_ns: 1_000,
                invocations: 2,
            },
        });

        let value = serde_json::to_value(totals.take_report()).unwrap();
        assert_eq!(value["same_frame"], 3);
        assert_eq!(value["changed_frame"], 4);
        assert_eq!(value["total"]["total_us"], 15);
        assert_eq!(value["total"]["invocations"], 7);
        assert_eq!(value["construct"]["total_us"], 3);
        assert_eq!(value["frame_scripts"]["total_us"], 5);
        assert_eq!(value["exit_broadcast"]["total_us"], 2);

        let reset = serde_json::to_value(totals.take_report()).unwrap();
        assert_eq!(reset["same_frame"], 0);
        assert_eq!(reset["changed_frame"], 0);
        assert_eq!(reset["total"]["total_us"], 0);
        assert_eq!(reset["total"]["invocations"], 0);
    }

    #[test]
    fn metrics_json_line_exposes_avm2_nested_goto_report() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "aether-nested-goto-metrics-{}-{unique}.jsonl",
            std::process::id()
        ));
        let disabled = AetherMetrics::new(false, path.clone());
        assert!(!ruffle_core::aether_metrics::avm2_nested_goto_metrics_enabled());
        drop(disabled);

        let mut metrics = AetherMetrics::new(true, path.clone());
        assert!(ruffle_core::aether_metrics::avm2_nested_goto_metrics_enabled());
        let mut snapshot = TickSnapshot::default();
        snapshot.avm2_nested_goto = Avm2NestedGotoSnapshot {
            same_frame: 4,
            changed_frame: 6,
            total: TimingSnapshot {
                total_ns: 12_000,
                invocations: 10,
            },
            construct: TimingSnapshot {
                total_ns: 3_000,
                invocations: 10,
            },
            frame_scripts: TimingSnapshot {
                total_ns: 5_000,
                invocations: 10,
            },
            exit_broadcast: TimingSnapshot {
                total_ns: 2_000,
                invocations: 10,
            },
        };
        metrics.record_tick(Duration::from_millis(1), snapshot);
        metrics.flush_summary();
        drop(metrics);
        assert!(!ruffle_core::aether_metrics::avm2_nested_goto_metrics_enabled());

        let contents = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        let _ = std::fs::remove_file(path);
        assert_eq!(value["avm2_nested_goto"]["same_frame"], 4);
        assert_eq!(value["avm2_nested_goto"]["changed_frame"], 6);
        assert_eq!(
            value["avm2_nested_goto"]["total_scope"],
            "outermost_lifecycle_chain"
        );
        assert_eq!(
            value["avm2_nested_goto"]["phase_scope"],
            "exclusive_all_depths"
        );
        assert_eq!(value["avm2_nested_goto"]["total"]["total_us"], 12);
        assert_eq!(value["avm2_nested_goto"]["frame_scripts"]["total_us"], 5);
    }

    #[test]
    fn enter_frame_handler_report_serializes_top_twenty_and_tail_shape() {
        let report = EnterFrameHandlerReport {
            total_us: 12,
            invocations: 4,
            unique_methods: 2,
            top: vec![EnterFrameHandlerSummary {
                descriptor: EnterFrameHandlerDescriptor {
                    qualified_name: "World/countDownAct".to_string(),
                    movie_url: "https://game.aq.com/game/gamefiles/spider.swf".to_string(),
                    abc_method_index: 7,
                    kind: "bytecode",
                },
                total_us: 9,
                invocations: 3,
                max_us: 4,
            }],
            tail_us: 3,
        };

        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["total_us"], 12);
        assert_eq!(value["invocations"], 4);
        assert_eq!(value["unique_methods"], 2);
        assert_eq!(value["tail_us"], 3);
        assert_eq!(value["top"][0]["qualified_name"], "World/countDownAct");
        assert_eq!(
            value["top"][0]["movie_url"],
            "https://game.aq.com/game/gamefiles/spider.swf"
        );
        assert_eq!(value["top"][0]["abc_method_index"], 7);
        assert_eq!(value["top"][0]["kind"], "bytecode");
        assert_eq!(value["top"][0]["total_us"], 9);
        assert_eq!(value["top"][0]["invocations"], 3);
        assert_eq!(value["top"][0]["max_us"], 4);
    }

    #[test]
    fn input_totals_keep_host_and_core_counts_separate() {
        let mut totals = InputTotals::default();
        totals.host_received();
        totals.host_received();
        totals.host_received();
        totals.host_duplicate();
        totals.host_coalesced();
        totals.host_dispatched();
        totals.add_core(InputSnapshot {
            mouse_moves: TimingSnapshot {
                total_ns: 900_000,
                invocations: 1,
            },
            mouse_picks: TimingSnapshot {
                total_ns: 300_000,
                invocations: 2,
            },
        });

        let report = totals.take_report();
        assert_eq!(report.host_mouse_moves_received, 3);
        assert_eq!(report.host_mouse_moves_duplicate, 1);
        assert_eq!(report.host_mouse_moves_coalesced, 1);
        assert_eq!(report.host_mouse_moves_dispatched, 1);
        assert_eq!(report.core_mouse_moves.total_us, 900);
        assert_eq!(report.core_mouse_moves.invocations, 1);
        assert_eq!(report.mouse_picks.total_us, 300);
        assert_eq!(report.mouse_picks.invocations, 2);
    }

    #[test]
    fn tick_totals_report_measured_and_other_microseconds() {
        let mut totals = TickPhaseTotals::default();
        let mut snapshot = TickSnapshot::default();
        snapshot.avm2_enter = TimingSnapshot {
            total_ns: 4_000,
            invocations: 1,
        };
        snapshot.avm2_enter_orphans = TimingSnapshot {
            total_ns: 250,
            invocations: 1,
        };
        snapshot.avm2_enter_stage = TimingSnapshot {
            total_ns: 2_500,
            invocations: 1,
        };
        snapshot.avm2_enter_broadcast = TimingSnapshot {
            total_ns: 1_000,
            invocations: 1,
        };
        snapshot.avm2_exit = TimingSnapshot {
            total_ns: 2_000,
            invocations: 1,
        };
        snapshot.timers = TimingSnapshot {
            total_ns: 1_000,
            invocations: 1,
        };

        totals.add(Duration::from_nanos(10_000), snapshot);
        let report = totals.take_report();

        assert_eq!(report.avm2_exit.total_us, 2);
        assert_eq!(report.timers.total_us, 1);
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["avm2_enter"]["total_us"], 4);
        assert_eq!(value["avm2_enter_orphans"]["total_us"], 0);
        assert_eq!(value["avm2_enter_stage"]["total_us"], 2);
        assert_eq!(value["avm2_enter_broadcast"]["total_us"], 1);
        assert_eq!(report.other.total_us, 3);
        assert_eq!(report.other.invocations, 1);
    }

    #[test]
    fn allocation_summary_groups_by_label_and_keeps_the_biggest_owners() {
        // The census counted objects, not bytes, so 4,700 textures and 37,900 buffers were
        // indistinguishable from 200 MB and 9.5 GB. The driver reports every allocation
        // separately; what decides the fix is which label owns the bytes.
        let allocations = vec![
            ("mesh vertex".to_string(), 4_000),
            ("bitmap".to_string(), 900_000),
            ("mesh vertex".to_string(), 6_000),
            (String::new(), 128),
            ("mesh index".to_string(), 1_000),
            ("bitmap".to_string(), 100_000),
        ];

        let summary = summarize_allocations(allocations, 3);

        assert_eq!(
            summary,
            vec![
                AllocationBucket {
                    name: "bitmap".to_string(),
                    count: 2,
                    bytes: 1_000_000,
                },
                AllocationBucket {
                    name: "mesh vertex".to_string(),
                    count: 2,
                    bytes: 10_000,
                },
                AllocationBucket {
                    name: "mesh index".to_string(),
                    count: 1,
                    bytes: 1_000,
                },
            ],
            "largest total bytes first, grouped by label"
        );
    }

    #[test]
    fn allocation_labels_collapse_per_object_identifiers() {
        // Ruffle labels these per shape ("Shape 4821 vertices"), so grouping on the raw
        // label yields one bucket per shape and attributes nothing. The class is the part
        // that matters; the id is not.
        let summary = summarize_allocations(
            vec![
                ("Shape 12 vertices".to_string(), 3_000),
                ("Shape 4821 vertices".to_string(), 5_000),
                ("Shape 12 indices".to_string(), 700),
                (
                    "Bitmap BitmapHandle(9) bind group (smoothed: true)".to_string(),
                    64,
                ),
            ],
            8,
        );

        assert_eq!(
            summary,
            vec![
                AllocationBucket {
                    name: "Shape # vertices".to_string(),
                    count: 2,
                    bytes: 8_000,
                },
                AllocationBucket {
                    name: "Shape # indices".to_string(),
                    count: 1,
                    bytes: 700,
                },
                AllocationBucket {
                    name: "Bitmap BitmapHandle(#) bind group (smoothed: true)".to_string(),
                    count: 1,
                    bytes: 64,
                },
            ]
        );
    }

    #[test]
    fn unlabelled_allocations_are_still_attributed_rather_than_dropped() {
        // Ruffle only labels resources under `render_debug_labels`. Silently discarding
        // unnamed allocations would under-report exactly the bytes we are hunting.
        let summary = summarize_allocations(
            vec![
                (String::new(), 2_048),
                ("bitmap".to_string(), 512),
                (String::new(), 1_024),
            ],
            8,
        );

        assert_eq!(
            summary,
            vec![
                AllocationBucket {
                    name: "<unlabelled>".to_string(),
                    count: 2,
                    bytes: 3_072,
                },
                AllocationBucket {
                    name: "bitmap".to_string(),
                    count: 1,
                    bytes: 512,
                },
            ]
        );
    }

    #[test]
    fn wgpu_totals_keep_pool_identity_and_serialize_as_integers() {
        let mut totals = WgpuTexturePoolTotals::default();
        totals.add(WgpuMetricsSnapshot {
            general: PoolSnapshot {
                requests: 2,
                reuses: 1,
                allocations: 1,
                allocated_bytes_estimate: 256,
                ..Default::default()
            },
            offscreen: PoolSnapshot {
                resets: 1,
                discarded_available_entries: 3,
                ..Default::default()
            },
        });

        let value = serde_json::to_value(totals.take()).unwrap();
        assert_eq!(value["general"]["allocated_bytes_estimate"], 256);
        assert_eq!(value["offscreen"]["discarded_available_entries"], 3);
    }

    #[test]
    fn wgpu_pool_aggregation_sums_counters_but_keeps_latest_gauges() {
        let mut totals = WgpuPoolTotals::default();
        totals.add(PoolSnapshot {
            maintenance_passes: 1,
            age_evicted_entries: 2,
            available_entries_after_maintenance: 20,
            available_bytes_after_maintenance: 2_000,
            globals_available_after_maintenance: 5,
            ..Default::default()
        });
        totals.add(PoolSnapshot {
            maintenance_passes: 3,
            age_evicted_entries: 4,
            available_entries_after_maintenance: 7,
            available_bytes_after_maintenance: 700,
            globals_available_after_maintenance: 2,
            ..Default::default()
        });

        assert_eq!(totals.maintenance_passes, 4);
        assert_eq!(totals.age_evicted_entries, 6);
        assert_eq!(totals.available_entries_after_maintenance, 7);
        assert_eq!(totals.available_bytes_after_maintenance, 700);
        assert_eq!(totals.globals_available_after_maintenance, 2);
    }

    fn distribution_of(mean_ms: f64, max_ms: f64) -> Distribution {
        Distribution {
            samples: 60,
            mean_ms,
            p50_ms: mean_ms,
            p95_ms: max_ms,
            p99_ms: max_ms,
            max_ms,
        }
    }

    #[test]
    fn the_perf_line_tells_a_cpu_bound_interval_from_a_gpu_bound_one() {
        // The whole point of the line: at 16 fps, is the time in the player or in the renderer?
        // Identical frame rates, opposite causes.
        let heavy = distribution_of(48.3, 91.0);
        let light = distribution_of(8.1, 22.4);
        let idle = distribution_of(1.2, 3.0);

        let cpu_bound = perf_summary_line(1_000_000, 16, 16, &heavy, &light, &idle);
        assert!(
            cpu_bound.contains("swf 16.0/s, render 16.0 fps"),
            "{cpu_bound}"
        );
        assert!(cpu_bound.contains("tick 48.3/91.0 ms"), "{cpu_bound}");
        assert!(cpu_bound.contains("render 8.1/22.4 ms"), "{cpu_bound}");

        let gpu_bound = perf_summary_line(1_000_000, 16, 16, &light, &heavy, &idle);
        assert!(gpu_bound.contains("tick 8.1/22.4 ms"), "{gpu_bound}");
        assert!(gpu_bound.contains("render 48.3/91.0 ms"), "{gpu_bound}");
    }

    #[test]
    fn a_zero_length_interval_reports_a_number_rather_than_infinity() {
        let idle = distribution_of(0.0, 0.0);
        let line = perf_summary_line(0, 5, 5, &idle, &idle, &idle);
        assert!(
            !line.contains("inf") && !line.contains("NaN"),
            "a flush that lands in the same microsecond must not print infinities: {line}"
        );
    }
}
