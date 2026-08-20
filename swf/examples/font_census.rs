//! List the fonts a movie embeds, so a name that text asks for can be told from one it does not.
//!
//! Usage: `cargo run -p swf --example font_census -- <movie.swf>`

use std::fs::File;
use swf::Tag;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: font_census <movie.swf>");
    let file = File::open(&path).expect("could not open the movie");
    let buf = swf::decompress_swf(file).expect("could not decompress the movie");
    let swf = swf::parse_swf(&buf).expect("could not parse the movie");

    let mut fonts: Vec<(u16, String, usize, bool, bool, bool)> = Vec::new();
    let mut queue: Vec<&Tag<'_>> = swf.tags.iter().collect();
    while let Some(tag) = queue.pop() {
        match tag {
            Tag::DefineFont2(font) => fonts.push((
                font.id.into(),
                font.name.to_string_lossy(swf::UTF_8),
                font.glyphs.len(),
                font.flags.contains(swf::FontFlag::HAS_LAYOUT),
                font.flags.contains(swf::FontFlag::IS_BOLD),
                font.flags.contains(swf::FontFlag::IS_ITALIC),
            )),
            Tag::DefineSprite(sprite) => queue.extend(sprite.tags.iter()),
            _ => {}
        }
    }
    println!("{path}: {} embedded font(s)", fonts.len());
    fonts.sort_by(|a, b| a.1.cmp(&b.1));
    for (id, name, glyphs, layout, bold, italic) in fonts {
        println!(
            "  id {id:<5} {name:<28} {glyphs:>5} glyphs  layout {layout:<5} BOLD {bold:<5} italic {italic}"
        );
    }
}
