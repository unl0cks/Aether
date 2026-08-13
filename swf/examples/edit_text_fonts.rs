//! Report what font each text field asks for, and whether it wants the embedded one.
//!
//! `use_outlines` is the SWF's `embedFonts`: set, the field wants the glyphs the movie carries;
//! clear, it wants whatever the system offers under that name. A field can name its font by
//! character id or by class name, and the two resolve differently.
//!
//! Usage: `cargo run -p swf --example edit_text_fonts -- <movie.swf>`

use std::collections::HashMap;
use std::fs::File;
use swf::{CharacterId, Tag};

fn main() {
    let path = std::env::args().nth(1).expect("usage: edit_text_fonts <movie.swf>");
    let file = File::open(&path).expect("could not open the movie");
    let buf = swf::decompress_swf(file).expect("could not decompress the movie");
    let swf = swf::parse_swf(&buf).expect("could not parse the movie");

    let mut font_names: HashMap<CharacterId, String> = HashMap::new();
    let mut fields = Vec::new();
    let mut queue: Vec<&Tag<'_>> = swf.tags.iter().collect();
    while let Some(tag) = queue.pop() {
        match tag {
            Tag::DefineFont2(font) => {
                font_names.insert(font.id, font.name.to_string_lossy(swf::UTF_8));
            }
            Tag::DefineEditText(text) => fields.push((
                text.font_id(),
                text.font_class().map(|c| c.to_string_lossy(swf::UTF_8)),
                text.height().map(|h| h.to_pixels()).unwrap_or(0.0),
                text.use_outlines(),
            )),
            Tag::DefineSprite(sprite) => queue.extend(sprite.tags.iter()),
            _ => {}
        }
    }

    let mut tally: HashMap<String, (usize, usize)> = HashMap::new();
    for (id, class, height, embedded) in &fields {
        let name = class.clone().or_else(|| {
            id.and_then(|id| font_names.get(&id).cloned())
        }).unwrap_or_else(|| "<none>".into());
        let key = format!("{name} @ {height:.0}pt");
        let entry = tally.entry(key).or_default();
        entry.0 += 1;
        if *embedded {
            entry.1 += 1;
        }
    }

    println!("{path}: {} text field(s)", fields.len());
    let mut rows: Vec<_> = tally.into_iter().collect();
    rows.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));
    println!("{:>6} {:>9}  font", "fields", "embedded");
    for (name, (count, embedded)) in rows.into_iter().take(22) {
        println!("{count:>6} {embedded:>9}  {name}");
    }
}
