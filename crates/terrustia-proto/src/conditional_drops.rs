//! Loot that depends on more than the thing that died.
//!
//! The flat table in [`crate::npc_drops`] covers what an enemy drops unconditionally. This is the
//! other half: drops that need to know what mode the world is in, how far through the game it is,
//! or where the kill happened.
//!
//! Three kinds matter enough to be worth having. A boss in expert mode drops a treasure bag
//! *instead of* its ordinary loot, which is the whole shape of expert progression. Every boss drops
//! a trophy one time in ten and a mask one in seven, which is what a trophy room is made of. And a
//! good deal of hardmode's crafting material — souls, dust, horns — only drops once the wall has
//! fallen, so dropping it before then would break the progression it exists to gate.

/// What the world was like when something died.
#[derive(Debug, Clone, Copy, Default)]
pub struct Conditions {
    /// Expert or above, which is what turns a boss's loot into a bag.
    pub expert: bool,
    /// Master, which is above expert. A handful of drops are rolled three ways rather than two.
    pub master: bool,
    pub hard_mode: bool,
    pub downed_plantera: bool,
    /// Which evil this *world* has, which is not the same question as where the kill happened:
    /// the Eye of Cthulhu drops the ore of its world's evil wherever it dies.
    pub world_is_crimson: bool,
    /// Where it happened, for the drops that only come from one biome.
    pub in_hallow: bool,
    pub in_corruption: bool,
    pub in_crimson: bool,
    /// Below the rock layer, which is where the souls live.
    pub underground: bool,
    /// For the Twins only: whether the *other* one is already dead.
    ///
    /// The game gates their whole loot on `Conditions.MissingTwin`, so killing one while its
    /// sibling still lives gives nothing. Without this the pair would drop twice over.
    pub other_twin_dead: bool,
    /// A blood moon, for `Conditions.IsBloodMoonAndNotFromStatue` — combined with `npc_from_statue`
    /// below rather than pre-combined here, so each stays what it actually is: a fact about the
    /// world, and a fact about the one NPC.
    pub blood_moon: bool,
    /// Whether *this* NPC came from a statue rather than an ordinary spawn. A statue farm must not
    /// be able to grind the blood-moon-exclusive drops below — that is the entire reason the game
    /// checks it at all.
    pub npc_from_statue: bool,
    /// A solar eclipse. Every NPC this gates (`Conditions.RegisterEclipse`'s own drops) can only be
    /// alive during one anyway, so this exists for completeness rather than because any of them
    /// could otherwise be reached outside an eclipse.
    pub eclipse: bool,
    /// `Conditions.BeatAnyMechBoss`: any one of the Destroyer, the Twins or Skeletron Prime.
    pub downed_mech_any: bool,
    /// `Conditions.DownedAllMechBosses`: all three, not just one.
    pub downed_all_mech_bosses: bool,
    /// The pumpkin moon's current wave, if one is running — `None` covers "no pumpkin moon at
    /// all" and "a frost moon is running instead" alike, since `PumpkinMoonDropGatingChance` only
    /// ever applies to the pumpkin moon's own NPCs, which cannot be alive during the other event.
    pub pumpkin_moon_wave: Option<i32>,
    /// The frost moon's current wave, the same way, for `FrostMoonDropGatingChance` and
    /// `FrostMoonDropGateForTrophies` (`Terraria.GameContent.ItemDropRules/Conditions.cs:55-89`, `:127-166`).
    ///
    /// A sibling field rather than one shared `moon_wave`, because the two gates are separate
    /// classes with separate constants (28 against 24, `-2` against `-1`) and because each checks
    /// its *own* moon first: a Frost Moon NPC alive during a Pumpkin Moon would be gated by
    /// neither, and collapsing them into one number would silently gate it by whichever moon
    /// happened to be up.
    ///
    /// Every Frost Moon boss drop used to be flattened to unconditional for want of this, which
    /// made wave-1 loot roughly ten times likelier than the game allows and the whole event's wave
    /// progression pointless. Six places read it now.
    pub frost_moon_wave: Option<i32>,
    /// `NPC.RedHatSkeletronAdjustmentsEnabled()` for *this* Skeletron (`NPC.cs:67435-67446`) —
    /// per-instance state, not a fact about npc_type 35 in general: the ordinary boss and the
    /// Clothier's repeatable vanity re-fight share a type and are told apart only by `ai[3]`. The
    /// caller reads it off the dying NPC itself, the same way `npc_from_statue` does.
    pub red_hat_skeletron: bool,
    /// `NPC.AI_120_HallowBoss_IsGenuinelyEnraged()` for *this* Empress (`NPC.cs:46321-46328`,
    /// `ai[3] == 2 || ai[3] == 3`): per-instance state again, and the only gate on the Terraprisma.
    ///
    /// It means "this fight was begun in daylight", not "it is daytime now". The mark is set only
    /// while she is at full health or on the tick she finishes arriving (`NPC.cs:46472-46475`,
    /// `:46565-46568`), so summoning her at night and letting dawn break mid-fight does not earn
    /// it. The caller reads it off the dying NPC, as `red_hat_skeletron` above does.
    pub empress_genuinely_enraged: bool,
}

/// One conditional drop.
///
/// `one_in` is the roll's denominator — real vanilla's own `chanceDenominator` — and the actual
/// chance is `numerator / one_in`, not always `1 / one_in`: `CommonDrop`/`ByCondition`'s own fifth
/// constructor argument (`chanceNumerator`) is usually `1`, but not always, and a handful of real
/// rules in `ItemDropDatabase.cs` roll `2`-in-`3` or `3`-in-`4` rather than a flat `1`-in-`N`. Every
/// constructor below except [`m_in_n`] defaults `numerator` to `1`, so nothing already using
/// `always`/`sometimes`/`a_few` changes rate.
///
/// `scaling` is which of `Luck`'s three rolls the rule uses, which vanilla decides by the
/// `ItemDropRule` constructor that built it - see [`crate::npc_drops::LuckScaling`]. `Full` is the
/// default because `Common`/`ByCondition` are the default, and both roll
/// `info.player.RollLuck(chanceDenominator) < chanceNumerator`; the ~30 registrations in
/// `ItemDropDatabase.cs` that do not are marked with [`not_scaling`] or [`only_bad_luck`] and each
/// cites its own line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conditional {
    pub item: u16,
    pub one_in: u32,
    pub numerator: u32,
    pub min: i16,
    pub max: i16,
    pub scaling: crate::npc_drops::LuckScaling,
}

/// The same rule with luck taken out of it: `ItemDropRule.NotScalingWithLuck`
/// (`ItemDropRule.cs:50-53`), whose `CommonDropNotScalingWithLuck` rolls `info.rng.Next`.
const fn not_scaling(rule: Conditional) -> Conditional {
    Conditional {
        scaling: crate::npc_drops::LuckScaling::None,
        ..rule
    }
}

// There is deliberately no `only_bad_luck` companion to [`not_scaling`].
// `ItemDropRule.ScalingWithOnlyBadLuck` (`ItemDropRule.cs:45-48`) is registered exactly once in
// the whole database (`ItemDropDatabase.cs:179`, the Groom's and the Bride's Bloody Tear), it
// carries no condition, and so it belongs to the generated table - which labels it
// `LuckScaling::OnlyBad` itself. A helper here would have no caller, and one written for a future
// caller is one nobody has checked against source.

const fn always(item: u16) -> Conditional {
    Conditional {
        item,
        one_in: 1,
        numerator: 1,
        min: 1,
        max: 1,
        scaling: crate::npc_drops::LuckScaling::Full,
    }
}

const fn sometimes(item: u16, one_in: u32) -> Conditional {
    Conditional {
        item,
        one_in,
        numerator: 1,
        min: 1,
        max: 1,
        scaling: crate::npc_drops::LuckScaling::Full,
    }
}

const fn a_few(item: u16, one_in: u32, min: i16, max: i16) -> Conditional {
    Conditional {
        item,
        one_in,
        numerator: 1,
        min,
        max,
        scaling: crate::npc_drops::LuckScaling::Full,
    }
}

/// An `M`-in-`N` roll — for the rules that use it, real vanilla's own `chanceNumerator`
/// (`CommonDrop`'s fifth constructor argument, `ByCondition`'s sixth) is not `1`. Only reach for
/// this when source actually passes a numerator other than the default; every other rule stays on
/// `sometimes`/`a_few` so a reader can tell "M in N" apart from the far more common "1 in N" at a
/// glance.
const fn m_in_n(item: u16, numerator: u32, one_in: u32, min: i16, max: i16) -> Conditional {
    Conditional {
        item,
        one_in,
        numerator,
        min,
        max,
        scaling: crate::npc_drops::LuckScaling::Full,
    }
}

/// The treasure bag a boss drops in expert mode, if it has one.
pub fn treasure_bag(npc_type: u16) -> Option<u16> {
    Some(match npc_type {
        50 => 3318,        // King Slime
        4 => 3319,         // Eye of Cthulhu
        13..=15 => 3320,   // Eater of Worlds, any segment
        266 => 3321,       // Brain of Cthulhu
        222 => 3322,       // Queen Bee
        35 | 36 => 3323,   // Skeletron
        113 => 3324,       // Wall of Flesh
        134 => 3325,       // The Destroyer
        125 | 126 => 3326, // The Twins
        127 => 3327,       // Skeletron Prime
        262 => 3328,       // Plantera
        245 => 3329,       // Golem
        370 => 3330,       // Duke Fishron
        439 => 3331,       // Lunatic Cultist
        398 => 3332,       // Moon Lord
        // The four 1.4-era bosses this table was missing entirely. Every one of them suppresses
        // its classic loot in expert (`classic_only` returns `&[]` there, matching vanilla's own
        // `Conditions.NotExpert` wrapper), so without a bag they dropped literally nothing in an
        // expert or master world.
        551 => 3860, // Betsy, `RegisterBoss_Betsy`, `ItemDropDatabase.cs:632-636`
        636 => 4782, // Empress of Light, `RegisterBoss_HallowBoss`, `:321-323`
        657 => 4957, // Queen Slime, `RegisterBoss_QueenSlime`, `:305-307`
        668 => 5111, // Deerclops, `RegisterBoss_Deerclops`, `:524-526`
        _ => return None,
    })
}

/// A boss's trophy, which is a one-in-ten drop whatever the mode.
pub fn trophy(npc_type: u16) -> Option<u16> {
    Some(match npc_type {
        4 => 1360,
        13..=15 => 1361,
        266 => 1362,
        35 | 36 => 1363,
        222 => 1364,
        113 => 1365,
        134 => 1366,
        127 => 1367,
        // 125 and 126 are deliberately absent, and it is the same reason 491, 551, 564, 565, 576
        // and 577 always were. `RegisterBossTrophies` (`ItemDropDatabase.cs:865-904`) writes most
        // of these as `ByCondition(condition, item, 10)`, which is this table's business, but
        // eight of them as a plain `Common(item, 10)` (`:896-903`) - and a plain `Common` with no
        // condition is exactly what the *generated* table takes. The Twins' two were in both, so
        // `drop_loot` rolled each trophy twice and Retinazer and Spazmatism dropped theirs at
        // about 19 per cent rather than 10. `no_item_is_registered_in_both_tables` is the guard.
        262 => 1370,
        245 => 1371,
        50 => 2489,
        370 => 2589,
        439 => 3357,
        395 => 3358,
        398 => 3595, // Moon Lord
        636 => 4783, // Empress of Light
        657 => 4958, // Queen Slime — was missing entirely; see `classic_only`'s own note
        668 => 5108, // Deerclops
        // The Frost Moon's three: `RegisterBoss_FrostMoon` (`ItemDropDatabase.cs:363-387`) wraps
        // every one of these three trophies in `ItemDropRule.ByCondition(FrostMoonDropGateForTrophies,
        // item)` — a custom, wave-dependent rate (`Conditions.cs:127-166`: unreachable before wave
        // 15, then 1-in-4 through wave 16, 1-in-3 through 18, 1-in-2 from 19 on, further shrunk by a
        // 1-in-3 chance in expert) rather than the flat 1-in-10 every other boss's own trophy uses —
        // and the whole thing is itself inside `RegisterToNPC(npc, new LeadingConditionRule(
        // FrostMoonDropGatingChance))`, a second, independent wave/luck roll on top of that. Neither
        // gate can be modelled here: both need the frost moon's live wave number, which has nowhere
        // to reach this module — `Conditions` only carries `pumpkin_moon_wave`, and adding a sibling
        // field would break every existing caller's struct literal, a change outside this lane's own
        // files. Modelled instead at the standard 1-in-10 flat rate every other trophy already uses,
        // a strictly more generous simplification (obtainable from wave 1, not just wave 15+) —
        // documented rather than left unreachable, the same call this module already makes for
        // hardmode's `NormalvsExpert` luck-scaling and the Groom/Bride's unscaled 1/5 floor.
        //
        // The item-to-boss mapping itself is not a mistake either way: real vanilla's own constant
        // names are internally swapped from what they drop — `RegisterBoss_FrostMoon` gives npc 345
        // (Santa-NK1) `ItemID.IceQueenTrophy` (1960) and npc 346 (Ice Queen) `ItemID.SantaNK1Trophy`
        // (1961); only npc 344 (Everscream) gets the trophy actually named for it (`EverscreamTrophy`,
        // 1962). Kept exactly as source drops them, not as the names suggest.
        344 => 1962, // Everscream
        345 => 1960, // Santa-NK1 (drops the item vanilla itself calls `IceQueenTrophy`)
        346 => 1961, // Ice Queen (drops the item vanilla itself calls `SantaNK1Trophy`)
        _ => return None,
    })
}

/// The lunar fragment one of the four Lunar Towers drops, if it is one.
///
/// `RegisterBoss_LunarTowers` (`ItemDropDatabase.cs:626-629`). The pairing is the game's own and
/// is not alphabetical: Solar (517) gives 3458, Vortex (422) gives 3456, Nebula (507) gives 3457,
/// Stardust (493) gives 3459.
pub fn lunar_fragment(npc_type: u16) -> Option<u16> {
    Some(match npc_type {
        517 => 3458,
        422 => 3456,
        507 => 3457,
        493 => 3459,
        _ => return None,
    })
}

/// Everything that only drops under some condition.
///
/// Returns an empty list for most types, which is the point: a condition that never applies costs
/// a match arm and nothing else.
pub fn conditional(npc_type: u16, at: Conditions) -> Vec<Conditional> {
    let mut out = Vec::new();

    // Expert turns a boss's loot into a bag. The ordinary drops still happen; the bag is extra,
    // and it is what carries the expert-only accessory.
    if at.expert
        && let Some(bag) = treasure_bag(npc_type)
    {
        out.push(always(bag));
    }
    if let Some(trophy) = trophy(npc_type) {
        // The Frost Moon's three are the one exception to the flat 1-in-10 every other boss's
        // trophy uses, and the exception is two rolls rather than one: `rule.OnSuccess(
        // ByCondition(FrostMoonDropGateForTrophies, item))` where `rule` is itself
        // `RegisterToNPC(npc, new LeadingConditionRule(FrostMoonDropGatingChance))`
        // (`ItemDropDatabase.cs:372`, `:378`, `:384`). Two independent gates multiply, exactly as
        // Pumpking's own trophy and medallion already do here.
        //
        // Below wave 15 the trophy gate cannot pass at all, so these three are genuinely
        // unobtainable early in the event - which the old flat 1-in-10 made them obtainable from
        // wave 1, a documented over-give this replaces.
        if matches!(npc_type, 344..=346) {
            if let Some(wave) = at.frost_moon_wave
                && let Some(trophy_gate) = moon_trophy_gate_denominator(wave)
            {
                let outer = frost_moon_gate_denominator(wave, at.expert);
                out.push(sometimes(trophy, outer * trophy_gate));
            }
        } else {
            out.push(sometimes(trophy, 10));
        }
    }

    // Master mode's own relics, pets and mounts. `ItemDropRule.MasterModeCommonDrop` and
    // `MasterModeDropOnAllPlayers` are `Conditions.IsMasterMode` and nothing else
    // (`ItemDropRule.cs:25-32`), which is flat enough to generate, so the table itself lives in
    // [`crate::npc_drops::master_drops`] beside the unconditional half rather than being retyped
    // here. This is the only thing that reaches it: without this call all 57 of those items are
    // reachable from nowhere, which is exactly the state the audit found them in.
    if at.master {
        out.extend(
            crate::npc_drops::master_drops(npc_type)
                .iter()
                .map(|d| Conditional {
                    item: d.item,
                    one_in: d.one_in,
                    numerator: 1,
                    min: d.min,
                    max: d.max,
                    // The generator worked this out from the constructor: a relic is
                    // `MasterModeCommonDrop` and scales, a master pet is
                    // `MasterModeDropOnAllPlayers` and does not.
                    scaling: d.luck,
                }),
        );
    }

    // The four Lunar Towers, whose fragments are the whole of the lunar tier
    // (`RegisterBoss_LunarTowers`, `ItemDropDatabase.cs:608-629`). Vanilla drops these with a
    // `DropOneByOne`: twelve to twenty separate chunks, each one to three of the fragment in
    // classic, and in expert a stack base scaled by 1.5 plus one more per player per chunk. The
    // range used here is vanilla's own reported one (`DropOneByOne.ReportDroprates`):
    // `MinimumItemDropsCount * (MinimumStackPerChunkBase + BonusMinDropsPerChunkPerPlayer)` to
    // `MaximumItemDropsCount * (MaximumStackPerChunkBase + BonusMaxDropsPerChunkPerPlayer)`, so
    // 12..=60 in classic and 24..=100 in expert-or-above. One roll of a flat range rather than
    // twenty of a narrow one: the same total, a slightly different distribution inside it.
    if let Some(fragment) = lunar_fragment(npc_type) {
        let (min, max) = if at.expert { (24, 100) } else { (12, 60) };
        out.push(a_few(fragment, 1, min, max));
    }

    // Skeletron's own RedHatSkeletron variant (the Clothier's repeatable, "Chippy" vanity
    // re-fight): five unconditional items, `ai[3] == 1` on this exact instance
    // (`NPC.RedHatSkeletronAdjustmentsEnabled`, `NPC.cs:67435-67446`). Independent of
    // expert/classic mode — `RegisterBoss_Skeletron`'s own `ByCondition(RedHatSkeletron, item)`
    // calls carry no `NotExpert` wrapper the way the rest of Skeletron's loot does
    // (`ItemDropDatabase.cs:565-569`), so this sits outside the `!at.expert` gate below rather
    // than inside `classic_only`.
    if npc_type == 35 && at.red_hat_skeletron {
        for item in [5624, 5625, 5626, 5628, 5737] {
            out.push(always(item));
        }
    }

    // The Terraprisma: the Empress of Light's one reward for fighting her in daylight, which is the
    // fight she is built to refuse (`RegisterBoss_HallowBoss`'s own
    // `LeadingConditionRule(EmpressOfLightIsGenuinelyEnraged).OnSuccess(Common(5005))`,
    // `ItemDropDatabase.cs:333-334`). Unconditional inside that gate, and outside the `NotExpert`
    // wrapper the rest of her loot sits under, so an expert daylight kill earns it too. It was
    // simply absent from the tables, which made the hardest thing in the game unobtainable.
    if npc_type == 636 && at.empress_genuinely_enraged {
        out.push(always(5005));
    }

    // Hardmode's crafting materials. Every one of these is what gates the tier above it, so
    // dropping any of them early would let a world skip a step.
    if at.hard_mode {
        match npc_type {
            // Souls of Night: anything in the evil underground.
            _ if at.underground && (at.in_corruption || at.in_crimson) => {
                out.push(a_few(547, 5, 1, 2));
            }
            _ => {}
        }
        if at.underground && at.in_hallow {
            out.push(a_few(548, 5, 1, 2));
        }
        match npc_type {
            // A wyvern is the only source of Soul of Flight.
            87 => out.push(a_few(754, 1, 20, 40)),
            // The hallow's own three.
            75 => {
                out.push(a_few(521, 2, 1, 5));
                out.push(sometimes(494, 25));
            }
            // A unicorn's horn.
            86 => out.push(sometimes(1327, 5)),
            // Mimics, which are the point of a hardmode chest.
            55 => out.push(sometimes(671, 1)),
            _ => {}
        }
    }

    // Plantera's first death gives the Grenade Launcher and its ammunition outright; every death
    // after that offers one weapon out of a pool instead, which needs a draw this table cannot
    // make. The first-kill half is the one that matters and is here.
    //
    // `ItemDropDatabase.RegisterBoss_Plantera` gates this on `FirstTimeKillingPlantera`, which is
    // the inverse of the flag the world already carries.
    if npc_type == 262 && !at.expert && !at.downed_plantera {
        out.push(always(758));
        out.push(a_few(771, 1, 50, 150));
    }
    if !at.expert {
        // Killing one twin while the other lives gives nothing: the game hangs the whole pair's
        // loot off `MissingTwin`, so it lands once rather than twice.
        if !matches!(npc_type, 125 | 126) || at.other_twin_dead {
            out.extend(classic_only(npc_type));
        }
        // The Eye of Cthulhu drops its world's evil ore, and only in classic — in expert the bag
        // carries it. Which evil is a property of the world, not of the ground it died on.
        if npc_type == 4 {
            if at.world_is_crimson {
                out.push(a_few(880, 1, 30, 90));
                out.push(a_few(2171, 1, 1, 3));
            } else {
                out.push(a_few(56, 1, 30, 90));
                out.push(a_few(59, 1, 1, 3));
            }
        }
    }
    // `ItemDropRule.ExpertGetsRerolls(item, chanceDenominator, expertRerolls)` — a classic-mode
    // roll of 1/chanceDenominator, plus `expertRerolls` extra independent attempts at the same
    // odds in expert-or-above (`ItemDropRule.cs:20`, `DropBasedOnExpertMode`). Every use in the
    // decompiled table has `expertRerolls == 1`, so expert mode's real rate is
    // `1 - (1 - 1/N)^2`, not `1/N` again. Modelled here as the flat classic rate in every mode —
    // the same documented simplification `tools/gen_drops.py`'s own `NormalvsExpert` handling
    // already uses elsewhere in this project ("an expert world under-rolls these... slightly").
    // `ItemDropDatabase.cs:258-274`, the dungeon-guardian family's shared registration block.
    match npc_type {
        290 => {
            out.push(sometimes(1513, 15)); // Paladin: Paladin's Hammer
            out.push(sometimes(938, 10)); // Paladin's Shield
        }
        287 => {
            out.push(sometimes(977, 12)); // Bone Lee: Black Belt
            out.push(sometimes(963, 12)); // Feral Claws
        }
        291 => {
            out.push(sometimes(1300, 12)); // Skeleton Sniper: Sniper Rifle
            out.push(sometimes(1254, 12)); // Steel Axe
        }
        292 => {
            out.push(sometimes(1514, 12)); // Tactical Skeleton: Tactical Shotgun
            out.push(sometimes(679, 12)); // Flak Jacket
        }
        293 => out.push(sometimes(759, 18)), // Skeleton Commando: Rocket Launcher
        289 => out.push(sometimes(4789, 25)), // Giant Cursed Skull: Cursed Skull vanity mask
        281 | 282 => out.push(sometimes(1446, 20)), // Ragged Caster (both): Ragged Casterhood
        283 | 284 => out.push(sometimes(1444, 20)), // Necromancer (both): Nercomantic Hood[sic]
        285 | 286 => out.push(sometimes(1445, 20)), // Diabolist (both): Diabolist Hood
        // The dungeon guardians proper — Rusty/Blue Armored/Hell Armored Bones, every variant.
        269..=280 => {
            out.push(sometimes(1183, 400)); // Golden Key
            out.push(sometimes(1266, 300)); // Muramasa
            out.push(sometimes(671, 200)); // Cobalt Shield
            out.push(sometimes(4679, 200)); // Ice Skates
        }
        // The Groom's and the Bride's Bloody Tear used to be pushed here as well as being in the
        // generated table, so `drop_loot` rolled it twice and the item landed on about 36 per cent
        // of kills against the 20 the game intends. `ScalingWithOnlyBadLuck` is in the generator's
        // own `FLAT` set (`ItemDropDatabase.cs:179` is an unconditional registration), so
        // `npc_drops` carries it, and now carries the `OnlyBad` label with it. Nothing belongs
        // here. `no_item_is_registered_in_both_tables` is the guard.
        // `LeadingConditionRule(NotRemixSeed()/RemixSeed())` (`ItemDropDatabase.cs:941-944`) picks
        // which of two items an ordinary vs. remix-seed world gives — not the same item at two
        // rates, two *different* items. Only the ordinary-world (`NotRemix`) branch is in scope;
        // remix seed is secret-seed content, tracked separately. The mapping is not symmetric
        // between these two NPCs — verified against source rather than assumed.
        49 => out.push(sometimes(1325, 250)), // Cave Bat: Vampire Frog Staff (ordinary-world item)
        109 => out.push(sometimes(1314, 5)), // Clown: Bombs (ordinary-world item; Clown's own rate)
        // Mimic's two flat pre-hardmode drops (`Conditions.Easymode` = `!Main.hardMode`,
        // `ItemDropDatabase.cs:228-229`) — the matching pool lives in `one_from`, not here.
        85 if !at.hard_mode => {
            out.push(sometimes(930, 20)); // Cloud in a Balloon
            out.push(sometimes(997, 20)); // Blindfold
        }
        // Ice Mimic's own flat pre-hardmode drop (`ItemDropDatabase.cs:238`), same shape as the
        // ordinary Mimic's. Its `OneFromOptions` pool chain is far more deeply nested (chained
        // `OnFailedRoll`s across remix/hardmode/easymode branches via a helper method) and is not
        // attempted here — out of scope for the same reason 44's mid-chain pool is.
        629 if !at.hard_mode => out.push(sometimes(997, 20)), // Blindfold
        // `RegisterToNPC(44, Common(118, 25))` (`ItemDropDatabase.cs:1157`) — only the chain's
        // flat first link. The rest of that chain (`.OnFailedRoll(OneFromOptions(4, 410, 411))
        // .OnFailedRoll(Common(166, 1, 1, 3))`) is a pool nested inside a fallback chain, a shape
        // neither this function (independent per-item rolls, no fallback ordering) nor
        // `npc_drops.rs` (flat chains, no pool inside one) can represent — left out rather than
        // approximated; item 118 itself is already covered by `npc_drops.rs`.
        //
        // Red Devil (156) is the same shape as Cave Bat/Clown above: `NotRemixSeedHardmode` gives
        // item 683, `RemixSeed` gives a *different* item (112) — not the same drop at two rates.
        // Only the ordinary-world branch is in scope (`ItemDropDatabase.cs:945-946`).
        156 if at.hard_mode => out.push(sometimes(683, 30)),
        // Chaos Elemental: `LeadingConditionRule(TenthAnniversaryIsUp/TenthAnniversaryIsNotUp)`
        // (`ItemDropDatabase.cs:939-940`) picks between a real-world-date anniversary item and the
        // ordinary one. This project has no calendar awareness of that event at all — it is never
        // "up" here — so the `TenthAnniversaryIsNotUp` branch is the only one that can ever apply
        // and is modelled unconditionally.
        120 => out.push(if at.expert {
            sometimes(1326, 400)
        } else {
            sometimes(1326, 500)
        }),
        _ => {}
    }

    // Solar-eclipse-only NPCs (`RegisterEclipse`, `ItemDropDatabase.cs:185-236`). Every one of
    // these can only be alive during an eclipse in the first place, so — same reasoning as the
    // pumpkin/frost moon rosters — nothing here re-checks `at.eclipse`; it exists on `Conditions`
    // for completeness, not because any of these could otherwise be reached.
    match npc_type {
        // Creature from the Deep.
        461 => out.push(sometimes(497, 50)),
        // Vampire / Vampire Bat, both.
        158 | 159 => out.push(sometimes(900, 35)),
        // Eyezor.
        251 => out.push(sometimes(1311, 15)),
        // The Butcher: three chainsaw parts, flat and unconditioned.
        460 => {
            out.push(sometimes(4740, 50));
            out.push(sometimes(4741, 50));
            out.push(sometimes(4742, 50));
        }
        // Dr. Man Fly: two drill parts, same shape.
        468 => {
            out.push(sometimes(4738, 50));
            out.push(sometimes(4739, 50));
        }
        // Black Recluse, wall-mounted or not: `DropBasedOnExpertMode(Common(2607, 2, 1, 3),
        // CommonDrop(2607, 10, 1, 3, 9))` (`ItemDropDatabase.cs:959`) — classic is the plain 1-in-2
        // `a_few` already had; expert's own branch is 9-in-10, not 1-in-10, which needed both a
        // real mode branch (there was none before — every mode got the classic rate) and the
        // numerator `Conditional` had no field for until `m_in_n` above. Found while auditing every
        // `chanceNumerator != 1` site in `ItemDropDatabase.cs` against this module for the
        // Brain-of-Cthulhu/Queen-Bee numerator fix below; this file's own prior comment already
        // named the gap but left it unfixed — now fixed alongside it.
        163 | 238 => out.push(if at.expert {
            m_in_n(2607, 9, 10, 1, 3)
        } else {
            a_few(2607, 2, 1, 3)
        }),
        // Zombie Merman / Eyeball Flying Fish: three single-item pools, each really just a flat
        // 1-in-8 (`RegisterBloodMoonFishing`, `ItemDropDatabase.cs:168-170`) — both NPCs are
        // themselves only ever caught blood-moon fishing, so nothing here re-checks `at.blood_moon`
        // for the same reason the eclipse-exclusive block above does not re-check `at.eclipse`.
        586 | 587 => {
            out.push(sometimes(4273, 8));
            out.push(sometimes(4381, 8));
            out.push(sometimes(4325, 8));
        }
        _ => {}
    }
    // DD2 mage/ogre flat items (`ItemDropDatabase.cs:745-777`): the `OneFromOptions` pools these
    // NPCs also drop live in `chance_pools`, not here — this is only the plain
    // `DropBasedOnExpertMode` items, mode-scaled. Every one is `NotScalingWithLuck`, so luck
    // cannot move any of them: the Old One's Army is the one event that gives a lucky player
    // exactly the same odds as everybody else.
    match npc_type {
        564 if at.expert => {
            out.push(not_scaling(always(3814)));
            out.push(not_scaling(a_few(3815, 1, 4, 4)));
        }
        564 => {
            out.push(not_scaling(sometimes(3814, 2)));
            out.push(not_scaling(a_few(3815, 2, 4, 4)));
        }
        565 | 577 if at.expert => {
            out.push(not_scaling(sometimes(3814, 4)));
            out.push(not_scaling(a_few(3815, 4, 4, 4)));
        }
        565 | 577 => {
            out.push(not_scaling(sometimes(3814, 8)));
            out.push(not_scaling(a_few(3815, 8, 4, 4)));
        }
        576 if at.expert => {
            out.push(not_scaling(sometimes(3814, 2)));
            out.push(not_scaling(a_few(3815, 2, 4, 4)));
            out.push(not_scaling(sometimes(3856, 4)));
        }
        576 => {
            out.push(not_scaling(sometimes(3814, 4)));
            out.push(not_scaling(a_few(3815, 4, 4, 4)));
            out.push(not_scaling(sometimes(3856, 5)));
        }
        _ => {}
    }
    // Eclipse-exclusive NPCs gated on `Conditions.DownedPlantera` specifically, distinct from the
    // unconditioned eclipse drops just above (`ItemDropDatabase.cs:207-215`).
    if at.downed_plantera {
        match npc_type {
            460 => out.push(sometimes(3098, 40)), // The Butcher: chainsaw.
            468 => out.push(sometimes(3105, 40)), // Dr. Man Fly: drill.
            466 => out.push(sometimes(3106, 40)), // Psycho: hatchet.
            467 => out.push(sometimes(3249, 30)), // Deadly Sphere: staff.
            _ => {}
        }
    }
    // Gated on having downed *all three* mechanical bosses, not just one — the Reaper's own
    // `LeadingConditionRule(DownedAllMechBosses)` (`ItemDropDatabase.cs:203`), inside the eclipse
    // registration above but a genuinely separate condition from "an eclipse is running."
    if npc_type == 253 && at.downed_all_mech_bosses {
        out.push(sometimes(1327, 40));
    }
    // Mothron: `RegisterEclipse`'s own `LeadingConditionRule(DownedAllMechBosses).OnSuccess(
    // ExpertGetsRerolls(1570, 4, 1))` (`ItemDropDatabase.cs:201-203`) — the Broken Hero Sword,
    // modelled at its flat classic rate in every mode, the same `ExpertGetsRerolls` simplification
    // this module already documents and uses elsewhere (see this function's own comment above the
    // dungeon-guardian family).
    if npc_type == 477 && at.downed_all_mech_bosses {
        out.push(sometimes(1570, 4));
    }
    // Pixie: `Conditions.BeatAnyMechBoss` (`ItemDropDatabase.cs:75`) — any *one*, not all three,
    // the weaker sibling of the Reaper's gate just above.
    if npc_type == 75 && at.downed_mech_any {
        out.push(sometimes(5662, 200));
    }
    // Two more on that same gate, both of which had no source anywhere in this project:
    // `ItemDropDatabase.cs:787-788`.
    if at.downed_mech_any {
        match npc_type {
            176 => out.push(sometimes(1521, 100)), // Pincushion Zombie
            205 => out.push(sometimes(1611, 2)),   // Chaos Elemental
            _ => {}
        }
    }
    // `Conditions.IsHardmode` (`ItemDropDatabase.cs:743`), the Princess's own weapon.
    if npc_type == 663 && at.hard_mode {
        out.push(sometimes(5065, 8));
    }
    // Bone, from the Angry Bones family (`npcNetIds20`, `ItemDropDatabase.cs:1162-1164`). These
    // two lines are the *only* place in the whole drop database that gives out item 154, and both
    // are `ByCondition`, so the flat generator correctly left them alone and nothing else picked
    // them up: Bone was unobtainable, which also makes every Bone recipe uncraftable.
    //
    // The classic branch is the tail of a failed-roll chain whose first three links
    // (932, 3095, 327) already live in [`crate::npc_drops`] as an ordinary chain, so it is rolled
    // independently here rather than as that chain's fourth link. The difference is that about
    // 3.4% of kills give both a weapon and the bones where vanilla gives only the weapon: a
    // documented over-give, and the alternative is no Bone in the world at all.
    if matches!(npc_type, 31 | 32 | 34 | 294 | 295 | 296 | 693) {
        out.push(if at.expert {
            a_few(154, 1, 2, 6)
        } else {
            a_few(154, 1, 1, 3)
        });
    }
    // Pumpking's expert-only extra: `rule.OnSuccess(ByCondition(IsExpert, 4444, 5))`
    // (`ItemDropDatabase.cs:351`), sitting under the same wave gate as its weapon pool, so the two
    // rolls multiply exactly as the Headless Horseman's medallion does just below.
    if npc_type == 325
        && at.expert
        && let Some(wave) = at.pumpkin_moon_wave
    {
        let gate = pumpkin_moon_gate_denominator(wave, at.expert);
        out.push(sometimes(4444, gate * 5));
    }
    // The Don't Starve crossover drops. Vanilla registers each of these twice, once behind
    // `Conditions.DontStarveIsUp` and once behind `DontStarveIsNotUp`, and the second is
    // `!Main.dontStarveWorld` (`Conditions.cs:1498-1503`), which is true in every ordinary world.
    // This project does not model the Don't Starve seed, so the ordinary-world rate is the only
    // one, and all five items had no source at all before. `ItemDropDatabase.cs:971-972`,
    // `:1034-1035`, `:1130-1133`, `:1172-1173`.
    match npc_type {
        170 | 171 | 180 => out.push(sometimes(5096, 25)), // the three Pigrons: Ham Bat
        49 | 51 | 60 | 93 | 137 | 150 | 151 | 152 | 634 => out.push(sometimes(5097, 300)), // bats
        177 => out.push(sometimes(5089, 100)),            // Mimic (Corrupt)
        _ => {}
    }
    if matches!(npc_type, 6 | 7 | 8 | 9 | 173 | 181 | 239 | 240) {
        out.push(sometimes(5094, 525)); // Tentacle Spike
    }
    if matches!(
        npc_type,
        6 | 7 | 8 | 9 | 81 | 94 | 98 | 99 | 100 | 101 | 173 | 174 | 181..=183 | 239..=242 | 268
    ) {
        out.push(sometimes(5091, 1500)); // Pig Pet
    }
    // `Conditions.IsBloodMoonAndNotFromStatue` (`ItemDropDatabase.cs:179-182`) — a statue farm must
    // not be able to grind these, which is the entire reason the game checks the NPC's own origin
    // rather than just the world's.
    if at.blood_moon && !at.npc_from_statue {
        match npc_type {
            489 | 490 => out.push(sometimes(4271, 100)),
            586 | 587 | 620 | 621 => out.push(sometimes(4271, 25)),
            _ => {}
        }
    }
    // The Headless Horseman's Pumpkin Medallion: `ByCondition(PumpkinMoonDropGatingChance, 1857,
    // 20)` (`ItemDropDatabase.cs:342`) — the wave gate and this rule's own `chanceDenominator: 20`
    // are two independent rolls, same as the scarecrow pool in `chance_pools` (see its own comment
    // for why they combine by multiplying rather than either alone).
    if npc_type == 315
        && let Some(wave) = at.pumpkin_moon_wave
    {
        let gate = pumpkin_moon_gate_denominator(wave, at.expert);
        out.push(sometimes(1857, gate * 20));
    }

    // Pumpking's and Mourning Wood's own trophies: `rule.OnSuccess(ByCondition(
    // PumpkinMoonDropGateForTrophies, item))` (`ItemDropDatabase.cs:350`/`360`) — a genuinely
    // separate roll from the weapon pool in `chance_pools`, both hanging off the same shared
    // `LeadingConditionRule(PumpkinMoonDropGatingChance)`; see `moon_trophy_gate_denominator`'s
    // own doc for the formula and its one disclosed simplification.
    if let Some(wave) = at.pumpkin_moon_wave
        && let Some(gate) = moon_trophy_gate_denominator(wave)
    {
        match npc_type {
            325 => out.push(sometimes(1855, gate)), // Pumpking (ItemID names these swapped)
            327 => out.push(sometimes(1856, gate)), // Mourning Wood
            _ => {}
        }
    }

    // Santa-NK1's Reindeer Bells: `rule2.OnSuccess(ItemDropRule.ByCondition(
    // Conditions.FromCertainWaveAndAbove(15), 1914, 15))` (`ItemDropDatabase.cs:376`) — its own
    // 1-in-15 roll, gated on `NPC.waveNumber >= 15` (`Conditions.cs`'s
    // `FromCertainWaveAndAbove`, a bare `NPC.waveNumber >= _wave` and nothing else) and on the
    // same outer `LeadingConditionRule(FrostMoonDropGatingChance)` every other Frost Moon boss
    // item shares. All three, now that the wave is here: two independent rolls multiplied and one
    // hard threshold.
    //
    // Wave 15 is over half way through a Frost Moon, and a mount is worth reaching. The old flat
    // 1-in-15 from wave 1 was a documented over-give in both directions at once: reachable when
    // it should not be, and no likelier at wave 20 than at wave 1.
    if npc_type == 345
        && let Some(wave) = at.frost_moon_wave
        && wave >= 15
    {
        let gate = frost_moon_gate_denominator(wave, at.expert);
        out.push(sometimes(1914, gate * 15));
    }

    out.extend(by_mode(npc_type, at));
    out
}

/// Pools a kill draws exactly one item from.
///
/// The game writes these as `OneFromOptions`, and they are not the same as a run of independent
/// rolls: a King Slime gives you *one* piece of the ninja set, never two and never none. Choosing
/// which needs a die, so the pools are returned here and the caller rolls.
///
/// Empty for anything with no such rule, which is nearly everything.
pub fn one_from(npc_type: u16, at: Conditions) -> &'static [&'static [u16]] {
    // The Mimic (85) is not a boss and has no expert-mode treasure bag replacing this pool — its
    // own gate is `Main.hardMode` (`Conditions.cs:1264`, confusingly named `Easymode` for the
    // *pre*-hardmode branch), a different axis entirely from player difficulty. Handled below,
    // ahead of the expert-mode guard everything else here is subject to.
    if npc_type == 85 {
        return if at.hard_mode {
            // `NotRemixSeedHardmode` pool (`ItemDropDatabase.cs:225`) — the remix variant
            // (item 3069 in place of 517) is excluded, tracked under the secret-seeds backlog.
            &[&[437, 517, 535, 536, 532, 554]]
        } else {
            // `Easymode` pool (`ItemDropDatabase.cs:227`). The two flat `Easymode`-gated drops
            // (930, 997, both 1/20) belong in `conditional()`, not here — a pool and a flat rule
            // are different shapes even when they share the same gate.
            &[&[49, 50, 53, 54, 5011, 975]]
        };
    }
    // Ice Queen (346) used to be here, as a guaranteed pick. Its pick *is* guaranteed - a bare
    // `1` denominator on the `OneFromOptions` - but the whole rule sits inside
    // `RegisterToNPC(346, new LeadingConditionRule(FrostMoonDropGatingChance))`
    // (`ItemDropDatabase.cs:383-385`), which is not, and this function's pools have no gate. It
    // has moved to `chance_pools`, whose pools do, now that `Conditions` carries the frost moon's
    // wave. See its arm there.
    if at.expert {
        // Expert replaces the lot with a treasure bag.
        return &[];
    }
    match npc_type {
        // The ninja set: hood, shirt, trousers.
        50 => &[&[256, 257, 258]],
        // The Wall of Flesh: an emblem and a weapon, one of each. The emblems are the whole of
        // early hardmode's damage progression, so a run that gets neither is noticeably poorer.
        113 => &[&[489, 490, 491, 2998], &[426, 434, 514, 4912]],
        // Queen Bee: one of the bee weapons, guaranteed (`OneFromOptionsNotScalingWithLuck(1,
        // 1121, 1123, 2888)`, `ItemDropDatabase.cs:545` — a bare `1` denominator, which is what
        // makes this belong here). The Bee-armor pool used to also be listed here as a second,
        // *unconditionally* drawn pool — that was bug #2 itself: real vanilla only reaches the
        // armor pool via a `ByCondition(1129, 3).OnFailedRoll(...)` chain, at ~1/3 overall and
        // mutually exclusive with the Hive Wand, which `conditional_chains` now owns. Left here it
        // would keep firing on every kill regardless of that chain, guaranteeing an armor piece
        // every time and letting it co-occur with the wand — exactly what was found and fixed.
        222 => &[&[1121, 1123, 2888]],
        // Golem: one of its seven, which is where the Picksaw's siblings live.
        245 => &[&[1258, 1122, 899, 1248, 1295, 1296, 1297]],
        // Queen Slime's three.
        657 => &[&[4982, 4983, 4984]],
        // The Empress's weapon.
        636 => &[&[4923, 4952, 4953, 4914]],
        // Duke Fishron's weapon. The seventh option differs on the remix seed, which this server
        // does not offer, so the ordinary set is the one here.
        370 => &[&[5526, 2624, 2622, 2621, 5478, 3291, 2623]],
        // Betsy.
        551 => &[&[3827, 3859, 3870, 3858]],
        // Deerclops.
        668 => &[&[5117, 5118, 5119, 5095]],
        // The Martian Saucer's weapon: one of six.
        395 => &[&[2797, 2749, 2795, 2796, 2880, 2769]],
        // The three Big Mimics: each drops one of its own five accessories, guaranteed
        // (`ItemDropRule.OneFromOptions(1, ...)` — a bare `1` denominator, unlike every gated pool
        // in `chance_pools`, is what makes this belong here instead) — `ItemDropDatabase.cs:987-989`.
        473 => &[&[3008, 3014, 3012, 3015, 3023]],
        474 => &[&[3006, 3007, 3013, 3016, 3020]],
        475 => &[&[3029, 3030, 3051, 6168, 3022]],
        // The Flying Dutchman's own ship: nineteen banners/decorations, one guaranteed per kill
        // (`ItemDropDatabase.cs:866`; each also has its own independent 1-in-300 roll elsewhere in
        // source, not modelled — the guaranteed pick alone already makes every one obtainable).
        491 => &[&[
            1704, 1705, 1710, 1716, 1720, 2379, 2389, 2405, 2843, 3885, 2663, 3910, 2238, 2133,
            2137, 2143, 2147, 2151, 2155,
        ]],
        // Moss Zombie: one of ten fossil-ore-family drops, guaranteed (`ItemDropDatabase.cs:1146`).
        691 => &[&[4352, 4350, 4349, 4353, 4351, 4354, 5127, 4378, 4377, 4389]],
        _ => &[],
    }
}

/// Moon Lord's own guaranteed draw of *two distinct* items from his ten-weapon pool —
/// `RegisterToNPC(398, new LeadingConditionRule(NotExpert)).OnSuccess(new
/// FromOptionsWithoutRepeatsDropRule(2, 3063, 3389, 3065, 1553, 3930, 3541, 3570, 3571, 3569,
/// 5480))` (`ItemDropDatabase.cs:605`) — Meowmere, Star Wrath, Terrarian, S.D.M.G., Last Prism,
/// Celebration Mk2, Lunar Flare Book, Rainbow Crystal Staff, Moonlord Turret Staff and the Moon
/// Lord Legacy Whip. Classic-only, same as every other boss's ordinary loot (the bag replaces it
/// in expert), and previously missing entirely — no case for npc 398 existed anywhere in this
/// module, so a classic-mode kill granted none of these.
///
/// Deliberately not a [`one_from`] arm: `FromOptionsWithoutRepeatsDropRule` draws its two items
/// *without replacement* (the game removes each pick from its temporary pool before the second
/// draw — `FromOptionsWithoutRepeatsDropRule.TryDroppingItem`), which `one_from`'s own consumer
/// cannot express — it draws one item per returned pool, independently, with no memory of what an
/// earlier pool already picked. Two entries of this same ten-item pool could then hand back the
/// same weapon twice — a real, material deviation (1-in-10 per kill) for what this project's own
/// audit called "the single most iconic loot in the game." The caller draws both indices itself,
/// using the same "pick one, then pick again excluding it" algorithm as source, so this returns
/// only the plain pool (or an empty slice when the condition does not hold).
pub fn moon_lord_weapons(npc_type: u16, at: Conditions) -> &'static [u16] {
    if npc_type == 398 && !at.expert {
        &[3063, 3389, 3065, 1553, 3930, 3541, 3570, 3571, 3569, 5480]
    } else {
        &[]
    }
}

/// A companion item some [`one_from`] picks bring along automatically.
///
/// Golem's Stynger draw also grants its own ammunition: `IItemDropRule itemDropRule =
/// ItemDropRule.Common(1258); itemDropRule.OnSuccess(ItemDropRule.Common(1261, 1, 60, 180),
/// hideLootReport: true);` nested inside the seven-way weapon pool (`ItemDropDatabase.cs:654-656`)
/// — both `Common` calls default to `chanceDenominator: 1`, so the bundle is unconditional
/// whenever Stynger itself is the pool's outcome, not a further chance. `one_from`'s own consumer
/// spawns exactly the one item it picked, with no notion of "this pick also grants something
/// else" — a shape distinct from mode-gating (already handled by `one_from`'s own early return),
/// so it gets its own small lookup rather than a change to `one_from`'s pool format.
///
/// Item 1261 (Stynger Bolt) has no other drop or craft source anywhere in this codebase, so
/// without this its only legitimate source was missing entirely.
pub fn bundled_with(item: u16) -> Option<(u16, i16, i16)> {
    match item {
        1258 => Some((1261, 60, 180)), // Stynger -> Stynger Bolt
        // Pumpking's own pool (`ItemDropDatabase.cs:347-349`): the Stake Launcher's own ammunition.
        1835 => Some((1836, 30, 60)), // StakeLauncher -> Stake
        // Mourning Wood's own pool (`ItemDropDatabase.cs:354-357`): both weapons' own ammunition.
        1782 => Some((1783, 50, 100)), // CandyCornRifle -> CandyCorn
        1784 => Some((1785, 25, 50)),  // JackOLanternLauncher -> ExplosiveJackOLantern
        // The Nail Gun's own ammunition, the same shape: `itemDropRule.OnSuccess(
        // ItemDropRule.Common(3108, 1, 100, 200), hideLootReport: true)`
        // (`ItemDropDatabase.cs:268-270`). Both items had no source anywhere before.
        3107 => Some((3108, 100, 200)), // NailGun -> Nail
        _ => None,
    }
}

/// One `OneFromOptions(chanceDenominator, ...)`-shaped pool: unlike [`one_from`]'s pools, which
/// always succeed and only pick *which* item, this first rolls `1` in `one_in` and only then picks
/// one from `options` — the caller must roll the gate itself, not just the pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChancePool {
    pub one_in: u32,
    pub options: &'static [u16],
    /// Which of `Luck`'s rolls the *gate* uses. `OneFromOptionsDropRule` rolls
    /// `info.player.RollLuck(chanceDenominator)` (`OneFromOptionsDropRule.cs`), and
    /// `OneFromOptionsNotScaledWithLuckDropRule` rolls `info.rng.Next`, which is the entire
    /// difference between the two classes and the reason vanilla has both.
    ///
    /// The *pick* among the options is never luck-scaled in either class, so there is nothing here
    /// for a pool with a gate of 1: it always succeeds and only chooses which item.
    pub scaling: crate::npc_drops::LuckScaling,
}

const fn pool(one_in: u32, options: &'static [u16]) -> ChancePool {
    ChancePool {
        one_in,
        options,
        scaling: crate::npc_drops::LuckScaling::Full,
    }
}

/// A pool whose gate luck cannot touch: `OneFromOptionsNotScalingWithLuck`.
const fn unlucky_pool(one_in: u32, options: &'static [u16]) -> ChancePool {
    ChancePool {
        one_in,
        options,
        scaling: crate::npc_drops::LuckScaling::None,
    }
}

/// `(24 - wave - expertBump) / 2.5`, floored toward zero the way the game's own `(int)` cast does,
/// minus one more in expert, floored at `1` — `Conditions.PumpkinMoonDropGatingChance.CanDrop`
/// (`ItemDropDatabase.cs:91-113`) with `info.player.RollLuck(n) == 0` read as the unscaled `1/n`
/// this project already uses everywhere luck would otherwise apply. Shared by every pumpkin-moon
/// drop gated on it, both here and in [`conditional`].
fn pumpkin_moon_gate_denominator(wave: i32, expert: bool) -> u32 {
    let adjusted = wave + if expert { 5 } else { 0 };
    let mut denominator = f64::from(24 - adjusted) / 2.5;
    if expert {
        denominator -= 1.0;
    }
    (denominator as i64).max(1) as u32
}

/// The same shape with the Frost Moon's own two constants: `(28 - wave - expertBump) / 2.5`, minus
/// *two* more in expert rather than one, floored at `1` — `Conditions.FrostMoonDropGatingChance.
/// CanDrop` (`Terraria.GameContent.ItemDropRules/Conditions.cs:57-78`). Shared by every Frost Moon boss drop, which is all of them:
/// `RegisterBoss_FrostMoon` wraps npc 344, 345 and 346's whole rule trees in one
/// `LeadingConditionRule(FrostMoonDropGatingChance)` (`ItemDropDatabase.cs:371`, `:377`, `:383`).
///
/// The Frost Moon runs to wave 20 and the Pumpkin Moon to 15, which is why the constants differ:
/// both reach a guaranteed drop a few waves before their own last one. Non-expert this is 1-in-10
/// at wave 1 and 1-in-1 from wave 26, so the flattening this replaces was a tenfold over-give at
/// the bottom of the event and correct only at the very top of it.
fn frost_moon_gate_denominator(wave: i32, expert: bool) -> u32 {
    let adjusted = wave + if expert { 5 } else { 0 };
    let mut denominator = f64::from(28 - adjusted) / 2.5;
    if expert {
        denominator -= 2.0;
    }
    (denominator as i64).max(1) as u32
}

/// `Conditions.PumpkinMoonDropGateForTrophies.CanDrop` (`Conditions.cs:179-217`) and
/// `FrostMoonDropGateForTrophies.CanDrop` (`Conditions.cs:127-166`), which are the same function
/// twice: identical bodies apart from which moon each checks is up, so one helper serves both and
/// the caller supplies the wave for whichever moon it is asking about.
///
/// Pumpking's and Mourning Wood's own trophies (`ItemID.MourningWoodTrophy`/`PumpkingTrophy`, the
/// constant names swapped from what they actually drop, the same way the Frost Moon's own three
/// are — see `trophy`'s own doc comment), and the Frost Moon's Everscream, Santa-NK1 and Ice
/// Queen. Unreachable before wave 15; the denominator then steps 4 (waves 15-16) -> 3 (17-18) ->
/// 2 (19+).
///
/// Expert mode's own further reduction (`if (Main.expertMode && Main.rand.Next(3) == 0) num--;`)
/// is a *second*, independent, un-seeded roll — not even on `info.rng`, the RNG every other gate
/// in this module already treats as the one this project's rolls model — so it is left out rather
/// than approximated with a coin flip this project's drop resolution has nowhere to carry. Modelled
/// at the wave-only denominator in every mode, a documented under-roll in expert specifically (a
/// smaller true chance there, never a larger one), the same direction this module's other luck/
/// expert simplifications already take.
fn moon_trophy_gate_denominator(wave: i32) -> Option<u32> {
    if wave < 15 {
        return None;
    }
    Some(match wave {
        15 | 16 => 4,
        17 | 18 => 3,
        _ => 2,
    })
}

/// Chance-gated pools: a kill rolls `1` in `one_in` first, and only on success draws one item from
/// `options`. Distinct from [`one_from`], whose pools always succeed — vanilla writes both under
/// the same `OneFromOptions` call, with the gate as its first argument; a bare `1` there means
/// "always", which is exactly what makes a pool belong in `one_from` instead of here.
///
/// Empty for anything with no such rule, which is most things. Returns an owned `Vec` rather than
/// a `'static` slice — unlike every other table in this module, the pumpkin-moon arm below needs a
/// denominator computed from the live wave number, which cannot be a compile-time constant.
pub fn chance_pools(npc_type: u16, at: Conditions) -> Vec<ChancePool> {
    match npc_type {
        // Eater of Souls: a Sunglasses-family vanity, `ItemDropDatabase.cs:1050`.
        6 => vec![pool(175, &[956, 957, 958])],
        // The whole hornet family (`npcNetIds8`, `ItemDropDatabase.cs:1051-1052`): Hornet, Man
        // Eater, and the five hardmode Honey Comb hornets. A Hive Pack piece.
        42 | 43 | 231 | 232 | 233 | 234 | 235 => vec![pool(100, &[960, 961, 962])],
        // Zombie Elf trio, the Frost Moon's own (`ItemDropDatabase.cs:389`). Registered directly
        // rather than under the event's `LeadingConditionRule`, so its 1-in-200 is the whole gate
        // and it needs no wave: `RegisterToMultipleNPCs(ItemDropRule.OneFromOptions(200, ...))`.
        338..=340 => vec![pool(200, &[1943, 1944, 1945])],
        // Ice Queen (346): `RegisterToNPC(346, new LeadingConditionRule(condition)).OnSuccess(
        // ItemDropRule.OneFromOptions(1, 1910, 1929))` (`ItemDropDatabase.cs:383-385`) — the
        // *pick* between ElfMelter and ChainGun is guaranteed (a bare `1` denominator), and the
        // rule reaching that pick at all is not: the outer `FrostMoonDropGatingChance` is the
        // pool's whole gate, and one-in-that is exactly what a `ChancePool` says.
        //
        // This lived in `one_from` as a guaranteed pick for want of the wave number, which made
        // an Ice Queen kill at wave 1 in a classic world hand over one of the two every time
        // rather than one time in ten. `treasure_bag` has no case for 346 (Frost Moon minibosses
        // have no expert-mode bag at all) and `RegisterBoss_FrostMoon` never wraps this in a
        // `NotExpert` guard, so it stands in every mode - `chance_pools` has no expert-mode
        // return to be caught by, unlike `one_from`, so the guard it needed there is gone too.
        346 => match at.frost_moon_wave {
            Some(wave) => vec![pool(
                frost_moon_gate_denominator(wave, at.expert),
                &[1910, 1929],
            )],
            None => vec![],
        },
        // Nailhead's Nail Gun, once Plantera is down (`ItemDropDatabase.cs:266-270`): a
        // `LeadingConditionRule(DownedPlantera)` over a 1-in-25 `Common(3107)` whose own
        // `OnSuccess` hands over 100-200 Nails. A one-option pool rather than an entry in
        // `conditional`, because this is the only shape whose consumer also drops the companion
        // item `bundled_with` names, and the ammunition is only granted when the gun lands.
        //
        // Expert really rerolls the 1-in-25 once (`WithRerolls(3107, 1, 25)`), which this project
        // already flattens to the classic rate everywhere it appears; see `conditional`'s own
        // `ExpertGetsRerolls` note.
        463 if at.downed_plantera => vec![pool(25, &[3107])],
        // Corrupt/Crimson Penguin (`ItemDropDatabase.cs:1137`).
        168 | 470 => vec![pool(50, &[3757, 3758, 3759])],
        // Zombie Eskimo, armed or not (`ItemDropDatabase.cs:1005`).
        161 | 431 => vec![pool(20, &[803, 804, 805])],
        // Zombie (Raincoat) (`ItemDropDatabase.cs:1112`).
        223 => vec![pool(20, &[1135, 1136])],
        // Greek Skeleton, a Golden Slime enemy — `ItemDropDatabase.cs:1148`.
        481 => vec![pool(7, &[3187, 3188, 3189])],
        // Light/Dark Mummy (Desert Lamia), `ItemDropDatabase.cs:1031`.
        528 | 529 => vec![pool(40, &[3786, 3785, 3784])],
        // Goblin Summoner's staff pool: `NormalvsExpertOneFromOptions(2, 1, ...)`
        // (`ItemDropDatabase.cs:935`) — expert halves the gate rather than swapping the pool.
        471 => vec![pool(if at.expert { 1 } else { 2 }, &[3052, 3053, 3054])],
        // DD2 Dark Mage, tiers 1 and 3 (`ItemDropDatabase.cs:767-777`): two independent
        // `NormalvsExpertOneFromOptionsNotScalingWithLuck` pools each, mode-scaled. The whole DD2
        // roster's loot is `...NotScalingWithLuck`, alone among the events - the Old One's Army
        // gives the same odds to a lucky player as to anybody else - so all eight of these pools
        // are `unlucky_pool`.
        564 if at.expert => vec![
            unlucky_pool(1, &[3810, 3809]),
            unlucky_pool(2, &[3857, 3855]),
        ],
        564 => vec![
            unlucky_pool(2, &[3810, 3809]),
            unlucky_pool(3, &[3857, 3855]),
        ],
        565 => vec![
            unlucky_pool(6, &[3810, 3809]),
            unlucky_pool(6, &[3857, 3855]),
        ],
        // DD2 Ogre, tiers 2 and 3 (`ItemDropDatabase.cs:751-763`): same shape as the Dark Mage
        // above, two mode-scaled pools each.
        576 if at.expert => {
            vec![
                unlucky_pool(2, &[3811, 3812]),
                unlucky_pool(1, &[3852, 3854, 3823, 3835, 3836]),
            ]
        }
        576 => vec![
            unlucky_pool(3, &[3811, 3812]),
            unlucky_pool(2, &[3852, 3854, 3823, 3835, 3836]),
        ],
        577 => vec![
            unlucky_pool(6, &[3811, 3812]),
            unlucky_pool(4, &[3852, 3854, 3823, 3835, 3836]),
        ],
        // The pumpkin moon's scarecrow family (`npcNetIds`, `ItemDropDatabase.cs:341-345`): the
        // wave gate and the pool's own `OneFromOptions(10, ...)` gate are two independent rolls
        // stacked (`ByCondition`/`LeadingConditionRule` wrapping a nested rule each roll their own
        // chance), which combine to exactly `1 / (wave_gate * 10)` — the Headless Horseman's single
        // item gets the same wave gate in `conditional`, combined with its own separate 1-in-20.
        305..=314 => {
            if let Some(wave) = at.pumpkin_moon_wave {
                let gate = pumpkin_moon_gate_denominator(wave, at.expert);
                vec![pool(gate * 10, &[1788, 1789, 1790])]
            } else {
                vec![]
            }
        }
        // Pumpking's own weapon pool: `rule.OnSuccess(new OneFromRulesRule(1, Common(1829),
        // Common(1831), Common(1835)/*+Stake, via bundled_with*/, Common(1837), Common(1845)))`
        // (`ItemDropDatabase.cs:347-349`) — a bare `chanceDenominator: 1` pick of one of five once
        // the shared wave gate passes, so (unlike the scarecrow pool just above) nothing multiplies
        // it further. The trophy this same `rule` also carries (item 1855) is a separate roll, in
        // `conditional`.
        325 => match at.pumpkin_moon_wave {
            Some(wave) => vec![pool(
                pumpkin_moon_gate_denominator(wave, at.expert),
                &[1829, 1831, 1835, 1837, 1845],
            )],
            None => vec![],
        },
        // Mourning Wood's own pool, the same shape: `rule2.OnSuccess(new OneFromRulesRule(1,
        // Common(1782)/*+CandyCorn*/, Common(1784)/*+ExplosiveJackOLantern*/, Common(1811),
        // Common(1826), Common(1801), Common(1802), Common(4680), Common(1798)))`
        // (`ItemDropDatabase.cs:354-359`). Its own trophy (1856) is likewise in `conditional`.
        327 => match at.pumpkin_moon_wave {
            Some(wave) => vec![pool(
                pumpkin_moon_gate_denominator(wave, at.expert),
                &[1782, 1784, 1811, 1826, 1801, 1802, 4680, 1798],
            )],
            None => vec![],
        },
        _ => vec![],
    }
}

/// What a boss drops when the world is *not* in expert.
///
/// In expert the treasure bag replaces all of this, which is why every one of these rules is
/// gated the same way: a classic world gets the loot directly, and an expert world gets the bag
/// that contains it. Most servers run classic, so leaving these out left most bosses dropping
/// nothing but coins and a trophy.
fn classic_only(npc_type: u16) -> Vec<Conditional> {
    match npc_type {
        4 => vec![
            a_few(2112, 7, 1, 1),
            a_few(1299, 40, 1, 1),
            a_few(47, 1, 20, 50),
        ],
        // Skeletron's three weapons — moved to `conditional_chains`: they are one
        // `ByCondition(...).OnFailedRoll(...).OnFailedRoll(...)` chain in the game, stopping at
        // the first that lands, which this function's independent per-item rolls cannot express.
        // King Slime's Slime Hook/Slime Gun pair is the same shape and lives there too.
        50 => vec![
            a_few(2430, 4, 1, 1),
            a_few(2493, 7, 1, 1),
            always(998),
            a_few(1309, 30, 1, 1),
        ],
        // The Twins. Gated on the other one already being dead, handled by the caller.
        125 | 126 => vec![
            a_few(2106, 7, 1, 1),
            a_few(1225, 1, 15, 30),
            a_few(549, 1, 25, 40),
        ],
        // Plantera. The key is the whole of hardmode's second half: without it the Jungle Temple
        // never opens, so Golem, the Cultist and the Moon Lord are all behind this one line.
        262 => vec![
            a_few(2109, 7, 1, 1),
            always(1141),
            a_few(1182, 20, 1, 1),
            a_few(1305, 50, 1, 1),
            a_few(1157, 4, 1, 1),
            a_few(3021, 10, 1, 1),
        ],
        // Empress of Light.
        636 => vec![
            a_few(4823, 15, 1, 1),
            a_few(4778, 4, 3, 3),
            a_few(4715, 50, 1, 1),
            a_few(4784, 7, 1, 1),
            a_few(5075, 20, 1, 1),
        ],
        // Queen Slime. Item 4958 is her trophy, not part of this boss-specific registration at
        // all — it belongs to the generic `RegisterBossTrophies` pass (`trophy()`, above), at the
        // standard 1-in-10 rate, in every mode, the same as every other boss's. This arm used to
        // carry a stray `a_few(4958, 20, 1, 1)` — half the real rate and wrongly classic-only,
        // since `trophy()` itself had no npc-657 case for it to double up with. Removed here now
        // that `trophy()` covers it correctly instead.
        657 => vec![
            a_few(4986, 1, 25, 75),
            a_few(4959, 7, 1, 1),
            a_few(4758, 4, 1, 1),
            a_few(4981, 4, 1, 1),
            // `NotScalingWithLuck(4980, 3)` (`ItemDropDatabase.cs:317`), alone among the five:
            // the Volatile Gelatin is the one Queen Slime drop luck cannot help with.
            not_scaling(a_few(4980, 3, 1, 1)),
        ],
        13 => vec![
            a_few(56, 1, 20, 60),
            a_few(994, 20, 1, 1),
            a_few(2111, 7, 1, 1),
        ],
        14 => vec![
            a_few(56, 1, 20, 60),
            a_few(994, 20, 1, 1),
            a_few(2111, 7, 1, 1),
        ],
        15 => vec![
            a_few(56, 1, 20, 60),
            a_few(994, 20, 1, 1),
            a_few(2111, 7, 1, 1),
        ],
        113 => vec![a_few(2105, 7, 1, 1), a_few(367, 1, 1, 1)],
        127 => vec![
            a_few(2107, 7, 1, 1),
            a_few(1225, 1, 15, 30),
            a_few(547, 1, 25, 40),
        ],
        134 => vec![
            a_few(2113, 7, 1, 1),
            a_few(1225, 1, 15, 30),
            a_few(548, 1, 25, 40),
        ],
        // Queen Bee. The Hive Wand at 1129 moved to `conditional_chains` — it and the Bee-armor
        // pool are one `ByCondition(...).OnFailedRoll(OneFromOptions(...))` chain in the game
        // (mutually exclusive, not two independent rolls), which this function's independent
        // per-item rolls cannot express.
        //
        // A further, real find while verifying this block against source, now fixed alongside
        // the Creeper's own numerator fix above rather than left disclosed-only: 1130's real
        // `ByCondition(condition, 1130, 4, 10, 30, 3)` has `chanceNumerator: 3`, so the true rate
        // is 3-in-4, not the 1-in-4 this project previously kept.
        222 => vec![
            a_few(2108, 7, 1, 1),
            a_few(1132, 3, 1, 1),
            a_few(1170, 15, 1, 1),
            a_few(2502, 20, 1, 1),
            a_few(5483, 15, 1, 1),
            m_in_n(1130, 3, 4, 10, 30),
            a_few(2431, 1, 17, 30),
        ],
        245 => vec![
            a_few(2110, 7, 1, 1),
            a_few(1294, 4, 1, 1),
            a_few(6158, 6, 1, 1),
            a_few(2218, 1, 4, 8),
        ],
        266 => vec![
            a_few(880, 1, 40, 90),
            a_few(2104, 7, 1, 1),
            a_few(3060, 20, 1, 1),
        ],
        370 => vec![a_few(2588, 7, 1, 1), a_few(2609, 15, 1, 1)],
        398 => vec![
            a_few(3373, 7, 1, 1),
            a_few(4469, 10, 1, 1),
            a_few(3384, 1, 1, 1),
            a_few(3460, 1, 70, 90),
        ],
        551 => vec![a_few(3863, 7, 1, 1), a_few(3883, 4, 1, 1)],
        668 => vec![
            a_few(5109, 7, 1, 1),
            a_few(5098, 3, 1, 1),
            a_few(5101, 3, 1, 1),
            a_few(5113, 3, 1, 1),
            a_few(5385, 14, 1, 1),
        ],
        _ => Vec::new(),
    }
}

/// One fallback chain among the classic-only rolls: tried in order, stopping at the first link
/// that lands — the same shape [`crate::npc_drops::DropChain`] already gives the flat table, kept
/// separate here rather than folded into it.
///
/// `one_in` is the *outer* gate: vanilla writes several of these as
/// `RegisterToNPC(npc, new LeadingConditionRule(cond)).OnSuccess(chain)`, where the whole chain is
/// only reached at all when the condition passes. `1` means no outer gate, which is every chain
/// but the Frost Moon's two. Folding such a gate into the links' own denominators is not possible
/// with integers: the Frost Moon chain's four links are exactly 15, 3, 2, 1 at gate 1, and at
/// gate 10 the second would have to be 447/14.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalChain {
    /// The outer `LeadingConditionRule`'s denominator: the chain is run one time in this.
    pub one_in: u32,
    /// The links, tried in order until one lands.
    pub links: Vec<Conditional>,
}

/// A chain with no outer gate, which is all but two of them.
fn chain(links: Vec<Conditional>) -> ConditionalChain {
    ConditionalChain { one_in: 1, links }
}

/// The classic-only rolls the game writes as an explicit `OnFailedRoll` chain rather than a set of
/// independent rules — `Common(a, N).OnFailedRoll(Common(b, M))` tries `b` only when `a`'s own
/// random roll misses, unlike every rule [`classic_only`] returns, which [`conditional`]'s own
/// consumer rolls independently. Kept as its own function (matching [`one_from`]/[`chance_pools`]'s
/// own precedent of one function per distinct shape) rather than changing `classic_only`'s return
/// type for the sake of three chains out of many flat rules.
///
/// Every one of these three is also `Conditions.NotExpert`-gated in source, exactly like
/// `classic_only`'s own rules — checked once here rather than per match arm. The flat table in
/// [`crate::npc_drops`] cannot express any of this: its own `drops()` takes no [`Conditions`] at
/// all (by design — see its module doc, "the other half... lives in `conditional_drops`"), so a
/// chain moved there would keep firing in expert mode instead of yielding to the boss bag. Checked
/// directly against that constraint rather than routed around it.
pub fn conditional_chains(npc_type: u16, at: Conditions) -> Vec<ConditionalChain> {
    // Everscream (344) and Santa-NK1 (345): each registers one `Common(primary, 15).OnFailedRoll(
    // OneFromOptions(1, a, b, c))` chain (`ItemDropDatabase.cs:373-374, 381-382`) — a 1-in-15 shot
    // at the primary item, and only on that roll's failure a *guaranteed* (bare `1` denominator)
    // pick among the other three. `treasure_bag` has no case for either npc (Frost Moon
    // minibosses have no expert-mode bag), and `RegisterBoss_FrostMoon` never wraps either chain
    // in a `NotExpert` guard the way every chain below is, so both must be checked ahead of this
    // function's own blanket `at.expert` return rather than be silently emptied by it.
    //
    // Flattened into four sequential links using the same "worked by hand" trick as Queen Bee's
    // Hive Wand/armour chain above: with the primary at its own real 1-in-15, the fallback pool's
    // three items must land at 1/3, then (of what is left) 1/2, then whatever remains,
    // guaranteed, to reproduce a uniform 1-in-3 pick across the *whole* 14-in-15 of kills the
    // primary did not claim — checked: 1/15 + (14/15)(1/3) + (14/15)(2/3)(1/2) +
    // (14/15)(2/3)(1/2) each equal 1/15, 14/45, 14/45, 14/45, summing to 1.
    //
    // The outer `LeadingConditionRule(FrostMoonDropGatingChance)` wrapping both whole chains is
    // `ConditionalChain::one_in`, which exists for exactly this: the chain is reached one kill in
    // `frost_moon_gate_denominator`, so at wave 1 in a classic world the whole tree fires on one
    // kill in ten rather than on every one. Without the wave the chain is unreachable, which is
    // right - both NPCs only exist during a Frost Moon, and a caller that does not know the wave
    // does not know the event is up either.
    if let Some(chain_links) = match npc_type {
        344 => Some(vec![
            a_few(1871, 15, 1, 1), // FestiveWings
            a_few(1916, 3, 1, 1),  // ChristmasHook
            a_few(1928, 2, 1, 1),  // ChristmasTreeSword
            always(1930),          // Razorpine
        ]),
        345 => Some(vec![
            a_few(1959, 15, 1, 1), // BabyGrinchMischiefWhistle
            a_few(1931, 3, 1, 1),  // BlizzardStaff
            a_few(1946, 2, 1, 1),  // SnowmanCannon
            always(1947),          // NorthPole
        ]),
        _ => None,
    } {
        return match at.frost_moon_wave {
            Some(wave) => vec![ConditionalChain {
                one_in: frost_moon_gate_denominator(wave, at.expert),
                links: chain_links,
            }],
            None => Vec::new(),
        };
    }
    if at.expert {
        return Vec::new();
    }
    match npc_type {
        // Queen Bee: `ItemDropRule.ByCondition(condition, 1129, 3).OnFailedRoll(
        // OneFromOptionsNotScalingWithLuck(2, 842, 843, 844))` (`ItemDropDatabase.cs:550`) — the
        // Hive Wand at 1/3, and only on failure a further 1/2 chance at one piece of the Bee set,
        // mutually exclusive with the wand rather than an independent extra roll.
        //
        // The nested `OneFromOptions(2, ...)` (a 1/2 gate, then a uniform pick among 3) is itself
        // a shape this flat chain format cannot represent directly, but is exactly reproducible as
        // three further sequential links: worked by hand, wanting each armor piece to land at the
        // real unconditional rate of (2/3 chance the wand already failed) * (1/2 gate) * (1/3
        // pick) = 1/9, the per-item chain denominator at step *i* (given every earlier link in the
        // chain has already failed) is `1 / (1/9 / (1 - already-assigned probability))` — 1/6,
        // then 1/5 of what remains, then 1/4 of what remains after that. Checked: 1/3 (wand) +
        // 1/9*3 (armor) + 1/3 (nothing) sums to 1, and 1/6, then (5/6)*(1/5)=1/6, then
        // (4/6)*(1/4)=1/6 reproduces the real 1/9 per piece exactly.
        // `ByCondition(condition, 1129, 3)` is a `CommonDrop` subclass and scales; the
        // `OneFromOptionsNotScalingWithLuck(2, 842, 843, 844)` it falls through to does not
        // (`ItemDropDatabase.cs:550`). The three hand-worked armour denominators reproduce that
        // pool's own gate, so they inherit its class rather than the wand's.
        222 => vec![chain(vec![
            a_few(1129, 3, 1, 1),
            not_scaling(a_few(842, 6, 1, 1)),
            not_scaling(a_few(843, 5, 1, 1)),
            not_scaling(a_few(844, 4, 1, 1)),
        ])],
        // Skeletron: `ByCondition(condition, 1281, 7).OnFailedRoll(Common(1273,
        // 7)).OnFailedRoll(Common(1313, 7))` (`ItemDropDatabase.cs:563`) — at most one of
        // Skeletron Hand, Bone Sword and Muramasa per kill, never independently.
        //
        // npc 35's own five-item RedHatSkeletron set (audit finding D3) is a *separate*,
        // unconditional roll per item, not part of this fallback chain — see `conditional`'s own
        // `at.red_hat_skeletron` arm, now that `Conditions` carries the per-instance state it
        // needs.
        35 | 36 => vec![chain(vec![
            a_few(1281, 7, 1, 1),
            a_few(1273, 7, 1, 1),
            a_few(1313, 7, 1, 1),
        ])],
        // King Slime: `NotScalingWithLuck(2585, 3).OnFailedRoll(Common(2610))`
        // (`ItemDropDatabase.cs:404`) — 1/3 chance of the Slime Hook, else the Slime Gun
        // guaranteed. Item 2610 previously appeared as a drop nowhere in this project.
        // `NotScalingWithLuck(2585, 3).OnFailedRoll(Common(2610))`
        // (`ItemDropDatabase.cs:404`): the two links are different classes, so the Slime Hook's
        // 1-in-3 is fixed and the Slime Gun that follows it does scale.
        50 => vec![chain(vec![not_scaling(a_few(2585, 3, 1, 1)), always(2610)])],
        _ => Vec::new(),
    }
}

/// The drops the game rolls differently depending on the world's mode.
///
/// The game writes these as one rule with two or three branches and picks the branch at the
/// moment of the kill, so they cannot live in the flat table: the same NPC drops different
/// amounts at different rates in classic, expert and master.
///
/// Only the branches that are plain rolls are here. Several of these rules have a branch that is
/// a treasure bag, a relic or a "one of these" draw, and those need machinery of their own.
fn by_mode(npc_type: u16, at: Conditions) -> Vec<Conditional> {
    /// Pick the branch the world is in: classic, expert, master.
    fn pick(
        at: Conditions,
        classic: Conditional,
        expert: Conditional,
        master: Conditional,
    ) -> Conditional {
        if at.master {
            master
        } else if at.expert {
            expert
        } else {
            classic
        }
    }

    match npc_type {
        // The Eater of Worlds: every segment. Shadow scales and demonite are what the whole
        // shadow-armour branch of progression is made of, and without them it is unreachable.
        13..=15 => vec![
            pick(
                at,
                a_few(86, 2, 1, 2),
                a_few(86, 5, 1, 2),
                a_few(86, 10, 1, 2),
            ),
            pick(
                at,
                a_few(56, 2, 2, 5),
                a_few(56, 2, 1, 3),
                a_few(56, 3, 1, 2),
            ),
        ],
        // King Slime's gel, and the Blue Slime's.
        326 => vec![pick(
            at,
            a_few(1729, 1, 1, 3),
            a_few(1729, 1, 1, 4),
            a_few(1729, 1, 2, 4),
        )],
        325 => vec![pick(
            at,
            a_few(1729, 1, 15, 30),
            a_few(1729, 1, 25, 40),
            a_few(1729, 1, 30, 50),
        )],
        // The Creeper — Brain of Cthulhu's own 20-strong minion escort, npc **267** — not the
        // Brain itself (npc 266, whose own classic-only trophy/orb/heart drops are in
        // `classic_only` and are unaffected by this). `RegisterBoss_BOC` registers this
        // `DropBasedOnMasterAndExpertMode` pair to a *separate* `short type2 = 267` local, a
        // distinct `RegisterToNPC` call from the Brain's own three lines just above
        // (`ItemDropDatabase.cs:501-503`). Wiring it to 266 was a real bug — this comment's own
        // earlier wording ("The Brain of Cthulhu: tissue samples and crimtane") was itself part
        // of the mistake, describing the wrong NPC — found by a parallel audit and independently
        // confirmed here directly against source.
        //
        // A second, real finding along the way, now fixed rather than left disclosed-only: every
        // one of these six branches' real `chanceNumerator` is **2** (`new CommonDrop(1329, 3, 2,
        // 5, 2)` etc. — the fifth constructor argument), not the implicit `1` `Conditional` could
        // represent at the time this was first found. True classic/expert odds are 2-in-3 for both
        // items; master's 1329 branch is 2-in-4 (an exact 1-in-2). `m_in_n` (above) now carries the
        // numerator generally, so this — and the same class of gap on the Black Recluse and Queen
        // Bee's own 1130 roll — is fixed rather than only disclosed.
        267 => vec![
            pick(
                at,
                m_in_n(1329, 2, 3, 2, 5),
                m_in_n(1329, 2, 3, 1, 3),
                m_in_n(1329, 2, 4, 1, 2),
            ),
            pick(
                at,
                m_in_n(880, 2, 3, 5, 12),
                m_in_n(880, 2, 3, 5, 7),
                m_in_n(880, 2, 3, 2, 4),
            ),
        ],
        // A wyvern's souls of flight, which are more generous in expert.
        87 => vec![if at.expert {
            a_few(575, 1, 10, 20)
        } else {
            a_few(575, 1, 5, 10)
        }],
        // The Dungeon Guardian's bone key.
        185 => vec![if at.expert {
            a_few(5070, 1, 1, 3)
        } else {
            a_few(5070, 1, 1, 2)
        }],
        // Giant worms and their kin: the whoopie cushion.
        10 | 11 | 12 | 95 | 96 | 97 => vec![sometimes(215, 50)],
        // Hornets and their variants: the stinger. `DropBasedOnExpertMode(CommonDrop(209, 3, 1, 1,
        // 2), Common(209))` (`ItemDropDatabase.cs:1170`) — expert's plain `Common(209)` really is
        // unconditional, but classic's own `CommonDrop` has `chanceNumerator: 2`, so the real rate
        // is 2-in-3, not the 1-in-3 kept here before. A genuinely new find, not one of the two
        // already known when this numerator audit started (Brain of Cthulhu/Creeper's tissue
        // sample, Queen Bee's 1130 roll) or the already-disclosed Black Recluse gap above — nothing
        // in this module had flagged this site before this pass checked every `chanceNumerator`
        // literal in `ItemDropDatabase.cs` against what this file actually models.
        42 | 231 | 232 | 233 | 234 | 235 => vec![if at.expert {
            always(209)
        } else {
            m_in_n(209, 2, 3, 1, 1)
        }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn plain() -> Conditions {
        Conditions::default()
    }

    /// A classic world with a Frost Moon running at `wave`, which is what every Frost Moon boss
    /// drop needs before it exists at all.
    fn frost_wave(wave: i32) -> Conditions {
        Conditions {
            frost_moon_wave: Some(wave),
            ..Conditions::default()
        }
    }

    /// A boss drops a bag in expert and not otherwise.
    #[test]
    fn expert_turns_a_boss_into_a_bag() {
        let classic = conditional(4, plain());
        assert!(
            !classic.iter().any(|d| d.item == 3319),
            "a bag in classic mode"
        );

        let expert = conditional(
            4,
            Conditions {
                expert: true,
                ..plain()
            },
        );
        assert!(expert.iter().any(|d| d.item == 3319 && d.one_in == 1));
    }

    /// Every bag and trophy is a real item, and no two bosses share one.
    #[test]
    fn the_bags_and_trophies_are_unique() {
        let mut bags = HashSet::new();
        let mut trophies = HashSet::new();
        for npc_type in 0..700u16 {
            if let Some(bag) = treasure_bag(npc_type) {
                // Two segments of the same worm share a bag, and the two eyes of Skeletron do too.
                bags.insert(bag);
            }
            if let Some(trophy) = trophy(npc_type) {
                trophies.insert(trophy);
            }
        }
        // Nineteen, not fifteen: Betsy, the Empress of Light, Queen Slime and Deerclops were
        // absent, and all four suppress their classic loot in expert, so an expert kill of any of
        // them used to yield nothing whatsoever. `ItemDropDatabase.cs` registers exactly these
        // nineteen `BossBag`/`BossBagByCondition` items.
        assert_eq!(bags.len(), 19, "nineteen bosses have bags: {bags:?}");
        // Moon Lord, Empress of Light and Deerclops added: their trophies (3595, 4783, 5108) were
        // simply absent from this table before, found by tools/check_drops.py against source.
        // Queen Slime's (4958) added later still: it was present only as a stray, wrongly
        // classic-only, half-rate entry in `classic_only(657)` — this table's own count of 19 was
        // itself proof of the miscount, since real vanilla's `RegisterBossTrophies` registers
        // twenty of these at the standard rate.
        //
        // The Frost Moon's three (1960, 1961, 1962) added later still, at that same standard
        // rate rather than their own real wave-gated one — see `trophy`'s own doc for why.
        assert_eq!(
            trophies.len(),
            21,
            "and twenty-one have trophies here: {trophies:?}"
        );
        // The Twins are the one boss whose halves have different trophies, and both of theirs are
        // registered as a plain `Common` (`ItemDropDatabase.cs:896-897`), so they belong to the
        // generated table rather than this one. They were in both, at nearly twice the rate.
        assert_eq!(trophy(125), None);
        assert_eq!(trophy(126), None);
        let flat_trophy = |npc: u16, item: u16| {
            crate::npc_drops::drops(npc)
                .iter()
                .flat_map(|chain| chain.iter())
                .any(|d| d.item == item && d.one_in == 10)
        };
        assert!(
            flat_trophy(125, 1368),
            "Retinazer's, at the standard 1-in-10"
        );
        assert!(flat_trophy(126, 1369), "and Spazmatism's");
        // ...but they share a bag.
        assert_eq!(treasure_bag(125), treasure_bag(126));
    }

    /// Hardmode materials do not drop before hardmode.
    #[test]
    fn hardmode_materials_wait_for_hardmode() {
        let underground_evil = Conditions {
            underground: true,
            in_corruption: true,
            ..plain()
        };
        assert!(
            conditional(3, underground_evil).is_empty(),
            "souls before the wall fell"
        );

        let after = Conditions {
            hard_mode: true,
            ..underground_evil
        };
        assert!(
            conditional(3, after).iter().any(|d| d.item == 547),
            "and none after"
        );
    }

    /// The souls are biome-specific: the evil's soul does not drop in the hallow.
    #[test]
    fn each_soul_keeps_to_its_own_biome() {
        let hallow = Conditions {
            hard_mode: true,
            underground: true,
            in_hallow: true,
            ..plain()
        };
        let drops: HashSet<u16> = conditional(3, hallow).iter().map(|d| d.item).collect();
        assert!(drops.contains(&548), "Soul of Light");
        assert!(!drops.contains(&547), "but not Soul of Night");
    }

    /// A soul needs depth as well as a biome.
    #[test]
    fn souls_need_depth() {
        let surface = Conditions {
            hard_mode: true,
            in_corruption: true,
            ..plain()
        };
        assert!(conditional(3, surface).is_empty(), "souls on the surface");
    }

    /// Plantera drops the Temple Key, every time, in a classic world.
    ///
    /// This test used to be named for the key and assert item **1293** — the Lihzahrd Power Cell,
    /// which the game drops from temple *enemies* and never from Plantera. So it passed while the
    /// key it is named after was not implemented at all, and the Jungle Temple could never be
    /// opened: no Golem, no Cultist, no Moon Lord. A test can hide a gap as easily as find one.
    #[test]
    fn plantera_drops_the_temple_key() {
        const TEMPLE_KEY: u16 = 1141;

        let first = Conditions {
            hard_mode: true,
            ..plain()
        };
        assert!(
            conditional(262, first).iter().any(|d| d.item == TEMPLE_KEY),
            "the first Plantera kill must give the key, or the temple never opens",
        );

        let again = Conditions {
            downed_plantera: true,
            ..first
        };
        assert!(
            conditional(262, again).iter().any(|d| d.item == TEMPLE_KEY),
            "and so must every kill after it",
        );

        // The Grenade Launcher and its rockets are the half that really is first-kill only.
        assert!(conditional(262, first).iter().any(|d| d.item == 758));
        assert!(!conditional(262, again).iter().any(|d| d.item == 758));

        // In expert the treasure bag carries all of it instead.
        let expert = Conditions {
            expert: true,
            ..first
        };
        assert!(
            !conditional(262, expert)
                .iter()
                .any(|d| d.item == TEMPLE_KEY)
        );
    }

    /// The Twins' loot lands once, when the second of them dies.
    #[test]
    fn the_twins_drop_only_when_the_pair_is_finished() {
        const SOUL_OF_SIGHT: u16 = 549;

        let alone = Conditions {
            hard_mode: true,
            ..plain()
        };
        for twin in [125u16, 126] {
            assert!(
                conditional(twin, alone)
                    .iter()
                    .all(|d| d.item != SOUL_OF_SIGHT),
                "killing one twin while the other lives must give nothing",
            );
        }

        let finished = Conditions {
            other_twin_dead: true,
            ..alone
        };
        assert!(
            conditional(125, finished)
                .iter()
                .any(|d| d.item == SOUL_OF_SIGHT),
            "no Soul of Sight means no Drax, so no Chlorophyte",
        );
    }

    /// Every boss gives up the item the next step of the game needs.
    ///
    /// This is the test the project did not have, and its absence is why three separate blockers
    /// sat behind 1,324 passing tests. The other drop tests check breadth — that a good many types
    /// drop *something*, that a chain stops at its first success. None of them walked the chain,
    /// so nothing noticed that the Temple Key was missing and the run could not be finished.
    ///
    /// Each row is "kill this, get that, or the game stops here".
    #[test]
    fn the_progression_chain_is_unbroken() {
        // (boss, item, what it unlocks)
        const CHAIN: &[(u16, u16, &str)] = &[
            (4, 56, "Eye of Cthulhu -> demonite, in a corruption world"),
            (13, 56, "Eater of Worlds -> demonite"),
            (266, 880, "Brain of Cthulhu -> crimtane"),
            (
                113,
                367,
                "Wall of Flesh -> the Pwnhammer, so altars can be broken",
            ),
            (134, 548, "The Destroyer -> Soul of Might"),
            (125, 549, "The Twins -> Soul of Sight"),
            (127, 547, "Skeletron Prime -> Soul of Fright"),
            (134, 1225, "a mechanical boss -> Hallowed Bars"),
            (262, 1141, "Plantera -> the Temple Key, so the temple opens"),
            (245, 1294, "Golem -> the Picksaw"),
            (398, 3460, "Moon Lord -> luminite"),
        ];

        let at = Conditions {
            hard_mode: true,
            other_twin_dead: true,
            ..plain()
        };

        let mut broken = Vec::new();
        for &(boss, item, what) in CHAIN {
            let dropped = conditional(boss, at).iter().any(|d| d.item == item)
                || crate::npc_drops::drops(boss)
                    .iter()
                    .any(|chain| chain.iter().any(|d| d.item == item));
            if !dropped {
                broken.push(what);
            }
        }
        assert!(
            broken.is_empty(),
            "{} link(s) of the progression chain are missing:\n  {}",
            broken.len(),
            broken.join("\n  "),
        );
    }

    /// The progression chain's items exist, at the exact vanilla roll — not just presence.
    ///
    /// `the_progression_chain_is_unbroken` above only checks that each item can drop at all. That
    /// caught missing items, but a rate that quietly drifted (an `OnFailedRoll` fallback promoted to
    /// the primary roll, a `min`/`max` transposed, a `ByCondition` denominator typo'd) would pass it
    /// just as happily — a link that drops the right item nine times rarer than vanilla is still "on
    /// the chain" by that test's own standard. This is the other half: the exact `one_in` and stack
    /// range each link uses in a classic, non-expert world, each cited to the `ItemDropDatabase.cs`
    /// call that registers it. Expert/master replace the item with (or add to) a treasure bag instead
    /// — covered separately by `expert_turns_a_boss_into_a_bag` — so every row here is evaluated
    /// under the same `Conditions.NotExpert` gate real vanilla uses.
    #[test]
    fn the_progression_chain_rates_match_vanilla() {
        /// One progression-critical drop, pinned against the exact call that registers it.
        struct Row {
            boss: u16,
            item: u16,
            one_in: u32,
            min: i16,
            max: i16,
            world_is_crimson: bool,
            source: &'static str,
        }

        const ROWS: &[Row] = &[
            Row {
                boss: 4,
                item: 56,
                one_in: 1,
                min: 30,
                max: 90,
                world_is_crimson: false,
                source: "Eye of Cthulhu -> Demonite Ore (corruption world): \
                         `ByCondition(condition3, 56, 1, 30, 90)`, ItemDropDatabase.cs:487",
            },
            // Item 56 has *two* independent registrations against npc 13-15 in source: a small
            // per-segment roll every worm piece can drop (`by_mode`'s own
            // `DropBasedOnMasterAndExpertMode`, ItemDropDatabase.cs:512) and this one, gated on
            // `LegacyHack_IsBossAndNotExpert` (true only for the segment the game itself flags as
            // the boss, ordinarily the head) at a flat, always-on `chanceDenominator: 1`. `conditional`
            // registers `classic_only`'s entries before `by_mode`'s (`conditional_drops.rs:253` runs
            // ahead of `:492`), so this is the one a plain `.find()` on item 56 actually reaches —
            // and, being unconditional rather than a 1-in-2 roll, it is the more reliable source of
            // the ore for progression purposes regardless.
            Row {
                boss: 13,
                item: 56,
                one_in: 1,
                min: 20,
                max: 60,
                world_is_crimson: false,
                source: "Eater of Worlds -> Demonite Ore (boss-flagged segment): \
                         `ByCondition(condition2, 56, 1, 20, 60)`, ItemDropDatabase.cs:517",
            },
            Row {
                boss: 266,
                item: 880,
                one_in: 1,
                min: 40,
                max: 90,
                world_is_crimson: false,
                source: "Brain of Cthulhu -> Crimtane Ore: `ByCondition(condition, 880, 1, 40, 90)`, \
                         ItemDropDatabase.cs:498",
            },
            Row {
                boss: 113,
                item: 367,
                one_in: 1,
                min: 1,
                max: 1,
                world_is_crimson: false,
                source: "Wall of Flesh -> Pwnhammer: `ByCondition(condition, 367)`, \
                         ItemDropDatabase.cs:580",
            },
            Row {
                boss: 134,
                item: 548,
                one_in: 1,
                min: 25,
                max: 40,
                world_is_crimson: false,
                source: "The Destroyer -> Soul of Might: `ByCondition(condition, 548, 1, 25, 40)`, \
                         ItemDropDatabase.cs:453",
            },
            Row {
                boss: 125,
                item: 549,
                one_in: 1,
                min: 25,
                max: 40,
                world_is_crimson: false,
                source: "The Twins -> Soul of Sight: `Common(549, 1, 25, 40)`, \
                         ItemDropDatabase.cs:465",
            },
            Row {
                boss: 127,
                item: 547,
                one_in: 1,
                min: 25,
                max: 40,
                world_is_crimson: false,
                source: "Skeletron Prime -> Soul of Fright: `ByCondition(condition, 547, 1, 25, \
                         40)`, ItemDropDatabase.cs:440",
            },
            Row {
                boss: 134,
                item: 1225,
                one_in: 1,
                min: 15,
                max: 30,
                world_is_crimson: false,
                source: "a mechanical boss -> Hallowed Bar: `ByCondition(condition, 1225, 1, 15, \
                         30)`, ItemDropDatabase.cs:452",
            },
            Row {
                boss: 262,
                item: 1141,
                one_in: 1,
                min: 1,
                max: 1,
                world_is_crimson: false,
                source: "Plantera -> Temple Key: `Common(1141)`, ItemDropDatabase.cs:420",
            },
            Row {
                boss: 245,
                item: 1294,
                one_in: 4,
                min: 1,
                max: 1,
                world_is_crimson: false,
                source: "Golem -> Picksaw: `ByCondition(condition, 1294, 4)`, \
                         ItemDropDatabase.cs:652",
            },
            Row {
                boss: 398,
                item: 3460,
                one_in: 1,
                min: 70,
                max: 90,
                world_is_crimson: false,
                source: "Moon Lord -> Luminite: `ByCondition(condition, 3460, 1, 70, 90)`, \
                         ItemDropDatabase.cs:604",
            },
        ];

        let mut wrong = Vec::new();
        for row in ROWS {
            let at = Conditions {
                hard_mode: true,
                other_twin_dead: true,
                world_is_crimson: row.world_is_crimson,
                ..plain()
            };
            let got = conditional(row.boss, at)
                .into_iter()
                .find(|d| d.item == row.item);
            match got {
                Some(d)
                    if d.one_in == row.one_in
                        && d.numerator == 1
                        && d.min == row.min
                        && d.max == row.max => {}
                Some(d) => wrong.push(format!(
                    "{}\n    got one_in={} numerator={} min={} max={}, want one_in={} min={} max={}",
                    row.source, d.one_in, d.numerator, d.min, d.max, row.one_in, row.min, row.max
                )),
                None => wrong.push(format!("{}\n    not found", row.source)),
            }
        }
        assert!(
            wrong.is_empty(),
            "{} progression rate(s) drifted from vanilla:\n  {}",
            wrong.len(),
            wrong.join("\n  "),
        );
    }

    /// The Lunatic Cultist gives up the Ancient Manipulator.
    ///
    /// Not on the chain above because the Moon Lord can be reached without it — but every Luminite
    /// item in the game is crafted at it, so without this the ending leads nowhere.
    ///
    /// This test used to assert item **3372** — `ItemID.BossMaskCultist`, the Lunatic Cultist's
    /// cosmetic boss mask (`ItemDropDatabase.cs:590`), not the Ancient Manipulator at all
    /// (`ItemID.LunarCraftingStation`, item **3549**, `ItemDropDatabase.cs:591`). It passed anyway,
    /// because `npc_drops::drops(439)` genuinely does carry both entries — so the test was checking
    /// the wrong claim and the doc comment's own claim went unverified. Same failure mode the
    /// `plantera_drops_the_temple_key` test above already documents finding once: a passing test
    /// naming the wrong item. Now checks 3549, and its exact rate alongside it.
    #[test]
    fn the_cultist_drops_the_ancient_manipulator() {
        const ANCIENT_MANIPULATOR: u16 = 3549;
        let found = crate::npc_drops::drops(439)
            .iter()
            .flat_map(|chain| chain.iter())
            .find(|d| d.item == ANCIENT_MANIPULATOR);
        // `RegisterToNPC(type, ItemDropRule.Common(3549));` (`ItemDropDatabase.cs:591`) — no
        // `ByCondition`, so this is unconditional and always lands: one_in 1, min/max 1.
        assert_eq!(
            found.copied(),
            Some(crate::npc_drops::Drop {
                item: ANCIENT_MANIPULATOR,
                one_in: 1,
                min: 1,
                max: 1,
                // `Common` -> `CommonDrop` -> `info.player.RollLuck`, which is the default and is
                // what makes this drop one of the many a lucky player sees more of.
                luck: crate::npc_drops::LuckScaling::Full,
            }),
        );
    }

    /// An ordinary enemy drops nothing conditional at all.
    #[test]
    fn most_things_drop_nothing_conditional() {
        let everything = Conditions {
            expert: true,
            master: false,
            world_is_crimson: false,
            hard_mode: true,
            downed_plantera: true,
            in_hallow: false,
            in_corruption: false,
            in_crimson: false,
            underground: false,
            other_twin_dead: true,
            blood_moon: true,
            npc_from_statue: false,
            eclipse: true,
            downed_mech_any: true,
            downed_all_mech_bosses: true,
            frost_moon_wave: Some(20),
            pumpkin_moon_wave: Some(1),
            red_hat_skeletron: true,
            empress_genuinely_enraged: true,
        };
        // A bunny, a goldfish, a guide.
        for ordinary in [46u16, 1, 22] {
            assert!(
                conditional(ordinary, everything).is_empty(),
                "{ordinary} dropped something conditional"
            );
        }
    }

    /// A `chance_pools` gate is a real gate: a pool with `one_in: 1` is `always` (matches
    /// `one_from`'s territory in spirit, but this is `chance_pools`' own contract, not a promise
    /// `1` never appears there) and `one_in: 0` would divide by zero in the caller's
    /// `random_ratio` — this project's own tables never emit that, but the type does not forbid
    /// it, so the shape itself is what is pinned here.
    #[test]
    fn chance_pools_are_empty_for_ordinary_enemies() {
        assert!(chance_pools(1, plain()).is_empty(), "a bunny");
        assert!(chance_pools(46, plain()).is_empty(), "a goldfish");
    }

    #[test]
    fn the_eater_of_souls_pool_is_the_real_one_in_one_hundred_seventy_five() {
        let pools = chance_pools(6, plain());
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0], pool(175, &[956, 957, 958]));
    }

    /// The whole hornet family shares one pool — `npcNetIds8` in source, seven NPCs, not six or
    /// eight.
    #[test]
    fn every_hornet_variant_shares_the_same_pool() {
        let expected = pool(100, &[960, 961, 962]);
        for npc in [42u16, 43, 231, 232, 233, 234, 235] {
            assert_eq!(chance_pools(npc, plain()), vec![expected], "npc {npc}");
        }
    }

    /// The goblin summoner's pool tightens in expert (2 -> 1) rather than swapping which items are
    /// in it — the mode axis and the pool axis are independent.
    #[test]
    fn the_goblin_summoners_pool_tightens_in_expert_without_changing_its_items() {
        let classic = chance_pools(471, plain());
        let expert = chance_pools(
            471,
            Conditions {
                expert: true,
                ..plain()
            },
        );
        assert_eq!(classic, vec![pool(2, &[3052, 3053, 3054])]);
        assert_eq!(expert, vec![pool(1, &[3052, 3053, 3054])]);
    }

    /// No pumpkin moon running: the scarecrow family's pool does not exist at all, not a pool with
    /// an infinite or nonsensical denominator.
    #[test]
    fn the_scarecrow_pool_is_absent_without_a_running_pumpkin_moon() {
        assert!(chance_pools(305, plain()).is_empty());
    }

    /// `PumpkinMoonDropGatingChance`'s own formula (`ItemDropDatabase.cs:91-113`), worked by hand
    /// for a classic wave 1 and an expert wave 15: `(24 - wave) / 2.5`, floored, minus one more in
    /// expert, floored at `1` — then the scarecrow pool's own `1-in-10` multiplies in on top.
    #[test]
    fn the_pumpkin_moon_wave_gate_matches_the_games_own_formula() {
        // Wave 1, classic: (24 - 1) / 2.5 = 9.2 -> 9. Combined with the pool's own 1-in-10: 90.
        let classic_wave_1 = Conditions {
            pumpkin_moon_wave: Some(1),
            ..plain()
        };
        assert_eq!(
            chance_pools(305, classic_wave_1),
            vec![pool(90, &[1788, 1789, 1790])]
        );

        // Wave 15, expert: adjusted = 15 + 5 = 20; (24 - 20) / 2.5 = 1.6 -> 1; minus one more for
        // expert = 0, floored at 1. Combined with the pool: 10.
        let expert_wave_15 = Conditions {
            pumpkin_moon_wave: Some(15),
            expert: true,
            ..plain()
        };
        assert_eq!(
            chance_pools(315, expert_wave_15).len(),
            0,
            "315 has no chance_pools entry — its item lives in conditional() instead"
        );
        assert_eq!(
            chance_pools(305, expert_wave_15),
            vec![pool(10, &[1788, 1789, 1790])]
        );
    }

    /// The Headless Horseman's own Pumpkin Medallion: same wave gate as the scarecrows, combined
    /// with its own separate 1-in-20 — `24` (its own gate's item, not a pool) rather than `230`
    /// would be the bug this pins against (multiplying wrongly, or not gating on the wave at all).
    #[test]
    fn the_headless_horseman_drops_the_medallion_only_while_a_pumpkin_moon_runs() {
        assert!(
            conditional(315, plain()).is_empty(),
            "no pumpkin moon running at all"
        );
        let wave_1 = Conditions {
            pumpkin_moon_wave: Some(1),
            ..plain()
        };
        let drops = conditional(315, wave_1);
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item, 1857);
        // (24 - 1) / 2.5 = 9.2 -> 9, times the rule's own 20: 180.
        assert_eq!(drops[0].one_in, 180);
    }

    /// A statue farm must not be able to grind the blood-moon fish drops.
    #[test]
    fn blood_moon_drops_need_a_real_spawn_not_a_statue() {
        let farmed = Conditions {
            blood_moon: true,
            npc_from_statue: true,
            ..plain()
        };
        assert!(
            conditional(489, farmed).is_empty(),
            "a statue-spawned zombie should give nothing extra"
        );

        let wild = Conditions {
            blood_moon: true,
            npc_from_statue: false,
            ..plain()
        };
        assert!(conditional(489, wild).iter().any(|d| d.item == 4271));
    }

    /// Pixie needs any one mechanical boss; the Reaper (an eclipse-only NPC) needs all three — the
    /// weaker and stronger versions of the same underlying condition must not be conflated.
    #[test]
    fn beat_any_mech_boss_and_downed_all_mech_bosses_are_genuinely_different_gates() {
        let only_one = Conditions {
            downed_mech_any: true,
            downed_all_mech_bosses: false,
            ..plain()
        };
        assert!(conditional(75, only_one).iter().any(|d| d.item == 5662));
        assert!(
            conditional(253, only_one).is_empty(),
            "one mech boss down should not be enough for the Reaper"
        );

        let all_three = Conditions {
            downed_mech_any: true,
            downed_all_mech_bosses: true,
            ..plain()
        };
        assert!(conditional(253, all_three).iter().any(|d| d.item == 1327));
    }

    /// Bug #1: Moon Lord's own ten-weapon pool was missing entirely — no case for npc 398 existed
    /// anywhere in this module, so classic-mode kills granted none of the signature weapons.
    /// Fails on the unfixed code (`moon_lord_weapons` did not exist / returned an empty pool for
    /// 398), passes now that the pool is wired up and correctly classic-only.
    #[test]
    fn moon_lord_has_his_own_weapon_pool() {
        const MOON_LORD: u16 = 398;
        let pool = moon_lord_weapons(MOON_LORD, plain());
        assert_eq!(pool.len(), 10, "the real ten-item pool: {pool:?}");
        for item in [3063, 3389, 3065, 1553, 3930, 3541, 3570, 3571, 3569, 5480] {
            assert!(pool.contains(&item), "missing {item} from the real pool");
        }

        assert!(
            moon_lord_weapons(
                MOON_LORD,
                Conditions {
                    expert: true,
                    ..plain()
                }
            )
            .is_empty(),
            "expert replaces this with the treasure bag, same as every other boss"
        );
        assert!(
            moon_lord_weapons(50, plain()).is_empty(),
            "King Slime has no business getting Moon Lord's pool"
        );
    }

    /// Bug #2: Queen Bee's Hive Wand and Bee-armor pool used to be modelled as independent rolls
    /// (`classic_only`'s flat `a_few(1129, 3, 1, 1)` plus a guaranteed `one_from` armor pool),
    /// which made the armor unconditional and let it co-occur with the wand — real vanilla is one
    /// `OnFailedRoll` chain, mutually exclusive, at an overall ~1/3. Fails on the unfixed code
    /// (1129 was in `classic_only`, not this chain, and the armor pool had no gate at all).
    #[test]
    fn queen_bees_bee_drops_are_one_chain_not_independent_rolls() {
        const QUEEN_BEE: u16 = 222;
        let chains = conditional_chains(QUEEN_BEE, plain());
        assert_eq!(chains.len(), 1, "one chain, not several: {chains:?}");
        let chain = &chains[0];
        assert_eq!(
            chain.one_in, 1,
            "no outer gate on this one: Queen Bee's chain is registered directly"
        );
        assert_eq!(
            chain.links,
            vec![
                a_few(1129, 3, 1, 1),
                not_scaling(a_few(842, 6, 1, 1)),
                not_scaling(a_few(843, 5, 1, 1)),
                not_scaling(a_few(844, 4, 1, 1)),
            ]
        );
        // 1129 must no longer also appear as an independent `classic_only` roll, or the chain
        // above and the old flat rule would both fire and double the wand's real odds.
        assert!(!classic_only(QUEEN_BEE).iter().any(|d| d.item == 1129));
        // Nor may the armor pieces still be an *unconditional* `one_from` pool — that was the
        // original bug (found by this exact assertion failing against a real, unfixed
        // `one_from(222)` while writing this test's own server.rs sibling): a guaranteed second
        // pool alongside this chain would give an armor piece on every single kill, chain or not.
        for pool in one_from(QUEEN_BEE, plain()) {
            for item in [842, 843, 844] {
                assert!(
                    !pool.contains(&item),
                    "one_from(222) must not also guarantee bee armor: {pool:?}"
                );
            }
        }

        assert!(
            conditional_chains(
                QUEEN_BEE,
                Conditions {
                    expert: true,
                    ..plain()
                }
            )
            .is_empty(),
            "expert replaces this with the treasure bag"
        );
    }

    /// Bug #3: Skeletron's three weapons used to be three independent `classic_only` rolls, so a
    /// single kill could grant 0, 1, 2 or all 3 — real vanilla is one chain, stopping at the
    /// first that lands, so at most one per kill. Fails on the unfixed code (all three were
    /// independent entries in `classic_only(35 | 36)`).
    #[test]
    fn skeletrons_three_weapons_are_one_chain_not_three_independent_rolls() {
        for skeletron in [35u16, 36] {
            let chains = conditional_chains(skeletron, plain());
            assert_eq!(chains.len(), 1, "npc {skeletron}: {chains:?}");
            assert_eq!(
                chains[0].links,
                vec![
                    a_few(1281, 7, 1, 1),
                    a_few(1273, 7, 1, 1),
                    a_few(1313, 7, 1, 1),
                ]
            );
            // None of the three may also be an independent `classic_only` roll any more.
            let flat = classic_only(skeletron);
            for item in [1281, 1273, 1313] {
                assert!(
                    !flat.iter().any(|d| d.item == item),
                    "npc {skeletron}: {item} must not also roll independently"
                );
            }
        }
    }

    /// Bug #4: the tissue-sample/crimtane rule was wired to npc 266 (the Brain of Cthulhu itself),
    /// but real vanilla's `RegisterBoss_BOC` registers it to npc **267** (the Creeper, the Brain's
    /// own escort) — a separate `RegisterToNPC` call on a separate local. Fails on the unfixed
    /// code (the rule lived on 266, and 267 had nothing).
    #[test]
    fn the_creeper_not_the_brain_carries_the_tissue_sample_and_crimtane_rule() {
        const BRAIN_OF_CTHULHU: u16 = 266;
        const CREEPER: u16 = 267;
        let hard_mode = Conditions {
            hard_mode: true,
            ..plain()
        };

        let creeper_drops = conditional(CREEPER, hard_mode);
        assert!(
            creeper_drops.iter().any(|d| d.item == 1329),
            "the Creeper should carry the Tissue Sample rule: {creeper_drops:?}"
        );
        assert!(
            creeper_drops.iter().any(|d| d.item == 880),
            "and the mode-scaled Crimtane Ore rule: {creeper_drops:?}"
        );

        let brain_drops = conditional(BRAIN_OF_CTHULHU, hard_mode);
        assert!(
            !brain_drops.iter().any(|d| d.item == 1329),
            "the Brain itself must not carry the Creeper's own rule: {brain_drops:?}"
        );
        // The Brain's own real classic-only drops (its own 880 roll included) are untouched.
        assert!(
            classic_only(BRAIN_OF_CTHULHU).iter().any(|d| d.item == 880),
            "the Brain's own separate classic-only crimtane roll must still be there"
        );
    }

    /// Bug #5: King Slime never dropped the Slime Gun — `classic_only(50)` only ever had the 1/3
    /// Slime Hook roll, with no fallback. Real vanilla: 1/3 Slime Hook, else the Slime Gun
    /// guaranteed. Fails on the unfixed code (item 2610 appeared nowhere).
    #[test]
    fn king_slime_falls_back_to_the_slime_gun() {
        const KING_SLIME: u16 = 50;
        let chains = conditional_chains(KING_SLIME, plain());
        assert_eq!(chains.len(), 1);
        assert_eq!(
            chains[0].links,
            vec![not_scaling(a_few(2585, 3, 1, 1)), always(2610)],
            "1/3 Slime Hook, else the Slime Gun guaranteed - and the Hook is the \
             `NotScalingWithLuck` link, so luck cannot help with it"
        );
        assert!(
            !classic_only(KING_SLIME).iter().any(|d| d.item == 2585),
            "2585 must not also roll independently now that it is chained"
        );
    }

    /// Bug #6: Queen Slime's trophy (4958) was missing from the generic `trophy()` table and
    /// instead sat as a stray, half-rate (1/20 vs. the real 1/10), wrongly classic-only entry in
    /// `classic_only(657)`. Fails on the unfixed code (`trophy(657)` was `None`, and the trophy
    /// dropped at the wrong rate only outside expert mode).
    #[test]
    fn queen_slimes_trophy_is_in_the_generic_table_at_the_standard_rate() {
        const QUEEN_SLIME: u16 = 657;
        assert_eq!(trophy(QUEEN_SLIME), Some(4958));

        // `conditional()` folds `trophy()` in at the standard 1-in-10, in every mode — including
        // expert, unlike the ordinary loot below it.
        let expert = conditional(
            QUEEN_SLIME,
            Conditions {
                expert: true,
                ..plain()
            },
        );
        assert!(
            expert.iter().any(|d| d.item == 4958 && d.one_in == 10),
            "the trophy still drops in expert, at 1-in-10: {expert:?}"
        );

        assert!(
            !classic_only(QUEEN_SLIME).iter().any(|d| d.item == 4958),
            "the stray classic-only half-rate entry must be gone"
        );
    }

    /// Bug #7: drawing Stynger (1258) from Golem's weapon pool must also grant its own 60-180
    /// Stynger Bolt (1261) — a nested `OnSuccess` in source. Fails on the unfixed code
    /// (`bundled_with` did not exist, so item 1261 had no source anywhere in this project).
    #[test]
    fn golems_stynger_brings_its_own_ammunition() {
        assert_eq!(bundled_with(1258), Some((1261, 60, 180)));
        // Nothing else in Golem's pool bundles anything.
        for item in [1122, 899, 1248, 1295, 1296, 1297] {
            assert_eq!(bundled_with(item), None, "item {item} should not bundle");
        }
    }

    /// A numerator audit over every `chanceNumerator` literal in `ItemDropDatabase.cs`, run after
    /// the Brain of Cthulhu/Creeper npc-id fix and the Queen Bee chain fix above disclosed (but did
    /// not fix) two `Conditional` entries whose real numerator was not `1`. Adding `m_in_n` closes
    /// those two, plus two more the audit itself found: the Black Recluse's expert branch (already
    /// named in a comment as a known gap, just never wired to `m_in_n` before it existed) and the
    /// hornet family's stinger roll (not previously flagged anywhere in this module).
    ///
    /// The Creeper's own six branches: `new CommonDrop(1329, 3, 2, 5, 2)`,
    /// `new CommonDrop(1329, 3, 1, 3, 2)`, `new CommonDrop(1329, 4, 1, 2, 2)`,
    /// `new CommonDrop(880, 3, 5, 12, 2)`, `new CommonDrop(880, 3, 5, 7, 2)`,
    /// `new CommonDrop(880, 3, 2, 4, 2)` (`ItemDropDatabase.cs:502-503`) — every one has
    /// `chanceNumerator: 2`. Fails on the pre-fix code, which had no numerator field at all and so
    /// modelled every branch here at the wrong (too-low) 1-in-N rate.
    #[test]
    fn the_creepers_tissue_sample_and_crimtane_rolls_are_two_in_three_or_one_in_two_not_one_in_three_or_one_in_four()
     {
        const CREEPER: u16 = 267;

        let classic = conditional(CREEPER, plain());
        let tissue = classic
            .iter()
            .find(|d| d.item == 1329)
            .expect("classic tissue sample");
        assert_eq!(
            (tissue.numerator, tissue.one_in, tissue.min, tissue.max),
            (2, 3, 2, 5),
            "classic tissue sample must be 2-in-3, not 1-in-3: {tissue:?}"
        );
        let crimtane = classic
            .iter()
            .find(|d| d.item == 880)
            .expect("classic crimtane");
        assert_eq!(
            (
                crimtane.numerator,
                crimtane.one_in,
                crimtane.min,
                crimtane.max
            ),
            (2, 3, 5, 12),
            "classic crimtane must be 2-in-3, not 1-in-3: {crimtane:?}"
        );

        let expert = conditional(
            CREEPER,
            Conditions {
                expert: true,
                ..plain()
            },
        );
        let tissue = expert
            .iter()
            .find(|d| d.item == 1329)
            .expect("expert tissue sample");
        assert_eq!(
            (tissue.numerator, tissue.one_in, tissue.min, tissue.max),
            (2, 3, 1, 3),
            "expert tissue sample must be 2-in-3, not 1-in-3: {tissue:?}"
        );
        let crimtane = expert
            .iter()
            .find(|d| d.item == 880)
            .expect("expert crimtane");
        assert_eq!(
            (
                crimtane.numerator,
                crimtane.one_in,
                crimtane.min,
                crimtane.max
            ),
            (2, 3, 5, 7),
            "expert crimtane must be 2-in-3, not 1-in-3: {crimtane:?}"
        );

        let master = conditional(
            CREEPER,
            Conditions {
                expert: true,
                master: true,
                ..plain()
            },
        );
        let tissue = master
            .iter()
            .find(|d| d.item == 1329)
            .expect("master tissue sample");
        assert_eq!(
            (tissue.numerator, tissue.one_in, tissue.min, tissue.max),
            (2, 4, 1, 2),
            "master tissue sample must be 2-in-4 (1-in-2), not 1-in-4: {tissue:?}"
        );
        let crimtane = master
            .iter()
            .find(|d| d.item == 880)
            .expect("master crimtane");
        assert_eq!(
            (
                crimtane.numerator,
                crimtane.one_in,
                crimtane.min,
                crimtane.max
            ),
            (2, 3, 2, 4),
            "master crimtane must be 2-in-3, not 1-in-3: {crimtane:?}"
        );
    }

    /// Queen Bee's own `ByCondition(condition, 1130, 4, 10, 30, 3)` (`ItemDropDatabase.cs:551`) —
    /// `chanceNumerator: 3`, so the real rate is 3-in-4, not the 1-in-4 this project kept before
    /// `m_in_n` existed. Fails on the pre-fix code (`a_few(1130, 4, 10, 30)`, an implicit
    /// numerator of 1).
    #[test]
    fn queen_bees_1130_roll_is_three_in_four_not_one_in_four() {
        const QUEEN_BEE: u16 = 222;
        let honey = classic_only(QUEEN_BEE)
            .into_iter()
            .find(|d| d.item == 1130)
            .expect("item 1130");
        assert_eq!(
            (honey.numerator, honey.one_in, honey.min, honey.max),
            (3, 4, 10, 30),
            "must be 3-in-4, not 1-in-4: {honey:?}"
        );
    }

    /// The Black Recluse's own `DropBasedOnExpertMode(Common(2607, 2, 1, 3), CommonDrop(2607, 10,
    /// 1, 3, 9))` (`ItemDropDatabase.cs:959`) — classic is a plain 1-in-2, but expert's own branch
    /// is 9-in-10, not the flat 1-in-2 kept in every mode before this fix (this project's own prior
    /// comment already named the gap, but left both the mode branch and the numerator unfixed).
    /// Fails on the pre-fix code, which returned `a_few(2607, 2, 1, 3)` regardless of `at.expert`.
    #[test]
    fn black_recluses_web_is_mode_branched_not_a_flat_rate_in_every_mode() {
        const BLACK_RECLUSE: u16 = 163;
        let classic = conditional(BLACK_RECLUSE, plain());
        let web = classic
            .iter()
            .find(|d| d.item == 2607)
            .expect("classic web");
        assert_eq!(
            (web.numerator, web.one_in, web.min, web.max),
            (1, 2, 1, 3),
            "classic stays 1-in-2: {web:?}"
        );

        let expert = conditional(
            BLACK_RECLUSE,
            Conditions {
                expert: true,
                ..plain()
            },
        );
        let web = expert.iter().find(|d| d.item == 2607).expect("expert web");
        assert_eq!(
            (web.numerator, web.one_in, web.min, web.max),
            (9, 10, 1, 3),
            "expert must be 9-in-10, not the classic 1-in-2: {web:?}"
        );

        // The wall-mounted variant (238) shares the same rule.
        let mounted = conditional(238, plain());
        assert!(mounted.iter().any(|d| d.item == 2607 && d.numerator == 1));
    }

    /// The hornet family's own `DropBasedOnExpertMode(CommonDrop(209, 3, 1, 1, 2), Common(209))`
    /// (`ItemDropDatabase.cs:1170`) — classic's own branch has `chanceNumerator: 2`, so the real
    /// rate is 2-in-3, not the 1-in-3 this project modelled before. Not one of the two numerator
    /// gaps already disclosed in this module (Creeper, Queen Bee) or the Black Recluse's
    /// already-named one — found fresh by this audit checking every `chanceNumerator` literal in
    /// `ItemDropDatabase.cs` against what this module actually models. Fails on the pre-fix code
    /// (`sometimes(209, 3)`, an implicit numerator of 1).
    #[test]
    fn hornet_stinger_is_two_in_three_in_classic_not_one_in_three() {
        const HORNET: u16 = 42;
        let classic = conditional(HORNET, plain());
        let stinger = classic
            .iter()
            .find(|d| d.item == 209)
            .expect("classic stinger");
        assert_eq!(
            (stinger.numerator, stinger.one_in),
            (2, 3),
            "must be 2-in-3, not 1-in-3: {stinger:?}"
        );

        let expert = conditional(
            HORNET,
            Conditions {
                expert: true,
                ..plain()
            },
        );
        let stinger = expert
            .iter()
            .find(|d| d.item == 209)
            .expect("expert stinger");
        assert_eq!(
            (stinger.numerator, stinger.one_in),
            (1, 1),
            "expert stays unconditional: {stinger:?}"
        );
    }

    /// Everscream (344) used to drop nothing at all: `RegisterBoss_FrostMoon`
    /// (`ItemDropDatabase.cs:363-375`) was never ported, so none of `conditional`,
    /// `conditional_chains` or `trophy` had a case for npc 344. Fails on the pre-fix code, whose
    /// `conditional_chains(344, ..)` returned an empty `Vec` and whose `trophy(344)` returned
    /// `None`.
    #[test]
    fn everscream_drops_its_frost_moon_loot() {
        let chains = conditional_chains(344, frost_wave(20));
        let items: Vec<u16> = chains
            .iter()
            .flat_map(|c| &c.links)
            .map(|c| c.item)
            .collect();
        assert!(items.contains(&1871), "FestiveWings");
        assert!(items.contains(&1916), "ChristmasHook");
        assert!(items.contains(&1928), "ChristmasTreeSword");
        assert!(items.contains(&1930), "Razorpine");
        // Razorpine is the chain's guaranteed last link — the fight that never dropped nothing
        // must never miss it either.
        assert_eq!(chains[0].links.last().map(|c| c.item), Some(1930));
        assert_eq!(chains[0].links.last().map(|c| c.one_in), Some(1));

        assert_eq!(trophy(344), Some(1962), "EverscreamTrophy");

        // No treasure bag exists for this npc, and real vanilla never gates this chain on
        // `NotExpert` the way a true boss's classic-only loot is — so expert must not empty it.
        let expert = conditional_chains(
            344,
            Conditions {
                expert: true,
                ..frost_wave(20)
            },
        );
        assert!(
            !expert.is_empty(),
            "Frost Moon loot must survive expert mode: {expert:?}"
        );
    }

    /// Santa-NK1 (345): same gap as Everscream, plus its own Reindeer Bells (`conditional`) and
    /// its own trophy — real vanilla's `IceQueenTrophy` (1960), not `SantaNK1Trophy`; see
    /// `trophy`'s own doc for why. Fails on the pre-fix code the same way Everscream's test does.
    #[test]
    fn santa_nk1_drops_its_frost_moon_loot() {
        let chains = conditional_chains(345, frost_wave(20));
        let items: Vec<u16> = chains
            .iter()
            .flat_map(|c| &c.links)
            .map(|c| c.item)
            .collect();
        assert!(items.contains(&1959), "BabyGrinchMischiefWhistle");
        assert!(items.contains(&1931), "BlizzardStaff");
        assert!(items.contains(&1946), "SnowmanCannon");
        assert!(items.contains(&1947), "NorthPole");
        assert_eq!(chains[0].links.last().map(|c| c.item), Some(1947));

        assert!(
            conditional(345, frost_wave(20))
                .iter()
                .any(|d| d.item == 1914),
            "ReindeerBells"
        );
        assert_eq!(trophy(345), Some(1960));
    }

    /// `Conditions.FrostMoonDropGatingChance.CanDrop` (`Terraria.GameContent.ItemDropRules/Conditions.cs:57-78`), read straight off
    /// source: `(28 - wave - expertBump) / 2.5`, the `(int)` cast truncating toward zero, minus two
    /// more in expert, floored at 1.
    ///
    /// Every Frost Moon boss drop used to ignore this entirely and fire unconditionally, which at
    /// wave 1 is ten times the rate the game allows and made the event's whole wave progression
    /// pointless: there was no reason to push past wave 1 for loot.
    #[test]
    fn the_frost_moon_wave_gate_matches_the_games_own_formula() {
        // Classic: (28 - wave) / 2.5, truncated.
        for (wave, expected) in [
            (1, 10), // 27 / 2.5 = 10.8 -> 10
            (5, 9),  // 23 / 2.5 = 9.2  -> 9
            (10, 7), // 18 / 2.5 = 7.2  -> 7
            (15, 5), // 13 / 2.5 = 5.2  -> 5
            (20, 3), // 8  / 2.5 = 3.2  -> 3
            (26, 1), // 2  / 2.5 = 0.8  -> 0, floored to 1
            (40, 1), // negative, floored to 1
        ] {
            assert_eq!(
                frost_moon_gate_denominator(wave, false),
                expected,
                "classic wave {wave}"
            );
        }
        // Expert: five waves' worth of head start, then two off the denominator.
        for (wave, expected) in [
            (1, 6),  // (28-6)/2.5 = 8.8 -> 8, minus 2 -> 6
            (10, 3), // (28-15)/2.5 = 5.2 -> 5, minus 2 -> 3
            (15, 1), // (28-20)/2.5 = 3.2 -> 3, minus 2 -> 1
            (20, 1), // floored
        ] {
            assert_eq!(
                frost_moon_gate_denominator(wave, true),
                expected,
                "expert wave {wave}"
            );
        }
        // It is the Frost Moon's own formula and not the Pumpkin Moon's: 28 against 24, and two
        // off in expert against one. Sharing one helper would be wrong in both directions.
        assert_ne!(
            frost_moon_gate_denominator(1, false),
            pumpkin_moon_gate_denominator(1, false)
        );
    }

    /// The gate tightens the loot as the wave falls, on every one of the six places that read it.
    /// A wave-1 kill must be strictly worse than a wave-20 kill everywhere, which is the whole
    /// point of a wave-based event.
    #[test]
    fn a_late_wave_is_better_than_an_early_one_everywhere() {
        // The two chains: the outer gate is the chain's own `one_in`.
        for npc in [344u16, 345] {
            let early = conditional_chains(npc, frost_wave(1));
            let late = conditional_chains(npc, frost_wave(20));
            assert_eq!(early[0].one_in, 10, "npc {npc} at wave 1");
            assert_eq!(late[0].one_in, 3, "npc {npc} at wave 20");
            assert_eq!(
                early[0].links, late[0].links,
                "npc {npc}: the wave changes how often the chain is reached, never what is in it"
            );
        }

        // The Ice Queen's pool.
        assert_eq!(chance_pools(346, frost_wave(1))[0].one_in, 10);
        assert_eq!(chance_pools(346, frost_wave(20))[0].one_in, 3);

        // Santa-NK1's Reindeer Bells: unreachable below wave 15, then the gate times its own 15.
        let bells = |wave| {
            conditional(345, frost_wave(wave))
                .into_iter()
                .find(|d| d.item == 1914)
                .map(|d| d.one_in)
        };
        assert_eq!(
            bells(14),
            None,
            "`FromCertainWaveAndAbove(15)` is a hard floor"
        );
        assert_eq!(bells(15), Some(5 * 15));
        assert_eq!(bells(20), Some(3 * 15));

        // The three trophies: the outer gate times the trophy gate, and nothing at all before 15.
        let trophy_rate = |npc: u16, wave| {
            let want = trophy(npc).expect("these three have trophies");
            conditional(npc, frost_wave(wave))
                .into_iter()
                .find(|d| d.item == want)
                .map(|d| d.one_in)
        };
        for npc in [344u16, 345, 346] {
            assert_eq!(trophy_rate(npc, 14), None, "npc {npc} before wave 15");
            assert_eq!(trophy_rate(npc, 15), Some(5 * 4), "npc {npc} at wave 15");
            assert_eq!(trophy_rate(npc, 18), Some(4 * 3), "npc {npc} at wave 18");
            assert_eq!(trophy_rate(npc, 20), Some(3 * 2), "npc {npc} at wave 20");
        }
    }

    /// A Frost Moon boss that somehow dies with no Frost Moon running drops none of its event
    /// loot, which is what vanilla's own `if (!Main.snowMoon) return false;` says. It is not a
    /// hypothetical: the same three NPCs can be spawned from a statue or by a console command.
    #[test]
    fn no_frost_moon_means_no_frost_moon_loot() {
        for npc in [344u16, 345, 346] {
            assert!(
                conditional_chains(npc, plain()).is_empty(),
                "npc {npc}: no chain without the event"
            );
            assert!(
                chance_pools(npc, plain()).is_empty(),
                "npc {npc}: no pool without the event"
            );
            let want = trophy(npc).expect("a trophy");
            assert!(
                !conditional(npc, plain()).iter().any(|d| d.item == want),
                "npc {npc}: no trophy without the event"
            );
        }
        assert!(
            !conditional(345, plain()).iter().any(|d| d.item == 1914),
            "and no Reindeer Bells"
        );
        // A *pumpkin* moon is not a frost moon: the two gates each check their own.
        let pumpkin = Conditions {
            pumpkin_moon_wave: Some(20),
            ..plain()
        };
        assert!(
            conditional_chains(344, pumpkin).is_empty(),
            "the wrong moon must not open the other one's gate"
        );
    }

    /// The trophy gate is `PumpkinMoonDropGateForTrophies` and `FrostMoonDropGateForTrophies`
    /// sharing one helper, which is only right because the two are the same function twice
    /// (`Conditions.cs:127-166` and `:179-217`, identical apart from which moon each checks).
    /// If either ever diverges this is where it shows.
    #[test]
    fn both_moons_use_the_same_trophy_step() {
        assert_eq!(moon_trophy_gate_denominator(14), None);
        for (wave, expected) in [
            (15, 4),
            (16, 4),
            (17, 3),
            (18, 3),
            (19, 2),
            (20, 2),
            (40, 2),
        ] {
            assert_eq!(
                moon_trophy_gate_denominator(wave),
                Some(expected),
                "wave {wave}"
            );
        }
    }

    /// Ice Queen (346): its `OneFromOptions(1, 1910, 1929)` pool, which is a guaranteed *pick*
    /// behind a wave *gate* and so lives in `chance_pools` rather than `one_from`, and its own
    /// trophy — real vanilla's `SantaNK1Trophy` (1961), not `IceQueenTrophy`; see `trophy`'s own
    /// doc for why.
    #[test]
    fn ice_queen_drops_its_frost_moon_loot() {
        let pools = chance_pools(346, frost_wave(20));
        assert_eq!(pools.len(), 1);
        assert!(pools[0].options.contains(&1910), "ElfMelter");
        assert!(pools[0].options.contains(&1929), "ChainGun");
        assert_eq!(trophy(346), Some(1961));
        assert!(
            one_from(346, frost_wave(20)).is_empty(),
            "the guaranteed-pool table must not also carry it, or it drops twice"
        );

        // No treasure bag for this npc either, so expert must not empty the pool.
        let expert = chance_pools(
            346,
            Conditions {
                expert: true,
                ..frost_wave(20)
            },
        );
        assert_eq!(expert.len(), 1, "must survive expert mode: {expert:?}");
    }

    /// `ai[3] == 1` (`Conditions.red_hat_skeletron`, `NPC.cs:67435-67446`) is what unlocks
    /// Skeletron's own RedHatSkeletron set, and it is unconditional on expert/classic mode
    /// (`ItemDropDatabase.cs:565-569` carries no `NotExpert` wrapper the way the weapon chain
    /// does) — audit finding D3.
    #[test]
    fn red_hat_skeletron_only_drops_its_own_set_when_ai3_is_set() {
        const SKELETRON: u16 = 35;
        const RED_HAT_SET: [u16; 5] = [5624, 5625, 5626, 5628, 5737];

        let ordinary = conditional(SKELETRON, plain());
        for item in RED_HAT_SET {
            assert!(
                !ordinary.iter().any(|d| d.item == item),
                "an ordinary Skeletron should not drop item {item}"
            );
        }

        let red_hat = conditional(
            SKELETRON,
            Conditions {
                red_hat_skeletron: true,
                ..plain()
            },
        );
        for item in RED_HAT_SET {
            let drop = red_hat.iter().find(|d| d.item == item).unwrap_or_else(|| {
                panic!("red-hat Skeletron should drop item {item}: {red_hat:?}")
            });
            assert_eq!(drop.one_in, 1, "item {item} should be unconditional");
        }

        // Expert mode does not gate this set the way it gates the rest of Skeletron's loot.
        let red_hat_expert = conditional(
            SKELETRON,
            Conditions {
                red_hat_skeletron: true,
                expert: true,
                ..plain()
            },
        );
        for item in RED_HAT_SET {
            assert!(
                red_hat_expert.iter().any(|d| d.item == item),
                "item {item} should survive expert mode too: {red_hat_expert:?}"
            );
        }
    }

    /// Mothron's Broken Hero Sword (`ItemDropDatabase.cs:201-203`) only drops once all three
    /// mechanical bosses are down, the same gate the Reaper Tooth Necklace already uses just
    /// above this test's own sibling in `conditional_drops.rs`.
    #[test]
    fn mothron_only_drops_the_broken_hero_sword_after_all_three_mechs() {
        const MOTHRON: u16 = 477;

        let too_early = conditional(MOTHRON, plain());
        assert!(!too_early.iter().any(|d| d.item == 1570));

        let ready = conditional(
            MOTHRON,
            Conditions {
                downed_all_mech_bosses: true,
                ..plain()
            },
        );
        assert!(ready.iter().any(|d| d.item == 1570 && d.one_in == 4));
    }

    /// Pumpking's own weapon pool and trophy, both gated on the shared pumpkin-moon wave —
    /// `ItemDropDatabase.cs:346-353`.
    #[test]
    fn pumpking_drops_its_weapon_pool_and_trophy_only_during_a_pumpkin_moon() {
        const PUMPKING: u16 = 325;

        assert!(
            chance_pools(PUMPKING, plain()).is_empty(),
            "no pumpkin moon running at all"
        );
        assert!(
            !conditional(PUMPKING, plain())
                .iter()
                .any(|d| d.item == 1855)
        );

        let at = Conditions {
            pumpkin_moon_wave: Some(20),
            ..plain()
        };
        let pools = chance_pools(PUMPKING, at);
        assert_eq!(pools.len(), 1, "{pools:?}");
        for item in [1829, 1831, 1835, 1837, 1845] {
            assert!(
                pools[0].options.contains(&item),
                "item {item} missing from Pumpking's pool: {pools:?}"
            );
        }
        assert!(
            conditional(PUMPKING, at).iter().any(|d| d.item == 1855),
            "wave 20 clears the trophy gate's wave-15 floor"
        );
        // Before wave 15 the trophy is unreachable even with the weapon-pool gate open.
        let early_wave = Conditions {
            pumpkin_moon_wave: Some(3),
            ..plain()
        };
        assert!(
            !conditional(PUMPKING, early_wave)
                .iter()
                .any(|d| d.item == 1855)
        );
    }

    /// Mourning Wood's own pool and trophy, the same shape as Pumpking's —
    /// `ItemDropDatabase.cs:354-362`.
    #[test]
    fn mourning_wood_drops_its_own_pool_and_trophy() {
        const MOURNING_WOOD: u16 = 327;

        let at = Conditions {
            pumpkin_moon_wave: Some(20),
            ..plain()
        };
        let pools = chance_pools(MOURNING_WOOD, at);
        assert_eq!(pools.len(), 1, "{pools:?}");
        for item in [1782, 1784, 1811, 1826, 1801, 1802, 4680, 1798] {
            assert!(
                pools[0].options.contains(&item),
                "item {item} missing from Mourning Wood's pool: {pools:?}"
            );
        }
        assert!(
            conditional(MOURNING_WOOD, at)
                .iter()
                .any(|d| d.item == 1856)
        );
    }

    /// The bundled ammunition for the two new pools, alongside Golem's Stynger.
    #[test]
    fn the_pumpkin_moon_bosses_own_weapons_bring_their_own_ammunition() {
        assert_eq!(bundled_with(1835), Some((1836, 30, 60)), "StakeLauncher");
        assert_eq!(bundled_with(1782), Some((1783, 50, 100)), "CandyCornRifle");
        assert_eq!(
            bundled_with(1784),
            Some((1785, 25, 50)),
            "JackOLanternLauncher"
        );
    }
}

/// Which of `Luck`'s three rolls each drop rule uses. Vanilla decides this by the
/// `ItemDropRule` constructor that built the rule, and the difference is real:
/// `CommonDrop.TryDroppingItem` is `info.player.RollLuck(chanceDenominator) < chanceNumerator`
/// (`CommonDrop.cs:36`), so an ordinary drop *is* likelier for a lucky player. This project had no
/// luck state, so every rule rolled the flat denominator and the distinction had nowhere to live.
#[cfg(test)]
mod luck_scaling {
    use super::*;
    use crate::npc_drops::LuckScaling;

    /// No item may be registered for the same NPC in both the generated flat table and the
    /// hand-written conditional one, because `drop_loot` rolls both and every duplicate is a
    /// silent over-drop: two independent 1-in-10 rolls land 19 per cent of the time, not 10.
    ///
    /// This found four when it was written - the Twins' two trophies, and the Groom's and the
    /// Bride's Bloody Tear - which is the whole reason it is a test rather than a one-off script.
    /// The seam it guards is real and easy to cross: `npc_drops` takes every rule with no
    /// condition, and "no condition" is a property of the *registration*, not of the item, so a
    /// trophy or a boss drop can quietly qualify for both halves.
    #[test]
    fn no_item_is_registered_in_both_tables() {
        let at = Conditions::default();
        let mut dupes = Vec::new();
        for npc in 0u16..=700 {
            let flat: std::collections::BTreeSet<u16> = crate::npc_drops::drops(npc)
                .iter()
                .flat_map(|chain| chain.iter())
                .map(|d| d.item)
                .collect();
            for rule in conditional(npc, at) {
                if flat.contains(&rule.item) {
                    dupes.push((npc, rule.item));
                }
            }
        }
        assert!(dupes.is_empty(), "{dupes:?}");
    }

    /// `Common` is the default and the overwhelming majority, which is what makes luck worth
    /// having at all: the great bulk of the table scales.
    #[test]
    fn the_generated_table_is_mostly_luck_scaled() {
        let mut full = 0;
        let mut none = 0;
        let mut only_bad = 0;
        for npc in 0u16..=700 {
            for chain in crate::npc_drops::drops(npc) {
                for rule in *chain {
                    match rule.luck {
                        LuckScaling::Full => full += 1,
                        LuckScaling::None => none += 1,
                        LuckScaling::OnlyBad => only_bad += 1,
                    }
                }
            }
        }
        assert!(full > 700, "the bulk of the table scales: {full}");
        // Exactly three, and each identified: npc 48's Spiky Ball
        // (`ItemDropRule.NotScalingWithLuck(320, 2)`, `ItemDropDatabase.cs:1181`) and the two DD2
        // Ogres' Ogre Masks, which source spells as a bare `new CommonDropNotScalingWithLuck(3865,
        // ...)` (`:753`, `:759`) - the generator rewrites that form before parsing, and this is
        // what proves the rewrite still happens. Every *other* `NotScalingWithLuck` registration
        // in the database is wrapped in a `DropBasedOnExpertMode` or is a `OneFromOptions` pool,
        // so it belongs to the hand-written table and is checked in the tests below instead.
        assert_eq!(
            none, 3,
            "the flat table's three unscaled rules, or the generator stopped reading the \
             constructor name"
        );
        // The Groom and the Bride, the only `ScalingWithOnlyBadLuck` in the whole database
        // (`ItemDropDatabase.cs:179`), one rule each.
        assert_eq!(only_bad, 2);

        // The master table is an even split, and that is the shape rather than a coincidence:
        // every boss has one `MasterModeCommonDrop` relic (which scales) and one
        // `MasterModeDropOnAllPlayers` pet or mount (which does not).
        let mut master_full = 0;
        let mut master_none = 0;
        for npc in 0u16..=700 {
            for rule in crate::npc_drops::master_drops(npc) {
                match rule.luck {
                    LuckScaling::Full => master_full += 1,
                    LuckScaling::None => master_none += 1,
                    LuckScaling::OnlyBad => panic!("no master rule uses only-bad luck"),
                }
            }
        }
        assert_eq!(
            (master_full, master_none),
            (32, 32),
            "one relic and one pet per boss"
        );
    }

    /// Spot-checks against source, one per variant, so a generator change that flattened the
    /// distinction would fail here rather than silently.
    #[test]
    fn the_variants_match_the_constructors_source_uses() {
        let rule_for = |npc: u16, item: u16| {
            crate::npc_drops::drops(npc)
                .iter()
                .flat_map(|chain| chain.iter())
                .find(|d| d.item == item)
                .copied()
        };
        // `RegisterToNPC(48, ItemDropRule.NotScalingWithLuck(320, 2))`
        // (`ItemDropDatabase.cs:1181`) — the Spike Ball's Spiky Ball.
        assert_eq!(
            rule_for(48, 320).map(|d| d.luck),
            Some(LuckScaling::None),
            "npc 48's item 320 is registered `NotScalingWithLuck`"
        );
        // `RegisterToMultipleNPCs(ItemDropRule.ScalingWithOnlyBadLuck(4271, 5), 53, 536)`
        // (`ItemDropDatabase.cs:179`) — the Groom and the Bride.
        for npc in [53u16, 536] {
            assert_eq!(
                rule_for(npc, 4271).map(|d| d.luck),
                Some(LuckScaling::OnlyBad),
                "npc {npc}'s item 4271 is `ScalingWithOnlyBadLuck`"
            );
        }
        // `RegisterToNPC(type, ItemDropRule.Common(3549))` (`ItemDropDatabase.cs:591`) — the
        // Cultist's Ancient Manipulator, an ordinary drop.
        assert_eq!(rule_for(439, 3549).map(|d| d.luck), Some(LuckScaling::Full));
    }

    /// The Old One's Army is the one event whose whole loot table is `NotScalingWithLuck`: a lucky
    /// player gets exactly the odds everybody else does. Both halves of it are marked, the pools
    /// in `chance_pools` and the flat items in `by_mode`.
    #[test]
    fn nothing_the_old_ones_army_drops_scales_with_luck() {
        for at in [
            Conditions::default(),
            Conditions {
                expert: true,
                ..Conditions::default()
            },
        ] {
            for npc in [564u16, 565, 576, 577] {
                let pools = chance_pools(npc, at);
                assert!(!pools.is_empty(), "npc {npc} must have pools to check");
                for pool in pools {
                    assert_eq!(
                        pool.scaling,
                        LuckScaling::None,
                        "npc {npc}'s pool {:?} is `OneFromOptionsNotScalingWithLuck`",
                        pool.options
                    );
                }
                // 3814 and 3815 are the Defender Medals and the Etherian Mana, on every one of them.
                for rule in conditional(npc, at) {
                    if matches!(rule.item, 3814 | 3815 | 3856 | 3865) {
                        assert_eq!(
                            rule.scaling,
                            LuckScaling::None,
                            "npc {npc}'s item {} is `NotScalingWithLuck`",
                            rule.item
                        );
                    }
                }
            }
        }
    }

    /// A chain's links can be different classes, and King Slime's are: the Slime Hook is
    /// `NotScalingWithLuck` and the Slime Gun that follows it is a plain `Common`
    /// (`ItemDropDatabase.cs:404`). Getting this wrong in either direction would be invisible in
    /// play and wrong in the numbers.
    #[test]
    fn one_chain_can_mix_scaled_and_unscaled_links() {
        let chains = conditional_chains(50, Conditions::default());
        let links = &chains[0].links;
        assert_eq!(links[0].item, 2585);
        assert_eq!(links[0].scaling, LuckScaling::None);
        assert_eq!(links[1].item, 2610);
        assert_eq!(links[1].scaling, LuckScaling::Full);
    }

    /// Queen Slime's five drops are four `Common` and one `NotScalingWithLuck`
    /// (`ItemDropDatabase.cs:313-317`), which is the finest-grained case in the table.
    #[test]
    fn queen_slimes_volatile_gelatin_is_the_one_that_does_not_scale() {
        let at = Conditions::default();
        let rules = conditional(657, at);
        let scaling_of = |item: u16| rules.iter().find(|d| d.item == item).map(|d| d.scaling);
        assert_eq!(scaling_of(4986), Some(LuckScaling::Full));
        assert_eq!(scaling_of(4959), Some(LuckScaling::Full));
        assert_eq!(scaling_of(4758), Some(LuckScaling::Full));
        assert_eq!(scaling_of(4981), Some(LuckScaling::Full));
        assert_eq!(
            scaling_of(4980),
            Some(LuckScaling::None),
            "the Volatile Gelatin"
        );
    }

    /// A master relic scales and a master pet does not, because they are built by two different
    /// factories (`ItemDropRule.cs:25-33`). Both come through `master_drops`, so the generated
    /// value has to survive being copied into a `Conditional`.
    #[test]
    fn a_master_relic_scales_where_a_master_pet_does_not() {
        let master = Conditions {
            master: true,
            ..Conditions::default()
        };
        // King Slime: relic 4929 (`MasterModeCommonDrop`) and pet 4797
        // (`MasterModeDropOnAllPlayers`).
        let rules = conditional(50, master);
        let scaling_of = |item: u16| rules.iter().find(|d| d.item == item).map(|d| d.scaling);
        assert_eq!(scaling_of(4929), Some(LuckScaling::Full), "the relic");
        assert_eq!(scaling_of(4797), Some(LuckScaling::None), "the pet");
    }
}
