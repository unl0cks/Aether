//! What a map's authored display list asks of the blend pipeline.
//!
//! The new 2026 town maps (Battleon 11aug26, Yulgar 2july26) render submit-bound at 21-30 FPS,
//! and the live metrics can only say "the map draws N blends per second". This reads the answer
//! straight out of the SWF instead: every PlaceObject with a non-Normal blend mode, a
//! cacheAsBitmap flag, or filters, at every timeline depth, including inside every DefineSprite.
//! The interleaving ORDER matters as much as the counts -- runs of one blend mode can share a
//! pass, alternating modes cannot -- so the per-sprite mode sequences are printed too.

use std::collections::BTreeMap;
use std::fs::File;

#[derive(Default)]
struct Census {
    blend_modes: BTreeMap<String, usize>,
    cached: usize,
    filtered: usize,
    filter_kinds: BTreeMap<String, usize>,
    placements: usize,
}

fn filter_name(filter: &swf::Filter) -> &'static str {
    match filter {
        swf::Filter::DropShadowFilter(_) => "DropShadow",
        swf::Filter::BlurFilter(_) => "Blur",
        swf::Filter::GlowFilter(_) => "Glow",
        swf::Filter::BevelFilter(_) => "Bevel",
        swf::Filter::GradientGlowFilter(_) => "GradientGlow",
        swf::Filter::ConvolutionFilter(_) => "Convolution",
        swf::Filter::ColorMatrixFilter(_) => "ColorMatrix",
        swf::Filter::GradientBevelFilter(_) => "GradientBevel",
    }
}

fn visit_place(place: &swf::PlaceObject, census: &mut Census, sequence: &mut Vec<String>) {
    census.placements += 1;
    if let Some(mode) = place.blend_mode
        && mode != swf::BlendMode::Normal
    {
        let name = format!("{mode:?}");
        *census.blend_modes.entry(name.clone()).or_default() += 1;
        sequence.push(name);
    }
    if place.is_bitmap_cached == Some(true) {
        census.cached += 1;
    }
    if let Some(filters) = &place.filters
        && !filters.is_empty()
    {
        census.filtered += 1;
        for filter in filters {
            *census
                .filter_kinds
                .entry(filter_name(filter).to_string())
                .or_default() += 1;
        }
    }
}

fn visit_tags(tags: &[swf::Tag], census: &mut Census, label: &str, report_sequences: bool) {
    let mut sequence = Vec::new();
    for tag in tags {
        match tag {
            swf::Tag::PlaceObject(place) => visit_place(place, census, &mut sequence),
            swf::Tag::DefineSprite(sprite) => {
                visit_tags(
                    &sprite.tags,
                    census,
                    &format!("{label}/sprite{}", sprite.id),
                    report_sequences,
                );
            }
            _ => {}
        }
    }
    // The order the batcher faces: only sequences with mode CHANGES defeat run-batching.
    if report_sequences && sequence.len() > 1 {
        let changes = sequence.windows(2).filter(|w| w[0] != w[1]).count();
        if changes > 0 {
            println!(
                "  {label}: {} blended placements, {} mode changes: {}",
                sequence.len(),
                changes,
                sequence.join(" ")
            );
        }
    }
}

/// Print every placement of one character id, with the attributes the cache-decision code reads.
fn report_character(tags: &[swf::Tag], wanted: u16, label: &str) {
    for tag in tags {
        match tag {
            swf::Tag::PlaceObject(place) => {
                let placed = match place.action {
                    swf::PlaceObjectAction::Place(id) | swf::PlaceObjectAction::Replace(id) => id,
                    swf::PlaceObjectAction::Modify => continue,
                };
                if placed == wanted {
                    println!(
                        "{label}: depth={} blend={:?} cacheAsBitmap={:?} colorTransform={:?} filters={} name={:?}",
                        place.depth,
                        place.blend_mode,
                        place.is_bitmap_cached,
                        place.color_transform,
                        place.filters.as_ref().map_or(0, Vec::len),
                        place
                            .name
                            .map(|n| String::from_utf8_lossy(n.as_bytes()).into_owned()),
                    );
                }
            }
            swf::Tag::DefineSprite(sprite) => {
                report_character(
                    &sprite.tags,
                    wanted,
                    &format!("{label}/sprite{}", sprite.id),
                );
            }
            _ => {}
        }
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: blend_census <movie.swf> [character_id]");
    let file = File::open(&path).expect("could not open the movie");
    let buf = swf::decompress_swf(file).expect("could not decompress the movie");
    let swf = swf::parse_swf(&buf).expect("could not parse the movie");

    if let Some(id) = std::env::args().nth(2) {
        let id: u16 = id.parse().expect("character id must be a number");
        report_character(&swf.tags, id, "root");
        return;
    }

    let mut census = Census::default();
    println!("== interleaving (sequences with mode changes) ==");
    visit_tags(&swf.tags, &mut census, "root", true);

    println!("== totals for {path} ==");
    println!("placements: {}", census.placements);
    println!("non-Normal blend placements by mode:");
    for (mode, n) in &census.blend_modes {
        println!("  {mode:14} {n}");
    }
    println!("cacheAsBitmap placements: {}", census.cached);
    println!("filtered placements: {} by kind:", census.filtered);
    for (kind, n) in &census.filter_kinds {
        println!("  {kind:14} {n}");
    }
}
