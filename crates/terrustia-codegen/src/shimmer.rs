//! `crates/terrustia-proto/src/shimmer.rs` — what shimmer turns things into.
//!
//! Four tables, all `Factory.CreateIntSet`/`CreateBoolSet` declarations:
//! `ItemID.Sets.ShimmerTransformToItem`, `ItemID.Sets.ShimmerCountsAsItem`,
//! `ItemID.Sets.CommonCoin` (`Terraria.ID/ItemID.cs`), and `NPCID.Sets.ShimmerTransformToNPC`
//! (`Terraria.ID/NPCID.cs`).
//!
//! Ported from `gen_shimmer.py`.
//!
//! The module doc's decraft paragraph was reconciled to what commit `78d07de` ("Split the
//! licence, and make the lints and licences enforceable") hand-edited into the committed `.rs`
//! without updating `gen_shimmer.py`'s own docstring to match: decraft was implemented by then
//! ([`crate::recipes`]), so the emitted text says so, rather than the stale "deliberately
//! absent" note the old docstring still carried.

use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;

use crate::csharp::{bool_set, read_lossy};

/// `Factory.CreateIntSet(default, key, value, key, value, ...)` as `(default, map)`.
fn int_set(text: &str, name: &str) -> (i64, BTreeMap<i64, i64>) {
    let re = Regex::new(&format!(
        r"(?s)public static int\[\] {} = Factory\.CreateIntSet\(([^;]*?)\);",
        regex::escape(name)
    ))
    .unwrap();
    let body = &re
        .captures(text)
        .unwrap_or_else(|| panic!("no int set {name}"))[1];
    let num_re = Regex::new(r"-?\d+").unwrap();
    let nums: Vec<i64> = num_re
        .find_iter(body)
        .map(|m| m.as_str().parse().unwrap())
        .collect();
    let default = nums[0];
    let rest = &nums[1..];
    assert!(
        rest.len().is_multiple_of(2),
        "{name} has an odd tail; the parse is wrong"
    );
    let mut map = BTreeMap::new();
    let mut i = 0;
    while i < rest.len() {
        map.insert(rest[i], rest[i + 1]);
        i += 2;
    }
    (default, map)
}

/// A sorted `(key, value)` table, which is much smaller than a full array for a sparse set.
fn pairs_table(
    name: &str,
    mapping: &BTreeMap<i64, i64>,
    count: i64,
    ty: &str,
    doc: &str,
) -> String {
    let rows: Vec<(i64, i64)> = mapping
        .iter()
        .filter(|&(&k, &v)| (0..count).contains(&k) && v > 0)
        .map(|(&k, &v)| (k, v))
        .collect();
    let mut lines: Vec<String> = vec![
        doc.to_string(),
        format!("const {name}: [({ty}, {ty}); {}] = [", rows.len()),
    ];
    let mut row: Vec<String> = Vec::new();
    for &(k, v) in &rows {
        row.push(format!("({k},{v}),"));
        if row.len() == 8 {
            lines.push(format!("    {}", row.join(" ")));
            row.clear();
        }
    }
    if !row.is_empty() {
        lines.push(format!("    {}", row.join(" ")));
    }
    lines.push("];".to_string());
    lines.join("\n")
}

pub fn generate(root: &Path) -> String {
    let item_cs = read_lossy(&root.join("Terraria.ID/ItemID.cs"));
    let npc_cs = read_lossy(&root.join("Terraria.ID/NPCID.cs"));

    let count_re = Regex::new(r"public static readonly (?:short|int) Count = (\d+);").unwrap();
    let item_count: i64 = count_re.captures(&item_cs).expect("no ItemID.Count")[1]
        .parse()
        .unwrap();
    let npc_count: i64 = count_re.captures(&npc_cs).expect("no NPCID.Count")[1]
        .parse()
        .unwrap();

    let (_, transform) = int_set(&item_cs, "ShimmerTransformToItem");
    let (_, counts_as) = int_set(&item_cs, "ShimmerCountsAsItem");
    let coins: Vec<i64> = bool_set(&item_cs, "CommonCoin")
        .into_iter()
        .map(i64::from)
        .collect();
    let (_, to_npc) = int_set(&npc_cs, "ShimmerTransformToNPC");

    assert!(
        transform.len() >= 200,
        "only {} transforms parsed; the set's shape changed",
        transform.len()
    );

    let copper_coin = coins.iter().min().copied().unwrap_or(71);

    let mut chunks: Vec<String> = Vec::new();

    chunks.push(
        "//! What shimmer turns things into, generated from the game's own tables.\n\
         //!\n\
         //! Shimmer transmutes. An item dropped into it becomes another item, or a creature, or — for\n\
         //! coins — luck. Which of those, and into what, is per-item data running to several hundred\n\
         //! pairs, so it lives here rather than anywhere near hand-written code.\n\
         //!\n\
         //! The other branch of the game's shimmer is **decrafting**: an item with no transform and no\n\
         //! creature is broken into its recipe's ingredients instead. That is implemented — it needed the\n\
         //! recipe database as a table, which is [`crate::recipes`], and the server drives it through\n\
         //! `decraft_recipe`. See `docs/shimmer.md` for how a cascade terminates.\n\
         //!\n\
         //! (This comment said decrafting was \"deliberately absent\" and \"a system this server does not\n\
         //! model at all\" long after it had been built, which is worth a moment's thought: a stale note\n\
         //! misleads more effectively than no note, because it reads like a decision.)\n\
         //!\n\
         //! Generated by `terrustia-codegen` from Terraria 1.4.5.8. Do not edit by hand.\n"
            .to_string(),
    );

    chunks.push(pairs_table(
        "TRANSFORMS",
        &transform,
        item_count,
        "u16",
        "\n/// `ItemID.Sets.ShimmerTransformToItem`: what an item becomes.\n\
         ///\n\
         /// Sorted by item id so it can be searched rather than indexed — it is a few hundred pairs out\n\
         /// of several thousand items, and a full array would be mostly zeroes.",
    ));

    chunks.push(pairs_table(
        "COUNTS_AS",
        &counts_as,
        item_count,
        "u16",
        "\n/// `ItemID.Sets.ShimmerCountsAsItem`: items that shimmer as though they were another.\n\
         ///\n\
         /// Checked first, so a variant transmutes the way the thing it stands in for does.",
    ));

    chunks.push(pairs_table(
        "NPC_TRANSFORMS",
        &to_npc,
        npc_count,
        "u16",
        "\n/// `NPCID.Sets.ShimmerTransformToNPC`: a creature let out of a jar becomes another creature.",
    ));

    chunks.push(format!(
        "\n/// The coins, whose shimmer is luck rather than an item. `ItemID.Sets.CommonCoin`.\npub const COPPER_COIN: u16 = {copper_coin};\n"
    ));

    chunks.push(
        "\nfn look_up(table: &[(u16, u16)], key: u16) -> Option<u16> {\n    \
             table\n        \
                 .binary_search_by_key(&key, |&(k, _)| k)\n        \
                 .ok()\n        \
                 .map(|at| table[at].1)\n\
         }\n\
         \n\
         /// The item this one shimmers *as*, which is usually itself.\n\
         ///\n\
         /// A handful of variants stand in for another item so they transmute the way it does. The game\n\
         /// calls this `GetShimmerEquivalentType`.\n\
         pub fn equivalent(item: u16) -> u16 {\n    \
             look_up(&COUNTS_AS, item).unwrap_or(item)\n\
         }\n\
         \n\
         /// What an item becomes in shimmer, if it becomes another item.\n\
         pub fn transforms_into(item: u16) -> Option<u16> {\n    \
             look_up(&TRANSFORMS, equivalent(item))\n\
         }\n\
         \n\
         /// What a caught creature becomes when its jar goes in.\n\
         pub fn npc_becomes(npc_type: u16) -> Option<u16> {\n    \
             look_up(&NPC_TRANSFORMS, npc_type)\n\
         }\n\
         \n\
         /// Whether an item is a coin, whose shimmer is coin luck rather than a transformation.\n\
         ///\n\
         /// The four are consecutive and always have been, which is why this is a range rather than a\n\
         /// table of four entries.\n\
         pub fn is_coin(item: u16) -> bool {\n    \
             (COPPER_COIN..COPPER_COIN + 4).contains(&item)\n\
         }\n\
         \n\
         /// What a stack of coins is worth in coppers, for the luck it grants.\n\
         ///\n\
         /// Silver is a hundred coppers, gold ten thousand, platinum a million — and platinum is capped\n\
         /// at one, which is the game's own rule and stops a stack of them being worth a billion.\n\
         pub fn coin_luck(item: u16, stack: i32) -> i32 {\n    \
             match item.checked_sub(COPPER_COIN) {\n        \
                 Some(0) => stack,\n        \
                 Some(1) => stack.saturating_mul(100),\n        \
                 Some(2) => stack.saturating_mul(10_000),\n        \
                 Some(3) => 1_000_000,\n        \
                 _ => 0,\n    \
             }\n\
         }\n\
         \n\
         #[cfg(test)]\n\
         mod tests {\n    \
             use super::*;\n\
             \n    \
             /// Both tables are sorted, or the binary searches above silently miss.\n    \
             #[test]\n    \
             fn the_tables_are_sorted() {\n        \
                 for (name, table) in [\n            \
                     (\"TRANSFORMS\", &TRANSFORMS[..]),\n            \
                     (\"COUNTS_AS\", &COUNTS_AS[..]),\n            \
                     (\"NPC_TRANSFORMS\", &NPC_TRANSFORMS[..]),\n        \
                 ] {\n            \
                     assert!(\n                \
                         table.windows(2).all(|w| w[0].0 < w[1].0),\n                \
                         \"{name} must be strictly ascending by key\"\n            \
                     );\n        \
                 }\n    \
             }\n\
             \n    \
             /// Spot checks against transmutations that are well known.\n    \
             #[test]\n    \
             fn the_known_transmutations_hold() {\n        \
                 // A Gold Chest's key mould line, and the ore/bar pairs, are the ones players learn first.\n        \
                 assert_eq!(transforms_into(9), Some(2), \"wood becomes stone\");\n        \
                 assert!(transforms_into(1).is_none(), \"an iron pickaxe does not\");\n    \
             }\n\
             \n    \
             /// Coins are luck, and the four of them are worth what they are worth.\n    \
             #[test]\n    \
             fn coins_are_worth_their_weight() {\n        \
                 assert!(is_coin(COPPER_COIN));\n        \
                 assert!(is_coin(COPPER_COIN + 3));\n        \
                 assert!(!is_coin(COPPER_COIN + 4));\n        \
                 assert_eq!(coin_luck(COPPER_COIN, 50), 50);\n        \
                 assert_eq!(coin_luck(COPPER_COIN + 1, 5), 500);\n        \
                 assert_eq!(coin_luck(COPPER_COIN + 2, 5), 50_000);\n        \
                 assert_eq!(\n            \
                     coin_luck(COPPER_COIN + 3, 99),\n            \
                     1_000_000,\n            \
                     \"platinum is capped at one, however many are thrown in\"\n        \
                 );\n        \
                 assert_eq!(coin_luck(1, 5), 0, \"and a pickaxe is not a coin\");\n    \
             }\n\
             \n    \
             /// A stack big enough to overflow is saturated rather than wrapped.\n    \
             #[test]\n    \
             fn an_absurd_stack_does_not_wrap() {\n        \
                 assert_eq!(coin_luck(COPPER_COIN + 2, i32::MAX), i32::MAX);\n    \
             }\n\
             \n    \
             /// An item that stands in for another transmutes the way that other one does.\n    \
             #[test]\n    \
             fn a_stand_in_follows_what_it_stands_for() {\n        \
                 for &(variant, real) in COUNTS_AS.iter() {\n            \
                     assert_eq!(\n                \
                         equivalent(variant),\n                \
                         real,\n                \
                         \"item {variant} should shimmer as {real}\"\n            \
                     );\n            \
                     assert_eq!(transforms_into(variant), transforms_into(real));\n        \
                 }\n    \
             }\n\
         }\n"
            .to_string(),
    );

    format!("{}\n", chunks.join("\n"))
}
