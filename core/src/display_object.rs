use crate::avm1::{
    ActivationIdentifier as Avm1ActivationIdentifier, Object as Avm1Object, Value as Avm1Value,
};
use crate::avm2::{
    Activation as Avm2Activation, Avm2, Error as Avm2Error, LoaderInfoObject,
    Multiname as Avm2Multiname, Object as Avm2Object, StageObject as Avm2StageObject, TObject as _,
    Value as Avm2Value,
};
use crate::context::{RenderContext, SlicePass, UpdateContext};
use crate::drawing::Drawing;
use crate::prelude::*;
use crate::string::{AvmString, WString};
use crate::tag_utils::SwfMovie;
use crate::types::{Degrees, Percent};
use crate::vminterface::Instantiator;
use bitflags::bitflags;
use gc_arena::barrier::{Write, unlock};
use gc_arena::lock::Lock;
use gc_arena::{Collect, Gc, Mutation};
use ruffle_macros::{enum_trait_object, istr};
use ruffle_render::perspective_projection::PerspectiveProjection;
use ruffle_render::pixel_bender::PixelBenderShaderHandle;
use ruffle_render::transform::{Transform, TransformStack};
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZero;
use std::sync::Arc;
use swf::{ColorTransform, Fixed8};

mod avm1_button;
mod avm2_button;
mod bitmap;
mod container;
mod edit_text;
mod graphic;
mod interactive;
mod loader_display;
mod morph_shape;
mod movie_clip;
mod stage;
mod text;
mod text_line;
mod video;

use crate::avm1::Activation;
use crate::display_object::bitmap::BitmapWeak;
pub use crate::display_object::container::{
    DisplayObjectContainer, TDisplayObjectContainer, dispatch_added_event_only,
    dispatch_added_to_stage_event_only,
};
pub use avm1_button::{Avm1Button, ButtonState, ButtonTracking};
pub use avm2_button::Avm2Button;
pub use bitmap::{Bitmap, BitmapClass};
#[allow(unused)]
pub use edit_text::LayoutDebugBoxesFlag;
pub use edit_text::{AutoSizeMode, EditText, TextSelection};
pub use graphic::Graphic;
pub use interactive::{Avm2MousePick, InteractiveObject, TInteractiveObject};
pub use loader_display::LoaderDisplay;
pub use morph_shape::MorphShape;
pub use movie_clip::{MovieClip, MovieClipHandle, MovieClipWeak, Scene};
use ruffle_render::backend::{BitmapCacheEntry, RenderBackend};
use ruffle_render::bitmap::{BitmapHandle, BitmapInfo, BitmapSize, PixelSnapping};
use ruffle_render::blend::ExtendedBlendMode;
use ruffle_render::commands::{CommandHandler, CommandList, RenderBlendMode};
use ruffle_render::filters::Filter;
pub use stage::{Stage, StageAlign, StageDisplayState, StageScaleMode, WindowMode};
pub use text::{Text, TextSnapshot};
pub use text_line::TextLine;
pub use video::Video;

use self::loader_display::LoaderDisplayWeak;

/// If a `DisplayObject` is marked `cacheAsBitmap` (via tag or AS),
/// this struct keeps the information required to uphold that cache.
/// A cached Display Object must have its bitmap invalidated when
/// any "visual" change happens, which can include:
/// - Changing the rotation
/// - Changing the scale
/// - Changing the alpha
/// - Changing the color transform
/// - Any "visual" change to children, **including** position changes
///
/// Position changes to the cached Display Object does not regenerate the cache,
/// allowing Display Objects to move freely without being regenerated.
///
/// Flash isn't very good at always recognising when it should be invalidated,
/// and there's cases such as changing the blend mode which don't always trigger it.
///
#[derive(Clone, Debug, Default)]
pub struct BitmapCache {
    /// The `Matrix.a` value that was last used with this cache
    matrix_a: f32,
    /// The `Matrix.b` value that was last used with this cache
    matrix_b: f32,
    /// The `Matrix.c` value that was last used with this cache
    matrix_c: f32,
    /// The `Matrix.d` value that was last used with this cache
    matrix_d: f32,

    /// The width of the original bitmap, pre-filters
    source_width: u32,

    /// The height of the original bitmap, pre-filters
    source_height: u32,

    /// Current logical output dimensions after filter growth. The backing texture may be larger
    /// because animation frames reuse a high-water capacity.
    output_width: u32,
    output_height: u32,

    /// The offset used to draw the final bitmap (i.e. if a filter increases the size)
    draw_offset: Point<i32>,

    /// Bounds origin relative to the object's translation at the last rebuild. Translation does
    /// not invalidate a Flash bitmap cache, so this remains valid while the clean-hit fast path
    /// is eligible.
    bounds_offset: Point<Twips>,

    /// Stage-view scale used when the cached filters and geometry were prepared.
    stage_scale_a: f32,
    stage_scale_d: f32,

    /// The current contents of the cache, if any. Values are post-filters.
    bitmap: Option<BitmapInfo>,

    /// Whether we warned that this bitmap was too large to be cached
    warned_for_oversize: bool,

    /// Consecutive dirty rebuilds of an explicitly filterless cache. A cache that is invalidated
    /// every rendered frame is more expensive than drawing its vector contents directly: it first
    /// renders the same contents offscreen and then copies the texture back to the stage.
    filterless_rebuild_streak: u8,

    /// Remaining frames for which a repeatedly invalidated, filterless cache is drawn directly.
    /// This preserves every authored animation frame while avoiding redundant offscreen work.
    filterless_direct_frames: u16,
}

const FILTERLESS_HOT_CACHE_REBUILD_THRESHOLD: u8 = 3;
const FILTERLESS_HOT_CACHE_DIRECT_FRAMES: u16 = 120;
const FILTERLESS_HOT_CACHE_MIN_PIXELS: u64 = 128 * 128;
const AETHER_ADAPTIVE_AVATAR_CACHE_STABLE_FRAMES: u8 = 3;
const AETHER_ADAPTIVE_AVATAR_CACHE_MAX_DIMENSION: u32 = 2_048;
const AETHER_ADAPTIVE_AVATAR_CACHE_MAX_PIXELS: u64 = 1_048_576;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BitmapCacheTexturePlan {
    Reuse,
    Allocate { width: u32, height: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BitmapCacheTexturePolicy {
    Exact,
    BoundedReuse,
}

/// Grid that cache texture sizes are rounded up to.
///
/// AQW avatars shift their bounds by a pixel or two every frame as they animate, so one object asks
/// for 247x226, then 248x227, then 249x228. Each is a separate pool bucket that nothing will ever
/// request again, which is why the offscreen pool sits at 75% reuse however its budget and idle
/// window are set: raising either just changes whether a doomed entry is discarded for age or for
/// bytes. Rounding up collapses a whole run of drift into one bucket.
const BITMAP_CACHE_SIZE_GRID: u32 = 64;

/// How many cells span a dimension, which is what makes the slack proportional.
///
/// The cell is the dimension's own power-of-two bracket divided by this, so a 250px avatar rounds
/// by 64 and a 3700px backdrop rounds by 256. Both waste roughly the same share of themselves,
/// and both collapse a run of drift into one bucket.
const BITMAP_CACHE_GRID_STEPS: u32 = 16;

/// Round a cache texture up so that a few pixels of bounds drift reuse the same texture.
///
/// The texture ends up larger than its contents, which the cache already handles: `output_width`
/// and `output_height` keep the true size, and both drawing and filtering work from that logical
/// region rather than from the texture's dimensions.
///
/// Surfaces above 1024px used to be exempt, on the theory that they were one-off backdrops that
/// never repeat. A device-loss census from an RX 6800 XT measured the opposite: 25,479 of the
/// session's 35,551 texture allocations landed in sizes past the tracking table, accounting for
/// 5.2 GB of the 7.3 GB churned in sixty seconds, while the pool serving them managed 84.3% reuse
/// against the gridded pool's 99.6%. Those surfaces repeat constantly. They just never repeat
/// exactly, because an animating object's bounds move a pixel or two per frame, and an exempt
/// size is a bucket nothing ever asks for twice.
///
/// The card had 1.33 GB resident at peak and 16 GB fitted. It ran out of memory because of the
/// churn and the fragmentation it leaves behind, not because of the total.
fn quantise_cache_texture_size(size: (u32, u32)) -> (u32, u32) {
    let round = |value: u32| {
        // Never below the old fixed cell: small textures were already collapsing well at 64, and
        // a finer grid there would only put the drift back.
        let cell =
            (value.next_power_of_two() / BITMAP_CACHE_GRID_STEPS).max(BITMAP_CACHE_SIZE_GRID);

        value.div_ceil(cell).saturating_mul(cell).max(value)
    };
    (round(size.0), round(size.1))
}

fn bitmap_cache_texture_plan(
    current: Option<(u32, u32)>,
    requested: (u32, u32),
    policy: BitmapCacheTexturePolicy,
) -> BitmapCacheTexturePlan {
    let requested = if aqw_cache_texture_grid() {
        quantise_cache_texture_size(requested)
    } else {
        requested
    };

    if current == Some(requested) {
        return BitmapCacheTexturePlan::Reuse;
    }

    if policy == BitmapCacheTexturePolicy::BoundedReuse
        && let Some((current_width, current_height)) = current
    {
        let requested_area = u64::from(requested.0) * u64::from(requested.1);
        let current_area = u64::from(current_width) * u64::from(current_height);
        let current_contains_requested =
            current_width >= requested.0 && current_height >= requested.1;

        if current_contains_requested
            && current_area <= requested_area.saturating_mul(2)
            && adaptive_avatar_cache_dimensions_allowed(current_width, current_height)
        {
            return BitmapCacheTexturePlan::Reuse;
        }

        let grown = (
            current_width.max(requested.0),
            current_height.max(requested.1),
        );
        let grown_area = u64::from(grown.0) * u64::from(grown.1);
        if grown_area <= requested_area.saturating_mul(2)
            && adaptive_avatar_cache_dimensions_allowed(grown.0, grown.1)
        {
            return BitmapCacheTexturePlan::Allocate {
                width: grown.0,
                height: grown.1,
            };
        }
    }

    BitmapCacheTexturePlan::Allocate {
        width: requested.0,
        height: requested.1,
    }
}

/// Whether AQW's own `cacheAsBitmap` surfaces may reuse a slightly oversized cache texture.
///
/// AQW avatars shift their bounds by a pixel or two every frame as they animate, and under
/// `Exact` each shift allocates a fresh texture while the previous one waits for the GPU to
/// retire it. Measured on a crowded Battleon that reached 300-1,729 MB of cache textures per
/// second and preceded every framerate collapse. `BoundedReuse` was already implemented and
/// already applied to Aether's own avatar caches; it was simply never extended to the
/// authored caches that produce most of the traffic.
#[cfg(feature = "aether_performance")]
fn aqw_bounded_cache_texture_reuse() -> bool {
    crate::aether_performance::adaptive_avatar_cache_enabled()
}

#[cfg(not(feature = "aether_performance"))]
fn aqw_bounded_cache_texture_reuse() -> bool {
    false
}

#[cfg(feature = "aether_performance")]
fn aqw_cache_texture_grid() -> bool {
    crate::aether_performance::cache_texture_grid_enabled()
}

#[cfg(not(feature = "aether_performance"))]
fn aqw_cache_texture_grid() -> bool {
    false
}

#[cfg(test)]
mod cache_texture_grid_tests {
    use super::*;

    #[test]
    fn a_run_of_drifting_sizes_collapses_to_one_bucket() {
        // Measured from a real session: one animating object asked for these three sizes in
        // sequence, and each became a pool bucket nothing ever asked for again.
        let drift = [(247, 226), (248, 227), (249, 228)];
        let quantised: Vec<_> = drift
            .iter()
            .map(|size| quantise_cache_texture_size(*size))
            .collect();

        assert!(
            quantised.windows(2).all(|pair| pair[0] == pair[1]),
            "a pixel of drift must not change the bucket: {quantised:?}"
        );
        assert_eq!(quantised[0], (256, 256));
    }

    #[test]
    fn quantising_never_shrinks_a_texture_below_its_contents() {
        for width in [1_u32, 63, 64, 65, 183, 512, 1023, 1024] {
            for height in [1_u32, 64, 145, 583, 1024] {
                let (quantised_width, quantised_height) =
                    quantise_cache_texture_size((width, height));
                assert!(
                    quantised_width >= width && quantised_height >= height,
                    "{width}x{height} must not shrink to {quantised_width}x{quantised_height}"
                );
            }
        }
    }

    /// Large surfaces were exempt from the grid, on the theory that they are one-off backdrops
    /// that never repeat. A crash census from an RX 6800 XT measured the opposite: 25,479 of the
    /// session's 35,551 texture allocations fell in sizes past the tracking table, 5.2 GB of the
    /// 7.3 GB churned in sixty seconds, and the pool that serves them managed 84.3% reuse against
    /// the gridded pool's 99.6%. They repeat constantly; they just never repeat *exactly*.
    #[test]
    fn oversized_surfaces_collapse_too() {
        // Two sizes the same backdrop asked for moments apart, read off that census.
        assert_eq!(
            quantise_cache_texture_size((3682, 1715)),
            quantise_cache_texture_size((3671, 1710)),
            "a few pixels of drift on a backdrop must not make a new bucket"
        );
        assert_eq!(
            quantise_cache_texture_size((2261, 1768)),
            quantise_cache_texture_size((2255, 1763))
        );
    }

    /// The cell grows with the surface, so a backdrop is not rounded by the same absolute amount
    /// as an avatar. A fixed 64px cell would barely dent the bucket count up here; a fixed 256px
    /// one would waste a quarter of every mid-sized surface.
    ///
    /// Only the large sizes are asserted. Small textures keep the existing 64px cell, where the
    /// slack is proportionally large and absolutely trivial, and 99.6% pool reuse says that
    /// trade was already right.
    #[test]
    fn the_slack_stays_proportional_on_large_surfaces() {
        for size in [(1024, 768), (1370, 988), (2261, 1768), (3682, 1715)] {
            let (width, height) = quantise_cache_texture_size(size);
            let before = f64::from(size.0) * f64::from(size.1);
            let after = f64::from(width) * f64::from(height);

            assert!(
                after / before <= 1.15,
                "{size:?} became {width}x{height}, wasting {:.0}%",
                (after / before - 1.0) * 100.0
            );
        }
    }

    /// Whatever the rounding does, it can never hand back a texture too small for its contents.
    #[test]
    fn quantising_never_shrinks_a_large_texture_either() {
        for width in [1_025_u32, 2_048, 2_261, 3_682, 4_097, 8_192] {
            for height in [1_710_u32, 1_768, 2_048, 4_096] {
                let (quantised_width, quantised_height) =
                    quantise_cache_texture_size((width, height));
                assert!(
                    quantised_width >= width && quantised_height >= height,
                    "{width}x{height} must not shrink to {quantised_width}x{quantised_height}"
                );
            }
        }
    }
}

fn adaptive_avatar_cache_dimensions_allowed(width: u32, height: u32) -> bool {
    width <= AETHER_ADAPTIVE_AVATAR_CACHE_MAX_DIMENSION
        && height <= AETHER_ADAPTIVE_AVATAR_CACHE_MAX_DIMENSION
        && u64::from(width) * u64::from(height) <= AETHER_ADAPTIVE_AVATAR_CACHE_MAX_PIXELS
}

impl BitmapCache {
    /// Forcefully make this BitmapCache invalid and require regeneration.
    /// This should be used for changes that aren't automatically detected, such as children.
    pub fn make_dirty(&mut self) {
        // Setting the old transform to something invalid is a cheap way of making it invalid,
        // without reserving an extra field for.
        self.matrix_a = f32::NAN;
    }

    /// Record a dirty rebuild of an explicit cache that has no effective filters. Returns true
    /// when repeated invalidation proves that the cache is not providing reusable work and should
    /// temporarily render directly instead.
    fn note_filterless_rebuild(&mut self) -> bool {
        self.filterless_rebuild_streak = self.filterless_rebuild_streak.saturating_add(1);
        if self.filterless_rebuild_streak < FILTERLESS_HOT_CACHE_REBUILD_THRESHOLD {
            return false;
        }

        self.filterless_rebuild_streak = 0;
        true
    }

    /// Start a bounded direct-render window after the subtree has passed the semantic-safety
    /// check. Keeping this separate from rebuild detection prevents an unchecked subtree from
    /// entering the bypass.
    fn begin_filterless_direct_rendering(&mut self) {
        self.filterless_direct_frames = FILTERLESS_HOT_CACHE_DIRECT_FRAMES;
    }

    fn has_filterless_direct_render_frames(&self) -> bool {
        self.filterless_direct_frames != 0
    }

    fn cancel_filterless_direct_rendering(&mut self) {
        self.filterless_direct_frames = 0;
    }

    /// Consume one temporary direct-render frame for a known-hot filterless cache.
    fn take_filterless_direct_render_frame(&mut self) -> bool {
        if self.filterless_direct_frames == 0 {
            return false;
        }

        self.filterless_direct_frames -= 1;
        true
    }

    /// A clean hit proves that this cache is reusable, so a partial hot-cache streak must not
    /// carry across it.
    fn note_cache_hit(&mut self) {
        self.filterless_rebuild_streak = 0;
    }

    fn dirty_reason(
        &self,
        other: &Matrix,
        source_width: u32,
        source_height: u32,
        stage_matrix: &Matrix,
    ) -> Option<&'static str> {
        if self.bitmap.is_none() {
            Some("missing_bitmap")
        } else if self.matrix_a.is_nan() {
            Some("explicit_invalidation")
        } else if self.matrix_a != other.a
            || self.matrix_b != other.b
            || self.matrix_c != other.c
            || self.matrix_d != other.d
        {
            Some("transform_change")
        } else if let Some(reason) = self.stage_scale_dirty_reason(stage_matrix) {
            Some(reason)
        } else {
            if self.source_width != source_width || self.source_height != source_height {
                Some("size_change")
            } else {
                None
            }
        }
    }

    fn stage_scale_dirty_reason(&self, stage_matrix: &Matrix) -> Option<&'static str> {
        (self.stage_scale_a != stage_matrix.a || self.stage_scale_d != stage_matrix.d)
            .then_some("stage_scale_change")
    }

    /// Clears any dirtiness and ensure there's an appropriately sized texture allocated
    #[expect(clippy::too_many_arguments)]
    fn update(
        &mut self,
        renderer: &mut dyn RenderBackend,
        matrix: Matrix,
        source_width: u32,
        source_height: u32,
        actual_width: u32,
        actual_height: u32,
        draw_offset: Point<i32>,
        bounds_offset: Point<Twips>,
        stage_scale_a: f32,
        stage_scale_d: f32,
        swf_version: u8,
        texture_policy: BitmapCacheTexturePolicy,
    ) {
        self.matrix_a = matrix.a;
        self.matrix_b = matrix.b;
        self.matrix_c = matrix.c;
        self.matrix_d = matrix.d;
        self.source_width = source_width;
        self.source_height = source_height;
        self.output_width = actual_width;
        self.output_height = actual_height;
        self.draw_offset = draw_offset;
        self.bounds_offset = bounds_offset;
        self.stage_scale_a = stage_scale_a;
        self.stage_scale_d = stage_scale_d;
        let texture_plan = bitmap_cache_texture_plan(
            self.bitmap
                .as_ref()
                .map(|current| (current.width, current.height)),
            (actual_width, actual_height),
            texture_policy,
        );
        let (allocation_width, allocation_height) = match texture_plan {
            BitmapCacheTexturePlan::Reuse => {
                #[cfg(feature = "aether_metrics")]
                crate::aether_metrics::bitmap_cache_texture_reused();
                return;
            }
            BitmapCacheTexturePlan::Allocate { width, height } => (width, height),
        };
        #[cfg(feature = "aether_metrics")]
        let resized = self.bitmap.is_some();
        let acceptable_size = if swf_version > 9 {
            let total = allocation_width * allocation_height;
            allocation_width < 8191 && allocation_height < 8191 && total < 16777215
        } else {
            allocation_width < 2880 && allocation_height < 2880
        };

        if renderer.is_offscreen_supported()
            && let Some(allocation_width) = NonZero::new(allocation_width)
            && let Some(allocation_height) = NonZero::new(allocation_height)
            && acceptable_size
        {
            match renderer.create_empty_texture(allocation_width, allocation_height) {
                Ok(handle) => {
                    #[cfg(feature = "aether_metrics")]
                    crate::aether_metrics::bitmap_cache_texture_allocated(
                        allocation_width.get(),
                        allocation_height.get(),
                        resized,
                    );
                    self.bitmap = Some(BitmapInfo {
                        width: allocation_width.get(),
                        height: allocation_height.get(),
                        handle,
                    });
                }
                Err(_) => self.bitmap = None,
            }
        } else {
            self.bitmap = None;
        }
    }

    /// Explicitly clears the cached value and drops any resources.
    /// This should only be used in situations where you can't render to the cache and it needs to be
    /// temporarily disabled.
    fn clear(&mut self) {
        self.bitmap = None;
    }

    fn handle(&self) -> Option<BitmapHandle> {
        self.bitmap.as_ref().map(|b| b.handle.clone())
    }

    fn output_size(&self) -> (u32, u32) {
        (self.output_width, self.output_height)
    }
}

#[derive(Clone)]
pub struct RenderOptions {
    /// Whether to skip rendering masks.
    ///
    /// Masks are usually skipped when rendering, but when e.g. rendering
    /// the mask itself, it can't be skipped.
    ///
    /// Masks are skipped by default.
    pub skip_masks: bool,

    /// Whether to apply object's base transform.
    ///
    /// For instance, when calling BitmapData.draw, object's transform is not
    /// applied.
    ///
    /// Transform is applied by default.
    pub apply_transform: bool,

    /// Whether to apply base transform's matrix when rendering.
    ///
    /// Sometimes we need to render an object without applying its matrix, but
    /// with applying other parts of its transform (e.g. color transform).
    /// This happens e.g. when rendering alpha masks.
    ///
    /// Matrix is applied by default.
    pub apply_matrix: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            apply_transform: true,
            skip_masks: true,
            apply_matrix: true,
        }
    }
}

#[derive(Clone, Collect, Debug)]
#[collect(no_drop)]
pub enum RenderMask<'gc> {
    /// There's no mask.
    None,

    /// Stencil masks are the classic, default masks used in Flash Player.
    ///
    /// The masker behaves like a stencil, and masks everything outside its
    /// rendered pixels irrespectively of the pixels themselves.
    /// The maskee acts like being masked with the masker's hit test image.
    Stencil(DisplayObject<'gc>),

    /// Alpha masks are the more advanced (and more intuitive) masks used when
    /// CAB is enabled.
    ///
    /// The maskee is being masked based on the value of the masker's alpha
    /// channel.
    Alpha(DisplayObject<'gc>),
}

/// AVM2 lifecycle tree walks that can safely skip a subtree until a mutation
/// makes that subtree relevant again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Avm2LifecycleTraversal {
    Enter,
    Construct,
    FrameScripts,
}

impl Avm2LifecycleTraversal {
    const ALL: u8 = (1 << 3) - 1;

    const fn bit(self) -> u8 {
        match self {
            Self::Enter => 1 << 0,
            Self::Construct => 1 << 1,
            Self::FrameScripts => 1 << 2,
        }
    }
}

/// Lifecycle summaries are runtime state, not template data. Display-object
/// instantiation clones library templates, so cloning this wrapper deliberately
/// resets every phase to dirty instead of copying a template's consumed bits.
#[derive(Collect)]
#[collect(require_static)]
struct Avm2LifecycleDirty(Cell<u8>);

impl Default for Avm2LifecycleDirty {
    fn default() -> Self {
        Self(Cell::new(Avm2LifecycleTraversal::ALL))
    }
}

impl Clone for Avm2LifecycleDirty {
    fn clone(&self) -> Self {
        Self::default()
    }
}

#[derive(Clone, Collect)]
#[collect(no_drop)]
// Ensure this always has the same alignment as its subclasses (needed for `Gc` casts).
#[repr(align(8))]
pub struct DisplayObjectBase<'gc> {
    cell: RefCell<DisplayObjectBaseMut>,
    parent: Lock<Option<DisplayObject<'gc>>>,
    place_frame: Cell<u16>,
    depth: Cell<Depth>,
    ratio: Cell<u16>,
    name: Lock<Option<AvmString<'gc>>>,
    clip_depth: Cell<Depth>,

    // The transform of this display object.
    // (Split into several fields for easier access)
    matrix: Cell<Matrix>,
    color_transform: Cell<ColorTransform>,
    perspective_projection: Cell<Option<PerspectiveProjection>>,

    // Cached transform properties `_xscale`, `_yscale`, `_rotation`.
    // These are expensive to calculate, so they will be calculated and cached
    // when AS requests one of these properties.
    rotation: Cell<Degrees>,
    scale_x: Cell<Percent>,
    scale_y: Cell<Percent>,
    skew: Cell<f64>,

    /// The sound transform of sounds playing via this display object.
    sound_transform: Cell<SoundTransform>,

    /// The display object that we are being masked by.
    masker: Lock<Option<DisplayObject<'gc>>>,

    /// The display object we are currently masking.
    maskee: Lock<Option<DisplayObject<'gc>>>,

    meta_data: Lock<Option<Avm2Object<'gc>>>,

    /// The blend mode used when rendering this display object.
    /// Values other than the default `BlendMode::Normal` implicitly cause cache-as-bitmap behavior.
    blend_mode: Cell<ExtendedBlendMode>,

    #[collect(require_static)]

    /// The opaque background color of this display object.
    /// The bounding box of the display object will be filled with the given color. This also
    /// triggers cache-as-bitmap behavior. Only solid backgrounds are supported; the alpha channel
    /// is ignored.
    opaque_background: Cell<Option<Color>>,

    /// Bit flags for various display object properties.
    flags: Cell<DisplayObjectFlags>,

    /// Conservative summaries for AVM2 lifecycle subtree walks. A set bit
    /// means that the object or one of its descendants may have work for that
    /// phase. Each walk clears its bit before executing so re-entrant
    /// mutations remain visible to the next walk.
    avm2_lifecycle_dirty: Avm2LifecycleDirty,

    /// Consecutive rendered frames for which an AQW AvatarMC candidate received no visual
    /// invalidation. This is internal renderer state and never changes the ActionScript-visible
    /// cacheAsBitmap preference.
    aether_adaptive_avatar_cache_clean_frames: Cell<u8>,

    /// Last non-translation transform observed by the adaptive AvatarMC cache state machine.
    /// Root translation is intentionally excluded because Flash bitmap caches can move without
    /// rebuilding, while scale/rotation/skew changes must return to direct rendering immediately.
    aether_adaptive_avatar_cache_matrix: Cell<[f32; 4]>,

    /// The 'internal' scroll rect used for rendering and methods like 'localToGlobal'.
    /// This is updated from 'pre_render'
    scroll_rect: Cell<Option<Rectangle<Twips>>>,

    /// The 'next' scroll rect, which we will copy to 'scroll_rect' from 'pre_render'.
    /// This is used by the ActionScript 'DisplayObject.scrollRect' getter, which sees
    /// changes immediately (without needing wait for a render)
    next_scroll_rect: Cell<Rectangle<Twips>>,

    /// Rectangle used for 9-slice scaling (`DisplayObject.scale9grid`).
    scaling_grid: Cell<Rectangle<Twips>>,
}

#[derive(Clone)]
struct DisplayObjectBaseMut {
    filters: Box<[Filter]>,

    blend_shader: Option<PixelBenderShaderHandle>,

    /// If this Display Object should cacheAsBitmap - and if so, the cache itself.
    /// None means not cached, Some means cached.
    cache: Option<BitmapCache>,
}

impl Default for DisplayObjectBase<'_> {
    fn default() -> Self {
        Self {
            cell: RefCell::new(DisplayObjectBaseMut {
                filters: Default::default(),
                blend_shader: None,
                cache: None,
            }),
            parent: Default::default(),
            place_frame: Default::default(),
            depth: Default::default(),
            ratio: Default::default(),
            name: Lock::new(None),
            clip_depth: Default::default(),
            matrix: Default::default(),
            color_transform: Default::default(),
            perspective_projection: Default::default(),
            rotation: Cell::new(Degrees::from_radians(0.0)),
            scale_x: Cell::new(Percent::from_unit(1.0)),
            scale_y: Cell::new(Percent::from_unit(1.0)),
            skew: Cell::new(0.0),
            masker: Lock::new(None),
            maskee: Lock::new(None),
            meta_data: Lock::new(None),
            sound_transform: Default::default(),
            blend_mode: Default::default(),
            opaque_background: Default::default(),
            flags: Cell::new(DisplayObjectFlags::VISIBLE),
            avm2_lifecycle_dirty: Default::default(),
            aether_adaptive_avatar_cache_clean_frames: Cell::new(0),
            aether_adaptive_avatar_cache_matrix: Cell::new([1.0, 0.0, 0.0, 1.0]),
            scroll_rect: Cell::new(None),
            next_scroll_rect: Default::default(),
            scaling_grid: Default::default(),
        }
    }
}

impl<'gc> DisplayObjectBase<'gc> {
    fn contains_flag(&self, flag: DisplayObjectFlags) -> bool {
        self.flags.get().contains(flag)
    }

    fn set_flag(&self, flag: DisplayObjectFlags, value: bool) {
        let mut flags = self.flags.get();
        flags.set(flag, value);
        self.flags.set(flags);
    }

    /// Reset all properties that would be adjusted by a movie load.
    fn reset_for_movie_load(&self) {
        let flags_to_keep = self.flags.get() & DisplayObjectFlags::LOCK_ROOT;
        self.flags.set(flags_to_keep | DisplayObjectFlags::VISIBLE);
        self.avm2_lifecycle_dirty.0.set(Avm2LifecycleTraversal::ALL);
        self.aether_adaptive_avatar_cache_clean_frames.set(0);
        self.aether_adaptive_avatar_cache_matrix
            .set(self.aether_adaptive_avatar_cache_matrix_components());
        self.recheck_cache_as_bitmap();
    }

    fn begin_avm2_lifecycle_traversal(&self, traversal: Avm2LifecycleTraversal) -> bool {
        let bit = traversal.bit();
        let dirty = self.avm2_lifecycle_dirty.0.get();
        self.avm2_lifecycle_dirty.0.set(dirty & !bit);
        dirty & bit != 0
    }

    fn mark_avm2_lifecycle_dirty(&self, traversal: Avm2LifecycleTraversal) {
        self.avm2_lifecycle_dirty
            .0
            .set(self.avm2_lifecycle_dirty.0.get() | traversal.bit());
    }

    fn is_avm2_lifecycle_dirty(&self, traversal: Avm2LifecycleTraversal) -> bool {
        self.avm2_lifecycle_dirty.0.get() & traversal.bit() != 0
    }

    fn depth(&self) -> Depth {
        self.depth.get()
    }

    fn set_depth(&self, depth: Depth) {
        self.depth.set(depth);
    }

    fn place_frame(&self) -> u16 {
        self.place_frame.get()
    }

    fn set_place_frame(&self, frame: u16) {
        self.place_frame.set(frame);
    }

    fn transform(&self, apply_matrix: bool) -> Transform {
        Transform {
            matrix: if apply_matrix {
                self.matrix.get()
            } else {
                Matrix::IDENTITY
            },
            color_transform: self.color_transform.get(),
            perspective_projection: self.perspective_projection.get(),
        }
    }

    pub fn matrix(&self) -> Matrix {
        self.matrix.get()
    }

    pub fn set_matrix(&self, matrix: Matrix) {
        self.matrix.set(matrix);
        self.set_scale_rotation_cached(false);
    }

    pub fn color_transform(&self) -> ColorTransform {
        self.color_transform.get()
    }

    pub fn set_color_transform(&self, color_transform: ColorTransform) {
        self.color_transform.set(color_transform);
    }

    pub fn perspective_projection(&self) -> Option<PerspectiveProjection> {
        self.perspective_projection.get()
    }

    pub fn set_perspective_projection(
        &self,
        perspective_projection: Option<PerspectiveProjection>,
    ) -> bool {
        let old = self.perspective_projection.replace(perspective_projection);
        perspective_projection != old
    }

    fn x(&self) -> Twips {
        self.matrix.get().tx
    }

    fn set_x(&self, x: Twips) -> bool {
        let mut matrix = self.matrix.get();
        let changed = matrix.tx != x;
        matrix.tx = x;
        self.matrix.set(matrix);
        self.set_transformed_by_script(true);
        changed
    }

    fn y(&self) -> Twips {
        self.matrix.get().ty
    }

    fn set_y(&self, y: Twips) -> bool {
        let mut matrix = self.matrix.get();
        let changed = matrix.ty != y;
        matrix.ty = y;
        self.matrix.set(matrix);
        self.set_transformed_by_script(true);
        changed
    }

    /// Caches the scale and rotation factors for this display object, if necessary.
    /// Calculating these requires heavy trig ops, so we only do it when `_xscale`, `_yscale` or
    /// `_rotation` is accessed.
    fn cache_scale_rotation(&self) {
        if !self.scale_rotation_cached() {
            let Matrix { a, b, c, d, .. } = self.matrix.get();
            let a = f64::from(a);
            let b = f64::from(b);
            let c = f64::from(c);
            let d = f64::from(d);

            // If this object's transform matrix is:
            // [[a c tx]
            //  [b d ty]]
            // After transformation, the X-axis and Y-axis will turn into the column vectors x' = <a, b> and y' = <c, d>.
            // We derive the scale, rotation, and skew values from these transformed axes.
            // The skew value is not exposed by ActionScript, but is remembered internally.
            // xscale = len(x')
            // yscale = len(y')
            // rotation = atan2(b, a)  (the rotation of x' from the normal x-axis).
            // skew = atan2(-c, d) - atan2(b, a)  (the signed difference between y' and x' rotation)

            // This can produce some surprising results due to the overlap between flipping/rotation/skewing.
            // For example, in Flash, using Modify->Transform->Flip Horizontal and then tracing _xscale, _yscale, and _rotation
            // will output 100, 100, and 180. (a horizontal flip could also be a 180 degree skew followed by 180 degree rotation!)
            let rotation_x = f64::atan2(b, a);
            let rotation_y = f64::atan2(-c, d);
            let scale_x = f64::sqrt(a * a + b * b);
            let scale_y = f64::sqrt(c * c + d * d);
            self.rotation.set(Degrees::from_radians(rotation_x));
            self.scale_x.set(Percent::from_unit(scale_x));
            self.scale_y.set(Percent::from_unit(scale_y));
            self.skew.set(rotation_y - rotation_x);
        }
    }

    fn rotation(&self) -> Degrees {
        self.cache_scale_rotation();
        self.rotation.get()
    }

    fn set_rotation(&self, degrees: Degrees) -> bool {
        self.set_transformed_by_script(true);
        self.cache_scale_rotation();
        let changed = self.rotation.get() != degrees;
        self.rotation.set(degrees);

        // FIXME - this isn't quite correct. In Flash player,
        // trying to set rotation to NaN does nothing if the current
        // matrix 'b' and 'd' terms are both zero. However, if one
        // of those terms is non-zero, then the entire matrix gets
        // modified in a way that depends on its starting values.
        // I haven't been able to figure out how to reproduce those
        // values, so for now, we never modify the matrix if the
        // rotation is NaN. Hopefully, there are no SWFs depending
        // on the weird behavior when b or d is non-zero.
        if degrees.into_radians().is_nan() {
            return changed;
        }

        let skew = self.skew.get();
        let cos_x = f64::cos(degrees.into_radians());
        let sin_x = f64::sin(degrees.into_radians());
        let cos_y = f64::cos(degrees.into_radians() + skew);
        let sin_y = f64::sin(degrees.into_radians() + skew);
        let scale_x = self.scale_x.get().unit();
        let scale_y = self.scale_y.get().unit();
        let mut matrix = self.matrix.get();
        matrix.a = (scale_x * cos_x) as f32;
        matrix.b = (scale_x * sin_x) as f32;
        matrix.c = (scale_y * -sin_y) as f32;
        matrix.d = (scale_y * cos_y) as f32;
        self.matrix.set(matrix);

        changed
    }

    fn scale_x(&self) -> Percent {
        self.cache_scale_rotation();
        self.scale_x.get()
    }

    fn set_scale_x(&self, mut value: Percent) -> bool {
        let changed = self.scale_x.get() != value;
        self.set_transformed_by_script(true);
        self.cache_scale_rotation();
        self.scale_x.set(value);

        // Note - in order to match Flash's behavior, the 'scale_x' field is set to NaN
        // (which gets reported back to ActionScript), but we treat it as 0 for
        // the purposes of updating the matrix
        if value.percent().is_nan() {
            value = 0.0.into();
        }

        // Similarly, a rotation of `NaN` can be reported to ActionScript, but we
        // treat it as 0.0 when calculating the matrix
        let mut rot = self.rotation.get().into_radians();
        if rot.is_nan() {
            rot = 0.0;
        }

        let cos = f64::cos(rot);
        let sin = f64::sin(rot);
        let mut matrix = self.matrix.get();
        matrix.a = (cos * value.unit()) as f32;
        matrix.b = (sin * value.unit()) as f32;
        self.matrix.set(matrix);

        changed
    }

    fn scale_y(&self) -> Percent {
        self.cache_scale_rotation();
        self.scale_y.get()
    }

    fn set_scale_y(&self, mut value: Percent) -> bool {
        let changed = self.scale_y.get() != value;
        self.set_transformed_by_script(true);
        self.cache_scale_rotation();
        self.scale_y.set(value);

        // Note - in order to match Flash's behavior, the 'scale_y' field is set to NaN
        // (which gets reported back to ActionScript), but we treat it as 0 for
        // the purposes of updating the matrix
        if value.percent().is_nan() {
            value = 0.0.into();
        }

        // Similarly, a rotation of `NaN` can be reported to ActionScript, but we
        // treat it as 0.0 when calculating the matrix
        let mut rot = self.rotation.get().into_radians();
        if rot.is_nan() {
            rot = 0.0;
        }

        let skew = self.skew.get();
        let cos = f64::cos(rot + skew);
        let sin = f64::sin(rot + skew);
        let mut matrix = self.matrix.get();
        matrix.c = (-sin * value.unit()) as f32;
        matrix.d = (cos * value.unit()) as f32;
        self.matrix.set(matrix);

        changed
    }

    fn name(&self) -> Option<AvmString<'gc>> {
        self.name.get()
    }

    fn set_name(this: &Write<Self>, name: AvmString<'gc>) {
        unlock!(this, Self, name).set(Some(name));
    }

    fn filters(&self) -> Ref<'_, [Filter]> {
        Ref::map(self.cell.borrow(), |c| &*c.filters)
    }

    fn set_filters(&self, filters: Box<[Filter]>) -> bool {
        let mut write = self.cell.borrow_mut();
        let changed = filters != write.filters;
        write.filters = filters;
        drop(write);
        if changed {
            self.recheck_cache_as_bitmap();
        }
        changed
    }

    fn alpha(&self) -> f64 {
        f64::from(self.color_transform().a_multiply)
    }

    fn set_alpha(&self, value: f64) -> bool {
        self.set_transformed_by_script(true);
        let value = Fixed8::from_f64(value);
        let mut tf = self.color_transform.get();
        let changed = tf.a_multiply != value;
        tf.a_multiply = value;
        self.color_transform.set(tf);
        changed
    }

    fn clip_depth(&self) -> Depth {
        self.clip_depth.get()
    }

    fn set_clip_depth(&self, depth: Depth) {
        self.clip_depth.set(depth);
    }

    fn parent(&self) -> Option<DisplayObject<'gc>> {
        self.parent.get()
    }

    /// You should almost always use `DisplayObject.set_parent` instead, which
    /// properly handles 'orphan' movie clips
    fn set_parent_ignoring_orphan_list(this: &Write<Self>, parent: Option<DisplayObject<'gc>>) {
        unlock!(this, Self, parent).set(parent)
    }

    fn avm1_removed(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::AVM1_REMOVED)
    }

    fn avm1_pending_removal(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::AVM1_PENDING_REMOVAL)
    }

    pub fn should_skip_next_enter_frame(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::SKIP_NEXT_ENTER_FRAME)
    }

    pub fn set_skip_next_enter_frame(&self, skip: bool) {
        self.set_flag(DisplayObjectFlags::SKIP_NEXT_ENTER_FRAME, skip);
    }

    fn set_avm1_removed(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::AVM1_REMOVED, value);
    }

    fn set_avm1_pending_removal(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::AVM1_PENDING_REMOVAL, value);
    }

    fn scale_rotation_cached(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::SCALE_ROTATION_CACHED)
    }

    fn set_scale_rotation_cached(&self, set_flag: bool) {
        let flags = if set_flag {
            self.flags.get() | DisplayObjectFlags::SCALE_ROTATION_CACHED
        } else {
            self.flags.get() - DisplayObjectFlags::SCALE_ROTATION_CACHED
        };
        self.flags.set(flags);
    }

    pub fn sound_transform(&self) -> SoundTransform {
        self.sound_transform.get()
    }

    pub fn set_sound_transform(&self, sound_transform: SoundTransform) {
        self.sound_transform.set(sound_transform);
    }

    fn visible(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::VISIBLE)
    }

    fn set_visible(&self, value: bool) -> bool {
        let changed = self.visible() != value;
        self.set_flag(DisplayObjectFlags::VISIBLE, value);
        changed
    }

    fn blend_mode(&self) -> ExtendedBlendMode {
        self.blend_mode.get()
    }

    fn set_blend_mode(&self, value: ExtendedBlendMode) -> bool {
        self.blend_mode.replace(value) != value
    }

    fn blend_shader(&self) -> Option<PixelBenderShaderHandle> {
        self.cell.borrow().blend_shader.clone()
    }

    fn set_blend_shader(&self, value: Option<PixelBenderShaderHandle>) {
        self.cell.borrow_mut().blend_shader = value;
    }

    /// The opaque background color of this display object.
    /// The bounding box of the display object will be filled with this color.
    fn opaque_background(&self) -> Option<Color> {
        self.opaque_background.get()
    }

    /// The opaque background color of this display object.
    /// The bounding box of the display object will be filled with the given color. This also
    /// triggers cache-as-bitmap behavior. Only solid backgrounds are supported; the alpha channel
    /// is ignored.
    fn set_opaque_background(&self, value: Option<Color>) -> bool {
        let value = value.map(|mut color| {
            color.a = 255;
            color
        });
        let changed = self.opaque_background.get() != value;
        self.opaque_background.set(value);
        changed
    }

    fn is_root(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::IS_ROOT)
    }

    fn set_is_root(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::IS_ROOT, value);
    }

    fn lock_root(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::LOCK_ROOT)
    }

    fn set_lock_root(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::LOCK_ROOT, value);
    }

    fn transformed_by_script(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::TRANSFORMED_BY_SCRIPT)
    }

    fn set_transformed_by_script(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::TRANSFORMED_BY_SCRIPT, value);
    }

    fn placed_by_avm1_script(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::PLACED_BY_AVM1_SCRIPT)
    }

    fn set_placed_by_avm1_script(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::PLACED_BY_AVM1_SCRIPT, value);
    }

    fn placed_by_avm2_script(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::PLACED_BY_AVM2_SCRIPT)
    }

    fn set_placed_by_avm2_script(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::PLACED_BY_AVM2_SCRIPT, value);
    }

    fn manual_frame_construct(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::MANUAL_FRAME_CONSTRUCT)
    }

    fn set_manual_frame_construct(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::MANUAL_FRAME_CONSTRUCT, value);
    }

    fn is_bitmap_cached_preference(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::CACHE_AS_BITMAP)
    }

    fn set_bitmap_cached_preference(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::CACHE_AS_BITMAP, value);
        self.recheck_cache_as_bitmap();
    }

    fn set_aether_adaptive_avatar_cache_candidate(&self, value: bool) {
        if self.contains_flag(DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_CANDIDATE) == value {
            return;
        }

        let was_active = self.aether_adaptive_avatar_cache_active();
        self.set_flag(
            DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_ACTIVE,
            false,
        );
        self.set_flag(
            DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_CANDIDATE,
            value,
        );
        self.aether_adaptive_avatar_cache_clean_frames.set(0);
        self.aether_adaptive_avatar_cache_matrix
            .set(self.aether_adaptive_avatar_cache_matrix_components());

        if was_active {
            self.recheck_cache_as_bitmap();
        }
    }

    fn set_aether_adaptive_avatar_cache_root_candidate(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_ROOT, value);
        self.set_aether_adaptive_avatar_cache_candidate(value);
    }

    fn aether_adaptive_avatar_cache_matrix_components(&self) -> [f32; 4] {
        let matrix = self.matrix.get();
        [matrix.a, matrix.b, matrix.c, matrix.d]
    }

    fn aether_adaptive_avatar_cache_active(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_ACTIVE)
    }

    /// Update the internal stable-avatar cache contribution before CACHE_INVALIDATED is cleared.
    ///
    /// A candidate is cached only after several completely clean rendered frames. The first
    /// descendant visual mutation deactivates the internal contribution before that dirty frame
    /// is drawn. Authored cacheAsBitmap and filter-required caches remain authoritative.
    #[cfg(test)]
    fn update_aether_adaptive_avatar_cache(&self, optimization_enabled: bool) {
        self.update_aether_adaptive_avatar_cache_with_transform(
            optimization_enabled,
            self.aether_adaptive_avatar_cache_matrix_components(),
        );
    }

    fn update_aether_adaptive_avatar_cache_with_transform(
        &self,
        optimization_enabled: bool,
        current_matrix: [f32; 4],
    ) {
        if !self.contains_flag(DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_CANDIDATE) {
            return;
        }

        let has_filters = !self.cell.borrow().filters.is_empty();
        let eligible = optimization_enabled
            && !has_filters
            && self.blend_mode.get() == ExtendedBlendMode::Normal
            && self.opaque_background.get().is_none();
        let transform_changed = current_matrix != self.aether_adaptive_avatar_cache_matrix.get();
        if transform_changed {
            self.aether_adaptive_avatar_cache_matrix.set(current_matrix);
        }
        let invalidated =
            transform_changed || self.contains_flag(DisplayObjectFlags::CACHE_INVALIDATED);

        if !eligible || invalidated {
            self.aether_adaptive_avatar_cache_clean_frames.set(0);
            if self.aether_adaptive_avatar_cache_active() {
                self.set_flag(
                    DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_ACTIVE,
                    false,
                );
                self.recheck_cache_as_bitmap();
            }
            return;
        }

        if self.aether_adaptive_avatar_cache_active() {
            return;
        }

        let clean_frames = self
            .aether_adaptive_avatar_cache_clean_frames
            .get()
            .saturating_add(1);
        self.aether_adaptive_avatar_cache_clean_frames
            .set(clean_frames);
        if clean_frames >= AETHER_ADAPTIVE_AVATAR_CACHE_STABLE_FRAMES {
            self.set_flag(
                DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_ACTIVE,
                true,
            );
            self.recheck_cache_as_bitmap();
        }
    }

    fn bitmap_cache_mut(&self) -> RefMut<'_, Option<BitmapCache>> {
        RefMut::map(self.cell.borrow_mut(), |c| &mut c.cache)
    }

    /// Invalidates a cached bitmap, if it exists.
    /// This may only be called once per frame - the first call will return true, regardless of
    /// if there was a cache.
    /// Any subsequent calls will return false, indicating that you do not need to invalidate the ancestors.
    /// This is reset during rendering.
    fn invalidate_cached_bitmap(&self) -> bool {
        if self.contains_flag(DisplayObjectFlags::CACHE_INVALIDATED) {
            return false;
        }
        if let Some(cache) = &mut *self.bitmap_cache_mut() {
            cache.make_dirty();
        }
        self.set_flag(DisplayObjectFlags::CACHE_INVALIDATED, true);
        true
    }

    /// Invalidate this object's cache even if normal per-frame invalidation has already propagated.
    ///
    /// Viewport changes can alter device-pixel bounds without changing the display object's local
    /// transform. Every live cache must therefore be rebuilt against the new stage matrix.
    fn invalidate_bitmap_cache_for_viewport_change(&self) {
        if let Some(cache) = &mut *self.bitmap_cache_mut() {
            cache.make_dirty();
        }
        self.set_flag(DisplayObjectFlags::CACHE_INVALIDATED, true);
    }

    fn clear_invalidate_flag(&self) {
        self.set_flag(DisplayObjectFlags::CACHE_INVALIDATED, false);
    }

    fn recheck_cache_as_bitmap(&self) {
        let mut write = self.cell.borrow_mut();
        let should_cache = self.is_bitmap_cached_preference()
            || self.aether_adaptive_avatar_cache_active()
            || !write.filters.is_empty();
        if should_cache {
            write.cache.get_or_insert_default();
        } else {
            write.cache = None;
        }
    }

    fn instantiated_by_timeline(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::INSTANTIATED_BY_TIMELINE)
    }

    fn set_instantiated_by_timeline(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::INSTANTIATED_BY_TIMELINE, value);
    }

    fn has_scroll_rect(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::HAS_SCROLL_RECT)
    }

    fn set_has_scroll_rect(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::HAS_SCROLL_RECT, value);
    }

    fn has_explicit_name(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::HAS_EXPLICIT_NAME)
    }

    fn set_has_explicit_name(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::HAS_EXPLICIT_NAME, value);
    }

    fn masker(&self) -> Option<DisplayObject<'gc>> {
        self.masker.get()
    }

    fn set_masker(this: &Write<Self>, node: Option<DisplayObject<'gc>>) {
        unlock!(this, Self, masker).set(node);
    }

    fn maskee(&self) -> Option<DisplayObject<'gc>> {
        self.maskee.get()
    }

    fn set_maskee(this: &Write<Self>, node: Option<DisplayObject<'gc>>) {
        unlock!(this, Self, maskee).set(node);
    }

    fn meta_data(&self) -> Option<Avm2Object<'gc>> {
        self.meta_data.get()
    }

    fn set_meta_data(this: &Write<Self>, value: Avm2Object<'gc>) {
        unlock!(this, Self, meta_data).set(Some(value));
    }

    pub fn has_matrix3d_stub(&self) -> bool {
        self.contains_flag(DisplayObjectFlags::HAS_MATRIX3D_STUB)
    }

    pub fn set_has_matrix3d_stub(&self, value: bool) {
        self.set_flag(DisplayObjectFlags::HAS_MATRIX3D_STUB, value)
    }
}

/// Indicates which kind of bounds should be returned by `self_bounds`.
/// In most cases `BoundsMode::Engine` should be used.
#[derive(Copy, Clone, Debug)]
pub enum BoundsMode {
    /// The bounds visible on the stage (e.g. takes MorphShape ratio into
    /// account). Used for hit testing and rendering.
    Engine,

    /// The bounds returned by ActionScript (e.g. doesn't take MorphShape
    /// ratio into account - always uses ratio 0 AKA start shape).
    /// This is used in AVM1 in MovieClip::getBounds(), getRect(), _width, _height, hitTest (object)
    /// Used in AVM2 in DO::getBounds(), getRect(), width, height, hitTestObject()
    /// Used in both AVM1 and AVM2 for Transform.pixelBounds.
    Script,
}

struct DrawCacheInfo {
    handle: BitmapHandle,
    dirty: bool,
    base_transform: Transform,
    bounds_offset: Point<Twips>,
    draw_offset: Point<i32>,
    logical_width: u32,
    logical_height: u32,
    filters: Vec<Filter>,
}

fn bitmap_cache_output_intersects_viewport(
    bounds: Rectangle<Twips>,
    filter_rect: Rectangle<i32>,
    viewport_width: u32,
    viewport_height: u32,
) -> bool {
    let output_bounds = Rectangle {
        x_min: bounds.x_min + Twips::from_pixels_i32(filter_rect.x_min),
        x_max: bounds.x_min + Twips::from_pixels_i32(filter_rect.x_max),
        y_min: bounds.y_min + Twips::from_pixels_i32(filter_rect.y_min),
        y_max: bounds.y_min + Twips::from_pixels_i32(filter_rect.y_max),
    };
    let viewport = Rectangle {
        x_min: Twips::ZERO,
        x_max: Twips::from_pixels_i32(i32::try_from(viewport_width).unwrap_or(i32::MAX)),
        y_min: Twips::ZERO,
        y_max: Twips::from_pixels_i32(i32::try_from(viewport_height).unwrap_or(i32::MAX)),
    };
    output_bounds.intersects(&viewport)
}

/// Whether replacing an authored filterless cache with direct subtree rendering preserves
/// Flash's group-compositing semantics.
///
/// A normal, unmasked subtree can be composited directly because normal source-over blending is
/// associative. A nested blend group or mask is different: `cacheAsBitmap` first resolves that
/// subtree against a transparent offscreen surface, while direct rendering may blend or clip it
/// against content that lies behind the cache. That difference is visible as rectangular seams
/// in AQW maps, so the hot-cache bypass must stay conservative here.
pub(crate) fn filterless_direct_render_subtree_is_semantically_safe(
    this: DisplayObject<'_>,
) -> bool {
    if this.blend_mode() != ExtendedBlendMode::Normal
        || this.opaque_background().is_some()
        || this.masker().is_some()
        || this.maskee().is_some()
        || this.clip_depth() > 0
        || this.scroll_rect().is_some()
    {
        return false;
    }

    if let Some(container) = this.as_container() {
        for child in container.iter_render_list() {
            if !filterless_direct_render_subtree_is_semantically_safe(child) {
                return false;
            }
        }
    }

    if let Some(button) = this.as_avm2_button()
        && let Some(state_child) = button.get_state_child(button.state().into())
        && !filterless_direct_render_subtree_is_semantically_safe(state_child)
    {
        return false;
    }

    true
}

/// A verified parent direct-renders the same complete subtree, so descendants must not repeat
/// the recursive semantic-safety walk. Without this inheritance, nested AQW avatar and map caches
/// repeatedly traverse the same display tree and can turn a render frame into quadratic work.
fn filterless_direct_render_safety_check_needed(
    inherited_subtree_safe: bool,
    direct_render_window_active: bool,
) -> bool {
    direct_render_window_active && !inherited_subtree_safe
}

/// Stable names for the blend attribution report, which is read as text rather than parsed back
/// into the enum.
#[cfg(feature = "aether_diagnostics")]
fn extended_blend_mode_name(mode: ExtendedBlendMode) -> &'static str {
    match mode {
        ExtendedBlendMode::Normal => "normal",
        ExtendedBlendMode::Layer => "layer",
        ExtendedBlendMode::Multiply => "multiply",
        ExtendedBlendMode::Screen => "screen",
        ExtendedBlendMode::Lighten => "lighten",
        ExtendedBlendMode::Darken => "darken",
        ExtendedBlendMode::Difference => "difference",
        ExtendedBlendMode::Add => "add",
        ExtendedBlendMode::Subtract => "subtract",
        ExtendedBlendMode::Invert => "invert",
        ExtendedBlendMode::Alpha => "alpha",
        ExtendedBlendMode::Erase => "erase",
        ExtendedBlendMode::Overlay => "overlay",
        ExtendedBlendMode::HardLight => "hardlight",
        ExtendedBlendMode::Shader => "shader",
    }
}

/// What a blend's subtree consists of, when it consists of exactly one thing.
///
/// A blend wrapping a single draw does not need its own offscreen target: compositing a group that
/// holds one draw is the same as drawing it with the group's blend state, and 89% of the blends AQW
/// issues are that shape. Whether the saving is available depends on WHICH draw, though. A bitmap
/// or an already-rendered texture carries a trivial blend mode on the draw itself, so the bypass is
/// free. A shape does not, and worse, one shape is several overlapping mesh draws internally, so
/// blending each of them separately is not the same picture.
///
/// This reports which, so the bypass is built against what AQW actually emits rather than a guess.
#[cfg(feature = "aether_diagnostics")]
fn sole_command_kind(commands: &CommandList) -> &'static str {
    use ruffle_render::commands::Command;

    let [only] = &commands.commands[..] else {
        return "several";
    };

    match only {
        Command::RenderBitmap { .. } => "bitmap",
        Command::RenderStage3D { .. } => "stage3d",
        Command::RenderShape { .. } => "shape",
        Command::RenderAlphaMask { .. } => "alpha_mask",
        Command::DrawRect { .. } => "rect",
        Command::DrawLine { .. } => "line",
        Command::DrawLineRect { .. } => "line_rect",
        Command::Blend(..) => "blend",
        Command::PushMask | Command::ActivateMask | Command::DeactivateMask | Command::PopMask => {
            "mask"
        }
    }
}

pub fn render_base<'gc>(
    this: DisplayObject<'gc>,
    context: &mut RenderContext<'_, 'gc>,
    options: RenderOptions,
) {
    #[cfg(feature = "aether_metrics")]
    crate::aether_metrics::display_object_entered();

    if options.skip_masks && this.maskee().is_some() {
        // Skip rendering masks (unless we are rendering one explicitly).
        #[cfg(feature = "aether_metrics")]
        crate::aether_metrics::display_object_mask_skipped();
        return;
    }

    if options.apply_transform {
        let transform = this.base().transform(options.apply_matrix);
        context.transform_stack.push(&transform);
    }

    let blend_mode = this.blend_mode();
    let original_commands = if blend_mode != ExtendedBlendMode::Normal {
        Some(std::mem::take(&mut context.commands))
    } else {
        None
    };

    let mut bitmap_cache_culled = false;

    // AQW contains large cacheAsBitmap animations whose cache is explicitly invalidated every
    // frame. With no filters, opaque background, or non-normal blend, drawing those contents
    // directly is visually equivalent while avoiding an offscreen render plus texture copy. The
    // cache itself learns that it is hot before this path activates, so static cacheAsBitmap
    // objects continue to receive normal cache hits.
    #[cfg(feature = "aether_performance")]
    let filterless_hot_cache_candidate =
        crate::aether_performance::filterless_hot_cache_bypass_enabled()
            && this.is_bitmap_cached_preference()
            && this.filters().is_empty()
            && blend_mode == ExtendedBlendMode::Normal
            && this.opaque_background().is_none()
            && context.transform_stack.transform().color_transform == ColorTransform::default();
    #[cfg(not(feature = "aether_performance"))]
    let filterless_hot_cache_candidate = false;

    let mut bitmap_cache_direct =
        if context.use_bitmap_cache && this.is_bitmap_cached() && filterless_hot_cache_candidate {
            let direct_render_window_active = {
                let base = this.base();
                let cache = base.bitmap_cache_mut();
                cache
                    .as_ref()
                    .is_some_and(BitmapCache::has_filterless_direct_render_frames)
            };

            if direct_render_window_active {
                let subtree_safe = !filterless_direct_render_safety_check_needed(
                    context.filterless_direct_subtree_safe,
                    direct_render_window_active,
                ) || filterless_direct_render_subtree_is_semantically_safe(this);

                let base = this.base();
                let mut cache = base.bitmap_cache_mut();
                if subtree_safe {
                    cache
                        .as_mut()
                        .is_some_and(BitmapCache::take_filterless_direct_render_frame)
                } else {
                    if let Some(cache) = cache.as_mut() {
                        cache.cancel_filterless_direct_rendering();
                    }
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

    let cache_info = if context.use_bitmap_cache && this.is_bitmap_cached() && !bitmap_cache_direct
    {
        #[cfg(feature = "aether_metrics")]
        crate::aether_metrics::bitmap_cache_check();

        let base_transform = context.transform_stack.transform();
        let stage_matrix = context.stage.view_matrix();

        {
            #[cfg(feature = "aether_metrics")]
            crate::aether_metrics::bitmap_cache_full_evaluation();

            let mut cache_info: Option<DrawCacheInfo> = None;
            #[cfg(feature = "aether_diagnostics")]
            let mut filter_room_report: Option<(Vec<String>, u32, u32, u32, u32)> = None;
            let bounds: Rectangle<Twips> = this.render_bounds_with_transform(
                &base_transform.matrix,
                false, // we want to do the filter growth for this object ourselves, to know the offsets
                &stage_matrix,
            );
            let bounds_offset = Point::new(
                bounds.x_min - base_transform.matrix.tx,
                bounds.y_min - base_transform.matrix.ty,
            );
            let name = this.name();
            let mut filters: Vec<Filter> = this.filters().to_owned();
            let swf_version = this.swf_version();
            filters.retain(|f| !f.impotent());

            if let Some(cache) = &mut *this.base().bitmap_cache_mut() {
                let width = bounds.width().to_pixels().ceil().max(0.0);
                let height = bounds.height().to_pixels().ceil().max(0.0);
                if width <= u16::MAX as f64 && height <= u16::MAX as f64 {
                    let width = width as u32;
                    let height = height as u32;
                    let mut filter_rect = Rectangle {
                        x_min: Twips::ZERO,
                        x_max: Twips::from_pixels_i32(width as i32),
                        y_min: Twips::ZERO,
                        y_max: Twips::from_pixels_i32(height as i32),
                    };
                    for filter in &mut filters {
                        // Scaling is done by *stage view matrix* only, nothing in-between.
                        //
                        // Tried the other way and it was wrong: a glow that shrinks with its object
                        // is nearly gone on an avatar drawn at a third size, which is how AQW draws
                        // them. Skua keeps it the size it is on screen, and so does upstream.
                        filter.scale(stage_matrix.a, stage_matrix.d);
                        filter_rect = filter.calculate_dest_rect(filter_rect);
                    }
                    let filter_rect = Rectangle {
                        x_min: filter_rect.x_min.to_pixels().floor() as i32,
                        x_max: filter_rect.x_max.to_pixels().ceil() as i32,
                        y_min: filter_rect.y_min.to_pixels().floor() as i32,
                        y_max: filter_rect.y_max.to_pixels().ceil() as i32,
                    };
                    let draw_offset = Point::new(filter_rect.x_min, filter_rect.y_min);
                    let texture_width = filter_rect.width().max(0) as u32;
                    let texture_height = filter_rect.height().max(0) as u32;
                    // This is where a filter is given room, so it is where to look when one is
                    // drawn without any. Reported after the cache borrow is released, because
                    // describing the ancestry reads the same objects.
                    #[cfg(feature = "aether_diagnostics")]
                    if !filters.is_empty() {
                        filter_room_report = Some((
                            crate::aether_diagnostics::filter_names(&filters),
                            width,
                            height,
                            texture_width,
                            texture_height,
                        ));
                    }
                    #[cfg(feature = "aether_performance")]
                    let adaptive_avatar_cache_only =
                        this.base().aether_adaptive_avatar_cache_active()
                            && !this.is_bitmap_cached_preference();
                    #[cfg(not(feature = "aether_performance"))]
                    let adaptive_avatar_cache_only = false;
                    let viewport = context.renderer.viewport_dimensions();
                    if !context.is_offscreen
                        && !bitmap_cache_output_intersects_viewport(
                            bounds,
                            filter_rect,
                            viewport.width,
                            viewport.height,
                        )
                    {
                        // Timelines and frame scripts still advance normally. Only the expensive
                        // cache rebuild/filter pass is omitted while its complete filtered output
                        // is outside the physical viewport. Leave the cache dirty so returning
                        // onscreen rebuilds it before the first visible draw.
                        bitmap_cache_culled = true;
                    } else if adaptive_avatar_cache_only
                        && !adaptive_avatar_cache_dimensions_allowed(texture_width, texture_height)
                    {
                        // Adaptive avatar caches are an optimization, not authored Flash
                        // semantics. Never let a single elaborate character retain a huge live
                        // render target; direct rendering is both safer and more predictable.
                        cache.clear();
                        cache_info = None;
                    } else if let Some(invalidation_reason) =
                        cache.dirty_reason(&base_transform.matrix, width, height, &stage_matrix)
                    {
                        let filterless_output_pixels =
                            u64::from(texture_width) * u64::from(texture_height);
                        let filterless_direct_threshold_reached = filterless_hot_cache_candidate
                            && filters.is_empty()
                            && filterless_output_pixels >= FILTERLESS_HOT_CACHE_MIN_PIXELS
                            && cache.note_filterless_rebuild();
                        let filterless_direct_subtree_safe = filterless_direct_threshold_reached
                            && (context.filterless_direct_subtree_safe
                                || filterless_direct_render_subtree_is_semantically_safe(this));
                        if filterless_direct_subtree_safe {
                            cache.begin_filterless_direct_rendering();
                            bitmap_cache_direct = cache.take_filterless_direct_render_frame();
                            cache_info = None;
                        } else {
                            #[cfg(not(feature = "aether_diagnostics"))]
                            let _ = invalidation_reason;
                            #[cfg(feature = "aether_metrics")]
                            crate::aether_metrics::bitmap_cache_rebuild();

                            #[cfg(feature = "aether_diagnostics")]
                            if crate::aether_diagnostics::cache_offender_enabled() {
                                crate::aether_diagnostics::record_cache_rebuild(
                                    crate::aether_diagnostics::CacheRebuildObservation {
                                        key: this.as_ptr() as usize as u64,
                                        descriptor: crate::aether_diagnostics::DisplayObjectDescriptor::from_display_object(this),
                                        source_width: width,
                                        source_height: height,
                                        texture_width,
                                        texture_height,
                                        filter_names: crate::aether_diagnostics::filter_names(&filters),
                                        invalidation_reason,
                                    },
                                );
                            }

                            #[cfg(feature = "aether_diagnostics")]
                            let cache_update_started = std::time::Instant::now();
                            cache.update(
                                context.renderer,
                                base_transform.matrix,
                                width,
                                height,
                                texture_width,
                                texture_height,
                                draw_offset,
                                bounds_offset,
                                stage_matrix.a,
                                stage_matrix.d,
                                swf_version,
                                if adaptive_avatar_cache_only || aqw_bounded_cache_texture_reuse() {
                                    BitmapCacheTexturePolicy::BoundedReuse
                                } else {
                                    BitmapCacheTexturePolicy::Exact
                                },
                            );
                            #[cfg(feature = "aether_diagnostics")]
                            crate::aether_diagnostics::record_cache_update(
                                this.as_ptr() as usize as u64,
                                cache_update_started.elapsed(),
                            );
                            let logical_size = cache.output_size();
                            cache_info = cache.handle().map(|handle| DrawCacheInfo {
                                handle,
                                dirty: true,
                                base_transform,
                                bounds_offset,
                                draw_offset,
                                logical_width: logical_size.0,
                                logical_height: logical_size.1,
                                filters,
                            });
                        }
                    } else {
                        cache.note_cache_hit();
                        #[cfg(feature = "aether_metrics")]
                        crate::aether_metrics::bitmap_cache_hit();

                        let logical_size = cache.output_size();
                        cache_info = cache.handle().map(|handle| DrawCacheInfo {
                            handle,
                            dirty: false,
                            base_transform,
                            bounds_offset,
                            draw_offset,
                            logical_width: logical_size.0,
                            logical_height: logical_size.1,
                            filters,
                        });
                    }
                } else {
                    #[cfg(feature = "aether_metrics")]
                    crate::aether_metrics::bitmap_cache_oversize();

                    if !cache.warned_for_oversize {
                        tracing::warn!(
                            "Skipping cacheAsBitmap for incredibly large object {:?} ({width} x {height})",
                            name
                        );
                        cache.warned_for_oversize = true;
                    }
                    cache.clear();
                    cache_info = None;
                }
            }

            #[cfg(feature = "aether_diagnostics")]
            if let Some((names, width, height, texture_width, texture_height)) = filter_room_report {
                crate::aether_diagnostics::record_filter_ancestry(
                    this,
                    &names,
                    width,
                    height,
                    texture_width,
                    texture_height,
                );
            }

            cache_info
        }
    } else {
        None
    };

    if bitmap_cache_culled {
        if let Some(original_commands) = original_commands {
            context.commands = original_commands;
        }
        if options.apply_transform {
            context.transform_stack.pop();
        }
        return;
    }

    // We can't hold `cache` (which will hold `base`), so this is split up
    if let Some(cache_info) = cache_info {
        // In order to render an object to a texture, we need to draw its entire bounds.
        // Calculate the offset from tx/ty in order to accommodate any drawings that extend the bounds
        // negatively
        let offset_x =
            cache_info.bounds_offset.x + Twips::from_pixels_i32(cache_info.draw_offset.x);
        let offset_y =
            cache_info.bounds_offset.y + Twips::from_pixels_i32(cache_info.draw_offset.y);

        if cache_info.dirty {
            #[cfg(feature = "aether_metrics")]
            crate::aether_metrics::offscreen_cache_draw();

            let mut transform_stack = TransformStack::new();
            transform_stack.push(&Transform {
                color_transform: Default::default(),
                matrix: Matrix {
                    tx: -offset_x,
                    ty: -offset_y,
                    ..cache_info.base_transform.matrix
                },
                perspective_projection: cache_info.base_transform.perspective_projection,
            });
            let mut offscreen_context = RenderContext {
                renderer: context.renderer,
                commands: CommandList::new(),
                cache_draws: context.cache_draws,
                gc_context: context.gc_context,
                library: context.library,
                transform_stack: &mut transform_stack,
                is_offscreen: true,
                slice_pass: Default::default(),
                use_bitmap_cache: true,
                filterless_direct_subtree_safe: false,
                stage: context.stage,
            };
            #[cfg(feature = "aether_diagnostics")]
            let offscreen_started = std::time::Instant::now();
            this.render_self(&mut offscreen_context);
            #[cfg(feature = "aether_diagnostics")]
            crate::aether_diagnostics::record_cache_offscreen_draw(
                this.as_ptr() as usize as u64,
                offscreen_started.elapsed(),
            );
            offscreen_context.cache_draws.push(BitmapCacheEntry {
                handle: cache_info.handle.clone(),
                commands: offscreen_context.commands,
                clear: this.opaque_background().unwrap_or_default(),
                logical_width: cache_info.logical_width,
                logical_height: cache_info.logical_height,
                filters: cache_info.filters,
            });
        }

        // When rendering it back, ensure we're only keeping the translation - scale/rotation is within the image already
        apply_standard_mask_and_scroll(
            this,
            context,
            |context| {
                context.commands.render_bitmap(
                    cache_info.handle.clone(),
                    Transform {
                        matrix: Matrix {
                            tx: context.transform_stack.transform().matrix.tx + offset_x,
                            ty: context.transform_stack.transform().matrix.ty + offset_y,
                            ..Default::default()
                        },
                        color_transform: cache_info.base_transform.color_transform,
                        perspective_projection: cache_info.base_transform.perspective_projection,
                    },
                    true,
                    PixelSnapping::Always, // cacheAsBitmap forces pixel snapping
                    Some(BitmapSize {
                        width: cache_info.logical_width,
                        height: cache_info.logical_height,
                    }),
                )
            },
            &options,
            // Replaying a finished image, which carries its own scaling and ignores the stack's.
            false,
        );
    } else {
        if let Some(background) = this.opaque_background() {
            // This is intended for use with cacheAsBitmap, but can be set for non-cached objects too
            // It wants the entire bounding box to be cleared before any draws happen
            let bounds: Rectangle<Twips> = this.render_bounds_with_transform(
                &context.transform_stack.transform().matrix,
                true,
                &context.stage.view_matrix(),
            );
            context
                .commands
                .draw_rect(background, Matrix::create_box_from_rectangle(&bounds));
        }
        apply_standard_mask_and_scroll(
            this,
            context,
            |context| {
                let previous_filterless_direct_subtree_safe =
                    context.filterless_direct_subtree_safe;
                if bitmap_cache_direct {
                    context.filterless_direct_subtree_safe = true;
                }
                this.render_self(context);
                context.filterless_direct_subtree_safe = previous_filterless_direct_subtree_safe;
            },
            &options,
            true,
        );
    }

    if let Some(original_commands) = original_commands {
        let sub_commands = std::mem::replace(&mut context.commands, original_commands);
        // If there's nothing to draw, throw away the blend entirely.
        if !sub_commands.is_empty() {
            let render_blend_mode = if let ExtendedBlendMode::Shader = blend_mode {
                // Note - Flash appears to let you set blend mode to shader
                // without having blend shader set.  In this case, Flash seems
                // to fall back to a normal blend.
                if let Some(shader) = this.blend_shader() {
                    RenderBlendMode::Shader(shader)
                } else {
                    RenderBlendMode::Builtin(swf::BlendMode::Normal)
                }
            } else {
                RenderBlendMode::Builtin(blend_mode.try_into().unwrap())
            };
            // Where these commands actually land, so the backend can size the blend's sub-target to
            // them instead of to the whole stage. Measured before the transform stack is popped, so
            // it is in the current render target's space — the same space the commands were recorded
            // in. `true` includes this object's own filters: a glow draws OUTSIDE the object, and a
            // target sized to the bare bounds would clip it.
            let stage_matrix = context.stage.view_matrix();
            let blend_bounds = this.render_bounds_with_transform(
                &context.transform_stack.transform().matrix,
                true,
                &stage_matrix,
            );
            #[cfg(feature = "aether_diagnostics")]
            if crate::aether_diagnostics::blend_attribution_enabled() {
                // Bounds are in twips on the render target, and the backend allocates and
                // composites a sub-target covering them, so pixel area is what a blend costs.
                let pixel_area = (blend_bounds.width().to_pixels().max(0.0)
                    * blend_bounds.height().to_pixels().max(0.0))
                    as u64;
                crate::aether_diagnostics::record_blend(
                    extended_blend_mode_name(blend_mode),
                    this,
                    pixel_area,
                    sub_commands.commands.len() as u64,
                    sole_command_kind(&sub_commands),
                );
            }

            context
                .commands
                .blend(sub_commands, render_blend_mode, Some(blend_bounds));
        }
    }

    if options.apply_transform {
        context.transform_stack.pop();
    }
}

/// How far a cell's clip reaches past its neighbour's, in **screen** pixels.
///
/// Only inwards, between cells. Reaching past the object's own outer edge would let a corner draw a
/// sliver of the middle beyond where the object ends.
///
/// Screen pixels rather than the object's own, because the object's own are worth however much it
/// has been scaled by. Half a pixel of overlap on a tooltip stretched ten times over is five pixels
/// of the middle band painted across the corner beside it, which is enough to swallow the rounded
/// corner whole -- the taller the tooltip, the squarer its corners came out.
const SLICE_BLEED: f64 = 0.5;

/// One axis of a nine-slice: where each of the three bands starts and how much it is scaled by.
///
/// The outer two bands keep the size they were authored at, whatever the object is scaled to, and
/// the middle one takes up whatever is left. That is the whole point of the grid: a border stays a
/// border instead of being stretched into an ellipse along with everything else.
#[derive(Clone, Copy)]
struct SliceAxis {
    /// Source edges, in the object's own space.
    source: [f64; 4],
    /// Where those edges land once the object is scaled, again in the object's own space.
    dest: [f64; 4],
}

impl SliceAxis {
    /// Returns `None` when slicing would not be an improvement, and the object should be drawn the
    /// ordinary way: no room left for the middle band, a grid that is not inside the bounds, or a
    /// scale so small that the borders alone would overflow.
    fn plan(low: f64, grid_low: f64, grid_high: f64, high: f64, scale: f64) -> Option<Self> {
        if !(low < grid_low && grid_low < grid_high && grid_high < high) || !scale.is_finite() {
            return None;
        }
        let scale = scale.abs();
        if scale < 0.001 {
            return None;
        }

        let leading = grid_low - low;
        let trailing = high - grid_high;
        // The borders are drawn unscaled, so in the object's own space they have to shrink by
        // exactly as much as the object is being grown.
        let (leading_dest, trailing_dest) = (leading / scale, trailing / scale);
        let middle_dest = (high - low) - leading_dest - trailing_dest;
        if middle_dest <= 0.0 {
            return None;
        }

        Some(Self {
            source: [low, grid_low, grid_high, high],
            dest: [
                low,
                low + leading_dest,
                low + leading_dest + middle_dest,
                high,
            ],
        })
    }

    /// How much band `index` is stretched by, and where it starts.
    fn band(&self, index: usize) -> (f64, f64, f64, f64) {
        let (source_start, source_end) = (self.source[index], self.source[index + 1]);
        let (dest_start, dest_end) = (self.dest[index], self.dest[index + 1]);
        let stretch = (dest_end - dest_start) / (source_end - source_start);
        (source_start, dest_start, stretch, dest_end)
    }
}

/// The extent of the art a scaling grid describes, ignoring anything merely positioned on top.
///
/// The bands are worked out by measuring the grid against the object's edges, so the edges have to
/// be the ones the grid was authored against -- which is the border art, and nothing else. Ordinary
/// bounds are the union of every child, so an icon or a label that reaches past the frame it sits
/// in drags the measured edge out with it and every band lands somewhere the artwork never
/// intended. Two panels skinned by the same frame then slice differently depending on what is
/// sitting on them, which is why some of the toolbar's buttons kept their corners and others did
/// not.
///
/// Falls back to the object's own bounds when it holds no art at all, so a bare shape carrying a
/// grid still measures itself.
fn scaling_grid_art_bounds(this: DisplayObject<'_>) -> Rectangle<Twips> {
    let Some(container) = this.as_container() else {
        return this.bounds(BoundsMode::Engine);
    };

    let mut art: Option<Rectangle<Twips>> = None;
    for child in container.iter_render_list() {
        if !matches!(
            child,
            DisplayObject::Graphic(_) | DisplayObject::Bitmap(_) | DisplayObject::MorphShape(_)
        ) {
            continue;
        }
        let bounds = child.bounds_with_transform(&child.base().matrix(), BoundsMode::Engine);
        art = Some(match art {
            Some(art) => art.union(&bounds),
            None => bounds,
        });
    }

    // The object's own drawing counts as art too, and is already in its self bounds.
    let own = this.self_bounds(BoundsMode::Engine);
    let art = match (art, own.is_valid()) {
        (Some(art), true) => art.union(&own),
        (Some(art), false) => art,
        (None, true) => own,
        (None, false) => return this.bounds(BoundsMode::Engine),
    };
    art
}

/// Draw an object, in nine pieces if it has a scaling grid and is being scaled.
///
/// `scale9Grid` was read from the file and answered to ActionScript but never reached the renderer,
/// so an object resized by setting its width -- which is how AQW sizes every panel, tooltip and
/// message box -- had its corners stretched along with its middle. The bigger the panel the rounder
/// the corners got.
///
/// Each of the nine cells is drawn as the whole object under a transform that maps that cell's
/// source band onto its destination band, masked to the destination so the other eight stay out of
/// it.
///
/// `sliceable` is false when `draw` replays a finished image rather than drawing the object. A
/// cached object's picture already has its scaling baked in, so its draw ignores everything in the
/// transform stack except the translation -- which turns each cell's transform into a bare offset
/// applied to an unscaled bitmap, and draws the object nine times, each copy a little further up and
/// to the left. That is what moved the drop-accept button, the buff icons and the text in the aura
/// bar, and what squared off the corners of tall tooltips.
fn draw_possibly_sliced<'gc, F>(
    this: DisplayObject<'gc>,
    context: &mut RenderContext<'_, 'gc>,
    draw: &mut F,
    sliceable: bool,
) where
    F: FnMut(&mut RenderContext<'_, 'gc>),
{
    let grid = this.scaling_grid();
    if sliceable && grid.is_valid() && grid.width() > Twips::ZERO && grid.height() > Twips::ZERO {
        // The art's own edges, not the union with whatever is sitting on it. See the function.
        let bounds = scaling_grid_art_bounds(this);
        let matrix = this.base().matrix();
        // Only an object that has actually been resized, and only along its own axes.
        //
        // A grid says nothing about an object drawn at the size it was authored, and slicing one
        // is nine draws to arrive back where it started. Rotation and skew are worse than useless
        // here: the bands are computed from `a` and `d` alone, which stop describing the object's
        // size the moment it is turned, so a turned object would be sliced along the wrong axes
        // and land somewhere else entirely.
        let upright = matrix.b == 0.0 && matrix.c == 0.0;
        let scale_x = f64::from(matrix.a);
        let scale_y = f64::from(matrix.d);
        let resized = (scale_x - 1.0).abs() > 0.001 || (scale_y - 1.0).abs() > 0.001;
        // Grown, never shrunk.
        //
        // A border is kept at its drawn size by dividing it by the object's scale, so below one
        // that division makes the border band *larger* than it was drawn and the cell covering it
        // magnifies whatever it happens to reach -- a sliver of the object's own interior, smeared
        // across its corner. That is the dark wedge that appeared at the top left of the buff icons
        // and the drop-accept button, both of which are placed smaller than they were drawn.
        //
        // Nothing is lost by declining: a grid exists to stop corners stretching as a panel grows,
        // and a panel that is not growing has no corners being stretched.
        let grown = scale_x >= 0.999 && scale_y >= 0.999;
        if upright
            && resized
            && grown
            && bounds.is_valid()
            && let Some(horizontal) = SliceAxis::plan(
                bounds.x_min.to_pixels(),
                grid.x_min.to_pixels(),
                grid.x_max.to_pixels(),
                bounds.x_max.to_pixels(),
                f64::from(matrix.a),
            )
            && let Some(vertical) = SliceAxis::plan(
                bounds.y_min.to_pixels(),
                grid.y_min.to_pixels(),
                grid.y_max.to_pixels(),
                bounds.y_max.to_pixels(),
                f64::from(matrix.d),
            )
        {
            // What one of the object's own pixels is worth on screen, so the overlap below can be
            // asked for in pixels the viewer would recognise.
            //
            // The stage pushes its view matrix before rendering anything, so the window's own
            // scaling is already in here. Multiplying by it again would square it, and the overlap
            // would shrink to nothing on a large window -- which is where the seams were seen.
            let to_screen = context.transform_stack.transform().matrix;
            let screen_scale_x = f64::from(to_screen.a).abs();
            let screen_scale_y = f64::from(to_screen.d).abs();
            let bleed_x = if screen_scale_x > 0.0 {
                SLICE_BLEED / screen_scale_x
            } else {
                0.0
            };
            let bleed_y = if screen_scale_y > 0.0 {
                SLICE_BLEED / screen_scale_y
            } else {
                0.0
            };

            for row in 0..3 {
                let (source_y, dest_y, stretch_y, dest_y_end) = vertical.band(row);
                for column in 0..3 {
                    let (source_x, dest_x, stretch_x, dest_x_end) = horizontal.band(column);

                    // The cell's own destination, used both to place it and to clip it.
                    //
                    // Grown by a hair towards its neighbours. Two masks that meet exactly on a
                    // boundary can both miss the pixels the boundary lands inside, which draws a
                    // thin seam down the middle of a panel. Cells that meet agree about what is at
                    // the join -- it is the same point of the same object -- so letting them
                    // overlap there costs nothing and closes the gap.
                    let bleed_left = if column > 0 { bleed_x } else { 0.0 };
                    let bleed_right = if column < 2 { bleed_x } else { 0.0 };
                    let bleed_up = if row > 0 { bleed_y } else { 0.0 };
                    let bleed_down = if row < 2 { bleed_y } else { 0.0 };
                    let clip = Matrix::create_box(
                        (dest_x_end - dest_x + bleed_left + bleed_right) as f32,
                        (dest_y_end - dest_y + bleed_up + bleed_down) as f32,
                        Twips::from_pixels(dest_x - bleed_left),
                        Twips::from_pixels(dest_y - bleed_up),
                    );
                    let clip = context.transform_stack.transform().matrix * clip;

                    context.commands.push_mask();
                    context.commands.draw_rect(Color::WHITE, clip);
                    context.commands.activate_mask();

                    context.transform_stack.push(&Transform {
                        matrix: Matrix {
                            a: stretch_x as f32,
                            b: 0.0,
                            c: 0.0,
                            d: stretch_y as f32,
                            tx: Twips::from_pixels(dest_x - source_x * stretch_x),
                            ty: Twips::from_pixels(dest_y - source_y * stretch_y),
                        },
                        color_transform: Default::default(),
                        perspective_projection: None,
                    });
                    context.slice_pass = SlicePass::ArtOnly;
                    draw(context);
                    context.slice_pass = SlicePass::Everything;
                    context.transform_stack.pop();

                    context.commands.deactivate_mask();
                    context.commands.draw_rect(Color::WHITE, clip);
                    context.commands.pop_mask();
                }
            }

            // Everything that was positioned rather than drawn, once, under the object's ordinary
            // transform and no cell mask -- which is to say exactly where it would have been had
            // none of this happened. A caption keeps its place while the border behind it keeps its
            // thickness, which is the whole point and is what the two passes are for.
            if this.as_container().is_some() {
                context.slice_pass = SlicePass::ContentOnly;
                draw(context);
                context.slice_pass = SlicePass::Everything;
            }
            return;
        }
    }

    draw(context);
}

/// This applies the **standard** method of `mask` and `scrollRect`.
///
/// It uses the stencil buffer so that any pixel drawn in the mask will allow the inner contents to show.
/// This is what is used for most cases, except for cacheAsBitmap-on-cacheAsBitmap.
///
/// `sliceable` says whether `draw` actually draws the object, and so whether a scaling grid can be
/// honoured by drawing it in nine pieces. See `draw_possibly_sliced`.
pub fn apply_standard_mask_and_scroll<'gc, F>(
    this: DisplayObject<'gc>,
    context: &mut RenderContext<'_, 'gc>,
    mut draw: F,
    options: &RenderOptions,
    sliceable: bool,
) where
    // `FnMut` rather than `FnOnce` because a nine-sliced object is drawn once per cell.
    F: FnMut(&mut RenderContext<'_, 'gc>),
{
    let scroll_rect_matrix = if let Some(rect) = this.scroll_rect() {
        let cur_transform = context.transform_stack.transform();
        // The matrix we use for actually drawing a rectangle for cropping purposes
        // Note that we do *not* apply the translation yet
        Some(
            cur_transform.matrix
                * Matrix::scale(
                    rect.width().to_pixels() as f32,
                    rect.height().to_pixels() as f32,
                ),
        )
    } else {
        None
    };

    if let Some(rect) = this.scroll_rect() {
        // Translate everything that we render (including DisplayObject.mask)
        context.transform_stack.push(&Transform {
            matrix: Matrix::translate(-rect.x_min, -rect.y_min),
            color_transform: Default::default(),
            perspective_projection: None,
        });
    }

    let mask = this.get_render_mask();
    let mut mask_transform = ruffle_render::transform::Transform::default();
    if let RenderMask::Stencil(m) | RenderMask::Alpha(m) = mask {
        if options.apply_transform {
            mask_transform.matrix = this.global_to_local_matrix().unwrap_or_default();
        }
        mask_transform.matrix *= m.local_to_global_matrix();
    }
    if let RenderMask::Stencil(m) = mask {
        context.commands.push_mask();
        context.transform_stack.push(&mask_transform);
        m.render_self(context);
        context.transform_stack.pop();
        context.commands.activate_mask();
    }

    // There are two parts to 'DisplayObject.scrollRect':
    // a scroll effect (translation), and a crop effect.
    // This scroll is implementing by applying a translation matrix
    // when we defined 'scroll_rect_matrix'.
    // The crop is implemented as a rectangular mask using the height
    // and width provided by 'scrollRect'.

    // Note that this mask is applied *in addition to* a mask defined
    // with 'DisplayObject.mask'. We will end up rendering content that
    // lies in the intersection of the scroll rect and DisplayObject.mask,
    // which is exactly the behavior that we want.
    if let Some(rect_mat) = scroll_rect_matrix {
        context.commands.push_mask();
        // The color doesn't matter, as this is a mask.
        context.commands.draw_rect(Color::WHITE, rect_mat);
        context.commands.activate_mask();
    }

    if let RenderMask::Alpha(m) = mask {
        let original_commands = std::mem::take(&mut context.commands);

        draw_possibly_sliced(this, context, &mut draw, sliceable);

        let maskee_commands = std::mem::take(&mut context.commands);

        context.transform_stack.push(&mask_transform);
        let options = RenderOptions {
            skip_masks: false,
            apply_matrix: false,
            ..Default::default()
        };
        m.render_with_options(context, options);
        context.transform_stack.pop();

        let mask_commands = std::mem::replace(&mut context.commands, original_commands);

        // The visible result is the intersection of maskee and mask, so the maskee's own extent
        // bounds everything that can appear -- measured here, before the stack unwinds, so it is in
        // the space the commands were recorded in. `true` includes this object's filters: a glow
        // draws outside the object, and a target sized to the bare bounds would clip it.
        let stage_matrix = context.stage.view_matrix();
        let mask_bounds = this.render_bounds_with_transform(
            &context.transform_stack.transform().matrix,
            true,
            &stage_matrix,
        );
        context
            .commands
            .render_alpha_mask(maskee_commands, mask_commands, Some(mask_bounds));
    } else {
        draw_possibly_sliced(this, context, &mut draw, sliceable);
    }

    if let Some(rect_mat) = scroll_rect_matrix {
        // Draw the rectangle again after deactivating the mask,
        // to reset the stencil buffer.
        context.commands.deactivate_mask();
        context.commands.draw_rect(Color::WHITE, rect_mat);
        context.commands.pop_mask();
    }

    if let RenderMask::Stencil(m) = mask {
        context.commands.deactivate_mask();
        context.transform_stack.push(&mask_transform);
        m.render_self(context);
        context.transform_stack.pop();
        context.commands.pop_mask();
    }

    if scroll_rect_matrix.is_some() {
        // Remove the translation that we pushed
        context.transform_stack.pop();
    }
}

#[enum_trait_object(
    #[derive(Clone, Collect, Debug, Copy)]
    #[collect(no_drop)]
    pub enum DisplayObject<'gc> {
        Stage(Stage<'gc>),
        Bitmap(Bitmap<'gc>),
        Avm1Button(Avm1Button<'gc>),
        Avm2Button(Avm2Button<'gc>),
        EditText(EditText<'gc>),
        TextLine(TextLine<'gc>),
        Graphic(Graphic<'gc>),
        MorphShape(MorphShape<'gc>),
        MovieClip(MovieClip<'gc>),
        Text(Text<'gc>),
        Video(Video<'gc>),
        LoaderDisplay(LoaderDisplay<'gc>)
    }
)]
pub trait TDisplayObject<'gc>:
    'gc + Clone + Copy + Collect<'gc> + Debug + Into<DisplayObject<'gc>>
{
    fn base(self) -> Gc<'gc, DisplayObjectBase<'gc>>;

    #[no_dynamic]
    fn as_ptr(self) -> *const DisplayObjectPtr {
        Gc::as_ptr(self.base()).cast()
    }

    /// The `SCALE_ROTATION_CACHED` flag should only be set in SWFv5+.
    /// So scaling/rotation values always have to get recalculated from the matrix in SWFv4.
    /// SWF version 0 means non-SWF content (a loaded image); since loading images requires
    /// `loadMovie` (SWFv5+) or `MovieClipLoader` (SWFv6+), this can't occur in a SWFv4 context.
    /// Therefore, loaded images are supposed to work the way SWF >= 5 movies do in this regard,
    /// but the SWF version of the MovieClips created for loaded images can't inherit their
    /// version from the loading movie - they have to be reported as -1 to ActionScript.
    #[no_dynamic]
    fn set_scale_rotation_cached(self) {
        if self.swf_version() == 0 || self.swf_version() >= 5 {
            self.base().set_scale_rotation_cached(true);
        }
    }

    fn id(self) -> CharacterId;

    #[no_dynamic]
    fn depth(self) -> Depth {
        self.base().depth()
    }

    #[no_dynamic]
    fn set_depth(self, depth: Depth) {
        self.base().set_depth(depth)
    }

    /// The untransformed inherent bounding box of this object.
    /// These bounds do **not** include child DisplayObjects.
    /// To get the bounds including children, use `bounds`, `local_bounds`, or `world_bounds`.
    ///
    /// The `mode` parameter indicates which kind of bounds to return:
    /// - `BoundsMode::Engine`: Actual visual bounds (for hit testing, rendering)
    /// - `BoundsMode::Script`: Bounds as reported by ActionScript (some objects like MorphShape
    ///   always return the start shape's bounds)
    ///
    /// Implementors must override this method.
    /// Leaf DisplayObjects should return their bounds.
    /// Composite DisplayObjects that only contain children should return `Default::default()`
    fn self_bounds(self, mode: BoundsMode) -> Rectangle<Twips>;

    /// The untransformed bounding box of this object including children.
    #[no_dynamic]
    fn bounds(self, mode: BoundsMode) -> Rectangle<Twips> {
        self.bounds_with_transform(&Matrix::default(), mode)
    }

    /// The local bounding box of this object including children, in its parent's coordinate system.
    #[no_dynamic]
    fn local_bounds(self, mode: BoundsMode) -> Rectangle<Twips> {
        self.bounds_with_transform(&self.base().matrix(), mode)
    }

    /// The world bounding box of this object including children, relative to the stage.
    #[no_dynamic]
    fn world_bounds(self, mode: BoundsMode) -> Rectangle<Twips> {
        self.bounds_with_transform(&self.local_to_global_matrix(), mode)
    }

    /// The world bounding box of this object, as reported by `Transform.pixelBounds`.
    fn pixel_bounds(self, mode: BoundsMode) -> Rectangle<Twips> {
        self.world_bounds(mode)
    }

    /// Bounds used for drawing debug rects and picking objects.
    #[no_dynamic]
    fn debug_rect_bounds(self) -> Rectangle<Twips> {
        // Make the rect at least as big as highlight bounds to ensure that anything
        // interactive is also highlighted even if not included in world bounds.
        let highlight_bounds = self
            .as_interactive()
            .map(|int| int.highlight_bounds())
            .unwrap_or_default();
        self.world_bounds(BoundsMode::Engine)
            .union(&highlight_bounds)
    }

    /// Gets the bounds of this object and all children, transformed by a given matrix.
    /// This function recurses down and transforms the AABB each child before adding
    /// it to the bounding box. This gives a tighter AABB then if we simply transformed
    /// the overall AABB.
    ///
    /// The `mode` parameter indicates which kind of bounds to return.
    fn bounds_with_transform(self, matrix: &Matrix, mode: BoundsMode) -> Rectangle<Twips> {
        // A scroll rect completely overrides an object's bounds,
        // and can even grow the bounding box to be larger than the actual content
        if let Some(scroll_rect) = self.scroll_rect() {
            return *matrix
                * Rectangle {
                    x_min: Twips::ZERO,
                    y_min: Twips::ZERO,
                    x_max: scroll_rect.width(),
                    y_max: scroll_rect.height(),
                };
        }

        let mut bounds = *matrix * self.self_bounds(mode);

        if let Some(ctr) = self.as_container() {
            for child in ctr.iter_render_list() {
                let matrix = *matrix * child.base().matrix();
                bounds = bounds.union(&child.bounds_with_transform(&matrix, mode));
            }
        }

        bounds
    }

    /// Gets the **render bounds** of this object and all its children.
    /// This differs from the bounds that are exposed to Flash, in two main ways:
    /// - It may be larger if filters are applied which will increase the size of what's shown
    /// - It does not respect scroll rects
    ///
    /// Uses `BoundsMode::Engine` as this is for rendering purposes.
    fn render_bounds_with_transform(
        self,
        matrix: &Matrix,
        include_own_filters: bool,
        view_matrix: &Matrix,
    ) -> Rectangle<Twips> {
        let mut bounds = *matrix * self.self_bounds(BoundsMode::Engine);

        if let Some(ctr) = self.as_container() {
            for child in ctr.iter_render_list() {
                let matrix = *matrix * child.base().matrix();
                bounds =
                    bounds.union(&child.render_bounds_with_transform(&matrix, true, view_matrix));
            }
        }

        if include_own_filters {
            // Must agree with the cache path above, or the room reserved for a filter and the
            // filter drawn into it come out different sizes.
            for mut filter in self.filters().iter().cloned() {
                filter.scale(view_matrix.a, view_matrix.d);
                bounds = filter.calculate_dest_rect(bounds);
            }
        }

        bounds
    }

    #[no_dynamic]
    fn place_frame(self) -> u16 {
        self.base().place_frame()
    }

    #[no_dynamic]
    fn set_place_frame(self, frame: u16) {
        self.base().set_place_frame(frame)
    }

    /// Sets the matrix of this object.
    /// This does NOT invalidate the cache, as it's often used with other operations.
    /// It is the callers responsibility to do so.
    fn set_matrix(self, matrix: Matrix) {
        self.base().set_matrix(matrix);
    }

    /// Sets the color transform of this object.
    /// This does NOT invalidate the cache, as it's often used with other operations.
    /// It is the callers responsibility to do so.
    #[no_dynamic]
    fn set_color_transform(self, color_transform: ColorTransform) {
        self.base().set_color_transform(color_transform)
    }

    /// Sets the perspective projection of this object.
    /// This invalidates any ancestors cacheAsBitmap automatically.
    fn set_perspective_projection(self, perspective_projection: Option<PerspectiveProjection>) {
        if self
            .base()
            .set_perspective_projection(perspective_projection)
            && let Some(parent) = self.parent()
        {
            // Self-transform changes are automatically handled,
            // we only want to inform ancestors to avoid unnecessary invalidations for tx/ty
            parent.invalidate_cached_bitmap();
        }
    }

    /// Should only be used to implement 'Transform.concatenatedMatrix'
    #[no_dynamic]
    fn local_to_global_matrix_without_own_scroll_rect(self) -> Matrix {
        let mut node = self.parent();
        let mut matrix = self.base().matrix();
        while let Some(display_object) = node {
            // We want to transform to Stage-local coordinates,
            // so do *not* apply the Stage's matrix
            if display_object.as_stage().is_some() {
                break;
            }
            if let Some(rect) = display_object.scroll_rect() {
                matrix = Matrix::translate(-rect.x_min, -rect.y_min) * matrix;
            }
            matrix = display_object.base().matrix() * matrix;
            node = display_object.parent();
        }
        matrix
    }

    /// Returns the matrix for transforming from this object's local space to global stage space.
    fn local_to_global_matrix(self) -> Matrix {
        let mut matrix = Matrix::IDENTITY;
        if let Some(rect) = self.scroll_rect() {
            matrix = Matrix::translate(-rect.x_min, -rect.y_min) * matrix;
        }
        self.local_to_global_matrix_without_own_scroll_rect() * matrix
    }

    /// Returns the matrix for transforming from global stage to this object's local space.
    /// `None` is returned if the object has zero scale.
    #[no_dynamic]
    fn global_to_local_matrix(self) -> Option<Matrix> {
        self.local_to_global_matrix().inverse()
    }

    /// Converts a local position to a global stage position
    #[no_dynamic]
    fn local_to_global(self, local: Point<Twips>) -> Point<Twips> {
        self.local_to_global_matrix() * local
    }

    /// Converts a local position on the stage to a local position on this display object
    /// Returns `None` if the object has zero scale.
    #[no_dynamic]
    fn global_to_local(self, global: Point<Twips>) -> Option<Point<Twips>> {
        self.global_to_local_matrix().map(|matrix| matrix * global)
    }

    /// Converts the mouse position on the stage to a local position on this display object.
    /// If the object has zero scale, then the stage `TWIPS_TO_PIXELS` matrix will be used.
    /// This matches Flash's behavior for `mouseX`/`mouseY` on an object with zero scale.
    #[no_dynamic]
    fn local_mouse_position(self, context: &UpdateContext<'gc>) -> Point<Twips> {
        let stage = context.stage;
        let pixel_ratio = stage.view_matrix().a;
        let virtual_to_device = Matrix::scale(pixel_ratio, pixel_ratio);

        // Get mouse pos in global device pixels
        let global_twips = *context.mouse_position;
        let global_device_twips = virtual_to_device * global_twips;
        let global_device_pixels = Matrix::TWIPS_TO_PIXELS * global_device_twips;

        // Make transformation matrix
        let local_twips_to_global_twips = self.local_to_global_matrix();
        let twips_to_device_pixels = virtual_to_device * Matrix::TWIPS_TO_PIXELS;
        let local_twips_to_global_device_pixels =
            twips_to_device_pixels * local_twips_to_global_twips;
        let global_device_pixels_to_local_twips = local_twips_to_global_device_pixels
            .inverse()
            .unwrap_or(Matrix::IDENTITY);

        // Get local mouse position in twips
        global_device_pixels_to_local_twips * global_device_pixels
    }

    /// The `x` position in pixels of this display object in local space.
    /// Returned by the `_x`/`x` ActionScript properties.
    fn x(self) -> Twips {
        self.base().x()
    }

    /// Sets the `x` position in pixels of this display object in local space.
    /// Set by the `_x`/`x` ActionScript properties.
    /// This invalidates any ancestors cacheAsBitmap automatically.
    fn set_x(self, x: Twips) {
        if self.base().set_x(x)
            && let Some(parent) = self.parent()
        {
            // Self-transform changes are automatically handled,
            // we only want to inform ancestors to avoid unnecessary invalidations for tx/ty
            parent.invalidate_cached_bitmap();
        }
    }

    /// The `y` position in pixels of this display object in local space.
    /// Returned by the `_y`/`y` ActionScript properties.
    fn y(self) -> Twips {
        self.base().y()
    }

    /// Sets the `y` position in pixels of this display object in local space.
    /// Set by the `_y`/`y` ActionScript properties.
    /// This invalidates any ancestors cacheAsBitmap automatically.
    fn set_y(self, y: Twips) {
        if self.base().set_y(y)
            && let Some(parent) = self.parent()
        {
            // Self-transform changes are automatically handled,
            // we only want to inform ancestors to avoid unnecessary invalidations for tx/ty
            parent.invalidate_cached_bitmap();
        }
    }

    /// The rotation in degrees this display object in local space.
    /// Returned by the `_rotation`/`rotation` ActionScript properties.
    #[no_dynamic]
    fn rotation(self) -> Degrees {
        let degrees = self.base().rotation();
        self.set_scale_rotation_cached();
        degrees
    }

    /// Sets the rotation in degrees this display object in local space.
    /// Set by the `_rotation`/`rotation` ActionScript properties.
    /// This invalidates any ancestors cacheAsBitmap automatically.
    #[no_dynamic]
    fn set_rotation(self, radians: Degrees) {
        if self.base().set_rotation(radians) {
            self.set_scale_rotation_cached();
            if let Some(parent) = self.parent() {
                // Self-transform changes are automatically handled,
                // we only want to inform ancestors to avoid unnecessary invalidations for tx/ty
                parent.invalidate_cached_bitmap();
            }
        }
    }

    /// The X axis scale for this display object in local space.
    /// Returned by the `_xscale`/`scaleX` ActionScript properties.
    #[no_dynamic]
    fn scale_x(self) -> Percent {
        let percent = self.base().scale_x();
        self.set_scale_rotation_cached();
        percent
    }

    /// Sets the X axis scale for this display object in local space.
    /// Set by the `_xscale`/`scaleX` ActionScript properties.
    /// This invalidates any ancestors cacheAsBitmap automatically.
    #[no_dynamic]
    fn set_scale_x(self, value: Percent) {
        if self.base().set_scale_x(value) {
            self.set_scale_rotation_cached();
            if let Some(parent) = self.parent() {
                // Self-transform changes are automatically handled,
                // we only want to inform ancestors to avoid unnecessary invalidations for tx/ty
                parent.invalidate_cached_bitmap();
            }
        }
    }

    /// The Y axis scale for this display object in local space.
    /// Returned by the `_yscale`/`scaleY` ActionScript properties.
    #[no_dynamic]
    fn scale_y(self) -> Percent {
        let percent = self.base().scale_y();
        self.set_scale_rotation_cached();
        percent
    }

    /// Sets the Y axis scale for this display object in local space.
    /// Returned by the `_yscale`/`scaleY` ActionScript properties.
    /// This invalidates any ancestors cacheAsBitmap automatically.
    #[no_dynamic]
    fn set_scale_y(self, value: Percent) {
        if self.base().set_scale_y(value) {
            self.set_scale_rotation_cached();
            if let Some(parent) = self.parent() {
                // Self-transform changes are automatically handled,
                // we only want to inform ancestors to avoid unnecessary invalidations for tx/ty
                parent.invalidate_cached_bitmap();
            }
        }
    }

    /// Gets the pixel width of the AABB containing this display object in local space.
    /// Returned by the ActionScript `_width`/`width` properties.
    fn width(self) -> f64 {
        self.local_bounds(BoundsMode::Script).width().to_pixels()
    }

    /// Sets the pixel width of this display object in local space.
    /// The width is based on the AABB of the object.
    /// Set by the ActionScript `_width`/`width` properties.
    /// This does odd things on rotated clips to match the behavior of Flash.
    fn set_width(self, _context: &mut UpdateContext<'gc>, value: f64) {
        let object_bounds = self.bounds(BoundsMode::Script);
        let object_width = object_bounds.width().to_pixels();
        let object_height = object_bounds.height().to_pixels();
        let aspect_ratio = object_height / object_width;

        let (target_scale_x, target_scale_y) = if object_width != 0.0 {
            (value / object_width, value / object_height)
        } else {
            (0.0, 0.0)
        };

        // No idea about the derivation of this -- figured it out via lots of trial and error.
        // It has to do with the length of the sides A, B of an AABB enclosing the object's OBB with sides a, b:
        // A = sin(t) * a + cos(t) * b
        // B = cos(t) * a + sin(t) * b
        let prev_scale_x = self.scale_x().unit();
        let prev_scale_y = self.scale_y().unit();
        let rotation = self.rotation();
        let cos = f64::abs(f64::cos(rotation.into_radians()));
        let sin = f64::abs(f64::sin(rotation.into_radians()));
        let mut new_scale_x = aspect_ratio * (cos * target_scale_x + sin * target_scale_y)
            / ((cos + aspect_ratio * sin) * (aspect_ratio * cos + sin));
        let mut new_scale_y =
            (sin * prev_scale_x + aspect_ratio * cos * prev_scale_y) / (aspect_ratio * cos + sin);

        if !new_scale_x.is_finite() {
            new_scale_x = 0.0;
        }

        if !new_scale_y.is_finite() {
            new_scale_y = 0.0;
        }

        self.set_scale_x(Percent::from_unit(new_scale_x));
        self.set_scale_y(Percent::from_unit(new_scale_y));
    }

    /// Gets the pixel height of the AABB containing this display object in local space.
    /// Returned by the ActionScript `_height`/`height` properties.
    fn height(self) -> f64 {
        self.local_bounds(BoundsMode::Script).height().to_pixels()
    }

    /// Sets the pixel height of this display object in local space.
    /// Set by the ActionScript `_height`/`height` properties.
    /// This does odd things on rotated clips to match the behavior of Flash.
    fn set_height(self, _context: &mut UpdateContext<'gc>, value: f64) {
        let object_bounds = self.bounds(BoundsMode::Script);
        let object_width = object_bounds.width().to_pixels();
        let object_height = object_bounds.height().to_pixels();
        let aspect_ratio = object_width / object_height;

        let (target_scale_x, target_scale_y) = if object_height != 0.0 {
            (value / object_width, value / object_height)
        } else {
            (0.0, 0.0)
        };

        // No idea about the derivation of this -- figured it out via lots of trial and error.
        // It has to do with the length of the sides A, B of an AABB enclosing the object's OBB with sides a, b:
        // A = sin(t) * a + cos(t) * b
        // B = cos(t) * a + sin(t) * b
        let prev_scale_x = self.scale_x().unit();
        let prev_scale_y = self.scale_y().unit();
        let rotation = self.rotation();
        let cos = f64::abs(f64::cos(rotation.into_radians()));
        let sin = f64::abs(f64::sin(rotation.into_radians()));
        let mut new_scale_x =
            (aspect_ratio * cos * prev_scale_x + sin * prev_scale_y) / (aspect_ratio * cos + sin);
        let mut new_scale_y = aspect_ratio * (sin * target_scale_x + cos * target_scale_y)
            / ((cos + aspect_ratio * sin) * (aspect_ratio * cos + sin));

        if !new_scale_x.is_finite() {
            new_scale_x = 0.0;
        }

        if !new_scale_y.is_finite() {
            new_scale_y = 0.0;
        }

        self.set_scale_x(Percent::from_unit(new_scale_x));
        self.set_scale_y(Percent::from_unit(new_scale_y));
    }

    #[no_dynamic]
    fn ratio(self) -> u16 {
        self.base().ratio.get()
    }

    #[no_dynamic]
    fn set_ratio(self, context: &mut UpdateContext<'gc>, ratio: u16) {
        self.base().ratio.set(ratio);
        self.invalidate_cached_bitmap();
        self.on_ratio_changed(context, ratio);
    }

    fn on_ratio_changed(self, _context: &mut UpdateContext<'gc>, _new_ratio: u16) {}

    /// The opacity of this display object.
    /// 1 is fully opaque.
    /// Returned by the `_alpha`/`alpha` ActionScript properties.
    #[no_dynamic]
    fn alpha(self) -> f64 {
        self.base().alpha()
    }

    /// Sets the opacity of this display object.
    /// 1 is fully opaque.
    /// Set by the `_alpha`/`alpha` ActionScript properties.
    /// This invalidates any cacheAsBitmap automatically.
    #[no_dynamic]
    fn set_alpha(self, value: f64) {
        if self.base().set_alpha(value)
            && let Some(parent) = self.parent()
        {
            // Self-transform changes are automatically handled
            parent.invalidate_cached_bitmap();
        }
    }

    #[no_dynamic]
    fn name(self) -> Option<AvmString<'gc>> {
        self.base().name()
    }

    #[no_dynamic]
    fn set_name(self, mc: &Mutation<'gc>, name: AvmString<'gc>) {
        DisplayObjectBase::set_name(Gc::write(mc, self.base()), name);
        #[cfg(feature = "aether_performance")]
        self.refresh_aether_adaptive_avatar_cache_candidates();
    }

    /// Refresh exact AQW AvatarMC root cache eligibility.
    ///
    /// Descendants are traversed so any stale eligibility from reparenting is cleared, but
    /// equipment sublayers never own independent live GPU caches.
    #[cfg(feature = "aether_performance")]
    fn refresh_aether_adaptive_avatar_cache_candidates(self) {
        let is_root = self
            .base()
            .contains_flag(DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_ROOT);
        let is_world_avatar = self.name().is_some_and(|name| {
            let name = name.as_wstr();
            name.len() > 1
                && name.get(0) == Some(u16::from(b'a'))
                && (1..name.len()).all(|index| {
                    name.get(index)
                        .is_some_and(|unit| (u16::from(b'0')..=u16::from(b'9')).contains(&unit))
                })
        });
        self.base()
            .set_aether_adaptive_avatar_cache_candidate(is_root && is_world_avatar);

        if let Some(container) = self.as_container() {
            for child in container.iter_render_list() {
                child.refresh_aether_adaptive_avatar_cache_candidates();
            }
        }
    }

    fn filters(self) -> Ref<'gc, [Filter]> {
        Gc::as_ref(self.base()).filters()
    }

    fn set_filters(self, filters: Box<[Filter]>) {
        if self.base().set_filters(filters) {
            self.invalidate_cached_bitmap();
        }
    }

    /// Returns the dot-syntax path to this display object, e.g. `_level0.foo.clip`
    #[no_dynamic]
    fn path(self) -> WString {
        if let Some(parent) = self.avm1_parent() {
            let mut path = parent.path();
            path.push_byte(b'.');
            if let Some(name) = self.name() {
                path.push_str(&name);
            }
            path
        } else {
            WString::from_utf8_owned(format!("_level{}", self.depth()))
        }
    }

    /// Returns the Flash 4 slash-syntax path to this display object, e.g. `/foo/clip`.
    /// Returned by the `_target` property in AVM1.
    #[no_dynamic]
    fn slash_path(self) -> WString {
        fn build_slash_path(object: DisplayObject<'_>) -> WString {
            if let Some(parent) = object.avm1_parent() {
                let mut path = build_slash_path(parent);
                path.push_byte(b'/');
                if let Some(name) = object.name() {
                    path.push_str(&name);
                }
                path
            } else {
                let level = object.depth();
                if level == 0 {
                    // _level0 does not append its name in slash syntax.
                    WString::new()
                } else {
                    // Other levels do append their name.
                    WString::from_utf8_owned(format!("_level{level}"))
                }
            }
        }

        if self.avm1_parent().is_some() {
            build_slash_path(self)
        } else {
            // _target of _level0 should just be '/'.
            WString::from_unit(b'/'.into())
        }
    }

    #[no_dynamic]
    fn clip_depth(self) -> Depth {
        self.base().clip_depth()
    }

    #[no_dynamic]
    fn set_clip_depth(self, depth: Depth) {
        self.base().set_clip_depth(depth);
    }

    /// Retrieve the parent of this display object.
    ///
    /// This version of the function merely exposes the display object parent,
    /// without any further filtering.
    #[no_dynamic]
    fn parent(self) -> Option<DisplayObject<'gc>> {
        self.base().parent()
    }

    /// Consume this subtree's conservative dirty bit for one AVM2 lifecycle
    /// walk. Clearing before doing work ensures a mutation made re-entrantly
    /// during the walk remains scheduled for the next pass.
    #[no_dynamic]
    fn begin_avm2_lifecycle_traversal(self, traversal: Avm2LifecycleTraversal) -> bool {
        let dirty = self.base().begin_avm2_lifecycle_traversal(traversal);

        // This diagnostics option intentionally retries a full construction
        // walk when a frame script observes an unconstructed descendant. Keep
        // that explicit compatibility behavior intact even for a clean
        // summary; it is off in normal production runs.
        #[cfg(feature = "aether_diagnostics")]
        if traversal == Avm2LifecycleTraversal::Construct
            && crate::aether_diagnostics::frame_construction_retry_enabled()
        {
            return true;
        }

        dirty
    }

    /// Mark this object and every current ancestor as potentially containing
    /// work for an AVM2 lifecycle walk. Mutations are uncommon compared with
    /// clean lifecycle passes, so propagating eagerly keeps the hot read path
    /// to one flag test per clean subtree.
    #[no_dynamic]
    fn mark_avm2_lifecycle_dirty(self, traversal: Avm2LifecycleTraversal) {
        let mut current = Some(self);
        while let Some(object) = current {
            object.base().mark_avm2_lifecycle_dirty(traversal);
            current = object.parent();
        }
    }

    /// Mark only this object's summary. Lifecycle recursion uses this after a
    /// visited child reports that it still has work for the next pass, which
    /// avoids walking the full ancestor chain once per active descendant.
    #[no_dynamic]
    fn mark_avm2_lifecycle_dirty_local(self, traversal: Avm2LifecycleTraversal) {
        self.base().mark_avm2_lifecycle_dirty(traversal);
    }

    /// Schedule the lifecycle work made observable by an explicit AVM2 goto.
    ///
    /// Flash runs nested-goto construction and frame-script phases synchronously. Marking the
    /// changed clip and its ancestors preserves that behavior without walking every unrelated
    /// display object on the stage for every combat animation goto.
    #[no_dynamic]
    fn schedule_avm2_nested_goto_lifecycle(self) {
        self.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::Construct);
        self.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::FrameScripts);
    }

    #[no_dynamic]
    fn is_avm2_lifecycle_dirty(self, traversal: Avm2LifecycleTraversal) -> bool {
        self.base().is_avm2_lifecycle_dirty(traversal)
    }

    #[no_dynamic]
    fn set_skip_next_enter_frame(self, skip: bool) {
        self.base().set_skip_next_enter_frame(skip);
        if skip {
            self.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::Enter);
        }
    }

    /// Set the parent of this display object.
    #[no_dynamic]
    fn set_parent(self, context: &mut UpdateContext<'gc>, parent: Option<DisplayObject<'gc>>) {
        let had_parent = self.parent().is_some();
        let write = Gc::write(context.gc(), self.base());
        DisplayObjectBase::set_parent_ignoring_orphan_list(write, parent);
        let parent_removed = had_parent && parent.is_none();

        #[cfg(feature = "aether_performance")]
        self.refresh_aether_adaptive_avatar_cache_candidates();

        if let Some(parent) = parent {
            parent.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::Enter);
            parent.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::Construct);
            parent.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::FrameScripts);
        }

        if parent_removed {
            if let Some(int) = self.as_interactive() {
                int.drop_focus(context);
            }

            self.on_parent_removed(context);
        }
    }

    /// This method is called when the parent is removed.
    /// It may be overwritten to inject some implementation-specific behavior.
    fn on_parent_removed(self, _context: &mut UpdateContext<'gc>) {}

    /// Retrieve the parent of this display object.
    ///
    /// This version of the function implements the concept of parenthood as
    /// seen in AVM1. Notably, it disallows access to the `Stage` and to
    /// non-AVM1 DisplayObjects; for an unfiltered concept of parent,
    /// use the `parent` method.
    #[no_dynamic]
    fn avm1_parent(self) -> Option<DisplayObject<'gc>> {
        self.parent()
            .filter(|p| p.as_stage().is_none())
            .filter(|p| !p.movie().is_action_script_3())
    }

    /// Retrieve the parent of this display object.
    ///
    /// This version of the function implements the concept of parenthood as
    /// seen in AVM2. Notably, it disallows access to non-container parents.
    #[no_dynamic]
    fn avm2_parent(self) -> Option<DisplayObject<'gc>> {
        self.parent().filter(|p| p.as_container().is_some())
    }

    #[no_dynamic]
    fn masker(self) -> Option<DisplayObject<'gc>> {
        self.base().masker()
    }

    #[no_dynamic]
    fn set_masker(
        self,
        mc: &Mutation<'gc>,
        node: Option<DisplayObject<'gc>>,
        remove_old_link: bool,
    ) {
        if remove_old_link {
            let old_masker = self.base().masker();
            if let Some(old_masker) = old_masker {
                old_masker.set_maskee(mc, None, false);
            }
            if let Some(parent) = self.parent() {
                // Masks are natively handled by cacheAsBitmap - don't invalidate self, only parents
                parent.invalidate_cached_bitmap();
            }
        }
        DisplayObjectBase::set_masker(Gc::write(mc, self.base()), node);
    }

    #[no_dynamic]
    fn maskee(self) -> Option<DisplayObject<'gc>> {
        self.base().maskee()
    }

    #[no_dynamic]
    fn set_maskee(
        self,
        mc: &Mutation<'gc>,
        node: Option<DisplayObject<'gc>>,
        remove_old_link: bool,
    ) {
        if remove_old_link {
            let old_maskee = self.base().maskee();
            if let Some(old_maskee) = old_maskee {
                old_maskee.set_masker(mc, None, false);
            }
            self.invalidate_cached_bitmap();
        }
        DisplayObjectBase::set_maskee(Gc::write(mc, self.base()), node);
    }

    #[no_dynamic]
    fn get_render_mask(self) -> RenderMask<'gc> {
        match self.masker() {
            None => RenderMask::None,
            Some(mask) if self.is_bitmap_cached() && mask.is_bitmap_cached() => {
                RenderMask::Alpha(mask)
            }
            Some(mask) => RenderMask::Stencil(mask),
        }
    }

    /// High level method for setting the mask. Sets both masker and maskee.
    ///
    /// Equivalent to setting the mask from AVM.
    #[no_dynamic]
    fn set_mask(self, mask: Option<DisplayObject<'gc>>, mc: &Mutation<'gc>) {
        self.set_clip_depth(0);
        self.set_masker(mc, mask, true);
        if let Some(mask) = mask {
            mask.set_clip_depth(0);
            mask.set_maskee(mc, Some(self), true);
        }
    }

    #[no_dynamic]
    fn scroll_rect(self) -> Option<Rectangle<Twips>> {
        self.base().scroll_rect.get()
    }

    #[no_dynamic]
    fn next_scroll_rect(self) -> Rectangle<Twips> {
        self.base().next_scroll_rect.get()
    }

    #[no_dynamic]
    fn set_next_scroll_rect(self, rectangle: Rectangle<Twips>) {
        self.base().next_scroll_rect.set(rectangle);

        // Scroll rect is natively handled by cacheAsBitmap - don't invalidate self, only parents
        if let Some(parent) = self.parent() {
            parent.invalidate_cached_bitmap();
        }
    }

    #[no_dynamic]
    fn scaling_grid(self) -> Rectangle<Twips> {
        self.base().scaling_grid.get()
    }

    #[no_dynamic]
    fn set_scaling_grid(self, rect: Rectangle<Twips>) {
        self.base().scaling_grid.set(rect);
    }

    #[no_dynamic]
    /// Whether this object has been removed. Only applies to AVM1.
    fn avm1_removed(self) -> bool {
        self.base().avm1_removed()
    }

    #[no_dynamic]
    // Sets whether this object has been removed. Only applies to AVM1
    fn set_avm1_removed(self, value: bool) {
        self.base().set_avm1_removed(value)
    }

    #[no_dynamic]
    /// Is this object waiting to be removed on the start of the next frame
    fn avm1_pending_removal(self) -> bool {
        self.base().avm1_pending_removal()
    }

    #[no_dynamic]
    fn set_avm1_pending_removal(self, value: bool) {
        self.base().set_avm1_pending_removal(value)
    }

    /// Whether this display object is visible.
    /// Invisible objects are not rendered, but otherwise continue to exist normally.
    /// Returned by the `_visible`/`visible` ActionScript properties.
    #[no_dynamic]
    fn visible(self) -> bool {
        self.base().visible()
    }

    /// Sets whether this display object will be visible.
    /// Invisible objects are not rendered, but otherwise continue to exist normally.
    /// Returned by the `_visible`/`visible` ActionScript properties.
    #[no_dynamic]
    fn set_visible(self, context: &mut UpdateContext<'gc>, value: bool) {
        if self.base().set_visible(value)
            && let Some(parent) = self.parent()
        {
            // We don't need to invalidate ourselves, we're just toggling if the bitmap is rendered.
            parent.invalidate_cached_bitmap();
        }

        if !value && let Some(int) = self.as_interactive() {
            // The focus is dropped when it's made invisible.
            int.drop_focus(context);
        }
    }

    #[no_dynamic]
    fn meta_data(self) -> Option<Avm2Object<'gc>> {
        self.base().meta_data()
    }

    #[no_dynamic]
    fn set_meta_data(self, mc: &Mutation<'gc>, value: Avm2Object<'gc>) {
        DisplayObjectBase::set_meta_data(Gc::write(mc, self.base()), value);
    }

    /// The blend mode used when rendering this display object.
    /// Values other than the default `BlendMode::Normal` implicitly cause cache-as-bitmap behavior.
    #[no_dynamic]
    fn blend_mode(self) -> ExtendedBlendMode {
        self.base().blend_mode()
    }

    /// Sets the blend mode used when rendering this display object.
    /// Values other than the default `BlendMode::Normal` implicitly cause cache-as-bitmap behavior.
    #[no_dynamic]
    fn set_blend_mode(self, value: ExtendedBlendMode) {
        if self.base().set_blend_mode(value)
            && let Some(parent) = self.parent()
        {
            // We don't need to invalidate ourselves, we're just toggling how the bitmap is rendered.

            // Note that Flash does not always invalidate on changing the blend mode;
            // but that's a bug we don't need to copy :)
            parent.invalidate_cached_bitmap();
        }
    }

    #[no_dynamic]
    fn blend_shader(self) -> Option<PixelBenderShaderHandle> {
        self.base().blend_shader()
    }

    #[no_dynamic]
    fn set_blend_shader(self, value: Option<PixelBenderShaderHandle>) {
        self.base().set_blend_shader(value);
        self.set_blend_mode(ExtendedBlendMode::Shader);
    }

    #[no_dynamic]
    /// The opaque background color of this display object.
    fn opaque_background(self) -> Option<Color> {
        self.base().opaque_background()
    }

    /// Sets the opaque background color of this display object.
    /// The bounding box of the display object will be filled with the given color. This also
    /// triggers cache-as-bitmap behavior. Only solid backgrounds are supported; the alpha channel
    /// is ignored.
    #[no_dynamic]
    fn set_opaque_background(self, value: Option<Color>) {
        if self.base().set_opaque_background(value) {
            self.invalidate_cached_bitmap();
        }
    }

    /// Whether this display object represents the root of loaded content.
    #[no_dynamic]
    fn is_root(self) -> bool {
        self.base().is_root()
    }

    /// Sets whether this display object represents the root of loaded content.
    #[no_dynamic]
    fn set_is_root(self, value: bool) {
        self.base().set_is_root(value);
    }

    /// The sound transform for sounds played inside this display object.
    #[no_dynamic]
    fn set_sound_transform(
        self,
        context: &mut UpdateContext<'gc>,
        sound_transform: SoundTransform,
    ) {
        self.base().set_sound_transform(sound_transform);
        context.set_sound_transforms_dirty();
    }

    /// Whether this display object is used as the _root of itself and its children.
    /// Returned by the `_lockroot` ActionScript property.
    #[no_dynamic]
    fn lock_root(self) -> bool {
        self.base().lock_root()
    }

    /// Sets whether this display object is used as the _root of itself and its children.
    /// Returned by the `_lockroot` ActionScript property.
    #[no_dynamic]
    fn set_lock_root(self, value: bool) {
        self.base().set_lock_root(value);
    }

    /// Whether this display object has been transformed by ActionScript.
    /// When this flag is set, changes from SWF `PlaceObject` tags are ignored.
    #[no_dynamic]
    fn transformed_by_script(self) -> bool {
        self.base().transformed_by_script()
    }

    /// Sets whether this display object has been transformed by ActionScript.
    /// When this flag is set, changes from SWF `PlaceObject` tags are ignored.
    #[no_dynamic]
    fn set_transformed_by_script(self, value: bool) {
        self.base().set_transformed_by_script(value)
    }

    /// Whether this display object prefers to be cached into a bitmap rendering.
    /// This is the PlaceObject `cacheAsBitmap` flag - and may be overridden if filters are applied.
    /// Consider `is_bitmap_cached` for if a bitmap cache is actually in use.
    #[no_dynamic]
    fn is_bitmap_cached_preference(self) -> bool {
        self.base().is_bitmap_cached_preference()
    }

    /// Whether this display object is using a bitmap cache, whether by preference or necessity.
    #[no_dynamic]
    fn is_bitmap_cached(self) -> bool {
        self.base().cell.borrow().cache.is_some()
    }

    /// Explicitly sets the preference of this display object to be cached into a bitmap rendering.
    /// Note that the object will still be bitmap cached if a filter is active.
    #[no_dynamic]
    fn set_bitmap_cached_preference(self, value: bool) {
        self.base().set_bitmap_cached_preference(value)
    }

    /// Whether this display object has a scroll rectangle applied.
    #[no_dynamic]
    fn has_scroll_rect(self) -> bool {
        self.base().has_scroll_rect()
    }

    /// Sets whether this display object has a scroll rectangle applied.
    #[no_dynamic]
    fn set_has_scroll_rect(self, value: bool) {
        self.base().set_has_scroll_rect(value)
    }

    /// Whether this display object has been created by ActionScript 1/2.
    #[no_dynamic]
    fn placed_by_avm1_script(self) -> bool {
        self.base().placed_by_avm1_script()
    }

    /// Sets whether this display object has been created by ActionScript 1/2.
    #[no_dynamic]
    fn set_placed_by_avm1_script(self, value: bool) {
        self.base().set_placed_by_avm1_script(value);
    }

    /// Whether this display object has been created by ActionScript 3.
    /// When this flag is set, changes from SWF `RemoveObject` tags are
    /// ignored.
    #[no_dynamic]
    fn placed_by_avm2_script(self) -> bool {
        self.base().placed_by_avm2_script()
    }

    /// When this flag is set, changes from SWF `RemoveObject` tags are
    /// ignored.
    #[no_dynamic]
    fn set_placed_by_avm2_script(self, value: bool) {
        self.base().set_placed_by_avm2_script(value)
    }

    #[no_dynamic]
    fn manual_frame_construct(&self) -> bool {
        self.base().manual_frame_construct()
    }

    /// When this flag is set, the object will not be instantiated in-line with
    /// normal frame construction by `MovieClip::construct_frame`.
    #[no_dynamic]
    fn set_manual_frame_construct(&self, value: bool) {
        self.base().set_manual_frame_construct(value);
    }

    /// Whether this display object has been instantiated by the timeline.
    /// When this flag is set, attempts to change the object's name from AVM2
    /// throw an exception.
    #[no_dynamic]
    fn instantiated_by_timeline(self) -> bool {
        self.base().instantiated_by_timeline()
    }

    /// Sets whether this display object has been instantiated by the timeline.
    /// When this flag is set, attempts to change the object's name from AVM2
    /// throw an exception.
    #[no_dynamic]
    fn set_instantiated_by_timeline(self, value: bool) {
        self.base().set_instantiated_by_timeline(value);
    }

    /// Whether this display object was placed by a SWF tag with an explicit
    /// name.
    ///
    /// When this flag is set, the object will attempt to set a dynamic property
    /// on the parent with the same name as itself.
    #[no_dynamic]
    fn has_explicit_name(self) -> bool {
        self.base().has_explicit_name()
    }

    /// Sets whether this display object was placed by a SWF tag with an
    /// explicit name.
    ///
    /// When this flag is set, the object will attempt to set a dynamic property
    /// on the parent with the same name as itself.
    #[no_dynamic]
    fn set_has_explicit_name(self, value: bool) {
        self.base().set_has_explicit_name(value);
    }
    fn state(&self) -> Option<ButtonState> {
        None
    }

    fn set_state(self, _context: &mut UpdateContext<'gc>, _state: ButtonState) {}

    /// Run any start-of-frame actions for this display object.
    ///
    /// When fired on `Stage`, this also emits the AVM2 `enterFrame` broadcast.
    fn enter_frame(self, _context: &mut UpdateContext<'gc>) {
        self.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Enter);
    }

    /// Construct all display objects that the timeline indicates should exist
    /// this frame, and their children.
    ///
    /// This function should ensure the following, from the point of view of
    /// downstream VMs:
    ///
    /// 1. That the object itself has been allocated, if not constructed
    /// 2. That newly created children have been instantiated and are present
    ///    as properties on the class
    fn construct_frame(self, _context: &mut UpdateContext<'gc>) {
        self.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct);
    }

    /// To be called when an AVM2 display object has finished being constructed.
    ///
    /// This function must be called once and ONLY once, after the object's
    /// AVM2 side has been constructed. Typically, this is in construct_frame,
    /// unless your object needs to construct itself earlier or later. When
    /// this function is called on the child, it will fire its add events and,
    /// if possible, set a named property on the parent matching the name of
    /// the object.
    ///
    /// This still needs to be called for objects placed by AVM2, since we
    /// need to stop the underlying MovieClip if the constructed class
    /// does not extend MovieClip.
    ///
    /// Since we construct AVM2 display objects after they are allocated and
    /// placed on the render list, these steps have to be done by the child
    /// object to signal to its parent that it was added.
    #[no_dynamic]
    #[inline(never)]
    fn on_construction_complete(self, context: &mut UpdateContext<'gc>) {
        let placed_by_avm2_script = self.placed_by_avm2_script();
        self.fire_added_events(context);
        // Check `self.placed_by_avm2_script()` before we fire events, since those
        // events might `placed_by_avm2_script`
        if !placed_by_avm2_script {
            self.set_on_parent_field(context);
        }

        if let Some(movie) = self.as_movie_clip() {
            let obj = movie
                .object2()
                .expect("MovieClip object should have been constructed");
            let movieclip_class = context.avm2.classes().movieclip.inner_class_definition();
            // It's possible to have a DefineSprite tag with multiple frames, but have
            // the corresponding `SymbolClass` *not* extend `MovieClip` (e.g. extending `Sprite` directly.)
            // When this occurs, Flash Player will run the first frame, and immediately stop.
            // However, Flash Player runs frames for the root movie clip, even if it doesn't extend `MovieClip`.
            if !obj.is_of_type(movieclip_class) && !movie.is_root() {
                movie.stop(context);
            }
            movie.set_initialized();
        }
    }

    #[no_dynamic]
    fn fire_added_events(self, context: &mut UpdateContext<'gc>) {
        if !self.placed_by_avm2_script() {
            // Since we construct AVM2 display objects after they are
            // allocated and placed on the render list, we have to emit all
            // events after this point.
            //
            // Children added to buttons by the timeline do not emit events.
            if self.parent().and_then(|p| p.as_avm2_button()).is_none() {
                dispatch_added_event_only(self, context);
                if self.avm2_stage(context).is_some() {
                    dispatch_added_to_stage_event_only(self, context);
                }
            }
        }
    }

    #[no_dynamic]
    fn set_on_parent_field(self, context: &mut UpdateContext<'gc>) {
        if self.has_explicit_name()
            && let Some(parent) = self.parent().and_then(|p| p.object2())
        {
            let parent = Avm2Value::from(parent);

            if let Some(child) = self.object2()
                && let Some(name) = self.name()
            {
                let domain = context
                    .library
                    .library_for_movie(self.movie())
                    .unwrap()
                    .avm2_domain();

                let mut activation = Avm2Activation::from_domain(context, domain);
                let multiname = Avm2Multiname::new(activation.avm2().find_public_namespace(), name);
                let set_result = parent.init_property(&multiname, child.into(), &mut activation);

                if let Err(err) = set_result {
                    Avm2::uncaught_error(
                        &mut activation,
                        Some(self),
                        err,
                        &format!("Error setting AVM2 child named \"{}\"", name),
                    );
                }
            }
        }
    }

    /// Run any frame scripts (if they exist and this object needs to run them).
    fn run_frame_scripts(self, context: &mut UpdateContext<'gc>) {
        if !self.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::FrameScripts) {
            return;
        }

        if let Some(container) = self.as_container() {
            for child in container.iter_render_list() {
                child.run_frame_scripts(context);
                if child.is_avm2_lifecycle_dirty(Avm2LifecycleTraversal::FrameScripts) {
                    self.mark_avm2_lifecycle_dirty_local(Avm2LifecycleTraversal::FrameScripts);
                }
            }
        }
    }

    /// Called before the child is about to be rendered.
    /// Note that this happens even if the child is invisible
    /// (as long as the child is still on a render list)
    #[no_dynamic]
    fn pre_render(self, context: &mut RenderContext<'_, 'gc>) {
        let this = self.base();
        #[cfg(feature = "aether_performance")]
        {
            let concatenated_matrix =
                context.transform_stack.transform().matrix * this.matrix.get();
            this.update_aether_adaptive_avatar_cache_with_transform(
                crate::aether_performance::adaptive_avatar_cache_enabled(),
                [
                    concatenated_matrix.a,
                    concatenated_matrix.b,
                    concatenated_matrix.c,
                    concatenated_matrix.d,
                ],
            );
        }
        this.clear_invalidate_flag();
        this.scroll_rect
            .set(this.has_scroll_rect().then(|| this.next_scroll_rect.get()));
    }

    fn render_self(self, _context: &mut RenderContext<'_, 'gc>) {}

    #[no_dynamic]
    fn render(self, context: &mut RenderContext<'_, 'gc>) {
        self.render_with_options(context, Default::default())
    }

    fn render_with_options(self, context: &mut RenderContext<'_, 'gc>, options: RenderOptions) {
        render_base(self.into(), context, options)
    }

    #[cfg(not(feature = "avm_debug"))]
    #[no_dynamic]
    fn display_render_tree(self, _depth: usize) {}

    #[cfg(feature = "avm_debug")]
    #[no_dynamic]
    fn display_render_tree(self, depth: usize) {
        let mut self_str = &*format!("{self:?}");
        if let Some(end_char) = self_str.find(|c: char| !c.is_ascii_alphanumeric()) {
            self_str = &self_str[..end_char];
        }

        let bounds = self.world_bounds(BoundsMode::Engine);

        let mut classname = "".to_string();
        if let Some(o) = self.object2() {
            classname = format!("{:?}", o.base().class_name());
        }

        println!(
            "{} rel({},{}) abs({},{}) {} {} {} id={} depth={}",
            " ".repeat(depth),
            self.x(),
            self.y(),
            bounds.x_min.to_pixels(),
            bounds.y_min.to_pixels(),
            classname,
            self.name().map(|s| s.to_string()).unwrap_or_default(),
            self_str,
            self.id(),
            depth
        );

        if let Some(ctr) = self.as_container() {
            ctr.recurse_render_tree(depth + 1);
        }
    }

    fn avm1_unload(self, context: &mut UpdateContext<'gc>) {
        // Unload children.
        if let Some(ctr) = self.as_container() {
            for child in ctr.iter_render_list() {
                child.avm1_unload(context);
            }
        }

        if let Some(node) = self.maskee() {
            node.set_masker(context.gc(), None, true);
        } else if let Some(node) = self.masker() {
            node.set_maskee(context.gc(), None, true);
        }

        // Unregister any text field variable bindings, and replace them on the unbound list.
        Avm1TextFieldBinding::unregister_bindings(self.into(), context);

        self.set_avm1_removed(true);
    }

    fn avm1_text_field_bindings(&self) -> Option<Ref<'_, [Avm1TextFieldBinding<'gc>]>> {
        None
    }

    fn avm1_text_field_bindings_mut(
        &self,
        _mc: &Mutation<'gc>,
    ) -> Option<RefMut<'_, Vec<Avm1TextFieldBinding<'gc>>>> {
        None
    }

    #[no_dynamic]
    fn apply_place_object(self, context: &mut UpdateContext<'gc>, place_object: &swf::PlaceObject) {
        // PlaceObject tags only apply if this object has not been dynamically moved by AS code.
        if !self.transformed_by_script() {
            if let Some(matrix) = place_object.matrix {
                self.set_matrix(matrix.into());
                if let Some(parent) = self.parent() {
                    // Self-transform changes are automatically handled,
                    // we only want to inform ancestors to avoid unnecessary invalidations for tx/ty
                    parent.invalidate_cached_bitmap();
                }
            }
            if let Some(color_transform) = &place_object.color_transform {
                self.set_color_transform(*color_transform);
                if let Some(parent) = self.parent() {
                    parent.invalidate_cached_bitmap();
                }
            }
            if let Some(ratio) = place_object.ratio {
                self.set_ratio(context, ratio);
            }
            if let Some(is_bitmap_cached) = place_object.is_bitmap_cached {
                self.set_bitmap_cached_preference(is_bitmap_cached);
            }
            if let Some(blend_mode) = place_object.blend_mode {
                self.set_blend_mode(blend_mode.into());
            }
            if self.swf_version() >= 11 {
                if let Some(visible) = place_object.is_visible {
                    self.set_visible(context, visible);
                }
                if let Some(mut color) = place_object.background_color {
                    let color = if color.a > 0 {
                        // Force opaque background to have no transpranecy.
                        color.a = 255;
                        Some(color)
                    } else {
                        None
                    };
                    self.set_opaque_background(color);
                }
            }
            if let Some(filters) = &place_object.filters {
                self.set_filters(filters.iter().map(Filter::from).collect());
            }
            // Purposely omitted properties:
            // name, clip_depth, clip_actions
            // These properties are only set on initial placement in `MovieClip::instantiate_child`
            // and can not be modified by subsequent PlaceObject tags.
        }
    }

    /// Called when this object should be replaced by a PlaceObject tag.
    fn replace_with(self, _context: &mut UpdateContext<'gc>, _id: CharacterId) {
        // Noop for most symbols; only shapes can replace their innards with another Graphic.
    }

    fn object1(self) -> Option<Avm1Object<'gc>>;

    #[no_dynamic]
    fn object1_or_undef(self) -> Avm1Value<'gc> {
        self.object1()
            .map(|o| o.into())
            .unwrap_or(Avm1Value::Undefined)
    }

    #[no_dynamic]
    fn object1_or_null(self) -> Avm1Value<'gc> {
        self.object1().map(|o| o.into()).unwrap_or(Avm1Value::Null)
    }

    /// Equivalent to `self.object1_or_undef().coerce_to_object_or_bare()`, but avoids
    /// the need for an activation.
    ///
    /// [MOULINS]: Like `coerce_to_object_bare`, I suspect that usages of this method
    /// are incorrect,
    #[no_dynamic]
    fn object1_or_bare(self, mc: &Mutation<'gc>) -> Avm1Object<'gc> {
        self.object1()
            .unwrap_or_else(|| Avm1Object::new_without_proto(mc))
    }

    fn object2(self) -> Option<Avm2StageObject<'gc>>;

    fn set_object2(self, _context: &mut UpdateContext<'gc>, _to: Avm2StageObject<'gc>) {}

    #[no_dynamic]
    fn object2_or_null(self) -> Avm2Value<'gc> {
        self.object2().map(|o| o.into()).unwrap_or(Avm2Value::Null)
    }

    /// Tests if a given stage position point intersects with the world bounds of this object.
    #[no_dynamic]
    fn hit_test_bounds(self, point: Point<Twips>) -> bool {
        self.world_bounds(BoundsMode::Engine).contains(point)
    }

    /// Tests if a given object's world bounds intersects with the world bounds
    /// of this object.
    #[no_dynamic]
    fn hit_test_object(self, other: DisplayObject<'gc>) -> bool {
        // This is only used in ActionScript so it gets a BoundsMode::Script.
        self.world_bounds(BoundsMode::Script)
            .intersects(&other.world_bounds(BoundsMode::Script))
    }

    /// Tests if a given stage position point intersects within this object, considering the art.
    fn hit_test_shape(
        self,
        _context: &mut UpdateContext<'gc>,
        point: Point<Twips>,
        options: HitTestOptions,
    ) -> bool {
        // Default to using bounding box.
        (!options.contains(HitTestOptions::SKIP_INVISIBLE) || self.visible())
            && self.hit_test_bounds(point)
    }

    fn post_instantiation(
        self,
        _context: &mut UpdateContext<'gc>,
        _init_object: Option<Avm1Object<'gc>>,
        _instantiated_by: Instantiator,
        _run_frame: bool,
    ) {
        // Noop.
    }

    /// Return the version of the SWF that created this movie clip.
    fn swf_version(self) -> u8 {
        self.movie().version()
    }

    /// Return the SWF that defines this display object.
    fn movie(self) -> Arc<SwfMovie>;

    fn loader_info(self) -> Option<LoaderInfoObject<'gc>> {
        None
    }

    fn instantiate(self, gc_context: &Mutation<'gc>) -> DisplayObject<'gc>;

    /// Whether this object can be used as a mask.
    /// If this returns false and this object is used as a mask, the mask will not be applied.
    /// This is used by movie clips to disable the mask when there are no children, for example.
    fn allow_as_mask(self) -> bool {
        true
    }

    /// Obtain the top-most non-Stage parent of the display tree hierarchy.
    ///
    /// This function implements the AVM1 concept of root clips. For the AVM2
    /// version, see `avm2_root`.
    #[no_dynamic]
    fn avm1_root(self) -> DisplayObject<'gc> {
        let mut root = self;
        loop {
            if root.lock_root() {
                break;
            }
            if let Some(parent) = root.avm1_parent() {
                if !parent.movie().is_action_script_3() {
                    root = parent;
                } else {
                    // We've traversed upwards into a loader AVM2 movie, so break.
                    break;
                }
            } else {
                break;
            }
        }
        root
    }

    /// `avm1_root`, but disregards _lockroot
    #[no_dynamic]
    fn avm1_root_no_lock(self) -> DisplayObject<'gc> {
        let mut root = self;
        while let Some(parent) = root.avm1_parent() {
            if !parent.movie().is_action_script_3() {
                root = parent;
            } else {
                // We've traversed upwards into a loader AVM2 movie, so break.
                break;
            }
        }
        root
    }

    /// Obtain the top-most Stage or LoaderDisplay object of the display tree hierarchy, for use in mixed AVM.
    #[no_dynamic]
    fn avm1_stage(self) -> DisplayObject<'gc> {
        let mut root = self;
        loop {
            if let Some(parent) = root.parent() {
                if matches!(
                    parent,
                    DisplayObject::LoaderDisplay(_) | DisplayObject::Stage(_)
                ) {
                    return parent;
                }
                root = parent;
            } else {
                return root;
            }
        }
    }

    /// Obtain the top-most non-Stage parent of the display tree hierarchy, if
    /// a suitable object exists.
    ///
    /// This function implements the AVM2 concept of root clips. For the AVM1
    /// version, see `avm1_root`.
    #[no_dynamic]
    fn avm2_root(self) -> Option<DisplayObject<'gc>> {
        let mut parent = Some(self);
        while let Some(p) = parent {
            if p.is_root() {
                return parent;
            }
            if let Some(p_parent) = p.parent()
                && !p_parent.movie().is_action_script_3()
            {
                // We've traversed upwards into a loader AVM1 movie, so return the current parent.
                return parent;
            }
            parent = p.parent();
        }
        None
    }

    /// Obtain the root of the display tree hierarchy, if a suitable object
    /// exists.
    ///
    /// This implements the AVM2 concept of `stage`. Notably, it deliberately
    /// will fail to locate the current player's stage for objects that are not
    /// rooted to the DisplayObject hierarchy correctly. If you just want to
    /// access the current player's stage, grab it from the context.
    #[no_dynamic]
    fn avm2_stage(self, _context: &UpdateContext<'gc>) -> Option<DisplayObject<'gc>> {
        let mut parent = Some(self);
        while let Some(p) = parent {
            if p.as_stage().is_some() {
                return parent;
            }
            parent = p.parent();
        }
        None
    }

    /// Determine if this display object is currently on the stage.
    #[no_dynamic]
    fn is_on_stage(self, context: &UpdateContext<'gc>) -> bool {
        let mut ancestor = self.avm2_parent();
        while let Some(parent) = ancestor {
            if parent.avm2_parent().is_some() {
                ancestor = parent.avm2_parent();
            } else {
                break;
            }
        }

        let ancestor = ancestor.unwrap_or(self);
        DisplayObject::ptr_eq(ancestor, context.stage.into())
    }

    /// Assigns a default instance name `instanceN` to this object.
    #[no_dynamic]
    fn set_default_instance_name(self, context: &mut UpdateContext<'gc>) {
        if self.base().name().is_none() {
            let name = format!("instance{}", *context.instance_counter);
            self.set_name(context.gc(), AvmString::new_utf8(context.gc(), name));
            *context.instance_counter = context.instance_counter.wrapping_add(1);
        }
    }

    /// Assigns a default root name to this object.
    ///
    /// The default root names change based on the AVM configuration of the
    /// clip; AVM2 clips get `rootN` while AVM1 clips get blank strings.
    #[no_dynamic]
    fn set_default_root_name(self, context: &mut UpdateContext<'gc>) {
        if self.movie().is_action_script_3() {
            let name = AvmString::new_utf8(context.gc(), format!("root{}", self.depth() + 1));
            self.set_name(context.gc(), name);
        } else {
            self.set_name(context.gc(), istr!(context, ""));
        }
    }

    /// Inform this object and its ancestors that it has visually changed and must be redrawn.
    /// If this object or any ancestor is marked as cacheAsBitmap, it will invalidate that cache.
    #[no_dynamic]
    fn invalidate_cached_bitmap(self) {
        if self.base().invalidate_cached_bitmap() {
            // Don't inform ancestors if we've already done so this frame
            if let Some(parent) = self.parent() {
                parent.invalidate_cached_bitmap();
            }
        }
    }

    /// Invalidate every cached descendant after the physical viewport changes.
    ///
    /// Unlike normal visual invalidation, this deliberately walks down the tree because cached
    /// descendants may remain locally unchanged while their device-pixel target changes.
    #[no_dynamic]
    fn invalidate_cached_bitmaps_for_viewport_change(self) {
        self.base().invalidate_bitmap_cache_for_viewport_change();

        if let Some(container) = self.as_container() {
            for child in container.iter_render_list() {
                child.invalidate_cached_bitmaps_for_viewport_change();
            }
        }

        if let Some(button) = self.as_avm2_button() {
            for state in [
                swf::ButtonState::UP,
                swf::ButtonState::OVER,
                swf::ButtonState::DOWN,
                swf::ButtonState::HIT_TEST,
            ] {
                if let Some(child) = button.get_state_child(state) {
                    child.invalidate_cached_bitmaps_for_viewport_change();
                }
            }
        }
    }

    /// Retrieve a named property from the AVM1 object.
    ///
    /// This is required as some boolean properties in AVM1 can in fact hold any value.
    #[no_dynamic]
    fn get_avm1_boolean_property<F>(
        self,
        name: AvmString<'gc>,
        context: &mut UpdateContext<'gc>,
        default: F,
    ) -> bool
    where
        F: FnOnce(&mut UpdateContext<'gc>) -> bool,
    {
        if let Some(object) = self.object1() {
            let mut activation = Activation::from_nothing(
                context,
                Avm1ActivationIdentifier::root("[AVM1 Boolean Property]"),
                self.avm1_root(),
            );
            if let Ok(value) = object.get(name, &mut activation) {
                match value {
                    Avm1Value::Undefined => default(activation.context),
                    _ => value.as_bool(activation.swf_version()),
                }
            } else {
                default(activation.context)
            }
        } else {
            false
        }
    }

    #[no_dynamic]
    fn set_avm1_property(
        self,
        name: AvmString<'gc>,
        value: Avm1Value<'gc>,
        context: &mut UpdateContext<'gc>,
    ) {
        if let Some(object) = self.object1() {
            let mut activation = Activation::from_nothing(
                context,
                Avm1ActivationIdentifier::root("[AVM1 Property Set]"),
                self.avm1_root(),
            );
            let _ = object.set(name, value, &mut activation);
        }
    }

    fn as_drawing(&self) -> Option<RefMut<'_, Drawing>> {
        None
    }

    #[no_dynamic]
    fn as_container(self) -> Option<DisplayObjectContainer<'gc>> {
        match self {
            Self::Avm1Button(dobj) => Some(DisplayObjectContainer::Avm1Button(dobj)),
            Self::LoaderDisplay(dobj) => Some(DisplayObjectContainer::LoaderDisplay(dobj)),
            Self::MovieClip(dobj) => Some(DisplayObjectContainer::MovieClip(dobj)),
            Self::Stage(dobj) => Some(DisplayObjectContainer::Stage(dobj)),
            _ => None,
        }
    }
}

pub enum DisplayObjectPtr {}

macro_rules! impl_downcast_methods {
    ($(
        $vis:vis fn $fn_name:ident for $variant:ident;
    )*) => { $(
        #[doc = concat!("Downcast this display object as a `", stringify!($variant), "`.")]
        #[inline(always)]
        $vis fn $fn_name(self) -> Option<$variant<'gc>> {
            if let Self::$variant(obj) = self {
                Some(obj)
            } else {
                None
            }
        }
    )* }
}

impl<'gc> DisplayObject<'gc> {
    pub fn ptr_eq(a: DisplayObject<'gc>, b: DisplayObject<'gc>) -> bool {
        std::ptr::eq(a.as_ptr(), b.as_ptr())
    }

    pub fn option_ptr_eq(a: Option<DisplayObject<'gc>>, b: Option<DisplayObject<'gc>>) -> bool {
        a.map(|o| o.as_ptr()) == b.map(|o| o.as_ptr())
    }

    impl_downcast_methods! {
        pub fn as_stage for Stage;
        pub fn as_avm1_button for Avm1Button;
        pub fn as_avm2_button for Avm2Button;
        pub fn as_movie_clip for MovieClip;
        pub fn as_edit_text for EditText;
        pub fn as_text_line for TextLine;
        pub fn as_text for Text;
        pub fn as_morph_shape for MorphShape;
        pub fn as_video for Video;
        pub fn as_bitmap for Bitmap;
    }

    pub fn as_interactive(self) -> Option<InteractiveObject<'gc>> {
        match self {
            Self::Avm1Button(dobj) => Some(InteractiveObject::Avm1Button(dobj)),
            Self::Avm2Button(dobj) => Some(InteractiveObject::Avm2Button(dobj)),
            Self::EditText(dobj) => Some(InteractiveObject::EditText(dobj)),
            Self::TextLine(dobj) => Some(InteractiveObject::TextLine(dobj)),
            Self::LoaderDisplay(dobj) => Some(InteractiveObject::LoaderDisplay(dobj)),
            Self::MovieClip(dobj) => Some(InteractiveObject::MovieClip(dobj)),
            Self::Stage(dobj) => Some(InteractiveObject::Stage(dobj)),
            _ => None,
        }
    }

    pub fn downgrade(self) -> DisplayObjectWeak<'gc> {
        match self {
            DisplayObject::MovieClip(mc) => DisplayObjectWeak::MovieClip(mc.downgrade()),
            DisplayObject::LoaderDisplay(l) => DisplayObjectWeak::LoaderDisplay(l.downgrade()),
            DisplayObject::Bitmap(b) => DisplayObjectWeak::Bitmap(b.downgrade()),
            _ => panic!("Downgrade not yet implemented for {self:?}"),
        }
    }
}

bitflags! {
    /// Bit flags used by `DisplayObject`.
    #[derive(Clone, Copy)]
    struct DisplayObjectFlags: u32 {
        /// Whether this object has been removed from the display list.
        /// Necessary in AVM1 to throw away queued actions from removed movie clips.
        const AVM1_REMOVED             = 1 << 0;

        /// If this object is visible (`_visible` property).
        const VISIBLE                  = 1 << 1;

        /// Whether the `_xscale`, `_yscale` and `_rotation` of the object have been calculated and cached.
        const SCALE_ROTATION_CACHED    = 1 << 2;

        /// Whether this object has been transformed by ActionScript.
        /// When this flag is set, changes from SWF `PlaceObject` tags are ignored.
        const TRANSFORMED_BY_SCRIPT    = 1 << 3;

        /// Whether this object has been placed in a container by ActionScript 3.
        /// When this flag is set, changes from SWF `RemoveObject` tags are ignored.
        // TODO [KJ] Can we repurpose it to cover PLACED_BY_AVM1_SCRIPT too?
        const PLACED_BY_AVM2_SCRIPT    = 1 << 4;

        /// Whether this object has been instantiated by a SWF tag.
        /// When this flag is set, attempts to change the object's name from AVM2 throw an exception.
        const INSTANTIATED_BY_TIMELINE = 1 << 5;

        /// Whether this object is a "root", the top-most display object of a loaded SWF or Bitmap.
        /// Used by `MovieClip.getBytesLoaded` in AVM1 and `DisplayObject.root` in AVM2.
        const IS_ROOT                  = 1 << 6;

        /// Whether this object has `_lockroot` set to true, in which case
        /// it becomes the _root of itself and of any children
        const LOCK_ROOT                = 1 << 7;

        /// Whether this object will be cached to bitmap.
        const CACHE_AS_BITMAP          = 1 << 8;

        /// Whether this object has a scroll rectangle applied.
        const HAS_SCROLL_RECT          = 1 << 9;

        /// Whether this object has an explicit name.
        const HAS_EXPLICIT_NAME        = 1 << 10;

        /// Flag set when we should skip running our next 'enterFrame'
        /// for ourself and our children.
        /// This is set for objects constructed from ActionScript,
        /// which are observed to lag behind objects placed by the timeline
        /// (even if they are both placed in the same frame)
        const SKIP_NEXT_ENTER_FRAME    = 1 << 11;

        /// If this object has already had `invalidate_cached_bitmap` called this frame
        const CACHE_INVALIDATED        = 1 << 12;

        /// If this AVM1 object is pending removal (will be removed on the next frame).
        const AVM1_PENDING_REMOVAL     = 1 << 13;

        /// Whether this object has matrix3D (used for stubbing).
        const HAS_MATRIX3D_STUB        = 1 << 14;

        /// Whether this object has been placed by an AVM1 method,
        /// i.e. attachMovie, createEmptyMovieClip, duplicateMovieClip.
        // TODO [KJ] Can this be merged with PLACED_BY_AVM2_SCRIPT?
        const PLACED_BY_AVM1_SCRIPT    = 1 << 15;

        /// Whether this object was placed by the timeline on a `MovieClip`
        /// before the `MovieClip` had its AVM2 object constructed. Such objects
        /// are only instantiated by `Sprite.constructChildren`, which is
        /// usually called when `super()` is called in a `Sprite` subclass.
        /// However, if `super()` (and therefore `Sprite.constructChildren()`)
        /// is never called, the object will never be instantiated. We mark all
        /// objects placed by the timeline on a load frame with this flag to
        /// ensure that `MovieClip::construct_frame` does not instantiate them
        /// (they need to be instantiated "manually" by
        /// `Sprite.constructChildren`).
        const MANUAL_FRAME_CONSTRUCT  = 1 << 16;

        /// Exact AQW AvatarMC root eligible for internal stable-output bitmap caching.
        const AETHER_ADAPTIVE_AVATAR_CACHE_CANDIDATE = 1 << 17;

        /// Internal adaptive cache contribution; distinct from the authored CACHE_AS_BITMAP bit.
        const AETHER_ADAPTIVE_AVATAR_CACHE_ACTIVE = 1 << 18;

        /// Exact AQW AvatarMC root eligible for one bounded adaptive cache.
        const AETHER_ADAPTIVE_AVATAR_CACHE_ROOT = 1 << 19;
    }
}

#[cfg(test)]
mod avm2_lifecycle_dirty_tests {
    use super::*;
    use gc_arena::arena::rootless_mutate;
    use std::sync::Arc;

    #[test]
    fn lifecycle_traversal_skips_clean_subtrees() {
        rootless_mutate(|mc| {
            let movie = Arc::new(SwfMovie::empty(10, None));
            let clip = MovieClip::new(movie, mc);

            assert!(clip.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Enter));
            assert!(clip.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct));
            assert!(clip.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::FrameScripts));

            assert!(!clip.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Enter));
            assert!(!clip.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct));
            assert!(!clip.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::FrameScripts));

            clip.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::Construct);

            assert!(!clip.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Enter));
            assert!(clip.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct));
            assert!(!clip.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::FrameScripts));
        });
    }

    #[test]
    fn nested_goto_schedules_only_the_changed_branch() {
        rootless_mutate(|mc| {
            let movie = Arc::new(SwfMovie::empty(10, None));
            let parent = MovieClip::new(movie.clone(), mc);
            let changed = MovieClip::new(movie.clone(), mc);
            let clean_sibling = MovieClip::new(movie, mc);

            for traversal in [
                Avm2LifecycleTraversal::Construct,
                Avm2LifecycleTraversal::FrameScripts,
            ] {
                assert!(parent.begin_avm2_lifecycle_traversal(traversal));
                assert!(changed.begin_avm2_lifecycle_traversal(traversal));
                assert!(clean_sibling.begin_avm2_lifecycle_traversal(traversal));
            }

            let changed_write = Gc::write(mc, changed.base());
            DisplayObjectBase::set_parent_ignoring_orphan_list(changed_write, Some(parent.into()));
            let sibling_write = Gc::write(mc, clean_sibling.base());
            DisplayObjectBase::set_parent_ignoring_orphan_list(sibling_write, Some(parent.into()));

            changed.schedule_avm2_nested_goto_lifecycle();

            for traversal in [
                Avm2LifecycleTraversal::Construct,
                Avm2LifecycleTraversal::FrameScripts,
            ] {
                assert!(parent.begin_avm2_lifecycle_traversal(traversal));
                assert!(changed.begin_avm2_lifecycle_traversal(traversal));
                assert!(!clean_sibling.begin_avm2_lifecycle_traversal(traversal));
            }
        });
    }

    #[test]
    fn lifecycle_mutation_dirties_clean_ancestors() {
        rootless_mutate(|mc| {
            let movie = Arc::new(SwfMovie::empty(10, None));
            let parent = MovieClip::new(movie.clone(), mc);
            let child = MovieClip::new(movie, mc);

            assert!(parent.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct));
            assert!(child.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct));

            let child_write = Gc::write(mc, child.base());
            DisplayObjectBase::set_parent_ignoring_orphan_list(child_write, Some(parent.into()));

            child.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::Construct);

            assert!(parent.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct));
            assert!(child.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct));
        });
    }

    #[test]
    fn mutation_during_traversal_remains_dirty_for_the_next_pass() {
        rootless_mutate(|mc| {
            let movie = Arc::new(SwfMovie::empty(10, None));
            let clip = MovieClip::new(movie, mc);

            assert!(clip.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct));
            clip.mark_avm2_lifecycle_dirty(Avm2LifecycleTraversal::Construct);

            assert!(clip.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct));
            assert!(!clip.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct));
        });
    }

    #[test]
    fn instantiated_clone_starts_dirty_even_when_its_template_is_clean() {
        rootless_mutate(|mc| {
            let movie = Arc::new(SwfMovie::empty(10, None));
            let template = MovieClip::new(movie, mc);

            assert!(template.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct,));
            assert!(!template.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct,));

            let instance = template.instantiate(mc);
            assert!(instance.begin_avm2_lifecycle_traversal(Avm2LifecycleTraversal::Construct,));
        });
    }

    #[test]
    fn bitmap_cache_stage_scale_change_requires_rebuild() {
        let cache = BitmapCache {
            stage_scale_a: 1.0,
            stage_scale_d: 1.0,
            ..Default::default()
        };
        let unchanged = Matrix {
            a: 1.0,
            d: 1.0,
            ..Default::default()
        };
        let resized = Matrix {
            a: 1.25,
            d: 1.25,
            ..Default::default()
        };

        assert_eq!(cache.stage_scale_dirty_reason(&unchanged), None);
        assert_eq!(
            cache.stage_scale_dirty_reason(&resized),
            Some("stage_scale_change")
        );
    }

    #[test]
    fn viewport_change_forcefully_invalidates_a_live_bitmap_cache() {
        let base = DisplayObjectBase::default();
        base.set_bitmap_cached_preference(true);
        assert!(
            base.bitmap_cache_mut()
                .as_ref()
                .is_some_and(|cache| !cache.matrix_a.is_nan())
        );

        base.invalidate_bitmap_cache_for_viewport_change();

        assert!(
            base.bitmap_cache_mut()
                .as_ref()
                .is_some_and(|cache| cache.matrix_a.is_nan())
        );
        assert!(base.contains_flag(DisplayObjectFlags::CACHE_INVALIDATED));
    }

    #[test]
    #[cfg(feature = "aether_performance")]
    fn a_content_size_change_alone_invalidates_the_cache() {
        // A glow whose radius animates, a drop shadow, or damage text growing from three digits to
        // four changes the cache's SOURCE SIZE while its transform stays identical. The full path
        // calls that "size_change" and rebuilds. The fast path has to agree, or it hands back the
        // previous frame's bitmap along with its stale draw_offset and output size — which is drawn
        // clipped to the old rectangle and shifted by the old offset.
        #[derive(Debug)]
        struct FakeHandle;
        impl ruffle_render::bitmap::BitmapHandleImpl for FakeHandle {}

        let mut cache = BitmapCache::default();
        let matrix = Matrix::scale(1.0, 1.0);
        let stage = Matrix::scale(1.0, 1.0);
        cache.bitmap = Some(ruffle_render::bitmap::BitmapInfo {
            handle: BitmapHandle(std::sync::Arc::new(FakeHandle)),
            width: 168,
            height: 40,
        });

        cache.matrix_a = matrix.a;
        cache.matrix_b = matrix.b;
        cache.matrix_c = matrix.c;
        cache.matrix_d = matrix.d;
        cache.stage_scale_a = stage.a;
        cache.stage_scale_d = stage.d;
        cache.source_width = 120;
        cache.source_height = 40;

        // Same transform, larger content.
        assert_eq!(
            cache.dirty_reason(&matrix, 168, 40, &stage),
            Some("size_change"),
            "the full path must rebuild when the content grew"
        );
        // There is deliberately no fast path left to ask. `fast_hit_info` validated the transform
        // and the stage scale but not this, so it answered "clean hit" here and served the previous
        // frame's bitmap with its stale draw_offset and output size.
    }

    #[test]
    fn bitmap_cache_capacity_tracks_alternating_animation_bounds_exactly() {
        assert_eq!(
            bitmap_cache_texture_plan(
                Some((463, 584)),
                (637, 498),
                BitmapCacheTexturePolicy::Exact,
            ),
            BitmapCacheTexturePlan::Allocate {
                width: 637,
                height: 498,
            }
        );
        assert_eq!(
            bitmap_cache_texture_plan(
                Some((637, 584)),
                (463, 584),
                BitmapCacheTexturePolicy::Exact,
            ),
            BitmapCacheTexturePlan::Allocate {
                width: 463,
                height: 584,
            }
        );
    }

    #[test]
    fn an_animating_avatar_reuses_its_cache_texture_across_one_pixel_bounds_changes() {
        // AQW avatars shift their bounds by a pixel or two every frame as they animate. Under
        // `Exact` that allocates a brand-new texture each time while the old one waits for the
        // GPU to retire it -- measured at 300-1,729 MB/second of cache textures on a crowded
        // Battleon. `BoundedReuse` keeps the existing surface when it still contains the
        // request and has not become wastefully large.
        for requested in [(462, 583), (461, 580), (400, 500)] {
            assert_eq!(
                bitmap_cache_texture_plan(
                    Some((463, 584)),
                    requested,
                    BitmapCacheTexturePolicy::BoundedReuse,
                ),
                BitmapCacheTexturePlan::Reuse,
                "{requested:?} fits inside the existing 463x584 surface"
            );
            assert_eq!(
                bitmap_cache_texture_plan(
                    Some((463, 584)),
                    requested,
                    BitmapCacheTexturePolicy::Exact,
                ),
                BitmapCacheTexturePlan::Allocate {
                    width: requested.0,
                    height: requested.1,
                },
                "Exact reallocates for the same request"
            );
        }
    }

    #[test]
    fn bitmap_cache_capacity_drops_a_transient_large_effect_immediately() {
        assert_eq!(
            bitmap_cache_texture_plan(
                Some((1_031, 840)),
                (463, 584),
                BitmapCacheTexturePolicy::Exact,
            ),
            BitmapCacheTexturePlan::Allocate {
                width: 463,
                height: 584,
            }
        );
    }

    #[test]
    fn bitmap_cache_capacity_is_exact_when_padding_could_be_visible() {
        assert_eq!(
            bitmap_cache_texture_plan(
                Some((637, 584)),
                (463, 584),
                BitmapCacheTexturePolicy::Exact,
            ),
            BitmapCacheTexturePlan::Allocate {
                width: 463,
                height: 584,
            }
        );
    }

    #[test]
    fn bitmap_cache_capacity_reuses_only_an_exact_match() {
        assert_eq!(
            bitmap_cache_texture_plan(
                Some((463, 584)),
                (463, 584),
                BitmapCacheTexturePolicy::Exact,
            ),
            BitmapCacheTexturePlan::Reuse
        );
    }

    #[test]
    fn adaptive_avatar_cache_reuses_bounded_high_water_capacity() {
        assert_eq!(
            bitmap_cache_texture_plan(
                Some((463, 584)),
                (637, 498),
                BitmapCacheTexturePolicy::BoundedReuse,
            ),
            BitmapCacheTexturePlan::Allocate {
                width: 637,
                height: 584,
            }
        );
        assert_eq!(
            bitmap_cache_texture_plan(
                Some((637, 584)),
                (463, 584),
                BitmapCacheTexturePolicy::BoundedReuse,
            ),
            BitmapCacheTexturePlan::Reuse
        );
    }

    #[test]
    fn adaptive_avatar_cache_drops_excessive_capacity() {
        assert_eq!(
            bitmap_cache_texture_plan(
                Some((1_031, 840)),
                (463, 584),
                BitmapCacheTexturePolicy::BoundedReuse,
            ),
            BitmapCacheTexturePlan::Allocate {
                width: 463,
                height: 584,
            }
        );
    }

    #[test]
    fn adaptive_avatar_cache_rejects_oversized_live_textures() {
        assert!(adaptive_avatar_cache_dimensions_allowed(1_024, 1_024));
        assert!(!adaptive_avatar_cache_dimensions_allowed(2_049, 1));
        assert!(!adaptive_avatar_cache_dimensions_allowed(2_048, 1_024));
    }

    #[test]
    fn repeatedly_invalidated_filterless_cache_enters_temporary_direct_rendering() {
        let mut cache = BitmapCache::default();
        assert!(!cache.note_filterless_rebuild());
        assert!(!cache.note_filterless_rebuild());
        assert!(cache.note_filterless_rebuild());

        // Reaching the hot threshold only requests a semantic-safety check. The expensive
        // descendant walk decides whether the direct-render window may actually begin.
        assert!(!cache.take_filterless_direct_render_frame());
        cache.begin_filterless_direct_rendering();
        assert!(cache.take_filterless_direct_render_frame());
    }

    #[test]
    fn filterless_direct_render_skips_redundant_nested_safety_scans() {
        assert!(!filterless_direct_render_safety_check_needed(false, false));
        assert!(filterless_direct_render_safety_check_needed(false, true));
        assert!(!filterless_direct_render_safety_check_needed(true, true));
    }

    #[test]
    fn clean_filterless_cache_hit_cancels_hot_rebuild_streak() {
        let mut cache = BitmapCache::default();
        assert!(!cache.note_filterless_rebuild());
        assert!(!cache.note_filterless_rebuild());
        cache.note_cache_hit();
        assert!(!cache.note_filterless_rebuild());
    }

    #[test]
    fn bitmap_cache_culling_uses_filtered_output_bounds() {
        let offscreen = Rectangle {
            x_min: Twips::from_pixels_i32(1_100),
            x_max: Twips::from_pixels_i32(1_200),
            y_min: Twips::from_pixels_i32(100),
            y_max: Twips::from_pixels_i32(200),
        };
        let no_growth = Rectangle {
            x_min: 0,
            x_max: 100,
            y_min: 0,
            y_max: 100,
        };
        assert!(!bitmap_cache_output_intersects_viewport(
            offscreen, no_growth, 960, 550,
        ));

        let near_edge = Rectangle {
            x_min: Twips::from_pixels_i32(965),
            x_max: Twips::from_pixels_i32(1_065),
            y_min: Twips::from_pixels_i32(100),
            y_max: Twips::from_pixels_i32(200),
        };
        let glow_growth = Rectangle {
            x_min: -10,
            x_max: 110,
            y_min: -10,
            y_max: 110,
        };
        assert!(bitmap_cache_output_intersects_viewport(
            near_edge,
            glow_growth,
            960,
            550,
        ));
    }

    #[test]
    fn aether_adaptive_avatar_cache_requires_three_clean_frames() {
        let base = DisplayObjectBase::default();
        base.set_aether_adaptive_avatar_cache_candidate(true);

        base.update_aether_adaptive_avatar_cache(true);
        base.update_aether_adaptive_avatar_cache(true);
        assert!(!base.aether_adaptive_avatar_cache_active());
        assert!(base.cell.borrow().cache.is_none());

        base.update_aether_adaptive_avatar_cache(true);
        assert!(base.aether_adaptive_avatar_cache_active());
        assert!(base.cell.borrow().cache.is_some());
        assert!(!base.is_bitmap_cached_preference());
    }

    #[test]
    fn aether_adaptive_avatar_cache_drops_on_dirty_frame_and_can_reactivate() {
        let base = DisplayObjectBase::default();
        base.set_aether_adaptive_avatar_cache_candidate(true);
        for _ in 0..3 {
            base.update_aether_adaptive_avatar_cache(true);
        }
        assert!(base.aether_adaptive_avatar_cache_active());

        assert!(base.invalidate_cached_bitmap());
        base.update_aether_adaptive_avatar_cache(true);
        assert!(!base.aether_adaptive_avatar_cache_active());
        assert!(base.cell.borrow().cache.is_none());

        base.clear_invalidate_flag();
        for _ in 0..2 {
            base.update_aether_adaptive_avatar_cache(true);
        }
        assert!(!base.aether_adaptive_avatar_cache_active());
        base.update_aether_adaptive_avatar_cache(true);
        assert!(base.aether_adaptive_avatar_cache_active());
    }

    #[test]
    #[cfg(feature = "aether_performance")]
    fn aether_adaptive_avatar_persistent_motion_never_uses_internal_cache() {
        let base = DisplayObjectBase::default();
        base.set_aether_adaptive_avatar_cache_candidate(true);

        for _ in 0..8 {
            base.set_flag(DisplayObjectFlags::CACHE_INVALIDATED, true);
            base.update_aether_adaptive_avatar_cache(true);

            assert!(!base.aether_adaptive_avatar_cache_active());
            assert!(base.cell.borrow().cache.is_none());

            base.clear_invalidate_flag();
        }
    }

    #[test]
    fn aether_adaptive_avatar_deactivation_retains_authored_cache() {
        let base = DisplayObjectBase::default();
        base.set_bitmap_cached_preference(true);
        base.set_aether_adaptive_avatar_cache_candidate(true);
        for _ in 0..3 {
            base.update_aether_adaptive_avatar_cache(true);
        }
        assert!(base.aether_adaptive_avatar_cache_active());
        assert!(base.is_bitmap_cached_preference());

        assert!(base.invalidate_cached_bitmap());
        base.update_aether_adaptive_avatar_cache(true);
        assert!(!base.aether_adaptive_avatar_cache_active());
        assert!(base.cell.borrow().cache.is_some());
        assert!(base.is_bitmap_cached_preference());
    }

    #[test]
    fn aether_adaptive_avatar_runtime_disable_removes_only_internal_cache() {
        let base = DisplayObjectBase::default();
        base.set_aether_adaptive_avatar_cache_candidate(true);
        for _ in 0..3 {
            base.update_aether_adaptive_avatar_cache(true);
        }
        assert!(base.aether_adaptive_avatar_cache_active());

        base.update_aether_adaptive_avatar_cache(false);
        assert!(!base.aether_adaptive_avatar_cache_active());
        assert!(base.cell.borrow().cache.is_none());
        assert!(!base.is_bitmap_cached_preference());
    }

    #[test]
    fn aether_adaptive_avatar_non_translation_transform_drops_internal_cache() {
        let base = DisplayObjectBase::default();
        base.set_aether_adaptive_avatar_cache_candidate(true);
        for _ in 0..3 {
            base.update_aether_adaptive_avatar_cache(true);
        }
        assert!(base.aether_adaptive_avatar_cache_active());

        base.set_matrix(Matrix {
            a: 1.25,
            d: 1.25,
            tx: Twips::from_pixels_i32(100),
            ty: Twips::from_pixels_i32(50),
            ..Default::default()
        });
        base.update_aether_adaptive_avatar_cache(true);

        assert!(!base.aether_adaptive_avatar_cache_active());
        assert!(base.cell.borrow().cache.is_none());
    }

    #[test]
    fn aether_adaptive_avatar_translation_keeps_internal_cache_active() {
        let base = DisplayObjectBase::default();
        base.set_aether_adaptive_avatar_cache_candidate(true);
        for _ in 0..3 {
            base.update_aether_adaptive_avatar_cache(true);
        }
        assert!(base.aether_adaptive_avatar_cache_active());

        base.set_matrix(Matrix {
            tx: Twips::from_pixels_i32(100),
            ty: Twips::from_pixels_i32(50),
            ..Default::default()
        });
        base.update_aether_adaptive_avatar_cache(true);

        assert!(base.aether_adaptive_avatar_cache_active());
        assert!(base.cell.borrow().cache.is_some());
    }

    #[test]
    #[cfg(feature = "aether_performance")]
    fn aether_adaptive_avatar_marks_only_the_avatar_root() {
        rootless_mutate(|mc| {
            let movie = Arc::new(SwfMovie::empty(10, None));
            let root: DisplayObject<'_> = MovieClip::new(movie.clone(), mc).into();
            let mc_char: DisplayObject<'_> = MovieClip::new(movie.clone(), mc).into();
            let chest: DisplayObject<'_> = MovieClip::new(movie.clone(), mc).into();
            let nested_symbol: DisplayObject<'_> = MovieClip::new(movie, mc).into();

            root.base()
                .set_aether_adaptive_avatar_cache_root_candidate(true);
            root.set_name(mc, AvmString::new_utf8(mc, "a123"));
            mc_char.set_name(mc, AvmString::new_utf8(mc, "mcChar"));
            DisplayObjectBase::set_parent_ignoring_orphan_list(
                Gc::write(mc, mc_char.base()),
                Some(root),
            );
            chest.set_name(mc, AvmString::new_utf8(mc, "chest"));
            DisplayObjectBase::set_parent_ignoring_orphan_list(
                Gc::write(mc, chest.base()),
                Some(mc_char),
            );
            DisplayObjectBase::set_parent_ignoring_orphan_list(
                Gc::write(mc, nested_symbol.base()),
                Some(chest),
            );

            root.refresh_aether_adaptive_avatar_cache_candidates();
            mc_char.refresh_aether_adaptive_avatar_cache_candidates();
            chest.refresh_aether_adaptive_avatar_cache_candidates();
            nested_symbol.refresh_aether_adaptive_avatar_cache_candidates();

            assert!(
                root.base()
                    .contains_flag(DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_CANDIDATE)
            );
            assert!(
                !mc_char
                    .base()
                    .contains_flag(DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_CANDIDATE)
            );
            assert!(
                !chest
                    .base()
                    .contains_flag(DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_CANDIDATE)
            );
            assert!(
                !nested_symbol
                    .base()
                    .contains_flag(DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_CANDIDATE)
            );
        });
    }

    #[test]
    #[cfg(feature = "aether_performance")]
    fn aether_adaptive_avatar_excludes_inventory_preview_roots() {
        rootless_mutate(|mc| {
            let movie = Arc::new(SwfMovie::empty(10, None));
            let preview: DisplayObject<'_> = MovieClip::new(movie, mc).into();

            preview
                .base()
                .set_aether_adaptive_avatar_cache_root_candidate(true);
            preview.set_name(mc, AvmString::new_utf8(mc, "previewMCB"));
            preview.refresh_aether_adaptive_avatar_cache_candidates();

            assert!(
                !preview
                    .base()
                    .contains_flag(DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_CANDIDATE)
            );
        });
    }

    #[test]
    #[cfg(feature = "aether_performance")]
    fn aether_adaptive_avatar_sublayers_never_become_independent_candidates() {
        rootless_mutate(|mc| {
            let movie = Arc::new(SwfMovie::empty(10, None));
            let root: DisplayObject<'_> = MovieClip::new(movie.clone(), mc).into();
            let mc_char: DisplayObject<'_> = MovieClip::new(movie.clone(), mc).into();
            let weapon: DisplayObject<'_> = MovieClip::new(movie.clone(), mc).into();
            let unrelated_parent: DisplayObject<'_> = MovieClip::new(movie, mc).into();

            root.base()
                .set_aether_adaptive_avatar_cache_root_candidate(true);
            mc_char.set_name(mc, AvmString::new_utf8(mc, "mcChar"));
            DisplayObjectBase::set_parent_ignoring_orphan_list(
                Gc::write(mc, mc_char.base()),
                Some(root),
            );
            weapon.set_name(mc, AvmString::new_utf8(mc, "weapon"));
            DisplayObjectBase::set_parent_ignoring_orphan_list(
                Gc::write(mc, weapon.base()),
                Some(mc_char),
            );
            root.refresh_aether_adaptive_avatar_cache_candidates();
            mc_char.refresh_aether_adaptive_avatar_cache_candidates();
            weapon.refresh_aether_adaptive_avatar_cache_candidates();
            assert!(
                !weapon
                    .base()
                    .contains_flag(DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_CANDIDATE)
            );

            DisplayObjectBase::set_parent_ignoring_orphan_list(
                Gc::write(mc, weapon.base()),
                Some(unrelated_parent),
            );
            weapon.refresh_aether_adaptive_avatar_cache_candidates();

            assert!(
                !weapon
                    .base()
                    .contains_flag(DisplayObjectFlags::AETHER_ADAPTIVE_AVATAR_CACHE_CANDIDATE)
            );
        });
    }

    #[test]
    fn aether_adaptive_avatar_uses_concatenated_non_translation_transform() {
        let base = DisplayObjectBase::default();
        base.set_aether_adaptive_avatar_cache_candidate(true);
        for _ in 0..3 {
            base.update_aether_adaptive_avatar_cache_with_transform(true, [1.0, 0.0, 0.0, 1.0]);
        }
        assert!(base.aether_adaptive_avatar_cache_active());

        base.update_aether_adaptive_avatar_cache_with_transform(true, [0.0, 1.0, -1.0, 0.0]);

        assert!(!base.aether_adaptive_avatar_cache_active());
        assert!(base.cell.borrow().cache.is_none());
    }
}

bitflags! {
    /// Defines how hit testing should be performed.
    /// Used for mouse picking and ActionScript's hitTestClip functions.
    #[derive(Clone, Copy)]
    pub struct HitTestOptions: u8 {
        /// Ignore objects used as masks (setMask / clipDepth).
        const SKIP_MASK = 1 << 0;

        /// Ignore objects with the ActionScript's visibility flag turned off.
        const SKIP_INVISIBLE = 1 << 1;

        /// Check only the specified object. Ignore any children of that object.
        const SKIP_CHILDREN = 1 << 2;

        /// The options used for `hitTest` calls in ActionScript.
        const AVM_HIT_TEST = Self::SKIP_MASK.bits();

        /// The options used for mouse picking, such as clicking on buttons.
        const MOUSE_PICK = Self::SKIP_MASK.bits() | Self::SKIP_INVISIBLE.bits();
    }
}

/// A binding from a property of an AVM1 StageObject to an EditText text field.
#[derive(Copy, Clone, Collect)]
#[collect(no_drop)]
pub struct Avm1TextFieldBinding<'gc> {
    pub text_field: EditText<'gc>,
    pub variable_name: AvmString<'gc>,
}

impl<'gc> Avm1TextFieldBinding<'gc> {
    pub fn bind_variables(activation: &mut Activation<'_, 'gc>) {
        // Check all unbound text fields to see if they apply to this object.
        // TODO: Replace with `Vec::drain_filter` when stable.
        let mut i = 0;
        let mut len = activation.context.unbound_text_fields.len();
        while i < len {
            if activation.context.unbound_text_fields[i]
                .try_bind_text_field_variable(activation, false)
            {
                activation.context.unbound_text_fields.swap_remove(i);
                len -= 1;
            } else {
                i += 1;
            }
        }
    }

    /// Registers a text field variable binding for this stage object.
    /// Whenever a property with the given name is changed, we should change the text in the text field.
    pub fn register_binding(self, dobj: DisplayObject<'gc>, mc: &Mutation<'gc>) {
        if let Some(mut bindings) = dobj.avm1_text_field_bindings_mut(mc) {
            bindings.push(self);
        }
    }

    /// Removes a text field binding for the given text field.
    /// Does not place the text field on the unbound list.
    /// Caller is responsible for placing the text field on the unbound list, if necessary.
    pub fn clear_binding(dobj: DisplayObject<'gc>, text_field: EditText<'gc>, mc: &Mutation<'gc>) {
        if let Some(mut bindings) = dobj.avm1_text_field_bindings_mut(mc) {
            bindings.retain(|b| !DisplayObject::ptr_eq(text_field.into(), b.text_field.into()));
        }
    }

    /// Clears all text field bindings from this stage object, and places the textfields on the unbound list.
    /// This is called when the object is removed from the stage.
    pub fn unregister_bindings(dobj: DisplayObject<'gc>, context: &mut UpdateContext<'gc>) {
        let mc = context.gc();
        if let Some(mut bindings) = dobj.avm1_text_field_bindings_mut(mc) {
            for binding in bindings.drain(..) {
                binding.text_field.clear_bound_display_object(context);
                context.unbound_text_fields.push(binding.text_field);
            }
        }
    }
}

/// Represents the sound transform of sounds played inside a Flash MovieClip.
/// Every value is a percentage (0-100), but out of range values are allowed.
/// In AVM1, this is returned by `Sound.getTransform`.
/// In AVM2, this is returned by `Sprite.soundTransform`.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct SoundTransform {
    pub volume: i32,
    pub left_to_left: i32,
    pub left_to_right: i32,
    pub right_to_left: i32,
    pub right_to_right: i32,
}

impl SoundTransform {
    pub const MAX_VOLUME: i32 = 100;

    /// Applies another SoundTransform on top of this SoundTransform.
    #[must_use]
    pub fn concat(mut self, other: SoundTransform) -> SoundTransform {
        const MAX_VOLUME: i64 = SoundTransform::MAX_VOLUME as i64;

        // It seems like Flash masks the results below to 30-bit integers:
        // * Specifically, 0x40000000, -0x40000000 and -0x80000000 are equivalent to zero.
        // Negative values are equivalent to their absolute value.
        const MASK: i32 = (1 << 30) - 1;

        self.volume =
            (i64::from(self.volume) * i64::from(other.volume) / MAX_VOLUME).abs() as i32 & MASK;

        // This is a 2x2 matrix multiply between the transforms.
        // Done with integer math to match Flash behavior.
        let ll0: i64 = self.left_to_left.into();
        let lr0: i64 = self.left_to_right.into();
        let rl0: i64 = self.right_to_left.into();
        let rr0: i64 = self.right_to_right.into();
        let ll1: i64 = other.left_to_left.into();
        let lr1: i64 = other.left_to_right.into();
        let rl1: i64 = other.right_to_left.into();
        let rr1: i64 = other.right_to_right.into();
        self.left_to_left = ((ll0 * ll1 + rl0 * lr1) / MAX_VOLUME) as i32 & MASK;
        self.left_to_right = ((lr0 * ll1 + rr0 * lr1) / MAX_VOLUME) as i32 & MASK;
        self.right_to_left = ((ll0 * rl1 + rl0 * rr1) / MAX_VOLUME) as i32 & MASK;
        self.right_to_right = ((lr0 * rl1 + rr0 * rr1) / MAX_VOLUME) as i32 & MASK;

        self
    }

    /// Returns the pan of this transform.
    /// -100 is full left and 100 is full right.
    /// This matches the behavior of AVM1 `Sound.getPan()`
    pub fn pan(&self) -> i32 {
        // It's not clear why Flash has the weird `abs` behavior, but this
        // matches the values that Flash returns (see `sound` regression test).
        if self.left_to_left != Self::MAX_VOLUME {
            Self::MAX_VOLUME - self.left_to_left.abs()
        } else {
            self.right_to_right.abs() - Self::MAX_VOLUME
        }
    }

    /// Modifies the pan of this transform.
    /// -100 is full left and 100 is full right.
    /// This matches the behavior of AVM1 `Sound.setPan()`.
    #[must_use]
    pub fn with_pan(mut self, pan: i32) -> SoundTransform {
        if pan >= 0 {
            self.left_to_left = Self::MAX_VOLUME - pan;
            self.right_to_right = Self::MAX_VOLUME;
        } else {
            self.left_to_left = Self::MAX_VOLUME;
            self.right_to_right = Self::MAX_VOLUME + pan;
        }
        self.left_to_right = 0;
        self.right_to_left = 0;
        self
    }

    pub fn from_avm2_object(as3_st: Avm2Object<'_>) -> Self {
        let sound_transform = as3_st
            .as_sound_transform()
            .expect("Should pass SoundTransform");

        SoundTransform {
            left_to_left: (sound_transform.left_to_left() * 100.0) as i32,
            left_to_right: (sound_transform.left_to_right() * 100.0) as i32,
            right_to_left: (sound_transform.right_to_left() * 100.0) as i32,
            right_to_right: (sound_transform.right_to_right() * 100.0) as i32,
            volume: (sound_transform.volume() * 100.0) as i32,
        }
    }

    pub fn into_avm2_object<'gc>(
        self,
        activation: &mut Avm2Activation<'_, 'gc>,
    ) -> Result<Avm2Object<'gc>, Avm2Error<'gc>> {
        let as3_st = activation
            .avm2()
            .classes()
            .soundtransform
            .construct(activation, &[])?
            .as_object()
            .unwrap()
            .as_sound_transform()
            .unwrap();

        as3_st.set_left_to_left(self.left_to_left as f64 / 100.0);
        as3_st.set_left_to_right(self.left_to_right as f64 / 100.0);
        as3_st.set_right_to_left(self.right_to_left as f64 / 100.0);
        as3_st.set_right_to_right(self.right_to_right as f64 / 100.0);
        as3_st.set_volume(self.volume as f64 / 100.0);

        Ok(as3_st.into())
    }
}

impl Default for SoundTransform {
    fn default() -> Self {
        Self {
            volume: 100,
            left_to_left: 100,
            left_to_right: 0,
            right_to_left: 0,
            right_to_right: 100,
        }
    }
}

/// A version of `DisplayObject` that holds weak pointers.
/// Currently, this is only used by orphan handling, so we only
/// need two variants. If other use cases arise, feel free
/// to add more variants.
#[derive(Copy, Clone, Collect)]
#[collect(no_drop)]
pub enum DisplayObjectWeak<'gc> {
    MovieClip(MovieClipWeak<'gc>),
    LoaderDisplay(LoaderDisplayWeak<'gc>),
    Bitmap(BitmapWeak<'gc>),
}

impl<'gc> DisplayObjectWeak<'gc> {
    pub fn as_ptr(&self) -> *const DisplayObjectPtr {
        match self {
            DisplayObjectWeak::MovieClip(mc) => mc.as_ptr(),
            DisplayObjectWeak::LoaderDisplay(ld) => ld.as_ptr(),
            DisplayObjectWeak::Bitmap(b) => b.as_ptr(),
        }
    }

    pub fn upgrade(&self, mc: &Mutation<'gc>) -> Option<DisplayObject<'gc>> {
        match self {
            DisplayObjectWeak::MovieClip(movie) => movie.upgrade(mc).map(|m| m.into()),
            DisplayObjectWeak::LoaderDisplay(ld) => ld.upgrade(mc).map(|ld| ld.into()),
            DisplayObjectWeak::Bitmap(b) => b.upgrade(mc).map(|ld| ld.into()),
        }
    }
}

#[cfg(test)]
mod nine_slice_tests {
    use super::SliceAxis;

    /// The point of the grid: a border keeps the size it was drawn at, however far the object is
    /// stretched. Only the middle band takes up the difference.
    #[test]
    fn borders_keep_their_size_when_the_object_is_scaled() {
        // A 100 wide object with 10-wide borders, drawn at three times its size.
        let axis = SliceAxis::plan(0.0, 10.0, 90.0, 100.0, 3.0).expect("a sane grid should plan");

        let (_, _, leading_stretch, _) = axis.band(0);
        let (_, _, middle_stretch, _) = axis.band(1);
        let (_, _, trailing_stretch, _) = axis.band(2);

        // Each border is squeezed to a third in the object's own space, so that the object's own
        // 3x scale puts it back at exactly the size it was authored.
        assert!((leading_stretch - 1.0 / 3.0).abs() < 1e-9);
        assert!((trailing_stretch - 1.0 / 3.0).abs() < 1e-9);
        // Everything the borders gave up goes to the middle.
        assert!(middle_stretch > 1.0);
    }

    /// Why a cached object must not be sliced, in the arithmetic that broke it.
    ///
    /// A cell's transform carries a stretch and a translation. The cached draw path reads only the
    /// translation off the transform stack and discards the stretch, because the image it replays
    /// already has its scaling baked in. So the translation arrives on its own, applied to an
    /// unscaled bitmap, and the object is drawn nine times at nine wrong offsets.
    ///
    /// The offset is `low * (1 - 1/scale)` for the leading band. It is zero only when the art
    /// starts at the object's own origin, which is why a test box drawn from (0,0) showed nothing
    /// wrong. Measured on a box centred on its origin instead, at 3x: the middle moved from 96,96
    /// to 60,60 -- 36 pixels up and to the left -- and the border art disappeared entirely.
    #[test]
    fn a_cells_translation_alone_would_move_a_centred_object() {
        // Art from -50 to 50 rather than 0 to 100: the same box, centred on its origin.
        let axis = SliceAxis::plan(-50.0, -38.0, 38.0, 50.0, 3.0).expect("a sane grid should plan");

        let (source_start, dest_start, stretch, _) = axis.band(0);
        let translation = dest_start - source_start * stretch;

        // With the stretch applied the band lands where it should.
        assert!((source_start * stretch + translation - dest_start).abs() < 1e-9);

        // Without it -- which is what a cached draw does -- the same translation is a bare shift,
        // and it is neither zero nor harmless.
        assert!(
            translation < -30.0,
            "a centred object's leading cell shifts it {translation} px, not nothing"
        );
    }

    /// Why an object placed smaller than it was drawn is declined rather than sliced.
    ///
    /// A border keeps its drawn size by being divided by the object's scale. Below one that
    /// division makes the band *bigger* than it was drawn, so the cell covering the corner
    /// magnifies whatever it reaches -- a sliver of the object's own interior, smeared across its
    /// corner. That is the dark wedge that appeared at the top left of the buff icons and the
    /// drop-accept button.
    #[test]
    fn shrinking_would_magnify_a_corner_instead_of_protecting_it() {
        let axis = SliceAxis::plan(0.0, 12.0, 88.0, 100.0, 0.5).expect("the plan itself is sound");
        let (_, _, stretch, _) = axis.band(0);
        assert!(
            (stretch - 2.0).abs() < 1e-9,
            "at half size the border band is drawn at {stretch}x, not 1x"
        );
        assert!(stretch > 1.0, "which is magnification, not protection");

        // Grown, the same band is drawn smaller, which is the whole point.
        let axis = SliceAxis::plan(0.0, 12.0, 88.0, 100.0, 3.0).unwrap();
        let (_, _, stretch, _) = axis.band(0);
        assert!(stretch < 1.0);
    }

    #[test]
    fn the_bands_tile_the_whole_object_without_gaps() {
        let axis = SliceAxis::plan(0.0, 10.0, 90.0, 100.0, 3.0).unwrap();
        let (_, first_start, _, first_end) = axis.band(0);
        let (_, second_start, _, second_end) = axis.band(1);
        let (_, third_start, _, third_end) = axis.band(2);

        assert!((first_start - 0.0).abs() < 1e-9);
        assert!((first_end - second_start).abs() < 1e-9);
        assert!((second_end - third_start).abs() < 1e-9);
        assert!((third_end - 100.0).abs() < 1e-9);
    }

    /// An unscaled object is already right, and slicing it would be nine draws for nothing -- but
    /// it still has to come out identical rather than subtly shifted.
    #[test]
    fn an_unscaled_object_is_left_alone() {
        let axis = SliceAxis::plan(0.0, 10.0, 90.0, 100.0, 1.0).unwrap();
        for band in 0..3 {
            let (source_start, dest_start, stretch, _) = axis.band(band);
            assert!((stretch - 1.0).abs() < 1e-9);
            assert!((source_start - dest_start).abs() < 1e-9);
        }
    }

    /// Squeezed so far that the borders alone would not fit, there is nothing sensible to do and
    /// the object is better drawn the ordinary way than folded inside out.
    #[test]
    fn a_grid_with_no_room_left_declines() {
        assert!(SliceAxis::plan(0.0, 10.0, 90.0, 100.0, 0.05).is_none());
    }

    #[test]
    fn a_grid_outside_the_bounds_declines() {
        assert!(SliceAxis::plan(0.0, -5.0, 90.0, 100.0, 2.0).is_none());
        assert!(SliceAxis::plan(0.0, 10.0, 120.0, 100.0, 2.0).is_none());
        // Inverted, and a zero scale.
        assert!(SliceAxis::plan(0.0, 90.0, 10.0, 100.0, 2.0).is_none());
        assert!(SliceAxis::plan(0.0, 10.0, 90.0, 100.0, 0.0).is_none());
    }
}
