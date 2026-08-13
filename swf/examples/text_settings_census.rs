//! Report the text rendering settings a movie asks for, which Aether currently ignores.
//!
//! `DefineCSMTextSettings` is how a Flash author says *how* text should be filled: advanced
//! antialiasing on or off, whether to snap stems to the pixel grid, and a thickness and sharpness
//! to fill them with. Nothing in `ruffle_core` reads any of it, so every one of these is answered
//! by drawing the glyph outline plainly -- which comes out thinner and softer than Flash.
//!
//! Usage: `cargo run -p swf --example text_settings_census -- <movie.swf>`

use std::collections::HashMap;
use std::fs::File;

use swf::Tag;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: text_settings_census <movie.swf>");
    let file = File::open(&path).expect("could not open the movie");
    let buf = swf::decompress_swf(file).expect("could not decompress the movie");
    let swf = swf::parse_swf(&buf).expect("could not parse the movie");

    let mut settings = Vec::new();
    let mut queue: Vec<&Tag<'_>> = swf.tags.iter().collect();
    while let Some(tag) = queue.pop() {
        match tag {
            Tag::CsmTextSettings(csm) => settings.push(csm.clone()),
            Tag::DefineSprite(sprite) => queue.extend(sprite.tags.iter()),
            _ => {}
        }
    }

    println!("{path}: {} text setting(s)", settings.len());
    if settings.is_empty() {
        return;
    }

    let mut shapes: HashMap<String, usize> = HashMap::new();
    for csm in &settings {
        let key = format!(
            "advanced {:<5} grid-fit {:<8} thickness {:>5.2} sharpness {:>6.2}",
            csm.use_advanced_rendering,
            format!("{:?}", csm.grid_fit),
            csm.thickness,
            csm.sharpness,
        );
        *shapes.entry(key).or_default() += 1;
    }

    let mut rows: Vec<_> = shapes.into_iter().collect();
    rows.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    for (shape, count) in rows {
        println!("  {count:>5} x  {shape}");
    }
}
