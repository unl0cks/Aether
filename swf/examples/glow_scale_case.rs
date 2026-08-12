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
    GlowFilterFlags, Header, Matrix, PlaceObject, PlaceObjectAction, Point, PointDelta, Rectangle,
    Shape, ShapeFlag, ShapeRecord, ShapeStyles, Sprite, StyleChangeData, Tag, Twips,
};

/// The square, in twips. Big enough that a 40 pixel glow around it is obvious, small enough that a
/// quarter-scale copy is still clearly a square.
const SQUARE: f64 = 120.0;

/// Deliberately large. Anything cut off in the output is cut off by the renderer, not by the stage.
const STAGE: f64 = 900.0;

/// Enough frames of drift for a reused cache texture to be well clear of the size its contents want.
const FRAMES: u16 = 24;

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

    let glow = vec![Filter::GlowFilter(Box::new(GlowFilter {
        color: Color::from_rgba(0xff00a5ff),
        blur_x: Fixed16::from_f64(40.0),
        blur_y: Fixed16::from_f64(40.0),
        strength: Fixed8::from_f64(2.0),
        flags: GlowFilterFlags::COMPOSITE_SOURCE | GlowFilterFlags::from_passes(3),
    }))];

    // Centred, so the glow has room on every side whatever the scale is.
    let drawn = SQUARE as f32 * scale;
    let offset = Twips::from_pixels(((STAGE as f32 - drawn) / 2.0) as f64);
    let mut matrix = Matrix::scale(Fixed16::from_f32(scale), Fixed16::from_f32(scale));
    matrix.tx = offset;
    matrix.ty = offset;

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
                filters: Some(glow.clone()),
                background_color: None,
                blend_mode: None,
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
        filters: (!nested).then(|| glow.clone()),
        background_color: None,
        blend_mode: if overlay {
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
        let drawn = SQUARE as f32 * drifted;
        let offset = Twips::from_pixels(((STAGE as f32 - drawn) / 2.0) as f64);
        let mut matrix = Matrix::scale(Fixed16::from_f32(drifted), Fixed16::from_f32(drifted));
        matrix.tx = offset;
        matrix.ty = offset;
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
    println!("{out}: {shape_of_it} to {scale}, 40px glow, {STAGE}px stage");
}
