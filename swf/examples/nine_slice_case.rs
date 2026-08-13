//! Write the smallest SWF that shows what a scaling grid is for.
//!
//! A bordered box: a coloured frame with a different colour inside it. Scaled up without a grid,
//! the border thickens with everything else, which is what turns a rounded corner into an ellipse.
//! With one, the border keeps the thickness it was drawn at and only the middle grows.
//!
//! Usage: `cargo run -p swf --example nine_slice_case -- <scale> <out.swf> [nogrid]`

use std::fs::File;

use swf::{
    CharacterId, Color, Compression, FillStyle, Fixed8, Fixed16, Header, Matrix, PlaceObject,
    PlaceObjectAction, Point, PointDelta, Rectangle, Shape, ShapeFlag, ShapeRecord, ShapeStyles,
    Sprite, StyleChangeData, Tag, Twips,
};

const BOX: f64 = 100.0;
const BORDER: f64 = 12.0;
const STAGE: f64 = 700.0;

/// A filled rectangle as a single shape record run.
fn rect(x: f64, y: f64, w: f64, h: f64, style: u32) -> Vec<ShapeRecord> {
    vec![
        ShapeRecord::StyleChange(Box::new(StyleChangeData {
            move_to: Some(Point::new(Twips::from_pixels(x), Twips::from_pixels(y))),
            fill_style_0: None,
            fill_style_1: Some(style),
            line_style: None,
            new_styles: None,
        })),
        ShapeRecord::StraightEdge {
            delta: PointDelta::new(Twips::from_pixels(w), Twips::ZERO),
        },
        ShapeRecord::StraightEdge {
            delta: PointDelta::new(Twips::ZERO, Twips::from_pixels(h)),
        },
        ShapeRecord::StraightEdge {
            delta: PointDelta::new(Twips::from_pixels(-w), Twips::ZERO),
        },
        ShapeRecord::StraightEdge {
            delta: PointDelta::new(Twips::ZERO, Twips::from_pixels(-h)),
        },
    ]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let scale: f32 = args
        .next()
        .and_then(|arg| arg.parse().ok())
        .expect("usage: nine_slice_case <scale> <out.swf> [nogrid]");
    let out = args.next().expect("usage: nine_slice_case <scale> <out.swf>");
    let rest: Vec<String> = args.collect();
    let no_grid = rest.iter().any(|arg| arg == "nogrid");
    // `cached` is the case that mattered: a cached object's draw replays a finished image and reads
    // only the translation off the transform stack, so slicing one moves it instead of slicing it.
    let cached = rest.iter().any(|arg| arg == "cached");
    // How far the art sits above and left of the sprite's own origin.
    //
    // A cell's transform translates by `low * (1 - 1/scale)`, where `low` is the near edge of the
    // object's bounds. Art drawn from the origin outwards makes that zero and hides every fault in
    // the translation; a panel centred on its origin -- which is how AQW builds them -- does not.
    // A non-art child inside the grid'd sprite: a nested sprite, which is what a caption, a stack
    // count or an icon amounts to. Slicing must decline when one is present.
    let label = rest.iter().any(|arg| arg == "label");
    // A positioned child that reaches past the frame it sits in, which is what an oversized icon
    // does to a toolbar button. It must not drag the measured edge out and move the bands.
    let overflow = rest.iter().any(|arg| arg == "overflow");
    let origin = rest
        .iter()
        .find_map(|arg| arg.strip_prefix("offset:"))
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0);

    let bounds = Rectangle {
        x_min: Twips::ZERO,
        x_max: Twips::from_pixels(BOX),
        y_min: Twips::ZERO,
        y_max: Twips::from_pixels(BOX),
    };

    // Red frame under a blue middle, as two shapes on two depths. Overlapping fills inside one
    // shape do not layer, so the frame has to be a separate object to be visible at all.
    let inner = BOX - BORDER * 2.0;
    let inner_bounds = Rectangle {
        x_min: Twips::ZERO,
        x_max: Twips::from_pixels(inner),
        y_min: Twips::ZERO,
        y_max: Twips::from_pixels(inner),
    };

    let frame = Tag::DefineShape(Box::new(Shape {
        version: 1,
        id: 1,
        shape_bounds: bounds,
        edge_bounds: bounds,
        flags: ShapeFlag::empty(),
        styles: ShapeStyles {
            fill_styles: vec![FillStyle::Color(Color::from_rgba(0xffe02020))],
            line_styles: vec![],
        },
        shape: rect(0.0, 0.0, BOX, BOX, 1),
    }));

    let middle = Tag::DefineShape(Box::new(Shape {
        version: 1,
        id: 3,
        shape_bounds: inner_bounds,
        edge_bounds: inner_bounds,
        flags: ShapeFlag::empty(),
        styles: ShapeStyles {
            fill_styles: vec![FillStyle::Color(Color::from_rgba(0xff2060e0))],
            line_styles: vec![],
        },
        shape: rect(0.0, 0.0, inner, inner, 1),
    }));

    let label_shape_bounds = Rectangle {
        x_min: Twips::ZERO,
        x_max: Twips::from_pixels(8.0),
        y_min: Twips::ZERO,
        y_max: Twips::from_pixels(8.0),
    };
    let label_shape = Tag::DefineShape(Box::new(Shape {
        version: 1,
        id: 5,
        shape_bounds: label_shape_bounds,
        edge_bounds: label_shape_bounds,
        flags: ShapeFlag::empty(),
        styles: ShapeStyles {
            fill_styles: vec![FillStyle::Color(Color::from_rgba(0xff20e020))],
            line_styles: vec![],
        },
        shape: rect(0.0, 0.0, 8.0, 8.0, 1),
    }));
    let label_sprite = Tag::DefineSprite(Sprite {
        id: 4,
        num_frames: 1,
        tags: vec![
            Tag::PlaceObject(Box::new(PlaceObject {
                version: 2,
                action: PlaceObjectAction::Place(CharacterId::from(5u16)),
                depth: 1,
                matrix: Some(Matrix::IDENTITY),
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
            })),
            Tag::ShowFrame,
        ],
    });

    // The grid goes on a sprite, because that is what a scaling grid can be set on.
    let mut inner_tags = vec![
            Tag::PlaceObject(Box::new(PlaceObject {
                version: 2,
                action: PlaceObjectAction::Place(CharacterId::from(1u16)),
                depth: 1,
                matrix: Some(Matrix::translate(
                    Twips::from_pixels(-origin),
                    Twips::from_pixels(-origin),
                )),
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
            })),
            Tag::PlaceObject(Box::new(PlaceObject {
                version: 2,
                action: PlaceObjectAction::Place(CharacterId::from(3u16)),
                depth: 2,
                matrix: Some(Matrix::translate(
                    Twips::from_pixels(BORDER - origin),
                    Twips::from_pixels(BORDER - origin),
                )),
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
            })),
    ];
    if label {
        inner_tags.push(Tag::PlaceObject(Box::new(PlaceObject {
            version: 2,
            action: PlaceObjectAction::Place(CharacterId::from(4u16)),
            depth: 3,
            matrix: Some(Matrix::translate(
                Twips::from_pixels(if overflow { BOX + 6.0 } else { BORDER * 2.0 } - origin),
                Twips::from_pixels(if overflow { BOX + 6.0 } else { BORDER * 2.0 } - origin),
            )),
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
    inner_tags.push(Tag::ShowFrame);

    let sprite = Tag::DefineSprite(Sprite {
        id: 2,
        num_frames: 1,
        tags: inner_tags,
    });

    let mut matrix = Matrix::scale(Fixed16::from_f32(scale), Fixed16::from_f32(scale));
    matrix.tx = Twips::from_pixels(60.0 + origin * f64::from(scale));
    matrix.ty = Twips::from_pixels(60.0 + origin * f64::from(scale));

    let mut tags = vec![
        Tag::SetBackgroundColor(Color::from_rgba(0xff808080)),
        frame,
        middle,
        label_shape,
        label_sprite,
        sprite,
    ];
    if !no_grid {
        tags.push(Tag::DefineScalingGrid {
            id: CharacterId::from(2u16),
            splitter_rect: Rectangle {
                x_min: Twips::from_pixels(BORDER - origin),
                x_max: Twips::from_pixels(BOX - BORDER - origin),
                y_min: Twips::from_pixels(BORDER - origin),
                y_max: Twips::from_pixels(BOX - BORDER - origin),
            },
        });
    }
    tags.push(Tag::PlaceObject(Box::new(PlaceObject {
        // `cacheAsBitmap` is a PlaceObject3 field. At version 2 the writer drops it without
        // complaint, and the case renders as though it had never been asked for.
        version: if cached { 3 } else { 2 },
        action: PlaceObjectAction::Place(CharacterId::from(2u16)),
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
        is_bitmap_cached: cached.then_some(true),
        is_visible: None,
        amf_data: None,
    })));
    tags.push(Tag::ShowFrame);

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
        num_frames: 1,
    };

    let file = File::create(&out).expect("could not create the output movie");
    swf::write_swf(&header, &tags, file).expect("could not write the output movie");
    println!(
        "{out}: {BORDER}px border on a {BOX}px box at {scale}x, grid {}, cacheAsBitmap {}, origin offset {origin}",
        if no_grid { "off" } else { "on" },
        if cached { "on" } else { "off" }
    );
}
