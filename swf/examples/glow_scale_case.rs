//! Write the smallest SWF that shows a glow on a scaled object.
//!
//! Two reports about glows point at object scale and disagree about which way. This builds the case
//! on its own so it can be rendered headlessly and looked at, instead of reasoned about: one white
//! square with a glow, placed at whatever scale is asked for, on a stage large enough that nothing
//! is ever clipped by the stage itself.
//!
//! Usage: `cargo run -p swf --example glow_scale_case -- <scale> <out.swf> [nested]`
//!
//! `nested` wraps the glowing square in a cached sprite and scales the sprite instead of the
//! square, which is how AQW is built: the weapon carries the filter, the character carries the
//! scale, and the character is the thing held as a bitmap.

use std::fs::File;

use swf::{
    BlendMode, Color, CharacterId, Compression, Fixed8, Fixed16, FillStyle, Filter, GlowFilter,
    GlowFilterFlags, GradientFilter, GradientFilterFlags, GradientRecord, Header, Matrix,
    PlaceObject, PlaceObjectAction, Point, PointDelta, Rectangle, Shape, ShapeFlag, ShapeRecord,
    ShapeStyles, Sprite, StyleChangeData, Tag, Twips,
};

/// The square, in twips. Big enough that a 40 pixel glow around it is obvious, small enough that a
/// quarter-scale copy is still clearly a square.
const SQUARE: f64 = 120.0;

/// Deliberately large. Anything cut off in the output is cut off by the renderer, not by the stage.
const STAGE: f64 = 900.0;

/// Enough frames of drift for a reused cache texture to be well clear of the size its contents want.
const FRAMES: u16 = 24;

/// The square, scaled and turned about its own centre, sitting in the middle of the stage whatever
/// those two are. Keeping the centre fixed is what makes two renders comparable: any difference in
/// how far the glow reaches is then a difference in the glow, not in where the square landed.
fn placed(scale: f32, rotation_degrees: f32) -> Matrix {
    let radians = rotation_degrees.to_radians();
    let (sin, cos) = radians.sin_cos();
    let (a, b, c, d) = (scale * cos, scale * sin, -scale * sin, scale * cos);
    let centre = SQUARE as f32 / 2.0;
    let stage_centre = STAGE as f32 / 2.0;
    Matrix {
        a: Fixed16::from_f32(a),
        b: Fixed16::from_f32(b),
        c: Fixed16::from_f32(c),
        d: Fixed16::from_f32(d),
        tx: Twips::from_pixels((stage_centre - (a * centre + c * centre)) as f64),
        ty: Twips::from_pixels((stage_centre - (b * centre + d * centre)) as f64),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let scale: f32 = args
        .next()
        .and_then(|arg| arg.parse().ok())
        .expect("usage: glow_scale_case <scale> <out.swf>");
    let out = args.next().expect("usage: glow_scale_case <scale> <out.swf>");
    let rest: Vec<String> = args.collect();
    let nested = rest.iter().any(|arg| arg == "nested");
    // A cached sprite under a trivial blend reaches the renderer as a blend wrapping exactly one
    // bitmap draw, which is the shape the blend bypass is about.
    let screen = rest.iter().any(|arg| arg == "screen");
    // What AQW actually uses. VoidKnightStar.swf carries no filters at all: the glow on a weapon
    // is blendMode 13, Overlay, which is a complex blend and takes the long way through a target.
    let overlay = rest.iter().any(|arg| arg == "overlay");
    // No filter, so the square stays a shape draw instead of being cached into a bitmap.
    let noglow = rest.iter().any(|arg| arg == "noglow");
    // The blend goes on the child inside the cached sprite rather than on the sprite itself, which
    // is how AQW is built: the weapon blends inside the character, and the character is the bitmap.
    let innerblend = rest.iter().any(|arg| arg == "innerblend");
    // Rotation, in degrees. The in-game camera tool shows a glow that is wrong when the weapon is
    // axis-aligned and clean when it is turned, so rotation has to be a variable here: a turned
    // object has a far larger axis-aligned bounding box, and therefore far more slack around it.
    let rotation: f32 = rest
        .iter()
        .find_map(|arg| arg.strip_prefix("rot:"))
        .and_then(|deg| deg.parse().ok())
        .unwrap_or(0.0);

    let square = Rectangle {
        x_min: Twips::ZERO,
        x_max: Twips::from_pixels(SQUARE),
        y_min: Twips::ZERO,
        y_max: Twips::from_pixels(SQUARE),
    };

    let side = Twips::from_pixels(SQUARE);
    let shape = Tag::DefineShape(Box::new(Shape {
        version: 1,
        id: 1,
        shape_bounds: square,
        edge_bounds: square,
        flags: ShapeFlag::empty(),
        styles: ShapeStyles {
            fill_styles: vec![FillStyle::Color(Color::from_rgba(0xff2090f0))],
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
    }));

    // The filter AQW actually ships on a weapon, copied field for field out of
    // `UltimateGameClaymore.swf`: a gradient glow, two stops, transparent red running to opaque
    // orange, offset by a short distance. This is the one that was never given room to draw.
    let gradient = rest.iter().any(|arg| arg == "gradient");
    let weapon_glow = vec![Filter::GradientGlowFilter(Box::new(GradientFilter {
        colors: vec![
            // Transparent red at the outside of the ramp, opaque orange at the object.
            GradientRecord {
                ratio: 0,
                color: Color::from_rgba(0x00ff0000),
            },
            GradientRecord {
                ratio: 255,
                color: Color::from_rgba(0xffff6600),
            },
        ],
        blur_x: Fixed16::from_f64(17.0),
        blur_y: Fixed16::from_f64(17.0),
        angle: Fixed16::from_f64(4.6425628662109375),
        distance: Fixed16::from_f64(4.0),
        strength: Fixed8::from_f64(1.7890625),
        flags: GradientFilterFlags::COMPOSITE_SOURCE | GradientFilterFlags::from_passes(3),
    }))];

    let glow = vec![Filter::GlowFilter(Box::new(GlowFilter {
        color: Color::from_rgba(0xff00a5ff),
        blur_x: Fixed16::from_f64(40.0),
        blur_y: Fixed16::from_f64(40.0),
        strength: Fixed8::from_f64(2.0),
        flags: GlowFilterFlags::COMPOSITE_SOURCE | GlowFilterFlags::from_passes(3),
    }))];

    let glow = if gradient { weapon_glow } else { glow };

    // Centred, so the glow has room on every side whatever the scale and rotation are.
    let matrix = placed(scale, rotation);

    // The wrapper carries the scale and is held as a bitmap; the square inside it carries the glow
    // at its authored size. Placed at the identity inside, so the only scale in the chain is the
    // wrapper's.
    let sprite = Tag::DefineSprite(Sprite {
        id: 2,
        num_frames: 1,
        tags: vec![
            Tag::PlaceObject(Box::new(PlaceObject {
                version: 3,
                action: PlaceObjectAction::Place(CharacterId::from(1u16)),
                depth: 1,
                matrix: Some(Matrix::IDENTITY),
                color_transform: None,
                ratio: None,
                name: None,
                clip_depth: None,
                class_name: None,
                filters: (!noglow).then(|| glow.clone()),
                background_color: None,
                blend_mode: innerblend.then_some(BlendMode::Overlay.into()),
                clip_actions: None,
                has_image: false,
                is_bitmap_cached: None,
                is_visible: None,
                amf_data: None,
            })),
            Tag::ShowFrame,
        ],
    });

    let place = Tag::PlaceObject(Box::new(PlaceObject {
        version: 3,
        action: PlaceObjectAction::Place(CharacterId::from(if nested { 2u16 } else { 1u16 })),
        depth: 1,
        matrix: Some(matrix),
        color_transform: None,
        ratio: None,
        name: None,
        clip_depth: None,
        class_name: None,
        filters: (!nested && !noglow).then(|| glow.clone()),
        background_color: None,
        blend_mode: if overlay && !innerblend {
            Some(BlendMode::Overlay.into())
        } else {
            screen.then_some(BlendMode::Screen.into())
        },
        clip_actions: None,
        has_image: false,
        is_bitmap_cached: nested.then_some(true),
        is_visible: None,
        amf_data: None,
    }));

    let header = Header {
        compression: Compression::None,
        version: 15,
        stage_size: Rectangle {
            x_min: Twips::ZERO,
            x_max: Twips::from_pixels(STAGE),
            y_min: Twips::ZERO,
            y_max: Twips::from_pixels(STAGE),
        },
        frame_rate: Fixed8::from_f64(24.0),
        num_frames: FRAMES,
    };

    // A mid grey stage, so an Overlay blend has something to act on and anything it does to a
    // pixel it should have left alone is visible.
    let mut tags = vec![Tag::SetBackgroundColor(Color::from_rgba(0xff808080)), shape];
    if nested {
        tags.push(sprite);
    }
    tags.push(place);
    tags.push(Tag::ShowFrame);

    // Then drift, a fraction of a percent a frame. This is the part of AQW that a still cannot
    // show: an animating character never asks for the same cache size twice, which is the whole
    // reason the bounded-reuse policy exists, and a reused texture is by definition larger than the
    // contents drawn into it.
    for frame in 1..FRAMES {
        let drifted = scale * (1.0 + frame as f32 * 0.017);
        let matrix = placed(drifted, rotation);
        tags.push(Tag::PlaceObject(Box::new(PlaceObject {
            version: 3,
            action: PlaceObjectAction::Modify,
            depth: 1,
            matrix: Some(matrix),
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
        tags.push(Tag::ShowFrame);
    }

    let file = File::create(&out).expect("could not create the output file");
    swf::write_swf(&header, &tags, file).expect("could not write the SWF");
    let shape_of_it = if nested { "glow inside a cached sprite scaled" } else { "glow on a square scaled" };
    println!("{out}: {shape_of_it} to {scale}, turned {rotation}deg, 40px glow, {STAGE}px stage");
}
