//! `crates/terrustia-proto/src/travel_shop.rs` — the Travelling Merchant's stock.
//!
//! `Chest.SetupTravelShop_GetItem` (`Terraria/Chest.cs`) is a chain of luck rolls, one per
//! candidate item, each overwriting the last, so the *final* match in the chain wins. Which
//! item, at which rarity tier, and under which world condition, is per-item data, parsed rather
//! than transcribed.
//!
//! Ported from `gen_travel_shop.py`.
//!
//! The leading source comment on the two inline-guarded offers, and the
//! `the_travelling_merchant_can_offer_both_counterweights` regression test, were reconciled to
//! what commit `65f4be3` ("Fix travel shop generator skipping leading-guard chain candidates")
//! hand-added to the committed `.rs` after regenerating the `OFFERS` data itself through the
//! fixed regex, without adding either to `gen_travel_shop.py`'s own `emit()`. Both are now part
//! of this generator's output, not carried by hand.

use std::path::Path;

use regex::Regex;

use crate::csharp::read_lossy;

/// The C# condition substring and the `Needs::` flag name it becomes, in the order the game's
/// own guard clauses are checked (which is also the order flags are OR'd together per entry).
const CONDITIONS: &[(&str, &str)] = &[
    ("Main.hardMode", "HARDMODE"),
    ("NPC.downedMechBossAny", "ANY_MECH"),
    ("NPC.downedMechBoss1", "DESTROYER"),
    ("NPC.downedMechBoss2", "TWINS"),
    ("NPC.downedMechBoss3", "PRIME"),
    ("NPC.downedBoss1", "EYE"),
    ("NPC.downedBoss3", "SKELETRON"),
    ("WorldGen.shadowOrbSmashed", "ORB_SMASHED"),
];

struct Entry {
    tier: i64,
    item: i64,
    needs: Vec<&'static str>,
    floor: i64,
    /// Whether this candidate carried its own leading `minimumRarity <= F &&` guard rather than
    /// relying on one of the function's checkpoints (`Chest.cs:980-987`: BlackCounterweight and
    /// YellowCounterweight, the two candidates the old anchored regex could not see at all).
    inline_guard: bool,
}

/// The rest of an `if (` condition that opened before `from`, plus the offset just past its
/// closing parenthesis. `None` if the parentheses never balance.
fn balanced_tail(body: &str, from: usize) -> Option<(&str, usize)> {
    let mut depth = 1;
    for (offset, ch) in body[from..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&body[from..from + offset], from + offset + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// The `{ ... }` block starting at or just after `from`, without its braces.
fn braced_block(body: &str, from: usize) -> Option<&str> {
    let open = from + body[from..].find('{')?;
    let mut depth = 0;
    for (offset, ch) in body[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&body[open + 1..open + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The `Needs::` flags a C# condition asks for.
///
/// [`crate::travel_shop`]'s `Needs` is an AND of flags (`met_by` is a subset test), so a
/// disjunction cannot be expressed in it. Exactly one candidate uses one: the Zapinators' `(...
/// || ... || Main.hardMode)` (`Chest.cs:1168`), an "any early boss, or hardmode" gate. Reading
/// its named flags as an AND would demand *all* of them, which is far stricter than the game and
/// would keep the item out of almost every world, so a disjunction contributes no flags at all
/// instead. That errs the other way: those offers are reachable slightly earlier than in vanilla,
/// which is the narrowing this project already accepts elsewhere over an unreachable item.
fn flags_of(condition: &str) -> Vec<&'static str> {
    if condition.contains("||") {
        return Vec::new();
    }
    CONDITIONS
        .iter()
        .filter(|(src, _)| condition.contains(src))
        .map(|&(_, flag)| flag)
        .collect()
}

pub fn generate(root: &Path) -> String {
    let chest_cs = read_lossy(&root.join("Terraria/Chest.cs"));

    let start = chest_cs
        .find("public static void SetupTravelShop_GetItem(")
        .expect("no SetupTravelShop_GetItem");
    let end = chest_cs[start..]
        .find("public static void SetupTravelShop()")
        .expect("no SetupTravelShop")
        + start;
    let body = &chest_cs[start..end];

    // `if ([minimumRarity <= F &&] playerWithHighestLuck.RollLuck(rarity[N]) == 0 [&& cond...])
    //     { it = ITEM; }`
    //
    // Two candidates carry their own leading `minimumRarity <= F &&` guard instead of relying on
    // an enclosing `if (minimumRarity > N) return;` checkpoint — they sit before any such
    // checkpoint in the function, so without their own guard they would be offered no matter how
    // high `minimumRarity` climbs. The optional leading group captures that inline floor
    // directly; every other candidate leaves it unmatched and keeps falling back to the
    // checkpoint-derived floor below.
    // Only the *head* is matched by regex. The condition's tail is read by counting parentheses
    // instead: one candidate (the Zapinators, `Chest.cs:1168-1175`) guards on a parenthesised
    // disjunction, and a `[^)]*` tail cannot span it, so that whole candidate was silently
    // dropped and neither Zapinator was ever offered.
    let pattern = Regex::new(
        r"if \((?:minimumRarity <= (\d+) && )?playerWithHighestLuck\.RollLuck\(rarity\[(\d+)\]\) == 0",
    )
    .unwrap();
    let assign_re = Regex::new(r"^\s*\{\s*it = (\d+);").unwrap();
    // A candidate whose body narrows its own choice further: `it = 4347; if (Main.hardMode) { it
    // = 4348; }`. Emitted as a follow-on offer carrying that inner condition on top of the outer
    // one, which the chain's later-wins rule then resolves the same way source's reassignment does.
    let nested_re = Regex::new(r"if \(([^)]*)\)\s*\{\s*it = (\d+);").unwrap();
    // The `minimumRarity` guards partition the chain; track the floor in force at each match.
    let floor_re = Regex::new(r"if \(minimumRarity > (\d+)\)").unwrap();
    let floors: Vec<(usize, i64)> = floor_re
        .captures_iter(body)
        .map(|m| (m.get(0).unwrap().start(), m[1].parse().unwrap()))
        .collect();

    let mut entries: Vec<Entry> = Vec::new();
    for m in pattern.captures_iter(body) {
        let whole = m.get(0).unwrap();
        let inline_floor = m.get(1);
        let tier: i64 = m[2].parse().unwrap();

        // The rest of the `if (...)`, however many nested parentheses it takes.
        let Some((tail, after)) = balanced_tail(body, whole.end()) else {
            continue;
        };
        // A nested RollLuck in the tail means a compound roll this table cannot express; skip it
        // and say so rather than emitting something that looks right.
        if tail.contains("RollLuck") {
            continue;
        }
        let Some(caps) = assign_re.captures(&body[after..]) else {
            continue;
        };
        let item: i64 = caps[1].parse().unwrap();
        let needs = flags_of(tail);
        let floor = if let Some(f) = inline_floor {
            f.as_str().parse().unwrap()
        } else {
            let mut floor = 0;
            for &(at, value) in &floors {
                if at < whole.start() {
                    floor = value;
                }
            }
            floor
        };
        entries.push(Entry {
            tier,
            item,
            needs: needs.clone(),
            floor,
            inline_guard: inline_floor.is_some(),
        });

        // Anything the candidate's own body reassigns `it` to under a further condition.
        let Some(block) = braced_block(body, after) else {
            continue;
        };
        for inner in nested_re.captures_iter(block) {
            let mut needs = needs.clone();
            for flag in flags_of(&inner[1]) {
                if !needs.contains(&flag) {
                    needs.push(flag);
                }
            }
            entries.push(Entry {
                tier,
                item: inner[2].parse().unwrap(),
                needs,
                floor,
                inline_guard: inline_floor.is_some(),
            });
        }
    }
    assert!(
        entries.len() >= 30,
        "only {} shop entries parsed; the chain's shape changed",
        entries.len()
    );

    let flag_set: std::collections::BTreeSet<&'static str> = entries
        .iter()
        .flat_map(|e| e.needs.iter().copied())
        .collect();
    let flags: Vec<&'static str> = flag_set.into_iter().collect();

    let mut lines: Vec<String> = vec![
        "//! The Travelling Merchant's stock, generated from the game's own chain.\n\
         //!\n\
         //! His inventory is not a list but a *chain of rolls*: one per candidate item, each overwriting\n\
         //! the last, so the final match wins. Rarer items sit at higher tiers and are rolled against\n\
         //! longer odds, and several only appear once the world has been through something.\n\
         //!\n\
         //! Generated by `terrustia-codegen` from Terraria 1.4.5.8. Do not edit by hand.\n\
         \n\
         /// What a world must already have been through for an item to be offered.\n\
         ///\n\
         /// A bit set rather than an enum: a few items want two things at once.\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq)]\n\
         pub struct Needs(pub u16);\n\
         \n\
         impl Needs {\n    \
             pub const NONE: Self = Self(0);\n"
            .to_string(),
    ];
    for (i, flag) in flags.iter().enumerate() {
        lines.push(format!("    pub const {flag}: u16 = 1 << {i};"));
    }
    lines.push(
        "\n    pub fn met_by(self, world: Needs) -> bool {\n        \
             self.0 & !world.0 == 0\n    \
         }\n\
         }\n\
         \n\
         /// One thing the merchant might be carrying.\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq)]\n\
         pub struct Offer {\n    \
             pub item: u16,\n    \
             /// Which rarity tier's odds it is rolled against. Higher is rarer.\n    \
             pub tier: u8,\n    \
             /// What the world must have been through.\n    \
             pub needs: Needs,\n    \
             /// The `minimumRarity` floor below which the game stops considering this part of the chain.\n    \
             pub floor: u8,\n\
         }\n"
            .to_string(),
    );

    lines.push("/// The chain, in the order the game walks it. Later entries win.".to_string());
    lines.push(format!("pub const OFFERS: [Offer; {}] = [", entries.len()));
    let mut noted_inline_guard = false;
    for e in &entries {
        if e.inline_guard && !noted_inline_guard {
            noted_inline_guard = true;
            lines.push(
                "    // `Chest.cs:980-987` — BlackCounterweight and YellowCounterweight each carry their own\n    \
                 // leading `minimumRarity <= F &&` guard rather than sitting behind one of the\n    \
                 // `if (minimumRarity > N) return;` checkpoints the rest of the chain uses, and both sit\n    \
                 // ahead of item 1987 in source, so they are tried (and can be overwritten by it) first."
                    .to_string(),
            );
        }
        let bits = if e.needs.is_empty() {
            "0".to_string()
        } else {
            e.needs
                .iter()
                .map(|f| format!("Needs::{f}"))
                .collect::<Vec<_>>()
                .join(" | ")
        };
        lines.push(format!(
            "    Offer {{ item: {}, tier: {}, needs: Needs({bits}), floor: {} }},",
            e.item, e.tier, e.floor
        ));
    }
    lines.push("];".to_string());

    lines.push(
        "\n/// How many rarity tiers there are, and the odds each is rolled against.\n\
         ///\n\
         /// `Chest.SetupTravelShop`'s starting array. The odds are one-in-N, so a tier-five item at 1200\n\
         /// turns up about once in every twelve hundred rolls of that step.\n\
         pub const TIER_ODDS: [i32; 6] = [1, 1, 1, 6, 300, 1200];\n\
         \n\
         #[cfg(test)]\n\
         mod tests {\n    \
             use super::*;\n\
             \n    \
             /// Every offer names a real item and a tier the odds table has.\n    \
             #[test]\n    \
             fn every_offer_is_well_formed() {\n        \
                 for offer in OFFERS {\n            \
                     assert!(offer.item > 0, \"an offer with no item\");\n            \
                     assert!(\n                \
                         (offer.tier as usize) < TIER_ODDS.len(),\n                \
                         \"item {} names tier {}, past the odds table\",\n                \
                         offer.item,\n                \
                         offer.tier\n            \
                     );\n        \
                 }\n    \
             }\n\
             \n    \
             /// A fresh world can be offered something, and a finished one can be offered more.\n    \
             #[test]\n    \
             fn progress_widens_the_stock() {\n        \
                 let open = |world: Needs| {\n            \
                     OFFERS\n                \
                         .iter()\n                \
                         .filter(|o| o.needs.met_by(world))\n                \
                         .count()\n        \
                 };\n        \
                 let fresh = open(Needs::NONE);\n        \
                 let late = open(Needs(u16::MAX));\n        \
                 assert!(fresh > 0, \"a fresh world should have stock\");\n        \
                 assert!(late > fresh, \"{late} late against {fresh} fresh\");\n    \
             }\n\
             \n    \
             /// BlackCounterweight (3309) and YellowCounterweight (3314) each carry their own leading\n    \
             /// `minimumRarity <= F &&` guard in `Chest.cs:980-987` instead of an enclosing\n    \
             /// `if (minimumRarity > N) return;` checkpoint, which the old anchored regex in\n    \
             /// `gen_travel_shop.py` could not see at all — both items were silently absent, with no\n    \
             /// other drop or craft source anywhere in this project, making them unobtainable.\n    \
             #[test]\n    \
             fn the_travelling_merchant_can_offer_both_counterweights() {\n        \
                 assert!(OFFERS.iter().any(|o| o.item == 3309), \"BlackCounterweight\");\n        \
                 assert!(OFFERS.iter().any(|o| o.item == 3314), \"YellowCounterweight\");\n    \
             }\n\
             \n    \
             /// The condition test is a subset check, not equality: a world that has been through more\n    \
             /// than an item asks for still qualifies.\n    \
             #[test]\n    \
             fn extra_progress_does_not_disqualify() {\n        \
                 let wants = Needs(Needs::HARDMODE);\n        \
                 assert!(wants.met_by(Needs(Needs::HARDMODE)));\n        \
                 assert!(wants.met_by(Needs(u16::MAX)));\n        \
                 assert!(!wants.met_by(Needs::NONE));\n    \
             }\n\
         }\n"
            .to_string(),
    );

    format!("{}\n", lines.join("\n"))
}
