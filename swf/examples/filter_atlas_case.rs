//! Write a SWF whose objects share a glow, so filter atlasing has something to group.
//!
//! Neither `Game3098r24.swf` nor `blend_corpus.swf` can ever exercise this: atlasing only engages
//! when two or more `cacheAsBitmap` entries in the same frame run the *same* blur kernel, and
//! neither corpus produces that. Without this case the atlas path would ship having never executed.
//!
//! Usage: `cargo run -p swf --example filter_atlas_case -- <count> <out.swf> [mixed] [gradient]`
//!
//! * `count`    how many glowing squares to place. Two or more forms a group.
//! * `mixed`    alternate two different blur radii, so the run splits into two groups instead of
//!              one. This is what proves the grouping is keyed on the kernel rather than on
//!              adjacency -- rendered with and without atlasing it must still match.
//! * `gradient` use the gradient glow AQW actually ships on a weapon, rather than a plain glow.
//!              Gradient glow is the filter that matters in practice, and it reaches the same
//!              shared blur by a different route (`gradient_glow_fallback`), so it is worth being
//!              able to render both.
//!
//! Every square is placed apart from its neighbours so that a bleed between atlas slots shows up as
//! a visible smear rather than hiding inside an overlap.

use std::fs::File;

use swf::{
    CharacterId, Color, Compression, FillStyle, Filter, Fixed8, Fixed16, GlowFilter,
    GlowFilterFlags, GradientFilter, GradientFilterFlags, GradientRecord, Header, Matrix,
    PlaceObject, PlaceObjectAction, Point, PointDelta, Rectangle, Shape, ShapeFlag, ShapeRecord,
    ShapeStyles, StyleChangeData, Tag, Twips,
};

/// The square, in twips. Large enough that a 40 pixel glow around it is unmistakable.
const SQUARE: f64 = 120.0;

/// Space between one square's left edge and the next. Comfortably more than the square plus twice
/// the widest glow, so neighbours never overlap and any bleed is obvious.
const SPACING: f64 = 320.0;

/// Margin around the whole row, so nothing is clipped by the stage.
const MARGIN: f64 = 120.0;

/// Enough frames that a cache entry is rebuilt more than once.
const FRAMES: u16 = 12;

fn square_shape(id: u16, color: u32) -> Tag<'static> {
    let bounds = Rectangle {
        x_min: Twips::ZERO,
        x_max: Twips::from_pixels(SQUARE),
        y_min: Twips::ZERO,
        y_max: Twips::from_pixels(SQUARE),
    };
    let side = Twips::from_pixels(SQUARE);
    Tag::DefineShape(Box::new(Shape {
        version: 1,
        id,
        shape_bounds: bounds,
        edge_bounds: bounds,
        flags: ShapeFlag::empty(),
        styles: ShapeStyles {
            fill_styles: vec![FillStyle::Color(Color::from_rgba(color))],
            line_styles: vec![],
        },
        shape: vec![
            ShapeRecord::StyleChange(Box::new(StyleChangeData {
                move_to: Some(Point::new(Twips::ZERO, Twips::ZERO)),
                fill_style_0: None,
                fill_style_1: Some(1),
                line_style: None,
                new_styles: None,
            })),
            ShapeRecord::StraightEdge {
                delta: PointDelta::new(side, Twips::ZERO),
            },
            ShapeRecord::StraightEdge {
                delta: PointDelta::new(Twips::ZERO, side),
            },
            ShapeRecord::StraightEdge {
                delta: PointDelta::new(-side, Twips::ZERO),
            },
            ShapeRecord::StraightEdge {
                delta: PointDelta::new(Twips::ZERO, -side),
            },
        ],
    }))
}

/// A plain glow at the given radius. Two of these with equal radii share a kernel; with different
/// radii they must not.
fn plain_glow(radius: f64) -> Filter {
    Filter::GlowFilter(Box::new(GlowFilter {
        color: Color::from_rgba(0xff00a5ff),
        blur_x: Fixed16::from_f64(radius),
        blur_y: Fixed16::from_f64(radius),
        strength: Fixed8::from_f64(2.0),
        flags: GlowFilterFlags::COMPOSITE_SOURCE | GlowFilterFlags::from_passes(3),
    }))
}

/// The filter AQW ships on a weapon, copied field for field out of `UltimateGameClaymore.swf`.
fn weapon_gradient_glow(radius: f64) -> Filter {
    Filter::GradientGlowFilter(Box::new(GradientFilter {
        colors: vec![
            GradientRecord {
                ratio: 0,
                color: Color::from_rgba(0x00ff0000),
            },
            GradientRecord {
                ratio: 255,
                color: Color::from_rgba(0xffff6600),
            },
        ],
        blur_x: Fixed16::from_f64(radius),
        blur_y: Fixed16::from_f64(radius),
        angle: Fixed16::from_f64(4.6425628662109375),
        distance: Fixed16::from_f64(4.0),
        strength: Fixed8::from_f64(1.7890625),
        flags: GradientFilterFlags::COMPOSITE_SOURCE | GradientFilterFlags::from_passes(3),
    }))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let count: usize = args
        .next()
        .and_then(|arg| arg.parse().ok())
        .expect("usage: filter_atlas_case <count> <out.swf> [mixed] [gradient]");
    let out = args
        .next()
        .expect("usage: filter_atlas_case <count> <out.swf> [mixed] [gradient]");
    let rest: Vec<String> = args.collect();
    let mixed = rest.iter().any(|arg| arg == "mixed");
    // First half at one radius, second half at another: two runs, so two groups in one frame.
    // `mixed` alternates instead, which forms no group at all -- consecutive entries never match.
    let blocks = rest.iter().any(|arg| arg == "blocks");
    let gradient = rest.iter().any(|arg| arg == "gradient");
    // Alternate each object between two scales every frame. A cache surface is sized from the
    // object's bounds, so this frees one texture and asks for another, then asks for the first size
    // again next frame. That is the only way to exercise the cache texture pool: a static scene
    // allocates once and never gives anything back.
    let churn = rest.iter().any(|arg| arg == "churn");

    let stage_width = MARGIN * 2.0 + SPACING * count.max(1) as f64;
    let stage_height = MARGIN * 2.0 + SQUARE;

    let mut tags = vec![square_shape(1, 0xff2090f0)];

    for index in 0..count {
        // Alternating radii when `mixed`, so the run cannot be one group. 40 and 17 are far enough
        // apart that a slot sized for one would visibly clip the other.
        let radius = if mixed && index % 2 == 1 {
            17.0
        } else if blocks && index >= count / 2 {
            17.0
        } else {
            40.0
        };
        let filter = if gradient {
            weapon_gradient_glow(radius)
        } else {
            plain_glow(radius)
        };

        tags.push(Tag::PlaceObject(Box::new(PlaceObject {
            version: 3,
            action: PlaceObjectAction::Place(CharacterId::from(1u16)),
            depth: 1 + index as u16,
            matrix: Some(Matrix {
                a: Fixed16::ONE,
                b: Fixed16::ZERO,
                c: Fixed16::ZERO,
                d: Fixed16::ONE,
                tx: Twips::from_pixels(MARGIN + SPACING * index as f64),
                ty: Twips::from_pixels(MARGIN),
            }),
            color_transform: None,
            ratio: None,
            name: None,
            clip_depth: None,
            class_name: None,
            filters: Some(vec![filter]),
            background_color: None,
            blend_mode: None,
            clip_actions: None,
            has_image: false,
            is_bitmap_cached: None,
            is_visible: None,
            amf_data: None,
        })));
    }

    for frame in 0..FRAMES {
        if churn && frame > 0 {
            // Two scales far enough apart to land in different cache-texture grid cells, so the
            // surface really is reallocated rather than reused in place.
            let scale = if frame % 2 == 1 { 2.0f32 } else { 1.0f32 };
            for index in 0..count {
                tags.push(Tag::PlaceObject(Box::new(PlaceObject {
                    version: 3,
                    action: PlaceObjectAction::Modify,
                    depth: 1 + index as u16,
                    matrix: Some(Matrix {
                        a: Fixed16::from_f32(scale),
                        b: Fixed16::ZERO,
                        c: Fixed16::ZERO,
                        d: Fixed16::from_f32(scale),
                        tx: Twips::from_pixels(MARGIN + SPACING * index as f64),
                        ty: Twips::from_pixels(MARGIN),
                    }),
                    color_transform: None,
                    ratio: None,
                    name: None,
                    clip_depth: None,
                    class_name: None,
                    filters: None,
                    background_color: None,
                    blend_mode: None,
                    clip_actions: None,
                    has_image: false,
                    is_bitmap_cached: None,
                    is_visible: None,
                    amf_data: None,
                })));
            }
        }
        tags.push(Tag::ShowFrame);
    }

    let header = Header {
        compression: Compression::None,
        version: 15,
        stage_size: Rectangle {
            x_min: Twips::ZERO,
            x_max: Twips::from_pixels(stage_width),
            y_min: Twips::ZERO,
            y_max: Twips::from_pixels(stage_height),
        },
        frame_rate: Fixed8::from_f64(24.0),
        num_frames: FRAMES,
    };

    let file = File::create(&out).expect("the output path must be writable");
    swf::write_swf(&header, &tags, file).expect("the case must be writable as a SWF");
    println!(
        "wrote {out}: {count} squares, {}, {}",
        if mixed {
            "two radii (should form two groups)"
        } else {
            "one radius (should form one group)"
        },
        if gradient {
            "gradient glow"
        } else {
            "plain glow"
        }
    );
}
