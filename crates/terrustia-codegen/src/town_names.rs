//! `crates/terrustia-proto/src/town_names.rs` — the names town NPCs are given.
//!
//! `NPC.getNewNPCNameInner` (`Terraria/NPC.cs`) is a `npcType switch` mapping a type to a
//! `Language.RandomFromCategory` call; the category names index into
//! `Terraria.Localization.Content.en-US.Town.json`. Three types are not one creature but six — a
//! cat, a dog and a bunny each roll a breed, and the breed decides both how it looks and which
//! list its name comes from — and those breeds are registered in
//! `Terraria.GameContent/TownNPCProfiles.cs` rather than in the name switch.
//!
//! Ported from `gen_town_names.py`.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use regex::Regex;
use serde_json::Value;

use crate::csharp::read_lossy;

/// A variant profile's breeds: `(breed name, its name list)`.
type Breeds = Vec<(String, Vec<String>)>;

/// Read a category's names in the localisation file's own order (`.values()`, not sorted).
fn names_in(data: &Value, category: &str) -> Vec<String> {
    let node = data
        .get(category)
        .unwrap_or_else(|| panic!("category {category} is not in the localisation file"));
    let obj = node
        .as_object()
        .unwrap_or_else(|| panic!("category {category} is not an object"));
    obj.values()
        .map(|v| v.as_str().expect("name is not a string").to_string())
        .collect()
}

/// `category.upper()` with anything that is not `A-Z0-9` turned into `_`.
fn const_name(category: &str) -> String {
    category
        .to_uppercase()
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() || c.is_ascii_digit() {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// One `const NAME: [&str; N] = [...]` array declaration, wrapped past 80 characters per row,
/// same as the Python original (`cargo fmt` reflows it regardless).
fn names_array(const_name: &str, names: &[String], count: i64) -> Vec<String> {
    let mut out = vec![format!("const {const_name}: [&str; {count}] = [")];
    let mut row: Vec<String> = Vec::new();
    let mut row_len: usize = 0;
    for n in names {
        let entry = format!("{:?},", n);
        row_len += entry.len();
        row.push(entry);
        if row_len > 80 {
            out.push(format!("    {}", row.join(" ")));
            row.clear();
            row_len = 0;
        }
    }
    if !row.is_empty() {
        out.push(format!("    {}", row.join(" ")));
    }
    out.push("];".to_string());
    out
}

pub fn generate(root: &Path) -> String {
    let npc_cs = read_lossy(&root.join("Terraria/NPC.cs"));
    let town_json_raw = read_lossy(&root.join("Terraria.Localization.Content.en-US.Town.json"));

    // The localisation files carry trailing commas, which JSON proper does not allow.
    let trailing_comma_re = Regex::new(r",(\s*[}\]])").unwrap();
    let cleaned = trailing_comma_re.replace_all(&town_json_raw, "$1");
    let data: Value = serde_json::from_str(&cleaned).expect("Town.json does not parse");

    // type -> category, straight out of getNewNPCNameInner's switch.
    let start = npc_cs
        .find("private static string getNewNPCNameInner")
        .expect("no getNewNPCNameInner");
    let end = npc_cs[start..]
        .find("public NetworkText GetTypeNetName")
        .expect("no GetTypeNetName")
        + start;
    let switch = &npc_cs[start..end];
    let pair_re = Regex::new(r#"(\d+) => Language\.RandomFromCategory\("([A-Za-z_]+)""#).unwrap();
    let pairs: Vec<(i64, String)> = pair_re
        .captures_iter(switch)
        .map(|c| (c[1].parse().unwrap(), c[2].to_string()))
        .collect();
    assert!(
        pairs.len() >= 30,
        "only {} name categories parsed; the switch shape changed",
        pairs.len()
    );

    let mut entries: BTreeMap<i64, (String, Vec<String>)> = BTreeMap::new();
    for (npc_type, category) in &pairs {
        let names = names_in(&data, category);
        entries.insert(*npc_type, (category.clone(), names));
    }

    // Three types are not one creature but six: a cat, a dog and a bunny each roll a breed, and
    // the breed decides both how it looks and which list its name comes from. The breeds are
    // registered in TownNPCProfiles rather than in the name switch, so they are read from there.
    let profiles_cs = read_lossy(&root.join("Terraria.GameContent/TownNPCProfiles.cs"));
    let variant_re = Regex::new(
        r#"\{\s*(\d+),\s*new Profiles\.VariantNPCProfile\(\s*"[^"]*",\s*"([A-Za-z]+)",\s*[A-Za-z]+,\s*((?:"[A-Za-z]+"(?:,\s*)?)+)\)"#,
    )
    .unwrap();
    let breed_re = Regex::new(r#""([A-Za-z]+)""#).unwrap();

    let mut variants: BTreeMap<i64, (String, Breeds)> = BTreeMap::new();
    for caps in variant_re.captures_iter(&profiles_cs) {
        let npc_type: i64 = caps[1].parse().unwrap();
        let base = caps[2].to_string();
        let breeds: Vec<String> = breed_re
            .captures_iter(&caps[3])
            .map(|c| c[1].to_string())
            .collect();
        let breed_names: Breeds = breeds
            .iter()
            .map(|b| {
                let category = format!("{base}Names_{b}");
                let names = names_in(&data, &category);
                (b.clone(), names)
            })
            .collect();
        variants.insert(npc_type, (base, breed_names));
    }
    assert!(
        !variants.is_empty(),
        "no variant profiles parsed; TownNPCProfiles' shape changed"
    );

    let mut lines: Vec<String> = vec![
        "//! The names town NPCs are given, generated from the game's own lists.\n\
         //!\n\
         //! A town NPC carries a name of its own on top of its type, and the client asks the server for\n\
         //! it the moment the NPC comes into view. Left unanswered, every guide in the world is \"Guide\"\n\
         //! and no two of them can be told apart.\n\
         //!\n\
         //! The lists are the localisation file's, and which list a type draws from is\n\
         //! `NPC.getNewNPCNameInner`. Generated by `terrustia-codegen` from Terraria 1.4.5.8. Do\n\
         //! not edit by hand.\n"
            .to_string(),
    ];

    let mut emitted: HashSet<String> = HashSet::new();
    for (&npc_type, (category, names)) in &entries {
        let const_name = const_name(category);
        emitted.insert(const_name.clone());
        lines.push(format!("/// `{category}`, for NPC type {npc_type}."));
        lines.extend(names_array(&const_name, names, names.len() as i64));
        lines.push(String::new());
    }

    lines.push(format!(
        "/// Which list a type draws its name from, in type order.\nconst LISTS: [(u16, &[&str]); {}] = [",
        entries.len()
    ));
    for (&npc_type, (category, _)) in &entries {
        let const_name = const_name(category);
        lines.push(format!("    ({npc_type}, &{const_name}),"));
    }
    lines.push("];".to_string());
    lines.push(String::new());

    for (base, breeds) in variants.values() {
        for (breed, names) in breeds {
            let const_name = const_name(&format!("{base}Names_{breed}"));
            if emitted.contains(&const_name) {
                continue;
            }
            emitted.insert(const_name.clone());
            lines.push(format!("/// `{base}Names_{breed}`."));
            lines.extend(names_array(&const_name, names, names.len() as i64));
            lines.push(String::new());
        }
    }

    lines.push(format!(
        "/// The types that are not one creature but several, and the name list each breed uses.\n\
         ///\n\
         /// A cat, a dog and a bunny each roll a breed on arrival; the breed decides how it looks and\n\
         /// which names it can have, which is why the two cannot be chosen independently.\n\
         const BREEDS: [(u16, &[&[&str]]); {}] = [",
        variants.len()
    ));
    for (&npc_type, (base, breeds)) in &variants {
        let inner: Vec<String> = breeds
            .iter()
            .map(|(breed, _)| format!("&{}", const_name(&format!("{base}Names_{breed}"))))
            .collect();
        lines.push(format!("    ({npc_type}, &[{}]),", inner.join(", ")));
    }
    lines.push("];".to_string());

    lines.push(
        "\n/// The names a type may be given, or an empty slice if it is not the kind of NPC that has one.\n\
         ///\n\
         /// Every NPC has a *type* name; this is the personal one on top of it, which only town NPCs,\n\
         /// town pets and town slimes carry.\n\
         pub fn names_for(npc_type: u16) -> &'static [&'static str] {\n    \
             match LISTS.binary_search_by_key(&npc_type, |&(ty, _)| ty) {\n        \
                 Ok(at) => LISTS[at].1,\n        \
                 Err(_) => &[],\n    \
             }\n\
         }\n\
         \n\
         /// Whether a type is given a personal name at all.\n\
         pub fn has_given_name(npc_type: u16) -> bool {\n    \
             !names_for(npc_type).is_empty()\n\
         }\n\
         \n\
         /// How many looks a type has to choose between when it arrives.\n\
         ///\n\
         /// One for almost everything. Six for the cat, the dog and the bunny, whose breed is rolled on\n\
         /// arrival and then decides both the sprite and the name list.\n\
         pub fn variation_count(npc_type: u16) -> usize {\n    \
             match BREEDS.binary_search_by_key(&npc_type, |&(ty, _)| ty) {\n        \
                 Ok(at) => BREEDS[at].1.len(),\n        \
                 Err(_) => 1,\n    \
             }\n\
         }\n\
         \n\
         /// The names a type may be given once its look has been chosen.\n\
         ///\n\
         /// For everything but the three pets this is exactly [`names_for`]; for those it is the chosen\n\
         /// breed's own list, because a Siamese is never called Rex.\n\
         pub fn names_for_variation(npc_type: u16, variation: usize) -> &'static [&'static str] {\n    \
             if let Ok(at) = BREEDS.binary_search_by_key(&npc_type, |&(ty, _)| ty)\n        \
                 && let Some(names) = BREEDS[at].1.get(variation)\n    \
             {\n        \
                 return names;\n    \
             }\n    \
             names_for(npc_type)\n\
         }\n\
         \n\
         #[cfg(test)]\n\
         mod tests {\n    \
             use super::*;\n\
             \n    \
             /// The list is sorted, or the binary search above silently misses.\n    \
             #[test]\n    \
             fn the_lists_are_in_type_order() {\n        \
                 assert!(\n            \
                     LISTS.windows(2).all(|w| w[0].0 < w[1].0),\n            \
                     \"LISTS must be strictly ascending by type\"\n        \
                 );\n    \
             }\n\
             \n    \
             /// The types the game names, and the ones it does not.\n    \
             #[test]\n    \
             fn the_named_types_are_the_town_ones() {\n        \
                 assert!(has_given_name(22), \"the guide has a name\");\n        \
                 assert!(has_given_name(17), \"so does the merchant\");\n        \
                 assert!(has_given_name(637), \"and the cat\");\n        \
                 assert!(!has_given_name(1), \"a green slime does not\");\n        \
                 assert!(!has_given_name(0), \"and neither does nothing\");\n    \
             }\n\
             \n    \
             /// Every list has something in it, and nothing in it is blank.\n    \
             #[test]\n    \
             fn every_list_holds_real_names() {\n        \
                 for (ty, names) in LISTS {\n            \
                     assert!(!names.is_empty(), \"type {ty} has an empty name list\");\n            \
                     assert!(\n                \
                         names.iter().all(|n| !n.trim().is_empty()),\n                \
                         \"type {ty} has a blank name\"\n            \
                     );\n        \
                 }\n    \
             }\n\
             \n    \
             /// The guide's list is the one the game ships, spot-checked against a name from it.\n    \
             #[test]\n    \
             fn the_guide_can_be_andrew() {\n        \
                 assert!(names_for(22).contains(&\"Andrew\"));\n    \
             }\n\
             \n    \
             /// The pets have six breeds each; everything else has one look.\n    \
             #[test]\n    \
             fn only_the_pets_have_breeds() {\n        \
                 assert!(BREEDS.windows(2).all(|w| w[0].0 < w[1].0));\n        \
                 for (ty, breeds) in BREEDS {\n            \
                     assert_eq!(breeds.len(), 6, \"type {ty} should have six breeds\");\n        \
                 }\n        \
                 assert_eq!(variation_count(22), 1, \"the guide has one look\");\n        \
                 assert_eq!(variation_count(637), 6, \"the cat has six\");\n    \
             }\n\
             \n    \
             /// A breed's names are its own, not the type's first list.\n    \
             #[test]\n    \
             fn a_breed_draws_from_its_own_names() {\n        \
                 let first = names_for_variation(637, 0);\n        \
                 let last = names_for_variation(637, 5);\n        \
                 assert!(!first.is_empty() && !last.is_empty());\n        \
                 assert_ne!(first, last, \"two breeds should not share a name list\");\n        \
                 assert_eq!(\n            \
                     names_for_variation(637, 0),\n            \
                     names_for(637),\n            \
                     \"the first breed is the type's default list\"\n        \
                 );\n        \
                 assert_eq!(\n            \
                     names_for_variation(637, 99),\n            \
                     names_for(637),\n            \
                     \"an impossible breed falls back rather than panicking\"\n        \
                 );\n    \
             }\n\
         }\n"
            .to_string(),
    );

    format!("{}\n", lines.join("\n"))
}
