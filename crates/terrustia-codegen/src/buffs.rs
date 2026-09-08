//! `crates/terrustia-proto/src/buffs.rs` — which buffs are debuffs, whip marks, removable,
//! frozen-timer or PvP-spreadable, and what each NPC type is immune to.
//!
//! Four things live here, all per-type data:
//!
//! * `Main.debuff` (`Terraria/Main.cs`) — which buff ids are debuffs, which decides what
//!   `AddBuff` may evict.
//! * `BuffID.Sets.IsAnNPCWhipDebuff`, `CanBeRemovedByNetMessage`, `TimeLeftDoesNotDecrease`
//!   (`Terraria.ID/BuffID.cs`).
//! * `NPCID.Sets.DebuffImmunitySets` plus `ShimmerImmunity` (`Terraria.ID/NPCID.cs`) — what each
//!   NPC type is immune to, with the three corrections `NPC.SetDefaults` applies on top.
//! * `Main.pvpBuff` (`Terraria/Main.cs`) — which buffs a PvP-flagged player may spread, gating
//!   packet 55.
//!
//! Ported from `gen_buffs.py`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

use regex::Regex;

use crate::csharp::{bool_set, read_lossy};

/// One `DebuffImmunitySets` entry that is not `null`.
struct ImmunityData {
    whips: bool,
    all: bool,
    specific: Vec<i64>,
}

/// `NPCID.Sets.DebuffImmunitySets`: `{ N, null }` or `{ N, new NPCDebuffImmunityData { ... } }`,
/// parsed by walking braces rather than by understanding C#.
fn parse_debuff_immunity_sets(npc_cs: &str) -> BTreeMap<i64, Option<ImmunityData>> {
    let start = npc_cs
        .find("public static Dictionary<int, NPCDebuffImmunityData> DebuffImmunitySets")
        .expect("no DebuffImmunitySets");
    let eq_pos = npc_cs[start..]
        .find('=')
        .expect("no = after DebuffImmunitySets")
        + start;
    let open_at = npc_cs[eq_pos..].find('{').expect("no { after =") + eq_pos;

    // Walk braces from the opening `{` of the initialiser to its match.
    let mut depth: i32 = 0;
    let mut end = open_at;
    for (off, ch) in npc_cs[open_at..].char_indices() {
        let abs = open_at + off;
        if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                end = abs;
                break;
            }
        }
    }
    let body = &npc_cs[open_at + 1..end];

    // Each entry is either `{ N, null },` or `{ N, new NPCDebuffImmunityData { ... } }`.
    let entry_re = Regex::new(r"\{\s*(\d+),\s*").unwrap();
    let specific_re = Regex::new(r"SpecificallyImmuneTo = new int\[\d+\]\s*\{([^}]*)\}").unwrap();
    let digit_re = Regex::new(r"\d+").unwrap();

    let mut entries: BTreeMap<i64, Option<ImmunityData>> = BTreeMap::new();
    let mut pos: usize = 0;
    while let Some(m) = entry_re.captures_at(body, pos) {
        let whole = m.get(0).unwrap();
        let npc_type: i64 = m[1].parse().unwrap();
        let rest = &body[whole.end()..];
        if rest.trim_start().starts_with("null") {
            entries.insert(npc_type, None);
            let null_idx = rest.find("null").unwrap();
            pos = whole.end() + null_idx + 4;
            continue;
        }
        // Find the NPCDebuffImmunityData initialiser block and match its braces.
        let init = rest.find('{').expect("no immunity data initialiser");
        let mut d: i32 = 0;
        let mut close: Option<usize> = None;
        for (off, ch) in rest[init..].char_indices() {
            let j = init + off;
            if ch == '{' {
                d += 1;
            } else if ch == '}' {
                d -= 1;
                if d == 0 {
                    close = Some(j);
                    break;
                }
            }
        }
        let j = close.expect("unbalanced braces in DebuffImmunitySets entry");
        let inner = &rest[init + 1..j];
        pos = whole.end() + j;

        let whips = inner.contains("ImmuneToWhips = true");
        let all = inner.contains("ImmuneToAllBuffsThatAreNotWhips = true");
        let mut specific: Vec<i64> = Vec::new();
        if let Some(sm) = specific_re.captures(inner) {
            specific = digit_re
                .find_iter(&sm[1])
                .map(|mm| mm.as_str().parse().unwrap())
                .collect();
        }
        entries.insert(
            npc_type,
            Some(ImmunityData {
                whips,
                all,
                specific,
            }),
        );
    }
    entries
}

/// The set of buff ids `npc_type` cannot be given, as `NPC.SetDefaults` builds it.
fn immunity_mask(
    npc_type: i64,
    entries: &BTreeMap<i64, Option<ImmunityData>>,
    whip: &BTreeSet<i64>,
    shimmer_immune: &BTreeSet<i64>,
    buff_count: i64,
) -> BTreeSet<i64> {
    let data = entries.get(&npc_type).and_then(|o| o.as_ref());
    let mut immune: BTreeSet<i64> = BTreeSet::new();
    if let Some(d) = data {
        if d.whips || d.all {
            for b in 1..buff_count {
                let is_whip = whip.contains(&b);
                if (is_whip && d.whips) || (!is_whip && d.all) {
                    immune.insert(b);
                }
            }
        }
        immune.extend(d.specific.iter().copied());
    }
    // The three corrections SetDefaults applies afterwards, in its order.
    if immune.contains(&20) {
        immune.insert(30);
        immune.insert(375);
    }
    if immune.contains(&69) {
        immune.insert(36);
    }
    if shimmer_immune.contains(&npc_type) {
        immune.insert(353);
    } else {
        immune.remove(&353);
    }
    immune
}

/// A buff-id set as a little-endian bitmap of u64 words.
fn bits(ids: &[i64], buff_count: i64) -> Vec<u64> {
    let mut words = vec![0u64; ((buff_count + 63) / 64) as usize];
    for &b in ids {
        if (0..buff_count).contains(&b) {
            words[(b / 64) as usize] |= 1u64 << (b % 64);
        }
    }
    words
}

/// One `pub const NAME: [bool; COUNT]` table, 16 values per row. `cargo fmt` reflows it
/// afterward, so the wrapping here only needs to match the emitted content, not its shape.
fn bool_table(name: &str, members: &BTreeSet<i64>, count: i64, doc: &str) -> String {
    let mut lines: Vec<String> = vec![
        doc.to_string(),
        format!("pub const {name}: [bool; {count}] = ["),
    ];
    let mut row: Vec<&str> = Vec::new();
    for i in 0..count {
        row.push(if members.contains(&i) {
            "true,"
        } else {
            "false,"
        });
        if row.len() == 16 {
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
    let main_cs = read_lossy(&root.join("Terraria/Main.cs"));
    let buff_cs = read_lossy(&root.join("Terraria.ID/BuffID.cs"));
    let npc_cs = read_lossy(&root.join("Terraria.ID/NPCID.cs"));

    let buff_count: i64 = Regex::new(r"public static readonly int Count = (\d+);")
        .unwrap()
        .captures(&buff_cs)
        .expect("no BuffID.Count")[1]
        .parse()
        .unwrap();

    let debuff: BTreeSet<i64> = Regex::new(r"\n\t\tdebuff\[(\d+)\] = true;")
        .unwrap()
        .captures_iter(&main_cs)
        .map(|c| c[1].parse().unwrap())
        .collect();
    assert!(!debuff.is_empty(), "no debuff entries found");

    let pvp_buff: BTreeSet<i64> = Regex::new(r"\n\t\tpvpBuff\[(\d+)\] = true;")
        .unwrap()
        .captures_iter(&main_cs)
        .map(|c| c[1].parse().unwrap())
        .collect();
    assert!(!pvp_buff.is_empty(), "no pvpBuff entries found");

    let whip: BTreeSet<i64> = bool_set(&buff_cs, "IsAnNPCWhipDebuff")
        .into_iter()
        .map(i64::from)
        .collect();
    let removable: BTreeSet<i64> = bool_set(&buff_cs, "CanBeRemovedByNetMessage")
        .into_iter()
        .map(i64::from)
        .collect();
    let frozen_time: BTreeSet<i64> = bool_set(&buff_cs, "TimeLeftDoesNotDecrease")
        .into_iter()
        .map(i64::from)
        .collect();
    let shimmer_immune: BTreeSet<i64> = bool_set(&npc_cs, "ShimmerImmunity")
        .into_iter()
        .map(i64::from)
        .collect();

    let entries = parse_debuff_immunity_sets(&npc_cs);
    assert!(
        entries.len() >= 500,
        "only {} immunity entries; the parse is wrong",
        entries.len()
    );

    let npc_count = entries.keys().next_back().copied().unwrap() + 1;
    let masks: Vec<BTreeSet<i64>> = (0..npc_count)
        .map(|t| immunity_mask(t, &entries, &whip, &shimmer_immune, buff_count))
        .collect();

    // Most types share one of a handful of masks, so intern them and index per type.
    let mut unique: BTreeMap<Vec<i64>, usize> = BTreeMap::new();
    let mut order: Vec<Vec<i64>> = Vec::new();
    let mut index: Vec<usize> = Vec::with_capacity(npc_count as usize);
    for mask in &masks {
        let key: Vec<i64> = mask.iter().copied().collect();
        let idx = *unique.entry(key.clone()).or_insert_with(|| {
            let idx = order.len();
            order.push(key.clone());
            idx
        });
        index.push(idx);
    }

    let mut chunks: Vec<String> = Vec::new();

    chunks.push(format!(
        "//! Buff tables, generated from the game's own.\n\
         //!\n\
         //! Nothing here is an algorithm. The rules that read these live in `game::npc` and\n\
         //! `game::server`; what varies per buff id or per NPC type is data, and data belongs in a\n\
         //! table rather than in a hand-written match a later version would silently invalidate.\n\
         //!\n\
         //! Generated by `terrustia-codegen` from Terraria 1.4.5.8. Do not edit by hand.\n\
         \n\
         /// How many buff ids exist. `BuffID.Count`.\n\
         pub const BUFF_COUNT: usize = {buff_count};\n"
    ));

    chunks.push(bool_table(
        "DEBUFF",
        &debuff,
        buff_count,
        "\n/// Whether a buff id is a debuff, from `Main.debuff`.\n\
         ///\n\
         /// `AddBuff` reads this to decide what it may evict when an NPC's twenty slots are full: a\n\
         /// good buff can be pushed out to make room, a debuff never can.",
    ));

    chunks.push(bool_table(
        "WHIP_MARK",
        &whip,
        buff_count,
        "\n/// Whether a buff id is a whip's mark, from `BuffID.Sets.IsAnNPCWhipDebuff`.\n\
         ///\n\
         /// Immunity treats the two kinds separately — several bosses shrug off every debuff but can\n\
         /// still be tagged by a whip — so the distinction has to be kept.",
    ));

    chunks.push(bool_table(
        "REMOVABLE_BY_REQUEST",
        &removable,
        buff_count,
        "\n/// Whether a client may ask the server to take a buff off an NPC, from\n\
         /// `BuffID.Sets.CanBeRemovedByNetMessage`.\n\
         ///\n\
         /// Empty in this version, and deliberately so: the packet exists and the game validates\n\
         /// against this set, so every request is refused. Keeping the table means a later version\n\
         /// that fills it in needs no code change.",
    ));

    chunks.push(bool_table(
        "TIME_DOES_NOT_DECREASE",
        &frozen_time,
        buff_count,
        "\n/// Buffs whose timer does not run down, from `BuffID.Sets.TimeLeftDoesNotDecrease`.",
    ));

    chunks.push(bool_table(
        "PVP_BUFF",
        &pvp_buff,
        buff_count,
        "\n/// Buffs a PvP-flagged player may spread to another PvP-flagged player, from `Main.pvpBuff`.\n\
         ///\n\
         /// Gates packet 55 (`AddPlayerBuffPvP`): a hostile-marked player who lands one of these on\n\
         /// another hostile-marked player asks the server to relay it, rather than the target's own\n\
         /// client trusting the attacker's client directly.",
    ));

    let words_per = (buff_count + 63) / 64;
    chunks.push(format!(
        "\n/// The distinct immunity masks, as bitmaps over buff ids.\n\
         ///\n\
         /// Six hundred and ninety-one NPC types share far fewer than that many distinct sets of\n\
         /// immunities, so the masks are interned and [`IMMUNITY_OF`] indexes into them.\n\
         const MASKS: [[u64; {words_per}]; {}] = [",
        order.len()
    ));
    for key in &order {
        let w = bits(key, buff_count);
        let hex: Vec<String> = w.iter().map(|x| format!("0x{x:016x}")).collect();
        chunks.push(format!("    [{}],", hex.join(", ")));
    }
    chunks.push("];".to_string());

    chunks.push(format!(
        "\n/// Which mask each NPC type uses, from `NPCID.Sets.DebuffImmunitySets` with the corrections\n\
         /// `NPC.SetDefaults` applies on top (poison implies bleeding and hemorrhage, ichor implies\n\
         /// broken armour, and shimmer immunity is set from its own list either way).\n\
         const IMMUNITY_OF: [u16; {npc_count}] = ["
    ));
    let mut row: Vec<String> = Vec::new();
    for &idx in &index {
        row.push(format!("{idx},"));
        if row.len() == 24 {
            chunks.push(format!("    {}", row.join(" ")));
            row.clear();
        }
    }
    if !row.is_empty() {
        chunks.push(format!("    {}", row.join(" ")));
    }
    chunks.push("];".to_string());

    chunks.push(
        "\n/// Whether `npc_type` can be given `buff`.\n\
         ///\n\
         /// An unknown type is immune to nothing, which matches the game: `SetDefaults` clears the\n\
         /// whole array for a type with no entry.\n\
         pub fn npc_is_immune(npc_type: u16, buff: u16) -> bool {\n    \
             let buff = buff as usize;\n    \
             if buff == 0 || buff >= BUFF_COUNT {\n        \
                 return true;\n    \
             }\n    \
             let Some(&slot) = IMMUNITY_OF.get(npc_type as usize) else {\n        \
                 return false;\n    \
             };\n    \
             MASKS[slot as usize][buff / 64] >> (buff % 64) & 1 == 1\n\
         }\n\
         \n\
         /// Whether a buff id is one the game counts as a debuff.\n\
         pub fn is_debuff(buff: u16) -> bool {\n    \
             DEBUFF.get(buff as usize).copied().unwrap_or(false)\n\
         }\n\
         \n\
         /// Whether a PvP-flagged player may spread `buff` to another over packet 55.\n\
         pub fn is_pvp_spreadable(buff: u16) -> bool {\n    \
             PVP_BUFF.get(buff as usize).copied().unwrap_or(false)\n\
         }\n"
            .to_string(),
    );

    format!("{}\n", chunks.join("\n"))
}
