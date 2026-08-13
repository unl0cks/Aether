//! Report whether a movie's ABC carries method names, and what `AvatarMC` looks like inside it.
//!
//! The movement stop guard classifies a method by the name in the ABC `MethodInfo`. That field is
//! optional, and a shipped build may leave it out entirely, in which case every method is called
//! `""` and nothing can ever match. A decompiler still shows names because it reads them off the
//! *traits* that point at the methods, which are a different table and are never absent.
//!
//! Usage: `cargo run -p swf --example abc_method_names -- <movie.swf> [class]`

use std::collections::HashMap;
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

/// The local half of a multiname, which is all that is needed to recognise a class.
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

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: abc_method_names <movie.swf> [class]");
    let wanted_class = args.next().unwrap_or_else(|| "AvatarMC".to_string());

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
    println!("{path}: {} ABC block(s)", blocks.len());

    let mut total_methods = 0usize;
    let mut unnamed_methods = 0usize;
    let mut named_examples: Vec<String> = Vec::new();

    for (block_index, bytes) in blocks.iter().enumerate() {
        let mut reader = Reader::new(bytes);
        let abc = match reader.read() {
            Ok(abc) => abc,
            Err(error) => {
                println!("  block {block_index}: could not be read: {error}");
                continue;
            }
        };

        for method in &abc.methods {
            total_methods += 1;
            if method.name.0 == 0 {
                unnamed_methods += 1;
            } else if named_examples.len() < 8 {
                let name = string(&abc, method.name);
                if !name.is_empty() {
                    named_examples.push(name);
                }
            }
        }

        // What the traits say, for the class we care about. Traits always carry a name.
        for instance in &abc.instances {
            let class_name = multiname_local(&abc, instance.name);
            if class_name != wanted_class {
                continue;
            }
            println!("  block {block_index}: instance `{class_name}`");
            let mut by_method: HashMap<u32, Vec<String>> = HashMap::new();
            for member in &instance.traits {
                let member_name = multiname_local(&abc, member.name);
                let method_index = match member.kind {
                    TraitKind::Method { method, .. }
                    | TraitKind::Getter { method, .. }
                    | TraitKind::Setter { method, .. } => method,
                    TraitKind::Function { function, .. } => function,
                    _ => continue,
                };
                by_method
                    .entry(method_index.0)
                    .or_default()
                    .push(member_name);
            }

            for wanted in ["walkTo", "stopWalking", "onEnterFrameWalk", "simulateTo"] {
                let found = by_method
                    .iter()
                    .find(|(_, names)| names.iter().any(|name| name == wanted));
                match found {
                    Some((method_index, _)) => {
                        let abc_name = abc
                            .methods
                            .get(*method_index as usize)
                            .map(|method| {
                                if method.name.0 == 0 {
                                    "<none>".to_string()
                                } else {
                                    format!("{:?}", string(&abc, method.name))
                                }
                            })
                            .unwrap_or_else(|| "<out of range>".to_string());
                        println!(
                            "    trait {wanted:<17} -> method #{method_index}, ABC method name {abc_name}"
                        );
                    }
                    None => println!("    trait {wanted:<17} -> not present on this class"),
                }
            }
        }
    }

    println!(
        "methods: {total_methods} total, {unnamed_methods} with no ABC name ({:.1}%)",
        if total_methods == 0 {
            0.0
        } else {
            unnamed_methods as f64 * 100.0 / total_methods as f64
        }
    );
    if named_examples.is_empty() {
        println!("no method in this movie carries an ABC name at all");
    } else {
        println!("named examples: {named_examples:?}");
    }
}
