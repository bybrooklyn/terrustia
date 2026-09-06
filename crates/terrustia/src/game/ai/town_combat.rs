//! Town NPCs fighting back.
//!
//! Vanilla drives every combat-capable town NPC through one function, `AI_007_TownEntities`
//! (`NPC.cs:53515-56130`), keyed by `NPCID.Sets.AttackType[type]` (`NPCID.cs:4855`): 0 and 1 and 2
//! are ranged (a projectile aimed at the target, states 10/12/14 respectively), 3 is melee (a
//! hitbox swung against the target via `StrikeNPCNoInteraction`, state 15). All 28 town NPCs
//! vanilla gives an `AttackType` are transcribed here — the first four (Merchant/Arms
//! Dealer/Wizard/Dye Trader) proved the mechanism end to end earlier this session; this pass is
//! the rest.
//!
//! **This is a reimplementation of each class's core behaviour, not a line-by-line port** — the
//! same standing distinction `npc_ai.rs`'s module doc draws for every other AI style. Specifically
//! not modelled, for every entry: the vertical aim-tolerance check the ranged classes use to
//! decide whether to even attempt a shot, and the hardmode upgrades listed below. Neither changes
//! *whether* a town NPC fights back or *what it hits with*.
//!
//! Two things this doc used to name as unmodelled are modelled, and it went on saying otherwise
//! long after they landed. Recorded because a stale disclosure is worse than none: a reader trusts
//! it and stops looking.
//! - **The multi-frame windup is real.** A shot leaves on its own `localAI[3]` mark rather than on
//!   the tick the decision is made (`NPC.cs:55325`), which is the telegraph; [`TownCombat::shots`]
//!   is those marks, and the four burst ladders are transcribed in full.
//! - **The attack gate is vanilla's own geometric roll**, `Main.rand.Next(AttackAverageChance) == 0`
//!   per tick (`NPC.cs:56012`), not the flat `AttackTime + AttackAverageChance` cooldown this doc
//!   described. See [`TownCombat::average_chance`], whose own doc already said this doc was wrong.
//!
//! A handful of NPCs needed a further, per-entry simplification, each called out at its own entry
//! below rather than silently folded into the general disclaimer:
//! - **Hardmode upgrades are never modelled** (Guide's Fire Arrow and +6 damage, the
//!   Steampunker's/Travelling Merchant's/Painter's/Pirate's damage or projectile changes,
//!   Princess's higher damage): every entry here uses vanilla's classic-mode values. The Arms
//!   Dealer's burst is the one exception, because it is the one ladder vanilla really does put
//!   behind `if (Main.hardMode)`; see [`hardmode_shots`].
//! - **Pirate's close-range special attack is not modelled** (`NPC.cs:55280-55287`, the
//!   `PrettySafe`-range branch that swaps in projectile 162 at 50 damage). Its six-shot burst is.
//! - **Cyborg picks one of three random projectiles per shot in vanilla** (`Utils.SelectRandom`
//!   among rocket/grenade/proximity-mine launchers); this always fires the rocket launcher variant
//!   (case `135`) rather than modelling the roll.
//! - **Truffle and Princess do not throw their projectile from themselves**: vanilla spawns it at
//!   a position near the target instead (a mushroom sprouting, a heart bursting), with no launch
//!   velocity at all. This doc used to say that was "modelled here as an ordinary aimed shot",
//!   because adding a shape for two NPCs "was judged not worth the complexity" while "both still
//!   deal real damage on a real cooldown, which is what matters".
//!
//!   Both halves of that stopped being true. The damage was never delivered - nothing in this
//!   server tested a town NPC's projectile against a hitbox until `Damage_PVE` was transcribed -
//!   and the approximation only held while neither projectile had an arm: `aiStyle 112`'s spore
//!   overwrites its velocity every tick with a vertical bob, so "thrown from the Truffle" and
//!   "sits on the Truffle" are the same thing. [`AttackKind::AtTarget`] is the shape, and the one
//!   part of it that is still an approximation - the size of the scatter box - says so at its own
//!   field rather than here.
//! - **Dryad's ranged attack does zero pre-scaling damage in vanilla** (`NPC.cs`'s `type == 20`
//!   branch never sets the damage local the way every other branch does, leaving it at its
//!   declared-zero default) — transcribed faithfully rather than "corrected," the same standing
//!   rule this session's other genuinely-dead-vanilla-branch transcriptions already follow. Her
//!   `AttackTime[20]` is also a real outlier at 600 (vs. 15-90 for everyone else), so she attacks
//!   far less often than the rest even before that.
//!
//!   What this doc used to conclude from that was wrong, and is the reason it is worth writing
//!   down: it read the zero damage as "a rare, harmless shot" that "reads as intentional in
//!   vanilla too (a nature spirit's projectile is closer to a visual effect than a weapon)". The
//!   damage is zero because *nothing about this attack is a collision*. Projectile 586 is the
//!   Dryad's Ward, and its whole behaviour is `AI_111_DryadsWard`
//!   (`Projectile.cs:41872-41978`): a circle that grows around her, blesses every town NPC inside
//!   it and puts Dryad's Bane on every hostile, every ten ticks for 570 ticks. It is her single
//!   most useful contribution to defending a town, and this module had it filed as decoration.
//!   The arm lives in `server::systems::tick_dryad_wards`, because it acts on the NPCs around it
//!   rather than on anything it touches.
//! - **Tax Collector's "Andrew" easter egg** (`GivenName == "Andrew"`, a doubled Tax Collector) is
//!   deliberately not carried over, the same call already made for Dye Trader's own easter egg —
//!   cosmetic flavour tied to a specific name, not a gameplay gap.

/// What one town NPC type's attack looks like.
#[derive(Debug, Clone, Copy)]
pub struct TownCombat {
    /// Vanilla's own `ai[0]` value for this attack class (`NPC.cs`'s state 10/12/14/15) — kept so
    /// a real client's own animation prediction, which reads this field for any NPC, has a state
    /// it recognises rather than a number invented for this port.
    pub state: f32,
    pub kind: AttackKind,
    /// How far a hostile has to be before this NPC notices it, from `NPCID.Sets.DangerDetectRange`.
    pub range: f32,
    /// `NPCID.Sets.AttackAverageChance[type]`: the denominator of vanilla's own per-tick gate on
    /// starting an attack, `Main.rand.Next(AttackAverageChance[type]) == 0` (`NPC.cs:56012`).
    ///
    /// A real geometric roll rather than the flat cooldown this used to carry. The module doc said
    /// this project had "no equivalent scheduling primitive" for it, which was never true: the
    /// routine is handed an rng. The flat number was also wrong for the Dye Trader, whose gate is
    /// `1` - it swings on every tick it can - and was modelled at a nine-tick gap.
    pub average_chance: i32,
    /// Which frames *within* the attack state a shot leaves on, in order — vanilla's `localAI[3]`
    /// marks (`NPC.cs:55049`, `:55372`, and the state-14 sibling), read straight off source.
    ///
    /// A shot does not leave on the tick the decision is made. The NPC enters the state, slows to
    /// a stop (`velocity.X *= 0.8f`), and the projectile leaves `windup` frames later; that gap is
    /// the whole telegraph, and it was the largest of the module's standing narrowings.
    ///
    /// Several types fire *more than once* per state, through a cascade of
    /// `if (localAI[3] > numNN) { numNN = <next>; }` steps. Because `localAI[3]` is read before its
    /// own increment and the shot fires on `localAI[3] == numNN` after it, each rung is one shot:
    /// the marks are exactly the assigned values, in order. The Pirate's six is the longest, and
    /// its absence was called out by name in the module doc.
    ///
    /// Melee has none: vanilla's state 15 has no `localAI[3]` gate at all and swings against
    /// whatever is in the box every tick of the state.
    pub shots: &'static [i32],
    /// `NPCID.Sets.AttackTime[type]`: how long the attack state itself lasts, which is the real
    /// floor on how often this NPC can attack. It cannot re-roll until the state ends.
    pub attack_time: i32,
}

/// The two town NPCs whose shot is given an explicit lifetime, and how long.
///
/// Zero means the projectile's own table value, which is what `NewProjectile` leaves every other
/// town shot with. These two are overridden a line after they are made
/// (`NPC.cs:55070-55077`) and they are the *only* two in the whole of
/// `AI_007_TownEntities`, checked by sweeping the function for `timeLeft`.
///
/// It matters more than a flourish. The Goblin Tinkerer's spiky ball declares 4,800 ticks and the
/// Golfer's ball 3,600, so without this a defending town leaves balls lying around for over a
/// minute each rather than eight seconds - which is a pile of live entities, and the Golfer's is
/// already the one projectile in this roster whose sheer count has shown up in a test.
pub fn shot_lifetime(npc_type: u16) -> u16 {
    match npc_type {
        // Goblin Tinkerer (`NPC.cs:55074-55077`) and Golfer (`:55070-55073`).
        107 | 588 => 480,
        _ => 0,
    }
}

/// The hardmode-only rungs. There is exactly one ladder in the game behind `if (Main.hardMode)`,
/// and it is the Arms Dealer's (`NPC.cs:55129-55147`).
///
/// This used to carry a second entry, and the count in the doc beside it read three, two and two in
/// three different places. All four ladders in `AI_007_TownEntities` were then read one block at a
/// time: the Arms Dealer's at `:55129-55147` is behind hardmode; the **Painter's** at
/// `:55159-55168`, the Steampunker's at `:55233-55242` and the Pirate's at `:55253-55279` are not.
/// The Painter's was here, which cost a classic-mode Painter two thirds of its defence. The
/// remaining two were already unconditional and stayed right.
///
/// The type ids are the trap: 227 is the Painter and 22 is the Guide, and this function's own
/// comment named them as the Cyborg (209) and cited `:55190`, which is the Guide's block.
pub fn hardmode_shots(npc_type: u16) -> Option<&'static [i32]> {
    Some(match npc_type {
        19 => &[1, 10, 20, 30],
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy)]
pub enum AttackKind {
    Ranged {
        projectile: u16,
        /// Vanilla's own pre-scaling damage — run it through [`town_npc_damage`] before use.
        damage: i32,
        speed: f32,
        knockback: f32,
    },
    /// A projectile that *appears at the target* rather than being thrown at it.
    ///
    /// Vanilla has exactly two, both in state 14 (`NPC.cs:55499-55529`): the Truffle's spore and
    /// the Princess's weapon. Neither is aimed and neither is given a speed - a mushroom sprouts
    /// where the enemy is standing and a heart bursts on it - so both roll a point in a box around
    /// the target, re-roll while that point is inside a solid tile, and spawn there at rest.
    ///
    /// This module used to model both as ordinary aimed shots at an invented six pixels a tick,
    /// disclosed in the module doc as not worth a shape of its own for two NPCs. That judgement
    /// was made when neither projectile had an arm and both therefore behaved like any other
    /// straight-line shot. It does not survive the arms: `aiStyle 112`'s spore overwrites its
    /// velocity every tick with a vertical bob and so never travels at all, which turns "thrown
    /// from the Truffle" into "sits on the Truffle", and `aiStyle 186`'s weapon lives sixty ticks.
    /// A spore that has to sprout on the enemy cannot be approximated by throwing it.
    AtTarget {
        projectile: u16,
        damage: i32,
        knockback: f32,
        /// Half the box the spawn point is rolled in, per axis, in pixels.
        ///
        /// **This is the one number here that is not vanilla's own.** Vanilla scales the box by
        /// the *target's* size - the Truffle's spans five of them and the Princess's exactly one -
        /// and [`crate::game::npc_ai::Target`] carries a centre and no size. Threading one through
        /// would touch a hundred and fifty literals for a scatter radius, so the box is evaluated
        /// once here against an 18x40 hitbox, which is the Zombie's and close to the median of the
        /// hostile roster. Everything else about the shape - at the target, at rest, re-rolled out
        /// of walls - is transcribed.
        scatter: (f32, f32),
        /// `while (num74 > 0 && WorldGen.SolidTile(...))`: how many times the point is re-rolled
        /// when it lands in a wall. Ten for the Truffle, five for the Princess.
        tries: u32,
    },
    Melee {
        damage: i32,
        knockback: f32,
        /// Half-width and half-height of the swing's hitbox, centred on the NPC.
        reach: (f32, f32),
    },
}

/// Every vanilla town NPC that has a real `AttackType`. `None` for everything else — town pets
/// (`NPCID.Sets.IsTownPet`, explicitly re-asserted as `-1` in vanilla's own `AttackType` set) and
/// anything without an `AttackType` entry at all — is correct, not a gap this function hides.
pub fn town_combat(npc_type: u16) -> Option<TownCombat> {
    Some(match npc_type {
        // ---- AttackType 0, state 10: ranged, aimed from the NPC toward the target's head ----
        // Demolitionist. NPC.cs state-10 block, `type == 38`: projectile 30, damage 20, speed 6,
        // knockback 7. AttackTime[38]=34, AttackAverageChance[38]=40, DangerDetectRange[38]=300.
        38 => TownCombat {
            state: 10.0,
            kind: AttackKind::Ranged {
                projectile: 30,
                damage: 20,
                speed: 6.0,
                knockback: 7.0,
            },
            range: 300.0,
            average_chance: 40,
            shots: &[10],
            attack_time: 34,
        },
        // Bestiary Girl. NPC.cs state-10 block, `type == 633`: projectile 880, damage 15, speed 24,
        // knockback 7 (the "lycantrope" full-moon variant, projectile 929 with 1.5x damage, is not
        // modelled — a calendar-gated cosmetic swap, same call as every other secret-seed/date-gated
        // branch this project skips). AttackTime[633]=12, AttackAverageChance[633]=1,
        // DangerDetectRange[633]=100.
        633 => TownCombat {
            state: 10.0,
            kind: AttackKind::Ranged {
                projectile: 880,
                damage: 15,
                speed: 24.0,
                knockback: 7.0,
            },
            range: 100.0,
            average_chance: 1,
            shots: &[1],
            attack_time: 12,
        },
        // DD2 Bartender (Tavernkeep). NPC.cs state-10 block, `type == 550`: projectile 669, damage
        // 24, speed 6, knockback 9. AttackTime[550]=34, AttackAverageChance[550]=40,
        // DangerDetectRange[550]=120.
        550 => TownCombat {
            state: 10.0,
            kind: AttackKind::Ranged {
                projectile: 669,
                damage: 24,
                speed: 6.0,
                knockback: 9.0,
            },
            range: 120.0,
            average_chance: 40,
            shots: &[10],
            attack_time: 34,
        },
        // Golfer. NPC.cs state-10 block, `type == 588`: projectile 721, damage 15, speed 8,
        // knockback 9. AttackTime[588]=20, AttackAverageChance[588]=20,
        // DangerDetectRange[588]=120.
        588 => TownCombat {
            state: 10.0,
            kind: AttackKind::Ranged {
                projectile: 721,
                damage: 15,
                speed: 8.0,
                knockback: 9.0,
            },
            range: 120.0,
            average_chance: 20,
            shots: &[5],
            attack_time: 20,
        },
        // Party Girl. NPC.cs state-10 block, `type == 208`: projectile 588, damage 30, speed 6,
        // knockback 6. AttackTime[208]=34, AttackAverageChance[208]=50,
        // DangerDetectRange[208]=400.
        208 => TownCombat {
            state: 10.0,
            kind: AttackKind::Ranged {
                projectile: 588,
                damage: 30,
                speed: 6.0,
                knockback: 6.0,
            },
            range: 400.0,
            average_chance: 50,
            shots: &[10],
            attack_time: 34,
        },
        // Merchant. NPC.cs:54969-54977: projectile 48, speed 9, damage 12, knockback 1.5.
        // AttackTime[17]=34, AttackAverageChance[17]=30, DangerDetectRange[17]=320
        // (`NPCID.cs:4851,4853,4841`). The `AttackTime` here read 40 and the cooldown 70; the
        // table's own pair for 17 is 34, so the Merchant was about nine percent slow.
        17 => TownCombat {
            state: 10.0,
            kind: AttackKind::Ranged {
                projectile: 48,
                damage: 12,
                speed: 9.0,
                knockback: 1.5,
            },
            range: 320.0,
            average_chance: 30,
            shots: &[10],
            attack_time: 34,
        },
        // Angler. NPC.cs state-10 block, `type == 369`: projectile 520, damage 10, speed 12,
        // knockback 3. AttackTime[369]=34, AttackAverageChance[369]=50,
        // DangerDetectRange[369]=300.
        369 => TownCombat {
            state: 10.0,
            kind: AttackKind::Ranged {
                projectile: 520,
                damage: 10,
                speed: 12.0,
                knockback: 3.0,
            },
            range: 300.0,
            average_chance: 50,
            shots: &[10],
            attack_time: 34,
        },
        // Skeleton Merchant. NPC.cs state-10 block, `type == 453`: projectile 21, damage 14, speed
        // 14, knockback 3. AttackTime[453]=34, AttackAverageChance[453]=30,
        // DangerDetectRange[453]=300.
        453 => TownCombat {
            state: 10.0,
            kind: AttackKind::Ranged {
                projectile: 21,
                damage: 14,
                speed: 14.0,
                knockback: 3.0,
            },
            range: 300.0,
            average_chance: 30,
            shots: &[10],
            attack_time: 34,
        },
        // Goblin Tinkerer. NPC.cs state-10 block, `type == 107`: projectile 24, damage 15, speed 5,
        // knockback 1. AttackTime[107]=60, AttackAverageChance[107]=60,
        // DangerDetectRange[107]=300.
        107 => TownCombat {
            state: 10.0,
            kind: AttackKind::Ranged {
                projectile: 24,
                damage: 15,
                speed: 5.0,
                knockback: 1.0,
            },
            range: 300.0,
            average_chance: 60,
            shots: &[10],
            attack_time: 60,
        },
        // Mechanic. NPC.cs state-10 block, `type == 124`: projectile 582, damage 11, speed 10,
        // knockback 3.5. AttackTime[124]=34, AttackAverageChance[124]=30,
        // DangerDetectRange[124]=800.
        124 => TownCombat {
            state: 10.0,
            kind: AttackKind::Ranged {
                projectile: 582,
                damage: 11,
                speed: 10.0,
                knockback: 3.5,
            },
            range: 800.0,
            average_chance: 30,
            shots: &[1],
            attack_time: 34,
        },
        // Nurse. NPC.cs state-10 block, `type == 18`: projectile 583, damage 8, speed 8, knockback
        // 2. AttackTime[18]=34, AttackAverageChance[18]=60, DangerDetectRange[18]=300.
        18 => TownCombat {
            state: 10.0,
            kind: AttackKind::Ranged {
                projectile: 583,
                damage: 8,
                speed: 8.0,
                knockback: 2.0,
            },
            range: 300.0,
            average_chance: 60,
            shots: &[1],
            attack_time: 34,
        },
        // Santa Claus. NPC.cs state-10 block, `type == 142`: projectile 589, damage 22, speed 7,
        // knockback 2. AttackTime[142]=34, AttackAverageChance[142]=50,
        // DangerDetectRange[142]=500.
        142 => TownCombat {
            state: 10.0,
            kind: AttackKind::Ranged {
                projectile: 589,
                damage: 22,
                speed: 7.0,
                knockback: 2.0,
            },
            range: 500.0,
            average_chance: 50,
            shots: &[1],
            attack_time: 34,
        },

        // ---- AttackType 1, state 12: ranged, aimed at the target's centre ----
        // Arms Dealer. NPC.cs:55114-55123, non-hardmode: projectile 14, speed 13, damage 24,
        // knockback 3. AttackTime[19]=40, AttackAverageChance[19]=30, DangerDetectRange[19]=900.
        19 => TownCombat {
            state: 12.0,
            kind: AttackKind::Ranged {
                projectile: 14,
                damage: 24,
                speed: 13.0,
                knockback: 3.0,
            },
            range: 900.0,
            average_chance: 30,
            shots: &[1],
            attack_time: 40,
        },
        // Painter. NPC.cs state-12 block, `type == 227`, non-hardmode: projectile 587, damage 8,
        // speed 10, knockback 1.75. AttackTime[227]=60, AttackAverageChance[227]=30,
        // DangerDetectRange[227]=800.
        //
        // Three shots per state, and *not* behind hardmode: the ladder is at `NPC.cs:55159-55168`
        // and the `if (Main.hardMode)` two lines under it (`:55169-55172`) only adds two damage.
        // This sat in `hardmode_shots` instead, so a Painter defending a town before the
        // mechanical bosses - which is most of a Painter's life, since one moves in at eight
        // townspeople - fired once where the game fires three times.
        227 => TownCombat {
            state: 12.0,
            kind: AttackKind::Ranged {
                projectile: 587,
                damage: 8,
                speed: 10.0,
                knockback: 1.75,
            },
            range: 800.0,
            average_chance: 30,
            shots: &[1, 12, 24],
            attack_time: 60,
        },
        // Travelling Merchant. NPC.cs state-12 block, `type == 368`, non-hardmode: projectile 14,
        // damage 24, speed 13, knockback 2. AttackTime[368]=60, AttackAverageChance[368]=40,
        // DangerDetectRange[368]=900.
        368 => TownCombat {
            state: 12.0,
            kind: AttackKind::Ranged {
                projectile: 14,
                damage: 24,
                speed: 13.0,
                knockback: 2.0,
            },
            range: 900.0,
            average_chance: 40,
            shots: &[1],
            attack_time: 60,
        },
        // Guide. NPC.cs state-12 block, `type == 22`, non-hardmode: projectile 1, damage 12, speed
        // 10, knockback 2.75. AttackTime[22]=30, AttackAverageChance[22]=30,
        // DangerDetectRange[22]=700.
        22 => TownCombat {
            state: 12.0,
            kind: AttackKind::Ranged {
                projectile: 1,
                damage: 12,
                speed: 10.0,
                knockback: 2.75,
            },
            range: 700.0,
            average_chance: 30,
            shots: &[1],
            attack_time: 30,
        },
        // Witch Doctor. NPC.cs state-12 block, `type == 228`: projectile 267, damage 20, speed 14,
        // knockback 3. AttackTime[228]=40, AttackAverageChance[228]=50,
        // DangerDetectRange[228]=800.
        228 => TownCombat {
            state: 12.0,
            kind: AttackKind::Ranged {
                projectile: 267,
                damage: 20,
                speed: 14.0,
                knockback: 3.0,
            },
            range: 800.0,
            average_chance: 50,
            shots: &[1],
            attack_time: 40,
        },
        // Steampunker. NPC.cs state-12 block, `type == 178`, non-hardmode: projectile 242, damage
        // 11, speed 13, knockback 2. AttackTime[178]=24, AttackAverageChance[178]=50,
        // DangerDetectRange[178]=900.
        178 => TownCombat {
            state: 12.0,
            kind: AttackKind::Ranged {
                projectile: 242,
                damage: 11,
                speed: 13.0,
                knockback: 2.0,
            },
            range: 900.0,
            average_chance: 50,
            shots: &[1, 8, 16],
            attack_time: 24,
        },
        // Pirate. NPC.cs state-12 block, `type == 229`: projectile 14, damage 24, speed 14,
        // knockback 2. AttackTime[229]=60, AttackAverageChance[229]=40,
        // DangerDetectRange[229]=1000. The six-shot ladder is `NPC.cs:55253-55279`; the
        // close-range special at `:55280-55287` is the part still not modelled.
        229 => TownCombat {
            state: 12.0,
            kind: AttackKind::Ranged {
                projectile: 14,
                damage: 24,
                speed: 14.0,
                knockback: 2.0,
            },
            range: 1000.0,
            average_chance: 40,
            shots: &[1, 16, 24, 32, 40, 48],
            attack_time: 60,
        },
        // Cyborg. NPC.cs state-12 block, `type == 209`, `case 135` only (see module doc — vanilla
        // picks one of three projectiles per shot): projectile 135, damage 30, speed 12, knockback
        // 7. AttackTime[209]=60, AttackAverageChance[209]=30, DangerDetectRange[209]=1000.
        209 => TownCombat {
            state: 12.0,
            kind: AttackKind::Ranged {
                projectile: 135,
                damage: 30,
                speed: 12.0,
                knockback: 7.0,
            },
            range: 1000.0,
            average_chance: 30,
            shots: &[1],
            attack_time: 60,
        },

        // ---- AttackType 2, state 14: ranged, aimed with a slight downward lead ----
        // Clothier. NPC.cs state-14 block, `type == 54`: projectile 585, damage 16, speed 10,
        // knockback 2. AttackTime[54]=60, AttackAverageChance[54]=30,
        // DangerDetectRange[54]=700.
        54 => TownCombat {
            state: 14.0,
            kind: AttackKind::Ranged {
                projectile: 585,
                damage: 16,
                speed: 10.0,
                knockback: 2.0,
            },
            range: 700.0,
            average_chance: 30,
            shots: &[30],
            attack_time: 60,
        },
        // Wizard. NPC.cs:55428-55438: projectile 15, speed 6, damage 18, knockback 3.
        // AttackTime[108]=30, AttackAverageChance[108]=30, DangerDetectRange[108]=700
        // (`NPCID.cs:4851,4853,4841`). The `AttackTime` here read 60 and the cooldown 90, which is
        // the Clothier's pair one entry up rather than the Wizard's own: a Wizard defending a town
        // fired half as often as vanilla's.
        108 => TownCombat {
            state: 14.0,
            kind: AttackKind::Ranged {
                projectile: 15,
                damage: 18,
                speed: 6.0,
                knockback: 3.0,
            },
            range: 700.0,
            average_chance: 30,
            shots: &[15],
            attack_time: 30,
        },
        // Truffle. `NPC.cs:55439-55450`, `type == 160`: projectile 590, damage 40, knockback 3,
        // and no launch speed at all. AttackTime[160]=60, AttackAverageChance[160]=60,
        // DangerDetectRange[160]=700.
        //
        // The spore sprouts on the enemy: `vector4 = npc.position - npc.Size * 2f + npc.Size *
        // RandomVector2(rand, 0f, 1f) * 5f` (`:55503`), re-rolled up to ten times out of a solid
        // tile, then `NewProjectile(vector4, 0f, 0f, ...)`. Five target-sizes across an 18x40
        // hitbox is the +/-45 by +/-100 below; see [`AttackKind::AtTarget`] for why the box is
        // fixed rather than read off the target.
        160 => TownCombat {
            state: 14.0,
            kind: AttackKind::AtTarget {
                projectile: 590,
                damage: 40,
                knockback: 3.0,
                scatter: (45.0, 100.0),
                tries: 10,
            },
            range: 700.0,
            average_chance: 60,
            shots: &[15],
            attack_time: 60,
        },
        // Princess. `NPC.cs:55451-55461`, `type == 663`, non-hardmode: projectile 950, damage 15,
        // knockback 3, no launch speed. AttackTime[663]=60, AttackAverageChance[663]=1,
        // DangerDetectRange[663]=700.
        //
        // Her weapon bursts inside the enemy's own box rather than around it: `vector5 =
        // npc.position + npc.Size * RandomVector2(rand, 0f, 1f) * 1f` (`:55519`), re-rolled up to
        // five times, which against an 18x40 hitbox is the +/-9 by +/-20 below.
        663 => TownCombat {
            state: 14.0,
            kind: AttackKind::AtTarget {
                projectile: 950,
                damage: 15,
                knockback: 3.0,
                scatter: (9.0, 20.0),
                tries: 5,
            },
            range: 700.0,
            average_chance: 1,
            shots: &[15],
            attack_time: 60,
        },
        // Dryad. `NPC.cs:55463-55469`, `type == 20`: a real vanilla zero-damage attack, see module
        // doc: projectile 586, damage 0, knockback 3, and **no speed at all**.
        // AttackTime[20]=600, AttackAverageChance[20]=60, DangerDetectRange[20]=1200.
        //
        // The zero is transcribed, not a placeholder. State 14's speed local (`num63`) is declared
        // at `:55394` and only two of its five branches ever assign it - the Clothier's 10 and the
        // Wizard's 6 - so the Dryad's aim vector is multiplied by zero at `:55486` and the ward
        // hangs exactly where it was cast. This entry read 6 with a comment calling it an
        // approximation, which put the circle 3,420 pixels from the town by the time it expired.
        20 => TownCombat {
            state: 14.0,
            kind: AttackKind::Ranged {
                projectile: 586,
                damage: 0,
                speed: 0.0,
                knockback: 3.0,
            },
            range: 1200.0,
            average_chance: 60,
            shots: &[24],
            attack_time: 600,
        },

        // ---- AttackType 3, state 15: melee, a hitbox swung at anything it overlaps ----
        // Dye Trader. NPC.cs:55611-55617: damage 11, knockback 4.25, hitbox 32x32. The "Andrew"
        // easter egg (NPC.cs:55618, a doubled Tax Collector) is deliberately not carried over —
        // it is cosmetic flavour tied to a specific `GivenName`, not a gameplay gap.
        207 => TownCombat {
            state: 15.0,
            kind: AttackKind::Melee {
                damage: 11,
                knockback: 4.25,
                reach: (32.0, 32.0),
            },
            range: 60.0,
            average_chance: 1,
            // Melee has no `localAI[3]` gate at all: state 15 swings against whatever is
            // in the box every tick it runs (`NPC.cs:55632-55676`).
            shots: &[],
            attack_time: 15,
        },
        // Tax Collector. NPC.cs state-15 block, `type == 441`: damage 9, knockback 3.5, hitbox
        // 28x28 (the "Andrew" easter egg, see module doc, not carried over). AttackTime[441]=15,
        // AttackAverageChance[441]=1, DangerDetectRange[441]=50.
        441 => TownCombat {
            state: 15.0,
            kind: AttackKind::Melee {
                damage: 9,
                knockback: 3.5,
                reach: (28.0, 28.0),
            },
            range: 50.0,
            average_chance: 1,
            // Melee has no `localAI[3]` gate at all: state 15 swings against whatever is
            // in the box every tick it runs (`NPC.cs:55632-55676`).
            shots: &[],
            attack_time: 15,
        },
        // Stylist. NPC.cs state-15 block, `type == 353`: damage 10, knockback 5, hitbox 32x32.
        // AttackTime[353]=12, AttackAverageChance[353]=1, DangerDetectRange[353]=60. The block
        // also sets a second local (15) alongside the damage one (10) — that second number feeds
        // only `ai[1] = <that local> + rand(maxValue4)`, the attack-cooldown roll, never
        // `StrikeNPCNoInteraction`'s damage argument. An earlier pass here read that cooldown
        // local as the damage instead, off by 5.
        353 => TownCombat {
            state: 15.0,
            kind: AttackKind::Melee {
                damage: 10,
                knockback: 5.0,
                reach: (32.0, 32.0),
            },
            range: 60.0,
            average_chance: 1,
            // Melee has no `localAI[3]` gate at all: state 15 swings against whatever is
            // in the box every tick it runs (`NPC.cs:55632-55676`).
            shots: &[],
            attack_time: 12,
        },
        _ => return None,
    })
}

/// `NPC.GetAttackDamage_ForTownNPC`, transcribed: `GameDifficultyData.TownNPCDamageMultiplier` is
/// 1.0 in classic and 1.5 in expert-or-better. Journey's 2.0 and master's separate 2.0 are not
/// modelled — this project has no Journey-mode power state yet (`README.md`), and `Conditions`
/// carries only `expert`, not a distinct master flag.
pub fn town_npc_damage(base: i32, expert: bool) -> i32 {
    let multiplier = if expert { 1.5 } else { 1.0 };
    ((base as f32) * multiplier) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every real vanilla `AttackType` NPC: 28 total, one per `(npc_type, is_melee)` pair from
    /// `NPCID.cs`'s own `AttackType` set (`Factory.CreateIntSet(-1, 38, 0, 17, 0, ...)`).
    const ALL_COMBAT_NPCS: [(u16, bool); 28] = [
        (38, false),
        (17, false),
        (107, false),
        (19, false),
        (22, false),
        (124, false),
        (228, false),
        (178, false),
        (18, false),
        (229, false),
        (209, false),
        (54, false),
        (108, false),
        (160, false),
        (20, false),
        (369, false),
        (453, false),
        (368, false),
        (207, true),
        (227, false),
        (208, false),
        (142, false),
        (441, true),
        (353, true),
        (633, false),
        (550, false),
        (588, false),
        (663, false),
    ];

    #[test]
    fn every_real_attack_type_npc_is_covered() {
        for (npc_type, expect_melee) in ALL_COMBAT_NPCS {
            let combat = town_combat(npc_type).unwrap_or_else(|| panic!("npc {npc_type}"));
            assert_eq!(
                matches!(combat.kind, AttackKind::Melee { .. }),
                expect_melee,
                "npc {npc_type}"
            );
            assert!(combat.range > 0.0, "npc {npc_type}");
            assert!(combat.average_chance > 0, "npc {npc_type}");
            assert!(combat.attack_time > 0, "npc {npc_type}");
            // Melee has no shot marks; every ranged type has at least one.
            assert_eq!(
                combat.shots.is_empty(),
                expect_melee,
                "npc {npc_type}'s shot marks"
            );
        }
    }

    #[test]
    fn no_two_covered_npcs_share_an_id() {
        let mut ids: Vec<u16> = ALL_COMBAT_NPCS.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        let mut deduped = ids.clone();
        deduped.dedup();
        assert_eq!(ids, deduped, "a duplicate id in the coverage table itself");
    }

    #[test]
    fn town_pets_still_do_not_fight() {
        // Town Cat/Dog/Bunny: vanilla's own `AttackType` set explicitly re-asserts `-1` for these
        // (`NPCID.Sets.IsTownPet`) rather than leaving them at the array's default — the same
        // "not a fighter" outcome, just spelled out in source rather than implied.
        for pet in [637u16, 638, 656] {
            assert!(town_combat(pet).is_none(), "npc {pet} is a town pet");
        }
        assert!(town_combat(1).is_none(), "a blue slime is not a town NPC");
    }

    #[test]
    fn expert_mode_scales_damage_by_one_and_a_half() {
        assert_eq!(town_npc_damage(12, false), 12);
        assert_eq!(town_npc_damage(12, true), 18);
    }

    #[test]
    fn the_stylists_melee_damage_is_ten_not_fifteen() {
        // NPC.cs's `type == 353` block sets two numbers close together — 10 for the damage that
        // actually reaches `StrikeNPCNoInteraction`, and a separate 15 used only for the
        // attack-cooldown roll. An earlier transcription used the cooldown's 15 as the damage.
        let combat = town_combat(353).unwrap();
        assert!(matches!(combat.kind, AttackKind::Melee { damage: 10, .. }));
    }

    /// BA3-03, fail-then-pass: every ranged cooldown is its own `AttackTime + AttackAverageChance`.
    ///
    /// The pairs below are transcribed from `NPCID.cs:4851` (`AttackTime`), `:4853`
    /// (`AttackAverageChance`) and `:4841` (`DangerDetectRange`) rather than restated from the
    /// entries they check, which is the whole point: the Merchant's cooldown was 70 (from an
    /// `AttackTime` of 40, where the table says 34) and the Wizard's was 90 (from 60, where the
    /// table says 30, and 60/90 is the Clothier's pair one entry above it in this file).
    ///
    /// Melee is deliberately not checked here. Vanilla's melee cadence is not this formula at all:
    /// state 15 holds `ai[1]` for `AttackTime` ticks and then rests for `num80 + rand(maxValue4)`
    /// (`NPC.cs:55581-55609,55641-55686`), so the three melee entries approximate something with a
    /// different shape and are covered by the module doc's disclosure instead.
    #[test]
    fn every_ranged_cooldown_is_its_own_attack_time_plus_average_chance() {
        // (type, AttackTime, AttackAverageChance, DangerDetectRange)
        const RANGED: [(u16, i32, i32, f32); 25] = [
            (38, 34, 40, 300.0),
            (17, 34, 30, 320.0),
            (107, 60, 60, 300.0),
            (19, 40, 30, 900.0),
            (22, 30, 30, 700.0),
            (124, 34, 30, 800.0),
            (228, 40, 50, 800.0),
            (178, 24, 50, 900.0),
            (18, 34, 60, 300.0),
            (229, 60, 40, 1000.0),
            (209, 60, 30, 1000.0),
            (54, 60, 30, 700.0),
            (108, 30, 30, 700.0),
            (160, 60, 60, 700.0),
            (20, 600, 60, 1200.0),
            (369, 34, 50, 300.0),
            (453, 34, 30, 300.0),
            (368, 60, 40, 900.0),
            (227, 60, 30, 800.0),
            (208, 34, 50, 400.0),
            (142, 34, 50, 500.0),
            (633, 12, 1, 100.0),
            (550, 34, 40, 120.0),
            (588, 20, 20, 120.0),
            (663, 60, 1, 700.0),
        ];
        for (npc_type, attack_time, average_chance, detect_range) in RANGED {
            let combat = town_combat(npc_type).unwrap_or_else(|| panic!("npc {npc_type}"));
            assert_eq!(
                combat.attack_time, attack_time,
                "npc {npc_type}'s AttackTime"
            );
            assert_eq!(
                combat.average_chance, average_chance,
                "npc {npc_type}'s AttackAverageChance"
            );
            assert_eq!(combat.range, detect_range, "npc {npc_type}'s detect range");
        }
    }

    #[test]
    fn dryads_attack_is_faithfully_harmless() {
        // Not a bug — see the module doc's own entry for why vanilla's `type == 20` branch never
        // sets a damage value.
        let combat = town_combat(20).unwrap();
        assert!(matches!(combat.kind, AttackKind::Ranged { damage: 0, .. }));
    }
}
