//! Where a blend or mask sub-target actually has to live.
//!
//! `WgpuCommandHandler::blend` builds its `Surface` at the whole stage size, so one `Command::Blend`
//! costs a stage-sized colour texture plus, at `StageQuality::High`, a 4x MSAA framebuffer beside it
//! — about 70 MiB at 2560x1440, whatever the blended object's actual size. AQW scenes are built from
//! hundreds of small blended effect clips, so peak texture memory tracks the NUMBER of blend nodes
//! rather than their area, which is what drove resident memory to 39.5 GB and the device out of it.
//!
//! The fix is to size each sub-target to its contents. The arithmetic is small but it is exactly what
//! a previous attempt got wrong — glow and blur were clipped to the object's own bounds — so it lives
//! here as a pure function with tests rather than inline in the command handler.
//!
//! Two rules make it safe:
//!
//! 1. **Grow before you clamp.** A filter draws OUTSIDE the bounds of the thing it is applied to. The
//!    grown rect is the region that must be rendered; clamping the ungrown rect is what clips a glow.
//! 2. **Clamp to the parent, never to the content.** Anything outside the parent surface cannot be
//!    seen, so dropping it is free. Anything inside it must be kept.
//!
//! A blend whose content is off-screen entirely produces `None`: there is nothing to allocate and
//! nothing to composite.

use ruffle_render::filters::Filter;
use swf::{Rectangle, Twips};

/// A sub-target's placement within its parent surface, in device pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlendRegion {
    /// Offset of this target's top-left within the parent surface.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl BlendRegion {
    /// The whole parent surface — what the renderer used to allocate unconditionally, and still the
    /// right answer when a blend really does cover everything, or when it is a kind of blend that
    /// cannot be moved.
    pub fn at(origin: (u32, u32), width: u32, height: u32) -> Self {
        Self {
            x: origin.0,
            y: origin.1,
            width,
            height,
        }
    }

    pub fn is_full(
        &self,
        parent_origin: (u32, u32),
        parent_width: u32,
        parent_height: u32,
    ) -> bool {
        self.x == parent_origin.0
            && self.y == parent_origin.1
            && self.width == parent_width
            && self.height == parent_height
    }

    /// Bytes saved against a full-surface target of the same format and sample count.
    pub fn saved_fraction(&self, parent_width: u32, parent_height: u32) -> f64 {
        let parent = f64::from(parent_width) * f64::from(parent_height);
        if parent <= 0.0 {
            return 0.0;
        }
        let mine = f64::from(self.width) * f64::from(self.height);
        (1.0 - mine / parent).clamp(0.0, 1.0)
    }
}

/// Plan the sub-target for a blend whose contents occupy `content` (already in stage space), with
/// `filters` applied to it, inside a parent surface of `parent_width` x `parent_height` pixels.
///
/// `stage_scale` is the stage view matrix's scale, the same pair `DisplayObject`'s cache path feeds
/// to `Filter::scale` — filter radii are authored in movie space and grow by the stage scale, so a
/// blur on a scaled-up stage covers proportionally more pixels.
///
/// Returns `None` when nothing lands inside the parent.
pub fn plan_blend_region(
    content: Rectangle<Twips>,
    filters: &[Filter],
    stage_scale: (f32, f32),
    parent_origin: (u32, u32),
    parent_width: u32,
    parent_height: u32,
) -> Option<BlendRegion> {
    if parent_width == 0 || parent_height == 0 || !content.is_valid() {
        return None;
    }

    // Rule 1: grow for every filter before looking at the parent at all.
    let mut grown = content;
    for filter in filters {
        let mut scaled = filter.clone();
        scaled.scale(stage_scale.0, stage_scale.1);
        grown = scaled.calculate_dest_rect(grown);
    }

    let x_min = grown.x_min.to_pixels().floor();
    let y_min = grown.y_min.to_pixels().floor();
    let x_max = grown.x_max.to_pixels().ceil();
    let y_max = grown.y_max.to_pixels().ceil();

    // Rule 2: clamp to the parent, which is the only thing that can be observed. The parent is not
    // necessarily at the origin — a blend nested inside another blend renders into a target that is
    // itself a window onto the stage — and `content` is always in the space the commands were
    // recorded in, so the clamp has to be against the parent's actual span rather than 0..size.
    let (origin_x, origin_y) = (f64::from(parent_origin.0), f64::from(parent_origin.1));
    let left = x_min.max(origin_x);
    let top = y_min.max(origin_y);
    let right = x_max.min(origin_x + f64::from(parent_width));
    let bottom = y_max.min(origin_y + f64::from(parent_height));

    if right <= left || bottom <= top {
        return None;
    }

    Some(BlendRegion {
        x: left as u32,
        y: top as u32,
        width: (right - left).ceil() as u32,
        height: (bottom - top).ceil() as u32,
    })
}

/// Round a region out to a grid so that near-identical requests share one pool bucket.
///
/// The pool keys buckets on exact dimensions, and animating content drifts by a pixel or two every
/// frame -- a census of one session recorded 512 distinct sizes with pairs like 2209x931 beside
/// 2209x853 and 1093x419 beside 1093x378. Each of those is a bucket that is allocated once, never
/// matched again, and meanwhile crowds the retention budget. Snapping to a grid collapses them.
///
/// Growing a region is always safe: it can only take in more of the surface, never less, so nothing
/// that was going to be drawn gets clipped. The origin rounds DOWN and the far edge rounds UP, then
/// both are clamped back inside the parent -- which is why edge-touching regions can still land off
/// the grid, and why that is fine.
pub fn quantise_region(
    region: BlendRegion,
    parent_origin: (u32, u32),
    parent_width: u32,
    parent_height: u32,
    grid: u32,
) -> BlendRegion {
    let grid = grid.max(1);
    let snap_down = |v: u32, floor: u32| floor + ((v.saturating_sub(floor)) / grid) * grid;
    let snap_up = |v: u32, floor: u32| floor + (v.saturating_sub(floor)).div_ceil(grid) * grid;

    let (ox, oy) = parent_origin;
    let left = snap_down(region.x, ox);
    let top = snap_down(region.y, oy);
    let right = snap_up(region.x + region.width, ox).min(ox + parent_width);
    let bottom = snap_up(region.y + region.height, oy).min(oy + parent_height);

    BlendRegion {
        x: left,
        y: top,
        width: right.saturating_sub(left).max(1),
        height: bottom.saturating_sub(top).max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x_min: f64, y_min: f64, x_max: f64, y_max: f64) -> Rectangle<Twips> {
        Rectangle {
            x_min: Twips::from_pixels(x_min),
            y_min: Twips::from_pixels(y_min),
            x_max: Twips::from_pixels(x_max),
            y_max: Twips::from_pixels(y_max),
        }
    }

    fn blur(x: f64, y: f64) -> Filter {
        Filter::BlurFilter(swf::BlurFilter {
            blur_x: swf::Fixed16::from_f64(x),
            blur_y: swf::Fixed16::from_f64(y),
            flags: swf::BlurFilterFlags::from_passes(1),
        })
    }

    #[test]
    fn a_small_effect_clip_does_not_get_a_stage_sized_target() {
        // The whole point: an 80x60 effect in the middle of a 1440p stage costs an 80x60 target,
        // not 2560x1440. At 4x MSAA that is the difference between ~75 KiB and ~59 MiB.
        let region = plan_blend_region(
            rect(400.0, 300.0, 480.0, 360.0),
            &[],
            (1.0, 1.0),
            (0, 0),
            2560,
            1440,
        )
        .expect("an on-screen clip has a region");

        assert_eq!(
            region,
            BlendRegion {
                x: 400,
                y: 300,
                width: 80,
                height: 60
            }
        );
        assert!(region.saved_fraction(2560, 1440) > 0.99);
    }

    #[test]
    fn a_blur_is_grown_into_the_target_rather_than_clipped_at_the_object_edge() {
        // The regression that killed the previous attempt. The filter draws outside the object, so
        // the target must cover the GROWN rect; sizing to the raw bounds cuts the glow off.
        let bare = plan_blend_region(
            rect(400.0, 300.0, 480.0, 360.0),
            &[],
            (1.0, 1.0),
            (0, 0),
            2560,
            1440,
        )
        .expect("region");
        let blurred = plan_blend_region(
            rect(400.0, 300.0, 480.0, 360.0),
            &[blur(16.0, 16.0)],
            (1.0, 1.0),
            (0, 0),
            2560,
            1440,
        )
        .expect("region");

        assert!(
            blurred.width > bare.width && blurred.height > bare.height,
            "a blur must enlarge the target ({blurred:?} vs {bare:?})"
        );
        assert!(
            blurred.x < bare.x && blurred.y < bare.y,
            "the grown target must start above and left of the object ({blurred:?} vs {bare:?})"
        );
    }

    #[test]
    fn filter_growth_follows_the_stage_scale() {
        // Filter radii are authored in movie space. Rendering the stage at 2x means a 16px blur
        // covers 32 device pixels, and a target sized for 16 would clip half of it.
        let at_1x = plan_blend_region(
            rect(400.0, 300.0, 480.0, 360.0),
            &[blur(16.0, 16.0)],
            (1.0, 1.0),
            (0, 0),
            2560,
            1440,
        )
        .expect("region");
        let at_2x = plan_blend_region(
            rect(400.0, 300.0, 480.0, 360.0),
            &[blur(16.0, 16.0)],
            (2.0, 2.0),
            (0, 0),
            2560,
            1440,
        )
        .expect("region");

        assert!(
            at_2x.width > at_1x.width,
            "a scaled-up stage must widen the target ({at_2x:?} vs {at_1x:?})"
        );
    }

    #[test]
    fn content_hanging_off_the_edge_is_clamped_to_the_parent_not_dropped() {
        // Half off the left edge: keep the visible half, and start at 0 rather than a negative x.
        let region = plan_blend_region(
            rect(-40.0, 300.0, 40.0, 360.0),
            &[],
            (1.0, 1.0),
            (0, 0),
            2560,
            1440,
        )
        .expect("partially visible content still has a region");

        assert_eq!(
            region,
            BlendRegion {
                x: 0,
                y: 300,
                width: 40,
                height: 60
            }
        );
    }

    #[test]
    fn fully_offscreen_content_gets_no_target_at_all() {
        assert_eq!(
            plan_blend_region(
                rect(-500.0, -500.0, -100.0, -100.0),
                &[],
                (1.0, 1.0),
                (0, 0),
                2560,
                1440
            ),
            None
        );
        assert_eq!(
            plan_blend_region(
                rect(4000.0, 300.0, 4200.0, 360.0),
                &[],
                (1.0, 1.0),
                (0, 0),
                2560,
                1440
            ),
            None
        );
    }

    #[test]
    fn a_blend_covering_everything_still_gets_the_whole_surface() {
        let region = plan_blend_region(
            rect(-10.0, -10.0, 3000.0, 2000.0),
            &[],
            (1.0, 1.0),
            (0, 0),
            2560,
            1440,
        )
        .expect("region");

        assert!(
            region.is_full((0, 0), 2560, 1440),
            "{region:?} should be the whole surface"
        );
        assert_eq!(region.saved_fraction(2560, 1440), 0.0);
    }

    #[test]
    fn a_blend_nested_inside_another_clamps_to_that_parents_window_not_to_the_stage() {
        // The parent is itself a 400x300 window onto the stage at (800, 500). A nested blend's
        // bounds are still in STAGE space, so clamping them against 0..400 x 0..300 would place the
        // child somewhere else entirely. It must clamp against 800..1200 x 500..800.
        let region = plan_blend_region(
            rect(700.0, 400.0, 900.0, 600.0),
            &[],
            (1.0, 1.0),
            (800, 500),
            400,
            300,
        )
        .expect("the overlapping part is visible");

        assert_eq!(
            region,
            BlendRegion {
                x: 800,
                y: 500,
                width: 100,
                height: 100
            }
        );
    }

    #[test]
    fn a_nested_blend_outside_its_parents_window_plans_nothing() {
        // On the stage, but not on this parent — so nothing of it can appear in this target.
        assert_eq!(
            plan_blend_region(
                rect(100.0, 100.0, 200.0, 200.0),
                &[],
                (1.0, 1.0),
                (800, 500),
                400,
                300
            ),
            None
        );
    }

    #[test]
    fn a_nested_blend_covering_its_parent_gets_that_whole_window() {
        let region = plan_blend_region(
            rect(0.0, 0.0, 4000.0, 4000.0),
            &[],
            (1.0, 1.0),
            (800, 500),
            400,
            300,
        )
        .expect("region");

        assert_eq!(
            region,
            BlendRegion {
                x: 800,
                y: 500,
                width: 400,
                height: 300
            }
        );
        assert!(region.is_full((800, 500), 400, 300));
    }

    #[test]
    fn quantising_snaps_a_region_out_to_the_grid() {
        // 400..480 x 300..360 on a 128 grid becomes 384..512 x 256..384: origin down, far edge up.
        let region = quantise_region(
            BlendRegion {
                x: 400,
                y: 300,
                width: 80,
                height: 60,
            },
            (0, 0),
            2560,
            1440,
            128,
        );

        assert_eq!(
            region,
            BlendRegion {
                x: 384,
                y: 256,
                width: 128,
                height: 128
            }
        );
    }

    #[test]
    fn quantising_only_ever_grows_a_region() {
        // The safety property the whole idea rests on: whatever was inside stays inside.
        for (x, y, w, h) in [
            (400, 300, 80, 60),
            (0, 0, 1, 1),
            (2000, 1000, 500, 400),
            (7, 9, 3, 5),
        ] {
            let exact = BlendRegion {
                x,
                y,
                width: w,
                height: h,
            };
            let snapped = quantise_region(exact, (0, 0), 2560, 1440, 128);

            assert!(snapped.x <= exact.x, "{snapped:?} starts after {exact:?}");
            assert!(snapped.y <= exact.y, "{snapped:?} starts below {exact:?}");
            assert!(
                snapped.x + snapped.width >= exact.x + exact.width,
                "{snapped:?} ends before {exact:?}"
            );
            assert!(
                snapped.y + snapped.height >= exact.y + exact.height,
                "{snapped:?} ends above {exact:?}"
            );
        }
    }

    #[test]
    fn quantising_never_escapes_the_parent() {
        // A region against the right/bottom edge cannot round up past the surface, so it comes back
        // off-grid. That is the one case where sizes stay irregular, and it is unavoidable.
        let region = quantise_region(
            BlendRegion {
                x: 2500,
                y: 1400,
                width: 60,
                height: 40,
            },
            (0, 0),
            2560,
            1440,
            128,
        );

        assert_eq!(region.x + region.width, 2560);
        assert_eq!(region.y + region.height, 1440);
    }

    #[test]
    fn quantising_a_nested_region_snaps_relative_to_its_parents_corner() {
        // Inside a parent at (800, 500) the grid starts there, not at the stage origin, so a nested
        // target lands on the same lattice its siblings do.
        let region = quantise_region(
            BlendRegion {
                x: 900,
                y: 600,
                width: 50,
                height: 50,
            },
            (800, 500),
            400,
            300,
            128,
        );

        // 900..950 is 100..150 from the parent's corner, which straddles the 128 boundary, so it
        // legitimately spans two cells rather than one.
        assert_eq!(
            region,
            BlendRegion {
                x: 800,
                y: 500,
                width: 256,
                height: 256
            }
        );
    }

    #[test]
    fn a_degenerate_or_absent_parent_plans_nothing() {
        assert_eq!(
            plan_blend_region(rect(0.0, 0.0, 10.0, 10.0), &[], (1.0, 1.0), (0, 0), 0, 1440),
            None
        );
        assert_eq!(
            plan_blend_region(rect(0.0, 0.0, 10.0, 10.0), &[], (1.0, 1.0), (0, 0), 2560, 0),
            None
        );
    }
}
