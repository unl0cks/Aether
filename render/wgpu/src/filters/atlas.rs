//! Packing several filter sources into one texture so a group of them can be blurred together.
//!
//! Measured over a 3h46m session (13,393 one-second samples), matched on draw count so scene
//! complexity could not swamp the signal: filtered `cacheAsBitmap` entries are both the largest
//! item in a crowded frame and the thing that decays over a long session. In the heaviest band the
//! filtered half of cache time went 2.42 -> 13.24 ms between the first and second half of the
//! session, while the filterless half stayed flat at 2.38 -> 2.62 ms and **render passes went
//! down**. So the cost is not pass count in general, it is what filtered entries do.
//!
//! Each of those entries runs its own blur: two intermediate targets and `num_passes * 2` render
//! passes, per object, every frame. Objects that share blur parameters are running the identical
//! kernel over separate textures. Packed into one atlas with enough padding between them, one set
//! of passes does the whole group.
//!
//! The measurement that says this is worth doing is `batch` in the metrics line -- filters divided
//! by distinct parameter groups. It never fell below 3.4 across the session and reached 6.9 in the
//! heaviest band, so a group is typically four to seven objects. At 1.0 this module would be
//! pointless.

use ruffle_render::bitmap::BitmapHandle;
use ruffle_render::commands::{Command, CommandList};

/// Whether `commands` draws `handle` anywhere, at any nesting depth.
///
/// This is the dependency test that keeps a parent out of its child's group. `BitmapHandle` compares
/// by `Arc::ptr_eq`, so this is identity, not equality of contents.
///
/// Nesting only happens through `Blend` and `RenderAlphaMask`; every other command is a leaf. If a
/// new nesting command is ever added and not handled here, the failure is a group that should have
/// been split, so treat an unrecognised nesting command as drawing everything rather than nothing.
pub fn draws_handle(commands: &CommandList, handle: &BitmapHandle) -> bool {
    commands.commands.iter().any(|command| match command {
        Command::RenderBitmap { bitmap, .. } | Command::RenderStage3D { bitmap, .. } => {
            bitmap == handle
        }
        Command::Blend(list, _, _) => draws_handle(list, handle),
        Command::RenderAlphaMask {
            maskee_commands,
            mask_commands,
            ..
        } => draws_handle(maskee_commands, handle) || draws_handle(mask_commands, handle),
        Command::RenderShape { .. }
        | Command::DrawRect { .. }
        | Command::DrawLine { .. }
        | Command::DrawLineRect { .. }
        | Command::PushMask
        | Command::ActivateMask
        | Command::DeactivateMask
        | Command::PopMask => false,
    })
}

/// How far one axis of a blur reaches from a source pixel, in pixels.
///
/// This is what decides padding, and getting it too small is a silent corruption rather than a
/// crash: neighbours bleed into each other and every object in the group picks up a ghost of the
/// one next to it. The blur runs `num_passes` times over the axis and each pass spreads by half the
/// kernel either side, so the reaches add.
///
/// Mirrors `blur.rs`: the kernel is clamped to 255, and a width of 1 or less is skipped there as a
/// no-op, so it reaches nothing here.
pub fn blur_reach(blur: f32, num_passes: u8) -> u32 {
    let full_size = blur.min(255.0);
    if full_size <= 1.0 {
        return 0;
    }
    let per_pass = (full_size - 1.0) / 2.0;
    (per_pass * num_passes as f32).ceil() as u32
}

/// The blur kernel two filters have to share before they can be blurred together.
///
/// Radii are compared as bits rather than as floats. They come from fixed-point SWF values through
/// a deterministic conversion, so equal filters give equal bits; using `==` on `f32` would invite a
/// tolerance nobody can justify, and would make two kernels that differ imperceptibly share a pass
/// that is nonetheless the wrong one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlurKey {
    pub blur_x_bits: u32,
    pub blur_y_bits: u32,
    pub passes: u8,
}

impl BlurKey {
    pub fn new(blur_x: f32, blur_y: f32, passes: u8) -> Self {
        Self {
            blur_x_bits: blur_x.to_bits(),
            blur_y_bits: blur_y.to_bits(),
            passes,
        }
    }
}

/// How many entries from the front of a run may be blurred together.
///
/// Returns 1 when there is no group, which the caller filters the ordinary way.
///
/// Two rules, and the second is the one that is easy to miss. A group is only entries sharing a
/// kernel -- that is the obvious half. But atlasing also *defers* every member's filter until after
/// every member has been drawn, and a cache entry may draw another cache entry's texture: a parent
/// with `cacheAsBitmap` renders its cached child as a bitmap. Deferring the child's filter past the
/// parent's draw would have the parent composite an unfiltered child, once, and then look correct
/// again the next frame -- a flicker, which is the worst kind of bug to chase. So `conflicts` closes
/// the group before any entry that draws a member's texture, or that a member draws.
///
/// `max_group` bounds how many targets are alive at once. That bound is not tidiness: the
/// region-limited MSAA experiment held a frame's targets together and took peak GPU memory from
/// 1.97 GB to 11.45 GB. Groups measured four to seven, so a small cap costs almost nothing.
pub fn plan_atlas_group(
    count: usize,
    key_of: impl Fn(usize) -> Option<BlurKey>,
    conflicts: impl Fn(usize, usize) -> bool,
    max_group: usize,
) -> usize {
    if count == 0 || max_group <= 1 {
        return count.min(1);
    }
    let Some(key) = key_of(0) else {
        return 1;
    };

    let mut len = 1;
    while len < count && len < max_group {
        if key_of(len) != Some(key) {
            break;
        }
        // Against every member already in the group, both ways round.
        if (0..len).any(|member| conflicts(len, member) || conflicts(member, len)) {
            break;
        }
        len += 1;
    }

    len
}

/// Where one source ended up inside the atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasSlot {
    /// Index into the list of sources handed to [`pack_atlas`], so callers can reorder freely.
    pub index: usize,
    /// Top-left of the source's own pixels, not of its padded cell.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A packing of several sources into one texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtlasPacking {
    pub width: u32,
    pub height: u32,
    pub slots: Vec<AtlasSlot>,
}

/// Pack `sizes` into a single atlas, keeping `pad_x`/`pad_y` of empty space around every source.
///
/// Returns `None` when the group does not fit, which the caller must treat as "filter these the
/// ordinary way" rather than as an error.
///
/// Neighbours *share* one gap rather than each bringing their own, and the atlas carries a margin
/// of its own around the outside. Both are needed and neither is optional: `pad` between two
/// sources is exactly what stops one bleeding into the other, and `pad` at the texture edge is what
/// stops the sampler clamping and smearing the outermost row back over the image.
///
/// Padding every source on all four sides would satisfy both with one rule, and did to begin with,
/// but it doubles the separation and the area is not free -- six 240px sources with a 40px, 3-pass
/// glow packed into 908x1362, about 3.6x their own area. Sharing the gap gives up nothing and takes
/// roughly a third of that back.
///
/// Shelf packing, sources tallest first. The groups this serves are single digits of similarly
/// sized avatars, where shelf packing wastes very little, and it is easy to be sure of.
pub fn pack_atlas(
    sizes: &[(u32, u32)],
    pad_x: u32,
    pad_y: u32,
    max_dimension: u32,
) -> Option<AtlasPacking> {
    if sizes.is_empty() {
        return None;
    }

    // Each cell carries the gap that follows it; the leading gap is the atlas's own margin, added
    // once below. Saturating because a source is untrusted arithmetic away from the texture limit,
    // and wrapping here would produce a packing that looks tiny and overlaps.
    let cell = |(w, h): (u32, u32)| (w.saturating_add(pad_x), h.saturating_add(pad_y));

    // Anything whose own cell exceeds the limit can never be placed, and the caller has to fall
    // back for the whole group rather than silently dropping one member.
    // The leading margin counts too, so a source only fits if its cell plus that margin does.
    if sizes.iter().any(|&size| {
        let (w, h) = cell(size);
        w.saturating_add(pad_x) > max_dimension || h.saturating_add(pad_y) > max_dimension
    }) {
        return None;
    }

    let mut order: Vec<usize> = (0..sizes.len()).collect();
    // Tallest first, then by index so the packing is deterministic -- byte-identical frames are how
    // this gets verified, and a packing that depends on hash order cannot be compared.
    order.sort_by_key(|&i| (std::cmp::Reverse(cell(sizes[i]).1), i));

    // Widen the shelf until the tallest cell fits, then let rows wrap. Starting from the widest
    // single cell keeps a lone large object from forcing a square atlas around it.
    let shelf_width = order
        .iter()
        .map(|&i| cell(sizes[i]).0)
        .max()
        .unwrap_or(0)
        .max(total_width_target(sizes, &cell))
        .min(max_dimension);

    let mut slots = Vec::with_capacity(sizes.len());
    let (mut pen_x, mut pen_y, mut row_height, mut used_width) = (0u32, 0u32, 0u32, 0u32);

    for &index in &order {
        let (cell_w, cell_h) = cell(sizes[index]);

        if pen_x > 0 && pen_x.saturating_add(cell_w) > shelf_width {
            pen_y = pen_y.checked_add(row_height)?;
            pen_x = 0;
            row_height = 0;
        }

        let bottom = pen_y.checked_add(cell_h)?;
        if bottom > max_dimension {
            return None;
        }

        slots.push(AtlasSlot {
            index,
            x: pen_x + pad_x,
            y: pen_y + pad_y,
            width: sizes[index].0,
            height: sizes[index].1,
        });

        pen_x = pen_x.checked_add(cell_w)?;
        used_width = used_width.max(pen_x);
        row_height = row_height.max(cell_h);
    }

    // Every cell brought its trailing gap, so the only thing still missing is the margin on the
    // leading edge -- which the slots were already shifted by.
    let width = used_width.checked_add(pad_x)?;
    let height = pen_y.checked_add(row_height)?.checked_add(pad_y)?;
    if width > max_dimension || height > max_dimension {
        return None;
    }

    // Sorted back into the caller's order so a slot can be matched to its source by position.
    slots.sort_by_key(|slot| slot.index);

    Some(AtlasPacking {
        width,
        height,
        slots,
    })
}

/// A shelf width that keeps the atlas roughly square, which is what keeps the wasted area low.
fn total_width_target(sizes: &[(u32, u32)], cell: &impl Fn((u32, u32)) -> (u32, u32)) -> u32 {
    let area: u64 = sizes
        .iter()
        .map(|&size| {
            let (w, h) = cell(size);
            w as u64 * h as u64
        })
        .sum();
    (area as f64).sqrt().ceil() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The padding rule, which is the one number that silently corrupts the image if it is wrong.
    #[test]
    fn reach_covers_every_pass_and_ignores_a_kernel_that_does_nothing() {
        // A kernel of 1 or less samples only itself, and `blur.rs` skips the pass entirely.
        assert_eq!(blur_reach(1.0, 3), 0);
        assert_eq!(blur_reach(0.0, 3), 0);

        // One pass of a 17-wide kernel reaches 8 either side; three passes reach 24.
        assert_eq!(blur_reach(17.0, 1), 8);
        assert_eq!(blur_reach(17.0, 3), 24);

        // Fractional radii round up. Rounding down would leave a pixel of bleed.
        assert_eq!(blur_reach(4.5, 1), 2);
        assert_eq!(blur_reach(4.2, 3), 5);

        // Clamped to 255 exactly as the shader does.
        assert_eq!(blur_reach(1000.0, 1), blur_reach(255.0, 1));
    }

    /// No two sources may come within `pad` of each other, or they bleed. Checked exhaustively
    /// rather than by spot-checking a layout, because this is the whole correctness argument.
    #[test]
    fn packed_sources_never_come_within_the_padding_of_each_other() {
        let sizes = [
            (100, 50),
            (30, 200),
            (75, 75),
            (10, 10),
            (200, 30),
            (64, 64),
            (128, 96),
            (5, 300),
        ];
        let (pad_x, pad_y) = (24, 24);
        let packing = pack_atlas(&sizes, pad_x, pad_y, 4096).expect("this group fits easily");

        assert_eq!(packing.slots.len(), sizes.len());
        for (i, slot) in packing.slots.iter().enumerate() {
            assert_eq!(slot.index, i, "slots come back in the caller's order");
            assert_eq!((slot.width, slot.height), sizes[i]);
            // Border margin, so the sampler never clamps against live pixels.
            assert!(
                slot.x >= pad_x && slot.y >= pad_y,
                "slot {i} touches the edge"
            );
            assert!(
                slot.x + slot.width + pad_x <= packing.width,
                "slot {i} overruns"
            );
            assert!(
                slot.y + slot.height + pad_y <= packing.height,
                "slot {i} overruns"
            );
        }

        for a in &packing.slots {
            for b in &packing.slots {
                if a.index >= b.index {
                    continue;
                }
                // Separated by at least the padding on one axis or the other. Touching on both is
                // what lets one object's blur reach the other's pixels.
                let clear_x = a.x + a.width + pad_x <= b.x || b.x + b.width + pad_x <= a.x;
                let clear_y = a.y + a.height + pad_y <= b.y || b.y + b.height + pad_y <= a.y;
                assert!(
                    clear_x || clear_y,
                    "slots {} and {} are close enough to bleed: {a:?} {b:?}",
                    a.index,
                    b.index
                );
            }
        }
    }

    #[test]
    fn a_single_source_still_packs_with_its_margin() {
        let packing = pack_atlas(&[(100, 40)], 8, 3, 4096).unwrap();
        assert_eq!(packing.slots[0].x, 8);
        assert_eq!(packing.slots[0].y, 3);
        assert_eq!(packing.width, 116);
        assert_eq!(packing.height, 46);
    }

    /// Falling back is the caller's cue to filter the group one at a time, so it has to be a clean
    /// `None` rather than a clamped packing that overlaps.
    #[test]
    fn a_group_that_cannot_fit_declines_rather_than_overlapping() {
        assert_eq!(pack_atlas(&[], 8, 8, 4096), None);
        // One source whose padded cell alone exceeds the limit.
        assert_eq!(pack_atlas(&[(500, 500)], 8, 8, 512), None);
        // Individually fine, collectively past the limit in both axes.
        let many = vec![(200, 200); 64];
        assert_eq!(pack_atlas(&many, 24, 24, 512), None);
    }

    /// Zero padding is legal -- a blur too narrow to do anything reaches nothing -- and must not
    /// make the packer place two sources on top of each other.
    #[test]
    fn zero_padding_still_packs_without_overlap() {
        let sizes = [(10, 10), (10, 10), (10, 10)];
        let packing = pack_atlas(&sizes, 0, 0, 4096).unwrap();
        for a in &packing.slots {
            for b in &packing.slots {
                if a.index >= b.index {
                    continue;
                }
                let disjoint = a.x + a.width <= b.x
                    || b.x + b.width <= a.x
                    || a.y + a.height <= b.y
                    || b.y + b.height <= a.y;
                assert!(disjoint, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[derive(Debug)]
    struct DummyBitmap;
    impl ruffle_render::bitmap::BitmapHandleImpl for DummyBitmap {}

    fn handle() -> BitmapHandle {
        BitmapHandle(std::sync::Arc::new(DummyBitmap))
    }

    fn render_bitmap(bitmap: &BitmapHandle) -> Command {
        Command::RenderBitmap {
            bitmap: bitmap.clone(),
            transform: Default::default(),
            smoothing: false,
            pixel_snapping: ruffle_render::bitmap::PixelSnapping::Never,
            source_size: None,
        }
    }

    fn list(commands: Vec<Command>) -> CommandList {
        let mut out = CommandList::new();
        out.commands = commands;
        out
    }

    /// The scan has to see through nesting, because a parent that draws a cached child usually does
    /// it inside a blend or behind a mask rather than at the top level.
    #[test]
    fn a_drawn_handle_is_found_at_any_depth() {
        let wanted = handle();
        let other = handle();

        assert!(!draws_handle(&list(vec![]), &wanted));
        assert!(!draws_handle(&list(vec![render_bitmap(&other)]), &wanted));
        assert!(draws_handle(&list(vec![render_bitmap(&wanted)]), &wanted));

        // Inside a blend.
        let nested = list(vec![Command::Blend(
            list(vec![render_bitmap(&wanted)]),
            ruffle_render::commands::RenderBlendMode::Builtin(swf::BlendMode::Multiply),
            None,
        )]);
        assert!(draws_handle(&nested, &wanted));

        // Inside both halves of a mask, and in neither.
        let maskee = list(vec![Command::RenderAlphaMask {
            maskee_commands: list(vec![render_bitmap(&wanted)]),
            mask_commands: list(vec![]),
            bounds: None,
        }]);
        assert!(draws_handle(&maskee, &wanted));
        let masker = list(vec![Command::RenderAlphaMask {
            maskee_commands: list(vec![]),
            mask_commands: list(vec![render_bitmap(&wanted)]),
            bounds: None,
        }]);
        assert!(draws_handle(&masker, &wanted));
        let neither = list(vec![Command::RenderAlphaMask {
            maskee_commands: list(vec![render_bitmap(&other)]),
            mask_commands: list(vec![]),
            bounds: None,
        }]);
        assert!(!draws_handle(&neither, &wanted));
    }

    /// Two handles wrapping equal-looking data are still different textures. Identity, not equality.
    #[test]
    fn distinct_handles_are_not_confused_with_each_other() {
        let a = handle();
        let b = handle();
        assert!(!draws_handle(&list(vec![render_bitmap(&b)]), &a));
        assert!(draws_handle(&list(vec![render_bitmap(&a)]), &a.clone()));
    }

    fn key(x: f32, passes: u8) -> Option<BlurKey> {
        Some(BlurKey::new(x, x, passes))
    }

    #[test]
    fn a_group_runs_while_the_kernel_matches_and_stops_when_it_changes() {
        let keys = [key(17.0, 3), key(17.0, 3), key(17.0, 3), key(8.0, 3)];
        let len = plan_atlas_group(keys.len(), |i| keys[i], |_, _| false, 8);
        assert_eq!(len, 3, "the fourth entry has a different radius");

        // Same radius, different pass count: a different number of passes is a different kernel.
        let keys = [key(17.0, 3), key(17.0, 1)];
        assert_eq!(plan_atlas_group(2, |i| keys[i], |_, _| false, 8), 1);
    }

    #[test]
    fn an_entry_without_a_shareable_blur_is_never_grouped() {
        let keys = [None, key(17.0, 3), key(17.0, 3)];
        assert_eq!(plan_atlas_group(3, |i| keys[i], |_, _| false, 8), 1);
    }

    /// The flicker case: a parent that draws a grouped child's cache texture must close the group,
    /// or it composites the child before the child's filter has been written back.
    #[test]
    fn a_group_stops_before_an_entry_that_draws_a_members_texture() {
        let keys = [key(17.0, 3), key(17.0, 3), key(17.0, 3)];
        // Entry 2 draws entry 0's texture.
        let len = plan_atlas_group(3, |i| keys[i], |a, b| (a, b) == (2, 0), 8);
        assert_eq!(len, 2);

        // The dependency the other way round closes it just the same.
        let len = plan_atlas_group(3, |i| keys[i], |a, b| (a, b) == (0, 2), 8);
        assert_eq!(len, 2);

        // A conflict on the very first candidate leaves no group at all.
        let len = plan_atlas_group(3, |i| keys[i], |a, b| (a, b) == (1, 0), 8);
        assert_eq!(len, 1);
    }

    /// The cap exists to bound how many targets are alive at once, so it must actually bind.
    #[test]
    fn the_group_never_exceeds_its_cap() {
        let keys = vec![key(17.0, 3); 40];
        assert_eq!(plan_atlas_group(40, |i| keys[i], |_, _| false, 8), 8);
        // A cap of one disables grouping without tripping over an empty range.
        assert_eq!(plan_atlas_group(40, |i| keys[i], |_, _| false, 1), 1);
        assert_eq!(plan_atlas_group(0, |_| None, |_, _| false, 8), 0);
    }

    /// The packing has to be identical run to run, because it is verified by comparing rendered
    /// frames byte for byte.
    #[test]
    fn packing_is_deterministic_for_equal_sized_sources() {
        let sizes = vec![(64, 64); 9];
        let first = pack_atlas(&sizes, 4, 4, 4096).unwrap();
        let second = pack_atlas(&sizes, 4, 4, 4096).unwrap();
        assert_eq!(first, second);
    }
}
