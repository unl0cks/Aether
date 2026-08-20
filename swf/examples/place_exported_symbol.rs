//! Put a real AQW item on a stage so it can be rendered headlessly.
//!
//! An item file exports its artwork as a symbol and never places it: the game does that, by
//! instantiating the class. So rendering the file directly produces an empty frame, and every
//! attempt at the weapon glow so far has had to approximate the artwork with a shape built by hand.
//! Approximations keep agreeing with each other and disagreeing with the game.
//!
//! This reads the item, finds the symbol its class is bound to, and writes a copy with that symbol
//! placed on the main timeline, centred on a stage big enough that nothing is clipped by the stage
//! itself. The result renders in the exporter like any other movie.
//!
//! Usage: `cargo run -p swf --example place_exported_symbol -- <in.swf> <out.swf> [scale]`

use std::fs::File;

use swf::{
    CharacterId, Compression, Fixed8, Fixed16, Header, Matrix, PlaceObject, PlaceObjectAction,
    Rectangle, Sprite, Tag, Twips,
};

/// Large enough that a glow reaching tens of pixels has room on every side.
const STAGE: f64 = 1200.0;

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .expect("usage: place_exported_symbol <in.swf> <out.swf> [scale]");
    let output = args
        .next()
        .expect("usage: place_exported_symbol <in.swf> <out.swf> [scale]");
    let scale: f32 = args.next().and_then(|arg| arg.parse().ok()).unwrap_or(1.0);

    let data = std::fs::read(&input).expect("could not read the input movie");
    let buf = swf::decompress_swf(&data[..]).expect("could not decompress the input movie");
    let movie = swf::parse_swf(&buf).expect("could not parse the input movie");

    // The class binding is what the game instantiates, so it names the symbol worth looking at.
    // Character 0 is the movie's own main timeline and would place the file inside itself.
    let mut exported: Vec<(CharacterId, String)> = Vec::new();
    for tag in &movie.tags {
        match tag {
            Tag::SymbolClass(links) => {
                for link in links {
                    if link.id != CharacterId::from(0u16) {
                        exported.push((link.id, link.class_name.to_string_lossy(swf::UTF_8)));
                    }
                }
            }
            // The game reaches most of its interface by `getDefinitionByName`, which resolves an
            // export name rather than a class binding, so those have to be reachable here too.
            Tag::ExportAssets(assets) => {
                for asset in assets.iter() {
                    if asset.id != CharacterId::from(0u16) {
                        exported.push((asset.id, asset.name.to_string_lossy(swf::UTF_8)));
                    }
                }
            }
            _ => {}
        }
    }

    let wanted = std::env::args().find_map(|arg| arg.strip_prefix("symbol:").map(str::to_owned));
    let (id, name) = match &wanted {
        Some(wanted) => exported
            .iter()
            .find(|(_, name)| name == wanted)
            .cloned()
            .unwrap_or_else(|| {
                let mut names: Vec<&str> = exported.iter().map(|(_, name)| name.as_str()).collect();
                names.sort_unstable();
                panic!("no symbol named `{wanted}`; this movie exports {names:?}")
            }),
        None => exported.first().cloned().expect(
            "this movie exports no symbol, so there is nothing for the game to instantiate",
        ),
    };

    // Everything up to and including the definitions, then the placement. Dropping the original
    // frames avoids the file's own timeline moving or hiding what we just placed.
    let mut tags: Vec<Tag> = movie
        .tags
        .into_iter()
        .filter(|tag| {
            !matches!(
                tag,
                Tag::ShowFrame | Tag::PlaceObject(_) | Tag::RemoveObject(_) | Tag::End
            )
        })
        .collect();

    // AQW holds a weapon at an angle, and a turned object is cached as its bounding box rather than
    // as itself, so rotation has to be reachable here.
    let degrees: f32 = std::env::args()
        .find_map(|arg| arg.strip_prefix("rot:").map(str::to_owned))
        .and_then(|deg| deg.parse().ok())
        .unwrap_or(0.0);
    let (sin, cos) = degrees.to_radians().sin_cos();
    let centre = Twips::from_pixels(STAGE / 2.0);
    let matrix = Matrix {
        a: Fixed16::from_f32(scale * cos),
        b: Fixed16::from_f32(scale * sin),
        c: Fixed16::from_f32(-scale * sin),
        d: Fixed16::from_f32(scale * cos),
        tx: centre,
        ty: centre,
    };

    // How AQW actually draws it: the weapon sits inside a character that is held as a bitmap and
    // carries the scale, rather than being scaled itself. The item alone renders correctly, so the
    // container is the remaining difference between here and the game.
    let wrapped = std::env::args().any(|arg| arg == "nested");
    let placed_id = if wrapped {
        let wrapper = CharacterId::from(u16::from(id).saturating_add(1000));
        tags.push(Tag::DefineSprite(Sprite {
            id: wrapper,
            num_frames: 1,
            tags: vec![
                Tag::PlaceObject(Box::new(PlaceObject {
                    version: 3,
                    action: PlaceObjectAction::Place(id),
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
        }));
        wrapper
    } else {
        id
    };

    tags.push(Tag::PlaceObject(Box::new(PlaceObject {
        version: 3,
        action: PlaceObjectAction::Place(placed_id),
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
        is_bitmap_cached: wrapped.then_some(true),
        is_visible: None,
        amf_data: None,
    })));
    tags.push(Tag::ShowFrame);

    let header = Header {
        compression: Compression::None,
        version: buf.header.version(),
        stage_size: Rectangle {
            x_min: Twips::ZERO,
            x_max: Twips::from_pixels(STAGE),
            y_min: Twips::ZERO,
            y_max: Twips::from_pixels(STAGE),
        },
        frame_rate: Fixed8::from_f64(24.0),
        num_frames: 1,
    };

    let file = File::create(&output).expect("could not create the output movie");
    swf::write_swf(&header, &tags, file).expect("could not write the output movie");
    println!("{output}: {name} (character {id:?}) placed at {scale}x on a {STAGE}px stage");
}
