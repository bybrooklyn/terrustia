//! `crates/terrustia-proto/src/npc_drops.rs` — the *unconditional* half of the loot tables.
//!
//! Reads `Terraria.GameContent.ItemDropRules/ItemDropDatabase.cs` and keeps only the
//! constructors whose first four arguments are `(item, chanceDenominator, min, max)` and which
//! carry no condition of their own: `Common`, `NotScalingWithLuck`, `ScalingWithOnlyBadLuck`,
//! `Food`, `StatusImmunityItem`. Everything conditional (boss bags, `ByCondition`,
//! `OneFromOptions`, mode-dependent rerolls) is left to the hand-written `conditional_drops.rs`.
//!
//! `.OnFailedRoll(...)` chains are preserved: `Common(a, 7).OnFailedRoll(Common(b, 7))` becomes
//! one [`DropChain`] of two rules, not two independent one-rule chains, because the second is
//! only tried when the first misses.
//!
//! Two shapes in source do not spell a flat rule out on the register line itself, and both are
//! normalised before the scan rather than being read wrong:
//!
//! - `ItemDropRule.Gel(chance, min, max)` (`ItemDropRule.cs:80-85`) wraps two `Common(23, ...)`
//!   rules in a `DropBasedOnExtraGel`, whose second branch is a special-seed feature. The normal
//!   branch always applies, so the call is rewritten to that branch's own `Common`.
//! - A rule built into a local one line above its `RegisterToNPC` (`RegisterIceMimic`,
//!   `ItemDropDatabase.cs:235-240`; the Empress's enrage gate, `:332-333`) is resolved by name,
//!   so the first is not lost and the second is not mistaken for an unconditional drop.
//!
//! Ported from `gen_drops.py`.

use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;

use crate::csharp::{read_lossy, resolve_locals};

/// Constructors whose first four arguments are (item, chance, min, max) and which carry no
/// condition of their own. Anything else is left for the hand-written table.
///
/// `ScalingWithOnlyBadLuck` (`ItemDropRule.cs:45-48`) has exactly `Common`'s signature and no
/// condition; it only changes how the player's luck stat bends the roll. It is *which* of these
/// five a rule is that decides that, so the name is captured and carried through to the table
/// rather than discarded: see [`luck_of`].
const FLAT: &str = "Common|NotScalingWithLuck|ScalingWithOnlyBadLuck|Food|StatusImmunityItem";

/// (item, one_in, min, max, luck-scaling variant name as `npc_drops` spells it).
type Rule = (i64, i64, i64, i64, &'static str);

/// Which `Luck` roll a `FLAT` constructor's rule uses, by the class it returns.
///
/// - `Common` -> `CommonDrop`, whose `TryDroppingItem` is
///   `info.player.RollLuck(chanceDenominator) < chanceNumerator` (`CommonDrop.cs:36`). Full luck,
///   and this is vanilla's default: an ordinary drop scales unless it is one of the exceptions.
/// - `Food` -> `ItemDropWithConditionRule`, which extends `CommonDrop` and inherits that roll
///   (`ItemDropRule.cs:107-110`).
/// - `StatusImmunityItem` -> `ExpertGetsRerolls` -> `CommonDropWithRerolls`, which also rolls
///   `info.player.RollLuck` (`ItemDropRule.cs:112-115`, `CommonDropWithRerolls.cs`).
/// - `NotScalingWithLuck` -> `CommonDropNotScalingWithLuck`, which rolls `info.rng.Next` and is
///   the reason this distinction has to exist at all (`ItemDropRule.cs:50-53`).
/// - `ScalingWithOnlyBadLuck` -> `CommonDropScalingWithOnlyBadLuck`, whose roll is
///   `info.player.RollOnlyBadLuck` (`CommonDropScalingWithOnlyBadLuck.cs:17`): bad luck makes it
///   *more* likely and good luck does nothing at all.
///
/// An unrecognised name is `Full`, which is the safe default in the sense that matters: it is what
/// `Common` is, and `Common` is the overwhelming majority. A new constructor added to `FLAT`
/// without a case here would be silently luck-scaled, so `FLAT` and this function are meant to be
/// edited together.
fn luck_of(constructor: &str) -> &'static str {
    match constructor {
        "NotScalingWithLuck" => "LuckScaling::None",
        "ScalingWithOnlyBadLuck" => "LuckScaling::OnlyBad",
        _ => "LuckScaling::Full",
    }
}
type Chain = Vec<Rule>;

/// Split one call's arguments, respecting nested parentheses, starting just past the opening `(`
/// (so `depth` begins at 1 for the call itself).
///
/// `RegisterToMultipleNPCs(ItemDropRule.Common(160, 200), npcNetIds21).OnFailedRoll(...)` has two
/// arguments, and finding them by counting parentheses backwards from the end lands inside the
/// chained call instead.
fn balanced_args(line: &str, start: usize) -> Vec<String> {
    let mut depth: i32 = 1;
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in line[start..].chars() {
        let mut closed = false;
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
            if depth == 0 {
                closed = true;
            }
        }
        if closed {
            break;
        }
        if depth == 1 && ch == ',' {
            args.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    args.push(current);
    args
}

/// `int[] npcNetIds = new int[N] {...}` is declared fresh, under the *same* name, in many
/// different functions in this file — it is a local, not a file-scoped constant. Collapse each
/// (possibly multi-line) declaration onto its own line first, so a later sequential scan resolves
/// a `RegisterToMultipleNPCs(rule, npcNetIds)` call against whichever declaration of that name
/// most recently preceded it, never one resolved by declaration order alone.
fn collapse_array_declarations(text: &str) -> String {
    let re = Regex::new(r"int\[\]\s+\w+\s*=\s*new int\[\d*\]\s*\{[^}]*\}").unwrap();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for m in re.find_iter(text) {
        out.push_str(&text[last..m.start()]);
        out.push_str(&m.as_str().replace('\n', " "));
        last = m.end();
    }
    out.push_str(&text[last..]);
    out
}

/// `ItemDropRule.Gel(chanceDenominator = 1, minimumDropped = 1, maximumDropped = 1)` is not a
/// rule of its own: `ItemDropRule.cs:80-85` returns
/// `DropBasedOnExtraGel(Common(23, c, lo, hi), Common(23, c, lo * 2, hi * 2))`. The second branch
/// only fires under `ShouldDropExtraGel`, a special-seed feature this project does not model, so
/// the first branch is what every ordinary world sees. Rewrite the call to exactly that branch and
/// let the ordinary `Common` scan pick it up.
///
/// Without this the eight `Gel` registrations in `ItemDropDatabase.cs` (`:937, 962, 1066, 1067,
/// 1080, 1082, 1087, 1088`, twenty-five slime types between them) matched nothing at all, so item
/// 23 never dropped and the Slime Crown could not be crafted.
fn desugar_gel(text: &str) -> String {
    const CALL: &str = "ItemDropRule.Gel(";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(CALL) {
        out.push_str(&rest[..at]);
        let args = balanced_args(rest, at + CALL.len());
        // Every parameter defaults to 1, so a missing or empty argument is a 1.
        let arg = |i: usize| -> &str {
            args.get(i)
                .map(|a| a.trim())
                .filter(|a| !a.is_empty())
                .unwrap_or("1")
        };
        out.push_str(&format!(
            "ItemDropRule.Common(23, {}, {}, {})",
            arg(0),
            arg(1),
            arg(2)
        ));
        // Step past the call, including its closing parenthesis.
        let consumed: usize = args.iter().map(String::len).sum::<usize>() + args.len() - 1;
        rest = &rest[at + CALL.len() + consumed + 1..];
    }
    out.push_str(rest);
    out
}

/// `ItemDropDatabase.cs`, with the two rewrites both parsers below want applied.
fn prepare(root: &Path) -> String {
    let raw = read_lossy(&root.join("Terraria.GameContent.ItemDropRules/ItemDropDatabase.cs"));
    // `new CommonDropNotScalingWithLuck(item, chance, min, max)` is what
    // `ItemDropRule.NotScalingWithLuck` returns, spelled out directly at `ItemDropDatabase.cs:753`
    // and `:759` (the two Martian Saucer turrets' own Charged Blaster Cannon).
    let text = collapse_array_declarations(&raw).replace(
        "new CommonDropNotScalingWithLuck(",
        "ItemDropRule.NotScalingWithLuck(",
    );
    desugar_gel(&text)
}

/// NPC type -> list of chains, each chain a list of (item, one_in, min, max).
fn parse(text: &str) -> (BTreeMap<i64, Vec<Chain>>, usize) {
    let call = Regex::new(&format!(
        r"ItemDropRule\.({FLAT})\(\s*(\d+)\s*(?:,\s*(\d+))?\s*(?:,\s*(\d+))?\s*(?:,\s*(\d+))?\s*\)"
    ))
    .unwrap();
    let type_re = Regex::new(r"^short type = (\d+);").unwrap();
    let arr_decl_re = Regex::new(r"^int\[\]\s+(\w+)\s*=\s*new int\[\d*\]\s*\{([^}]*)\}").unwrap();
    let num_re = Regex::new(r"-?\d+").unwrap();
    // Anything with a condition, a pool, a bag or a mode branch is not ours to flatten. A chain
    // can *end* in one of these without the chain's own leading links needing it, so truncate at
    // the first excluded keyword instead of dropping the whole line — a chain's genuinely-flat
    // prefix survives even when its tail does not.
    let excluded_re = Regex::new(
        r"ByCondition|BossBag|MasterMode|OneFromOptions|ExpertGetsRerolls|LeadingConditionRule|DropBasedOn|OneFromRules|DropNothing|Coins|WithRerolls|RemixSeed",
    )
    .unwrap();
    let register_single_re = Regex::new(r"RegisterToNPC\((-?\d+)\s*,").unwrap();
    let normal_vs_expert_re = Regex::new(r"NormalvsExpert\(\s*(\d+)\s*,\s*(\d+)").unwrap();
    // A rule built into a local and registered on a later line. Source does this both to attach
    // `OnFailedRoll` chains to it afterwards (`RegisterIceMimic`, `ItemDropDatabase.cs:235-240`,
    // whose Toy Sled is the rule itself) and to hang `OnSuccess` off a condition
    // (`RegisterBoss_HallowBoss`, `:332-333`, the Empress's daytime-enrage gate on the
    // Terraprisma). Reading only the register line loses the first outright and reads the second
    // as an *unconditional* drop, because the excluded-keyword scan never sees the
    // `LeadingConditionRule` it was hiding behind a variable name.
    let rule_decl_re =
        Regex::new(r"^(?:IItemDropRule|LeadingConditionRule|IItemDropRuleWithChainedRules)\s+(\w+)\s*=\s*(.+);$")
            .unwrap();

    let mut out: BTreeMap<i64, Vec<Chain>> = BTreeMap::new();
    let mut current_type: Option<i64> = None;
    let mut arrays: std::collections::HashMap<String, Vec<i64>> = std::collections::HashMap::new();
    let mut rule_vars: BTreeMap<String, String> = BTreeMap::new();

    for raw_line in text.lines() {
        let trimmed = raw_line.trim();

        if let Some(caps) = type_re.captures(trimmed) {
            current_type = Some(caps[1].parse().unwrap());
            continue;
        }

        if let Some(caps) = rule_decl_re.captures(trimmed) {
            // These are function locals, and the same handful of names (`entry`, `itemDropRule`,
            // `rule`) is redeclared in function after function, so a declaration must *replace*
            // whatever the name last meant rather than only ever adding to the map. A local whose
            // initialiser is itself a registration (`IItemDropRule entry =
            // RegisterToMultipleNPCs(...)`, kept only so `RemoveFromMultipleNPCs` can undo it) is
            // not a rule expression: splicing it back in would register the same rule twice, so
            // that case clears the name instead of binding it.
            if caps[2].contains("Register") {
                rule_vars.remove(&caps[1]);
            } else {
                rule_vars.insert(caps[1].to_string(), caps[2].to_string());
            }
            // Fall through: the line may still be a registration in its own right.
        }

        if let Some(caps) = arr_decl_re.captures(trimmed) {
            let name = caps[1].to_string();
            let nums: Vec<i64> = num_re
                .find_iter(&caps[2])
                .map(|m| m.as_str().parse().unwrap())
                .collect();
            arrays.insert(name, nums);
            continue;
        }

        if !trimmed.contains("RegisterToNPC(") && !trimmed.contains("RegisterToMultipleNPCs(") {
            continue;
        }

        let mut line = resolve_locals(trimmed, &rule_vars);
        if let Some(m) = excluded_re.find(&line) {
            line.truncate(m.start());
            if line.is_empty()
                || (!line.contains("RegisterToNPC(") && !line.contains("RegisterToMultipleNPCs("))
            {
                continue;
            }
        }

        // Which NPCs.
        let mut targets: Vec<i64> = Vec::new();
        if let Some(caps) = register_single_re.captures(&line) {
            targets.push(caps[1].parse().unwrap());
        } else if line.contains("RegisterToNPC(type") {
            if let Some(t) = current_type {
                targets.push(t);
            }
        } else if let Some(at) = line.find("RegisterToMultipleNPCs(") {
            let start = at + "RegisterToMultipleNPCs(".len();
            let args = balanced_args(&line, start);
            for arg in args.iter().skip(1) {
                let key = arg.trim();
                if let Some(ids) = arrays.get(key) {
                    targets.extend(ids.iter().copied());
                } else {
                    targets.extend(
                        num_re
                            .find_iter(arg)
                            .map(|m| m.as_str().parse::<i64>().unwrap()),
                    );
                }
            }
        }
        targets.retain(|&t| t > 0);
        if targets.is_empty() {
            continue;
        }

        // The rules on this line, in order. A `.OnFailedRoll(` between two of them chains them.
        let mut rules: Chain = Vec::new();
        for caps in call.captures_iter(&line) {
            let item: i64 = caps[2].parse().unwrap();
            let one_in: i64 = caps.get(3).map_or(1, |m| m.as_str().parse().unwrap());
            let min: i64 = caps.get(4).map_or(1, |m| m.as_str().parse().unwrap());
            let max: i64 = caps.get(5).map_or(1, |m| m.as_str().parse().unwrap());
            rules.push((item, one_in, min, max, luck_of(&caps[1])));
        }
        // `NormalvsExpert(item, classicChance, expertChance)` rolls at a different rate depending
        // on the world. The classic branch is taken here and the difference is a known
        // simplification: an expert world under-rolls these twenty-four drops slightly. Recorded
        // in GAPS.md rather than pretended away.
        for caps in normal_vs_expert_re.captures_iter(&line) {
            let item: i64 = caps[1].parse().unwrap();
            let one_in: i64 = caps[2].parse().unwrap();
            // `NormalvsExpert` builds two `Common` rules (`ItemDropRule.cs:87-90`), so both
            // branches scale. `NormalvsExpertNotScalingWithLuck` is a separate constructor and is
            // not in `normal_vs_expert_re`'s pattern.
            rules.push((item, one_in, 1, 1, "LuckScaling::Full"));
        }
        if rules.is_empty() {
            continue;
        }

        let chained = line.contains(".OnFailedRoll(");
        let chains: Vec<Chain> = if chained {
            vec![rules]
        } else {
            rules.into_iter().map(|r| vec![r]).collect()
        };

        for &npc in &targets {
            out.entry(npc).or_default().extend(chains.clone());
        }
    }

    let total: usize = out.values().map(|v| v.len()).sum();
    (out, total)
}

/// The master-mode-only drops: `ItemDropRule.MasterModeCommonDrop(item)` (the relics) and
/// `ItemDropRule.MasterModeDropOnAllPlayers(item, chance)` (the pets and mounts).
///
/// Both are `Conditions.IsMasterMode` and nothing else (`ItemDropRule.cs:25-32`), so unlike the
/// rest of `conditional_drops.rs`'s subject matter there is no condition *tree* to flatten and
/// getting them wrong by hand is the greater risk: they were left out of the generator entirely
/// and 57 items were reachable from nowhere.
///
/// Two narrowings, both of them recorded rather than hidden:
///
/// - `MasterModeDropOnAllPlayers` is a `DropPerPlayerOnThePlayer`: *each* player present rolls the
///   1-in-4 separately, and each winner gets their own copy. Modelled as one roll, which is exact
///   for a single player and stingy for a crowd.
/// - The three Frost Moon and two Pumpkin Moon minibosses hang theirs off the same wave-gated
///   `LeadingConditionRule` this project already cannot model (see `conditional_drops::trophy`),
///   so theirs come out ungated, matching how their trophies are already handled.
///
/// Resolution is per method body, because the locals these hang off (`rule`, `leadingConditionRule`)
/// are redeclared in method after method, and the Twins' registration (`ItemDropDatabase.cs:457-469`)
/// comes *after* the `OnSuccess` calls that use it, so one sequential pass cannot see it.
fn parse_master(text: &str, master_rng: i64) -> BTreeMap<i64, Vec<Rule>> {
    let master_re = Regex::new(
        r"ItemDropRule\.MasterMode(CommonDrop|DropOnAllPlayers)\(\s*(\d+)\s*(?:,\s*(\w+))?",
    )
    .unwrap();
    // Only the bare `type`: a body that also declares `type2` (`RegisterBoss_BrainOfCthulhu`)
    // registers its master drops against `type`, and letting `type2` overwrite the binding would
    // hand them to the wrong NPC.
    let type_re = Regex::new(r"^short type = (\d+);").unwrap();
    let arr_decl_re = Regex::new(r"^int\[\]\s+(\w+)\s*=\s*new int\[\d*\]\s*\{([^}]*)\}").unwrap();
    let num_re = Regex::new(r"-?\d+").unwrap();
    // `IItemDropRule rule = RegisterToNPC(...)`, and the mirror shape where the local is built
    // first and registered afterwards: `RegisterToMultipleNPCs(leadingConditionRule, 126, 125)`.
    let bind_decl_re = Regex::new(r"^\w+ (\w+) = (RegisterTo\w+\(.*)$").unwrap();
    let bind_late_re = Regex::new(r"^(RegisterTo\w+\(.*)$").unwrap();
    let owner_re = Regex::new(r"^(\w+)\.On(?:Success|FailedRoll)\(").unwrap();

    let mut out: BTreeMap<i64, Vec<Rule>> = BTreeMap::new();
    for body in text.split("\tprivate ") {
        let mut current_type: Option<i64> = None;
        let mut arrays: std::collections::HashMap<String, Vec<i64>> =
            std::collections::HashMap::new();
        let mut bindings: std::collections::HashMap<String, Vec<i64>> =
            std::collections::HashMap::new();

        // Whose NPCs a `RegisterToNPC(...)`/`RegisterToMultipleNPCs(...)` call names.
        let targets_of = |call: &str,
                          current_type: Option<i64>,
                          arrays: &std::collections::HashMap<String, Vec<i64>>|
         -> Vec<i64> {
            let mut targets = Vec::new();
            if let Some(at) = call.find("RegisterToNPC(") {
                let args = balanced_args(call, at + "RegisterToNPC(".len());
                let first = args
                    .first()
                    .map(|a| a.trim().to_string())
                    .unwrap_or_default();
                match first.parse::<i64>() {
                    Ok(n) => targets.push(n),
                    Err(_) if first == "type" => targets.extend(current_type),
                    Err(_) => {}
                }
            } else if let Some(at) = call.find("RegisterToMultipleNPCs(") {
                let args = balanced_args(call, at + "RegisterToMultipleNPCs(".len());
                for arg in args.iter().skip(1) {
                    let key = arg.trim();
                    if let Some(ids) = arrays.get(key) {
                        targets.extend(ids.iter().copied());
                    } else {
                        targets.extend(
                            num_re
                                .find_iter(arg)
                                .map(|m| m.as_str().parse::<i64>().unwrap()),
                        );
                    }
                }
            }
            targets.retain(|&t| t > 0);
            targets
        };

        // First pass: what each local ends up registered to, wherever in the body that happens.
        for line in body.lines() {
            let trimmed = line.trim();
            if let Some(caps) = type_re.captures(trimmed) {
                current_type = Some(caps[1].parse().unwrap());
            }
            if let Some(caps) = arr_decl_re.captures(trimmed) {
                arrays.insert(
                    caps[1].to_string(),
                    num_re
                        .find_iter(&caps[2])
                        .map(|m| m.as_str().parse().unwrap())
                        .collect(),
                );
            }
            if let Some(caps) = bind_decl_re.captures(trimmed) {
                let targets = targets_of(&caps[2], current_type, &arrays);
                if !targets.is_empty() {
                    bindings.insert(caps[1].to_string(), targets);
                }
            } else if let Some(caps) = bind_late_re.captures(trimmed) {
                // `RegisterToNPC(type, leadingConditionRule);` names the local as its *rule*.
                let call = &caps[1];
                let start = call.find('(').unwrap() + 1;
                let args = balanced_args(call, start);
                let rule_arg = if call.starts_with("RegisterToNPC(") {
                    args.get(1)
                } else {
                    args.first()
                };
                if let Some(name) = rule_arg.map(|a| a.trim())
                    && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && !name.is_empty()
                    && name.parse::<i64>().is_err()
                {
                    let targets = targets_of(call, current_type, &arrays);
                    if !targets.is_empty() {
                        bindings.insert(name.to_string(), targets);
                    }
                }
            }
        }

        // Second pass: attach each master-mode rule to whatever it was registered under.
        let mut current_type: Option<i64> = None;
        for line in body.lines() {
            let trimmed = line.trim();
            if let Some(caps) = type_re.captures(trimmed) {
                current_type = Some(caps[1].parse().unwrap());
            }
            let Some(caps) = master_re.captures(trimmed) else {
                continue;
            };
            let item: i64 = caps[2].parse().unwrap();
            let one_in: i64 = match caps.get(3).map(|m| m.as_str()) {
                None => 1,
                Some(arg) => arg.parse().unwrap_or(master_rng),
            };
            let targets = if let Some(owner) = owner_re.captures(trimmed) {
                bindings.get(&owner[1]).cloned().unwrap_or_default()
            } else {
                targets_of(trimmed, current_type, &arrays)
            };
            // `MasterModeCommonDrop` is `ByCondition(IsMasterMode, item)`
            // (`ItemDropRule.cs:25-28`) -> `ItemDropWithConditionRule`, which *extends*
            // `CommonDrop` and inherits its `info.player.RollLuck` roll, so a relic scales with
            // luck. `MasterModeDropOnAllPlayers` is `DropPerPlayerOnThePlayer`
            // (`ItemDropRule.cs:30-33`), whose whole body rolls `info.rng`
            // (`DropPerPlayerOnThePlayer.cs:24`), so a master pet does not.
            let luck = if &caps[1] == "CommonDrop" {
                "LuckScaling::Full"
            } else {
                "LuckScaling::None"
            };
            for npc in targets {
                let rules = out.entry(npc).or_default();
                if !rules.contains(&(item, one_in, 1, 1, luck)) {
                    rules.push((item, one_in, 1, 1, luck));
                }
            }
        }
    }
    out
}

fn emit(
    drops: &BTreeMap<i64, Vec<Chain>>,
    total: usize,
    master: &BTreeMap<i64, Vec<Rule>>,
) -> String {
    let mut lines: Vec<String> = vec![
        "//! What NPCs drop when they die, generated from `ItemDropDatabase`.".into(),
        "//!".into(),
        "//! This is the **unconditional** half: a rule registered straight to an NPC with nothing"
            .into(),
        "//! gating it. The other half — boss bags, master-mode drops, `ByCondition`,".into(),
        "//! `OneFromOptions`, mode-dependent rerolls — lives in [`crate::conditional_drops`],".into(),
        "//! written by hand because flattening a condition tree is how you hand somebody the wrong"
            .into(),
        "//! loot forever without noticing. `tools/check_drops.py` compares both against the game."
            .into(),
        "//!".into(),
        "//! **Chains are preserved and matter.** `Common(a, 7).OnFailedRoll(Common(b, 7))` is one"
            .into(),
        "//! chain, not two rolls: the second is tried only when the first misses. Flattening them"
            .into(),
        "//! would make both far more common than the game intends, which is the kind of bug that"
            .into(),
        "//! looks like generosity until somebody compares drop rates.".into(),
        "//!".into(),
        "//! This file was hand-written once and held 248 rules. It was the only table in the".into(),
        "//! project with no generator, and 226 enemies were short at least one drop between them."
            .into(),
        "//!".into(),
        "//! Generated by `terrustia-codegen` from Terraria 1.4.5.8. Do not edit by hand.".into(),
        "".into(),
        "/// One thing an NPC might drop.".into(),
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]".into(),
        "pub struct Drop {".into(),
        "    pub item: u16,".into(),
        "    /// A one-in-this chance. One means always.".into(),
        "    pub one_in: u32,".into(),
        "    pub min: i16,".into(),
        "    pub max: i16,".into(),
        "    /// How the interacting player's luck bends this roll.".into(),
        "    pub luck: LuckScaling,".into(),
        "}".into(),
        "".into(),
        "/// Which of `Luck`'s three rolls a drop rule uses.".into(),
        "///".into(),
        "/// Vanilla decides this by which `ItemDropRule` constructor built the rule, and it is not".into(),
        "/// cosmetic: `CommonDrop.TryDroppingItem` is `info.player.RollLuck(chanceDenominator)`".into(),
        "/// (`CommonDrop.cs:36`), so an ordinary drop really does get likelier for a lucky player,".into(),
        "/// while `CommonDropNotScalingWithLuck` rolls `info.rng` and cannot.".into(),
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]".into(),
        "pub enum LuckScaling {".into(),
        "    /// `Luck.RollLuck`: good luck narrows the range, bad luck widens it.".into(),
        "    Full,".into(),
        "    /// `info.rng.Next`: luck cannot touch it.".into(),
        "    None,".into(),
        "    /// `Luck.RollOnlyBadLuck`: bad luck narrows the range and good luck does nothing.".into(),
        "    OnlyBad,".into(),
        "}".into(),
        "".into(),
        "/// A run of alternatives, tried in order until one of them lands.".into(),
        "pub type DropChain = &'static [Drop];".into(),
        "".into(),
        format!("/// How many rules the table holds, across {} NPC types.", drops.len()),
        format!("pub const RULES: usize = {total};"),
        "".into(),
        "/// What a type drops.".into(),
        "pub fn drops(npc_type: u16) -> &'static [DropChain] {".into(),
        "    match npc_type {".into(),
    ];

    for (&npc, chains) in drops {
        lines.push(format!("        {npc} => &["));
        for chain in chains {
            lines.push("            &[".into());
            for &(item, one_in, low, high, luck) in chain {
                lines.push("                Drop {".into());
                lines.push(format!("                    item: {item},"));
                lines.push(format!("                    one_in: {one_in},"));
                lines.push(format!("                    min: {low},"));
                lines.push(format!("                    max: {high},"));
                lines.push(format!("                    luck: {luck},"));
                lines.push("                },".into());
            }
            lines.push("            ],".into());
        }
        lines.push("        ],".into());
    }

    lines.extend(["        _ => &[],", "    }", "}", ""].map(String::from));

    let master_rules: usize = master.values().map(Vec::len).sum();
    lines.extend([
        "/// What a type drops **only in master mode**.".into(),
        "///".into(),
        "/// `ItemDropRule.MasterModeCommonDrop(item)` (the relics) and".into(),
        "/// `ItemDropRule.MasterModeDropOnAllPlayers(item, chance)` (the pets and mounts) are"
            .into(),
        "/// `Conditions.IsMasterMode` and nothing else (`ItemDropRule.cs:25-32`), so they are"
            .into(),
        "/// generated here rather than hand-written: there is no condition tree to flatten, and"
            .into(),
        "/// the 57 items between them were previously reachable from no table at all.".into(),
        "///".into(),
        "/// `MasterModeDropOnAllPlayers` really rolls once *per player present*, each winner"
            .into(),
        "/// getting their own copy. One roll is modelled, which is exact for a lone player."
            .into(),
        "/// [`crate::conditional_drops::conditional`] is what reaches these, under `master`."
            .into(),
        format!("pub const MASTER_RULES: usize = {master_rules};"),
        "".into(),
        "/// The master-mode-only drops for a type, in registration order.".into(),
        "pub fn master_drops(npc_type: u16) -> &'static [Drop] {".into(),
        "    match npc_type {".into(),
    ]);
    for (&npc, rules) in master {
        lines.push(format!("        {npc} => &["));
        for &(item, one_in, low, high, luck) in rules {
            lines.push("            Drop {".into());
            lines.push(format!("                item: {item},"));
            lines.push(format!("                one_in: {one_in},"));
            lines.push(format!("                min: {low},"));
            lines.push(format!("                max: {high},"));
            lines.push(format!("                luck: {luck},"));
            lines.push("            },".into());
        }
        lines.push("        ],".into());
    }
    lines.extend(
        [
            "        _ => &[],",
            "    }",
            "}",
            "",
            "#[cfg(test)]",
            "mod tests {",
            "    use super::*;",
            "",
            "    /// The table is populated, and did not silently regenerate empty.",
            "    #[test]",
            "    fn the_table_is_populated() {",
        ]
        .map(String::from),
    );
    lines.push(format!("        assert_eq!(RULES, {total});"));
    lines.push(format!("        assert_eq!(MASTER_RULES, {master_rules});"));
    lines.extend(
        [
            "        assert!(!drops(3).is_empty(), \"a zombie drops something\");",
            "        assert!(",
            "            !master_drops(50).is_empty(),",
            "            \"King Slime has a master-mode relic\"",
            "        );",
            "    }",
            "",
            "    /// A chain is one roll after another, not several at once.",
            "    ///",
            "    /// The caller stops at the first success. If these were separate chains a skeleton",
            "    /// would hand out every weapon at once instead of at most one.",
            "    #[test]",
            "    fn chains_stay_chained() {",
            "        let chained = (0..700u16)",
            "            .flat_map(drops)",
            "            .filter(|chain| chain.len() > 1)",
            "            .count();",
            "        assert!(chained > 0, \"no chains survived generation\");",
            "    }",
            "",
            "    /// Nothing rolls a zero-in-N chance, which would divide by zero downstream.",
            "    #[test]",
            "    fn every_chance_is_rollable() {",
            "        for kind in 0..700u16 {",
            "            for rule in drops(kind).iter().flat_map(|chain| chain.iter()).chain(master_drops(kind)) {",
            "                assert!(rule.one_in >= 1, \"npc {kind} has an impossible chance\");",
            "                assert!(rule.max >= rule.min, \"npc {kind} has a backwards stack\");",
            "            }",
            "        }",
            "    }",
            "",
            "    /// A type with no rules drops nothing rather than panicking.",
            "    #[test]",
            "    fn something_with_no_rules_drops_nothing() {",
            "        assert!(drops(u16::MAX).is_empty());",
            "    }",
            "}",
            "",
        ]
        .map(String::from),
    );

    lines.join("\n")
}

pub fn generate(root: &Path) -> String {
    let text = prepare(root);
    let (drops, total) = parse(&text);
    assert!(
        total >= 200,
        "only parsed {total} rules; the parser is wrong"
    );
    // `private int _masterModeDropRng = 4;` (`ItemDropDatabase.cs:15`) is the denominator every
    // `MasterModeDropOnAllPlayers` registration passes.
    let master_rng: i64 = Regex::new(r"private int _masterModeDropRng = (\d+);")
        .unwrap()
        .captures(&text)
        .expect("no _masterModeDropRng")[1]
        .parse()
        .unwrap();
    let master = parse_master(&text, master_rng);
    let master_rules: usize = master.values().map(Vec::len).sum();
    assert!(
        master_rules >= 50,
        "only parsed {master_rules} master-mode rules; the parser is wrong"
    );
    emit(&drops, total, &master)
}
