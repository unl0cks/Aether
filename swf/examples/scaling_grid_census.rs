//! Report every scaling grid in a movie and what the object carrying it is made of.
//!
//! Whether slicing may be applied to a whole object depends entirely on what is inside it. A grid on
//! a sprite that holds nothing but background shapes can be sliced freely: there is nothing in it
//! but the border art the grid exists to protect. A grid on a sprite that also holds text, buttons
//! or nested sprites cannot, because slicing draws each band under its own transform, and a caption
//! sitting in a border band would be drawn at its authored size anchored to the object's edge --
//! which is to say, moved up and to the left of where it belongs.
//!
//! Usage: `cargo run -p swf --example scaling_grid_census -- <movie.swf>`

use std::collections::{HashMap, HashSet};
use std::fs::File;

use swf::{CharacterId, Tag};

/// What a character is, in the one word that matters here.
fn kind_of(tag: &Tag<'_>) -> Option<(CharacterId, &'static str)> {
    Some(match tag {
        Tag::DefineShape(shape) => (shape.id, "shape"),
        Tag::DefineSprite(sprite) => (sprite.id, "sprite"),
        Tag::DefineEditText(text) => (text.id(), "TEXT"),
        Tag::DefineText(text) => (text.id, "TEXT"),
        Tag::DefineText2(text) => (text.id, "TEXT"),
        Tag::DefineButton(button) => (button.id, "BUTTON"),
        Tag::DefineButton2(button) => (button.id, "BUTTON"),
        Tag::DefineBitsLossless(bits) => (bits.id, "bitmap"),
        Tag::DefineBitsJpeg2 { id, .. } => (*id, "bitmap"),
        Tag::DefineBitsJpeg3(bits) => (bits.id, "bitmap"),
        Tag::DefineMorphShape(shape) => (shape.id, "morph"),
        _ => return None,
    })
}

/// The characters a sprite places on its timeline.
fn placed_by(sprite: &swf::Sprite<'_>) -> Vec<CharacterId> {
    let mut placed = Vec::new();
    for tag in &sprite.tags {
        if let Tag::PlaceObject(place) = tag
            && let swf::PlaceObjectAction::Place(id) | swf::PlaceObjectAction::Replace(id) =
                place.action
        {
            placed.push(id);
        }
    }
    placed
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: scaling_grid_census <movie.swf>");
    let file = File::open(&path).expect("could not open the movie");
    let buf = swf::decompress_swf(file).expect("could not decompress the movie");
    let swf = swf::parse_swf(&buf).expect("could not parse the movie");

    let mut kinds: HashMap<CharacterId, &'static str> = HashMap::new();
    let mut sprites: HashMap<CharacterId, Vec<CharacterId>> = HashMap::new();
    let mut names: HashMap<CharacterId, String> = HashMap::new();
    let mut grids: Vec<(CharacterId, swf::Rectangle<swf::Twips>)> = Vec::new();

    // Sprites nest, so walk into them rather than only over the top level.
    let mut queue: Vec<&Tag<'_>> = swf.tags.iter().collect();
    while let Some(tag) = queue.pop() {
        if let Some((id, kind)) = kind_of(tag) {
            kinds.insert(id, kind);
        }
        match tag {
            Tag::DefineSprite(sprite) => {
                sprites.insert(sprite.id, placed_by(sprite));
                queue.extend(sprite.tags.iter());
            }
            Tag::DefineScalingGrid { id, splitter_rect } => {
                grids.push((*id, splitter_rect.clone()));
            }
            Tag::SymbolClass(symbols) => {
                for symbol in symbols.iter() {
                    names.insert(
                        symbol.id,
                        String::from_utf8_lossy(symbol.class_name.as_bytes()).into_owned(),
                    );
                }
            }
            Tag::ExportAssets(assets) => {
                for asset in assets.iter() {
                    names.insert(
                        asset.id,
                        String::from_utf8_lossy(asset.name.as_bytes()).into_owned(),
                    );
                }
            }
            _ => {}
        }
    }

    println!("{path}: {} scaling grid(s)", grids.len());

    let mut art_only = 0usize;
    let mut with_content = 0usize;

    for (id, rect) in &grids {
        let name = names.get(id).cloned().unwrap_or_default();
        let kind = kinds.get(id).copied().unwrap_or("<undefined>");
        let children = sprites.get(id).cloned().unwrap_or_default();

        // What is in it, counted by kind, one level down and then through nested sprites.
        let mut seen: HashSet<CharacterId> = HashSet::new();
        let mut stack = children.clone();
        let mut census: HashMap<&'static str, usize> = HashMap::new();
        let mut depth_limit = 20_000;
        while let Some(child) = stack.pop() {
            if !seen.insert(child) || depth_limit == 0 {
                continue;
            }
            depth_limit -= 1;
            let child_kind = kinds.get(&child).copied().unwrap_or("<undefined>");
            *census.entry(child_kind).or_default() += 1;
            if let Some(grandchildren) = sprites.get(&child) {
                stack.extend(grandchildren.iter().copied());
            }
        }

        let interactive = census.get("TEXT").copied().unwrap_or(0)
            + census.get("BUTTON").copied().unwrap_or(0);
        if interactive > 0 {
            with_content += 1;
        } else {
            art_only += 1;
        }

        let mut summary: Vec<String> = census
            .iter()
            .map(|(kind, count)| format!("{count} {kind}"))
            .collect();
        summary.sort();
        println!(
            "  id {id:<5} {kind:<7} {:<34} grid {:.0}x{:.0}  contents: {}",
            if name.is_empty() { "-" } else { &name },
            rect.width().to_pixels(),
            rect.height().to_pixels(),
            if summary.is_empty() {
                "<nothing>".to_string()
            } else {
                summary.join(", ")
            }
        );
    }

    println!(
        "{art_only} grid(s) over art only, {with_content} grid(s) over text or buttons as well"
    );
}
