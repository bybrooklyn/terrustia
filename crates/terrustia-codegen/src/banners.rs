//! `crates/terrustia-proto/src/banners.rs` — which enemies have a banner, and how many kills each
//! needs.
//!
//! Three tables, all per-type data rather than an algorithm:
//!
//! * `NPCtoBanner` (`Terraria.GameContent/BannerSystem.cs`) — a switch of a few hundred cases,
//!   NPC type to banner index.
//! * `BannerToItem` (same file) — banner index to item, written as a ladder of ranges.
//! * `KillsToBanner` (`Terraria.ID/ItemID.cs`) — how many kills each banner needs, fifty by
//!   default with a few dozen overrides.
//!
//! Ported from `gen_banners.py`.

use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;

use crate::csharp::read_lossy;

/// `case <npc>: return <banner>;`, allowing several cases to share one return.
fn parse_npc_to_banner(root: &Path) -> BTreeMap<i64, i64> {
    let text = read_lossy(&root.join("Terraria.GameContent/BannerSystem.cs"));
    let start = text
        .find("public static int NPCtoBanner")
        .expect("no NPCtoBanner");
    let body_from = &text[start..];
    // The tenth character on is where the search for the next `\n\tpublic ` begins, mirroring
    // `body.index("\n\tpublic ", 10)` in the Python original.
    let end_rel = body_from[10..]
        .find("\n\tpublic ")
        .expect("no method boundary after NPCtoBanner")
        + 10;
    let body = &body_from[..end_rel];

    // `re.match` in the original only anchors at the start, not the end, so these are too.
    let case_re = Regex::new(r"^case (-?\d+):").unwrap();
    let return_re = Regex::new(r"^return (\d+);").unwrap();

    let mut out: BTreeMap<i64, i64> = BTreeMap::new();
    let mut pending: Vec<i64> = Vec::new();
    for raw in body.lines() {
        let line = raw.trim();
        if let Some(c) = case_re.captures(line) {
            pending.push(c[1].parse().unwrap());
        } else if let Some(c) = return_re.captures(line) {
            let banner: i64 = c[1].parse().unwrap();
            for &npc in &pending {
                if npc >= 0 {
                    out.insert(npc, banner);
                }
            }
            pending.clear();
        } else if line == "default:" || line == "}" {
            pending.clear();
        }
    }
    out
}

/// One rung of `BannerSystem.BannerToItem`'s ladder, in source order.
#[derive(Debug, Clone, Copy)]
enum Rung {
    /// `if (banner == n) return item;`
    Exact { banner: i64, item: i64 },
    /// `if (banner >= from) return base + banner - from;`
    Run { from: i64, base: i64 },
    /// The method's closing `return base + banner - from;`, with no condition above it.
    Fallback { from: i64, base: i64 },
}

/// `BannerSystem.BannerToItem`, read out of source rather than remembered.
///
/// This ladder used to be a Rust string literal in `emit` below, and it had silently fallen four
/// rungs behind the game: 1.4.5's `banner == 290`, `banner == 289` and `banner >= 276` were all
/// missing, so fifteen banner indices fell through to the `>= 274` run and handed over the wrong
/// item (fifty Orca kills paid out a Quad Barrel Shotgun). Parsing it means the next block of
/// banner items lands on its own.
fn parse_banner_to_item(root: &Path) -> Vec<Rung> {
    let text = read_lossy(&root.join("Terraria.GameContent/BannerSystem.cs"));
    let start = text
        .find("public static int BannerToItem")
        .expect("no BannerToItem");
    let body_from = &text[start..];
    let end_rel = body_from[10..]
        .find("\n\tpublic ")
        .expect("no method boundary after BannerToItem")
        + 10;
    let body = &body_from[..end_rel];

    let eq_re = Regex::new(r"^if \(banner == (\d+)\)$").unwrap();
    let ge_re = Regex::new(r"^if \(banner >= (\d+)\)$").unwrap();
    let const_re = Regex::new(r"^return (\d+);$").unwrap();
    let run_re = Regex::new(r"^return (\d+) \+ banner - (\d+);$").unwrap();

    let mut rungs = Vec::new();
    // The condition seen but not yet consumed by its `return`: `(is_a_range, value)`.
    let mut pending: Option<(bool, i64)> = None;
    for raw in body.lines() {
        let line = raw.trim();
        if let Some(c) = eq_re.captures(line) {
            pending = Some((false, c[1].parse().unwrap()));
        } else if let Some(c) = ge_re.captures(line) {
            pending = Some((true, c[1].parse().unwrap()));
        } else if let Some(c) = const_re.captures(line) {
            let item: i64 = c[1].parse().unwrap();
            let (is_range, banner) = pending
                .take()
                .unwrap_or_else(|| panic!("bare `return {item};` in BannerToItem"));
            assert!(!is_range, "a range rung returning a single item");
            rungs.push(Rung::Exact { banner, item });
        } else if let Some(c) = run_re.captures(line) {
            let base: i64 = c[1].parse().unwrap();
            let from: i64 = c[2].parse().unwrap();
            match pending.take() {
                Some((true, at)) => {
                    assert_eq!(at, from, "a run that does not start where it is tested");
                    rungs.push(Rung::Run { from, base });
                }
                None => rungs.push(Rung::Fallback { from, base }),
                Some((false, at)) => panic!("`banner == {at}` returning a run"),
            }
        }
    }
    assert!(
        matches!(rungs.last(), Some(Rung::Fallback { .. })),
        "BannerToItem's ladder has no closing fallback"
    );
    rungs
}

/// The default threshold and the per-item overrides.
fn parse_kills(root: &Path) -> (i64, BTreeMap<i64, i64>) {
    let text = read_lossy(&root.join("Terraria.ID/ItemID.cs"));
    let default: i64 = Regex::new(r"DefaultKillsForBannerNeeded = (\d+);")
        .unwrap()
        .captures(&text)
        .expect("no DefaultKillsForBannerNeeded")[1]
        .parse()
        .unwrap();
    let line = &Regex::new(r"KillsToBanner = Factory\.CreateIntSet\(([^;]*)\);")
        .unwrap()
        .captures(&text)
        .expect("no KillsToBanner")[1];
    // The first argument is the *name* `DefaultKillsForBannerNeeded`, not a number, so every
    // digit on the line is already part of an (item, kills) pair. Skipping one shifts the whole
    // list and pairs each item with the next item's threshold.
    let numbers: Vec<i64> = Regex::new(r"-?\d+")
        .unwrap()
        .find_iter(line)
        .map(|m| m.as_str().parse().unwrap())
        .collect();
    assert!(
        numbers.len().is_multiple_of(2),
        "KillsToBanner has {} numbers, which cannot pair up",
        numbers.len()
    );
    let mut overrides: BTreeMap<i64, i64> = BTreeMap::new();
    let mut i = 0;
    while i < numbers.len() {
        overrides.insert(numbers[i], numbers[i + 1]);
        i += 2;
    }
    (default, overrides)
}

fn emit(
    npc_banner: &BTreeMap<i64, i64>,
    ladder: &[Rung],
    default: i64,
    overrides: &BTreeMap<i64, i64>,
) -> String {
    let rows: Vec<(i64, i64)> = npc_banner.iter().map(|(&k, &v)| (k, v)).collect();
    let mut lines: Vec<String> = vec![
        "//! Which enemies have a banner, and how many of them it takes to earn one.".into(),
        "//!".into(),
        "//! Nothing counted kills before this, so the banner rewards never arrived and the world"
            .into(),
        "//! file's banner section was written as two zeroes. Not progression — but one of the few"
            .into(),
        "//! absences a player notices directly, because killing a hundred of something is supposed"
            .into(),
        "//! to be remarked upon.".into(),
        "//!".into(),
        "//! Generated by `terrustia-codegen` from Terraria 1.4.5.8. Do not edit by hand.".into(),
        "".into(),
        "/// Kills needed when a banner says nothing else.".into(),
        format!("pub const DEFAULT_KILLS: u32 = {default};"),
        "".into(),
        "/// How many enemy types have a banner at all.".into(),
        format!("pub const WITH_BANNERS: usize = {};", rows.len()),
        "".into(),
        "/// The banner index for an enemy type, if it has one.".into(),
        "pub fn banner_of(npc_type: u16) -> Option<u16> {".into(),
        "    Some(match npc_type {".into(),
    ];
    for &(npc, banner) in &rows {
        lines.push(format!("        {npc} => {banner},"));
    }
    lines.extend(
        [
            "        _ => return None,",
            "    })",
            "}",
            "",
            "/// The item a banner index hands over.",
            "///",
            "/// A ladder of ranges rather than a table, exactly as `BannerSystem.BannerToItem` writes",
            "/// it — the banner items are laid out in runs across several id blocks.",
            "pub fn banner_item(banner: u16) -> u16 {",
            "    match banner {",
        ]
        .map(String::from),
    );
    for rung in ladder {
        lines.push(match *rung {
            Rung::Exact { banner, item } => format!("        {banner} => {item},"),
            Rung::Run { from, base } => format!("        b if b >= {from} => {base} + b - {from},"),
            Rung::Fallback { from, base } => format!("        b => {base} + b - {from},"),
        });
    }
    lines.extend(
        [
            "    }",
            "}",
            "",
            "/// How many kills that banner's item asks for.",
            "pub fn kills_needed(item: u16) -> u32 {",
            "    match item {",
        ]
        .map(String::from),
    );
    for (&item, &kills) in overrides {
        lines.push(format!("        {item} => {kills},"));
    }
    lines.extend(
        [
            "        _ => DEFAULT_KILLS,",
            "    }",
            "}",
            "",
            "#[cfg(test)]",
            "mod tests {",
            "    use super::*;",
            "",
            "    /// The table is populated and did not regenerate empty.",
            "    #[test]",
            "    fn the_table_is_populated() {",
        ]
        .map(String::from),
    );
    lines.push(format!("        assert_eq!(WITH_BANNERS, {});", rows.len()));
    lines.extend(
        [
            "    }",
            "",
            "    /// A zombie has a banner and a slime does not, which is the shape of the set.",
            "    #[test]",
            "    fn ordinary_enemies_have_banners() {",
            "        let zombie = banner_of(3).expect(\"a zombie has a banner\");",
            "        let item = banner_item(zombie);",
            "        assert!(item > 0);",
            "        assert!(kills_needed(item) >= 10);",
            "    }",
            "",
            "    /// Something with no banner says so rather than guessing one.",
            "    #[test]",
            "    fn not_everything_has_a_banner() {",
            "        assert!(banner_of(u16::MAX).is_none());",
            "    }",
            "",
            "    /// Every banner maps to a distinct item, or two enemies would share a reward.",
            "    #[test]",
            "    fn banners_do_not_collide() {",
            "        let mut items = std::collections::HashSet::new();",
            "        let mut banners: Vec<u16> = (0..700u16).filter_map(banner_of).collect();",
            "        banners.sort_unstable();",
            "        banners.dedup();",
            "        for banner in banners {",
            "            assert!(",
            "                items.insert(banner_item(banner)),",
            "                \"banner {banner} shares an item with another\",",
            "            );",
            "        }",
            "    }",
            "}",
            "",
        ]
        .map(String::from),
    );
    lines.join("\n")
}

pub fn generate(root: &Path) -> String {
    let npc_banner = parse_npc_to_banner(root);
    let ladder = parse_banner_to_item(root);
    let (default, overrides) = parse_kills(root);
    assert!(
        npc_banner.len() >= 100,
        "only parsed {} banners; the parser is wrong",
        npc_banner.len()
    );
    assert!(
        ladder.len() >= 10,
        "only parsed {} BannerToItem rungs; the parser is wrong",
        ladder.len()
    );
    emit(&npc_banner, &ladder, default, &overrides)
}
