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
    let no_grid = args.any(|arg| arg == "nogrid");

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

    // The grid goes on a sprite, because that is what a scaling grid can be set on.
    let sprite = Tag::DefineSprite(Sprite {
        id: 2,
        num_frames: 1,
        tags: vec![
            Tag::PlaceObject(Box::new(PlaceObject {
                version: 2,
                action: PlaceObjectAction::Place(CharacterId::from(1u16)),
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
            Tag::PlaceObject(Box::new(PlaceObject {
                version: 2,
                action: PlaceObjectAction::Place(CharacterId::from(3u16)),
                depth: 2,
                matrix: Some(Matrix::translate(
                    Twips::from_pixels(BORDER),
                    Twips::from_pixels(BORDER),
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
            Tag::ShowFrame,
        ],
    });

    let mut matrix = Matrix::scale(Fixed16::from_f32(scale), Fixed16::from_f32(scale));
    matrix.tx = Twips::from_pixels(60.0);
    matrix.ty = Twips::from_pixels(60.0);

    let mut tags = vec![
        Tag::SetBackgroundColor(Color::from_rgba(0xff808080)),
        frame,
        middle,
        sprite,
    ];
    if !no_grid {
        tags.push(Tag::DefineScalingGrid {
            id: CharacterId::from(2u16),
            splitter_rect: Rectangle {
                x_min: Twips::from_pixels(BORDER),
                x_max: Twips::from_pixels(BOX - BORDER),
                y_min: Twips::from_pixels(BORDER),
                y_max: Twips::from_pixels(BOX - BORDER),
            },
        });
    }
    tags.push(Tag::PlaceObject(Box::new(PlaceObject {
        version: 2,
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
        is_bitmap_cached: None,
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
        "{out}: {BORDER}px border on a {BOX}px box at {scale}x, grid {}",
        if no_grid { "off" } else { "on" }
    );
}
