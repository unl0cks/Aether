//! Aether-only diagnostic instrumentation.
//!
//! This module is compiled only for Aether's diagnostic/metrics build. It deliberately stores
//! owned strings and aggregate statistics so no GC-managed Flash values escape a frame.

use crate::aether_compatibility::TimelineChildRebindSummary;
pub use crate::aether_compatibility::set_timeline_child_rebind_enabled;
use crate::avm2::globals::slots::flash_events_event_dispatcher as event_dispatcher_slots;
use crate::avm2::{TObject as _, Value as Avm2Value};
use crate::context::UpdateContext;
use crate::display_object::{
    BoundsMode, DisplayObject, MovieClip, TDisplayObject, TDisplayObjectContainer,
    TInteractiveObject,
};
use crate::events::ClipEvent;
use crate::frame_lifecycle::FramePhase;
use crate::string::AvmString;
use either::Either;
use fnv::FnvHashMap;
use ruffle_render::filters::Filter;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_CACHE_OFFENDERS: usize = 4_096;
const STALE_CACHE_FRAMES: u64 = 24 * 60 * 5;
const INPUT_FLUSH_INTERVAL: Duration = Duration::from_millis(250);
const INPUT_FLUSH_EVENT_COUNT: usize = 32;
const AQW_MOVEMENT_NO_PROGRESS_DELAY_MS: f64 = 50.0;
// Coalesced mouse dispatch runs on the rendered frame. On a heavily loaded map the callback
// observed one frame later can therefore arrive 67-100 ms after the MouseMove instead of the
// 14-16 ms seen at 60 FPS. Keep the window bounded, but wide enough for two 10 FPS frames.
const AQW_DELAYED_MOUSE_STOP_GRACE: Duration = Duration::from_millis(250);
const MOVEMENT_PROGRESS_EPSILON_PX: f64 = 0.25;
const MAX_MOVEMENT_CALLBACKS_WITHOUT_PROGRESS: u32 = 120;
const MAX_MOVEMENT_COLLISIONS_PER_SEQUENCE: usize = 128;

static CACHE_OFFENDER_ENABLED: AtomicBool = AtomicBool::new(false);
static ENTER_FRAME_HANDLER_ATTRIBUTION_ENABLED: AtomicBool = AtomicBool::new(false);
static INPUT_TRACE_ENABLED: AtomicBool = AtomicBool::new(false);
static TIMELINE_TRACE_ENABLED: AtomicBool = AtomicBool::new(false);
static MOVEMENT_STOP_GUARD_ENABLED: AtomicBool = AtomicBool::new(false);
static RENDER_FRAME_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct EnterFrameHandlerDescriptor {
    pub qualified_name: String,
    pub movie_url: String,
    pub abc_method_index: u32,
    pub kind: &'static str,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct EnterFrameHandlerSummary {
    #[serde(flatten)]
    pub descriptor: EnterFrameHandlerDescriptor,
    pub total_us: u64,
    pub invocations: u64,
    pub max_us: u64,
}

impl std::ops::Deref for EnterFrameHandlerSummary {
    type Target = EnterFrameHandlerDescriptor;

    fn deref(&self) -> &Self::Target {
        &self.descriptor
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct EnterFrameHandlerReport {
    pub total_us: u64,
    pub invocations: u64,
    pub unique_methods: usize,
    pub top: Vec<EnterFrameHandlerSummary>,
    pub tail_us: u64,
}

#[derive(Debug)]
struct EnterFrameHandlerRecord {
    descriptor: EnterFrameHandlerDescriptor,
    total_ns: u64,
    invocations: u64,
    max_ns: u64,
}

#[derive(Default)]
struct EnterFrameHandlerAttributionState {
    methods: FnvHashMap<usize, EnterFrameHandlerRecord>,
    total_ns: u64,
    invocations: u64,
}

thread_local! {
    static ENTER_FRAME_HANDLER_ATTRIBUTION: RefCell<EnterFrameHandlerAttributionState> =
        RefCell::new(EnterFrameHandlerAttributionState::default());
}

impl EnterFrameHandlerAttributionState {
    /// Reached from [`record_enter_frame_handler`], which only the metrics build calls.
    #[cfg_attr(not(feature = "aether_metrics"), allow(dead_code))]
    fn record_with(
        &mut self,
        method_key: usize,
        elapsed_ns: u64,
        descriptor: impl FnOnce() -> EnterFrameHandlerDescriptor,
    ) {
        self.total_ns = self.total_ns.saturating_add(elapsed_ns);
        self.invocations = self.invocations.saturating_add(1);

        match self.methods.entry(method_key) {
            Entry::Occupied(mut entry) => {
                let record = entry.get_mut();
                record.total_ns = record.total_ns.saturating_add(elapsed_ns);
                record.invocations = record.invocations.saturating_add(1);
                record.max_ns = record.max_ns.max(elapsed_ns);
            }
            Entry::Vacant(entry) => {
                entry.insert(EnterFrameHandlerRecord {
                    descriptor: descriptor(),
                    total_ns: elapsed_ns,
                    invocations: 1,
                    max_ns: elapsed_ns,
                });
            }
        }
    }

    fn take_report(&mut self, limit: usize) -> EnterFrameHandlerReport {
        let total_ns = std::mem::take(&mut self.total_ns);
        let invocations = std::mem::take(&mut self.invocations);
        let methods = std::mem::take(&mut self.methods);
        let unique_methods = methods.len();
        let mut records: Vec<_> = methods.into_iter().collect();
        records.sort_by(|(left_key, left), (right_key, right)| {
            right
                .total_ns
                .cmp(&left.total_ns)
                .then_with(|| {
                    left.descriptor
                        .qualified_name
                        .cmp(&right.descriptor.qualified_name)
                })
                .then_with(|| left.descriptor.movie_url.cmp(&right.descriptor.movie_url))
                .then_with(|| {
                    left.descriptor
                        .abc_method_index
                        .cmp(&right.descriptor.abc_method_index)
                })
                .then_with(|| left.descriptor.kind.cmp(right.descriptor.kind))
                .then_with(|| left_key.cmp(right_key))
        });
        records.truncate(limit);
        let top_ns = records.iter().fold(0_u64, |total, (_, record)| {
            total.saturating_add(record.total_ns)
        });
        let tail_ns = total_ns.saturating_sub(top_ns);
        let component_ns: Vec<_> = records
            .iter()
            .map(|(_, record)| record.total_ns)
            .chain(std::iter::once(tail_ns))
            .collect();
        let mut component_us = allocate_reconciled_microseconds(total_ns, &component_ns);
        let tail_us = component_us.pop().unwrap_or_default();
        let top = records
            .into_iter()
            .zip(component_us)
            .map(|((_, record), total_us)| EnterFrameHandlerSummary {
                descriptor: record.descriptor,
                total_us,
                invocations: record.invocations,
                max_us: record.max_ns / 1_000,
            })
            .collect();

        EnterFrameHandlerReport {
            total_us: total_ns / 1_000,
            invocations,
            unique_methods,
            top,
            tail_us,
        }
    }
}

/// Convert an exact nanosecond partition to whole microseconds without making the published
/// components disagree with the published total. Fractional microseconds are assigned by largest
/// remainder, with the already-deterministic component order breaking equal-remainder ties.
fn allocate_reconciled_microseconds(total_ns: u64, component_ns: &[u64]) -> Vec<u64> {
    let mut component_us: Vec<_> = component_ns.iter().map(|value| value / 1_000).collect();
    let assigned_us = component_us
        .iter()
        .fold(0_u64, |total, value| total.saturating_add(*value));
    let remaining_us = (total_ns / 1_000).saturating_sub(assigned_us);
    let mut remainder_order: Vec<_> = component_ns
        .iter()
        .enumerate()
        .map(|(index, value)| (index, value % 1_000))
        .collect();
    remainder_order.sort_unstable_by(|(left_index, left), (right_index, right)| {
        right.cmp(left).then_with(|| left_index.cmp(right_index))
    });

    for (index, _) in remainder_order
        .into_iter()
        .take(usize::try_from(remaining_us).unwrap_or(usize::MAX))
    {
        component_us[index] = component_us[index].saturating_add(1);
    }

    component_us
}

pub fn set_enter_frame_handler_attribution_enabled(enabled: bool) {
    ENTER_FRAME_HANDLER_ATTRIBUTION_ENABLED.store(enabled, Ordering::Relaxed);
    ENTER_FRAME_HANDLER_ATTRIBUTION.with(|state| {
        *state.borrow_mut() = EnterFrameHandlerAttributionState::default();
    });
}

#[inline]
pub fn enter_frame_handler_attribution_enabled() -> bool {
    ENTER_FRAME_HANDLER_ATTRIBUTION_ENABLED.load(Ordering::Relaxed)
}

/// Only the metrics build calls this, from the AVM2 broadcast dispatch. The tests below cover it
/// either way, so it stays compiled rather than being cfg'd out and losing that coverage.
#[cfg_attr(not(feature = "aether_metrics"), allow(dead_code))]
pub(crate) fn record_enter_frame_handler(
    method_key: usize,
    elapsed: Duration,
    descriptor: impl FnOnce() -> EnterFrameHandlerDescriptor,
) {
    let elapsed_ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
    ENTER_FRAME_HANDLER_ATTRIBUTION.with(|state| {
        state
            .borrow_mut()
            .record_with(method_key, elapsed_ns, descriptor);
    });
}

pub fn take_enter_frame_handler_report(limit: usize) -> EnterFrameHandlerReport {
    ENTER_FRAME_HANDLER_ATTRIBUTION.with(|state| state.borrow_mut().take_report(limit))
}

#[derive(Clone, Debug, Serialize)]
pub struct DisplayObjectDescriptor {
    object_type: &'static str,
    instance_name: Option<String>,
    class_name: Option<String>,
    display_path: String,
    movie_url: String,
    character_id: u16,
    depth: i32,
    action_script: &'static str,
    actor_classification: &'static str,
}

impl DisplayObjectDescriptor {
    pub fn from_display_object(object: DisplayObject<'_>) -> Self {
        let object_type = object_type_name(object);

        let instance_name = object.name().map(|name| name.to_string());
        let class_name = object.object2().map(|avm2_object| {
            match avm2_object
                .instance_class()
                .name()
                .to_qualified_name_no_mc()
            {
                Either::Left(name) => name.to_string(),
                Either::Right(name) => name.to_utf8_lossy().into_owned(),
            }
        });
        let display_path = build_display_path(object);
        let movie_url = object.movie().url().to_string();
        let action_script = if object.movie().is_action_script_3() {
            "as3"
        } else {
            "as2"
        };
        let actor_classification = classify_actor(
            instance_name.as_deref(),
            class_name.as_deref(),
            &display_path,
            &movie_url,
        );

        Self {
            object_type,
            instance_name,
            class_name,
            display_path,
            movie_url,
            character_id: object.id(),
            depth: object.depth(),
            action_script,
            actor_classification,
        }
    }
}

fn object_type_name(object: DisplayObject<'_>) -> &'static str {
    match object {
        DisplayObject::Stage(_) => "stage",
        DisplayObject::Bitmap(_) => "bitmap",
        DisplayObject::Avm1Button(_) => "avm1_button",
        DisplayObject::Avm2Button(_) => "avm2_button",
        DisplayObject::EditText(_) => "edit_text",
        DisplayObject::TextLine(_) => "text_line",
        DisplayObject::Graphic(_) => "graphic",
        DisplayObject::MorphShape(_) => "morph_shape",
        DisplayObject::MovieClip(_) => "movie_clip",
        DisplayObject::Text(_) => "text",
        DisplayObject::Video(_) => "video",
        DisplayObject::LoaderDisplay(_) => "loader_display",
    }
}

fn build_display_path(mut object: DisplayObject<'_>) -> String {
    let mut segments = Vec::with_capacity(8);
    for _ in 0..64 {
        let segment = object.name().map_or_else(
            || format!("{}#{}", object_type_name(object), object.depth()),
            |name| name.to_string(),
        );
        segments.push(segment);
        let Some(parent) = object.parent() else {
            break;
        };
        object = parent;
    }
    segments.reverse();
    segments.join("/")
}

fn classify_actor(
    instance_name: Option<&str>,
    class_name: Option<&str>,
    display_path: &str,
    movie_url: &str,
) -> &'static str {
    let haystack = format!(
        "{} {} {} {}",
        instance_name.unwrap_or_default(),
        class_name.unwrap_or_default(),
        display_path,
        movie_url
    )
    .to_ascii_lowercase();

    if haystack.contains("myavatar") || haystack.contains("my_avatar") {
        "local_player"
    } else if haystack.contains("weapon") || haystack.contains("wep") {
        "weapon"
    } else if haystack.contains("cape") || haystack.contains("backitem") {
        "cape"
    } else if haystack.contains("battlepet") || haystack.contains("pet/") {
        "pet"
    } else if haystack.contains("monster") || haystack.contains("enemy") {
        "monster"
    } else if haystack.contains("avatar") || haystack.contains("player") {
        "remote_player"
    } else if haystack.contains("interface")
        || haystack.contains("gamemenu")
        || haystack.contains("mcui")
        || haystack.contains("/ui")
    {
        "ui"
    } else if haystack.contains("map-") || haystack.contains("maps/") || haystack.contains("/map") {
        "map"
    } else {
        "unknown"
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct MouseRouteNode {
    descriptor: DisplayObjectDescriptor,
    interactive: bool,
    mouse_enabled: Option<bool>,
    mouse_children: Option<bool>,
    forced_button_mode: Option<bool>,
    has_click_listener: Option<bool>,
    has_mouse_down_listener: Option<bool>,
    has_mouse_up_listener: Option<bool>,
}

pub fn mouse_route_chain<'gc>(
    mut object: DisplayObject<'gc>,
    context: &mut UpdateContext<'gc>,
) -> Vec<MouseRouteNode> {
    let mut chain = Vec::with_capacity(8);
    for _ in 0..64 {
        let interactive = object.as_interactive();
        let (has_click_listener, has_mouse_down_listener, has_mouse_up_listener) =
            avm2_mouse_listener_flags(object, context);
        chain.push(MouseRouteNode {
            descriptor: DisplayObjectDescriptor::from_display_object(object),
            interactive: interactive.is_some(),
            mouse_enabled: interactive.map(|interactive| interactive.mouse_enabled()),
            mouse_children: object
                .as_container()
                .map(|container| container.raw_container().mouse_children()),
            forced_button_mode: object.as_movie_clip().map(|clip| clip.forced_button_mode()),
            has_click_listener,
            has_mouse_down_listener,
            has_mouse_up_listener,
        });

        let Some(parent) = object.parent() else {
            break;
        };
        object = parent;
    }
    chain
}

fn avm2_mouse_listener_flags<'gc>(
    object: DisplayObject<'gc>,
    context: &mut UpdateContext<'gc>,
) -> (Option<bool>, Option<bool>, Option<bool>) {
    let Some(avm2_object) = object.object2() else {
        return (None, None, None);
    };
    let Avm2Value::Object(dispatch_object) =
        avm2_object.get_slot(event_dispatcher_slots::DISPATCH_LIST)
    else {
        return (Some(false), Some(false), Some(false));
    };
    let click = AvmString::new_utf8(context.gc(), "click");
    let mouse_down = AvmString::new_utf8(context.gc(), "mouseDown");
    let mouse_up = AvmString::new_utf8(context.gc(), "mouseUp");

    let Some(dispatch_list) = dispatch_object.as_dispatch_mut(context.gc()) else {
        return (Some(false), Some(false), Some(false));
    };

    (
        Some(dispatch_list.has_event_listener(click)),
        Some(dispatch_list.has_event_listener(mouse_down)),
        Some(dispatch_list.has_event_listener(mouse_up)),
    )
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ConstructionSnapshot {
    pub direct_children: usize,
    pub total_descendants: usize,
    pub unconstructed_direct_children: usize,
    pub unconstructed_descendants: usize,
    pub manual_construct_descendants: usize,
    pub unconstructed_samples: Vec<DisplayObjectDescriptor>,
}

pub fn construction_snapshot(clip: MovieClip<'_>) -> ConstructionSnapshot {
    let mut snapshot = ConstructionSnapshot::default();
    let mut stack: Vec<(DisplayObject<'_>, bool)> =
        clip.iter_render_list().map(|child| (child, true)).collect();
    snapshot.direct_children = stack.len();

    while let Some((object, is_direct)) = stack.pop() {
        snapshot.total_descendants = snapshot.total_descendants.saturating_add(1);
        if object.manual_frame_construct() {
            snapshot.manual_construct_descendants =
                snapshot.manual_construct_descendants.saturating_add(1);
        }
        if object.object2().is_none() {
            snapshot.unconstructed_descendants =
                snapshot.unconstructed_descendants.saturating_add(1);
            if is_direct {
                snapshot.unconstructed_direct_children =
                    snapshot.unconstructed_direct_children.saturating_add(1);
            }
            if snapshot.unconstructed_samples.len() < 16 {
                snapshot
                    .unconstructed_samples
                    .push(DisplayObjectDescriptor::from_display_object(object));
            }
        }
        if let Some(container) = object.as_container() {
            stack.extend(container.iter_render_list().map(|child| (child, false)));
        }
        if snapshot.total_descendants >= 16_384 {
            break;
        }
    }

    snapshot
}

#[inline]
pub fn set_movement_stop_guard_enabled(enabled: bool) {
    MOVEMENT_STOP_GUARD_ENABLED.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn movement_stop_guard_enabled() -> bool {
    MOVEMENT_STOP_GUARD_ENABLED.load(Ordering::Relaxed)
}

#[inline]
pub fn movement_tracking_enabled() -> bool {
    input_trace_enabled() || movement_stop_guard_enabled()
}

pub fn filter_names(filters: &[Filter]) -> Vec<String> {
    filters
        .iter()
        .map(|filter| match filter {
            Filter::BevelFilter(_) => "bevel",
            Filter::BlurFilter(_) => "blur",
            Filter::ColorMatrixFilter(_) => "color_matrix",
            Filter::ConvolutionFilter(_) => "convolution",
            Filter::DisplacementMapFilter(_) => "displacement_map",
            Filter::DropShadowFilter(_) => "drop_shadow",
            Filter::GlowFilter(_) => "glow",
            Filter::GradientBevelFilter(_) => "gradient_bevel",
            Filter::GradientGlowFilter(_) => "gradient_glow",
            Filter::ShaderFilter(_) => "shader",
        })
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Debug)]
pub struct CacheRebuildObservation {
    pub key: u64,
    pub descriptor: DisplayObjectDescriptor,
    pub source_width: u32,
    pub source_height: u32,
    pub texture_width: u32,
    pub texture_height: u32,
    pub filter_names: Vec<String>,
    pub invalidation_reason: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct CacheOffenderSummary {
    object_key: u64,
    descriptor: DisplayObjectDescriptor,
    source_width: u32,
    source_height: u32,
    texture_width: u32,
    texture_height: u32,
    filters: Vec<String>,
    rebuild_count: u64,
    offscreen_draw_count: u64,
    total_offscreen_pixel_area: u64,
    total_filter_count: u64,
    cache_update_ms: f64,
    max_cache_update_ms: f64,
    offscreen_command_build_ms: f64,
    max_offscreen_command_build_ms: f64,
    total_rebuild_cpu_ms: f64,
    average_cache_lifetime_frames: Option<f64>,
    shortest_cache_lifetime_frames: Option<u64>,
    invalidation_reasons: BTreeMap<String, u64>,
}

#[derive(Debug)]
struct CacheOffenderRecord {
    object_key: u64,
    descriptor: DisplayObjectDescriptor,
    source_width: u32,
    source_height: u32,
    texture_width: u32,
    texture_height: u32,
    filters: Vec<String>,
    last_rebuild_frame: Option<u64>,
    lifetime_frames_total: u64,
    lifetime_samples: u64,
    shortest_lifetime_frames: Option<u64>,
    last_seen_frame: u64,
    period_rebuild_count: u64,
    period_offscreen_draw_count: u64,
    period_total_offscreen_pixel_area: u64,
    period_total_filter_count: u64,
    period_cache_update_ns: u64,
    period_max_cache_update_ns: u64,
    period_offscreen_command_build_ns: u64,
    period_max_offscreen_command_build_ns: u64,
    period_invalidation_reasons: BTreeMap<String, u64>,
}

impl CacheOffenderRecord {
    fn from_first_rebuild(observation: CacheRebuildObservation, frame: u64) -> Self {
        let pixel_area =
            u64::from(observation.texture_width) * u64::from(observation.texture_height);
        let filter_count = observation.filter_names.len() as u64;
        let mut invalidation_reasons = BTreeMap::new();
        invalidation_reasons.insert(observation.invalidation_reason.to_owned(), 1);

        Self {
            object_key: observation.key,
            descriptor: observation.descriptor,
            source_width: observation.source_width,
            source_height: observation.source_height,
            texture_width: observation.texture_width,
            texture_height: observation.texture_height,
            filters: observation.filter_names,
            last_rebuild_frame: Some(frame),
            lifetime_frames_total: 0,
            lifetime_samples: 0,
            shortest_lifetime_frames: None,
            last_seen_frame: frame,
            period_rebuild_count: 1,
            period_offscreen_draw_count: 0,
            period_total_offscreen_pixel_area: pixel_area,
            period_total_filter_count: filter_count,
            period_cache_update_ns: 0,
            period_max_cache_update_ns: 0,
            period_offscreen_command_build_ns: 0,
            period_max_offscreen_command_build_ns: 0,
            period_invalidation_reasons: invalidation_reasons,
        }
    }

    fn observe_rebuild(&mut self, observation: CacheRebuildObservation, frame: u64) {
        if let Some(previous_frame) = self.last_rebuild_frame {
            let lifetime = frame.saturating_sub(previous_frame);
            self.lifetime_frames_total = self.lifetime_frames_total.saturating_add(lifetime);
            self.lifetime_samples = self.lifetime_samples.saturating_add(1);
            self.shortest_lifetime_frames = Some(
                self.shortest_lifetime_frames
                    .map_or(lifetime, |current| current.min(lifetime)),
            );
        }

        self.descriptor = observation.descriptor;
        self.source_width = observation.source_width;
        self.source_height = observation.source_height;
        self.texture_width = observation.texture_width;
        self.texture_height = observation.texture_height;
        self.filters = observation.filter_names;
        self.last_rebuild_frame = Some(frame);
        self.last_seen_frame = frame;
        self.period_rebuild_count = self.period_rebuild_count.saturating_add(1);
        self.period_total_offscreen_pixel_area = self
            .period_total_offscreen_pixel_area
            .saturating_add(u64::from(self.texture_width) * u64::from(self.texture_height));
        self.period_total_filter_count = self
            .period_total_filter_count
            .saturating_add(self.filters.len() as u64);
        *self
            .period_invalidation_reasons
            .entry(observation.invalidation_reason.to_owned())
            .or_default() += 1;
    }

    fn observe_cache_update(&mut self, duration: Duration, frame: u64) {
        let nanos = duration.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.last_seen_frame = frame;
        self.period_cache_update_ns = self.period_cache_update_ns.saturating_add(nanos);
        self.period_max_cache_update_ns = self.period_max_cache_update_ns.max(nanos);
    }

    fn observe_offscreen_draw(&mut self, duration: Duration, frame: u64) {
        let nanos = duration.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.last_seen_frame = frame;
        self.period_offscreen_draw_count = self.period_offscreen_draw_count.saturating_add(1);
        self.period_offscreen_command_build_ns =
            self.period_offscreen_command_build_ns.saturating_add(nanos);
        self.period_max_offscreen_command_build_ns =
            self.period_max_offscreen_command_build_ns.max(nanos);
    }

    fn has_period_data(&self) -> bool {
        self.period_rebuild_count > 0 || self.period_offscreen_draw_count > 0
    }

    fn summary(&self) -> CacheOffenderSummary {
        CacheOffenderSummary {
            object_key: self.object_key,
            descriptor: self.descriptor.clone(),
            source_width: self.source_width,
            source_height: self.source_height,
            texture_width: self.texture_width,
            texture_height: self.texture_height,
            filters: self.filters.clone(),
            rebuild_count: self.period_rebuild_count,
            offscreen_draw_count: self.period_offscreen_draw_count,
            total_offscreen_pixel_area: self.period_total_offscreen_pixel_area,
            total_filter_count: self.period_total_filter_count,
            cache_update_ms: self.period_cache_update_ns as f64 / 1_000_000.0,
            max_cache_update_ms: self.period_max_cache_update_ns as f64 / 1_000_000.0,
            offscreen_command_build_ms: self.period_offscreen_command_build_ns as f64 / 1_000_000.0,
            max_offscreen_command_build_ms: self.period_max_offscreen_command_build_ns as f64
                / 1_000_000.0,
            total_rebuild_cpu_ms: (self
                .period_cache_update_ns
                .saturating_add(self.period_offscreen_command_build_ns))
                as f64
                / 1_000_000.0,
            average_cache_lifetime_frames: (self.lifetime_samples > 0)
                .then(|| self.lifetime_frames_total as f64 / self.lifetime_samples as f64),
            shortest_cache_lifetime_frames: self.shortest_lifetime_frames,
            invalidation_reasons: self.period_invalidation_reasons.clone(),
        }
    }

    fn reset_period(&mut self) {
        self.period_rebuild_count = 0;
        self.period_offscreen_draw_count = 0;
        self.period_total_offscreen_pixel_area = 0;
        self.period_total_filter_count = 0;
        self.period_cache_update_ns = 0;
        self.period_max_cache_update_ns = 0;
        self.period_offscreen_command_build_ns = 0;
        self.period_max_offscreen_command_build_ns = 0;
        self.period_invalidation_reasons.clear();
    }
}

#[derive(Default)]
struct CacheState {
    offenders: HashMap<u64, CacheOffenderRecord>,
}

fn cache_state() -> &'static Mutex<CacheState> {
    static STATE: OnceLock<Mutex<CacheState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(CacheState::default()))
}

pub fn set_cache_offender_enabled(enabled: bool) {
    CACHE_OFFENDER_ENABLED.store(enabled, Ordering::Relaxed);
    if !enabled {
        let mut state = match cache_state().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.offenders.clear();
    }
}

#[inline]
pub fn cache_offender_enabled() -> bool {
    CACHE_OFFENDER_ENABLED.load(Ordering::Relaxed)
}

pub fn record_cache_rebuild(observation: CacheRebuildObservation) {
    if !cache_offender_enabled() {
        return;
    }

    let frame = RENDER_FRAME_ID.load(Ordering::Relaxed);
    let mut state = match cache_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };

    if !state.offenders.contains_key(&observation.key)
        && state.offenders.len() >= MAX_CACHE_OFFENDERS
    {
        return;
    }

    let key = observation.key;
    match state.offenders.entry(key) {
        Entry::Occupied(mut entry) => entry.get_mut().observe_rebuild(observation, frame),
        Entry::Vacant(entry) => {
            entry.insert(CacheOffenderRecord::from_first_rebuild(observation, frame));
        }
    }
}

pub fn record_cache_update(key: u64, duration: Duration) {
    if !cache_offender_enabled() {
        return;
    }

    let frame = RENDER_FRAME_ID.load(Ordering::Relaxed);
    let mut state = match cache_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(record) = state.offenders.get_mut(&key) {
        record.observe_cache_update(duration, frame);
    }
}

pub fn record_cache_offscreen_draw(key: u64, duration: Duration) {
    if !cache_offender_enabled() {
        return;
    }

    let frame = RENDER_FRAME_ID.load(Ordering::Relaxed);
    let mut state = match cache_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(record) = state.offenders.get_mut(&key) {
        record.observe_offscreen_draw(duration, frame);
    }
}

pub fn advance_render_frame() {
    RENDER_FRAME_ID.fetch_add(1, Ordering::Relaxed);
}

pub fn take_cache_offender_summaries(limit: usize) -> Vec<CacheOffenderSummary> {
    if !cache_offender_enabled() {
        return Vec::new();
    }

    let current_frame = RENDER_FRAME_ID.load(Ordering::Relaxed);
    let mut state = match cache_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };

    let mut records: Vec<&CacheOffenderRecord> = state
        .offenders
        .values()
        .filter(|record| record.has_period_data())
        .collect();
    records.sort_by(|left, right| {
        right
            .period_cache_update_ns
            .saturating_add(right.period_offscreen_command_build_ns)
            .cmp(
                &left
                    .period_cache_update_ns
                    .saturating_add(left.period_offscreen_command_build_ns),
            )
            .then_with(|| right.period_rebuild_count.cmp(&left.period_rebuild_count))
            .then_with(|| {
                right
                    .period_total_offscreen_pixel_area
                    .cmp(&left.period_total_offscreen_pixel_area)
            })
    });

    let summaries = records
        .into_iter()
        .take(limit)
        .map(CacheOffenderRecord::summary)
        .collect();

    for record in state.offenders.values_mut() {
        record.reset_period();
    }
    state.offenders.retain(|_, record| {
        current_frame.saturating_sub(record.last_seen_frame) <= STALE_CACHE_FRAMES
    });

    summaries
}

/// Which symbol issued a blend, independent of how many objects on stage happen to be drawing it.
///
/// A crowded AQW map draws one armour layer once per player, and the useful question is "what does
/// that layer cost across the whole stage", not "what did player nine cost". A character id is only
/// unique within its movie, so the movie is part of the identity too.
///
/// Every field is a plain integer or a static string. This key is built for each of the ~170 blends
/// an AQW frame emits, so it must not allocate or walk the display tree; the readable name for the
/// symbol is resolved once, when the site is first seen.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct BlendSiteKey {
    pub blend_mode: &'static str,
    /// Identity of the movie the symbol came from, as a pointer, never dereferenced.
    pub movie: usize,
    pub character_id: u16,
}

/// One blend as it was emitted, before it is folded into its site.
#[derive(Clone, Copy, Debug)]
pub struct BlendObservation {
    /// Identifies the display object, so a site can report how many of them drew it.
    pub instance_key: u64,
    /// Area of the blend's bounds on the render target, in pixels. This is what the backend has
    /// to allocate a sub-target for and composite, so it is the closest thing to a cost.
    pub pixel_area: u64,
    /// Draw commands inside the blend, which is roughly how much art the sub-target holds.
    pub sub_commands: u64,
    /// What the subtree is, when it is exactly one thing. See `sole_command_kind`.
    pub sole_command_kind: &'static str,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct BlendSiteSummary {
    pub blend_mode: &'static str,
    pub actor_classification: &'static str,
    pub class_name: String,
    pub example_display_path: String,
    pub movie_url: String,
    pub character_id: u16,
    pub blends: u64,
    pub instances: usize,
    pub sub_commands: u64,
    pub total_pixel_area: u64,
    pub max_pixel_area: u64,
    /// How the site's blends break down by what their subtree holds, so the count of blends a
    /// single-draw bypass could actually take is readable rather than inferred.
    pub sole_command_kinds: BTreeMap<&'static str, u64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct BlendAttributionReport {
    pub blends: u64,
    pub unique_sites: usize,
    pub total_pixel_area: u64,
    pub by_mode: BTreeMap<&'static str, u64>,
    pub by_actor: BTreeMap<&'static str, u64>,
    pub by_sole_command: BTreeMap<&'static str, u64>,
    pub top: Vec<BlendSiteSummary>,
    /// Blends belonging to sites that did not make the table, so the top rows can be read as a
    /// share of the frame rather than as the whole of it.
    pub tail_blends: u64,
}

const MAX_BLEND_SITES: usize = 2_048;
/// Instances are counted to tell "one object blending constantly" apart from "many objects
/// blending once", which needs a headcount, not an exact roster.
const MAX_BLEND_SITE_INSTANCES: usize = 512;

static BLEND_ATTRIBUTION_ENABLED: AtomicBool = AtomicBool::new(false);

/// The readable half of a site, resolved once when the site is first seen.
#[derive(Clone, Debug, Default)]
pub struct BlendSiteIdentity {
    pub actor_classification: &'static str,
    pub class_name: String,
    pub display_path: String,
    pub movie_url: String,
}

#[derive(Debug, Default)]
struct BlendSiteRecord {
    identity: BlendSiteIdentity,
    blends: u64,
    sub_commands: u64,
    total_pixel_area: u64,
    max_pixel_area: u64,
    instances: HashSet<u64>,
    sole_command_kinds: BTreeMap<&'static str, u64>,
}

impl BlendSiteRecord {
    fn observe(&mut self, observation: BlendObservation) {
        self.blends = self.blends.saturating_add(1);
        self.sub_commands = self.sub_commands.saturating_add(observation.sub_commands);
        self.total_pixel_area = self.total_pixel_area.saturating_add(observation.pixel_area);
        self.max_pixel_area = self.max_pixel_area.max(observation.pixel_area);
        if self.instances.len() < MAX_BLEND_SITE_INSTANCES {
            self.instances.insert(observation.instance_key);
        }
        *self
            .sole_command_kinds
            .entry(observation.sole_command_kind)
            .or_default() += 1;
    }

    fn summary(&self, key: &BlendSiteKey) -> BlendSiteSummary {
        BlendSiteSummary {
            blend_mode: key.blend_mode,
            actor_classification: self.identity.actor_classification,
            class_name: self.identity.class_name.clone(),
            example_display_path: self.identity.display_path.clone(),
            movie_url: self.identity.movie_url.clone(),
            character_id: key.character_id,
            blends: self.blends,
            instances: self.instances.len(),
            sub_commands: self.sub_commands,
            total_pixel_area: self.total_pixel_area,
            max_pixel_area: self.max_pixel_area,
            sole_command_kinds: self.sole_command_kinds.clone(),
        }
    }
}

#[derive(Default)]
struct BlendAttributionState {
    sites: HashMap<BlendSiteKey, BlendSiteRecord>,
}

impl BlendAttributionState {
    /// `identity` is a closure so that naming the symbol, which walks the display tree and
    /// allocates, happens once per site rather than once per blend.
    fn record(
        &mut self,
        key: BlendSiteKey,
        observation: BlendObservation,
        identity: impl FnOnce() -> BlendSiteIdentity,
    ) {
        let full = self.sites.len() >= MAX_BLEND_SITES;
        match self.sites.entry(key) {
            Entry::Occupied(mut entry) => entry.get_mut().observe(observation),
            Entry::Vacant(entry) if !full => {
                entry
                    .insert(BlendSiteRecord {
                        identity: identity(),
                        ..BlendSiteRecord::default()
                    })
                    .observe(observation);
            }
            Entry::Vacant(_) => {}
        }
    }

    fn take_report(&mut self, limit: usize) -> BlendAttributionReport {
        let mut report = BlendAttributionReport::default();
        let mut ranked: Vec<(&BlendSiteKey, &BlendSiteRecord)> =
            Vec::with_capacity(self.sites.len());

        for (key, record) in &self.sites {
            if record.blends == 0 {
                continue;
            }
            report.blends = report.blends.saturating_add(record.blends);
            report.total_pixel_area = report
                .total_pixel_area
                .saturating_add(record.total_pixel_area);
            *report.by_mode.entry(key.blend_mode).or_default() += record.blends;
            *report
                .by_actor
                .entry(record.identity.actor_classification)
                .or_default() += record.blends;
            for (kind, count) in &record.sole_command_kinds {
                *report.by_sole_command.entry(kind).or_default() += count;
            }
            ranked.push((key, record));
        }

        report.unique_sites = ranked.len();
        // Area, not count: a swarm of sparks issues more blends than one full-stage tint while
        // costing a fraction of the fill, and the table exists to point at the expensive one.
        ranked.sort_by(|(left_key, left), (right_key, right)| {
            right
                .total_pixel_area
                .cmp(&left.total_pixel_area)
                .then_with(|| right.blends.cmp(&left.blends))
                .then_with(|| left.identity.class_name.cmp(&right.identity.class_name))
                .then_with(|| left_key.character_id.cmp(&right_key.character_id))
        });

        report.top = ranked
            .iter()
            .take(limit)
            .map(|(key, record)| record.summary(key))
            .collect();
        report.tail_blends = ranked
            .iter()
            .skip(limit)
            .map(|(_, record)| record.blends)
            .sum();

        self.sites.clear();
        report
    }
}

fn blend_attribution_state() -> &'static Mutex<BlendAttributionState> {
    static STATE: OnceLock<Mutex<BlendAttributionState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(BlendAttributionState::default()))
}

pub fn set_blend_attribution_enabled(enabled: bool) {
    BLEND_ATTRIBUTION_ENABLED.store(enabled, Ordering::Relaxed);
    if !enabled {
        let mut state = match blend_attribution_state().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.sites.clear();
    }
}

#[inline]
pub fn blend_attribution_enabled() -> bool {
    BLEND_ATTRIBUTION_ENABLED.load(Ordering::Relaxed)
}

/// Records one emitted blend. The descriptor is built lazily because walking a display object's
/// ancestry to name it costs more than the blend itself when attribution is off.
pub fn record_blend(
    blend_mode: &'static str,
    object: DisplayObject<'_>,
    pixel_area: u64,
    sub_commands: u64,
    sole_command_kind: &'static str,
) {
    if !blend_attribution_enabled() {
        return;
    }

    let key = BlendSiteKey {
        blend_mode,
        movie: Arc::as_ptr(&object.movie()) as usize,
        character_id: object.id(),
    };
    let observation = BlendObservation {
        instance_key: object.as_ptr() as u64,
        pixel_area,
        sub_commands,
        sole_command_kind,
    };

    let mut state = match blend_attribution_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.record(key, observation, || {
        let descriptor = DisplayObjectDescriptor::from_display_object(object);
        BlendSiteIdentity {
            actor_classification: descriptor.actor_classification,
            class_name: descriptor
                .class_name
                .or(descriptor.instance_name)
                .unwrap_or_else(|| descriptor.object_type.to_string()),
            display_path: descriptor.display_path,
            movie_url: descriptor.movie_url,
        }
    });
}

pub fn take_blend_attribution_report(limit: usize) -> BlendAttributionReport {
    if !blend_attribution_enabled() {
        return BlendAttributionReport::default();
    }

    let mut state = match blend_attribution_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.take_report(limit)
}

#[derive(Serialize)]
struct HostMouseTraceRecord<'a> {
    record_type: &'static str,
    unix_time_ms: u128,
    elapsed_ms: f64,
    event: &'a str,
    button: Option<&'a str>,
    window_x: f64,
    window_y: f64,
    movie_x: f64,
    movie_y: f64,
}

#[derive(Serialize)]
struct CoreClipTraceRecord<'a> {
    record_type: &'static str,
    unix_time_ms: u128,
    elapsed_ms: f64,
    event: &'a str,
    stage_x: f64,
    stage_y: f64,
    target: DisplayObjectDescriptor,
    hovered: Option<DisplayObjectDescriptor>,
    pressed: Option<DisplayObjectDescriptor>,
    target_route: Vec<MouseRouteNode>,
    hovered_route: Vec<MouseRouteNode>,
    pressed_route: Vec<MouseRouteNode>,
    clip_handler_handled: bool,
    avm2_dispatch_handled: bool,
}

#[derive(Serialize)]
struct TimelineChildRebindRecord<'a> {
    record_type: &'static str,
    unix_time_ms: u128,
    elapsed_ms: f64,
    event: &'static str,
    phase: &'static str,
    clip: DisplayObjectDescriptor,
    current_frame: u16,
    summary: &'a TimelineChildRebindSummary,
}

#[derive(Serialize)]
struct TimelineTraceRecord<'a> {
    record_type: &'static str,
    unix_time_ms: u128,
    elapsed_ms: f64,
    event: &'a str,
    phase: &'static str,
    clip: DisplayObjectDescriptor,
    current_frame: u16,
    queued_script_frame: u16,
    last_queued_script_frame: Option<u16>,
    playing: bool,
    pending_script: bool,
    before: Option<ConstructionSnapshot>,
    after: Option<ConstructionSnapshot>,
    duration_ms: Option<f64>,
    error: Option<String>,
}

struct TimelineTraceState {
    writer: Option<BufWriter<File>>,
    started_at: Instant,
    last_flush: Instant,
    pending_records: usize,
}

impl Default for TimelineTraceState {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            writer: None,
            started_at: now,
            last_flush: now,
            pending_records: 0,
        }
    }
}

fn timeline_trace_state() -> &'static Mutex<TimelineTraceState> {
    static STATE: OnceLock<Mutex<TimelineTraceState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(TimelineTraceState::default()))
}

pub fn configure_timeline_trace(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    let now = Instant::now();
    let mut state = match timeline_trace_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.writer = Some(BufWriter::new(file));
    state.started_at = now;
    state.last_flush = now;
    state.pending_records = 0;
    TIMELINE_TRACE_ENABLED.store(true, Ordering::Relaxed);
    Ok(())
}

pub fn shutdown_timeline_trace() {
    TIMELINE_TRACE_ENABLED.store(false, Ordering::Relaxed);
    let mut state = match timeline_trace_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(writer) = state.writer.as_mut() {
        let _ = writer.flush();
    }
    state.writer = None;
    state.pending_records = 0;
}

#[inline]
pub fn timeline_trace_enabled() -> bool {
    TIMELINE_TRACE_ENABLED.load(Ordering::Relaxed)
}

/// Every timeline jump and every visibility flip, with the object's path.
///
/// For faults that are visible on screen and silent in the log. The world map flicker is a panel
/// appearing and disappearing at frame rate with no error of any kind, and after the AQW paths and
/// the bitmap cache were both eliminated there was nothing left to read but what the display list is
/// actually being told to do. Whatever oscillates shows up here as an obvious repeating pair.
///
/// Budgeted rather than rate limited: a flood truncated in the middle of a cycle is useless, so this
/// records the FIRST `PANEL_CHURN_BUDGET` events and then stops for good. That is enough to cover a
/// reproduction that takes under a minute, and it cannot fill a disk if one takes longer.
static PANEL_CHURN_ENABLED: AtomicBool = AtomicBool::new(false);
static PANEL_CHURN_REMAINING: AtomicU64 = AtomicU64::new(0);
const PANEL_CHURN_BUDGET: u64 = 40_000;

pub fn set_panel_churn_enabled(enabled: bool) {
    PANEL_CHURN_ENABLED.store(enabled, Ordering::Relaxed);
    PANEL_CHURN_REMAINING.store(
        if enabled { PANEL_CHURN_BUDGET } else { 0 },
        Ordering::Relaxed,
    );
}

#[inline]
pub fn panel_churn_enabled() -> bool {
    PANEL_CHURN_ENABLED.load(Ordering::Relaxed)
}

/// Claim one event from the budget. False once it is spent, which also stops path construction.
fn claim_panel_churn_budget() -> bool {
    PANEL_CHURN_REMAINING
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
            remaining.checked_sub(1).filter(|_| remaining > 0)
        })
        .is_ok()
}

/// Identify an object well enough to tell two of them apart.
///
/// NOT `DisplayObject::path()`. That walks `avm1_parent()`, which is `None` for AVM2 content, so it
/// degrades to `_level{depth}` for every object in an AS3 movie. Five different objects that happen
/// to share a depth then log as five identical lines, which is exactly how the first attempt at this
/// probe wasted a run. `build_display_path` walks real `parent()` links and uses instance names, and
/// the pointer makes identity unambiguous even when two objects have the same path.
fn panel_churn_identity(object: DisplayObject<'_>) -> String {
    let class = object
        .object2()
        .map(|avm2_object| {
            avm2_object
                .instance_class()
                .name()
                .local_name()
                .to_utf8_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    format!(
        "#{:x} {} [{}]",
        object.as_ptr() as usize,
        build_display_path(object),
        class,
    )
}

pub fn record_panel_goto(clip: MovieClip<'_>, from: u16, to: u16, stop: bool) {
    if !panel_churn_enabled() || !claim_panel_churn_budget() {
        return;
    }

    tracing::info!(
        target: "aether::panel_churn",
        "goto {} {} -> {} ({})",
        panel_churn_identity(clip.into()),
        from,
        to,
        if stop { "Stop" } else { "Play" },
    );
}

/// Display-list membership, the third way an object stops being drawn.
///
/// The first run of this probe watched only gotos and visibility and saw well under one event per
/// frame, which cannot account for a per-frame flicker. `mcPopup.loadMap` does
/// `mcMap.removeChildAt(0)` followed by `addChild(new Loader())` every time it runs, so add/remove
/// was the obvious hole.
pub fn record_panel_child(parent: DisplayObject<'_>, child: DisplayObject<'_>, action: &str) {
    if !panel_churn_enabled() || !claim_panel_churn_budget() {
        return;
    }

    tracing::info!(
        target: "aether::panel_churn",
        "{} {} <- {}",
        action,
        panel_churn_identity(parent),
        panel_churn_identity(child),
    );
}

pub fn record_panel_visible(object: DisplayObject<'_>, value: bool) {
    if !panel_churn_enabled() || !claim_panel_churn_budget() {
        return;
    }

    tracing::info!(
        target: "aether::panel_churn",
        "visible {} = {}",
        panel_churn_identity(object),
        value,
    );
}

#[expect(clippy::too_many_arguments)]
pub fn record_timeline_event(
    event: &str,
    context: &UpdateContext<'_>,
    clip: MovieClip<'_>,
    before: Option<ConstructionSnapshot>,
    after: Option<ConstructionSnapshot>,
    duration: Option<Duration>,
    error: Option<String>,
) {
    if !timeline_trace_enabled() {
        return;
    }
    let elapsed_ms = {
        let state = match timeline_trace_state().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.started_at.elapsed().as_secs_f64() * 1_000.0
    };
    let record = TimelineTraceRecord {
        record_type: "timeline",
        unix_time_ms: unix_time_ms(),
        elapsed_ms,
        event,
        phase: frame_phase_name(*context.frame_phase),
        clip: DisplayObjectDescriptor::from_display_object(clip.into()),
        current_frame: clip.current_frame(),
        queued_script_frame: clip.queued_script_frame(),
        last_queued_script_frame: clip.last_queued_script_frame(),
        playing: clip.playing(),
        pending_script: clip.has_pending_script(),
        before,
        after,
        duration_ms: duration.map(|duration| duration.as_secs_f64() * 1_000.0),
        error,
    };
    write_timeline_trace(&record);
}

pub fn record_timeline_child_rebind(
    context: &UpdateContext<'_>,
    clip: MovieClip<'_>,
    summary: &TimelineChildRebindSummary,
) {
    if !timeline_trace_enabled() {
        return;
    }
    let elapsed_ms = {
        let state = match timeline_trace_state().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.started_at.elapsed().as_secs_f64() * 1_000.0
    };
    let record = TimelineChildRebindRecord {
        record_type: "timeline",
        unix_time_ms: unix_time_ms(),
        elapsed_ms,
        event: "timeline_child_rebind",
        phase: frame_phase_name(*context.frame_phase),
        clip: DisplayObjectDescriptor::from_display_object(clip.into()),
        current_frame: clip.current_frame(),
        summary,
    };
    write_timeline_trace(&record);
}

fn frame_phase_name(phase: FramePhase) -> &'static str {
    match phase {
        FramePhase::Enter => "enter",
        FramePhase::Construct => "construct",
        FramePhase::FrameScripts => "frame_scripts",
        FramePhase::Exit => "exit",
        FramePhase::Idle => "idle",
    }
}

fn write_timeline_trace(record: &impl Serialize) {
    let mut state = match timeline_trace_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    let next_pending = state.pending_records.saturating_add(1);
    let should_flush = next_pending >= INPUT_FLUSH_EVENT_COUNT
        || state.last_flush.elapsed() >= INPUT_FLUSH_INTERVAL;

    let write_result = {
        let Some(writer) = state.writer.as_mut() else {
            return;
        };
        serde_json::to_writer(&mut *writer, record)
            .map_err(io::Error::other)
            .and_then(|_| writer.write_all(b"\n"))
            .and_then(|_| if should_flush { writer.flush() } else { Ok(()) })
    };

    if write_result.is_err() {
        TIMELINE_TRACE_ENABLED.store(false, Ordering::Relaxed);
        state.writer = None;
        state.pending_records = 0;
        return;
    }

    if should_flush {
        state.pending_records = 0;
        state.last_flush = Instant::now();
    } else {
        state.pending_records = next_pending;
    }
}

struct InputTraceState {
    writer: Option<BufWriter<File>>,
    started_at: Instant,
    last_flush: Instant,
    pending_records: usize,
}

impl Default for InputTraceState {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            writer: None,
            started_at: now,
            last_flush: now,
            pending_records: 0,
        }
    }
}

fn input_trace_state() -> &'static Mutex<InputTraceState> {
    static STATE: OnceLock<Mutex<InputTraceState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(InputTraceState::default()))
}

pub fn configure_input_trace(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    let now = Instant::now();
    let mut state = match input_trace_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.writer = Some(BufWriter::new(file));
    state.started_at = now;
    state.last_flush = now;
    state.pending_records = 0;
    INPUT_TRACE_ENABLED.store(true, Ordering::Relaxed);
    Ok(())
}

pub fn shutdown_input_trace() {
    INPUT_TRACE_ENABLED.store(false, Ordering::Relaxed);
    let mut state = match input_trace_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(writer) = state.writer.as_mut() {
        let _ = writer.flush();
    }
    state.writer = None;
    state.pending_records = 0;
}

#[inline]
pub fn input_trace_enabled() -> bool {
    INPUT_TRACE_ENABLED.load(Ordering::Relaxed)
}

pub fn record_host_mouse(
    event: &str,
    button: Option<&str>,
    window_x: f64,
    window_y: f64,
    movie_x: f64,
    movie_y: f64,
) {
    if !input_trace_enabled() {
        return;
    }
    let elapsed_ms = input_trace_elapsed_ms();
    let record = HostMouseTraceRecord {
        record_type: "host_mouse",
        unix_time_ms: unix_time_ms(),
        elapsed_ms,
        event,
        button,
        window_x,
        window_y,
        movie_x,
        movie_y,
    };
    write_input_trace(&record);
}

#[expect(clippy::too_many_arguments)]
pub fn record_core_clip_event(
    event: &str,
    stage_x: f64,
    stage_y: f64,
    target: DisplayObjectDescriptor,
    hovered: Option<DisplayObjectDescriptor>,
    pressed: Option<DisplayObjectDescriptor>,
    target_route: Vec<MouseRouteNode>,
    hovered_route: Vec<MouseRouteNode>,
    pressed_route: Vec<MouseRouteNode>,
    clip_handler_handled: bool,
    avm2_dispatch_handled: bool,
) {
    if !input_trace_enabled() {
        return;
    }
    let elapsed_ms = input_trace_elapsed_ms();
    let record = CoreClipTraceRecord {
        record_type: "core_clip_event",
        unix_time_ms: unix_time_ms(),
        elapsed_ms,
        event,
        stage_x,
        stage_y,
        target,
        hovered,
        pressed,
        target_route,
        hovered_route,
        pressed_route,
        clip_handler_handled,
        avm2_dispatch_handled,
    };
    write_input_trace(&record);
}

pub fn clip_event_name(event: ClipEvent<'_>) -> &'static str {
    match event {
        ClipEvent::DragOut { .. } => "DragOut",
        ClipEvent::DragOver { .. } => "DragOver",
        ClipEvent::MouseUpInside => "MouseUpInside",
        ClipEvent::RightMouseUpInside => "RightMouseUpInside",
        ClipEvent::MiddleMouseUpInside => "MiddleMouseUpInside",
        ClipEvent::MouseMoveInside => "MouseMoveInside",
        ClipEvent::Press { .. } => "Press",
        ClipEvent::RightPress => "RightPress",
        ClipEvent::MiddlePress => "MiddlePress",
        ClipEvent::RollOut { .. } => "RollOut",
        ClipEvent::RollOver { .. } => "RollOver",
        ClipEvent::Release { .. } => "Release",
        ClipEvent::RightRelease => "RightRelease",
        ClipEvent::MiddleRelease => "MiddleRelease",
        ClipEvent::ReleaseOutside => "ReleaseOutside",
        ClipEvent::RightReleaseOutside => "RightReleaseOutside",
        ClipEvent::MiddleReleaseOutside => "MiddleReleaseOutside",
        _ => "Other",
    }
}

fn input_trace_elapsed_ms() -> f64 {
    let state = match input_trace_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.started_at.elapsed().as_secs_f64() * 1_000.0
}

fn write_input_trace(record: &impl Serialize) {
    let mut state = match input_trace_state().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    let next_pending = state.pending_records.saturating_add(1);
    let should_flush = next_pending >= INPUT_FLUSH_EVENT_COUNT
        || state.last_flush.elapsed() >= INPUT_FLUSH_INTERVAL;

    let write_result = {
        let Some(writer) = state.writer.as_mut() else {
            return;
        };
        serde_json::to_writer(&mut *writer, record)
            .map_err(io::Error::other)
            .and_then(|_| writer.write_all(b"\n"))
            .and_then(|_| if should_flush { writer.flush() } else { Ok(()) })
    };

    if write_result.is_err() {
        INPUT_TRACE_ENABLED.store(false, Ordering::Relaxed);
        state.writer = None;
        state.pending_records = 0;
        return;
    }

    if should_flush {
        state.pending_records = 0;
        state.last_flush = Instant::now();
    } else {
        state.pending_records = next_pending;
    }
}

#[derive(Clone, Debug, Default)]
struct MovementTraceRuntime {
    sequence_id: u64,
    active: bool,
    phase: &'static str,
    owner_path: Option<String>,
    guard_eligible: bool,
    collision_count: usize,
    blocking_collision_count: usize,
    collision_overflow_reported: bool,
    target: Option<MovementPoint>,
    walk_started_at: Option<Instant>,
    expected_duration_ms: f64,
    best_remaining_distance: Option<f64>,
    callbacks_without_progress: u32,
    enter_frame_depth: usize,
    enter_frame_start_position: Option<MovementPoint>,
    enter_frame_stop_suppressed: bool,
    mouse_move_depth: usize,
    last_mouse_move_at: Option<Instant>,
    suppressed_stop_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MovementWalkDisposition {
    Owner,
    Standalone,
    Foreign,
}

fn movement_receiver_matches_owner(owner: Option<&str>, candidate: Option<&str>) -> bool {
    matches!(
        (owner, candidate),
        (Some(owner), Some(candidate))
            if !owner.is_empty() && !candidate.is_empty() && owner == candidate
    )
}

fn movement_object_belongs_to_owner(owner: Option<&str>, candidate: &str) -> bool {
    let Some(owner) = owner.filter(|owner| !owner.is_empty()) else {
        return false;
    };
    let Some(suffix) = candidate.strip_prefix(owner) else {
        return false;
    };
    suffix.starts_with('/') && suffix.len() > 1
}

fn movement_collision_needs_geometry(
    owner: Option<&str>,
    moving_path: &str,
    trace_enabled: bool,
) -> bool {
    trace_enabled || movement_object_belongs_to_owner(owner, moving_path)
}

impl MovementTraceRuntime {
    fn begin_or_update_walk(
        &mut self,
        receiver_path: Option<&str>,
        target: MovementPoint,
        expected_duration_ms: f64,
        started_at: Instant,
    ) -> MovementWalkDisposition {
        let disposition = if self.active {
            if !movement_receiver_matches_owner(self.owner_path.as_deref(), receiver_path) {
                return MovementWalkDisposition::Foreign;
            }
            MovementWalkDisposition::Owner
        } else {
            self.sequence_id = self.sequence_id.saturating_add(1);
            self.active = true;
            self.owner_path = receiver_path
                .filter(|path| !path.is_empty())
                .map(str::to_owned);
            self.guard_eligible = false;
            self.suppressed_stop_count = 0;
            MovementWalkDisposition::Standalone
        };

        self.collision_count = 0;
        self.blocking_collision_count = 0;
        self.collision_overflow_reported = false;
        self.phase = "walk";
        self.target = Some(target);
        self.walk_started_at = Some(started_at);
        self.expected_duration_ms = expected_duration_ms;
        self.best_remaining_distance = None;
        self.callbacks_without_progress = 0;
        disposition
    }

    fn finish_if_owner(&mut self, receiver_path: Option<&str>) -> bool {
        if !self.active
            || !movement_receiver_matches_owner(self.owner_path.as_deref(), receiver_path)
        {
            return false;
        }
        self.active = false;
        self.phase = "idle";
        self.owner_path = None;
        self.guard_eligible = false;
        self.target = None;
        self.walk_started_at = None;
        self.expected_duration_ms = 0.0;
        self.best_remaining_distance = None;
        self.callbacks_without_progress = 0;
        self.enter_frame_depth = 0;
        self.enter_frame_start_position = None;
        self.enter_frame_stop_suppressed = false;
        self.last_mouse_move_at = None;
        true
    }

    fn enter_frame_if_owner(
        &mut self,
        receiver_path: Option<&str>,
        receiver_position: Option<MovementPoint>,
    ) -> bool {
        if !self.active
            || !self.guard_eligible
            || !movement_receiver_matches_owner(self.owner_path.as_deref(), receiver_path)
        {
            return false;
        }

        let Some(receiver_position) = receiver_position else {
            return false;
        };
        if self.enter_frame_depth == 0 {
            // A collision can authorize a stop only in the authored movement callback which
            // observed it. If the avatar continued walking afterward, carrying that collision
            // forward would permanently disable the guard in dense maps.
            self.blocking_collision_count = 0;
            self.enter_frame_start_position = Some(receiver_position);
            self.enter_frame_stop_suppressed = false;
            if let Some(target) = self.target {
                let remaining_distance =
                    (target.x - receiver_position.x).hypot(target.y - receiver_position.y);
                if remaining_distance.is_finite() {
                    match self.best_remaining_distance {
                        Some(best) if remaining_distance + MOVEMENT_PROGRESS_EPSILON_PX < best => {
                            self.best_remaining_distance = Some(remaining_distance);
                            self.callbacks_without_progress = 0;
                        }
                        Some(_) => {
                            self.callbacks_without_progress =
                                self.callbacks_without_progress.saturating_add(1);
                        }
                        None => {
                            self.best_remaining_distance = Some(remaining_distance);
                            self.callbacks_without_progress = 1;
                        }
                    }
                }
            }
        }
        self.enter_frame_depth = self.enter_frame_depth.saturating_add(1);
        true
    }

    fn exit_enter_frame(&mut self) {
        self.enter_frame_depth = self.enter_frame_depth.saturating_sub(1);
        if self.enter_frame_depth == 0 {
            self.enter_frame_start_position = None;
            self.enter_frame_stop_suppressed = false;
        }
    }

    fn enter_mouse_move(&mut self) {
        self.enter_mouse_move_at(Instant::now());
    }

    fn enter_mouse_move_at(&mut self, now: Instant) {
        self.mouse_move_depth = self.mouse_move_depth.saturating_add(1);
        self.last_mouse_move_at = Some(now);
    }

    fn exit_mouse_move(&mut self) {
        self.mouse_move_depth = self.mouse_move_depth.saturating_sub(1);
    }

    fn finish_mouse_move_render_at(&mut self, now: Instant) {
        if self.active && self.guard_eligible && self.last_mouse_move_at.is_some() {
            self.last_mouse_move_at = Some(now);
        }
    }

    fn try_record_suppressed_stop(
        &mut self,
        receiver_position: MovementPoint,
        remaining_distance: f64,
        walk_elapsed_ms: f64,
    ) -> Option<usize> {
        self.try_record_suppressed_stop_at(
            receiver_position,
            remaining_distance,
            walk_elapsed_ms,
            Instant::now(),
        )
    }

    /// The three callback scopes in which a `stopWalking` call is eligible for suppression:
    /// inside `onEnterFrameWalk`, inside a live `Mouse.onMouseMove` dispatch, or inside the
    /// one-frame-delayed callback that immediately follows such a dispatch.
    ///
    /// Shared by the suppression decision and its decline telemetry so the two cannot disagree.
    fn guarded_callback_scope(&self, now: Instant) -> (bool, bool, bool) {
        let in_enter_frame = self.enter_frame_depth != 0;
        let in_mouse_move = self.mouse_move_depth != 0;
        let in_delayed_mouse_callback = !in_mouse_move
            && self.blocking_collision_count == 0
            && self.last_mouse_move_at.is_some_and(|last_mouse_move_at| {
                now.checked_duration_since(last_mouse_move_at)
                    .is_some_and(|elapsed| elapsed <= AQW_DELAYED_MOUSE_STOP_GRACE)
            });
        (in_enter_frame, in_mouse_move, in_delayed_mouse_callback)
    }

    fn try_record_suppressed_stop_at(
        &mut self,
        receiver_position: MovementPoint,
        remaining_distance: f64,
        walk_elapsed_ms: f64,
        now: Instant,
    ) -> Option<usize> {
        let (in_enter_frame, in_mouse_move, in_delayed_mouse_callback) =
            self.guarded_callback_scope(now);
        if !self.active
            || !self.guard_eligible
            || (!in_enter_frame && !in_mouse_move && !in_delayed_mouse_callback)
            || !movement_stop_guard_should_suppress(
                remaining_distance,
                walk_elapsed_ms,
                self.expected_duration_ms,
            )
        {
            return None;
        }

        if in_enter_frame {
            let start_position = self.enter_frame_start_position?;
            // A collision callback can legitimately call stopWalking twice: first from the
            // collision branch and then from the rounded no-progress branch. Let that second
            // call preserve the wall stop. Without a blocking collision, repeated matching
            // calls are the same premature-stop condition and must all remain suppressed.
            if (self.enter_frame_stop_suppressed && self.blocking_collision_count > 0)
                || self.callbacks_without_progress >= MAX_MOVEMENT_CALLBACKS_WITHOUT_PROGRESS
                || !movement_points_have_same_rounded_position(start_position, receiver_position)
            {
                return None;
            }
            self.enter_frame_stop_suppressed = true;
        }

        self.suppressed_stop_count = self.suppressed_stop_count.saturating_add(1);
        Some(self.suppressed_stop_count)
    }

    fn record_collision_if_owner(
        &mut self,
        moving_path: &str,
        moving_bounds: MovementBounds,
        collider_bounds: MovementBounds,
    ) -> Option<(usize, bool, bool)> {
        if !self.active
            || !movement_object_belongs_to_owner(self.owner_path.as_deref(), moving_path)
        {
            return None;
        }

        let blocks_stop_guard = if self.phase == "walk" {
            self.target.map_or(true, |target| {
                movement_collision_blocks_stop_guard(moving_bounds, collider_bounds, target)
            })
        } else {
            true
        };
        self.collision_count = self.collision_count.saturating_add(1);
        if blocks_stop_guard {
            self.blocking_collision_count = self.blocking_collision_count.saturating_add(1);
        }
        let overflow = if self.collision_count > MAX_MOVEMENT_COLLISIONS_PER_SEQUENCE {
            let should_report = !self.collision_overflow_reported;
            self.collision_overflow_reported = true;
            should_report
        } else {
            false
        };
        Some((self.collision_count, blocks_stop_guard, overflow))
    }
}

fn movement_trace_runtime() -> &'static Mutex<MovementTraceRuntime> {
    static STATE: OnceLock<Mutex<MovementTraceRuntime>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(MovementTraceRuntime::default()))
}

#[derive(Clone, Copy, Debug, Serialize)]
struct MovementPoint {
    x: f64,
    y: f64,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct MovementBounds {
    x_min: f64,
    y_min: f64,
    x_max: f64,
    y_max: f64,
}

#[derive(Serialize)]
struct MovementMethodRecord<'a> {
    record_type: &'static str,
    unix_time_ms: u128,
    elapsed_ms: f64,
    event: &'a str,
    sequence_id: u64,
    phase: &'a str,
    receiver: Option<DisplayObjectDescriptor>,
    receiver_position: Option<MovementPoint>,
    requested: Option<MovementPoint>,
    target: Option<MovementPoint>,
    speed: Option<f64>,
    result: Option<&'a str>,
    collision_count: usize,
}

#[derive(Serialize)]
struct MovementCollisionRecord<'a> {
    record_type: &'static str,
    unix_time_ms: u128,
    elapsed_ms: f64,
    event: &'static str,
    sequence_id: u64,
    phase: &'a str,
    collision_index: usize,
    moving_object: DisplayObjectDescriptor,
    moving_bounds: MovementBounds,
    collider: DisplayObjectDescriptor,
    collider_bounds: MovementBounds,
    blocks_stop_guard: bool,
}

#[derive(Serialize)]
struct MovementStopGuardRecord {
    record_type: &'static str,
    unix_time_ms: u128,
    elapsed_ms: f64,
    event: &'static str,
    sequence_id: u64,
    receiver: Option<DisplayObjectDescriptor>,
    receiver_position: Option<MovementPoint>,
    target: MovementPoint,
    remaining_distance: f64,
    walk_elapsed_ms: f64,
    expected_duration_ms: f64,
    best_remaining_distance: Option<f64>,
    callbacks_without_progress: u32,
    collision_count: usize,
    blocking_collision_count: usize,
    mc_char_on_move: bool,
    suppression_index: usize,
}

fn movement_display_position(
    value: Avm2Value<'_>,
) -> (Option<DisplayObjectDescriptor>, Option<MovementPoint>) {
    let Some(object) = value.as_object() else {
        return (None, None);
    };
    let Some(display) = object.as_display_object() else {
        return (None, None);
    };
    (
        Some(DisplayObjectDescriptor::from_display_object(display)),
        Some(MovementPoint {
            x: display.x().to_pixels(),
            y: display.y().to_pixels(),
        }),
    )
}

fn movement_display_path_and_position(
    value: Avm2Value<'_>,
) -> (Option<String>, Option<MovementPoint>) {
    let Some(display) = value
        .as_object()
        .and_then(|object| object.as_display_object())
    else {
        return (None, None);
    };
    let path = build_display_path(display);
    (
        (!path.is_empty()).then_some(path),
        Some(MovementPoint {
            x: display.x().to_pixels(),
            y: display.y().to_pixels(),
        }),
    )
}

fn movement_bounds(object: DisplayObject<'_>) -> MovementBounds {
    let bounds = object.world_bounds(BoundsMode::Script);
    MovementBounds {
        x_min: bounds.x_min.to_pixels(),
        y_min: bounds.y_min.to_pixels(),
        x_max: bounds.x_max.to_pixels(),
        y_max: bounds.y_max.to_pixels(),
    }
}

fn movement_collision_blocks_stop_guard(
    moving_bounds: MovementBounds,
    collider_bounds: MovementBounds,
    target: MovementPoint,
) -> bool {
    let center_x = (moving_bounds.x_min + moving_bounds.x_max) * 0.5;
    let center_y = (moving_bounds.y_min + moving_bounds.y_max) * 0.5;
    let direction_x = target.x - center_x;
    let direction_y = target.y - center_y;
    let values = [
        center_x,
        center_y,
        direction_x,
        direction_y,
        collider_bounds.x_min,
        collider_bounds.y_min,
        collider_bounds.x_max,
        collider_bounds.y_max,
    ];
    if values.iter().any(|value| !value.is_finite()) {
        return true;
    }

    let direction_length_squared = direction_x * direction_x + direction_y * direction_y;
    if !direction_length_squared.is_finite() || direction_length_squared <= f64::EPSILON {
        return true;
    }

    for (x, y) in [
        (collider_bounds.x_min, collider_bounds.y_min),
        (collider_bounds.x_min, collider_bounds.y_max),
        (collider_bounds.x_max, collider_bounds.y_min),
        (collider_bounds.x_max, collider_bounds.y_max),
    ] {
        let projection = (x - center_x) * direction_x + (y - center_y) * direction_y;
        if !projection.is_finite() || projection >= 0.0 {
            return true;
        }
    }

    false
}

pub struct MovementEnterFrameGuard {
    active: bool,
}

impl Drop for MovementEnterFrameGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = match movement_trace_runtime().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.exit_enter_frame();
    }
}

/// Mark execution of AQW's `AvatarMC.onEnterFrameWalk` so nested `stopWalking` calls can be
/// distinguished from intentional stops made by unrelated game code.
pub fn enter_movement_frame(receiver: Avm2Value<'_>) -> MovementEnterFrameGuard {
    let active = if movement_tracking_enabled() {
        let (receiver_path, receiver_position) = movement_display_path_and_position(receiver);
        let mut state = match movement_trace_runtime().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.enter_frame_if_owner(receiver_path.as_deref(), receiver_position)
    } else {
        false
    };
    MovementEnterFrameGuard { active }
}

pub struct MovementMouseMoveGuard {
    active: bool,
}

impl Drop for MovementMouseMoveGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = match movement_trace_runtime().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.exit_mouse_move();
    }
}

/// Keep AQW's global `Mouse.onMouseMove` callbacks from cancelling a live walk. The guard spans
/// both the global mouse listener and the rollover actions dispatched by the same input event;
/// mouse clicks and unrelated scripted stops remain outside this scope.
pub fn enter_movement_mouse_move() -> MovementMouseMoveGuard {
    let active = movement_tracking_enabled();
    if active {
        let mut state = match movement_trace_runtime().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.enter_mouse_move();
    }
    MovementMouseMoveGuard { active }
}

/// Start AQW's short delayed-callback grace period after the frame caused by a coalesced mouse
/// event has finished rendering. On crowded maps that render can take longer than the entire grace
/// period, allowing the next authored callback to cancel an otherwise valid walk.
pub fn finish_movement_mouse_move_render() {
    if !movement_tracking_enabled() {
        return;
    }

    let mut state = match movement_trace_runtime().lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.finish_mouse_move_render_at(Instant::now());
}

fn movement_stop_guard_should_suppress(
    remaining_distance: f64,
    walk_elapsed_ms: f64,
    expected_duration_ms: f64,
) -> bool {
    remaining_distance.is_finite()
        && walk_elapsed_ms.is_finite()
        && expected_duration_ms.is_finite()
        && remaining_distance > 2.0
        && walk_elapsed_ms > AQW_MOVEMENT_NO_PROGRESS_DELAY_MS
        && expected_duration_ms > 0.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovementStopGuardAction {
    Allow,
    Suppress,
    SuppressAndResume,
}

fn movement_stop_guard_action(
    mc_char_on_move: Option<bool>,
    stop_matches_guard: bool,
) -> MovementStopGuardAction {
    match (mc_char_on_move, stop_matches_guard) {
        (Some(true), true) => MovementStopGuardAction::Suppress,
        (Some(false), true) => MovementStopGuardAction::SuppressAndResume,
        _ => MovementStopGuardAction::Allow,
    }
}

/// Why the AQW premature-stop guard declined to suppress a `stopWalking` call.
///
/// The guard has several independent preconditions but previously emitted telemetry only when it
/// succeeded. A live capture could therefore not distinguish "the guard ran and disagreed" from
/// "the guard was never reachable", which is why repeated scope widenings could not be evaluated
/// against evidence. Each decline is attributed to its first failing gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum MovementStopGuardDecline {
    GuardDisabled,
    NoCharacterMoveFlag,
    NoReceiverPosition,
    NoActiveSequence,
    NotGuardEligible,
    ForeignReceiver,
    NoTarget,
    NoWalkStart,
    NotInGuardedCallback,
    StopLooksLegitimate,
}

impl MovementStopGuardDecline {
    const KINDS: usize = 10;

    fn index(self) -> usize {
        match self {
            Self::GuardDisabled => 0,
            Self::NoCharacterMoveFlag => 1,
            Self::NoReceiverPosition => 2,
            Self::NoActiveSequence => 3,
            Self::NotGuardEligible => 4,
            Self::ForeignReceiver => 5,
            Self::NoTarget => 6,
            Self::NoWalkStart => 7,
            Self::NotInGuardedCallback => 8,
            Self::StopLooksLegitimate => 9,
        }
    }
}

/// A populated map calls `stopWalking` for every visible avatar, so an unbounded decline record
/// would reproduce the trace-volume stalls this project already hit. One answer per reason is
/// enough to identify a blocking gate; this caps each reason for the whole session.
const MAX_MOVEMENT_STOP_GUARD_DECLINE_REPORTS: u8 = 32;

#[derive(Clone, Debug, Default)]
struct MovementStopGuardDeclineBudget {
    reported: [u8; MovementStopGuardDecline::KINDS],
}

impl MovementStopGuardDeclineBudget {
    fn admit(&mut self, reason: MovementStopGuardDecline) -> bool {
        let slot = &mut self.reported[reason.index()];
        if *slot >= MAX_MOVEMENT_STOP_GUARD_DECLINE_REPORTS {
            return false;
        }
        *slot += 1;
        true
    }
}

fn movement_stop_guard_decline_budget() -> &'static Mutex<MovementStopGuardDeclineBudget> {
    static BUDGET: OnceLock<Mutex<MovementStopGuardDeclineBudget>> = OnceLock::new();
    BUDGET.get_or_init(|| Mutex::new(MovementStopGuardDeclineBudget::default()))
}

#[derive(Serialize)]
struct MovementStopGuardDeclineRecord {
    record_type: &'static str,
    unix_time_ms: u128,
    elapsed_ms: f64,
    event: &'static str,
    sequence_id: u64,
    reason: MovementStopGuardDecline,
    receiver: Option<DisplayObjectDescriptor>,
    receiver_position: Option<MovementPoint>,
}

/// Record why the guard let a `stopWalking` through, then allow it.
///
/// Must not be called while the movement runtime lock is held; it takes its own budget lock.
fn movement_stop_guard_declined(
    reason: MovementStopGuardDecline,
    sequence_id: u64,
    receiver: Option<DisplayObjectDescriptor>,
    receiver_position: Option<MovementPoint>,
) -> MovementStopGuardAction {
    if input_trace_enabled() {
        let admitted = {
            let mut budget = match movement_stop_guard_decline_budget().lock() {
                Ok(budget) => budget,
                Err(poisoned) => poisoned.into_inner(),
            };
            budget.admit(reason)
        };
        if admitted {
            let record = MovementStopGuardDeclineRecord {
                record_type: "movement",
                unix_time_ms: unix_time_ms(),
                elapsed_ms: input_trace_elapsed_ms(),
                event: "stop_guard_declined",
                sequence_id,
                reason,
                receiver,
                receiver_position,
            };
            write_input_trace(&record);
        }
    }
    MovementStopGuardAction::Allow
}

/// Classify the callback-scope and suppression-predicate gates.
///
/// `NotInGuardedCallback` means AQW called `stopWalking` outside `onEnterFrameWalk`, outside a
/// live `Mouse.onMouseMove` dispatch, and outside the delayed-callback grace window that follows
/// one. `StopLooksLegitimate` means the guard was reachable and decided the stop was real.
fn movement_stop_guard_callback_decline(
    in_enter_frame: bool,
    in_mouse_move: bool,
    in_delayed_mouse_callback: bool,
    predicate_matched: bool,
) -> Option<MovementStopGuardDecline> {
    if !in_enter_frame && !in_mouse_move && !in_delayed_mouse_callback {
        return Some(MovementStopGuardDecline::NotInGuardedCallback);
    }
    if !predicate_matched {
        return Some(MovementStopGuardDecline::StopLooksLegitimate);
    }
    None
}

/// Classify the caller-supplied preconditions checked before any movement state is inspected.
///
/// `mc_char_on_move` is `None` when the receiver exposes no `mcChar`, or when that child's
/// dynamic `onMove` property cannot be read. That case disables the guard entirely and is
/// evaluated before every other gate, so it must be distinguishable in a capture.
fn movement_stop_guard_input_decline(
    guard_enabled: bool,
    mc_char_on_move: Option<bool>,
    receiver_position: Option<MovementPoint>,
) -> Option<MovementStopGuardDecline> {
    if !guard_enabled {
        return Some(MovementStopGuardDecline::GuardDisabled);
    }
    if mc_char_on_move.is_none() {
        return Some(MovementStopGuardDecline::NoCharacterMoveFlag);
    }
    if receiver_position.is_none() {
        return Some(MovementStopGuardDecline::NoReceiverPosition);
    }
    None
}

/// Classify the runtime-state preconditions the guard checks before it evaluates a stop.
///
/// Returns `None` when the sequence is fully armed for this receiver and the decision passes to
/// the suppression predicate.
fn movement_stop_guard_state_decline(
    state: &MovementTraceRuntime,
    receiver_path: Option<&str>,
) -> Option<MovementStopGuardDecline> {
    if !state.active {
        return Some(MovementStopGuardDecline::NoActiveSequence);
    }
    if !state.guard_eligible {
        return Some(MovementStopGuardDecline::NotGuardEligible);
    }
    if !movement_receiver_matches_owner(state.owner_path.as_deref(), receiver_path) {
        return Some(MovementStopGuardDecline::ForeignReceiver);
    }
    if state.target.is_none() {
        return Some(MovementStopGuardDecline::NoTarget);
    }
    if state.walk_started_at.is_none() {
        return Some(MovementStopGuardDecline::NoWalkStart);
    }
    None
}

fn movement_points_have_same_rounded_position(
    start: MovementPoint,
    current: MovementPoint,
) -> bool {
    if !start.x.is_finite()
        || !start.y.is_finite()
        || !current.x.is_finite()
        || !current.y.is_finite()
    {
        return false;
    }

    fn actionscript_round(value: f64) -> f64 {
        (value + 0.5).floor()
    }

    actionscript_round(start.x) == actionscript_round(current.x)
        && actionscript_round(start.y) == actionscript_round(current.y)
}

/// Return true only for the specific impossible early-stop state observed in AQW:
/// `stopWalking` is called from `onEnterFrameWalk`, a real `Mouse.onMouseMove` dispatch, or the
/// one-frame-delayed callback immediately following that dispatch while the avatar is still
/// materially short of its target. If the timeline child lost its dynamic `onMove` flag between
/// callbacks, the caller restores that flag before suppressing the stop. A bounded authored-
/// callback stall counter prevents an infinite guard without letting rendering delays consume the
/// movement budget.
///
/// A live collision deliberately does not disqualify the first matching call. AQW's collision
/// branch restores the old coordinates and calls `stopWalking`, then immediately executes the
/// same rounded no-progress check and calls `stopWalking` again. The per-callback latch allows
/// that second call only when the current callback observed a blocking collision, preserving the
/// genuine wall stop without allowing duplicate no-collision stops to bypass the guard. Treating
/// any positive collision test as a veto instead disabled the guard on dense maps even when AQW
/// had successfully resolved the collision along one axis.
pub fn movement_stop_guard_action_for_receiver(
    receiver: Avm2Value<'_>,
    mc_char_on_move: Option<bool>,
) -> MovementStopGuardAction {
    let (receiver_descriptor, receiver_position) = movement_display_position(receiver);

    if let Some(reason) = movement_stop_guard_input_decline(
        movement_stop_guard_enabled(),
        mc_char_on_move,
        receiver_position,
    ) {
        return movement_stop_guard_declined(reason, 0, receiver_descriptor, receiver_position);
    }
    let Some(receiver_position) = receiver_position else {
        return MovementStopGuardAction::Allow;
    };
    let receiver_path = receiver_descriptor
        .as_ref()
        .map(|descriptor| descriptor.display_path.as_str());

    // Each `Err` leaves the runtime lock before its record is written; the decline reporter takes
    // its own budget lock and would otherwise deadlock against this one.
    let decision = {
        let mut state = match movement_trace_runtime().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(reason) = movement_stop_guard_state_decline(&state, receiver_path) {
            Err((reason, state.sequence_id))
        } else {
            let Some(target) = state.target else {
                return MovementStopGuardAction::Allow;
            };
            let Some(walk_started_at) = state.walk_started_at else {
                return MovementStopGuardAction::Allow;
            };

            let remaining_distance =
                (target.x - receiver_position.x).hypot(target.y - receiver_position.y);
            let walk_elapsed_ms = walk_started_at.elapsed().as_secs_f64() * 1_000.0;

            // AQW's own completion test uses 0.5 px. Keep a wider distance margin. Wall-clock
            // time is diagnostic only: dense rendering and mouse input can delay authored
            // callbacks for seconds while the walk is still valid, so only callbacks without net
            // target progress consume the bounded stall budget.
            match state.try_record_suppressed_stop(
                receiver_position,
                remaining_distance,
                walk_elapsed_ms,
            ) {
                Some(suppression_index) => Ok((
                    state.sequence_id,
                    target,
                    remaining_distance,
                    walk_elapsed_ms,
                    state.expected_duration_ms,
                    state.best_remaining_distance,
                    state.callbacks_without_progress,
                    state.collision_count,
                    state.blocking_collision_count,
                    suppression_index,
                )),
                None => {
                    let (in_enter_frame, in_mouse_move, in_delayed_mouse_callback) =
                        state.guarded_callback_scope(Instant::now());
                    let reason = movement_stop_guard_callback_decline(
                        in_enter_frame,
                        in_mouse_move,
                        in_delayed_mouse_callback,
                        movement_stop_guard_should_suppress(
                            remaining_distance,
                            walk_elapsed_ms,
                            state.expected_duration_ms,
                        ),
                    )
                    .unwrap_or(MovementStopGuardDecline::StopLooksLegitimate);
                    Err((reason, state.sequence_id))
                }
            }
        }
    };

    let (
        sequence_id,
        target,
        remaining_distance,
        walk_elapsed_ms,
        expected_duration_ms,
        best_remaining_distance,
        callbacks_without_progress,
        collision_count,
        blocking_collision_count,
        suppression_index,
    ) = match decision {
        Ok(decision) => decision,
        Err((reason, sequence_id)) => {
            return movement_stop_guard_declined(
                reason,
                sequence_id,
                receiver_descriptor,
                Some(receiver_position),
            );
        }
    };

    if input_trace_enabled() {
        let record = MovementStopGuardRecord {
            record_type: "movement",
            unix_time_ms: unix_time_ms(),
            elapsed_ms: input_trace_elapsed_ms(),
            event: "premature_stop_suppressed",
            sequence_id,
            receiver: receiver_descriptor,
            receiver_position: Some(receiver_position),
            target,
            remaining_distance,
            walk_elapsed_ms,
            expected_duration_ms,
            best_remaining_distance,
            callbacks_without_progress,
            collision_count,
            blocking_collision_count,
            mc_char_on_move: mc_char_on_move.unwrap_or(false),
            suppression_index,
        };
        write_input_trace(&record);
    }

    movement_stop_guard_action(mc_char_on_move, true)
}

/// Start a traced AQW `AvatarMC.simulateTo` call.
///
/// The returned sequence id must be passed to `record_movement_simulate_end`.
pub fn record_movement_simulate_start(
    receiver: Avm2Value<'_>,
    requested_x: f64,
    requested_y: f64,
    speed: f64,
) -> u64 {
    if !movement_tracking_enabled() {
        return 0;
    }

    let (receiver_descriptor, receiver_position) = movement_display_position(receiver);
    let owner_path = receiver_descriptor
        .as_ref()
        .map(|descriptor| descriptor.display_path.clone())
        .filter(|path| !path.is_empty());
    let (sequence_id, collision_count) = {
        let mut state = match movement_trace_runtime().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.sequence_id = state.sequence_id.saturating_add(1);
        state.active = true;
        state.phase = "simulate";
        state.owner_path = owner_path;
        state.guard_eligible = state.owner_path.is_some();
        state.collision_count = 0;
        state.blocking_collision_count = 0;
        state.collision_overflow_reported = false;
        state.target = None;
        state.walk_started_at = None;
        state.expected_duration_ms = 0.0;
        state.enter_frame_depth = 0;
        state.enter_frame_start_position = None;
        state.enter_frame_stop_suppressed = false;
        state.suppressed_stop_count = 0;
        (state.sequence_id, state.collision_count)
    };

    if input_trace_enabled() {
        let record = MovementMethodRecord {
            record_type: "movement",
            unix_time_ms: unix_time_ms(),
            elapsed_ms: input_trace_elapsed_ms(),
            event: "simulate_start",
            sequence_id,
            phase: "simulate",
            receiver: receiver_descriptor,
            receiver_position,
            requested: Some(MovementPoint {
                x: requested_x,
                y: requested_y,
            }),
            target: None,
            speed: Some(speed),
            result: None,
            collision_count,
        };
        write_input_trace(&record);
    }
    sequence_id
}

pub fn record_movement_simulate_end(sequence_id: u64, result: &'static str) {
    if !movement_tracking_enabled() || sequence_id == 0 {
        return;
    }

    let collision_count = {
        let mut state = match movement_trace_runtime().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.sequence_id != sequence_id {
            return;
        }
        state.phase = "await_walk";
        state.collision_count
    };

    if input_trace_enabled() {
        let record = MovementMethodRecord {
            record_type: "movement",
            unix_time_ms: unix_time_ms(),
            elapsed_ms: input_trace_elapsed_ms(),
            event: "simulate_end",
            sequence_id,
            phase: "await_walk",
            receiver: None,
            receiver_position: None,
            requested: None,
            target: None,
            speed: None,
            result: Some(result),
            collision_count,
        };
        write_input_trace(&record);
    }
}

pub fn record_movement_walk_to(receiver: Avm2Value<'_>, target_x: f64, target_y: f64, speed: f64) {
    if !movement_tracking_enabled() {
        return;
    }

    let (receiver_descriptor, receiver_position) = movement_display_position(receiver);
    let target = MovementPoint {
        x: target_x,
        y: target_y,
    };
    let expected_duration_ms = receiver_position.map_or(0.0, |position| {
        if speed > 0.0 {
            (1_000.0 * (target.x - position.x).hypot(target.y - position.y) / (speed * 22.0))
                .round()
        } else {
            0.0
        }
    });
    let receiver_path = receiver_descriptor
        .as_ref()
        .map(|descriptor| descriptor.display_path.as_str());

    let (sequence_id, phase, collision_count, disposition) = {
        let mut state = match movement_trace_runtime().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        // `simulateTo` may legitimately collide while choosing a safe endpoint. Those collisions
        // must not disable the guard for the subsequent live walk; any real walk collision will
        // increment this counter again before `stopWalking` is evaluated.
        let disposition =
            state.begin_or_update_walk(receiver_path, target, expected_duration_ms, Instant::now());
        (
            state.sequence_id,
            state.phase,
            state.collision_count,
            disposition,
        )
    };

    if input_trace_enabled() {
        let record = MovementMethodRecord {
            record_type: "movement",
            unix_time_ms: unix_time_ms(),
            elapsed_ms: input_trace_elapsed_ms(),
            event: if disposition == MovementWalkDisposition::Foreign {
                "foreign_walk_ignored"
            } else {
                "walk_to"
            },
            sequence_id,
            phase,
            receiver: receiver_descriptor,
            receiver_position,
            requested: None,
            target: Some(target),
            speed: Some(speed),
            result: None,
            collision_count,
        };
        write_input_trace(&record);
    }
}

pub fn record_movement_stop(receiver: Avm2Value<'_>) {
    if !movement_tracking_enabled() {
        return;
    }

    let (receiver_descriptor, receiver_position) = movement_display_position(receiver);
    let receiver_path = receiver_descriptor
        .as_ref()
        .map(|descriptor| descriptor.display_path.as_str());
    let (sequence_id, phase, collision_count, owner_stop) = {
        let mut state = match movement_trace_runtime().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !state.active {
            return;
        }
        let values = (
            state.sequence_id,
            state.phase,
            state.collision_count,
            movement_receiver_matches_owner(state.owner_path.as_deref(), receiver_path),
        );
        if values.3 {
            state.finish_if_owner(receiver_path);
        }
        values
    };

    if input_trace_enabled() {
        let record = MovementMethodRecord {
            record_type: "movement",
            unix_time_ms: unix_time_ms(),
            elapsed_ms: input_trace_elapsed_ms(),
            event: if owner_stop {
                "stop_walking"
            } else {
                "foreign_stop_ignored"
            },
            sequence_id,
            phase,
            receiver: receiver_descriptor,
            receiver_position,
            requested: None,
            target: None,
            speed: None,
            result: None,
            collision_count,
        };
        write_input_trace(&record);
    }
}

/// Record positive `hitTestObject` results while an AQW movement simulation or walk is active.
///
/// This intentionally records only intersections, not every negative test. A single click may
/// test hundreds of solid regions.
pub fn record_movement_collision(moving_object: DisplayObject<'_>, collider: DisplayObject<'_>) {
    if !movement_tracking_enabled() {
        return;
    }

    // Positive hit tests are common in combat even when the avatar is not walking. Avoid display
    // path construction and world-bounds traversal unless a movement sequence is actually active.
    {
        let state = match movement_trace_runtime().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !state.active {
            return;
        }
    }

    let trace_enabled = input_trace_enabled();
    let moving_path = build_display_path(moving_object);
    {
        let state = match movement_trace_runtime().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !state.active
            || !movement_collision_needs_geometry(
                state.owner_path.as_deref(),
                &moving_path,
                trace_enabled,
            )
        {
            return;
        }
    }

    let moving_bounds = movement_bounds(moving_object);
    let collider_bounds = movement_bounds(collider);

    let (sequence_id, phase, collision_index, blocks_stop_guard, overflow, foreign) = {
        let mut state = match movement_trace_runtime().lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !state.active {
            return;
        }
        let sequence_id = state.sequence_id;
        let phase = state.phase;
        match state.record_collision_if_owner(&moving_path, moving_bounds, collider_bounds) {
            Some((collision_index, blocks_stop_guard, overflow)) => (
                sequence_id,
                phase,
                collision_index,
                blocks_stop_guard,
                overflow,
                false,
            ),
            None => (
                state.sequence_id,
                state.phase,
                state.collision_count,
                false,
                false,
                true,
            ),
        }
    };

    if !input_trace_enabled() {
        return;
    }

    if foreign {
        let record = MovementCollisionRecord {
            record_type: "movement",
            unix_time_ms: unix_time_ms(),
            elapsed_ms: input_trace_elapsed_ms(),
            event: "foreign_collision_ignored",
            sequence_id,
            phase,
            collision_index,
            moving_object: DisplayObjectDescriptor::from_display_object(moving_object),
            moving_bounds,
            collider: DisplayObjectDescriptor::from_display_object(collider),
            collider_bounds,
            blocks_stop_guard: false,
        };
        write_input_trace(&record);
        return;
    }

    if collision_index > MAX_MOVEMENT_COLLISIONS_PER_SEQUENCE {
        if overflow {
            let record = MovementMethodRecord {
                record_type: "movement",
                unix_time_ms: unix_time_ms(),
                elapsed_ms: input_trace_elapsed_ms(),
                event: "collision_limit_reached",
                sequence_id,
                phase,
                receiver: None,
                receiver_position: None,
                requested: None,
                target: None,
                speed: None,
                result: None,
                collision_count: collision_index,
            };
            write_input_trace(&record);
        }
        return;
    }

    let record = MovementCollisionRecord {
        record_type: "movement",
        unix_time_ms: unix_time_ms(),
        elapsed_ms: input_trace_elapsed_ms(),
        event: "collision",
        sequence_id,
        phase,
        collision_index,
        moving_object: DisplayObjectDescriptor::from_display_object(moving_object),
        moving_bounds,
        collider: DisplayObjectDescriptor::from_display_object(collider),
        collider_bounds,
        blocks_stop_guard,
    };
    write_input_trace(&record);
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod blend_attribution_tests {
    use super::*;

    /// The symbol identity the renderer sees. `class_name` is not part of it; it rides along on the
    /// identity resolved when the site is first recorded, which is what `named` supplies.
    fn site(character_id: u16) -> BlendSiteKey {
        BlendSiteKey {
            blend_mode: "multiply",
            movie: 0xA9_1234,
            character_id,
        }
    }

    fn named(class_name: &str) -> impl FnOnce() -> BlendSiteIdentity + use<'_> {
        move || BlendSiteIdentity {
            actor_classification: "remote_player",
            class_name: class_name.to_string(),
            display_path: "stage#0/root1/avatar#3/armor".to_string(),
            movie_url: "https://game.aq.com/game/gamefiles/spider.swf".to_string(),
        }
    }

    fn observation(pixel_area: u64, sub_commands: u64) -> BlendObservation {
        BlendObservation {
            instance_key: 1,
            pixel_area,
            sub_commands,
            sole_command_kind: "bitmap",
        }
    }

    /// The question this instrumentation exists to answer is "which symbol issues the blends", and
    /// a crowded map draws the same armour layer once per player. Fifteen players sharing one
    /// symbol have to read as one site worth fifteen blends, not as fifteen sites worth one.
    #[test]
    fn blends_from_one_symbol_aggregate_across_the_instances_that_draw_it() {
        let mut state = BlendAttributionState::default();

        for instance in 0..15u64 {
            state.record(
                site(42),
                BlendObservation {
                    instance_key: instance,
                    ..observation(10_000, 3)
                },
                named("liteAssets.draw::armorLayer"),
            );
        }

        let report = state.take_report(8);

        assert_eq!(report.blends, 15);
        assert_eq!(report.unique_sites, 1, "one symbol is one site");
        assert_eq!(report.top.len(), 1);
        assert_eq!(report.top[0].blends, 15);
        assert_eq!(report.top[0].instances, 15, "but fifteen objects drew it");
        assert_eq!(report.top[0].total_pixel_area, 150_000);
        assert_eq!(report.top[0].sub_commands, 45);
    }

    /// Ranking by blend count alone would put a swarm of tiny sparks above the one full-screen
    /// layer that actually costs the frame, so the table is ordered by the area it composites.
    #[test]
    fn sites_are_ranked_by_composited_area_rather_than_by_count() {
        let mut state = BlendAttributionState::default();

        for _ in 0..50 {
            state.record(site(1), observation(100, 1), named("spark"));
        }
        state.record(site(2), observation(480_000, 1), named("fullscreen_tint"));

        let report = state.take_report(8);

        assert_eq!(report.top[0].class_name, "fullscreen_tint");
        assert_eq!(report.top[1].class_name, "spark");
        assert_eq!(report.blends, 51);
    }

    /// The report is read as "what did the last second cost", so counts must not carry over.
    #[test]
    fn taking_the_report_clears_the_period_it_covered() {
        let mut state = BlendAttributionState::default();
        state.record(site(1), observation(10, 1), named("armor"));

        assert_eq!(state.take_report(8).blends, 1);
        assert_eq!(
            state.take_report(8).blends,
            0,
            "a second read of the same period must report nothing"
        );
    }

    /// Mode and actor totals are the first thing read off the report, before any single site.
    #[test]
    fn report_totals_split_blends_by_mode_and_by_actor() {
        let mut state = BlendAttributionState::default();

        state.record(site(1), observation(10, 1), named("armor"));
        state.record(
            BlendSiteKey {
                blend_mode: "overlay",
                ..site(2)
            },
            observation(10, 1),
            named("armor_highlight"),
        );
        state.record(
            BlendSiteKey {
                blend_mode: "overlay",
                ..site(3)
            },
            observation(10, 1),
            || BlendSiteIdentity {
                actor_classification: "map",
                ..named("map_shade")()
            },
        );

        let report = state.take_report(8);

        assert_eq!(report.by_mode.get("multiply"), Some(&1));
        assert_eq!(report.by_mode.get("overlay"), Some(&2));
        assert_eq!(report.by_actor.get("remote_player"), Some(&2));
        assert_eq!(report.by_actor.get("map"), Some(&1));
    }

    /// A map that grew without bound would be the diagnostic leaking on the very frames it is
    /// meant to describe, so new sites stop being admitted once the table is full.
    #[test]
    fn the_site_table_stops_admitting_new_symbols_once_it_is_full() {
        let mut state = BlendAttributionState::default();

        for id in 0..(MAX_BLEND_SITES + 32) {
            state.record(site(id as u16), observation(10, 1), named("armor"));
        }

        assert_eq!(state.sites.len(), MAX_BLEND_SITES);
        let report = state.take_report(8);
        assert_eq!(
            report.blends, MAX_BLEND_SITES as u64,
            "blends past the cap are dropped rather than silently merged into another site"
        );
    }

    /// Everything above is off unless the flag is set, so the release path pays nothing.
    #[test]
    fn attribution_is_disabled_until_it_is_switched_on() {
        assert!(!blend_attribution_enabled());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_to_starts_live_collision_accounting_at_zero() {
        let owner = "stage#0/game/avatar";
        let mut state = MovementTraceRuntime {
            sequence_id: 7,
            active: true,
            phase: "await_walk",
            owner_path: Some(owner.to_string()),
            guard_eligible: true,
            collision_count: 6,
            blocking_collision_count: 4,
            ..MovementTraceRuntime::default()
        };
        let disposition = state.begin_or_update_walk(
            Some(owner),
            MovementPoint { x: 349.0, y: 272.9 },
            1_000.0,
            Instant::now(),
        );

        assert_eq!(disposition, MovementWalkDisposition::Owner);
        assert_eq!(
            (state.collision_count, state.blocking_collision_count),
            (0, 0),
            "simulateTo collisions must not carry into live-walk collision accounting"
        );
    }

    #[test]
    fn stop_guard_keeps_suppressing_no_collision_stops_near_expected_time() {
        assert!(
            movement_stop_guard_should_suppress(12.82, 1941.0, 2014.0),
            "sequence 1 stopped 12.82px short 73ms before expected completion"
        );
        assert!(
            movement_stop_guard_should_suppress(9.69, 1976.0, 2031.0),
            "sequence 6 stopped 9.69px short 55ms before expected completion"
        );
        assert!(
            !movement_stop_guard_should_suppress(2.0, 1976.0, 2031.0),
            "near-target completion must still be allowed"
        );
        assert!(
            movement_stop_guard_should_suppress(12.82, 2300.0, 2014.0),
            "wall-clock rendering delay must not expire a progressing walk"
        );
    }

    #[test]
    fn stop_guard_rearms_an_owner_walk_when_on_move_was_lost() {
        assert_eq!(
            movement_stop_guard_action(Some(false), true),
            MovementStopGuardAction::SuppressAndResume
        );
    }

    #[test]
    fn stop_guard_starts_only_after_aqw_no_progress_window() {
        assert!(
            !movement_stop_guard_should_suppress(50.0, 50.0, 1_000.0),
            "AQW cannot execute its rounded no-progress stop during the first 50ms"
        );
        assert!(
            movement_stop_guard_should_suppress(50.0, 50.001, 1_000.0),
            "the exact AQW no-progress branch becomes eligible after 50ms"
        );
    }

    #[test]
    fn stop_guard_rejects_non_finite_distance_and_timing() {
        assert!(!movement_stop_guard_should_suppress(
            f64::INFINITY,
            100.0,
            1_000.0
        ));
        assert!(!movement_stop_guard_should_suppress(
            50.0,
            f64::INFINITY,
            1_000.0
        ));
        assert!(!movement_stop_guard_should_suppress(
            50.0,
            100.0,
            f64::INFINITY
        ));
    }

    #[test]
    fn live_collision_wholly_behind_leftward_walk_does_not_block_guard() {
        assert!(!movement_collision_blocks_stop_guard(
            MovementBounds {
                x_min: 862.6,
                y_min: 396.15,
                x_max: 909.6,
                y_max: 411.15,
            },
            MovementBounds {
                x_min: 907.3,
                y_min: 305.55,
                x_max: 960.2,
                y_max: 499.25,
            },
            MovementPoint { x: 195.0, y: 438.0 },
        ));
    }

    #[test]
    fn live_collision_ahead_of_rightward_walk_blocks_guard() {
        assert!(movement_collision_blocks_stop_guard(
            MovementBounds {
                x_min: 862.6,
                y_min: 396.15,
                x_max: 909.6,
                y_max: 411.15,
            },
            MovementBounds {
                x_min: 907.3,
                y_min: 305.55,
                x_max: 960.2,
                y_max: 499.25,
            },
            MovementPoint { x: 950.0, y: 402.0 },
        ));
    }

    #[test]
    fn diagonal_collision_wholly_behind_walk_does_not_block_guard() {
        assert!(!movement_collision_blocks_stop_guard(
            MovementBounds {
                x_min: 0.0,
                y_min: 0.0,
                x_max: 2.0,
                y_max: 2.0,
            },
            MovementBounds {
                x_min: -5.0,
                y_min: -5.0,
                x_max: -2.0,
                y_max: -2.0,
            },
            MovementPoint { x: 10.0, y: 10.0 },
        ));
    }

    #[test]
    fn collision_straddling_movement_plane_blocks_guard() {
        assert!(movement_collision_blocks_stop_guard(
            MovementBounds {
                x_min: 0.0,
                y_min: 0.0,
                x_max: 2.0,
                y_max: 2.0,
            },
            MovementBounds {
                x_min: 0.0,
                y_min: 0.0,
                x_max: 2.0,
                y_max: 2.0,
            },
            MovementPoint { x: 10.0, y: 1.0 },
        ));
    }

    #[test]
    fn zero_length_movement_blocks_guard() {
        assert!(movement_collision_blocks_stop_guard(
            MovementBounds {
                x_min: 0.0,
                y_min: 0.0,
                x_max: 2.0,
                y_max: 2.0,
            },
            MovementBounds {
                x_min: -5.0,
                y_min: -5.0,
                x_max: -2.0,
                y_max: -2.0,
            },
            MovementPoint { x: 1.0, y: 1.0 },
        ));
    }

    #[test]
    fn non_finite_collision_geometry_blocks_guard() {
        assert!(movement_collision_blocks_stop_guard(
            MovementBounds {
                x_min: 0.0,
                y_min: 0.0,
                x_max: 2.0,
                y_max: 2.0,
            },
            MovementBounds {
                x_min: f64::NAN,
                y_min: -5.0,
                x_max: -2.0,
                y_max: -2.0,
            },
            MovementPoint { x: 10.0, y: 10.0 },
        ));
    }

    const V6_OWNER_PATH: &str = "stage#0/instance51/instance2567/instance2571/a105661";
    const V6_FOREIGN_PATH: &str = "stage#0/instance51/instance2567/instance2571/a105655";

    #[test]
    fn movement_owner_requires_an_exact_non_empty_receiver_path() {
        assert!(movement_receiver_matches_owner(
            Some(V6_OWNER_PATH),
            Some(V6_OWNER_PATH)
        ));
        assert!(!movement_receiver_matches_owner(
            Some(V6_OWNER_PATH),
            Some(V6_FOREIGN_PATH)
        ));
        assert!(!movement_receiver_matches_owner(None, Some(V6_OWNER_PATH)));
        assert!(!movement_receiver_matches_owner(Some(""), Some("")));
    }

    #[test]
    fn movement_collision_requires_a_slash_boundary_descendant() {
        assert!(movement_object_belongs_to_owner(
            Some(V6_OWNER_PATH),
            &format!("{V6_OWNER_PATH}/shadow")
        ));
        assert!(!movement_object_belongs_to_owner(
            Some(V6_OWNER_PATH),
            &format!("{V6_OWNER_PATH}2/shadow")
        ));
        assert!(!movement_object_belongs_to_owner(
            None,
            &format!("{V6_OWNER_PATH}/shadow")
        ));
    }

    #[test]
    fn collision_geometry_is_skipped_for_untraced_foreign_movers() {
        assert!(movement_collision_needs_geometry(
            Some(V6_OWNER_PATH),
            &format!("{V6_OWNER_PATH}/shadow"),
            false,
        ));
        assert!(!movement_collision_needs_geometry(
            Some(V6_OWNER_PATH),
            &format!("{V6_FOREIGN_PATH}/shadow"),
            false,
        ));
        assert!(movement_collision_needs_geometry(
            Some(V6_OWNER_PATH),
            &format!("{V6_FOREIGN_PATH}/shadow"),
            true,
        ));
    }

    fn owned_v6_runtime() -> MovementTraceRuntime {
        MovementTraceRuntime {
            sequence_id: 4,
            active: true,
            phase: "walk",
            owner_path: Some(V6_OWNER_PATH.to_string()),
            guard_eligible: true,
            collision_count: 2,
            blocking_collision_count: 1,
            target: Some(MovementPoint { x: 810.0, y: 409.0 }),
            walk_started_at: Some(Instant::now()),
            expected_duration_ms: 3_861.0,
            ..MovementTraceRuntime::default()
        }
    }

    #[test]
    fn foreign_walk_cannot_replace_owner_target_or_timing() {
        let mut state = owned_v6_runtime();
        let previous_start = state.walk_started_at;
        let disposition = state.begin_or_update_walk(
            Some(V6_FOREIGN_PATH),
            MovementPoint { x: 683.0, y: 476.0 },
            777.0,
            Instant::now(),
        );

        assert_eq!(disposition, MovementWalkDisposition::Foreign);
        let target = state.target.expect("owner target must remain");
        assert_eq!((target.x, target.y), (810.0, 409.0));
        assert_eq!(state.expected_duration_ms, 3_861.0);
        assert_eq!(state.walk_started_at, previous_start);
        assert_eq!(
            (state.collision_count, state.blocking_collision_count),
            (2, 1)
        );
    }

    #[test]
    fn foreign_stop_cannot_deactivate_owner_sequence() {
        let mut state = owned_v6_runtime();
        assert!(!state.finish_if_owner(Some(V6_FOREIGN_PATH)));
        assert!(state.active);
        assert_eq!(state.phase, "walk");
        assert!(state.target.is_some());
    }

    #[test]
    fn foreign_frame_cannot_increment_owner_depth() {
        let mut state = owned_v6_runtime();
        let position = Some(MovementPoint { x: 640.0, y: 470.0 });
        assert!(!state.enter_frame_if_owner(Some(V6_FOREIGN_PATH), position));
        assert_eq!(state.enter_frame_depth, 0);
        assert!(state.enter_frame_if_owner(Some(V6_OWNER_PATH), position));
        assert_eq!(state.enter_frame_depth, 1);
    }

    #[test]
    fn completed_collision_does_not_block_a_later_movement_callback() {
        let mut state = owned_v6_runtime();
        assert_eq!(state.blocking_collision_count, 1);

        assert!(state.enter_frame_if_owner(
            Some(V6_OWNER_PATH),
            Some(MovementPoint { x: 640.0, y: 470.0 }),
        ));

        assert_eq!(
            state.blocking_collision_count, 0,
            "only a collision in the current movement callback may authorize its stop"
        );
    }

    #[test]
    fn foreign_collision_cannot_change_owner_counters() {
        let mut state = owned_v6_runtime();
        let moving_bounds = MovementBounds {
            x_min: 659.5,
            y_min: 468.5,
            x_max: 706.5,
            y_max: 483.5,
        };
        let collider_bounds = MovementBounds {
            x_min: 82.05,
            y_min: 459.0,
            x_max: 891.95,
            y_max: 500.0,
        };

        assert!(
            state
                .record_collision_if_owner(
                    &format!("{V6_FOREIGN_PATH}/shadow"),
                    moving_bounds,
                    collider_bounds,
                )
                .is_none()
        );
        assert_eq!(
            (state.collision_count, state.blocking_collision_count),
            (2, 1)
        );
    }

    #[test]
    fn standalone_walk_is_traceable_but_guard_ineligible() {
        let mut state = MovementTraceRuntime::default();
        let disposition = state.begin_or_update_walk(
            Some(V6_FOREIGN_PATH),
            MovementPoint { x: 689.0, y: 474.0 },
            1_000.0,
            Instant::now(),
        );

        assert_eq!(disposition, MovementWalkDisposition::Standalone);
        assert!(state.active);
        assert_eq!(state.owner_path.as_deref(), Some(V6_FOREIGN_PATH));
        assert!(!state.guard_eligible);
    }

    #[test]
    fn stop_guard_survives_wall_clock_stalls_while_authored_callbacks_can_progress() {
        let owner = "stage#0/game/avatar";
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            target: Some(MovementPoint { x: 150.0, y: 200.0 }),
            expected_duration_ms: 1_000.0,
            ..MovementTraceRuntime::default()
        };

        assert!(
            state.enter_frame_if_owner(Some(owner), Some(MovementPoint { x: 100.0, y: 200.0 }))
        );
        assert_eq!(
            state.try_record_suppressed_stop(MovementPoint { x: 100.4, y: 200.4 }, 50.0, 10_000.0,),
            Some(1),
            "render stalls must not expire a collision-free walk while callback progress is still possible"
        );
        state.exit_enter_frame();
    }

    #[test]
    fn stop_guard_eventually_allows_a_true_callback_stall() {
        let owner = "stage#0/game/avatar";
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            target: Some(MovementPoint { x: 150.0, y: 200.0 }),
            expected_duration_ms: 1_000.0,
            ..MovementTraceRuntime::default()
        };

        for callback in 0..120 {
            assert!(
                state.enter_frame_if_owner(Some(owner), Some(MovementPoint { x: 100.0, y: 200.0 }))
            );
            let decision =
                state.try_record_suppressed_stop(MovementPoint { x: 100.4, y: 200.4 }, 50.0, 100.0);
            state.exit_enter_frame();
            if callback < 119 {
                assert!(decision.is_some());
            } else {
                assert_eq!(
                    decision, None,
                    "120 authored callbacks without net progress is a real stall"
                );
            }
        }
    }

    #[test]
    fn stop_guard_suppresses_duplicate_no_progress_stops_without_a_collision() {
        let owner = "stage#0/game/avatar";
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            expected_duration_ms: 1_000.0,
            ..MovementTraceRuntime::default()
        };
        assert!(
            state.enter_frame_if_owner(Some(owner), Some(MovementPoint { x: 100.0, y: 200.0 }))
        );

        let current = MovementPoint { x: 100.4, y: 200.4 };
        assert_eq!(
            state.try_record_suppressed_stop(current, 50.0, 100.0),
            Some(1)
        );
        assert_eq!(
            state.try_record_suppressed_stop(current, 50.0, 100.0),
            Some(2),
            "duplicate no-progress stops without a collision must not bypass the guard"
        );
        state.exit_enter_frame();
    }

    #[test]
    fn stop_guard_suppresses_cursor_move_callbacks_only_during_mouse_dispatch() {
        let owner = "stage#0/game/avatar";
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            target: Some(MovementPoint { x: 150.0, y: 200.0 }),
            expected_duration_ms: 1_000.0,
            ..MovementTraceRuntime::default()
        };
        let current = MovementPoint { x: 100.0, y: 200.0 };
        let now = Instant::now();

        assert_eq!(
            state.try_record_suppressed_stop(current, 50.0, 100.0),
            None,
            "an unrelated scripted stop outside an authored movement or mouse callback must pass through"
        );

        state.enter_mouse_move_at(now);
        assert_eq!(
            state.try_record_suppressed_stop(current, 50.0, 100.0),
            Some(1),
            "AQW's global Mouse.onMouseMove callback must not cancel a live walk"
        );
        assert_eq!(
            state.try_record_suppressed_stop(current, 50.0, 100.0),
            Some(2),
            "multiple cursor callbacks in the same dispatch must remain harmless"
        );
        state.exit_mouse_move();

        assert_eq!(
            state.try_record_suppressed_stop_at(
                current,
                50.0,
                100.0,
                now + AQW_DELAYED_MOUSE_STOP_GRACE + Duration::from_millis(1),
            ),
            None,
            "the cursor-specific guard must end after the delayed callback grace window"
        );
    }

    #[test]
    fn stop_guard_covers_the_one_frame_delayed_mouse_callback_seen_in_aqw() {
        let owner = "stage#0/game/avatar";
        let now = Instant::now();
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            target: Some(MovementPoint { x: 500.0, y: 200.0 }),
            expected_duration_ms: 1_000.0,
            ..MovementTraceRuntime::default()
        };
        let current = MovementPoint { x: 100.0, y: 200.0 };

        state.enter_mouse_move_at(now);
        state.exit_mouse_move();

        assert_eq!(
            state.try_record_suppressed_stop_at(
                current,
                400.0,
                125.0,
                now + Duration::from_millis(16),
            ),
            Some(1),
            "captured premature stops arrived 14-16ms after the preceding MouseMove callback"
        );
    }

    #[test]
    fn stop_guard_covers_a_delayed_mouse_callback_at_low_frame_rates() {
        let owner = "stage#0/game/avatar";
        let now = Instant::now();
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            target: Some(MovementPoint { x: 500.0, y: 200.0 }),
            expected_duration_ms: 1_000.0,
            ..MovementTraceRuntime::default()
        };
        let current = MovementPoint { x: 100.0, y: 200.0 };

        state.enter_mouse_move_at(now);
        state.exit_mouse_move();

        assert_eq!(
            state.try_record_suppressed_stop_at(
                current,
                400.0,
                200.0,
                now + Duration::from_millis(100),
            ),
            Some(1),
            "a 10 FPS frame must not let AQW's delayed mouse-triggered stop escape the guard"
        );
    }

    #[test]
    fn stop_guard_grace_starts_after_a_slow_mouse_triggered_render() {
        let owner = "stage#0/game/avatar";
        let mouse_dispatched_at = Instant::now();
        let render_finished_at = mouse_dispatched_at + Duration::from_millis(400);
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            target: Some(MovementPoint { x: 500.0, y: 200.0 }),
            expected_duration_ms: 1_000.0,
            ..MovementTraceRuntime::default()
        };
        let current = MovementPoint { x: 100.0, y: 200.0 };

        state.enter_mouse_move_at(mouse_dispatched_at);
        state.exit_mouse_move();
        state.finish_mouse_move_render_at(render_finished_at);

        assert_eq!(
            state.try_record_suppressed_stop_at(
                current,
                400.0,
                500.0,
                render_finished_at + Duration::from_millis(100),
            ),
            Some(1),
            "a crowded frame must not consume the delayed mouse-stop grace before AQW can run its follow-up callback"
        );
    }

    #[test]
    fn stop_guard_does_not_claim_unrelated_stops_after_mouse_grace_expires() {
        let owner = "stage#0/game/avatar";
        let now = Instant::now();
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            target: Some(MovementPoint { x: 500.0, y: 200.0 }),
            expected_duration_ms: 1_000.0,
            ..MovementTraceRuntime::default()
        };
        let current = MovementPoint { x: 100.0, y: 200.0 };

        state.enter_mouse_move_at(now);
        state.exit_mouse_move();

        assert_eq!(
            state.try_record_suppressed_stop_at(
                current,
                400.0,
                250.0,
                now + AQW_DELAYED_MOUSE_STOP_GRACE + Duration::from_millis(1),
            ),
            None
        );
    }

    #[test]
    fn delayed_mouse_guard_does_not_override_a_blocking_collision() {
        let owner = "stage#0/game/avatar";
        let now = Instant::now();
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            target: Some(MovementPoint { x: 500.0, y: 200.0 }),
            expected_duration_ms: 1_000.0,
            blocking_collision_count: 1,
            ..MovementTraceRuntime::default()
        };

        state.enter_mouse_move_at(now);
        state.exit_mouse_move();

        assert_eq!(
            state.try_record_suppressed_stop_at(
                MovementPoint { x: 100.0, y: 200.0 },
                400.0,
                125.0,
                now + Duration::from_millis(16),
            ),
            None
        );
    }

    #[test]
    fn stop_guard_preserves_a_real_collision_through_the_second_stop_call() {
        let owner = "stage#0/game/avatar";
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            expected_duration_ms: 1_000.0,
            ..MovementTraceRuntime::default()
        };
        assert!(
            state.enter_frame_if_owner(Some(owner), Some(MovementPoint { x: 100.0, y: 200.0 }))
        );
        state.blocking_collision_count = 1;

        let current = MovementPoint { x: 100.4, y: 200.4 };
        assert_eq!(
            state.try_record_suppressed_stop(current, 50.0, 100.0),
            Some(1),
            "AQW's collision branch must reach the per-frame latch instead of disabling the guard"
        );
        assert_eq!(
            state.try_record_suppressed_stop(current, 50.0, 100.0),
            None,
            "the immediately following no-progress stop must pass through and preserve the wall"
        );
        state.exit_enter_frame();
    }

    #[test]
    fn stop_guard_requires_the_aqw_rounded_no_progress_signature() {
        let owner = "stage#0/game/avatar";
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            expected_duration_ms: 1_000.0,
            ..MovementTraceRuntime::default()
        };
        assert!(
            state.enter_frame_if_owner(Some(owner), Some(MovementPoint { x: 100.0, y: 200.0 }))
        );

        assert_eq!(
            state.try_record_suppressed_stop(MovementPoint { x: 101.0, y: 200.0 }, 50.0, 100.0,),
            None,
            "a stop after visible rounded movement is not AQW's no-progress branch"
        );
        state.exit_enter_frame();
    }

    #[test]
    fn stop_guard_uses_actionscript_rounding_for_negative_half_pixels() {
        assert!(movement_points_have_same_rounded_position(
            MovementPoint { x: -1.5, y: -2.5 },
            MovementPoint { x: -1.49, y: -2.49 },
        ));
    }

    #[test]
    fn stop_guard_rejects_non_finite_positions() {
        assert!(!movement_points_have_same_rounded_position(
            MovementPoint {
                x: f64::INFINITY,
                y: 0.0,
            },
            MovementPoint {
                x: f64::INFINITY,
                y: 0.0,
            },
        ));
    }

    #[test]
    fn nested_movement_frame_preserves_outer_snapshot_and_latch() {
        let owner = "stage#0/game/avatar";
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            expected_duration_ms: 1_000.0,
            ..MovementTraceRuntime::default()
        };
        assert!(
            state.enter_frame_if_owner(Some(owner), Some(MovementPoint { x: 100.0, y: 200.0 }))
        );
        assert!(
            state.enter_frame_if_owner(Some(owner), Some(MovementPoint { x: 140.0, y: 240.0 }))
        );
        assert_eq!(
            state.try_record_suppressed_stop(MovementPoint { x: 100.4, y: 200.4 }, 50.0, 100.0,),
            Some(1),
            "nested entry must not replace the outer callback's no-progress snapshot"
        );
        state.exit_enter_frame();
        assert!(state.enter_frame_stop_suppressed);
        state.exit_enter_frame();
        assert!(state.enter_frame_start_position.is_none());
        assert!(!state.enter_frame_stop_suppressed);
    }

    #[test]
    fn finishing_owner_walk_clears_per_frame_guard_state() {
        let owner = "stage#0/game/avatar";
        let mut state = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            expected_duration_ms: 1_000.0,
            ..MovementTraceRuntime::default()
        };
        assert!(
            state.enter_frame_if_owner(Some(owner), Some(MovementPoint { x: 100.0, y: 200.0 }))
        );
        assert!(state.finish_if_owner(Some(owner)));
        assert_eq!(state.enter_frame_depth, 0);
        assert!(state.enter_frame_start_position.is_none());
        assert!(!state.enter_frame_stop_suppressed);

        // The active RAII guard may still drop after stopWalking finishes; this must stay cleared.
        state.exit_enter_frame();
        assert_eq!(state.enter_frame_depth, 0);
        assert!(state.enter_frame_start_position.is_none());
    }

    fn handler_descriptor(name: &str) -> EnterFrameHandlerDescriptor {
        EnterFrameHandlerDescriptor {
            qualified_name: name.to_string(),
            movie_url: "https://game.aq.com/game/gamefiles/spider.swf".to_string(),
            abc_method_index: 7,
            kind: "bytecode",
        }
    }

    fn sourced_handler_descriptor(
        name: &str,
        movie_url: &str,
        abc_method_index: u32,
        kind: &'static str,
    ) -> EnterFrameHandlerDescriptor {
        EnterFrameHandlerDescriptor {
            qualified_name: name.to_string(),
            movie_url: movie_url.to_string(),
            abc_method_index,
            kind,
        }
    }

    #[test]
    fn enter_frame_handler_attribution_allocates_descriptor_only_for_new_method() {
        let mut state = EnterFrameHandlerAttributionState::default();
        let mut descriptor_builds = 0;

        state.record_with(11, 1_500, || {
            descriptor_builds += 1;
            handler_descriptor("World/countDownAct")
        });
        state.record_with(11, 2_500, || {
            descriptor_builds += 1;
            handler_descriptor("must-not-replace")
        });

        assert_eq!(descriptor_builds, 1);
        let report = state.take_report(20);
        assert_eq!(report.total_us, 4);
        assert_eq!(report.invocations, 2);
        assert_eq!(report.unique_methods, 1);
        assert_eq!(report.top.len(), 1);
        assert_eq!(report.top[0].qualified_name, "World/countDownAct");
        assert_eq!(report.top[0].total_us, 4);
        assert_eq!(report.top[0].invocations, 2);
        assert_eq!(report.top[0].max_us, 2);
        assert_eq!(report.tail_us, 0);
    }

    #[test]
    fn enter_frame_handler_attribution_reports_exact_top_and_tail_then_resets() {
        let mut state = EnterFrameHandlerAttributionState::default();
        state.record_with(1, 5_000, || handler_descriptor("A/hot"));
        state.record_with(2, 3_000, || handler_descriptor("B/warm"));
        state.record_with(3, 2_000, || handler_descriptor("C/tail"));

        let report = state.take_report(2);
        assert_eq!(report.total_us, 10);
        assert_eq!(report.invocations, 3);
        assert_eq!(report.unique_methods, 3);
        assert_eq!(
            report
                .top
                .iter()
                .map(|entry| entry.qualified_name.as_str())
                .collect::<Vec<_>>(),
            ["A/hot", "B/warm"]
        );
        assert_eq!(report.tail_us, 2);

        assert_eq!(state.take_report(20), EnterFrameHandlerReport::default());
    }

    #[test]
    fn enter_frame_handler_attribution_breaks_equal_time_ties_by_full_source_identity() {
        let mut state = EnterFrameHandlerAttributionState::default();
        let name = "Avatar/onEnterFrame";
        state.record_with(90, 5_000, || {
            sourced_handler_descriptor(name, "a.swf", 7, "bytecode")
        });
        state.record_with(80, 5_000, || {
            sourced_handler_descriptor(name, "a.swf", 7, "native")
        });
        state.record_with(70, 5_000, || {
            sourced_handler_descriptor(name, "a.swf", 8, "bytecode")
        });
        state.record_with(60, 5_000, || {
            sourced_handler_descriptor(name, "z.swf", 7, "bytecode")
        });

        let report = state.take_report(20);
        assert_eq!(
            report
                .top
                .iter()
                .map(|entry| { (entry.movie_url.as_str(), entry.abc_method_index, entry.kind,) })
                .collect::<Vec<_>>(),
            [
                ("a.swf", 7, "bytecode"),
                ("a.swf", 7, "native"),
                ("a.swf", 8, "bytecode"),
                ("z.swf", 7, "bytecode"),
            ]
        );
    }

    #[test]
    fn enter_frame_handler_attribution_uses_method_identity_as_final_tie_breaker() {
        let mut state = EnterFrameHandlerAttributionState::default();
        for method_key in 1..=16 {
            for _ in 1..method_key {
                state.record_with(method_key, 1, || handler_descriptor("Duplicate/name"));
            }
            state.record_with(method_key, 16_001 - method_key as u64, || {
                handler_descriptor("Duplicate/name")
            });
        }

        let report = state.take_report(1);
        assert_eq!(report.top.len(), 1);
        assert_eq!(report.top[0].invocations, 1);
    }

    #[test]
    fn enter_frame_handler_attribution_allocates_fractional_microseconds_exactly() {
        let mut state = EnterFrameHandlerAttributionState::default();
        state.record_with(1, 1_600, || handler_descriptor("A/hot"));
        state.record_with(2, 1_500, || handler_descriptor("B/warm"));
        state.record_with(3, 1_234, || handler_descriptor("C/tail"));

        let report = state.take_report(2);
        assert_eq!(report.total_us, 4);
        assert_eq!(
            report
                .top
                .iter()
                .map(|entry| entry.total_us)
                .collect::<Vec<_>>(),
            [2, 1]
        );
        assert_eq!(report.tail_us, 1);
        assert_eq!(
            report.total_us,
            report.top.iter().map(|entry| entry.total_us).sum::<u64>() + report.tail_us
        );
    }

    #[test]
    fn stop_guard_state_decline_reports_the_first_failing_gate() {
        let owner = "stage#0/game/a105661";
        let ready = MovementTraceRuntime {
            active: true,
            guard_eligible: true,
            owner_path: Some(owner.to_string()),
            target: Some(MovementPoint { x: 500.0, y: 200.0 }),
            walk_started_at: Some(Instant::now()),
            ..MovementTraceRuntime::default()
        };

        assert_eq!(
            movement_stop_guard_state_decline(&ready, Some(owner)),
            None,
            "a fully armed owner sequence must reach the suppression predicate"
        );

        assert_eq!(
            movement_stop_guard_state_decline(&ready, Some("stage#0/game/a105655")),
            Some(MovementStopGuardDecline::ForeignReceiver),
            "another avatar's stop must be attributed, not silently allowed"
        );

        assert_eq!(
            movement_stop_guard_state_decline(
                &MovementTraceRuntime {
                    active: false,
                    ..ready.clone()
                },
                Some(owner)
            ),
            Some(MovementStopGuardDecline::NoActiveSequence)
        );

        assert_eq!(
            movement_stop_guard_state_decline(
                &MovementTraceRuntime {
                    guard_eligible: false,
                    ..ready.clone()
                },
                Some(owner)
            ),
            Some(MovementStopGuardDecline::NotGuardEligible)
        );

        assert_eq!(
            movement_stop_guard_state_decline(
                &MovementTraceRuntime {
                    target: None,
                    ..ready.clone()
                },
                Some(owner)
            ),
            Some(MovementStopGuardDecline::NoTarget)
        );

        assert_eq!(
            movement_stop_guard_state_decline(
                &MovementTraceRuntime {
                    walk_started_at: None,
                    ..ready
                },
                Some(owner)
            ),
            Some(MovementStopGuardDecline::NoWalkStart)
        );
    }

    #[test]
    fn stop_guard_input_decline_separates_a_disabled_switch_from_a_missing_character_flag() {
        let position = Some(MovementPoint { x: 100.0, y: 200.0 });

        assert_eq!(
            movement_stop_guard_input_decline(false, Some(true), position),
            Some(MovementStopGuardDecline::GuardDisabled),
            "a build or preset that never enabled the guard must say so"
        );

        assert_eq!(
            movement_stop_guard_input_decline(true, None, position),
            Some(MovementStopGuardDecline::NoCharacterMoveFlag),
            "an AvatarMC whose mcChar.onMove cannot be read disables the guard silently today"
        );

        assert_eq!(
            movement_stop_guard_input_decline(true, Some(true), None),
            Some(MovementStopGuardDecline::NoReceiverPosition)
        );

        assert_eq!(
            movement_stop_guard_input_decline(true, Some(false), position),
            None,
            "a lost onMove flag is a suppress-and-resume case, not a decline"
        );
    }

    #[test]
    fn stop_guard_callback_decline_separates_callback_scope_from_a_legitimate_stop() {
        assert_eq!(
            movement_stop_guard_callback_decline(false, false, false, true),
            Some(MovementStopGuardDecline::NotInGuardedCallback),
            "the captured premature stops arrived outside every guarded callback scope"
        );

        assert_eq!(
            movement_stop_guard_callback_decline(false, false, true, false),
            Some(MovementStopGuardDecline::StopLooksLegitimate),
            "a stop at its destination must be attributed to the predicate, not the scope"
        );

        for (in_enter_frame, in_mouse_move, in_delayed) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            assert_eq!(
                movement_stop_guard_callback_decline(
                    in_enter_frame,
                    in_mouse_move,
                    in_delayed,
                    true
                ),
                None,
                "every guarded scope must reach suppression when the predicate matches"
            );
        }
    }

    #[test]
    fn stop_guard_decline_budget_caps_each_reason_independently() {
        let mut budget = MovementStopGuardDeclineBudget::default();

        for attempt in 0..MAX_MOVEMENT_STOP_GUARD_DECLINE_REPORTS {
            assert!(
                budget.admit(MovementStopGuardDecline::ForeignReceiver),
                "report {attempt} within the budget must be admitted"
            );
        }

        assert!(
            !budget.admit(MovementStopGuardDecline::ForeignReceiver),
            "a crowded room must not be able to flood the trace with one reason"
        );

        assert!(
            budget.admit(MovementStopGuardDecline::NoCharacterMoveFlag),
            "an unrelated reason keeps its own independent budget"
        );
    }
}

/// Report what a filtered object is wrapped in, once per object.
///
/// A filter that draws outside its object needs room reserved for it, and the room is computed from
/// the object's bounds and everything between it and the stage. AQW's weapon glow renders correctly
/// in every case that can be built outside the game -- the real item, at any size, turned, and held
/// as a bitmap -- and still renders as a rectangle in it. So the difference is in the containers,
/// and the containers are the one thing a case built by hand cannot guess at.
///
/// Switched on with `AETHER_GLOW_ANCESTRY=1`, off otherwise, and each object reports only once.
pub fn filter_ancestry_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::env::var_os("AETHER_GLOW_ANCESTRY").is_some_and(|value| value != "0"))
}

// The login screen and UI alone spend dozens of these before a map is even loaded, so the cap has
// to be well clear of them or the report never reaches anything in the world.
const MAX_REPORTED_FILTER_ANCESTRIES: usize = 512;

/// One link in the chain between a filtered object and the stage.
fn describe_ancestor(object: DisplayObject<'_>) -> String {
    let descriptor = DisplayObjectDescriptor::from_display_object(object);
    let matrix = object.base().matrix();
    let filters = filter_names(&object.filters());
    let mut notes: Vec<String> = Vec::new();
    if object.is_bitmap_cached_preference() {
        notes.push("cacheAsBitmap".into());
    }
    if object.masker().is_some() {
        notes.push("masked".into());
    }
    if object.maskee().is_some() {
        notes.push("is-a-mask".into());
    }
    if object.clip_depth() > 0 {
        notes.push(format!("clipDepth={}", object.clip_depth()));
    }
    if let Some(rect) = object.scroll_rect() {
        notes.push(format!(
            "scrollRect={}x{}",
            rect.width().to_pixels().round(),
            rect.height().to_pixels().round()
        ));
    }
    if !object.visible() {
        notes.push("hidden".into());
    }
    if !filters.is_empty() {
        notes.push(format!("filters={}", filters.join("+")));
    }
    notes.push(format!("blend={:?}", object.blend_mode()));

    format!(
        "{} name={:?} class={:?} matrix=[{:.3} {:.3} {:.3} {:.3}] {}",
        descriptor.object_type,
        descriptor.instance_name.as_deref().unwrap_or("-"),
        descriptor.class_name.as_deref().unwrap_or("-"),
        matrix.a,
        matrix.b,
        matrix.c,
        matrix.d,
        notes.join(" "),
    )
}

/// Log the chain from a filtered object up to the stage, with the room reserved for its filter.
pub fn record_filter_ancestry(
    object: DisplayObject<'_>,
    filters: &[String],
    source_width: u32,
    source_height: u32,
    texture_width: u32,
    texture_height: u32,
) {
    if !filter_ancestry_enabled() {
        return;
    }

    static REPORTED: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    let reported = REPORTED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut reported = match reported.lock() {
        Ok(reported) => reported,
        Err(poisoned) => poisoned.into_inner(),
    };
    if reported.len() >= MAX_REPORTED_FILTER_ANCESTRIES
        || !reported.insert(object.as_ptr() as usize)
    {
        return;
    }
    drop(reported);

    // The growth is the whole question: if the texture is no larger than the source, the filter has
    // been given no room and will be cut off at the object's own outline.
    let grew_by = (
        texture_width as i64 - source_width as i64,
        texture_height as i64 - source_height as i64,
    );

    let mut chain = Vec::with_capacity(8);
    let mut walker = Some(object);
    for _ in 0..64 {
        let Some(current) = walker else { break };
        chain.push(describe_ancestor(current));
        walker = current.parent();
    }

    tracing::warn!(
        filters = ?filters,
        source = format!("{source_width}x{source_height}"),
        texture = format!("{texture_width}x{texture_height}"),
        grew_by = format!("{}x{}", grew_by.0, grew_by.1),
        "AQW filtered object ancestry (innermost first):\n  {}",
        chain.join("\n  "),
    );
}
