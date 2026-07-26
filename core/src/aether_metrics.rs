//! Lightweight render counters used by the Aether profiling build.
//!
//! These counters are deliberately lock-free and approximate. They are intended to answer
//! "where did the frame go?" without materially changing the renderer's behavior.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

fn saturating_atomic_add(counter: &AtomicU64, value: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(value))
    });
}

struct TimingCounter {
    total_ns: AtomicU64,
    invocations: AtomicU64,
}

impl TimingCounter {
    const fn new() -> Self {
        Self {
            total_ns: AtomicU64::new(0),
            invocations: AtomicU64::new(0),
        }
    }

    fn record(&self, elapsed_ns: u64) {
        saturating_atomic_add(&self.total_ns, elapsed_ns);
        saturating_atomic_add(&self.invocations, 1);
    }

    fn take(&self) -> TimingSnapshot {
        TimingSnapshot {
            total_ns: self.total_ns.swap(0, Ordering::Relaxed),
            invocations: self.invocations.swap(0, Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TickPhase {
    Preload,
    Avm2Enter,
    Avm2EnterOrphans,
    Avm2EnterStage,
    Avm2EnterBroadcast,
    Avm2Construct,
    Avm2FrameScripts,
    Avm2Exit,
    Avm2Post,
    Avm1Frame,
    SoundUpdate,
    LocalConnections,
    PostFrameCallbacks,
    AudioSkew,
    Sockets,
    NetConnections,
    Timers,
    Streams,
    AudioTick,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TimingSnapshot {
    pub total_ns: u64,
    pub invocations: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InputSnapshot {
    pub mouse_moves: TimingSnapshot,
    pub mouse_picks: TimingSnapshot,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Avm2BroadcastSnapshot {
    pub targets_examined: u64,
    pub targets_dispatched: u64,
    pub stale_registrations_pruned: u64,
    pub handler_snapshot_spills: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Avm2ExplicitGotoKind {
    SameFrame,
    ChangedFrame,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Avm2NestedGotoPhase {
    Total,
    Construct,
    FrameScripts,
    ExitBroadcast,
}

impl Avm2NestedGotoPhase {
    fn timing_index(self) -> usize {
        match self {
            Self::Construct => 0,
            Self::FrameScripts => 1,
            Self::ExitBroadcast => 2,
            Self::Total => panic!("total is not an exclusive nested-goto phase"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Avm2NestedGotoSnapshot {
    pub same_frame: u64,
    pub changed_frame: u64,
    pub total: TimingSnapshot,
    pub construct: TimingSnapshot,
    pub frame_scripts: TimingSnapshot,
    pub exit_broadcast: TimingSnapshot,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TickSnapshot {
    pub authored_frames_executed: u64,
    pub authored_catch_up_resets: u64,
    pub avm2_broadcast: Avm2BroadcastSnapshot,
    pub avm2_nested_goto: Avm2NestedGotoSnapshot,
    pub preload: TimingSnapshot,
    pub avm2_enter: TimingSnapshot,
    pub avm2_enter_orphans: TimingSnapshot,
    pub avm2_enter_stage: TimingSnapshot,
    pub avm2_enter_broadcast: TimingSnapshot,
    pub avm2_construct: TimingSnapshot,
    pub avm2_frame_scripts: TimingSnapshot,
    pub avm2_exit: TimingSnapshot,
    pub avm2_post: TimingSnapshot,
    pub avm1_frame: TimingSnapshot,
    pub sound_update: TimingSnapshot,
    pub local_connections: TimingSnapshot,
    pub post_frame_callbacks: TimingSnapshot,
    pub audio_skew: TimingSnapshot,
    pub sockets: TimingSnapshot,
    pub net_connections: TimingSnapshot,
    pub timers: TimingSnapshot,
    pub streams: TimingSnapshot,
    pub audio_tick: TimingSnapshot,
}

static PRELOAD: TimingCounter = TimingCounter::new();
static AUTHORED_FRAMES_EXECUTED: AtomicU64 = AtomicU64::new(0);
static AUTHORED_CATCH_UP_RESETS: AtomicU64 = AtomicU64::new(0);
static AVM2_BROADCAST_TARGETS_EXAMINED: AtomicU64 = AtomicU64::new(0);
static AVM2_BROADCAST_TARGETS_DISPATCHED: AtomicU64 = AtomicU64::new(0);
static AVM2_BROADCAST_STALE_REGISTRATIONS_PRUNED: AtomicU64 = AtomicU64::new(0);
static AVM2_BROADCAST_HANDLER_SNAPSHOT_SPILLS: AtomicU64 = AtomicU64::new(0);
static AVM2_NESTED_GOTO_METRICS_ENABLED: AtomicBool = AtomicBool::new(false);
static AVM2_EXPLICIT_GOTO_SAME_FRAME: AtomicU64 = AtomicU64::new(0);
static AVM2_EXPLICIT_GOTO_CHANGED_FRAME: AtomicU64 = AtomicU64::new(0);
static AVM2_NESTED_GOTO_TOTAL: TimingCounter = TimingCounter::new();
static AVM2_NESTED_GOTO_CONSTRUCT: TimingCounter = TimingCounter::new();
static AVM2_NESTED_GOTO_FRAME_SCRIPTS: TimingCounter = TimingCounter::new();
static AVM2_NESTED_GOTO_EXIT_BROADCAST: TimingCounter = TimingCounter::new();
static MOUSE_MOVES: TimingCounter = TimingCounter::new();
static MOUSE_PICKS: TimingCounter = TimingCounter::new();
static AVM2_ENTER: TimingCounter = TimingCounter::new();
static AVM2_ENTER_ORPHANS: TimingCounter = TimingCounter::new();
static AVM2_ENTER_STAGE: TimingCounter = TimingCounter::new();
static AVM2_ENTER_BROADCAST: TimingCounter = TimingCounter::new();
static AVM2_CONSTRUCT: TimingCounter = TimingCounter::new();
static AVM2_FRAME_SCRIPTS: TimingCounter = TimingCounter::new();
static AVM2_EXIT: TimingCounter = TimingCounter::new();
static AVM2_POST: TimingCounter = TimingCounter::new();
static AVM1_FRAME: TimingCounter = TimingCounter::new();
static SOUND_UPDATE: TimingCounter = TimingCounter::new();
static LOCAL_CONNECTIONS: TimingCounter = TimingCounter::new();
static POST_FRAME_CALLBACKS: TimingCounter = TimingCounter::new();
static AUDIO_SKEW: TimingCounter = TimingCounter::new();
static SOCKETS: TimingCounter = TimingCounter::new();
static NET_CONNECTIONS: TimingCounter = TimingCounter::new();
static TIMERS: TimingCounter = TimingCounter::new();
static STREAMS: TimingCounter = TimingCounter::new();
static AUDIO_TICK: TimingCounter = TimingCounter::new();

impl TickPhase {
    fn counter(self) -> &'static TimingCounter {
        match self {
            Self::Preload => &PRELOAD,
            Self::Avm2Enter => &AVM2_ENTER,
            Self::Avm2EnterOrphans => &AVM2_ENTER_ORPHANS,
            Self::Avm2EnterStage => &AVM2_ENTER_STAGE,
            Self::Avm2EnterBroadcast => &AVM2_ENTER_BROADCAST,
            Self::Avm2Construct => &AVM2_CONSTRUCT,
            Self::Avm2FrameScripts => &AVM2_FRAME_SCRIPTS,
            Self::Avm2Exit => &AVM2_EXIT,
            Self::Avm2Post => &AVM2_POST,
            Self::Avm1Frame => &AVM1_FRAME,
            Self::SoundUpdate => &SOUND_UPDATE,
            Self::LocalConnections => &LOCAL_CONNECTIONS,
            Self::PostFrameCallbacks => &POST_FRAME_CALLBACKS,
            Self::AudioSkew => &AUDIO_SKEW,
            Self::Sockets => &SOCKETS,
            Self::NetConnections => &NET_CONNECTIONS,
            Self::Timers => &TIMERS,
            Self::Streams => &STREAMS,
            Self::AudioTick => &AUDIO_TICK,
        }
    }
}

#[inline]
pub fn record_tick_phase(phase: TickPhase, duration: Duration) {
    let elapsed_ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
    record_tick_phase_ns(phase, elapsed_ns);
}

#[inline]
pub fn record_tick_phase_ns(phase: TickPhase, elapsed_ns: u64) {
    phase.counter().record(elapsed_ns);
}

#[inline]
pub fn authored_frame_executed() {
    AUTHORED_FRAMES_EXECUTED.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn authored_catch_up_reset() {
    AUTHORED_CATCH_UP_RESETS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn avm2_broadcast_target_examined() {
    AVM2_BROADCAST_TARGETS_EXAMINED.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn avm2_broadcast_target_dispatched() {
    AVM2_BROADCAST_TARGETS_DISPATCHED.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn avm2_broadcast_stale_registration_pruned() {
    AVM2_BROADCAST_STALE_REGISTRATIONS_PRUNED.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn avm2_broadcast_handler_snapshot_spilled() {
    AVM2_BROADCAST_HANDLER_SNAPSHOT_SPILLS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn avm2_nested_goto_metrics_enabled() -> bool {
    AVM2_NESTED_GOTO_METRICS_ENABLED.load(Ordering::Relaxed)
}

pub fn set_avm2_nested_goto_metrics_enabled(enabled: bool) {
    AVM2_NESTED_GOTO_METRICS_ENABLED.store(false, Ordering::Relaxed);
    let _ = take_avm2_nested_goto_snapshot();
    AVM2_NESTED_GOTO_METRICS_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Count an explicit AVM2 goto and return whether its nested lifecycle should be profiled.
///
/// The classifier is lazy so a metrics-capable binary performs only one flag load when
/// metrics output is disabled.
#[inline]
pub fn begin_avm2_explicit_goto_profile(
    classify: impl FnOnce() -> Option<Avm2ExplicitGotoKind>,
) -> bool {
    if !avm2_nested_goto_metrics_enabled() {
        return false;
    }

    let Some(kind) = classify() else {
        return false;
    };
    let counter = match kind {
        Avm2ExplicitGotoKind::SameFrame => &AVM2_EXPLICIT_GOTO_SAME_FRAME,
        Avm2ExplicitGotoKind::ChangedFrame => &AVM2_EXPLICIT_GOTO_CHANGED_FRAME,
    };
    saturating_atomic_add(counter, 1);
    true
}

#[inline]
fn record_avm2_nested_goto_phase_ns(phase: Avm2NestedGotoPhase, elapsed_ns: u64) {
    let counter = match phase {
        Avm2NestedGotoPhase::Total => &AVM2_NESTED_GOTO_TOTAL,
        Avm2NestedGotoPhase::Construct => &AVM2_NESTED_GOTO_CONSTRUCT,
        Avm2NestedGotoPhase::FrameScripts => &AVM2_NESTED_GOTO_FRAME_SCRIPTS,
        Avm2NestedGotoPhase::ExitBroadcast => &AVM2_NESTED_GOTO_EXIT_BROADCAST,
    };
    counter.record(elapsed_ns);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NestedGotoLifecycleToken {
    depth: usize,
    is_root: bool,
}

#[derive(Clone, Copy, Debug)]
struct ActiveNestedGotoPhase {
    owner_depth: usize,
    phase: Avm2NestedGotoPhase,
    started_ns: u64,
}

#[derive(Debug)]
struct NestedGotoTimingFrame {
    started_ns: u64,
    suspended_parent_phase: Option<ActiveNestedGotoPhase>,
    phase_ns: [u64; 3],
    phase_seen: [bool; 3],
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CompletedNestedGotoTiming {
    total_ns: Option<u64>,
    construct: Option<u64>,
    frame_scripts: Option<u64>,
    exit_broadcast: Option<u64>,
}

#[derive(Debug, Default)]
struct NestedGotoTimingState {
    frames: Vec<NestedGotoTimingFrame>,
    active_phase: Option<ActiveNestedGotoPhase>,
}

impl NestedGotoTimingState {
    fn flush_active_phase(&mut self, now_ns: u64) -> Option<ActiveNestedGotoPhase> {
        let active = self.active_phase.take()?;
        let elapsed_ns = now_ns.saturating_sub(active.started_ns);
        let frame = &mut self.frames[active.owner_depth];
        let phase_ns = &mut frame.phase_ns[active.phase.timing_index()];
        *phase_ns = phase_ns.saturating_add(elapsed_ns);
        Some(active)
    }

    fn begin_lifecycle(&mut self, now_ns: u64) -> NestedGotoLifecycleToken {
        let suspended_parent_phase = self.flush_active_phase(now_ns);
        let depth = self.frames.len();
        self.frames.push(NestedGotoTimingFrame {
            started_ns: now_ns,
            suspended_parent_phase,
            phase_ns: [0; 3],
            phase_seen: [false; 3],
        });
        NestedGotoLifecycleToken {
            depth,
            is_root: depth == 0,
        }
    }

    fn begin_phase(&mut self, owner_depth: usize, phase: Avm2NestedGotoPhase, now_ns: u64) {
        debug_assert_eq!(owner_depth + 1, self.frames.len());
        debug_assert!(self.active_phase.is_none());
        self.frames[owner_depth].phase_seen[phase.timing_index()] = true;
        self.active_phase = Some(ActiveNestedGotoPhase {
            owner_depth,
            phase,
            started_ns: now_ns,
        });
    }

    fn end_phase(&mut self, owner_depth: usize, phase: Avm2NestedGotoPhase, now_ns: u64) {
        let active = self
            .flush_active_phase(now_ns)
            .expect("nested-goto phase ended without an active phase");
        debug_assert_eq!(active.owner_depth, owner_depth);
        debug_assert_eq!(active.phase, phase);
    }

    fn end_lifecycle(&mut self, owner_depth: usize, now_ns: u64) -> CompletedNestedGotoTiming {
        debug_assert_eq!(owner_depth + 1, self.frames.len());
        if self
            .active_phase
            .is_some_and(|active| active.owner_depth == owner_depth)
        {
            let _ = self.flush_active_phase(now_ns);
        }

        let frame = self
            .frames
            .pop()
            .expect("nested-goto lifecycle ended without an active frame");
        self.active_phase = frame
            .suspended_parent_phase
            .map(|suspended| ActiveNestedGotoPhase {
                started_ns: now_ns,
                ..suspended
            });

        let phase = |index: usize| frame.phase_seen[index].then_some(frame.phase_ns[index]);
        CompletedNestedGotoTiming {
            total_ns: (owner_depth == 0).then_some(now_ns.saturating_sub(frame.started_ns)),
            construct: phase(0),
            frame_scripts: phase(1),
            exit_broadcast: phase(2),
        }
    }

    #[cfg(test)]
    fn is_idle(&self) -> bool {
        self.frames.is_empty() && self.active_phase.is_none()
    }
}

struct NestedGotoRuntimeTiming {
    origin: Instant,
    state: NestedGotoTimingState,
}

impl NestedGotoRuntimeTiming {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            state: NestedGotoTimingState::default(),
        }
    }

    fn now_ns(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

thread_local! {
    static AVM2_NESTED_GOTO_TIMING: RefCell<NestedGotoRuntimeTiming> =
        RefCell::new(NestedGotoRuntimeTiming::new());
}

#[must_use]
pub struct Avm2NestedGotoProfileScope {
    token: NestedGotoLifecycleToken,
}

impl Avm2NestedGotoProfileScope {
    pub fn begin_phase(&self, phase: Avm2NestedGotoPhase) -> Avm2NestedGotoPhaseScope {
        AVM2_NESTED_GOTO_TIMING.with(|runtime| {
            let mut runtime = runtime.borrow_mut();
            let now_ns = runtime.now_ns();
            runtime.state.begin_phase(self.token.depth, phase, now_ns);
        });
        Avm2NestedGotoPhaseScope {
            owner_depth: self.token.depth,
            phase,
        }
    }
}

impl Drop for Avm2NestedGotoProfileScope {
    fn drop(&mut self) {
        let completed = AVM2_NESTED_GOTO_TIMING.with(|runtime| {
            let mut runtime = runtime.borrow_mut();
            let now_ns = runtime.now_ns();
            runtime.state.end_lifecycle(self.token.depth, now_ns)
        });
        debug_assert_eq!(self.token.is_root, completed.total_ns.is_some());

        if !avm2_nested_goto_metrics_enabled() {
            return;
        }
        if let Some(elapsed_ns) = completed.total_ns {
            record_avm2_nested_goto_phase_ns(Avm2NestedGotoPhase::Total, elapsed_ns);
        }
        for (phase, elapsed_ns) in [
            (Avm2NestedGotoPhase::Construct, completed.construct),
            (Avm2NestedGotoPhase::FrameScripts, completed.frame_scripts),
            (Avm2NestedGotoPhase::ExitBroadcast, completed.exit_broadcast),
        ] {
            if let Some(elapsed_ns) = elapsed_ns {
                record_avm2_nested_goto_phase_ns(phase, elapsed_ns);
            }
        }
    }
}

#[must_use]
pub struct Avm2NestedGotoPhaseScope {
    owner_depth: usize,
    phase: Avm2NestedGotoPhase,
}

impl Drop for Avm2NestedGotoPhaseScope {
    fn drop(&mut self) {
        AVM2_NESTED_GOTO_TIMING.with(|runtime| {
            let mut runtime = runtime.borrow_mut();
            let now_ns = runtime.now_ns();
            runtime
                .state
                .end_phase(self.owner_depth, self.phase, now_ns);
        });
    }
}

#[inline]
pub fn begin_avm2_nested_goto_profile(
    enabled_for_goto: bool,
) -> Option<Avm2NestedGotoProfileScope> {
    if !enabled_for_goto {
        return None;
    }
    let token = AVM2_NESTED_GOTO_TIMING.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        let now_ns = runtime.now_ns();
        runtime.state.begin_lifecycle(now_ns)
    });
    Some(Avm2NestedGotoProfileScope { token })
}

#[inline]
pub fn record_mouse_move(duration: Duration) {
    MOUSE_MOVES.record(u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX));
}

#[inline]
pub fn record_mouse_pick(duration: Duration) {
    MOUSE_PICKS.record(u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX));
}

pub fn take_input_snapshot() -> InputSnapshot {
    InputSnapshot {
        mouse_moves: MOUSE_MOVES.take(),
        mouse_picks: MOUSE_PICKS.take(),
    }
}

pub fn take_tick_snapshot() -> TickSnapshot {
    TickSnapshot {
        authored_frames_executed: AUTHORED_FRAMES_EXECUTED.swap(0, Ordering::Relaxed),
        authored_catch_up_resets: AUTHORED_CATCH_UP_RESETS.swap(0, Ordering::Relaxed),
        avm2_broadcast: Avm2BroadcastSnapshot {
            targets_examined: AVM2_BROADCAST_TARGETS_EXAMINED.swap(0, Ordering::Relaxed),
            targets_dispatched: AVM2_BROADCAST_TARGETS_DISPATCHED.swap(0, Ordering::Relaxed),
            stale_registrations_pruned: AVM2_BROADCAST_STALE_REGISTRATIONS_PRUNED
                .swap(0, Ordering::Relaxed),
            handler_snapshot_spills: AVM2_BROADCAST_HANDLER_SNAPSHOT_SPILLS
                .swap(0, Ordering::Relaxed),
        },
        avm2_nested_goto: take_avm2_nested_goto_snapshot(),
        preload: PRELOAD.take(),
        avm2_enter: AVM2_ENTER.take(),
        avm2_enter_orphans: AVM2_ENTER_ORPHANS.take(),
        avm2_enter_stage: AVM2_ENTER_STAGE.take(),
        avm2_enter_broadcast: AVM2_ENTER_BROADCAST.take(),
        avm2_construct: AVM2_CONSTRUCT.take(),
        avm2_frame_scripts: AVM2_FRAME_SCRIPTS.take(),
        avm2_exit: AVM2_EXIT.take(),
        avm2_post: AVM2_POST.take(),
        avm1_frame: AVM1_FRAME.take(),
        sound_update: SOUND_UPDATE.take(),
        local_connections: LOCAL_CONNECTIONS.take(),
        post_frame_callbacks: POST_FRAME_CALLBACKS.take(),
        audio_skew: AUDIO_SKEW.take(),
        sockets: SOCKETS.take(),
        net_connections: NET_CONNECTIONS.take(),
        timers: TIMERS.take(),
        streams: STREAMS.take(),
        audio_tick: AUDIO_TICK.take(),
    }
}

fn take_avm2_nested_goto_snapshot() -> Avm2NestedGotoSnapshot {
    Avm2NestedGotoSnapshot {
        same_frame: AVM2_EXPLICIT_GOTO_SAME_FRAME.swap(0, Ordering::Relaxed),
        changed_frame: AVM2_EXPLICIT_GOTO_CHANGED_FRAME.swap(0, Ordering::Relaxed),
        total: AVM2_NESTED_GOTO_TOTAL.take(),
        construct: AVM2_NESTED_GOTO_CONSTRUCT.take(),
        frame_scripts: AVM2_NESTED_GOTO_FRAME_SCRIPTS.take(),
        exit_broadcast: AVM2_NESTED_GOTO_EXIT_BROADCAST.take(),
    }
}

static DISPLAY_OBJECTS_ENTERED: AtomicU64 = AtomicU64::new(0);
static DISPLAY_OBJECTS_MASK_SKIPPED: AtomicU64 = AtomicU64::new(0);
static CONTAINER_CHILDREN_VISITED: AtomicU64 = AtomicU64::new(0);
static CONTAINER_CHILDREN_INVISIBLE: AtomicU64 = AtomicU64::new(0);
static CONTAINER_MASKS_RENDERED: AtomicU64 = AtomicU64::new(0);
static BITMAP_CACHE_CHECKS: AtomicU64 = AtomicU64::new(0);
static BITMAP_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static BITMAP_CACHE_FAST_HITS: AtomicU64 = AtomicU64::new(0);
static BITMAP_CACHE_FULL_EVALUATIONS: AtomicU64 = AtomicU64::new(0);
static BITMAP_CACHE_REBUILDS: AtomicU64 = AtomicU64::new(0);
static BITMAP_CACHE_OVERSIZE: AtomicU64 = AtomicU64::new(0);
static OFFSCREEN_CACHE_DRAWS: AtomicU64 = AtomicU64::new(0);
static BITMAP_CACHE_TEXTURE_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BITMAP_CACHE_TEXTURE_ALLOCATED_BYTES_ESTIMATE: AtomicU64 = AtomicU64::new(0);
static BITMAP_CACHE_TEXTURE_REUSES: AtomicU64 = AtomicU64::new(0);
static BITMAP_CACHE_TEXTURE_RESIZE_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default)]
pub struct RenderSnapshot {
    pub display_objects_entered: u64,
    pub display_objects_mask_skipped: u64,
    pub container_children_visited: u64,
    pub container_children_invisible: u64,
    pub container_masks_rendered: u64,
    pub bitmap_cache_checks: u64,
    pub bitmap_cache_hits: u64,
    pub bitmap_cache_fast_hits: u64,
    pub bitmap_cache_full_evaluations: u64,
    pub bitmap_cache_rebuilds: u64,
    pub bitmap_cache_oversize: u64,
    pub offscreen_cache_draws: u64,
    pub bitmap_cache_texture_allocations: u64,
    pub bitmap_cache_texture_allocated_bytes_estimate: u64,
    pub bitmap_cache_texture_reuses: u64,
    pub bitmap_cache_texture_resize_allocations: u64,
}

#[inline]
pub fn display_object_entered() {
    DISPLAY_OBJECTS_ENTERED.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn display_object_mask_skipped() {
    DISPLAY_OBJECTS_MASK_SKIPPED.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn container_child_visited() {
    CONTAINER_CHILDREN_VISITED.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn container_child_invisible() {
    CONTAINER_CHILDREN_INVISIBLE.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn container_mask_rendered() {
    CONTAINER_MASKS_RENDERED.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn bitmap_cache_check() {
    BITMAP_CACHE_CHECKS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn bitmap_cache_hit() {
    BITMAP_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn bitmap_cache_fast_hit() {
    BITMAP_CACHE_FAST_HITS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn bitmap_cache_full_evaluation() {
    BITMAP_CACHE_FULL_EVALUATIONS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn bitmap_cache_rebuild() {
    BITMAP_CACHE_REBUILDS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn bitmap_cache_oversize() {
    BITMAP_CACHE_OVERSIZE.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn offscreen_cache_draw() {
    OFFSCREEN_CACHE_DRAWS.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn bitmap_cache_texture_reused() {
    BITMAP_CACHE_TEXTURE_REUSES.fetch_add(1, Ordering::Relaxed);
}

#[inline]
pub fn bitmap_cache_texture_allocated(width: u32, height: u32, resized: bool) {
    BITMAP_CACHE_TEXTURE_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    let bytes = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4);
    saturating_atomic_add(&BITMAP_CACHE_TEXTURE_ALLOCATED_BYTES_ESTIMATE, bytes);
    if resized {
        BITMAP_CACHE_TEXTURE_RESIZE_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Return the counters accumulated since the previous call and reset them to zero.
///
/// The frontend should call this once after each rendered movie frame.
pub fn take_render_snapshot() -> RenderSnapshot {
    let snapshot = RenderSnapshot {
        display_objects_entered: DISPLAY_OBJECTS_ENTERED.swap(0, Ordering::Relaxed),
        display_objects_mask_skipped: DISPLAY_OBJECTS_MASK_SKIPPED.swap(0, Ordering::Relaxed),
        container_children_visited: CONTAINER_CHILDREN_VISITED.swap(0, Ordering::Relaxed),
        container_children_invisible: CONTAINER_CHILDREN_INVISIBLE.swap(0, Ordering::Relaxed),
        container_masks_rendered: CONTAINER_MASKS_RENDERED.swap(0, Ordering::Relaxed),
        bitmap_cache_checks: BITMAP_CACHE_CHECKS.swap(0, Ordering::Relaxed),
        bitmap_cache_hits: BITMAP_CACHE_HITS.swap(0, Ordering::Relaxed),
        bitmap_cache_fast_hits: BITMAP_CACHE_FAST_HITS.swap(0, Ordering::Relaxed),
        bitmap_cache_full_evaluations: BITMAP_CACHE_FULL_EVALUATIONS.swap(0, Ordering::Relaxed),
        bitmap_cache_rebuilds: BITMAP_CACHE_REBUILDS.swap(0, Ordering::Relaxed),
        bitmap_cache_oversize: BITMAP_CACHE_OVERSIZE.swap(0, Ordering::Relaxed),
        offscreen_cache_draws: OFFSCREEN_CACHE_DRAWS.swap(0, Ordering::Relaxed),
        bitmap_cache_texture_allocations: BITMAP_CACHE_TEXTURE_ALLOCATIONS
            .swap(0, Ordering::Relaxed),
        bitmap_cache_texture_allocated_bytes_estimate:
            BITMAP_CACHE_TEXTURE_ALLOCATED_BYTES_ESTIMATE.swap(0, Ordering::Relaxed),
        bitmap_cache_texture_reuses: BITMAP_CACHE_TEXTURE_REUSES.swap(0, Ordering::Relaxed),
        bitmap_cache_texture_resize_allocations: BITMAP_CACHE_TEXTURE_RESIZE_ALLOCATIONS
            .swap(0, Ordering::Relaxed),
    };
    crate::aether_diagnostics::advance_render_frame();
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static METRICS_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_metrics_test() -> MutexGuard<'static, ()> {
        METRICS_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn tick_phase_snapshot_accumulates_and_resets() {
        let _guard = lock_metrics_test();
        let _ = take_tick_snapshot();

        record_tick_phase_ns(TickPhase::Avm2Enter, 1_500);
        record_tick_phase_ns(TickPhase::Avm2Enter, 2_500);
        record_tick_phase_ns(TickPhase::Avm2EnterOrphans, 500);
        record_tick_phase_ns(TickPhase::Avm2EnterStage, 2_000);
        record_tick_phase_ns(TickPhase::Avm2EnterBroadcast, 750);
        record_tick_phase_ns(TickPhase::Timers, 900);

        let snapshot = take_tick_snapshot();
        assert_eq!(snapshot.avm2_enter.total_ns, 4_000);
        assert_eq!(snapshot.avm2_enter.invocations, 2);
        assert_eq!(snapshot.avm2_enter_orphans.total_ns, 500);
        assert_eq!(snapshot.avm2_enter_orphans.invocations, 1);
        assert_eq!(snapshot.avm2_enter_stage.total_ns, 2_000);
        assert_eq!(snapshot.avm2_enter_stage.invocations, 1);
        assert_eq!(snapshot.avm2_enter_broadcast.total_ns, 750);
        assert_eq!(snapshot.avm2_enter_broadcast.invocations, 1);
        assert_eq!(snapshot.timers.total_ns, 900);
        assert_eq!(snapshot.timers.invocations, 1);

        assert_eq!(take_tick_snapshot(), TickSnapshot::default());
    }

    #[test]
    fn avm2_enter_attribution_variants_are_publicly_recordable() {
        let _guard = lock_metrics_test();
        let _ = take_tick_snapshot();
        for phase in [
            TickPhase::Avm2EnterOrphans,
            TickPhase::Avm2EnterStage,
            TickPhase::Avm2EnterBroadcast,
        ] {
            record_tick_phase_ns(phase, 1);
        }
        let snapshot = take_tick_snapshot();
        assert_eq!(snapshot.avm2_enter_orphans.total_ns, 1);
        assert_eq!(snapshot.avm2_enter_stage.total_ns, 1);
        assert_eq!(snapshot.avm2_enter_broadcast.total_ns, 1);
    }

    #[test]
    fn avm2_nested_goto_snapshot_accumulates_counts_timings_and_resets() {
        let _guard = lock_metrics_test();
        set_avm2_nested_goto_metrics_enabled(true);
        let _ = take_tick_snapshot();

        assert!(begin_avm2_explicit_goto_profile(|| Some(
            Avm2ExplicitGotoKind::SameFrame
        )));
        assert!(begin_avm2_explicit_goto_profile(|| Some(
            Avm2ExplicitGotoKind::SameFrame
        )));
        assert!(begin_avm2_explicit_goto_profile(|| Some(
            Avm2ExplicitGotoKind::ChangedFrame
        )));
        record_avm2_nested_goto_phase_ns(Avm2NestedGotoPhase::Total, 9_000);
        record_avm2_nested_goto_phase_ns(Avm2NestedGotoPhase::Construct, 2_000);
        record_avm2_nested_goto_phase_ns(Avm2NestedGotoPhase::FrameScripts, 3_000);
        record_avm2_nested_goto_phase_ns(Avm2NestedGotoPhase::ExitBroadcast, 1_000);

        let snapshot = take_tick_snapshot().avm2_nested_goto;
        assert_eq!(snapshot.same_frame, 2);
        assert_eq!(snapshot.changed_frame, 1);
        assert_eq!(
            snapshot.total,
            TimingSnapshot {
                total_ns: 9_000,
                invocations: 1
            }
        );
        assert_eq!(
            snapshot.construct,
            TimingSnapshot {
                total_ns: 2_000,
                invocations: 1
            }
        );
        assert_eq!(
            snapshot.frame_scripts,
            TimingSnapshot {
                total_ns: 3_000,
                invocations: 1
            }
        );
        assert_eq!(
            snapshot.exit_broadcast,
            TimingSnapshot {
                total_ns: 1_000,
                invocations: 1
            }
        );

        assert_eq!(
            take_tick_snapshot().avm2_nested_goto,
            Avm2NestedGotoSnapshot::default()
        );
        set_avm2_nested_goto_metrics_enabled(false);
    }

    #[test]
    fn avm2_nested_goto_runtime_gate_records_only_while_enabled_and_clears_transitions() {
        let _guard = lock_metrics_test();
        set_avm2_nested_goto_metrics_enabled(false);

        assert!(!avm2_nested_goto_metrics_enabled());
        let classifier_called = std::cell::Cell::new(false);
        assert!(!begin_avm2_explicit_goto_profile(|| {
            classifier_called.set(true);
            Some(Avm2ExplicitGotoKind::SameFrame)
        }));
        assert!(!classifier_called.get());
        assert!(begin_avm2_nested_goto_profile(false).is_none());
        assert_eq!(
            take_tick_snapshot().avm2_nested_goto,
            Avm2NestedGotoSnapshot::default()
        );

        set_avm2_nested_goto_metrics_enabled(true);
        assert!(avm2_nested_goto_metrics_enabled());
        assert!(begin_avm2_explicit_goto_profile(|| Some(
            Avm2ExplicitGotoKind::SameFrame
        )));
        assert!(begin_avm2_explicit_goto_profile(|| Some(
            Avm2ExplicitGotoKind::ChangedFrame
        )));
        let enabled = take_tick_snapshot().avm2_nested_goto;
        assert_eq!(enabled.same_frame, 1);
        assert_eq!(enabled.changed_frame, 1);

        assert!(begin_avm2_explicit_goto_profile(|| Some(
            Avm2ExplicitGotoKind::ChangedFrame
        )));
        set_avm2_nested_goto_metrics_enabled(false);
        assert_eq!(
            take_tick_snapshot().avm2_nested_goto,
            Avm2NestedGotoSnapshot::default()
        );
        assert!(!begin_avm2_explicit_goto_profile(|| Some(
            Avm2ExplicitGotoKind::SameFrame
        )));
    }

    #[test]
    fn nested_goto_runtime_scopes_count_all_gotos_but_time_one_root_chain() {
        let _guard = lock_metrics_test();
        set_avm2_nested_goto_metrics_enabled(true);

        let outer_enabled =
            begin_avm2_explicit_goto_profile(|| Some(Avm2ExplicitGotoKind::ChangedFrame));
        let outer = begin_avm2_nested_goto_profile(outer_enabled).unwrap();
        {
            let _outer_scripts = outer.begin_phase(Avm2NestedGotoPhase::FrameScripts);
            let inner_enabled =
                begin_avm2_explicit_goto_profile(|| Some(Avm2ExplicitGotoKind::SameFrame));
            let inner = begin_avm2_nested_goto_profile(inner_enabled).unwrap();
            {
                let _inner_construct = inner.begin_phase(Avm2NestedGotoPhase::Construct);
                std::hint::black_box(());
            }
            {
                let _inner_exit = inner.begin_phase(Avm2NestedGotoPhase::ExitBroadcast);
                std::hint::black_box(());
            }
        }
        drop(outer);

        let snapshot = take_tick_snapshot().avm2_nested_goto;
        assert_eq!(snapshot.same_frame, 1);
        assert_eq!(snapshot.changed_frame, 1);
        assert_eq!(snapshot.total.invocations, 1);
        assert_eq!(snapshot.construct.invocations, 1);
        assert_eq!(snapshot.frame_scripts.invocations, 1);
        assert_eq!(snapshot.exit_broadcast.invocations, 1);
        assert!(
            snapshot.construct.total_ns
                + snapshot.frame_scripts.total_ns
                + snapshot.exit_broadcast.total_ns
                <= snapshot.total.total_ns
        );
        set_avm2_nested_goto_metrics_enabled(false);
    }

    #[test]
    fn nested_goto_timing_state_keeps_root_total_and_exclusive_recursive_phases() {
        let mut state = NestedGotoTimingState::default();

        let outer = state.begin_lifecycle(0);
        assert!(outer.is_root);
        state.begin_phase(outer.depth, Avm2NestedGotoPhase::Construct, 10);
        state.end_phase(outer.depth, Avm2NestedGotoPhase::Construct, 20);
        state.begin_phase(outer.depth, Avm2NestedGotoPhase::FrameScripts, 20);

        let inner = state.begin_lifecycle(30);
        assert!(!inner.is_root);
        state.begin_phase(inner.depth, Avm2NestedGotoPhase::Construct, 30);
        state.end_phase(inner.depth, Avm2NestedGotoPhase::Construct, 40);
        state.begin_phase(inner.depth, Avm2NestedGotoPhase::FrameScripts, 40);
        state.end_phase(inner.depth, Avm2NestedGotoPhase::FrameScripts, 50);
        state.begin_phase(inner.depth, Avm2NestedGotoPhase::ExitBroadcast, 50);
        state.end_phase(inner.depth, Avm2NestedGotoPhase::ExitBroadcast, 60);
        let inner_completed = state.end_lifecycle(inner.depth, 65);

        assert_eq!(inner_completed.total_ns, None);
        assert_eq!(inner_completed.construct, Some(10));
        assert_eq!(inner_completed.frame_scripts, Some(10));
        assert_eq!(inner_completed.exit_broadcast, Some(10));

        state.end_phase(outer.depth, Avm2NestedGotoPhase::FrameScripts, 75);
        state.begin_phase(outer.depth, Avm2NestedGotoPhase::ExitBroadcast, 75);
        state.end_phase(outer.depth, Avm2NestedGotoPhase::ExitBroadcast, 85);
        let outer_completed = state.end_lifecycle(outer.depth, 100);

        assert_eq!(outer_completed.total_ns, Some(100));
        assert_eq!(outer_completed.construct, Some(10));
        assert_eq!(outer_completed.frame_scripts, Some(20));
        assert_eq!(outer_completed.exit_broadcast, Some(10));
        assert!(state.is_idle());

        let aggregate_construct =
            inner_completed.construct.unwrap() + outer_completed.construct.unwrap();
        let aggregate_frame_scripts =
            inner_completed.frame_scripts.unwrap() + outer_completed.frame_scripts.unwrap();
        let aggregate_exit =
            inner_completed.exit_broadcast.unwrap() + outer_completed.exit_broadcast.unwrap();
        assert_eq!(aggregate_construct, 20);
        assert_eq!(aggregate_frame_scripts, 30);
        assert_eq!(aggregate_exit, 20);
        assert!(aggregate_construct + aggregate_frame_scripts + aggregate_exit <= 100);
    }

    #[test]
    fn authored_cadence_counters_accumulate_and_reset_with_tick_snapshot() {
        let _guard = lock_metrics_test();
        let _ = take_tick_snapshot();

        authored_frame_executed();
        authored_frame_executed();
        authored_catch_up_reset();

        let snapshot = take_tick_snapshot();
        assert_eq!(snapshot.authored_frames_executed, 2);
        assert_eq!(snapshot.authored_catch_up_resets, 1);

        let reset = take_tick_snapshot();
        assert_eq!(reset.authored_frames_executed, 0);
        assert_eq!(reset.authored_catch_up_resets, 0);
    }

    #[test]
    fn avm2_broadcast_counters_accumulate_and_reset_with_tick_snapshot() {
        let _guard = lock_metrics_test();
        let _ = take_tick_snapshot();

        avm2_broadcast_target_examined();
        avm2_broadcast_target_examined();
        avm2_broadcast_target_dispatched();
        avm2_broadcast_stale_registration_pruned();
        avm2_broadcast_handler_snapshot_spilled();

        let snapshot = take_tick_snapshot().avm2_broadcast;
        assert_eq!(snapshot.targets_examined, 2);
        assert_eq!(snapshot.targets_dispatched, 1);
        assert_eq!(snapshot.stale_registrations_pruned, 1);
        assert_eq!(snapshot.handler_snapshot_spills, 1);
        assert_eq!(
            take_tick_snapshot().avm2_broadcast,
            Avm2BroadcastSnapshot::default()
        );
    }

    #[test]
    fn input_snapshot_accumulates_timings_and_resets() {
        let _guard = lock_metrics_test();
        let _ = take_input_snapshot();

        record_mouse_move(Duration::from_micros(250));
        record_mouse_move(Duration::from_micros(750));
        record_mouse_pick(Duration::from_micros(100));

        let snapshot = take_input_snapshot();
        assert_eq!(snapshot.mouse_moves.total_ns, 1_000_000);
        assert_eq!(snapshot.mouse_moves.invocations, 2);
        assert_eq!(snapshot.mouse_picks.total_ns, 100_000);
        assert_eq!(snapshot.mouse_picks.invocations, 1);
        assert_eq!(take_input_snapshot(), InputSnapshot::default());
    }

    #[test]
    fn bitmap_cache_texture_snapshot_classifies_reuse_and_resize() {
        let _guard = lock_metrics_test();
        let _ = take_render_snapshot();

        bitmap_cache_texture_reused();
        bitmap_cache_texture_allocated(10, 20, false);
        bitmap_cache_texture_allocated(4, 8, true);

        let snapshot = take_render_snapshot();
        assert_eq!(snapshot.bitmap_cache_texture_reuses, 1);
        assert_eq!(snapshot.bitmap_cache_texture_allocations, 2);
        assert_eq!(snapshot.bitmap_cache_texture_resize_allocations, 1);
        assert_eq!(snapshot.bitmap_cache_texture_allocated_bytes_estimate, 928);
    }
}
