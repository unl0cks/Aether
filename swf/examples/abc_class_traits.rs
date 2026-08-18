//! Dump every trait a class declares, so a compatibility hook can be written against what is
//! actually there rather than what a string search suggested might be.
//!
//! The ABC string pool is one flat table shared by the whole movie, so names that sit near each
//! other in the file need not belong to the same class. Reading a hook's field names out of that
//! neighbourhood is a guess; reading them off the class's own trait table is not.
//!
//! Usage: `cargo run -p swf --example abc_class_traits -- <movie.swf> <class-substring>`

use std::fs::File;

use swf::avm2::read::Reader;
use swf::avm2::types::{AbcFile, Index, Multiname, TraitKind};

/// A string out of the constant pool. Index 0 is "no string", which reads as empty.
fn string(abc: &AbcFile, index: Index<String>) -> String {
    if index.0 == 0 {
        return String::new();
    }
    abc.constant_pool
        .strings
        .get(index.0 as usize - 1)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .unwrap_or_default()
}

/// The local half of a multiname, which is all that is needed to recognise a class or a trait.
fn multiname_local(abc: &AbcFile, index: Index<Multiname>) -> String {
    if index.0 == 0 {
        return String::new();
    }
    match abc.constant_pool.multinames.get(index.0 as usize - 1) {
        Some(
            Multiname::QName { name, .. }
            | Multiname::QNameA { name, .. }
            | Multiname::RTQName { name }
            | Multiname::RTQNameA { name }
            | Multiname::Multiname { name, .. }
            | Multiname::MultinameA { name, .. },
        ) => string(abc, *name),
        _ => String::new(),
    }
}

fn kind_of(member: &swf::avm2::types::Trait) -> &'static str {
    match member.kind {
        TraitKind::Slot { .. } => "slot",
        TraitKind::Const { .. } => "const",
        TraitKind::Method { .. } => "method",
        TraitKind::Getter { .. } => "getter",
        TraitKind::Setter { .. } => "setter",
        TraitKind::Function { .. } => "function",
        TraitKind::Class { .. } => "class",
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: abc_class_traits <movie.swf> <class-substring>");
    let wanted = args
        .next()
        .expect("usage: abc_class_traits <movie.swf> <class-substring>");

    let file = File::open(&path).expect("could not open the movie");
    let buf = swf::decompress_swf(file).expect("could not decompress the movie");
    let swf = swf::parse_swf(&buf).expect("could not parse the movie");

    let mut blocks = Vec::new();
    for tag in &swf.tags {
        match tag {
            swf::Tag::DoAbc(bytes) => blocks.push(*bytes),
            swf::Tag::DoAbc2(abc) => blocks.push(abc.data),
            _ => {}
        }
    }

    for bytes in &blocks {
        let mut reader = Reader::new(bytes);
        let Ok(abc) = reader.read() else { continue };

        for instance in &abc.instances {
            let class_name = multiname_local(&abc, instance.name);
            if !class_name.to_lowercase().contains(&wanted.to_lowercase()) {
                continue;
            }
            println!(
                "class `{class_name}` extends `{}`",
                multiname_local(&abc, instance.super_name)
            );
            for member in &instance.traits {
                println!(
                    "    {:8} {}",
                    kind_of(member),
                    multiname_local(&abc, member.name)
                );
            }
            println!();
        }
    }
}
