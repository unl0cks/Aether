//! Measure the drawn box in a rendered nine-slice case: where it starts, how big it is, and how
//! thick its border came out.
//!
//! The case is a red frame around a blue middle on a grey stage. Sampling the middle row rather than
//! the top one matters: every row of the box is the same width, so "the widest row" is satisfied by
//! row zero, which is all border and says nothing about the border's thickness.
//!
//! Usage: `cargo run -p exporter --example measure_box -- <image.png> [more.png ...]`

use image::GenericImageView;

/// Whether a pixel is the frame colour, the middle colour, or the stage behind them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Frame,
    Middle,
    /// The positioned child: it must not move when the art around it is sliced.
    Marker,
    Stage,
}

fn part_of(pixel: [u8; 3]) -> Part {
    let [r, g, b] = pixel;
    if r > 150 && g < 110 && b < 110 {
        Part::Frame
    } else if g > 150 && r < 110 && b < 110 {
        Part::Marker
    } else if b > 150 && r < 110 {
        Part::Middle
    } else {
        Part::Stage
    }
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: measure_box <image.png> [more.png ...]");
        std::process::exit(2);
    }

    println!(
        "{:<38} {:>5} {:>5} {:>6} {:>6} {:>8} {:>8} {:>10} {:>9}",
        "image", "left", "top", "width", "height", "border-l", "border-t", "middle-at", "marker-at"
    );

    for path in paths {
        let image = match image::open(&path) {
            Ok(image) => image.to_rgb8(),
            Err(error) => {
                println!("{path}: could not be read: {error}");
                continue;
            }
        };
        let (width, height) = image.dimensions();

        // The box's extent: anything that is not the stage.
        let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..height {
            for x in 0..width {
                if part_of(image.get_pixel(x, y).0) != Part::Stage {
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x);
                    bottom = bottom.max(y);
                }
            }
        }
        if left == u32::MAX {
            println!("{path:<22} nothing was drawn");
            continue;
        }

        // Border thickness, read across the middle of the box where the frame and the middle both
        // appear, in both directions.
        let middle_y = (top + bottom) / 2;
        let border_left = (left..=right)
            .take_while(|x| part_of(image.get_pixel(*x, middle_y).0) == Part::Frame)
            .count();
        let middle_x = (left + right) / 2;
        let border_top = (top..=bottom)
            .take_while(|y| part_of(image.get_pixel(middle_x, *y).0) == Part::Frame)
            .count();

        // Where the middle actually starts, and how much frame survived. Between them these say
        // whether the contents moved and whether the border art is still being drawn at all.
        let (mut middle_left, mut middle_top) = (u32::MAX, u32::MAX);
        let (mut marker_left, mut marker_top) = (u32::MAX, u32::MAX);
        let mut frame_pixels = 0usize;
        for y in 0..height {
            for x in 0..width {
                match part_of(image.get_pixel(x, y).0) {
                    Part::Middle => {
                        middle_left = middle_left.min(x);
                        middle_top = middle_top.min(y);
                    }
                    Part::Frame => frame_pixels += 1,
                    Part::Marker => {
                        marker_left = marker_left.min(x);
                        marker_top = marker_top.min(y);
                    }
                    Part::Stage => {}
                }
            }
        }

        println!(
            "{:<38} {left:>5} {top:>5} {:>6} {:>6} {border_left:>8} {border_top:>8} {:>10} {:>9}",
            path,
            right - left + 1,
            bottom - top + 1,
            if middle_left == u32::MAX {
                "none".to_string()
            } else {
                format!("{middle_left},{middle_top}")
            },
            if marker_left == u32::MAX {
                "none".to_string()
            } else {
                format!("{marker_left},{marker_top}")
            },
        );
        let _ = frame_pixels;
    }
}
