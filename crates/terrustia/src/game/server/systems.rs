//! What the world does on its own, once a tick.
//!
//! Everything [`GameServer::tick`] calls and the helpers those calls share: NPC buffs and AI, the
//! projectile and contact-damage passes, loot and coin drops, town residents and their housing,
//! tile entities, the wiring circuits and what they fire, liquids, growth and biome spread, the
//! weather and calendar events, the moons, invasions and the Old One's Army, and the spawn scan.
//! None of it is driven by a client: a packet handler in `dispatch.rs` may set something off, but
//! the work itself happens here, between two ticks, on the game task's own schedule.
//!
//! Like `dispatch.rs`, and for the same reason, this file takes the parent module's prelude with
//! `use super::*` rather than restating several dozen imports and private constants.

use super::*;

/// A desert worm head raised this tick and the chain it wants: its slot, where it is, its body and
/// tail types, and the inclusive segment range to roll in
/// (`npc_params::self_growing_sand_worm`).
type SandWormToGrow = (u8, (f32, f32), u16, u16, usize, usize);

/// Whether this NPC's life *is* another NPC's, which is vanilla's `realLife != -1`.
///
/// `realLife` names the NPC a segment reads its health from (`NPC.cs:7363-7366` assigns `statLife`
/// and `statLifeMax` straight off it), so it is set for worm segments and the Wall of Flesh mouth
/// and for nothing else. Soul Drain checks it (`soulDrain && realLife == -1`, `NPC.cs:92802`) so a
/// worm is drained once rather than once per link.
///
/// [`Npc::follows`] is this project's own worm link and matches that exactly. [`Npc::follows_boss`]
/// used to be ORed in with it and should not have been: a boss *part* steers itself and holds its
/// own life (`free_the_golem_head`'s own comment says the freed head reads its own health rather
/// than the body's), so vanilla drains one. Skeletron's hands and Golem's fists were silently immune
/// here, which is the sort of thing nobody reports as a bug because a debuff quietly doing nothing
/// looks exactly like a debuff that was never applied.
fn shares_a_life_pool(npc: &crate::game::npc::Npc) -> bool {
    npc.follows.is_some()
}

/// Whether a player or NPC's hitbox overlaps the pixel square one tile occupies — the entity half
/// of `Collision.EmptyTile(x, y, ignoreTiles: true)`, which skips the ordinary "is a block
/// already there" check entirely and tests only entities. `occupants` is
/// [`GameServer::entity_hitboxes`]'s own list, built once per call site rather than once per tile
/// tested.
pub(super) fn tile_occupied(x: i32, y: i32, occupants: &[(f32, f32, f32, f32)]) -> bool {
    let tile = (
        (x * 16) as f32,
        (y * 16) as f32,
        (x * 16 + 16) as f32,
        (y * 16 + 16) as f32,
    );
    occupants.iter().any(|&b| boxes_overlap(tile, b))
}

/// The facts about *this* dead NPC that its loot depends on, read off it before the table lets it
/// go. Every one of these is a per-instance condition in `ItemDropDatabase`, not a fact about the
/// type: two NPCs of the same type dying side by side can answer them differently.
#[derive(Debug, Clone, Copy, Default)]
struct DeadNpc {
    /// It came out of a statue, so a farm cannot grind the blood-moon-exclusive drops.
    from_statue: bool,
    /// It was the Clothier's repeatable vanity Skeletron rather than the ordinary boss.
    red_hat_skeletron: bool,
    /// This Empress's fight was begun in daylight, which is the Terraprisma's only gate.
    empress_genuinely_enraged: bool,
}

impl GameServer {
    /// Advance every NPC and tell clients about the ones that changed.
    /// Run every NPC's buffs: count the timers down, work out what they cost, and tell clients.
    ///
    /// Kept apart from [`Self::tick_npcs`] because a debuff can kill, and dying reaches far
    /// outside the NPC table — loot, coins, invasion counts, boss flags — none of which the AI
    /// loop can touch while it holds the table borrowed.
    ///
    /// The whole pass costs nothing when nothing is burning, which is the ordinary case: an NPC
    /// with no buffs is skipped before any of the per-tick work.
    pub(super) fn tick_npc_buffs(&mut self) {
        // Nothing anywhere is buffed: the common case, and worth one scan to establish.
        if !self.npcs.iter().any(|(_, n)| !n.buffs.is_empty()) {
            return;
        }

        let dryad = self.dryad_bane_rate();
        // Five debuffs are worth however many of a projectile are stuck in the target, so the
        // projectile table is counted once here rather than once per NPC per debuff.
        let stacks = self.stacked_debuff_projectiles();

        let mut changed: Vec<u8> = Vec::new();
        let mut hits: Vec<(u8, i16)> = Vec::new();
        let mut deaths: Vec<(u8, u16, (f32, f32), f32)> = Vec::new();

        for (index, npc) in self.npcs.iter_mut() {
            if npc.buffs.is_empty() {
                if std::mem::take(&mut npc.buffs_dirty) {
                    changed.push(index);
                }
                continue;
            }

            npc.buffs.set_flags(npc.npc_type, npc.ai[1]);
            if npc.buffs.clear_expired() {
                npc.buffs_dirty = true;
            }

            let count = |projectile: u16| {
                stacks
                    .get(&(index, projectile))
                    .copied()
                    .unwrap_or_default()
            };
            let around = crate::game::buffs::Around {
                npc_type: npc.npc_type,
                ai1: npc.ai[1],
                is_segment: shares_a_life_pool(npc),
                get_good: false,
                lava_wet: false,
                daybreaks: count(DAYBREAK_SPEAR),
                javelins: count(JAVELIN),
                tentacles: count(TENTACLE_SPIKE),
                blood_knives: count(BLOOD_BUTCHERER),
                cells: count(STARDUST_CELL),
                dryad_bane_dps: dryad,
            };

            let immortal = is_immortal(npc);
            let toll = npc.buffs.dots(&around, immortal, npc.invulnerable);
            if toll.healed > 0 && npc.life < npc.life_max {
                npc.life = (npc.life + toll.healed).min(npc.life_max);
                npc.dirty = true;
            }
            if toll.hurt > 0 && !immortal {
                // The game reports each crossing separately, so a heavy stack shows as several
                // numbers rather than one large one.
                let per_hit = toll.hurt / toll.hits.max(1);
                for _ in 0..toll.hits {
                    hits.push((index, i16::try_from(per_hit).unwrap_or(i16::MAX)));
                }
                npc.life -= toll.hurt;
                npc.dirty = true;
                // A debuff never lands the killing blow itself: the game drops the NPC to one
                // hit point and then strikes it for everything, which is what makes the death
                // go through the ordinary path rather than leaving a corpse at zero.
                if npc.life <= 0 {
                    npc.life = 0;
                    deaths.push((
                        index,
                        npc.npc_type,
                        npc.center(),
                        if npc.from_statue {
                            0.0
                        } else {
                            npc.stats.value
                        },
                    ));
                }
            }
            if std::mem::take(&mut npc.buffs_dirty) {
                changed.push(index);
            }
        }

        for (index, amount) in hits {
            if let Ok(frame) = packets::npc_debuff_damage(index, amount) {
                self.broadcast(frame, None);
            }
        }
        for index in changed {
            self.broadcast_npc_buffs(index);
        }
        for (index, npc_type, center, value) in deaths {
            self.npc_died(index, npc_type, center, value);
        }
    }

    /// Tell everyone what is currently on an NPC.
    ///
    /// Not optional decoration: a client computes its own armour penetration from the buff list
    /// it believes the target has, so an enemy covered in ichor that nobody was told about takes
    /// fifteen points less from every hit than it should. A real, unconditional broadcast, not
    /// [`Self::broadcast_near`]'s distance-gated one: every real send site (`NPC.cs:81959`,
    /// `91090`, `91130`, `93029`) is `NetMessage.SendData(54, -1, -1, ...)` — `remoteClient` and
    /// `ignoreClient` both `-1`, vanilla's own shape for "everyone, no exceptions" — with no
    /// proximity check anywhere in source. A distant player still needs the right armour
    /// penetration the moment they come back into range, which withholding this exactly like an
    /// ordinary position sync would risk losing to the same skip budget.
    pub(super) fn broadcast_npc_buffs(&mut self, index: u8) {
        let Some(npc) = self.npcs.get(index) else {
            return;
        };
        let slots: Vec<(u16, i32)> = npc.buffs.active().map(|s| (s.kind, s.time)).collect();
        if let Ok(frame) = packets::npc_buffs(index, slots) {
            self.broadcast(frame, None);
        }
    }

    /// How much the Dryad's Bane is worth in this world right now.
    fn dryad_bane_rate(&self) -> i32 {
        let p = &self.world.progress;
        crate::game::buffs::dryad_bane_dps(
            &crate::game::buffs::BossesDowned {
                eye: p.downed_boss1,
                evil: p.downed_boss2,
                skeletron: p.downed_boss3,
                queen_bee: p.downed_queen_bee,
                hard_mode: p.hard_mode,
                queen_slime: p.downed_queen_slime,
                destroyer: p.downed_mech1,
                twins: p.downed_mech2,
                prime: p.downed_mech3,
                plantera: p.downed_plantera,
                golem: p.downed_golem,
                cultist: p.downed_ancient_cultist,
                empress: p.downed_empress_of_light,
                fishron: p.downed_fishron,
                infected_seed: false,
            },
            self.effective_difficulty(),
            false,
        )
    }

    /// How many of each stacking debuff's projectile is lodged in each NPC.
    ///
    /// The game's own test is `ai[0] == 1 && ai[1] == whoAmI` — the first says the projectile has
    /// stuck rather than still flying, the second says what it stuck in.
    fn stacked_debuff_projectiles(&self) -> std::collections::HashMap<(u8, u16), usize> {
        let mut counts = std::collections::HashMap::new();
        for (_, projectile) in self.projectiles.iter() {
            if !matches!(
                projectile.projectile_type,
                DAYBREAK_SPEAR | JAVELIN | TENTACLE_SPIKE | BLOOD_BUTCHERER | STARDUST_CELL
            ) {
                continue;
            }
            if projectile.ai[0] != 1.0 {
                continue;
            }
            let stuck_in = projectile.ai[1];
            if !(0.0..=255.0).contains(&stuck_in) {
                continue;
            }
            *counts
                .entry((stuck_in as u8, projectile.projectile_type))
                .or_insert(0usize) += 1;
        }
        counts
    }

    /// Under a blood moon, critters turn evil where they stand.
    ///
    /// `NPC.UpdateNPC_BloodMoonTransformations`, `NPC.cs:93033-93048`: the server runs
    /// `AttemptToConvertNPCToEvil(WorldGen.crimson)` over every NPC while `Main.bloodMoon` is set.
    /// Guarded on the flag here so it costs nothing on an ordinary night, which is the only
    /// difference from vanilla's shape: it makes the same test per NPC per tick either way.
    ///
    /// This is the only way a Corrupt Bunny is supposed to exist. Ours used to list type 47
    /// directly in the surface-corruption spawn pool, so corrupt bunnies appeared out of nothing in
    /// broad daylight, while vanilla's `NPC.Spawner` never spawns one at all.
    fn convert_critters_under_a_blood_moon(&mut self) {
        let crimson = self.world.crimson;
        let converted: Vec<u8> = self
            .npcs
            .iter_mut()
            .filter(|(_, npc)| npc.is_alive())
            .filter_map(|(index, npc)| npc.attempt_to_convert_to_evil(crimson).then_some(index))
            .collect();
        // A transform is a different creature in the same slot, so every client has to be told;
        // `Transform` sets `netUpdate` in vanilla for the same reason.
        for index in converted {
            self.broadcast_npc(index);
        }
    }

    pub(super) fn tick_npcs(&mut self) {
        if self.world.blood_moon {
            self.convert_critters_under_a_blood_moon();
        }
        let targets: Vec<Target> = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.is_playing())
            .map(|p| Target {
                slot: p.slot,
                center: (p.position.0 + 10.0, p.position.1 + 21.0),
                velocity: p.velocity,
                alive: p.life > 0,
            })
            .collect();

        // The AI needs the world and the NPC table at once, so the tile view is built separately
        // rather than borrowing `self` twice.
        // Worm segments trail whatever is in front of them, so leaders are read before anything
        // moves and the whole chain shifts as one.
        let leaders: Vec<(u8, u8, (f32, f32))> = self
            .npcs
            .iter()
            .filter_map(|(index, npc)| npc.follows.map(|ahead| (index, ahead)))
            .filter_map(|(index, ahead)| self.npcs.get(ahead).map(|l| (index, ahead, l.center())))
            .collect();
        for (index, ahead, center) in leaders {
            // A segment whose leader is gone becomes the new head of what remains.
            if self.npcs.get(ahead).is_none() {
                if let Some(npc) = self.npcs.get_mut(index) {
                    npc.follows = None;
                }
                continue;
            }
            if let Some(npc) = self.npcs.get_mut(index) {
                npc_ai::follow_leader(npc, center);
            }
        }

        let mut expired = Vec::new();
        // Things a routine killed, as opposed to things that wandered off.
        let mut slain: Vec<(u8, u16, (f32, f32), f32)> = Vec::new();
        let mut transformed = Vec::new();
        let mut blasts = Vec::new();
        // Life carried home by leeches this tick, delivered once everything has moved.
        let mut healing: Vec<i32> = Vec::new();
        let mut gates: Vec<(i32, i32, bool)> = Vec::new();
        let mut releases: Vec<((f32, f32), bool)> = Vec::new();
        let mut ended: Option<bool> = None;
        let mut close_gates = false;
        let mut raisings: Vec<(f32, f32)> = Vec::new();
        let mut screams = 0usize;
        let mut roars: Vec<(f32, f32)> = Vec::new();
        let mut rituals: Vec<(f32, f32)> = Vec::new();
        let mut clear_stage = false;
        // A boss that wants some of its own minions destroyed, as (its slot, their type, how many).
        let mut culls: Vec<(u8, u16, usize)> = Vec::new();
        // Slots of NPCs one of whose parts was just destroyed and that owe a penalty for it.
        let mut punished: Vec<u8> = Vec::new();
        let mut auras: Vec<((f32, f32), f32)> = Vec::new();
        // A buff a routine wants put straight onto one named player, as (slot, buff id, ticks) —
        // a latched nebula headcrab's Obstructed is currently the only source of these
        // (`Effects::player_buff`'s own doc comment, `NPC.cs:37508-37526`).
        let mut player_buffs: Vec<(u8, u16, i32)> = Vec::new();
        // Items a routine wants left in the world without anything having been killed for them, as
        // (item id, where). A pal's pet is the only source (`Effects::reward`).
        let mut rewards: Vec<(i16, (f32, f32))> = Vec::new();
        // Taken out of the event's own state for the tick so a mage can read it while the table
        // is borrowed, and put back once everything has moved.
        let mut raisable: Vec<(f32, f32)>;
        let mut escaped_probe = false;
        let mut carrying = Vec::new();
        // Solar Crawltipede heads that need their own body grown — see the loop below.
        let mut crawltipedes_to_grow: Vec<(u8, (f32, f32))> = Vec::new();
        // ...and Wyverns, which grow theirs the same way.
        let mut wyverns_to_grow: Vec<(u8, (f32, f32))> = Vec::new();
        // ...and the two desert worms, whose segment counts are rolled rather than fixed, so each
        // carries the body, tail and range to draw in.
        let mut sand_worms_to_grow: Vec<SandWormToGrow> = Vec::new();
        let mut ai_out = npc_ai::AiOutput::default();
        {
            // What the timid critters flee from. Only two styles read it, so the list is only
            // built when one of them is actually about.
            let anything_timid = self
                .npcs
                .iter()
                .any(|(_, n)| matches!(n.stats.ai_style, 26 | 65));
            let hazards: Vec<npc_ai::Hazard> = if anything_timid {
                self.npcs
                    .iter()
                    .filter(|(_, n)| !n.stats.friendly && n.stats.damage > 0)
                    .map(|(_, n)| npc_ai::Hazard {
                        center: n.center(),
                        half: (n.width() / 2.0, n.height() / 2.0),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            // Two styles jostle for space, and they want different lists: a pirate ghost keeps
            // away from other pirate ghosts, a shimmerfly from anything alive at all. Both lists
            // are a scan of the table, so neither is built unless something present reads it.
            let avoid: Vec<(f32, f32, f32)> = {
                use npc_ai::Avoids;
                use terrustia_proto::npc_params::{
                    SHIMMERFLY_SHY_OF_NPCS, SHIMMERFLY_SHY_OF_PLAYERS,
                };
                let wanted: Vec<(u16, Avoids)> = self
                    .npcs
                    .iter()
                    .filter_map(|(_, n)| {
                        npc_ai::avoidance(n.stats.ai_style).map(|a| (n.npc_type, a))
                    })
                    .collect();
                if wanted.is_empty() {
                    Vec::new()
                } else {
                    let own_kind: Vec<u16> = wanted
                        .iter()
                        .filter(|(_, a)| *a == Avoids::OwnKind)
                        .map(|(ty, _)| *ty)
                        .collect();
                    let anything = wanted.iter().any(|(_, a)| *a == Avoids::AnythingAlive);
                    // The reach rides along with each entry because it belongs to the entry, not
                    // to the reader: `NPC.cs:34446` counts a hostile within a hundred pixels and
                    // `NPC.cs:34451` a player within a hundred and fifty. An entry that is only in
                    // the list because some pirate ghost asked for its own kind is not something a
                    // shimmerfly flees at all, so its reach is zero.
                    let mut list: Vec<(f32, f32, f32)> = self
                        .npcs
                        .iter()
                        .filter(|(_, n)| {
                            own_kind.contains(&n.npc_type)
                                || (anything && !n.stats.friendly && n.stats.damage > 0)
                        })
                        .map(|(_, n)| {
                            let (x, y) = n.center();
                            let reach = if !n.stats.friendly && n.stats.damage > 0 {
                                SHIMMERFLY_SHY_OF_NPCS
                            } else {
                                0.0
                            };
                            (x, y, reach)
                        })
                        .collect();
                    if anything {
                        list.extend(
                            targets
                                .iter()
                                .map(|t| (t.center.0, t.center.1, SHIMMERFLY_SHY_OF_PLAYERS)),
                        );
                    }
                    list
                }
            };
            // Where Plantera's hooks have bitten, and how many are still on their way somewhere.
            // The body's own tentacles (the ones with no hook of their own in `ai[3]`) are counted
            // in the same pass rather than a second one, because only they regrow and the roll is
            // against a count of themselves.
            let mut hook_anchors: Vec<(f32, f32)> = Vec::new();
            let mut body_tentacles = 0usize;
            for (_, n) in self.npcs.iter() {
                if n.stats.ai_style == 52 {
                    hook_anchors.push(n.center());
                } else if n.npc_type == terrustia_proto::npc_params::PLANTERA_TENTACLE
                    && n.ai[3] == 0.0
                {
                    body_tentacles += 1;
                }
            }
            let hooks = if hook_anchors.is_empty() {
                None
            } else {
                let count = hook_anchors.len() as f32;
                Some((
                    hook_anchors.iter().map(|a| a.0).sum::<f32>() / count,
                    hook_anchors.iter().map(|a| a.1).sum::<f32>() / count,
                ))
            };
            let moving_hooks = self
                .npcs
                .iter()
                .filter(|(_, n)| n.stats.ai_style == 52 && n.velocity != (0.0, 0.0))
                .count();

            // Which NPCs are currently riding a player, and whose head each is on.
            let latched: Vec<(u16, u8)> = self
                .npcs
                .iter()
                .filter(|(_, n)| n.stats.ai_style == 85 && n.ai[0] == 5.0)
                .map(|(_, n)| (n.npc_type, n.target as u8))
                .collect();
            let world_size = (self.world.width(), self.world.height());
            // Where the hurt are, and where goblins have fallen: a Dark Mage reads both, and
            // building either inside the loop would mean scanning the table once per mage.
            let hurt: Vec<(f32, f32)> = if self.npcs.iter().any(|(_, n)| n.stats.ai_style == 109) {
                self.npcs
                    .iter()
                    .filter(|(_, n)| n.life != n.life_max)
                    .map(|(_, n)| n.center())
                    .collect()
            } else {
                Vec::new()
            };
            // Where hostile NPCs are, for town residents fighting back — built once here rather
            // than scanning the whole table per resident, the same reasoning `hurt` above uses
            // for the Dark Mage.
            //
            // `damage > 0` is load-bearing, not incidental: real vanilla's own candidate filter
            // for this exact system (`AI_007_TownEntities`, `NPC.cs:54033`) skips a candidate
            // whenever `(friendly || damage <= 0) && !stinky` — a harmless critter (a bug, a
            // firefly, a worm) is `friendly: false` in this project's own data the same way a real
            // hostile is, but always has `damage: 0`, and without this check a town resident reads
            // it as a threat and opens fire on it. `stinky` (a real, separate override letting a
            // town NPC treat *any* nearby NPC as a target while under that specific debuff) and
            // `NPCID.Sets.CritterThatCanTurnOnPlayers`/the Skeleton Merchant's own faction
            // exception in that same real condition are not modelled — narrower simplifications on
            // top of the fix, not alternatives to it, in the same spirit this module's own doc
            // comment already discloses its other approximations.
            let hostiles: Vec<Hostile> = if self.npcs.iter().any(|(_, n)| {
                n.stats.town_npc && crate::game::ai::town_combat::town_combat(n.npc_type).is_some()
            }) {
                self.npcs
                    .iter()
                    .filter(|(_, n)| {
                        !n.stats.friendly && !n.stats.town_npc && n.stats.damage > 0 && n.is_alive()
                    })
                    .map(|(slot, n)| (slot, n.center(), n.velocity))
                    .collect()
            } else {
                Vec::new()
            };
            // The Brain of Cthulhu's own centre, for its Creepers' `ai[2..3]` (ai_style 55 in
            // `ai/mod.rs`, whose own comment already says this is the server's job: "The Brain's
            // position is threaded in through ai[2..3] by the server, which knows where every NPC
            // is"). Nothing ever did that threading, so every Creeper read `ai[2] == ai[3] ==
            // 0.0` — its own untouched default — on every one of its own ticks and asked to be
            // removed (`creeper::update`'s `BrainGone` branch) from the moment it spawned. It only
            // ever looked alive because `tick_life`'s ordinary despawn timer resets right back
            // over that removal for as long as a player stands nearby, and lets the removal
            // through the instant one does not — indistinguishable, from a client's own tracked
            // view, from a boss whose escort simply never reliably syncs. Scanned only when a
            // Creeper actually exists, the same guard `hurt`/`hostiles` above use.
            let brain_center: Option<(f32, f32)> = self
                .npcs
                .iter()
                .any(|(_, n)| n.npc_type == terrustia_proto::npc_params::CREEPER)
                .then(|| {
                    self.npcs
                        .iter()
                        .find(|(_, n)| n.npc_type == 266)
                        .map(|(_, n)| n.center())
                })
                .flatten();
            raisable = std::mem::take(&mut self.army.corpses);
            // The event as its own fixtures see it. The arena is surveyed once, when the crystal
            // first asks for its gates, and kept: re-walking it every tick would let a player
            // change where the gates are by building mid-fight.
            let army = crate::game::ai::ArmyView {
                rate: self
                    .army
                    .tier
                    .map_or(0, |tier| tier.lane_spawn_rate(self.army.wave)),
                on_hold: self.army.spawning_on_hold(),
                crystal_alive: self
                    .npcs
                    .iter()
                    .any(|(_, n)| n.npc_type == terrustia_proto::npc_params::DD2_ETERNIA_CRYSTAL),
                arena: self.army_arena,
            };
            // A Moon Lord socket that has been broken open stays in the fight as an empty
            // shell, so counting the parts is not enough to know how far along the fight is.
            let sockets_open = self
                .npcs
                .iter()
                .filter(|(_, n)| matches!(n.stats.ai_style, 78 | 79) && n.ai[0] == -2.0)
                .count();
            // A handful of routines wait on how many of some other type are still alive: the
            // Brain's armour, the Wall's leeches, a pal's escort. One pass counts them all, and
            // only for the types anything actually asks about.
            let census: Vec<(u16, usize)> = crate::game::ai::CENSUS_TYPES
                .iter()
                .map(|&ty| {
                    (
                        ty,
                        self.npcs.iter().filter(|(_, n)| n.npc_type == ty).count(),
                    )
                })
                .filter(|(_, count)| *count > 0)
                .collect();
            // What every NPC that owns parts looks like, read before anything moves so the whole
            // assembly shifts as one. Not only bosses: a Flying Dutchman's cannon and a scutlix's
            // rider hang off ordinary NPCs the same way.
            let parents: std::collections::HashMap<u8, crate::game::ai::boss::skeletron::Parent> =
                self.npcs
                    .iter()
                    .map(|(index, n)| {
                        (
                            index,
                            crate::game::ai::boss::skeletron::Parent {
                                position: n.position,
                                size: (n.width(), n.height()),
                                rotation: n.rotation,
                                scale: n.scale,
                                velocity: n.velocity,
                                direction: n.direction,
                                sprite_direction: n.sprite_direction,
                                time_left: n.time_left,
                                state: n.ai[1],
                                phase: n.ai[0],
                                health: n.life as f32 / n.life_max.max(1) as f32,
                            },
                        )
                    })
                    .collect();
            let tiles = WorldTiles(&self.world);
            // The zone scan reads a forty-tile square, so it runs once a tick for the nearest
            // player rather than once per NPC that happens to care.
            let biome = targets
                .first()
                .map_or(crate::game::spawn::Biome::Forest, |t| {
                    crate::game::spawn::biome_at(
                        &self.world,
                        (t.center.0 / crate::game::npc::TILE) as i32,
                        (t.center.1 / crate::game::npc::TILE) as i32,
                    )
                });
            let conditions = self.ai_conditions(biome);
            for (index, npc) in self.npcs.iter_mut() {
                // Segments are positioned by their leader, not by a routine of their own.
                if npc.follows.is_some() {
                    continue;
                }
                // See `brain_center` above: a Creeper reads its own escort centre from here.
                if npc.npc_type == terrustia_proto::npc_params::CREEPER {
                    let (bx, by) = brain_center.unwrap_or((0.0, 0.0));
                    npc.ai[2] = bx;
                    npc.ai[3] = by;
                }
                // A Solar Crawltipede head grows its own body on its own first tick
                // (`NPC.cs:51913-51936`) — its head is `dontTakeDamage` by design (npc_data.rs's
                // own 412 entry), genuinely not the target, so one with no body attached is not
                // merely incomplete, it is unkillable. `self.npcs` cannot be borrowed mutably
                // again from inside this loop, so the actual growth happens once the loop ends.
                if npc.npc_type == terrustia_proto::npc_params::SOLAR_CRAWLTIPEDE_HEAD
                    && npc.ai[0] == 0.0
                {
                    npc.ai[0] = 1.0;
                    crawltipedes_to_grow.push((index, npc.position));
                }
                // A Wyvern does the same on its own first tick (`NPC.cs:51700-51730`,
                // `type == 87 && ai[0] == 0f`), and it has to, because the sky spawns the head
                // alone: `SpawnAnNPC` calls `SpawnNPC(x, y, 87)` with no segments
                // (`NPC.cs:1411`). One that never grew would be a flying face.
                if npc.npc_type == terrustia_proto::npc_params::WYVERN_HEAD && npc.ai[0] == 0.0 {
                    npc.ai[0] = 1.0;
                    wyverns_to_grow.push((index, npc.position));
                }
                // The Dune Splicer and the Tomb Crawler, likewise (`NPC.cs:51772-51819`). They have
                // to be here rather than in `worm_body`: both arrive from ambient desert spawning,
                // which `spawn` calls straight rather than through any of the spawn-time paths that
                // consult that table, so before this a sandstorm's Dune Splicer was a lone head.
                if let Some((body, tail, least, most)) =
                    terrustia_proto::npc_params::self_growing_sand_worm(npc.npc_type)
                    && npc.ai[0] == 0.0
                {
                    npc.ai[0] = 1.0;
                    sand_worms_to_grow.push((index, npc.position, body, tail, least, most));
                }
                // A part reads its parent through this; it cannot see the table itself. A pal's
                // Goblin Archer is not a *part* of it (it is an ordinary enemy that can be killed,
                // looted and banner-counted on its own), so it carries no `follows_boss`; the pal
                // it stands over is named by the same negative `ai[3]` back-link the guard branch
                // reads, and `parents` is keyed by slot already.
                let parent = npc
                    .follows_boss
                    .or_else(|| {
                        (npc.npc_type == terrustia_proto::npc_params::PAL_ESCORT && npc.ai[3] < 0.0)
                            .then(|| u8::try_from((0.0 - npc.ai[3] - 1.0) as i32).ok())
                            .flatten()
                    })
                    .and_then(|slot| parents.get(&slot).copied());
                let (parent_state, parent_health) =
                    parent.map_or((0.0, 1.0), |p| (p.state, p.health));
                npc_ai::update_with(
                    npc,
                    &tiles,
                    &targets,
                    &mut self.rng,
                    &mut ai_out,
                    npc_ai::Surroundings {
                        conditions,
                        hazards: &hazards,
                        avoid: &avoid,
                        // A headcrab already on this player's head is the only thing that stops
                        // another trying, so it is worked out once per NPC that asks.
                        // Plantera swings from the average of wherever its hooks have bitten; its
                        // hook-borne tentacles want them one at a time.
                        hooks,
                        hook_anchors: &hook_anchors,
                        body_tentacles,
                        // A hook holds on while any of its siblings is still travelling.
                        kin_moving: npc.stats.ai_style == 52
                            && moving_hooks > usize::from(npc.velocity != (0.0, 0.0)),
                        target_taken: npc.stats.ai_style == 85
                            && latched.iter().any(|(ty, slot)| {
                                *ty == npc.npc_type
                                    && Some(*slot) == targets.first().map(|t| t.slot)
                            }),
                        // The nearest *visible* hostile a resident might fight back against — see
                        // `nearest_visible_hostile`'s own doc comment for why this is filtered on
                        // line of sight before distance is compared, not merely left for
                        // `try_combat` to refuse later.
                        hostile: if npc.stats.town_npc {
                            nearest_visible_hostile(&tiles, npc, &hostiles)
                        } else {
                            None
                        },
                        census: &census,
                        army,
                        // A fairy hunting for something to show you is the one routine that
                        // wants a survey of the world rather than a look at its neighbours, so
                        // it is done here and only for the two states that ask.
                        treasure: if npc.stats.ai_style == 112 && matches!(npc.ai[2], 2.0 | 6.0) {
                            crate::game::ai::fairy::treasure(
                                &tiles,
                                npc.center(),
                                (world_size.0, world_size.1),
                            )
                        } else {
                            None
                        },
                        // A Dark Mage picks its spell from what is around it: how many of its
                        // side are hurt, and whether there are goblins on the ground to raise.
                        mage: if npc.stats.ai_style == 109 {
                            let here = npc.center();
                            crate::game::ai::army::mage::MageView {
                                wounded: hurt
                                    .iter()
                                    .filter(|(x, y)| {
                                        (x - here.0).abs() <= HEAL_REACH.0
                                            && (y - here.1).abs() <= HEAL_REACH.1
                                    })
                                    .count(),
                                can_raise: raisable
                                    .iter()
                                    .filter(|c| {
                                        (c.0 - here.0).hypot(c.1 - here.1) <= RAISE_CHECK_RANGE
                                    })
                                    .count()
                                    >= RAISE_MINIMUM,
                            }
                        } else {
                            Default::default()
                        },
                        sockets_open,
                        // `AI_127_Pal_TryUnpackNPC` (`NPC.cs:43496-43508`) over the pal's own two
                        // handles: `(int)aiValue - 1`, in range, and the slot still occupied.
                        // `parents` is keyed by slot and built from the occupied slots only, so
                        // membership in it is vanilla's `Main.npc[num].active` (read at the top of
                        // the tick rather than live, which at most keeps a guard killed earlier in
                        // this same tick counted for one more).
                        //
                        // Only a pal asks. Every other style gets a zero for the cost of one
                        // comparison.
                        own_escorts: if npc.stats.ai_style == 127 {
                            [npc.ai[1], npc.ai[2]]
                                .into_iter()
                                .filter(|handle| {
                                    u8::try_from(*handle as i32 - 1)
                                        .is_ok_and(|slot| parents.contains_key(&slot))
                                })
                                .count()
                        } else {
                            0
                        },
                        parent,
                        parent_state,
                        parent_health,
                        slot: index,
                    },
                );
                // A part raised this tick belongs to the NPC that raised it, which only the
                // caller knows the slot of.
                for summon in &mut ai_out.spawn {
                    if summon.parent == Some(npc_ai::Spawn::OWN_PARENT) {
                        summon.parent = Some(index);
                    }
                }
                if let Some(into) = ai_out.transform.take() {
                    transformed.push((index, into, std::mem::take(&mut ai_out.rest_for)));
                }
                // A bomb that went off does its damage through its own hitbox, which the routine
                // has already swollen; what is left is to make sure it is gone afterwards.
                if std::mem::take(&mut ai_out.detonated) {
                    blasts.push(index);
                }
                if std::mem::take(&mut ai_out.called_invasion) {
                    escaped_probe = true;
                }
                if let (Some(at), Some(rider)) = (ai_out.carry.take(), npc.passenger) {
                    carrying.push((rider, at, npc.velocity));
                }
                // Gates the crystal wants raised, enemies a gate wants let out, and the tick the
                // whole thing ends: all decided by a fixture, all carried out by the server.
                gates.extend(std::mem::take(&mut ai_out.gates));
                if let Some(left) = ai_out.release.take() {
                    releases.push((npc.center(), left));
                }
                if let Some(won) = ai_out.army_ended.take() {
                    ended = Some(won);
                }
                if std::mem::take(&mut ai_out.close_gates) {
                    close_gates = true;
                }
                if std::mem::take(&mut ai_out.raising) {
                    raisings.push(npc.center());
                }
                // Betsy's scream also brings wyverns down through the lane portals, which is what
                // makes it a wall of them rather than the one she calls to herself.
                if std::mem::take(&mut ai_out.screamed) {
                    screams += 1;
                }
                // A roar leaves everyone within earshot slowed, which is what makes Deerclops's
                // opening something you have to be somewhere else for.
                if std::mem::take(&mut ai_out.roared) {
                    roars.push(npc.center());
                }
                // A latched nebula headcrab wants Obstructed put on the player it is riding.
                if let Some(buff) = ai_out.player_buff.take() {
                    player_buffs.push(buff);
                }
                // A pal handing over its pet. Not a drop and not loot: `AI_127_Pal_GiveRewerd`
                // (`NPC.cs:43481-43489`) is a bare `Item.NewItem` at the pal's own centre, and the
                // `life = 0; active = false;` that follows it is a removal rather than a kill, so
                // nothing here goes near `npc_died`.
                if let Some(item) = ai_out.reward.take() {
                    rewards.push((item, npc.center()));
                }
                // A wither beast standing in its aura weakens whoever is standing in it too.
                if let Some(reach) = ai_out.aura.take() {
                    let here = npc.center();
                    auras.push((here, reach));
                }
                // A boss that vanished and wants to come back somewhere else. It is applied here
                // rather than in the routine because the routine cannot see the world's edges.
                if let Some(at) = ai_out.teleport_to.take() {
                    npc.position = (at.0 - npc.width() / 2.0, at.1 - npc.height());
                    npc.velocity = (0.0, 0.0);
                    npc.dirty = true;
                }
                // The tablet finished breaking: the Cultist rises where it stood.
                if std::mem::take(&mut ai_out.ritual_complete) {
                    rituals.push(npc.center());
                }
                // BS3-M5: a second into the Moon Lord's death drama the stage is cleared.
                clear_stage |= std::mem::take(&mut ai_out.cleared_stage);
                // The Lunatic Cultist's ritual, both ways round. A right guess destroys some of
                // its own decoys; a decoy destroyed by a wrong guess stuns whatever it was a copy
                // of. Both reach past the NPC being ticked, so both are carried out below.
                if let Some((npc_type, count)) = ai_out.cull_kin.take() {
                    culls.push((index, npc_type, count));
                }
                if std::mem::take(&mut ai_out.punish_owner)
                    && let Some(owner) = npc.follows_boss
                {
                    punished.push(owner);
                }
                // A leech that got home puts its load into whichever part is worst off, which is
                // what makes ignoring them cost you work you have already done.
                if std::mem::take(&mut ai_out.healed) > 0 {
                    healing.push(std::mem::take(&mut ai_out.healed));
                }
                // A routine that decided this one is dead — a burst spore, an uprooted plant, a
                // fallen lunar pillar, the Moon Lord finishing its ten seconds of coming apart.
                //
                // `effects.died` only sets the life to zero; nothing reaped it, so these lingered
                // at zero health forever and **never dropped anything or recorded the kill**. For
                // the Moon Lord that meant beating the game left no luminite and no flag: the
                // world did not notice you had won.
                if npc.life <= 0 {
                    slain.push((
                        index,
                        npc.npc_type,
                        npc.center(),
                        if npc.from_statue {
                            0.0
                        } else {
                            npc.stats.value
                        },
                    ));
                } else if npc.time_left <= 0 {
                    // Outside the world is a separate reason from running out of time, and it has
                    // to be here or nothing catches it: a flying routine that keeps its vertical
                    // velocity does not turn round at the sky, so a bat or a bird leaves through
                    // the top and carries on for ever. Found in a five-minute capture where one
                    // reached y = -8338 — five hundred tiles above the world — and was still being
                    // simulated and broadcast to every client, five hundred and fifteen times, at
                    // coordinates nothing can draw. The game's own check is the same four-sided
                    // hundred-pixel margin.
                    expired.push(index);
                }
            }
        }

        // A Solar Crawltipede head raised its own body this tick — see the loop above.
        for (head_index, at) in crawltipedes_to_grow {
            use terrustia_proto::npc_params::{
                SOLAR_CRAWLTIPEDE_BODY, SOLAR_CRAWLTIPEDE_SEGMENTS, SOLAR_CRAWLTIPEDE_TAIL,
            };
            self.npcs.grow_worm_body(
                head_index,
                SOLAR_CRAWLTIPEDE_BODY,
                SOLAR_CRAWLTIPEDE_TAIL,
                SOLAR_CRAWLTIPEDE_SEGMENTS,
                at,
            );
        }

        // ...and so did any Wyvern that arrived out of the sky this tick.
        for (head_index, at) in wyverns_to_grow {
            self.npcs
                .grow_worm_chain(head_index, terrustia_proto::npc_params::WYVERN_SEGMENTS, at);
        }

        // ...and any desert worm, whose length is rolled here rather than in the loop above,
        // because that loop holds the NPC table borrowed and this needs the server's own RNG.
        for (head_index, at, body, tail, least, most) in sand_worms_to_grow {
            let segments = self.rng.random_range(least..=most);
            self.npcs
                .grow_worm_body(head_index, body, tail, segments, at);
        }

        // Each load goes to the most hurt part still standing.
        for amount in healing {
            let worst = self
                .npcs
                .iter()
                .filter(|(_, n)| matches!(n.stats.ai_style, 77..=79) && n.life < n.life_max)
                .min_by_key(|(_, n)| n.life)
                .map(|(index, _)| index);
            if let Some(index) = worst
                && let Some(npc) = self.npcs.get_mut(index)
            {
                npc.life = (npc.life + amount).min(npc.life_max);
                npc.dirty = true;
            }
        }

        self.army.corpses = std::mem::take(&mut raisable);

        // Skeletons a mage called up out of the ground where goblins fell.
        for spot in raisings {
            let tier = self.army.tier.map_or(0, |t| t as usize);
            let npc_type =
                terrustia_proto::npc_params::DD2_SKELETON_BY_TIER[tier.saturating_sub(1).min(2)];
            for corpse in self.army.take_raisable(spot) {
                let column = (corpse.0 / crate::game::npc::TILE) as i32;
                let from = (corpse.1 / crate::game::npc::TILE) as i32;
                let Some(ground) = spawn::find_ground(&self.world, column, from) else {
                    continue;
                };
                let at = (corpse.0, (ground - 1) as f32 * crate::game::npc::TILE);
                if let Some(index) = self.npcs.spawn(npc_type, at) {
                    self.broadcast_npc(index);
                }
            }
        }

        // A roar is one moment rather than a state, so what it leaves lasts on its own.
        for at in roars {
            let caught: Vec<u8> = self
                .players
                .iter()
                .flatten()
                .filter(|p| p.is_playing() && p.life > 0)
                .filter(|p| {
                    let (x, y) = (p.position.0 + 10.0, p.position.1 + 21.0);
                    (x - at.0).hypot(y - at.1) < ROAR_REACH
                })
                .map(|p| p.slot)
                .collect();
            for slot in caught {
                if let Ok(frame) =
                    terrustia_proto::packets::add_player_buff(slot, BUFF_SLOW, ROAR_SLOW_TICKS)
                {
                    self.broadcast(frame, None);
                }
            }
        }

        // The aura is refreshed every tick it is out, so a short buff is enough: leaving it is
        // what makes it stop, rather than waiting for a timer.
        for (at, reach) in auras {
            let caught: Vec<u8> = self
                .players
                .iter()
                .flatten()
                .filter(|p| p.is_playing() && p.life > 0)
                .filter(|p| {
                    let (x, y) = (p.position.0 + 10.0, p.position.1 + 21.0);
                    (x - at.0).hypot(y - at.1) < reach
                })
                .map(|p| p.slot)
                .collect();
            for slot in caught {
                if let Ok(frame) = terrustia_proto::packets::add_player_buff(
                    slot,
                    terrustia_proto::npc_params::BUFF_WITHERED_ARMOR,
                    3,
                ) {
                    self.broadcast(frame, None);
                }
            }
        }

        // Same shape as the roar/aura broadcasts above, but each one names its own player rather
        // than catching everyone in a radius. `!player22.creativeGodMode` (`NPC.cs:37522`) is
        // this server's own `journey.is_godmode` gate, the same one `hurt_player` uses for the
        // one other place this server decides something on a player's behalf.
        for (slot, buff, ticks) in player_buffs {
            let alive = self.players[slot as usize]
                .as_ref()
                .is_some_and(|p| p.is_playing() && p.life > 0);
            if alive
                && !self.journey.is_godmode(slot)
                && let Ok(frame) = terrustia_proto::packets::add_player_buff(slot, buff, ticks)
            {
                self.broadcast(frame, None);
            }
        }

        // A pal's pet, left where the pal was standing. `Item.NewItem(GetItemSource_Loot(),
        // base.Center, num, 1, -1)` (`NPC.cs:43488`): one, no prefix, and no owner.
        for (item, at) in rewards {
            self.spawn_item(ItemStack::new(i32::from(item), 1, 0), at);
        }

        for at in rituals {
            if self
                .npcs
                .iter()
                .any(|(_, n)| n.npc_type == terrustia_proto::npc_params::CULTIST)
            {
                continue;
            }
            if let Some(index) = self.npcs.spawn(terrustia_proto::npc_params::CULTIST, at) {
                self.announce("The Lunatic Cultist has awoken!");
                self.broadcast_npc(index);
            }
        }

        for _ in 0..screams {
            let gates: Vec<(f32, f32)> = self
                .npcs
                .iter()
                .filter(|(_, n)| n.npc_type == terrustia_proto::npc_params::DD2_LANE_PORTAL)
                .map(|(_, n)| n.center())
                .collect();
            if gates.is_empty() {
                continue;
            }
            for _ in 0..3 {
                let at = gates[rand::Rng::random_range(&mut self.rng, 0..gates.len())];
                if let Some(index) = self
                    .npcs
                    .spawn(terrustia_proto::npc_params::BETSY_WYVERN, at)
                {
                    self.broadcast_npc(index);
                }
            }
        }

        self.apply_army(gates, releases, ended, close_gates);

        // Doors a fighter finished working at.
        for action in std::mem::take(&mut ai_out.doors) {
            self.apply_door_action(action);
        }

        // Doors a resident opened on its way in, or pulled shut on its way out.
        for action in std::mem::take(&mut ai_out.town_doors) {
            match action {
                crate::game::ai::town::DoorAction::Open { x, y, direction } => {
                    self.apply_door_action(crate::game::ai::fighter::Action::OpenDoor {
                        x,
                        y,
                        direction,
                    });
                }
                crate::game::ai::town::DoorAction::Close { x, y } => self.close_door(x, y, false),
                crate::game::ai::town::DoorAction::None => {}
            }
        }

        // Projectiles a routine threw.
        for shot in std::mem::take(&mut ai_out.shots) {
            self.shots_thrown += 1;
            if let Some(index) = self.projectiles.launch(
                shot.projectile,
                shot.position,
                shot.velocity,
                shot.damage,
                i32::from(shot.time_left),
            ) {
                self.broadcast_projectile(index);
            }
        }

        // A town resident's melee attack landing on a nearby hostile. Mirrors `on_damage_npc`'s
        // death handling, minus the parts that only make sense for a player-originated hit (an
        // ack, a stale-generation check against a client's own aim, a crit roll).
        for hit in std::mem::take(&mut ai_out.melee_hits) {
            let Some(npc) = self.npcs.get_mut(hit.target) else {
                continue;
            };
            // A townsperson's blow never crits (the melee-hit path skips the crit roll on purpose).
            let killed = npc.take_damage(hit.damage, hit.knockback, hit.direction);
            let (npc_type, center) = (npc.npc_type, npc.center());
            let value = if npc.from_statue {
                0.0
            } else {
                npc.stats.value
            };
            if killed {
                self.npc_died(hit.target, npc_type, center, value);
            } else {
                self.broadcast_npc(hit.target);
            }
        }

        // Minions a boss asked for. Capped so a long fight cannot fill every slot with servants.
        for summon in ai_out.spawn {
            if self.npcs.used_slots() >= MAX_MINION_SLOTS {
                break;
            }
            if let Some(index) = self.npcs.spawn(summon.npc_type, summon.position) {
                if let Some(npc) = self.npcs.get_mut(index) {
                    npc.velocity = summon.velocity;
                    // A boss part is raised bound to the boss that asked for it and does not move
                    // on its own; unless the caller pinned ai[0] outright, its side rides in the
                    // velocity's sign, as Skeletron's hands are.
                    if let Some(owner) = summon.parent {
                        npc.follows_boss = Some(owner);
                        npc.velocity = (0.0, 0.0);
                        if summon.ai[0].is_none() {
                            npc.ai[0] = summon.velocity.0.signum();
                        }
                    }
                    // Whatever ai identity the caller pinned: a Wall Hungry's band in ai[0], a
                    // saucer part's side in ai[1], a Moon Lord hand's in ai[2], a Pumpking blade's
                    // phase in ai[3]. Seeded here before the part's own style ever runs.
                    for (slot, value) in summon.ai.iter().enumerate() {
                        if let Some(v) = value {
                            npc.ai[slot] = *v;
                        }
                    }
                }
                // ...and the link back the other way, for a spawner that keeps hold of what it
                // raised: `ai[1 + i] = num2 + 1` on a pal (`NPC.cs:43401`). `NewNPC` hands the slot
                // back in the game; here only this loop knows it, so this is where it is written.
                if let Some((spawner, ai_slot)) = summon.handle
                    && let Some(owner) = self.npcs.get_mut(spawner)
                    && let Some(cell) = owner.ai.get_mut(ai_slot)
                {
                    *cell = f32::from(index) + 1.0;
                    owner.dirty = true;
                }
                self.broadcast_npc(index);
            }
        }

        // Whatever is hanging from a balloon goes exactly where the balloon says, at the
        // balloon's own velocity — it is carried, not trailing.
        for (rider, at, velocity) in carrying {
            if let Some(npc) = self.npcs.get_mut(rider) {
                npc.position = (at.0 - npc.width() / 2.0, at.1 - npc.height() / 2.0);
                npc.velocity = velocity;
                npc.dirty = true;
            }
        }

        // A lost girl who has stopped pretending, or a truffle worm that has gone to ground.
        // The slot is kept so clients see one NPC change rather than one vanish and another
        // appear somewhere in the table.
        for (index, into, rest_for) in transformed {
            if let Some(npc) = self.npcs.get_mut(index) {
                npc.become_type(into);
                if rest_for > 0 {
                    npc.ai[1] = rest_for as f32;
                }
            }
            self.broadcast_npc(index);
        }

        // Anything that detonated has already hurt whatever was inside it, through the enlarged
        // hitbox contact damage reads. It only remains to take it off the table.
        for index in blasts {
            self.npcs.remove(index);
            self.broadcast_npc_death(index);
        }

        // A probe that got away with what it saw brings the Martians down on the world.
        if escaped_probe {
            self.start_invasion(Invasion::Martian);
        }

        // The Lunatic Cultist's right guess: up to N of its own decoys, destroyed outright. A bare
        // kill (`NPC.cs:65243-65262` sets `life = 0; active = false;`), so no loot and no credit.
        for (owner, npc_type, count) in culls {
            let doomed: Vec<u8> = self
                .npcs
                .iter()
                .filter(|(_, n)| n.npc_type == npc_type && n.follows_boss == Some(owner))
                .map(|(index, _)| index)
                .take(count)
                .collect();
            for index in doomed {
                self.npcs.remove(index);
                self.broadcast_npc_death(index);
            }
        }

        // And its wrong guess: the decoy that took the hit has gone, and whatever it was a copy of
        // is stunned for two seconds.
        for owner in punished {
            if let Some(npc) = self.npcs.get_mut(owner)
                && npc.npc_type == terrustia_proto::npc_params::CULTIST
            {
                crate::game::ai::boss::cultist::punish(npc);
            }
        }

        // BS3-M5: a second into the Moon Lord's death drama, every True Eye still hunting is killed
        // outright and every shot the fight left in the air is dropped (`NPC.cs:41752-41764`). The
        // eyes go through `remove`, not `npc_died`: vanilla clears them with a bare
        // `HitEffect(); active = false;`, which pays no loot and records no kill.
        if clear_stage {
            let eyes: Vec<u8> = self
                .npcs
                .iter()
                .filter(|(_, n)| n.npc_type == terrustia_proto::npc_params::MOON_LORD_FREE_EYE)
                .map(|(index, _)| index)
                .collect();
            for index in eyes {
                self.npcs.remove(index);
                self.broadcast_npc_death(index);
            }
            let shots: Vec<u16> = self
                .projectiles
                .iter()
                .filter(|(_, p)| {
                    crate::game::ai::boss::moon_lord::MOON_LORD_SHOTS.contains(&p.projectile_type)
                })
                .map(|(index, _)| index)
                .collect();
            for index in shots {
                self.kill_projectile(index);
            }
        }

        // Deaths first: `npc_died` drops the loot and records the kill, which is the difference
        // between beating the Moon Lord and merely making it go away.
        for (index, npc_type, center, value) in slain {
            self.npc_died(index, npc_type, center, value);
        }

        for index in expired {
            self.npcs.remove(index);
            // A silently vanished NPC would linger on every client, so tell them it is gone.
            self.broadcast_npc_death(index);
        }
        self.resolve_worm_chains();

        // How often an NPC's full state goes out, and to whom.
        //
        // Ported from `NPC.UpdateNetworkCode` and `NPC.StreamUpdatesToNearbyPlayers`, which are two
        // mechanisms rather than one and only make sense together:
        //
        // * a **token bucket** limits full syncs to one per thirty ticks sustained — five for a
        //   boss — with three allowed back to back on top of that;
        // * **proximity streaming** then tops that up for anything actually moving, weighted by how
        //   near each player is, so a creature you are standing next to updates several times a
        //   second while the same creature across the world does not.
        //
        // This server previously had neither, and sent every changed NPC every six ticks to
        // everyone nearby: twenty times the game's sustained rate, measured at seven times its
        // bandwidth over a five-minute capture against the real server on the same world.
        self.tick_npc_syncs();
    }

    /// One tick of NPC network bookkeeping: the rate-limited full sync, then the proximity stream.
    fn tick_npc_syncs(&mut self) {
        // ---- full syncs, rate limited ------------------------------------------------------
        let ready: Vec<u8> = self
            .npcs
            .iter()
            .filter(|(_, npc)| npc.dirty)
            .map(|(index, _)| index)
            .collect();
        for index in ready {
            let Some(npc) = self.npcs.get_mut(index) else {
                continue;
            };
            let cost = if npc.stats.boss {
                crate::game::npc::NET_SPAM_PER_PACKET_BOSS
            } else {
                crate::game::npc::NET_SPAM_PER_PACKET
            };
            if npc.net_spam > crate::game::npc::NET_SPAM_PACKET_LIMIT * cost {
                // Out of tokens. It stays dirty and is tried again next tick, which is what makes
                // this a delay rather than a dropped update.
                continue;
            }
            npc.net_spam += cost;
            npc.dirty = false;
            SYNC_FULL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Cleared before sending and put back if anybody was skipped. Clearing it
            // unconditionally silently loses a one-off change to a distant NPC: it is marked dirty,
            // the one broadcast it earns is withheld from every faraway player, and since nothing
            // changes it again it is never sent. A player who was elsewhere at that moment sees
            // that NPC at its old health for the rest of the session.
            if self.broadcast_npc(index)
                && let Some(npc) = self.npcs.get_mut(index)
            {
                npc.dirty = true;
            }
        }

        // The bucket drains a tick at a time, which is what sets the sustained rate.
        for (_, npc) in self.npcs.iter_mut() {
            if npc.net_spam > 0 {
                npc.net_spam -= 1;
            }
        }

        // ---- proximity streaming ------------------------------------------------------------
        //
        // Only for things that are moving: a stationary creature has nothing to interpolate and is
        // already correct on every client that has been told about it once.
        let streaming: Vec<(u8, (f32, f32))> = self
            .npcs
            .iter_mut()
            .filter(|(_, npc)| {
                !npc.stats.town_npc
                    && npc.velocity.0.abs() + npc.velocity.1.abs() > 0.5
                    // The three the game excludes from proximity syncing, via
                    // `NPCID.Sets.UsesMultiplayerProximitySyncing`.
                    && !matches!(npc.npc_type, 396..=398)
            })
            .filter_map(|(index, npc)| {
                npc.net_stream = npc.net_stream.saturating_add(1);
                if npc.net_stream < crate::game::npc::NPC_STREAM_SPEED {
                    return None;
                }
                npc.net_stream = 0;
                Some((index, npc.center()))
            })
            .collect();

        for (index, at) in streaming {
            let watchers: Vec<(u8, (f32, f32))> = self
                .players
                .iter()
                .flatten()
                .filter(|p| p.is_playing())
                .map(|p| (p.slot, p.position))
                .collect();
            for (slot, position) in watchers {
                let distance = ((position.0 - at.0).powi(2) + (position.1 - at.1).powi(2)).sqrt();
                let weight = stream_weight(distance);
                if weight == 0 {
                    continue;
                }
                let counter = self.npc_stream.entry((index, slot)).or_insert(0);
                *counter = counter.saturating_add(weight);
                if *counter < STREAM_THRESHOLD {
                    continue;
                }
                *counter = 0;
                SYNC_STREAM.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if let Some(sync) = self.npc_sync(index)
                    && let Ok(frame) = sync.encode()
                {
                    self.send(slot, frame);
                }
            }
        }
    }

    /// Put the Eater of Worlds back together after something has been cut out of it.
    ///
    /// This is the whole reason the fight is what it is: a severed body segment does not leave a
    /// gap, it becomes two worms. The segment ahead of the wound grows a tail and the one behind it
    /// grows a head, and both keep fighting. A head with nothing behind it, or a tail with nothing
    /// ahead, is a single segment and dies.
    fn resolve_worm_chains(&mut self) {
        use terrustia_proto::npc_params::splitting_worm;

        // Who follows whom, as it stands now.
        let leaders: std::collections::HashMap<u8, u8> = self
            .npcs
            .iter()
            .filter_map(|(index, npc)| npc.follows.map(|leader| (index, leader)))
            .collect();
        let followed: std::collections::HashSet<u8> = leaders.values().copied().collect();

        let mut transformed = Vec::new();
        let mut orphaned = Vec::new();
        for (index, npc) in self.npcs.iter() {
            let Some((head, body, tail)) = splitting_worm(npc.npc_type) else {
                continue;
            };
            let has_leader = npc
                .follows
                .is_some_and(|leader| self.npcs.get(leader).is_some());
            let has_follower = followed.contains(&index);

            if !has_leader && !has_follower {
                // A lone segment is not a worm.
                orphaned.push(index);
            } else if npc.npc_type == body && !has_leader {
                // The wound is ahead of it: it becomes the head of what is left.
                transformed.push((index, head));
            } else if npc.npc_type == body && !has_follower {
                // The wound is behind it: it becomes the tail.
                transformed.push((index, tail));
            } else if (npc.npc_type == head && !has_follower)
                || (npc.npc_type == tail && !has_leader)
            {
                // An end with nothing attached to it is the last of its worm.
                orphaned.push(index);
            }
        }

        for (index, into) in transformed {
            if let Some(npc) = self.npcs.get_mut(index) {
                // The chain link survives the change of type; only what it is changes.
                let follows = npc.follows;
                npc.become_type(into);
                npc.follows = follows;
            }
            self.broadcast_npc(index);
        }
        for index in orphaned {
            self.npcs.remove(index);
            self.broadcast_npc_death(index);
        }
    }

    /// Which of a type's looks a newly-arrived one wears.
    ///
    /// One for almost everything; a cat, a dog or a bunny rolls one of six breeds, which decides
    /// both its sprite and which names it can be given.
    pub(super) fn roll_town_variation(&mut self, npc_type: u16) -> i32 {
        let count = terrustia_proto::town_names::variation_count(npc_type);
        if count <= 1 {
            return 0;
        }
        rand::Rng::random_range(&mut self.rng, 0..count) as i32
    }

    /// Pick a name, preferring one nobody in the world is already using.
    ///
    /// The game does not guarantee uniqueness either, but it does re-roll the duplicates when a
    /// second of a type arrives, and a town with two Andrews reads as a bug. Falling back to a
    /// plain roll when every name is taken is what keeps a world with sixty cats working.
    pub(super) fn roll_town_name(&mut self, npc_type: u16, variation: i32) -> String {
        let names = terrustia_proto::town_names::names_for_variation(
            npc_type,
            usize::try_from(variation).unwrap_or(0),
        );
        if names.is_empty() {
            return String::new();
        }
        let taken: Vec<&str> = self
            .npcs
            .iter()
            .filter(|(_, n)| n.npc_type == npc_type && !n.given_name.is_empty())
            .map(|(_, n)| n.given_name.as_str())
            .collect();
        let free: Vec<&&str> = names.iter().filter(|n| !taken.contains(n)).collect();
        if free.is_empty() {
            let at = rand::Rng::random_range(&mut self.rng, 0..names.len());
            return names[at].to_string();
        }
        let at = rand::Rng::random_range(&mut self.rng, 0..free.len());
        (*free[at]).to_string()
    }

    /// Move every projectile, and remove the ones that are finished.
    pub(super) fn tick_projectiles(&mut self) {
        let mut spent = Vec::new();
        let mut emitted = Vec::new();
        {
            let tiles = WorldTiles(&self.world);
            for (index, projectile) in self.projectiles.iter_mut() {
                if crate::game::projectile::step(projectile, &tiles, &mut emitted)
                    == crate::game::projectile::Outcome::Spent
                {
                    spent.push(index);
                }
            }
        }
        for index in spent {
            self.kill_projectile(index);
        }
        // A flamethrower's flames, and anything else a projectile decided to put in the air.
        for emit in emitted {
            if let Some(index) = self.projectiles.launch(
                emit.projectile_type,
                emit.position,
                emit.velocity,
                emit.damage,
                0,
            ) {
                self.broadcast_projectile(index);
            }
        }

        // Clients interpolate between updates, so projectiles go out at the same rate NPCs do.
        if self.ticks.is_multiple_of(NPC_SYNC_INTERVAL) {
            let dirty: Vec<u16> = self
                .projectiles
                .iter()
                .filter(|(_, p)| p.dirty)
                .map(|(index, _)| index)
                .collect();
            for index in dirty {
                if let Some(p) = self.projectiles.get_mut(index) {
                    p.dirty = false;
                }
                self.broadcast_projectile(index);
            }
        }
    }

    /// Move the Purification Powder clouds in flight, and turn whatever they cover.
    ///
    /// `Projectile.Damage_TryUsingPowders` (`Projectile.cs:14787-14826`), both arms:
    ///
    /// ```csharp
    /// if (type == 10 && Main.netMode != 1) {
    ///     for (int i = 0; i < Main.maxNPCs; i++) {
    ///         NPC nPC = Main.npc[i];
    ///         if (!nPC.active) continue;
    ///         if (nPC.type == 534) {
    ///             if (projRectangle.Intersects(nPC.Hitbox)) { nPC.Transform(441); }
    ///         } else {
    ///             if (nPC.type != 687 || !projRectangle.Intersects(nPC.Hitbox)) continue;
    ///             nPC.Transform(683);
    ///             ... Utils.PoofOfSmoke(vector); ...
    ///             if (!NPC.unlockedSlimeYellowSpawn) {
    ///                 NPC.unlockedSlimeYellowSpawn = true;
    ///                 if (Main.netMode == 2) { NetMessage.SendData(7); }
    /// ```
    ///
    /// This is the whole recruitment for both: no cost beyond the powder the throw already spent,
    /// no second interaction, no guard against re-purifying (there is nothing left to purify, the
    /// Tortured Soul is gone), and no announcement, because `Transform` makes none. The Tax
    /// Collector and the Yellow Slime are the two townsfolk in the game who arrive this way, which
    /// is why neither sits in the rescue table.
    ///
    /// Two deliberate narrowings, both disclosed rather than papered over. Vanilla's
    /// `Utils.PoofOfSmoke` and its packet 106 are cosmetic and this server treats 106 as a
    /// client-only packet, so the puff of smoke is not sent. And the cloud's own flight is followed
    /// rather than the client's word for where it went: packet 27 arrives once, at the throw, so
    /// its 180 ticks of drift are what [`crate::game::projectile::Powder::step`] reproduces.
    pub(super) fn tick_powders(&mut self) {
        if self.powders.is_empty() {
            return;
        }
        self.powders
            .retain_mut(crate::game::projectile::Powder::step);

        // Vanilla walks every NPC per powder; this walks the two convertible types once and asks
        // the powders, which is the same test with the loops the other way up. A world holds at
        // most one Tortured Soul (`NPC.cs:4877`'s own `!AnyNPCs(534)`) and at most one bound
        // Yellow Slime (`NPC.cs:5623`'s `!AnyNPCs(687)`), so this is nearly always an empty scan.
        let turned: Vec<(u8, u16)> =
            self.npcs
                .iter()
                .filter_map(|(index, npc)| {
                    let into = match npc.npc_type {
                        crate::game::spawn::TORTURED_SOUL => TAX_COLLECTOR,
                        crate::game::spawn::BOUND_TOWN_SLIME_YELLOW => YELLOW_SLIME,
                        _ => return None,
                    };
                    let covered = npc.is_alive()
                        && self.powders.iter().any(|powder| {
                            powder.overlaps(npc.position, (npc.width(), npc.height()))
                        });
                    covered.then_some((index, into))
                })
                .collect();

        for (index, into) in turned {
            if let Some(npc) = self.npcs.get_mut(index) {
                npc.become_type(into);
            }
            // `NPC.AI_007_TownEntities_UpdateSavedStates` (`NPC.cs:53489-53491`) sets
            // `savedTaxCollector` from the Tax Collector's own AI, on the first tick he runs one.
            // Setting it here instead is the same moment a tick earlier, and it is what shuts the
            // underworld's spawn branch so a second Tortured Soul never appears. The Yellow Slime's
            // own flag is set right here in vanilla too (`Projectile.cs:14818-14824`), and it is
            // what shuts `SpawnFrog`'s first arm.
            crate::game::rescues::remember(&mut self.world.progress, into);
            self.broadcast_npc(index);
            self.broadcast_world_data();
            if into == TAX_COLLECTOR {
                info!("a Tortured Soul was purified into the Tax Collector");
            } else {
                info!("a bound Yellow Slime was purified and moved in");
            }
        }
    }

    /// Hurt anyone standing in an enemy or in something one of them threw.
    ///
    /// The invulnerability window is what makes this survivable: without it a player inside a
    /// zombie would take sixty hits a second rather than one every half second.
    pub(super) fn tick_contact_damage(&mut self) {
        // The difficulty the hostile-projectile curve is sampled at, read once for the whole tick.
        let difficulty = self.effective_difficulty();

        for slot in 0..self.players.len() {
            let Some(player) = self.players[slot].as_ref() else {
                continue;
            };
            if !player.is_playing() || player.life <= 0 {
                continue;
            }
            if player.immune_ticks > 0 {
                if let Some(player) = self.players[slot].as_mut() {
                    player.immune_ticks -= 1;
                }
                continue;
            }
            let box_at = player.position;
            let box_size = (
                crate::game::ai::PLAYER_WIDTH as f32,
                crate::game::ai::PLAYER_HEIGHT as f32,
            );
            // The push always points away from the attacker, measured off the player's centre, not
            // its left edge. `Update_NPCCollision` (`Player.cs:31618`) and `Projectile.Damage`
            // (`Projectile.cs:14894`): hitDirection is +1 when the attacker's centre is left of the
            // player's centre, -1 when it is right. The old code inverted both and compared against
            // the player's left edge, so contact knockback shoved the player the wrong way.
            let player_centre = box_at.0 + box_size.0 / 2.0;

            // An enemy you are standing in. The skip is on the *live* damage, as vanilla's own
            // `Main.npc[i].damage <= 0` is (`Player.cs:31564`), so a routine that has zeroed its
            // damage for the phase it is in - a Big Mimic that has given up, say - is walked
            // through rather than merely hit for the table's minimum of one.
            let hit = self.npcs.iter().find(|(_, npc)| {
                !npc.stats.friendly
                    && npc.contact_damage() > 0
                    && npc.is_alive()
                    && npc.position.0 < box_at.0 + box_size.0
                    && npc.position.0 + npc.width() > box_at.0
                    && npc.position.1 < box_at.1 + box_size.1
                    && npc.position.1 + npc.height() > box_at.1
            });
            if let Some((index, npc)) = hit {
                // Contact damage is the NPC's *live* `damage` (`Player.cs:31623` reads
                // `Main.npc[i].damage` straight, times a GetMeleeCollisionData multiplier we leave
                // at one and a DamageVar the owning client rolls for itself). Vanilla's live number
                // is the spawn-scaled `defDamage` as the routine currently has it, which is
                // [`Npc::contact_damage`]: the difficulty scaling from `ScaleStats` plus whatever
                // phase multiplier the AI is holding. Reading `stats.damage` here instead dropped
                // every one of those phases on the floor, so a Prime spin hit like a hover and a
                // Mothron chase hit twice as hard as it should.
                let damage = npc.contact_damage();
                let direction = if npc.center().0 < player_centre {
                    1
                } else {
                    -1
                };
                let npc_type = npc.npc_type;
                let hurt = self.hurt_player(
                    slot,
                    damage,
                    direction,
                    terrustia_proto::hurt::DeathReason::from_npc(i16::from(index)),
                );
                // Over half the roster leaves something behind as well as the damage, and for
                // several of them that is the actual difficulty of the biome they live in. The game
                // only lands these when the hit itself lands (`Player.cs:31659-31661`, `StatusFromNPC`
                // gated on `Hurt(...) > 0`), so a godmoded or already-immune player is spared them.
                if hurt {
                    self.apply_touch_debuffs(slot as u8, npc_type, difficulty);
                }
                continue;
            }

            // Or something one of them threw. Only hostile shots hurt a player: a friendly town-NPC
            // musket ball that happens to overlap one does not (`Projectile.Damage`'s own
            // `if (!hostile) return` at the top).
            let struck = self
                .projectiles
                .iter()
                .find(|(_, p)| p.stats.hostile && p.damage > 0 && p.overlaps(box_at, box_size))
                .map(|(index, p)| (index, p.damage, p.center().0, p.projectile_type));
            if let Some((index, base_damage, from_x, projectile_type)) = struck {
                // A hostile shot delivers `base * hostileDamageScaling(difficulty) * 2`, and the game
                // applies both on the client at impact (`Projectile.cs:14916-14919`), not baked into
                // the projectile. The wire carries the base (see `broadcast_projectile`), so a real
                // client scales it once itself rather than twice.
                let damage = (base_damage as f32
                    * terrustia_proto::difficulty::hostile_projectile_multiplier(difficulty)
                    * 2.0) as i32;
                let direction = if from_x < player_centre { 1 } else { -1 };
                self.hurt_player(
                    slot,
                    damage,
                    direction,
                    terrustia_proto::hurt::DeathReason::from_projectile(
                        index as i16,
                        projectile_type as i16,
                    ),
                );
                // A projectile with a hit budget spends one, and dies when it runs out.
                let spent = self.projectiles.get_mut(index).is_some_and(|p| {
                    if p.penetrate > 0 {
                        p.penetrate -= 1;
                    }
                    p.penetrate == 0
                });
                if spent {
                    self.kill_projectile(index);
                }
            }
        }

        self.tick_town_casualties();
    }

    /// Enemies hurt the townsfolk too.
    ///
    /// A blood moon or an invasion that walks through a town and leaves it standing is not a
    /// threat, it is scenery. This is also the only thing that makes a townsperson's armour mean
    /// anything, and their armour is most of what the world's history does for them: the guide who
    /// died to a zombie on the first night can hold a doorway by hardmode.
    fn tick_town_casualties(&mut self) {
        /// How long a townsperson is safe for after being hit.
        ///
        /// Counted on the world's clock rather than per NPC: they are all hit on the same tick,
        /// which costs one field fewer on every NPC and is indistinguishable in play.
        const TOWN_IMMUNE_TICKS: u64 = 30;
        if !self.ticks.is_multiple_of(TOWN_IMMUNE_TICKS) {
            return;
        }

        let residents: Vec<u8> = self
            .npcs
            .iter()
            .filter(|(_, n)| n.stats.town_npc && n.is_alive())
            .map(|(index, _)| index)
            .collect();
        if residents.is_empty() {
            return;
        }
        let toughness = self.town_toughness();
        // `AI_007_TownEntities` rebuilds `defense` from `defDefense` every tick, and two buffs sit
        // on either side of the progression bonus: Dryad's Blessing raises the base by difficulty
        // (`NPC.cs:53550-53561`), and Tipsy multiplies the finished figure (`NPC.cs:53699-53706`).
        let ward = if self.is_master() {
            20
        } else if self.is_expert() {
            15
        } else {
            10
        };

        for index in residents {
            // Their armour is the type's plus everything the world has beaten.
            let Some(resident) = self.npcs.get_mut(index) else {
                continue;
            };
            let flags = resident.buffs.flags;
            let base = resident.stats.defense + if flags.dryad_ward { ward } else { 0 };
            resident.defense = base + toughness.defense;
            if flags.tipsy {
                // `defense = (int)((double)defense * 1.1)`, truncating.
                resident.defense = (f64::from(resident.defense) * 1.1) as i32;
            }
            let (at, size) = (resident.position, (resident.width(), resident.height()));

            let attacker = self.npcs.iter().find(|(other, n)| {
                *other != index
                    && !n.stats.friendly
                    && n.contact_damage() > 0
                    && n.is_alive()
                    && n.position.0 < at.0 + size.0
                    && n.position.0 + n.width() > at.0
                    && n.position.1 < at.1 + size.1
                    && n.position.1 + n.height() > at.1
            });
            let Some((_, enemy)) = attacker else {
                continue;
            };
            // The live number again, not the table's: a rolling tortoise that walks through a town
            // hits the residents as hard as it hits a player.
            let (damage, from_x) = (enemy.contact_damage(), enemy.center().0);
            let Some(resident) = self.npcs.get_mut(index) else {
                continue;
            };
            let taken = damage_taken(damage, resident.defense, false);
            // hitDirection points away from the attacker, off the resident's centre, as everywhere
            // else (`Player.cs:31618`): the old form inverted it and measured off the left edge. It
            // is inert while the knockback is zero, but correct for when it is not. The knockback
            // itself, and the global 30-tick immune clock this loop runs on, stay deliberate
            // simplifications of the town-casualty path: the game strikes a townsperson through the
            // attacking enemy's own AI, which carries a per-enemy contact knockback this server does
            // not model.
            let resident_centre = at.0 + resident.width() / 2.0;
            let direction = if from_x < resident_centre { 1 } else { -1 };
            let (killed, name) = (
                resident.take_damage(taken, 0.0, direction),
                resident.stats.name,
            );
            if killed {
                self.npcs.remove(index);
                self.broadcast_npc_death(index);
                // `LegacyMisc.19` is `"{0} was slain..."`. Vanilla carries a structured
                // `PlayerDeathReason` for the richer forms; this is the plain one.
                let who = NetworkText::literal(name);
                self.announce_key("LegacyMisc.19", vec![who]);
                info!(name, "a townsperson was killed");
            } else {
                self.broadcast_npc(index);
            }
        }
    }

    /// How tough this world's townsfolk are, from everything it has beaten.
    fn town_toughness(&self) -> terrustia_proto::npc_params::TownToughness {
        let p = &self.world.progress;
        terrustia_proto::npc_params::town_toughness(
            &[
                p.downed_king_slime,
                p.downed_boss1,
                p.downed_deerclops,
                p.downed_boss2,
                p.downed_boss3,
                p.downed_queen_bee,
                p.hard_mode,
                p.downed_queen_slime,
                p.downed_mech1,
                p.downed_mech2,
                p.downed_mech3,
                p.downed_plantera,
                p.downed_empress_of_light,
                p.downed_fishron,
                p.downed_golem,
            ],
            (p.combat_book, p.combat_book_two),
        )
    }

    /// Take health off a player, tell everyone, and announce a death if it was fatal.
    ///
    /// Returns whether the hit actually landed, so the caller knows whether to follow it with a
    /// touch debuff (the game only applies those when `Hurt` returned damage).
    fn hurt_player(
        &mut self,
        slot: usize,
        damage: i32,
        direction: i8,
        reason: terrustia_proto::hurt::DeathReason,
    ) -> bool {
        // Journey mode's `Godmode`. Real vanilla's own `creativeGodMode` gates apply client-side
        // (`Player.cs:31557`/`38486`/`39107`), since most damage in that game is client-decided —
        // this is the one place *this* server decides damage on a player's behalf at all (NPC
        // contact and NPC-thrown projectiles, this function's only two call sites), so it is the
        // one place this server needs its own gate to match.
        if self.journey.is_godmode(slot as u8) {
            return false;
        }
        let Some(player) = self.players[slot].as_mut() else {
            return false;
        };
        let taken = damage.max(1) as i16;
        player.life -= taken;
        // The invulnerability window, from `Hurt` (`Player.cs:38672`): a real hit gives forty ticks,
        // a bare one-damage hit only twenty. `longInvince` (a Cross-Necklace-class accessory) would
        // double both to eighty and forty, but the server does not track player accessories, so that
        // doubling is a documented gap rather than modelled. This was a flat thirty before, so every
        // enemy hit a player slightly too often and a chip-damage hit far too often.
        player.immune_ticks = if taken == 1 { 20 } else { 40 };
        let died = player.life <= 0;
        if died {
            player.life = 0;
        }
        let index = player.slot;

        if died {
            if let Ok(frame) = (terrustia_proto::hurt::PlayerDeath {
                player: index,
                reason,
                damage: taken,
                direction,
                pvp: false,
            })
            .encode()
            {
                self.broadcast(frame, None);
            }
        } else if let Ok(frame) = (terrustia_proto::hurt::PlayerHurt {
            player: index,
            reason,
            damage: taken,
            direction,
            crit: false,
            pvp: false,
            cooldown: -1,
        })
        .encode()
        {
            self.broadcast(frame, None);
        }
        true
    }

    /// Tell everyone a projectile is gone, and free its slot.
    fn kill_projectile(&mut self, index: u16) {
        let Some(projectile) = self.projectiles.remove(index) else {
            return;
        };
        if let Ok(frame) = (terrustia_proto::projectile::KillProjectile {
            key: projectile.key,
            position: projectile.position,
        })
        .encode()
        {
            self.broadcast(frame, None);
        }
        // The server ends a projectile here; a client reporting its own kill ends it in
        // `on_client_projectile_kill`. Both have to drop the skip runs, because a projectile
        // identity carries a generation that keeps climbing and so is never reused. Forgetting on
        // only the client path would leak one entry per distant player for every projectile the
        // server expired on its own, which is most of them.
        self.forget_skips(Withheld::Projectile(projectile.key.pack()));
    }

    /// Tell everyone where a projectile is.
    fn broadcast_projectile(&mut self, index: u16) {
        let Some(p) = self.projectiles.get(index) else {
            return;
        };
        let sync = terrustia_proto::projectile::SyncProjectile {
            key: p.key,
            position: p.position,
            velocity: p.velocity,
            projectile_type: p.projectile_type as i16,
            ai: p.ai,
            banner: 0,
            damage: p.damage as i16,
            knockback: p.knockback,
            original_damage: p.damage as i16,
        };
        let at = sync.position;
        if let Ok(frame) = sync.encode() {
            // Same rule as an NPC's, which is the game's: a projectile is only news to the
            // players whose part of the world it is flying through. Unlike an NPC it has no skip
            // cap, because one that has left never needs catching up on — it is gone.
            self.broadcast_to_nearby(frame, at);
        }
    }

    /// Send a frame only to the players near a point.
    pub(super) fn broadcast_to_nearby(&mut self, frame: Vec<u8>, at: (f32, f32)) {
        let bytes = Bytes::from(frame);
        for slot in self.players_near(at) {
            self.send_bytes(slot, bytes.clone());
        }
    }

    /// The players whose loaded part of the world covers a point.
    fn players_near(&self, at: (f32, f32)) -> Vec<u8> {
        let section = section_of(at);
        self.players
            .iter()
            .flatten()
            .filter(|p| p.is_playing() && near_section(p.position, section))
            .map(|p| p.slot)
            .collect()
    }

    /// Keep every player's own block of sections loaded as they move.
    ///
    /// The server pushes sections; it does not wait to be asked. `Main.cs:65601` calls
    /// `RemoteClient.CheckSection(k, player[k].position)` for every active player on every server
    /// tick, and `CheckSection_ForClient` (`RemoteClient.cs:152-190`) walks the `fluff = 1` block
    /// around wherever that player is *now*, announces how many of those sections are new
    /// (`SendData(9, ..)`, `Lang.inter[44]`) and then sends each one. Packet 159 is not the
    /// streaming path: its only two senders in the whole game are a dropped item's owner search
    /// and a rope placement onto an unloaded tile, both repairs for a miss.
    ///
    /// Without this a player who walked out of the block sent at their join saw sky forever, and
    /// could not build out there either, because `on_tile_manipulation` and `on_place_object` both
    /// refuse an edit in a section the client was never sent.
    ///
    /// Two departures from the game's own shape, both for the 16.67ms tick at 255 players:
    ///
    /// - A player whose section has not changed since the last tick is skipped outright, rather
    ///   than re-walking nine sections to conclude the same thing (`Player::last_section`).
    /// - New sections are queued rather than sent here, so they go out through the same shared,
    ///   time-bounded drain a join already uses (`drain_section_streams`,
    ///   [`SECTION_STREAM_BUDGET`]). Sending three to five sections inline would reproduce exactly
    ///   the synchronous burst that drain exists to prevent, just triggered by walking instead of
    ///   joining.
    ///
    /// Every server-side relocation is covered by this one place, because each of them moves
    /// `Player::position`: a Teleportation Potion, a pylon (whose own
    /// `RemoteClient.CheckSection`, `TeleportPylonsSystem.cs:199`, is this check run a tick
    /// early), a magic mirror, a death respawn.
    pub(super) fn check_player_sections(&mut self) {
        let (max_x, max_y) = (self.world.sections_x(), self.world.sections_y());
        // Slot and how many sections are newly owed, for the status line vanilla sends first.
        let mut announce: Vec<(u8, i32)> = Vec::new();

        for player in self.players.iter_mut().flatten() {
            // `player[k].active` in vanilla's own loop: someone actually in the world. A client
            // still working through its join stream has a queue of its own and is not moving yet.
            if !player.is_playing() {
                continue;
            }
            let at = section_of(player.position);
            if player.last_section == Some(at) {
                continue;
            }
            player.last_section = Some(at);

            let mut owed = 0;
            // `SECTION_REACH` is vanilla's `fluff` under another name, and deliberately the same
            // constant `near_section` culls broadcasts by: what a client is sent and what the
            // server assumes it has loaded must be the same block, or an NPC gets skipped for a
            // section the player really does have (or vice versa).
            for sx in (at.0 - SECTION_REACH)..=(at.0 + SECTION_REACH) {
                for sy in (at.1 - SECTION_REACH)..=(at.1 + SECTION_REACH) {
                    if sx < 0 || sy < 0 || sx >= max_x || sy >= max_y {
                        continue;
                    }
                    if player.sent_sections.contains(&(sx, sy)) {
                        continue;
                    }
                    player.pending_sections.push_back((sx, sy));
                    owed += 1;
                }
            }
            if owed > 0 {
                announce.push((player.slot, owed));
            }
        }

        for (slot, owed) in announce {
            // The key rather than the English, for the same reason the join sends the key.
            match packets::status_text(owed, &NetworkText::key("LegacyInterface.44", Vec::new()), 0)
            {
                Ok(frame) => self.send(slot, frame),
                Err(e) => warn!(slot, error = %e, "could not encode a section status line"),
            }
        }
    }

    /// Drop cached sections whose tiles have changed.
    ///
    /// This has to run before a section is served, not merely once a tick: an edit and a join can
    /// land in the same batch of events, and a section sent in between would carry stale tiles.
    pub(super) fn flush_dirty_sections(&mut self) {
        for section in self.world.take_dirty_sections() {
            self.section_cache.remove(&section);
        }
    }

    fn npc_sync(&self, index: u8) -> Option<SyncNpc> {
        let npc = self.npcs.get(index)?;
        Some(SyncNpc {
            index,
            generation: npc.generation,
            position: npc.position,
            velocity: npc.velocity,
            target: npc.target,
            direction: npc.direction,
            direction_y: npc.direction_y,
            sprite_direction: npc.sprite_direction,
            ai: npc.ai,
            net_id: npc.npc_type as i16,
            life: npc.life,
            life_max: npc.life_max,
            release_owner: 255,
        })
    }

    /// Returns whether the frame was withheld from at least one player, so the caller knows this
    /// NPC still owes somebody an update.
    pub(super) fn broadcast_npc(&mut self, index: u8) -> bool {
        let Some(sync) = self.npc_sync(index) else {
            return false;
        };
        let Ok(frame) = sync.encode() else {
            return false;
        };
        let at = sync.position;
        self.broadcast_near(frame, at, Withheld::Npc(index), MAX_NPC_SYNC_SKIPS, None)
    }

    /// Send an update only to the players whose part of the world it happened in.
    ///
    /// A broadcast to everybody is what a server can least afford: with two hundred NPCs awake and
    /// a sync every six ticks, sending each to every player is thousands of frames a second per
    /// client, and a client that cannot drain that fast is dropped for being slow. The game's own
    /// rule is to skip an NPC for a client whose loaded sections do not cover it — but never more
    /// than four times in a row, so something far away still gets an occasional update rather than
    /// freezing where it was last seen.
    ///
    /// `what` identifies the thing being withheld so each has its own run of skips against each
    /// player, and `budget` is how many may go by before one is sent anyway. NPCs use the game's
    /// own four ([`MAX_NPC_SYNC_SKIPS`]); things that arrive every tick rather than every sixth
    /// need a larger one to actually shed the fan-out ([`MAX_PLAYER_SYNC_SKIPS`]).
    ///
    /// Returns whether anybody was skipped, which is the caller's cue to try again next interval.
    pub(super) fn broadcast_near(
        &mut self,
        frame: Vec<u8>,
        at: (f32, f32),
        what: Withheld,
        budget: u8,
        except: Option<u8>,
    ) -> bool {
        let bytes = Bytes::from(frame);
        let mut withheld = false;
        let section = section_of(at);
        let targets: Vec<(u8, bool)> = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.is_playing() && Some(p.slot) != except)
            .map(|p| (p.slot, near_section(p.position, section)))
            .collect();
        for (slot, near) in targets {
            if !near {
                let skipped = self.skips.entry((what, slot)).or_insert(0);
                if *skipped < budget {
                    *skipped += 1;
                    withheld = true;
                    continue;
                }
            }
            self.skips.remove(&(what, slot));
            self.send_bytes(slot, bytes.clone());
        }
        withheld
    }

    /// Drop every player's skip run for something that no longer exists.
    ///
    /// NPC indices and player slots are both small fixed ranges, so their entries are naturally
    /// bounded and get reused. A projectile identity is not: it carries a generation that keeps
    /// climbing, so without this a long-running server would hold one entry per player for every
    /// projectile ever fired.
    pub(super) fn forget_skips(&mut self, what: Withheld) {
        self.skips.retain(|&(which, _), _| which != what);
    }

    /// Carry out what a fighter decided to do to a door.
    /// Pull a door shut behind a resident who has walked through it.
    ///
    /// The close half of this was being dropped on the floor: a town NPC produced the action, the
    /// server matched it and did nothing, and `ai/town.rs` documented the behaviour as "opened and
    /// then closed behind it". So every door an NPC ever used stayed open on every client, and at
    /// night that is the difference between a sealed house and an invitation.
    ///
    /// `action: 1` is the game's close, against `0` for open (`MessageBuffer.cs:1310`).
    /// Open a door and tell clients, returning whether it actually moved. Moving the tiles rather
    /// than only broadcasting keeps the *server's* view of the door in step with everyone else's —
    /// broadcasting alone once left the server sure every door was shut, so an NPC at one opened it
    /// on every look, for ever (eighteen thousand door packets in five minutes on a real town). The
    /// bool lets a wired door try the other swing side when this one is blocked.
    fn open_door_broadcast(&mut self, x: i32, y: i32, direction: i8) -> bool {
        if !crate::world::doors::open(&mut self.world, x, y, direction) {
            return false;
        }
        let toggle = terrustia_proto::objects::DoorToggle {
            action: 0,
            x: x as i16,
            y: y as i16,
            direction: if direction > 0 { 1 } else { 0 },
        };
        if let Ok(frame) = toggle.encode() {
            self.broadcast(frame, None);
        }
        true
    }

    /// A door a circuit reached — `Wiring.cs`'s `type == 10`/`type == 11`. A shut door opens on a
    /// random swing side, falling back to the other if that one is blocked; an open door is forced
    /// shut. Resolved against the door's live state each time, so the per-tile toggling vanilla does
    /// when more than one of a door's three tiles carries wire falls out on its own.
    fn fire_wired_door(&mut self, x: i32, y: i32) {
        let tile = self.world.tile(x, y);
        if !tile.is_active() {
            return;
        }
        match tile.block {
            crate::world::doors::DOOR_CLOSED => {
                let side: i8 = if rand::Rng::random_range(&mut self.rng, 0..2) == 0 {
                    -1
                } else {
                    1
                };
                if !self.open_door_broadcast(x, y, side) {
                    self.open_door_broadcast(x, y, -side);
                }
            }
            crate::world::doors::DOOR_OPEN => self.close_door(x, y, true),
            _ => {}
        }
    }

    /// Every playing player's and every active NPC's hitbox, in world pixels as `(left, top, right,
    /// bottom)` — the boxes vanilla's `Collision.EmptyTile` tests a tile against, so an unforced
    /// door close can tell whether something is standing in the column it would shut on.
    pub(super) fn entity_hitboxes(&self) -> Vec<(f32, f32, f32, f32)> {
        let mut boxes = Vec::new();
        for player in self.players.iter().flatten() {
            if !player.is_playing() {
                continue;
            }
            let (px, py) = player.position;
            boxes.push((px, py, px + PLAYER_HALF_WIDTH * 2.0, py + PLAYER_HEIGHT));
        }
        for (_, npc) in self.npcs.iter() {
            let (nx, ny) = npc.position;
            boxes.push((nx, ny, nx + npc.width(), ny + npc.height()));
        }
        boxes
    }

    /// Shut a door, telling clients. Unless `forced`, refuses while a player or NPC is standing in
    /// the column the shut door lands on (`WorldGen.CloseDoor`'s own `Collision.EmptyTile` guard,
    /// `WorldGen.cs:32155`) — a resident pulling its door shut must not trap whoever is in the
    /// doorway. A wire signal forces it, as vanilla's `case 11` does.
    fn close_door(&mut self, x: i32, y: i32, forced: bool) {
        if !self.world.in_bounds(x, y) {
            return;
        }
        let occupants = if forced {
            Vec::new()
        } else {
            self.entity_hitboxes()
        };
        let moved = crate::world::doors::close_checked(&mut self.world, x, y, forced, |tx, ty| {
            let tile = (
                (tx * 16) as f32,
                (ty * 16) as f32,
                (tx * 16 + 16) as f32,
                (ty * 16 + 16) as f32,
            );
            occupants.iter().any(|&b| boxes_overlap(tile, b))
        });
        if !moved {
            return;
        }
        let toggle = terrustia_proto::objects::DoorToggle {
            action: 1,
            x: x as i16,
            y: y as i16,
            direction: 0,
        };
        if let Ok(frame) = toggle.encode() {
            self.broadcast(frame, None);
        }
    }

    /// A trapdoor a circuit reached — `Wiring.cs:1443-1456`. Tries `player_above: true` first and
    /// falls back to `false`, exactly the wire trigger's own two-attempt order
    /// (`WorldGen.ShiftTrapdoor(i, j, playerAbove: true)`, then `playerAbove: false` only if that
    /// failed) — there is no real player standing anywhere in this path; it is only which of the
    /// two possible landing rows the shift tries first. `shift_trapdoor` mutates nothing when it
    /// refuses, so retrying on the same still-unchanged tile is safe.
    fn fire_wired_trapdoor(&mut self, x: i32, y: i32) {
        use crate::world::trapdoors::{TRAPDOOR_CLOSED, TRAPDOOR_OPEN, shift_trapdoor};

        let before = self.world.tile(x, y);
        if !before.is_active() || !matches!(before.block, TRAPDOOR_CLOSED | TRAPDOOR_OPEN) {
            return;
        }
        let occupants = self.entity_hitboxes();
        let mut player_above = true;
        let mut moved = shift_trapdoor(&mut self.world, x, y, true, |tx, ty| {
            tile_occupied(tx, ty, &occupants)
        });
        if !moved {
            player_above = false;
            moved = shift_trapdoor(&mut self.world, x, y, false, |tx, ty| {
                tile_occupied(tx, ty, &occupants)
            });
        }
        if !moved {
            return;
        }
        // `3 - value.ToInt()` (`Wiring.cs:1453`) with `value = (type == TRAPDOOR_OPEN)`: closing
        // (the tile was open) sends action 2, opening sends 3 — see `DoorToggle::action`'s own
        // doc for why that reads backwards from the door/gate pair either side of it in source.
        let toggle = terrustia_proto::objects::DoorToggle {
            action: if before.block == TRAPDOOR_OPEN { 2 } else { 3 },
            x: x as i16,
            y: y as i16,
            direction: u8::from(player_above),
        };
        if let Ok(frame) = toggle.encode() {
            self.broadcast(frame, None);
        }
    }

    /// A tall gate a circuit reached — `Wiring.cs:1457-1463`. Unforced, so it still refuses while
    /// a player or NPC stands in the column, the same as a wired trapdoor's own opening refuses —
    /// real vanilla's own wire trigger never passes `forced` here either.
    fn fire_wired_gate(&mut self, x: i32, y: i32) {
        use crate::world::trapdoors::{TALL_GATE_CLOSED, TALL_GATE_OPEN, shift_tall_gate};

        let before = self.world.tile(x, y);
        if !before.is_active() || !matches!(before.block, TALL_GATE_CLOSED | TALL_GATE_OPEN) {
            return;
        }
        let closing = before.block == TALL_GATE_OPEN;
        let occupants = self.entity_hitboxes();
        let moved = shift_tall_gate(&mut self.world, x, y, closing, false, |tx, ty| {
            tile_occupied(tx, ty, &occupants)
        });
        if !moved {
            return;
        }
        // `4 + flag4.ToInt()` (`Wiring.cs:1461`) with `flag4 = (type == TALL_GATE_OPEN)`: closing
        // sends action 5, opening sends 4 — matching this project's own `DoorToggle::action` doc.
        let toggle = terrustia_proto::objects::DoorToggle {
            action: if closing { 5 } else { 4 },
            x: x as i16,
            y: y as i16,
            // Real vanilla's own `SendData` call for this case never passes a fourth number, so
            // the wire's direction byte defaults to 0 (`NetMessage.cs:544`,
            // `(number4 == 1f) ? 1 : 0` against the unset default).
            direction: 0,
        };
        if let Ok(frame) = toggle.encode() {
            self.broadcast(frame, None);
        }
    }

    fn apply_door_action(&mut self, action: crate::game::ai::fighter::Action) {
        use crate::game::ai::fighter::Action;
        match action {
            Action::None => {}
            Action::OpenDoor { x, y, direction } => {
                self.open_door_broadcast(x, y, direction);
            }
            Action::BreakDoor { x, y } => {
                // A broken door really is gone, so the tiles are cleared here and the change is
                // sent as an ordinary tile edit.
                for dy in -1..=1 {
                    let mut tile = self.world.tile(x, y + dy);
                    if !tile.is_active() {
                        continue;
                    }
                    tile.flags.set(TileFlags::ACTIVE, false);
                    tile.block = 0;
                    tile.frame_x = -1;
                    tile.frame_y = -1;
                    self.world.set_tile(x, y + dy, tile);
                    self.liquids.disturb(x, y + dy);

                    let edit = TileManipulation {
                        action: 0,
                        x: x as i16,
                        y: (y + dy) as i16,
                        arg: 0,
                        style: 0,
                    };
                    if let Ok(frame) = edit.encode() {
                        self.broadcast(frame, None);
                    }
                }
                info!(x, y, "a door was broken down");
            }
        }
    }

    /// Tell clients an NPC is gone. Zero health in packet 23 is what removes it.
    pub(super) fn broadcast_npc_death(&mut self, index: u8) {
        let sync = SyncNpc {
            index,
            generation: 0,
            position: (0.0, 0.0),
            velocity: (0.0, 0.0),
            target: 255,
            direction: 1,
            direction_y: 1,
            sprite_direction: 1,
            ai: [0.0; 4],
            net_id: 0,
            life: 0,
            life_max: 1,
            release_owner: 255,
        };
        if let Ok(frame) = sync.encode() {
            self.broadcast(frame, None);
        }
    }

    /// Everything that follows an NPC running out of life.
    ///
    /// Shared rather than inlined at the one hit path, because a hit is not the only way to die:
    /// a debuff can finish something off between two ticks, with no player credited, and every
    /// one of these still has to happen.
    pub(super) fn npc_died(&mut self, index: u8, npc_type: u16, center: (f32, f32), value: f32) {
        // The bound Purple Slime does not die, whatever killed it. It is the one town slime freed
        // by being beaten rather than talked to or powdered, so the death itself is the rescue.
        if self.free_the_purple_slime(index, npc_type) {
            return;
        }
        // Named rather than three positional bools, because they are all `false` most of the time
        // and swapping two of them would be silent.
        // Read before the removal takes it: `Conditions.IsBloodMoonAndNotFromStatue` cares whether
        // *this* NPC came from a statue, not just whether one exists somewhere in the world, and
        // `RedHatSkeletronAdjustmentsEnabled` (`NPC.cs:67435-67446`) reads `ai[3]` off this exact
        // instance, not off npc_type 35 in general — the ordinary boss and the Clothier's
        // repeatable vanity re-fight (`spawn_skeletron_from`'s own `red_hat` flag) share a type.
        let removed = self.npcs.remove(index);
        let from_statue = removed.as_ref().is_some_and(|npc| npc.from_statue);
        let red_hat_skeletron =
            npc_type == 35 && removed.as_ref().is_some_and(|npc| npc.ai[3] == 1.0);
        // The Terraprisma's only gate, and per-instance for the same reason:
        // `AI_120_HallowBoss_IsGenuinelyEnraged` (`NPC.cs:46321-46328`) asks *this* Empress whether
        // her fight was begun in daylight, which her own routine records in `ai[3]`.
        let empress_genuinely_enraged = npc_type == 636
            && removed
                .as_ref()
                .is_some_and(|npc| matches!(npc.ai[3] as i32, 2 | 3));
        // Midas (`NPC.cs:80448`) multiplies the coin drop; read off this exact NPC before it goes.
        let midas = removed.as_ref().is_some_and(|npc| npc.buffs.flags.midas);
        // `GetWereThereAnyInteractions()`, off this exact corpse before it goes, for the one death
        // event that asks it (`NPC.cs:80310`).
        let hit_by_player = removed.as_ref().is_some_and(|npc| npc.hit_by_player);
        // GOL-1: the Golem's head comes off when it dies and flies on its own
        // (`NPC.cs:85913-85918`, `HitEffect`'s `type == 246` branch). Nothing in production ever
        // spawned type 249, so the whole style-48 free-head routine was unreachable code. Read off
        // the corpse before it is gone: vanilla's `NewNPC` places it by its bottom centre, and it
        // inherits the head's link to the body, which every threshold in style 48 keys on.
        let free_head = removed
            .as_ref()
            .filter(|_| npc_type == terrustia_proto::npc_params::GOLEM_HEAD)
            .map(|head| {
                (
                    (head.center().0, head.position.1 + head.height()),
                    head.follows_boss,
                )
            });
        self.broadcast_npc_death(index);
        if let Some((bottom, body)) = free_head {
            self.free_the_golem_head(bottom, body);
        }
        // An expert boss's coins ride inside its treasure bag, so the bag's own drop zeroes the
        // NPC's money (`CommonCode.DropItemLocalPerClientAndSetNPCMoneyTo0` sets `npc.value = 0f`,
        // `CommonCode.cs:31`). Without this an expert boss paid out both the loose coins and the
        // bag that already holds them.
        let value = if self.is_expert()
            && terrustia_proto::conditional_drops::treasure_bag(npc_type).is_some()
        {
            0.0
        } else {
            value
        };
        self.drop_coins(value, center, midas);
        self.drop_loot(
            npc_type,
            center,
            DeadNpc {
                from_statue,
                red_hat_skeletron,
                empress_genuinely_enraged,
            },
        );
        self.note_invasion_kill(npc_type);
        self.army.note_corpse(npc_type, (center.0, center.1 + 16.0));
        self.note_army_kill(npc_type);
        self.note_moon_kill(npc_type);
        self.lunar.note_kill(npc_type);
        self.split_on_death(npc_type, center);
        self.wake_the_empress(npc_type, center, hit_by_player);
        self.note_banner_kill(npc_type, center);
        self.note_boss_kill(npc_type);
        self.note_slime_rain_kill(npc_type, center);
    }

    /// The Empress of Light's only summon: a Prismatic Lacewing killed by a player
    /// (`NPC.cs:80309-80319`, the `case 661` of `DoDeathEvents`).
    ///
    /// ```csharp
    /// case 661:
    ///     if (Main.netMode != 1 && GetWereThereAnyInteractions())
    ///     {
    ///         int num = 636;
    ///         if (!AnyNPCs(num))
    ///         {
    ///             Vector2 vector = base.Center + new Vector2(0f, -200f)
    ///                 + Main.rand.NextVector2Circular(50f, 50f);
    ///             SpawnBoss((int)vector.X, (int)vector.Y, num, closestPlayer.whoAmI);
    ///         }
    ///     }
    ///     break;
    /// ```
    ///
    /// There is no summon item, no altar and no other route: the Lacewing's death is the whole of
    /// it, which is why the missing spawn arm took the entire boss with it. *Catching* one is not a
    /// summon either (it has a `catchItem`, `NPC.cs:17397`) - only killing it counts, and only if a
    /// player did the killing, which is what [`Npc::hit_by_player`] answers.
    ///
    /// `NextVector2Circular(50f, 50f)` is `NextVector2Unit() * (50, 50) * NextFloat()`
    /// (`Utils.cs:1301-1304`): a random direction at a radius scaled linearly rather than by an
    /// area-uniform square root, so the scatter is denser near the middle. Transcribed as written.
    ///
    /// Two disclosed narrowings, both against `SpawnBoss` (`NPC.cs:81485-81520`) rather than against
    /// this case: `timeLeft *= 20` is not applied, because no boss-summon path in this server
    /// applies it, and the target player index is not carried onto the new NPC, because this
    /// server's routines pick their own target every tick. Neither changes who or where she is.
    fn wake_the_empress(&mut self, npc_type: u16, center: (f32, f32), hit_by_player: bool) {
        use rand::Rng;
        use terrustia_proto::npc_params::{EMPRESS_OF_LIGHT, PRISMATIC_LACEWING};

        if npc_type != PRISMATIC_LACEWING || !hit_by_player {
            return;
        }
        if self
            .npcs
            .iter()
            .any(|(_, n)| n.npc_type == EMPRESS_OF_LIGHT && n.is_alive())
        {
            return;
        }
        let angle = self.rng.random::<f32>() * std::f32::consts::TAU;
        let radius = self.rng.random::<f32>() * 50.0;
        let at = (
            center.0 + angle.cos() * radius,
            center.1 - 200.0 + angle.sin() * radius,
        );
        let Some(index) = self.spawn_at_bottom(EMPRESS_OF_LIGHT, at) else {
            return;
        };
        // The same keyed name every other boss-summon path builds: our own names are the game's
        // `NPCName.*` keys, so the two line up without a translation table.
        let name = self
            .npcs
            .get(index)
            .map(|n| n.stats.name)
            .unwrap_or("Something");
        let who = NetworkText::key(format!("NPCName.{name}"), Vec::new());
        self.announce_key("Announcement.HasAwoken", vec![who]);
        self.broadcast_npc(index);
        info!(x = at.0, y = at.1, "a Prismatic Lacewing woke the Empress");
    }

    /// A bound Purple Slime beaten to nothing, which is how that one is freed.
    ///
    /// `NPC.HitEffect` (`NPC.cs:82596-82627`), stripped of its gore:
    ///
    /// ```csharp
    /// if (type == 686 && life <= 0) {
    ///     ...
    ///     if (Main.netMode != 1) {
    ///         position = base.Bottom + new Vector2(0f, 48f);
    ///         Transform(680);
    ///         if (!unlockedSlimePurpleSpawn) {
    ///             unlockedSlimePurpleSpawn = true;
    ///             if (Main.netMode == 2) { NetMessage.SendData(7); }
    /// ```
    ///
    /// Placed at the head of [`Self::npc_died`] rather than at any one hit path, because vanilla
    /// runs `HitEffect` from `StrikeNPC` (`NPC.cs:82323`) before anything reaps the corpse, so
    /// *every* way of running it out of life frees it: a player's swing, a townsperson's, a
    /// debuff ticking between frames. There is no owner to credit and no loot to drop; the slime
    /// simply stops being bound. It is worth nothing (`value: 0.0` in the table), so the coins and
    /// the banner tally this skips would have been nothing anyway.
    ///
    /// `position = Bottom + (0, 48)` is transcribed exactly as vanilla writes it, quirk included:
    /// `Bottom` is `(position.X + width / 2, position.Y + height)` and the result is assigned to
    /// `position`, which is a top-left, so the freed slime lands half its own width to the right
    /// and 48 pixels below where the bound one was. Vanilla's own `Transform` then keeps the feet
    /// where they are (`NPC.cs:81919-81925`, `position.Y += height` around `SetDefaults`), which
    /// [`Npc::become_type`] does not do; the two sizes are 20 by 20 and 18 by 20, so the height is
    /// unchanged and only the two-pixel width differs.
    ///
    /// One disclosed divergence: vanilla's `Transform` carries life across proportionally
    /// (`life = num2 * lifeMax / num3`, clamped up to 1), so a Purple Slime freed at zero life
    /// arrives with exactly 1 of its 250 hit points and heals back through the town-NPC regen.
    /// [`Npc::become_type`] gives the new form full life, which is this project's existing
    /// behaviour for every transform it already has, and changing it here alone would make the
    /// Golfer and the Mechanic disagree with the slime for no reason.
    fn free_the_purple_slime(&mut self, index: u8, npc_type: u16) -> bool {
        if npc_type != crate::game::spawn::BOUND_TOWN_SLIME_PURPLE {
            return false;
        }
        let Some(npc) = self.npcs.get_mut(index) else {
            return false;
        };
        npc.position = (
            npc.position.0 + npc.width() / 2.0,
            npc.position.1 + npc.height() + 48.0,
        );
        npc.become_type(PURPLE_SLIME);
        npc.dirty = true;
        crate::game::rescues::remember(&mut self.world.progress, PURPLE_SLIME);
        self.broadcast_npc(index);
        self.broadcast_world_data();
        info!("a bound Purple Slime was beaten free and moved in");
        true
    }

    /// Put the Golem's freed head into the world at the dead head's bottom centre.
    ///
    /// `bottom` is vanilla's `(Center.X, position.Y + height)`, which `NewNPC` reads as a bottom
    /// centre; ours is a top-left, so the new head's own size comes back off it.
    fn free_the_golem_head(&mut self, bottom: (f32, f32), body: Option<u8>) {
        let npc_type = terrustia_proto::npc_params::GOLEM_HEAD_FREE;
        let Some(index) = self.spawn_at_bottom(npc_type, bottom) else {
            return;
        };
        if let Some(npc) = self.npcs.get_mut(index) {
            npc.follows_boss = body;
        }
        self.broadcast_npc(index);
    }

    /// The two lunar minions that leave something behind when they die.
    ///
    /// * The Stardust Cell (`NPC.cs:84381-84403`): a big cell (405) bursts into up to four small
    ///   ones (406), fewer the more of them are already about.
    /// * The Vortex Hornet Queen (`NPC.cs:83981-83994`): a queen (426) leaves three larvae (428)
    ///   behind, unless the swarm is already twenty strong.
    ///
    /// Neither child appears anywhere in vanilla's ambient spawning, so a split is the only way
    /// either one is ever seen at all. Neither counts toward a pillar's shield either: the game's
    /// own credit lists (`NPC.cs:80095-80136`, which [`crate::game::lunar::belongs_to`]
    /// transcribes) name 425-427 and 429 for the Vortex and 402/405/407/409/411 for the Stardust,
    /// and exclude both children.
    ///
    /// Both counts include the NPC that is dying. Vanilla runs `HitEffect` from `StrikeNPC`
    /// (`NPC.cs:82325`) while it is still `active`, so its own `CountNPCS` sees it; here
    /// [`Self::npc_died`] has already taken it out of the store, so it is added back by hand.
    fn split_on_death(&mut self, npc_type: u16, center: (f32, f32)) {
        use rand::Rng;

        let count = |server: &Self, ty: u16| {
            server
                .npcs
                .iter()
                .filter(|(_, n)| n.npc_type == ty && n.is_alive())
                .count()
        };
        let Some(parent) = terrustia_proto::npc_data::npc_stats(npc_type) else {
            return;
        };

        match npc_type {
            // `NPC.cs:84381-84403`.
            405 => {
                let about = count(self, 406) + count(self, 405) + 1;
                let children = match about {
                    0..=3 => 4,
                    4..=6 => 3,
                    7..=9 => 2,
                    _ => 1,
                };
                // `NewNPC(Center.X, Bottom.Y, 406)`.
                let from = (center.0, center.1 + parent.height as f32 / 2.0);
                for _ in 0..children {
                    // `Vector2.UnitY.RotatedByRandom(2pi) * (3f + rand.NextFloat() * 4f)`.
                    let angle = self.rng.random_range(0.0..std::f32::consts::TAU);
                    let speed = 3.0 + self.rng.random::<f32>() * 4.0;
                    self.spawn_split(406, from, (-angle.sin() * speed, angle.cos() * speed));
                }
            }
            // `NPC.cs:83981-83994`.
            426 => {
                let swarm = count(self, 428) + count(self, 427) + (count(self, 426) + 1) * 3;
                if swarm >= 20 {
                    return;
                }
                // `NewNPC(Center.X, Center.Y, 428)`, three of them.
                for _ in 0..3 {
                    // `-Vector2.UnitY.RotatedByRandom(2pi) * rand.Next(3, 6) - Vector2.UnitY * 2f`.
                    let angle = self.rng.random_range(0.0..std::f32::consts::TAU);
                    let speed = self.rng.random_range(3..6) as f32;
                    self.spawn_split(
                        428,
                        center,
                        (angle.sin() * speed, -angle.cos() * speed - 2.0),
                    );
                }
            }
            _ => {}
        }
    }

    /// One child of a split, thrown clear of where its parent died.
    fn spawn_split(&mut self, npc_type: u16, bottom: (f32, f32), velocity: (f32, f32)) {
        let Some(index) = self.spawn_at_bottom(npc_type, bottom) else {
            return;
        };
        if let Some(child) = self.npcs.get_mut(index) {
            child.velocity = velocity;
            child.dirty = true;
        }
        self.broadcast_npc(index);
    }

    /// `NewNPC`'s own placement: its argument is a bottom centre, while [`NpcStore::spawn`] takes a
    /// top-left, so the new NPC's own size comes back off it. Not broadcast, because both callers
    /// have a field to set on it first.
    fn spawn_at_bottom(&mut self, npc_type: u16, bottom: (f32, f32)) -> Option<u8> {
        let stats = terrustia_proto::npc_data::npc_stats(npc_type)?;
        self.npcs.spawn(
            npc_type,
            (
                bottom.0 - stats.width as f32 / 2.0,
                bottom.1 - stats.height as f32,
            ),
        )
    }

    /// `DoDeathEvents_AdvanceSlimeRain`. Advances the kill count while a rain is up and, once the
    /// threshold is reached, summons King Slime at the *closest* player to this kill
    /// (`SpawnOnPlayer(closestPlayer.whoAmI, 50)`, real vanilla's own choice — not a random one).
    fn note_slime_rain_kill(&mut self, npc_type: u16, center: (f32, f32)) {
        let king_slime_present = self
            .npcs
            .iter()
            .any(|(_, n)| n.npc_type == crate::game::slime_rain::KING_SLIME);
        let summon = self.slime_rain.note_kill(
            npc_type,
            king_slime_present,
            self.world.progress.downed_king_slime,
        );
        if !summon {
            return;
        }
        let closest = self
            .players
            .iter()
            .enumerate()
            .filter_map(|(slot, p)| p.as_ref().map(|p| (slot as u8, p)))
            .filter(|(_, p)| p.is_playing())
            .min_by(|(_, a), (_, b)| {
                let da = (a.position.0 - center.0).powi(2) + (a.position.1 - center.1).powi(2);
                let db = (b.position.0 - center.0).powi(2) + (b.position.1 - center.1).powi(2);
                da.total_cmp(&db)
            })
            .map(|(slot, _)| slot);
        if let Some(slot) = closest {
            self.summon_on_player(slot, crate::game::slime_rain::KING_SLIME);
        }
    }

    /// Drop whatever an NPC was carrying.
    ///
    /// Each chain — both the unconditional ones in `drop_flat_loot`, below, and the classic-only
    /// ones `conditional_chains` returns (Queen Bee's Hive Wand/Bee-armor, Skeletron's three
    /// weapons, King Slime's Slime Hook/Slime Gun) — is rolled in order and stops at the first
    /// success, which is what keeps a run of alternatives rare rather than giving independent
    /// chances at every one of them.
    ///
    /// On top of that come the drops that depend on the world rather than the thing that died: a
    /// treasure bag in expert, a trophy, and the hardmode materials that only exist once the wall
    /// has fallen.
    fn drop_loot(&mut self, npc_type: u16, center: (f32, f32), dead: DeadNpc) {
        let DeadNpc {
            from_statue,
            red_hat_skeletron,
            empress_genuinely_enraged,
        } = dead;
        let (tx, ty) = (
            (center.0 / crate::game::npc::TILE) as i32,
            (center.1 / crate::game::npc::TILE) as i32,
        );
        let ground = self.world.tile(tx, ty).block;
        let p = &self.world.progress;
        let at = terrustia_proto::conditional_drops::Conditions {
            expert: self.is_expert(),
            master: self.is_master(),
            world_is_crimson: self.world.crimson,
            hard_mode: p.hard_mode,
            downed_plantera: p.downed_plantera,
            in_hallow: matches!(
                terrustia_proto::convert::biome_of(ground),
                Some(terrustia_proto::convert::Biome::Hallow)
            ),
            in_corruption: matches!(
                terrustia_proto::convert::biome_of(ground),
                Some(terrustia_proto::convert::Biome::Corruption)
            ),
            in_crimson: matches!(
                terrustia_proto::convert::biome_of(ground),
                Some(terrustia_proto::convert::Biome::Crimson)
            ),
            underground: ty > i32::from(self.world.rock_layer),
            // The sibling has to be gone already, and the one that just died is still in the
            // roster at this point, so it is excluded by index rather than by type.
            other_twin_dead: !self
                .npcs
                .iter()
                .any(|(_, n)| matches!(n.npc_type, 125 | 126) && n.is_alive()),
            blood_moon: self.world.blood_moon,
            npc_from_statue: from_statue,
            eclipse: self.world.eclipse,
            downed_mech_any: p.downed_mech_any,
            downed_all_mech_bosses: p.downed_mech1 && p.downed_mech2 && p.downed_mech3,
            pumpkin_moon_wave: matches!(self.moon.moon, Some(crate::game::moons::Moon::Pumpkin))
                .then_some(self.moon.wave),
            red_hat_skeletron,
            empress_genuinely_enraged,
        };

        // Pools that give exactly one of their options.
        for pool in terrustia_proto::conditional_drops::one_from(npc_type, at) {
            let pick = pool[rand::Rng::random_range(&mut self.rng, 0..pool.len())];
            self.spawn_item(ItemStack::new(i32::from(pick), 1, 0), center);
            // Some picks bring a companion item along automatically — Golem's Stynger with its
            // own ammunition (`ItemDropDatabase.cs:654-656`). Unconditional once the pick lands:
            // real vanilla's own nested `OnSuccess` has no further gate of its own.
            self.drop_bundled_companion(pick, center);
        }
        // Moon Lord: two *distinct* items drawn from his own ten-weapon pool
        // (`FromOptionsWithoutRepeatsDropRule`) — empty for every other npc and in expert mode, so
        // this is a no-op there. Mirrors the game's own algorithm exactly: pick one index, then
        // pick a second uniformly from what remains, rather than drawing from `one_from`'s
        // independent-per-pool mechanism, which could otherwise repeat the same weapon.
        let moon_lord_pool = terrustia_proto::conditional_drops::moon_lord_weapons(npc_type, at);
        if moon_lord_pool.len() >= 2 {
            let first = rand::Rng::random_range(&mut self.rng, 0..moon_lord_pool.len());
            let mut second = rand::Rng::random_range(&mut self.rng, 0..moon_lord_pool.len() - 1);
            if second >= first {
                second += 1;
            }
            for &item in &[moon_lord_pool[first], moon_lord_pool[second]] {
                self.spawn_item(ItemStack::new(i32::from(item), 1, 0), center);
            }
        }
        // Chance-gated pools: roll the gate first, and only on success pick which option.
        for pool in terrustia_proto::conditional_drops::chance_pools(npc_type, at) {
            if pool.one_in > 1 && !rand::Rng::random_ratio(&mut self.rng, 1, pool.one_in) {
                continue;
            }
            let pick = pool.options[rand::Rng::random_range(&mut self.rng, 0..pool.options.len())];
            self.spawn_item(ItemStack::new(i32::from(pick), 1, 0), center);
            // Pumpking's Stake Launcher and Mourning Wood's own two weapons each bring their own
            // ammunition the same way Golem's Stynger does above — `bundled_with` is shared rather
            // than re-checked only from `one_from`'s own loop.
            self.drop_bundled_companion(pick, center);
        }
        let treasure_bag = terrustia_proto::conditional_drops::treasure_bag(npc_type);
        for rule in terrustia_proto::conditional_drops::conditional(npc_type, at) {
            // Almost every rule here is a plain 1-in-`one_in` roll, but a handful of real vanilla
            // rules (`CommonDrop`/`ByCondition`'s own `chanceNumerator`) roll `M`-in-`N` instead —
            // `rule.numerator` is `1` for everything but those, so this is exactly the old roll for
            // every rule that never needed the field.
            if rule.one_in > 1
                && !rand::Rng::random_ratio(&mut self.rng, rule.numerator, rule.one_in)
            {
                continue;
            }
            // The expert treasure bag is instanced, not shared: one for each interacting player,
            // sent only to them (`CommonCode.DropItemLocalPerClientAndSetNPCMoneyTo0`). The rest are
            // ordinary world drops everybody can see and race for.
            if Some(rule.item) == treasure_bag {
                self.drop_instanced_bag(i32::from(rule.item), center);
                continue;
            }
            let stack = if rule.max > rule.min {
                rand::Rng::random_range(&mut self.rng, rule.min..=rule.max)
            } else {
                rule.min
            };
            self.spawn_item(ItemStack::new(i32::from(rule.item), stack, 0), center);
        }
        // Fallback chains among the classic-only rolls (Queen Bee's Hive Wand/Bee-armor,
        // Skeletron's three weapons, King Slime's Slime Hook/Slime Gun): stop at the first link
        // that lands, the same break-on-first-success shape `drop_flat_loot` already has below —
        // these three cannot live in the flat table itself, which has no notion of expert/classic
        // mode at all (see `conditional_chains`'s own doc for why).
        for chain in terrustia_proto::conditional_drops::conditional_chains(npc_type, at) {
            for rule in chain {
                if rule.one_in > 1
                    && !rand::Rng::random_ratio(&mut self.rng, rule.numerator, rule.one_in)
                {
                    continue;
                }
                let stack = if rule.max > rule.min {
                    rand::Rng::random_range(&mut self.rng, rule.min..=rule.max)
                } else {
                    rule.min
                };
                self.spawn_item(ItemStack::new(i32::from(rule.item), stack, 0), center);
                break;
            }
        }
        self.drop_flat_loot(npc_type, center);
    }

    /// The item a `one_from`/`chance_pools` pick brings with it automatically, if any — Golem's
    /// Stynger with its own ammunition, Pumpking's Stake Launcher with its own, Mourning Wood's
    /// two weapons with theirs (`terrustia_proto::conditional_drops::bundled_with`'s own doc).
    /// Unconditional once the pick lands: real vanilla's own nested `OnSuccess` has no further
    /// gate of its own.
    fn drop_bundled_companion(&mut self, pick: u16, center: (f32, f32)) {
        if let Some((companion, min, max)) = terrustia_proto::conditional_drops::bundled_with(pick)
        {
            let stack = if max > min {
                rand::Rng::random_range(&mut self.rng, min..=max)
            } else {
                min
            };
            self.spawn_item(ItemStack::new(i32::from(companion), stack, 0), center);
        }
    }

    /// The unconditional table.
    fn drop_flat_loot(&mut self, npc_type: u16, center: (f32, f32)) {
        for chain in terrustia_proto::npc_drops::drops(npc_type) {
            for rule in *chain {
                if !rand::Rng::random_ratio(&mut self.rng, 1, rule.one_in) {
                    continue;
                }
                let stack = if rule.max > rule.min {
                    rand::Rng::random_range(&mut self.rng, rule.min..=rule.max)
                } else {
                    rule.min
                };
                self.spawn_item(ItemStack::new(i32::from(rule.item), stack, 0), center);
                break;
            }
        }
    }

    /// Scatter an NPC's coin value as item entities.
    ///
    /// Ported from `NPC.NPCLoot_DropMoney` (`NPC.cs:80436-80567`). The value is never paid at face:
    /// it is varied (a base -20%..+75% roll plus a cascade of rarer jackpot bonuses), doubled up to
    /// on a blood moon and raised by Midas, then peeled off largest-denomination-first into several
    /// scattered stacks rather than one. The old code paid the raw value as one tidy pile of coins.
    ///
    /// Two disclosed narrowings. The luck double-roll (`num2 = 2` when a `luck` check passes, keep
    /// the better or, for bad luck, worse of two rolls) is left out: the server does not track a
    /// player's luck, so luck reads as zero and the roll happens once, which is the exact `luck ==
    /// 0` behaviour. And the divide-into-more-stacks step is guarded to leave at least one coin of a
    /// denomination it has entered, where source lets platinum/gold/silver divide to zero: that is a
    /// latent spin loop and empty-item drop in source (only its copper branch guards it), unwanted
    /// on this server's packet path.
    fn drop_coins(&mut self, value: f32, center: (f32, f32), midas: bool) {
        if value <= 0.0 {
            return;
        }
        let mut num = value;
        if midas {
            num *= 1.0 + rand::Rng::random_range(&mut self.rng, 30..=50) as f32 * 0.01;
        }
        num *= 1.0 + rand::Rng::random_range(&mut self.rng, -20..=75) as f32 * 0.01;
        // The jackpot cascade: each rarer than the last, and each worth a little more.
        for (one_in, lo, hi) in [
            (2u32, 5i32, 10i32),
            (4, 10, 20),
            (8, 15, 30),
            (16, 20, 40),
            (32, 25, 50),
            (64, 50, 100),
        ] {
            if rand::Rng::random_ratio(&mut self.rng, 1, one_in) {
                num *= 1.0 + rand::Rng::random_range(&mut self.rng, lo..=hi) as f32 * 0.01;
            }
        }
        if self.world.blood_moon {
            num *= 1.0 + rand::Rng::random_range(&mut self.rng, 0..=100) as f32 * 0.01;
        }

        // Peel denominations off the top, scattering each into its own stack, high to low: copper
        // 71, silver 72, gold 73, platinum 74.
        while num as i64 > 0 {
            let (unit, item, second_div_max): (f32, i32, i32) = if num > 1_000_000.0 {
                (1_000_000.0, 74, 3)
            } else if num > 10_000.0 {
                (10_000.0, 73, 3)
            } else if num > 100.0 {
                (100.0, 72, 3)
            } else {
                (1.0, 71, 4)
            };
            let mut count = (num / unit) as i64;
            if count > 50 && rand::Rng::random_ratio(&mut self.rng, 1, 5) {
                count /= i64::from(rand::Rng::random_range(&mut self.rng, 1..=3));
            }
            if rand::Rng::random_ratio(&mut self.rng, 1, 5) {
                count /= i64::from(rand::Rng::random_range(&mut self.rng, 1..=second_div_max));
            }
            // Source only floors the copper branch at one; flooring every branch is what keeps the
            // loop from stalling when a divide zeroes a higher denomination it has already entered.
            let count = count.max(1);
            num -= unit * count as f32;
            // Platinum caps a stack at 999 in source; the others never approach the i16 limit but
            // are clamped for safety all the same.
            let mut left = count;
            while left > 0 {
                let stack = left.min(999).min(i64::from(i16::MAX)) as i16;
                left -= i64::from(stack);
                self.spawn_item(ItemStack::new(item, stack, 0), center);
            }
        }
    }

    /// Stream every live NPC to a client that has just finished loading the world.
    ///
    /// A `23` and then a `54` per NPC, which is exactly vanilla's own join loop
    /// (`MessageBuffer.cs:851-857`, case 8). The buff list went out only when it *changed*
    /// (`broadcast_npc_buffs`), so a player joining a world where something was already poisoned or
    /// ichor-covered was told nothing about it and computed its own armour penetration from an
    /// empty list. Vanilla sends the packet even when there is nothing on the NPC, and this does
    /// too: the empty list is what tells a client to clear whatever it had cached for that slot.
    pub(super) fn send_npcs(&mut self, slot: u8) -> terrustia_proto::Result<()> {
        let live: Vec<(u8, Vec<(u16, i32)>)> = self
            .npcs
            .iter()
            .map(|(index, npc)| {
                (
                    index,
                    npc.buffs.active().map(|s| (s.kind, s.time)).collect(),
                )
            })
            .collect();
        for (index, buffs) in live {
            if let Some(sync) = self.npc_sync(index) {
                self.send(slot, sync.encode()?);
                self.send(slot, packets::npc_buffs(index, buffs)?);
            }
        }
        Ok(())
    }

    /// Mend hurt townsfolk a point at a time, the way the game does (`NPC.CheckLifeRegen`,
    /// `NPC.cs:93622-93648`).
    ///
    /// A town NPC below full health builds a regen counter by one a tick, and heals a single point
    /// each time it passes 180, so a resident who took a beating in a blood moon recovers over a
    /// couple of minutes instead of standing at a sliver of health until they die or the world
    /// reloads. Two named residents mend faster, exactly as the game's own switch does: the Guide
    /// (`type 22`) adds five a tick and the Cyborg (`type 209`) nine (`NPC.cs:93631-93639`). The
    /// base step is one, matching vanilla's `int num = 1`. The Dryad's ward (`+10`) is not modelled,
    /// since this server has no dryad-ward buff. A heal marks the NPC dirty so the ordinary sync
    /// carries the new health out, as vanilla's `NetUpdateLowPriority` does.
    ///
    /// Vanilla runs `CheckLifeRegen` for every friendly NPC (`NPC.cs:91592-91598`), not only town
    /// NPCs; this pass narrows to town NPCs, the only friendly NPCs this server both hurts and keeps
    /// around, so the counter never runs for anything else.
    pub(super) fn tick_town_regen(&mut self) {
        for (_, npc) in self.npcs.iter_mut() {
            if !npc.stats.town_npc || !npc.is_alive() || npc.life >= npc.life_max {
                continue;
            }
            let step = 1 + match npc.npc_type {
                22 => 5,  // Guide
                209 => 9, // Cyborg
                _ => 0,
            };
            npc.friendly_regen += step;
            if npc.friendly_regen > 180 {
                npc.friendly_regen = 0;
                npc.life = (npc.life + 1).min(npc.life_max);
                npc.dirty = true;
            }
        }
    }

    /// Look for a free house near the players and move a town NPC into it.
    ///
    /// Vanilla gates each resident behind conditions that mostly read the players' inventories,
    /// which this server does not model. The Guide is the exception — it arrives as soon as there
    /// is somewhere to live — so that is the one moved in automatically; the rest are placed with
    /// `/spawn` and will take a house of their own.
    pub(super) fn tick_town_npcs(&mut self) {
        if !self.ticks.is_multiple_of(HOUSING_SCAN_INTERVAL) {
            return;
        }

        // House any resident that does not have one yet — but never the Old Man, the Travelling
        // Merchant or the Skeleton Merchant, none of whom ever seek a house in real vanilla
        // (`WorldGen.FindAnyHomelessTownNPC`'s own exclusion list, `nPC.type != 37 && != 453 &&
        // != 368`). Without this, a real, reproducible bug: the Old Man is a real, already-
        // homeless town NPC by design (he haunts the dungeon entrance, never moves in anywhere),
        // so any tick where he happens to be nearby when a freshly-built house first becomes
        // findable, he claims it ahead of whichever real newcomer that house was built for — found
        // live when `moonlord.rs`'s own Guide-house trigger lost this exact race to the Old Man,
        // 2-for-2, on real full runs.
        let homeless: Vec<u8> = self
            .npcs
            .iter()
            .filter(|(_, npc)| {
                npc.stats.town_npc
                    && npc.home.is_none()
                    && !matches!(
                        npc.npc_type,
                        OLD_MAN | TRAVELLING_MERCHANT | SKELETON_MERCHANT
                    )
            })
            .map(|(index, _)| index)
            .collect();

        // Who would arrive if there were somewhere to put them. Worked out before the house
        // search because that search is the expensive half and there is no point paying for it
        // when nobody is homeless and nobody is waiting.
        let guide_present = self.npcs.iter().any(|(_, n)| n.npc_type == GUIDE);
        let newcomer = if guide_present {
            self.next_arrival()
        } else {
            Some((GUIDE, "Guide"))
        };
        if homeless.is_empty() && newcomer.is_none() {
            return;
        }

        let Some(house) = self.find_free_house() else {
            return;
        };
        let (hx, hy) = house;

        if let Some(index) = homeless.first() {
            if let Some(npc) = self.npcs.get_mut(*index) {
                npc.home = Some(house);
                npc.position = (hx as f32 * 16.0, (hy - 3) as f32 * 16.0);
                npc.dirty = true;
            }
            let name = self
                .npcs
                .get(*index)
                .map(|n| n.stats.name)
                .unwrap_or("Someone");
            self.announce(&format!("{name} has moved in."));
            self.broadcast_npc(*index);
            // Where they now live, so every client's housing screen shows the room as taken
            // rather than as still empty.
            self.broadcast_npc_home(*index);
            return;
        }

        // Nobody is homeless, so the newcomer worked out above can take the house.
        let Some((npc_type, name)) = newcomer else {
            return;
        };

        if let Some(index) = self
            .npcs
            .spawn(npc_type, (hx as f32 * 16.0, (hy - 3) as f32 * 16.0))
        {
            if let Some(npc) = self.npcs.get_mut(index) {
                npc.home = Some(house);
            }
            self.announce(&format!("The {name} has moved in."));
            self.broadcast_npc(index);
            self.broadcast_npc_home(index);
        }
    }

    /// Copy the live townsfolk into the world, so a save records who actually lives here.
    ///
    /// The world file's NPC section used to be carried through untouched, which meant every
    /// resident was a session-long guest: their name was regenerated on the next start and their
    /// house forgotten.
    ///
    /// The Travelling Merchant is deliberately excluded: `WorldFile.SaveNPCs`'s own resident loop
    /// skips him by type (`nPC.type != 368`, `WorldFile.cs:1724`), because he is not a resident at
    /// all — he arrives and leaves on his own schedule, and saving him into section 4 would have
    /// him greet a reloaded world already standing in someone's yard rather than arriving properly
    /// the next time he is due.
    pub(super) fn record_town_npcs(&mut self) {
        let residents: Vec<crate::world::objects::TownNpc> = self
            .npcs
            .iter()
            .filter(|(_, npc)| {
                npc.stats.town_npc && npc.is_alive() && npc.npc_type != TRAVELLING_MERCHANT
            })
            .map(|(_, npc)| {
                let home = npc.home.unwrap_or((0, 0));
                crate::world::objects::TownNpc {
                    net_id: i32::from(npc.npc_type),
                    name: npc.given_name.clone(),
                    position: npc.position,
                    homeless: npc.home.is_none(),
                    home,
                    variation: npc.town_variation,
                    // Carried from the live NPC rather than hardcoded: this server has no
                    // despawn-timer routine that ever sets it, but a value a load decoded (a real
                    // vanilla world saved mid-eviction) must still round-trip rather than being
                    // silently reset to "not leaving" on this session's first save.
                    homeless_despawn: npc.homeless_despawn,
                }
            })
            .collect();
        self.world.town_npcs = residents;
    }

    /// Put a loaded world's townsfolk back into the roster.
    ///
    /// Called once at startup. Without it a world with a full town opened here empty, and the
    /// arrival logic would slowly re-invite everyone under new names.
    pub(super) fn restore_town_npcs(&mut self) {
        let saved = std::mem::take(&mut self.world.town_npcs);
        let mut restored = 0usize;
        for npc in &saved {
            let Ok(net_id) = i16::try_from(npc.net_id) else {
                continue;
            };
            let npc_type = terrustia_proto::npc_data::from_net_id(net_id);
            let Some(index) = self.npcs.spawn(npc_type, npc.position) else {
                break; // out of slots
            };
            if let Some(live) = self.npcs.get_mut(index) {
                live.given_name = npc.name.clone();
                live.town_variation = npc.variation;
                live.home = (!npc.homeless).then_some(npc.home);
                live.homeless_despawn = npc.homeless_despawn;
                live.dirty = true;
            }
            restored += 1;
        }
        self.world.town_npcs = saved;
        if restored > 0 {
            info!(residents = restored, "the town's residents are back");
        }
    }

    /// Copy the live Lunar Pillars into the world, matching `WorldFile.SaveNPCs`'s second loop
    /// (`WorldFile.cs:1745-1755`, gated on `NPCID.Sets.SavesAndLoads` - `NPCID.cs:4807`, which in
    /// this build's target version names only the four pillars).
    ///
    /// Without this, whatever a save carries in the second list is whatever was in the file when
    /// it was opened - typically nothing, since a freshly generated world has no pillars at all -
    /// and the next load's first `tick_lunar` reads "no pillar standing" against a `tower_active_*`
    /// flag that still says one is, and marks it defeated (`L3-02`). Called alongside
    /// [`Self::record_town_npcs`], right before every save.
    pub(super) fn record_lunar_pillars(&mut self) {
        self.world.saved_npcs = self
            .npcs
            .iter()
            .filter(|(_, npc)| {
                crate::game::lunar::PILLARS.contains(&npc.npc_type) && npc.is_alive()
            })
            .map(|(_, npc)| crate::world::objects::SavedNpc {
                net_id: i32::from(npc.npc_type),
                position: npc.position,
            })
            .collect();
    }

    /// Put a loaded world's standing Lunar Pillars back as live NPCs.
    ///
    /// Called once at startup, alongside [`Self::restore_town_npcs`] and before the first tick
    /// (`GameServer::run`) - the whole point is to have them standing before `tick_lunar` ever
    /// runs, or its own "a pillar that was active and is not standing has fallen" diff reads an
    /// empty roster as every tower having just been beaten.
    ///
    /// Vanilla's own second `SaveNPCs` loop carries no shield/AI state (only
    /// active/netID/position, `WorldFile.cs:1745-1755`), and this server's own event tracker
    /// (`self.lunar`, distinct from the world's `lunar_apocalypse_up`/`tower_active_*` flags)
    /// starts every session at `LunarState::default()`. Left there, the very first `tick_lunar`
    /// would see `self.lunar.up == false` while the pillars are about to come back standing,
    /// which both loses the sky's own memory of the fight being on, and, worse, means the branch
    /// that starts the Moon Lord's countdown once the last pillar falls never fires, because it
    /// is gated on `self.lunar.up`. So a restored pillar comes back at full shield strength (half
    /// once the Moon Lord has already been beaten once), exactly as
    /// [`Self::trigger_lunar_apocalypse`] would set it fresh.
    pub(super) fn restore_lunar_pillars(&mut self) {
        let saved = std::mem::take(&mut self.world.saved_npcs);
        if saved.is_empty() {
            self.world.saved_npcs = saved;
            return;
        }

        self.lunar.up = true;
        self.lunar.countdown = 0;
        let strength = if self.world.progress.downed_moon_lord {
            crate::game::lunar::SHIELD_STRENGTH / 2
        } else {
            crate::game::lunar::SHIELD_STRENGTH
        };
        self.lunar.shields = [strength; 4];

        let mut restored = 0usize;
        for npc in &saved {
            let Ok(npc_type) = u16::try_from(npc.net_id.max(0)) else {
                continue;
            };
            if !crate::game::lunar::PILLARS.contains(&npc_type) {
                // Not a type this build's `read_town_npcs` should ever have put here - the second
                // list is gated on `NPCID.Sets.SavesAndLoads`, which names only the four pillars -
                // but a hand-edited or future-version file is not this reader's to trust blindly.
                continue;
            }
            let Some(index) = self.npcs.spawn(npc_type, npc.position) else {
                break; // out of slots
            };
            if let Some(live) = self.npcs.get_mut(index) {
                live.shield = strength;
                live.dirty = true;
            }
            restored += 1;
        }
        self.world.saved_npcs = saved;
        if restored > 0 {
            info!(
                pillars = restored,
                "the lunar apocalypse's pillars are back"
            );
        } else {
            // Nothing usable came back. Do not leave the event flagged "up" with no pillars to
            // show for it, or the very first tick starts the Moon Lord countdown out of nowhere.
            self.lunar.up = false;
            self.lunar.shields = [0; 4];
        }
    }

    /// Copy the live Journey powers into the world, so a save records what a Journey world's
    /// players actually set (`L3-23`).
    ///
    /// Mirrors `self.journey`'s six `IPersistentPerWorldContent` fields onto `world.journey_*`
    /// (`wld_save::write_journey_powers` reads only the world's own fields, the same separation
    /// `record_town_npcs`/`world.town_npcs` uses), right before every save.
    pub(super) fn record_journey_powers(&mut self) {
        let j = &self.journey;
        let w = &mut self.world;
        w.journey_freeze_time = j.freeze_time;
        w.journey_freeze_rain = j.freeze_rain;
        w.journey_freeze_wind = j.freeze_wind;
        w.journey_stop_biome_spread = j.stop_biome_spread;
        w.journey_time_rate_slider = j.time_rate_slider;
        w.journey_difficulty_slider = j.difficulty_slider;
    }

    /// Put a loaded world's Journey powers back into `self.journey`.
    ///
    /// Called once at startup, alongside `restore_town_npcs`/`restore_lunar_pillars`. Without
    /// this the toggles and sliders `wld::read_journey_powers` decoded off the file never reach
    /// anywhere a routine (`tick_world_update`'s own Stop Biome Spread gate, the weather tick's
    /// freeze checks, `GameServer::tick`'s time-rate multiplier) actually reads.
    pub(super) fn restore_journey_powers(&mut self) {
        let w = &self.world;
        self.journey.freeze_time = w.journey_freeze_time;
        self.journey.freeze_rain = w.journey_freeze_rain;
        self.journey.freeze_wind = w.journey_freeze_wind;
        self.journey.stop_biome_spread = w.journey_stop_biome_spread;
        self.journey.time_rate_slider = w.journey_time_rate_slider;
        self.journey.difficulty_slider = w.journey_difficulty_slider;
    }

    /// Who is waiting to move in, given what the world has been through and what people carry.
    ///
    /// Only the Guide ever arrived before this, so a town was one house and one resident forever.
    /// The cost of that was not cosmetic: the Mechanic sells the only wire in the game, and the
    /// entire wiring system was therefore unreachable.
    fn next_arrival(&mut self) -> Option<(u16, &'static str)> {
        use crate::game::arrivals::{Town, ready};

        let mut coins: i64 = 0;
        let mut best_life = 0i32;
        let (mut has_explosives, mut has_gun, mut has_dye_material) = (false, false, false);
        for player in self.players.iter().flatten().filter(|p| p.is_playing()) {
            best_life = best_life.max(i32::from(player.life_max));
            for slot in player.inventory.values() {
                let (kind, stack) = (slot.item.id, i64::from(slot.item.stack));
                coins += match kind {
                    71 => stack,
                    72 => stack * 100,
                    73 => stack * 10_000,
                    74 => stack * 1_000_000,
                    _ => 0,
                };
                // The real vanilla triggers, transcribed and tested in `arrivals`. The previous
                // inline sets named a Wooden Sword as a gun and Orichalcum ore as dye material.
                has_explosives |= crate::game::arrivals::counts_as_explosive(kind);
                has_gun |= crate::game::arrivals::counts_as_gun(kind);
                has_dye_material |= crate::game::arrivals::counts_as_dye_material(kind);
            }
        }

        let residents = self
            .npcs
            .iter()
            .filter(|(_, n)| n.stats.town_npc && n.is_alive())
            .count();
        let here: std::collections::HashSet<u16> = self
            .npcs
            .iter()
            .filter(|(_, n)| n.is_alive())
            .map(|(_, n)| n.npc_type)
            .collect();

        let town = Town {
            progress: &self.world.progress,
            coins,
            best_life,
            has_explosives,
            has_gun,
            has_dye_material,
            residents,
            hard_mode: self.world.progress.hard_mode,
            // A genuine party (`BirthdayParty.GenuineParty`) is what a town Green Slime moves in
            // during; a manually forced party is not.
            party: self.party.genuine,
        };
        ready(town, &|kind| here.contains(&kind))
            .into_iter()
            .next()
            .map(|arrival| (arrival.npc_type, arrival.name))
    }

    /// Find a valid house near a player that no town NPC has claimed.
    /// Look for a room somebody could move into, around one player.
    ///
    /// One player, not all of them: the search is four hundred and twenty-five probes and each
    /// promising one is a flood fill, which is a few tenths of a millisecond. That is nothing once
    /// every five seconds — but it is *per player*, and thirty players would put the whole tick
    /// budget into a single tick. Taking them in turn caps the cost at one player's worth however
    /// many are on, and only means a house is found within a few scans rather than the first.
    fn find_free_house(&mut self) -> Option<(i32, i32)> {
        let playing: Vec<u8> = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.is_playing())
            .map(|p| p.slot)
            .collect();
        if playing.is_empty() {
            return None;
        }
        self.housing_turn = (self.housing_turn + 1) % playing.len();
        let whose = playing[self.housing_turn];

        let taken: Vec<(i32, i32)> = self.npcs.iter().filter_map(|(_, npc)| npc.home).collect();
        for player in self
            .players
            .iter()
            .flatten()
            .filter(|p| p.slot == whose && p.is_playing())
        {
            let (px, py) = (
                (player.position.0 / 16.0) as i32,
                (player.position.1 / 16.0) as i32,
            );
            // Probe a coarse grid around the player rather than every tile: a house is at least
            // sixty tiles, so a five-tile step cannot miss one.
            for dx in (-60..=60).step_by(5) {
                for dy in (-40..=40).step_by(5) {
                    let (x, y) = (px + dx, py + dy);
                    if let Ok(room) = housing::check_room(&self.world, x, y) {
                        let home = room.home_tile();
                        // Two residents never share a room.
                        if taken
                            .iter()
                            .any(|(tx, ty)| (tx - home.0).abs() < 20 && (ty - home.1).abs() < 20)
                        {
                            continue;
                        }
                        return Some(home);
                    }
                }
            }
        }
        None
    }

    /// Give a piece of furniture the tile entity that makes it work, and tell everybody.
    ///
    /// Called when the tile goes down rather than when a client asks, because for most kinds
    /// that is the only moment there is: the game's placement request does nothing for an item
    /// frame, a mannequin, a hat rack, a food platter, a logic sensor or a display jar.
    pub(super) fn add_tile_entity(
        &mut self,
        kind: terrustia_proto::tile_entity::EntityKind,
        x: i16,
        y: i16,
    ) {
        if self
            .world
            .tile_entities
            .iter()
            .any(|e| e.x == x && e.y == y)
        {
            return;
        }
        let id = self.world.next_tile_entity;
        self.world.next_tile_entity += 1;
        self.world
            .tile_entities
            .push(terrustia_proto::tile_entity::TileEntity::new(
                id, kind, x, y,
            ));
        self.share_tile_entity(id);
        debug!(x, y, ?kind, id, "tile entity created with its tile");
    }

    /// Tell everyone nearby what a tile entity now holds.
    ///
    /// Until this goes out an entity does not exist as far as a client is concerned. An item
    /// frame hangs empty, a mannequin stands bare, and a pylon is scenery you cannot travel to —
    /// which is what every one of them was before this was sent at all.
    /// To everyone rather than only to those nearby, which is what the game does at every one of
    /// its own call sites. It matters: a client keeps its copy of an entity after it has walked
    /// away, and a section is only re-sent when its *tiles* change — which filling an item frame
    /// does not do. Sending only to those in range would leave that client believing in the
    /// contents it saw last, permanently. There are a few hundred of these in a world and they
    /// change when somebody touches them, so the cost is nothing like an NPC sync's.
    pub(super) fn share_tile_entity(&mut self, id: i32) {
        let Some(entity) = self.world.tile_entities.iter().find(|e| e.id == id) else {
            return;
        };
        let is_pylon = entity.kind == terrustia_proto::tile_entity::EntityKind::TeleportationPylon;
        let where_it_is = (entity.x, entity.y);
        let Ok(frame) = terrustia_proto::tile_entity::share(entity) else {
            return;
        };
        self.broadcast(frame, None);

        // A pylon needs a second announcement: the tile-entity message puts it in the world, and
        // module 8 is what puts it on the travel map. Only the second one is what a player sees.
        if is_pylon
            && let Some(pylon) = self
                .pylons()
                .into_iter()
                .find(|p| (p.x, p.y) == where_it_is)
        {
            self.pylon_kinds.insert(where_it_is, pylon.kind);
            self.broadcast_pylon(net_module::PylonMessage::Added, pylon);
        }
    }

    /// Tell everyone a tile entity has gone.
    fn unshare_tile_entity(&mut self, id: i32) {
        // Read before the caller removes it, so a pylon can be taken off the travel map by the
        // same call that takes it out of the world.
        let pylon = self
            .world
            .tile_entities
            .iter()
            .find(|e| {
                e.id == id && e.kind == terrustia_proto::tile_entity::EntityKind::TeleportationPylon
            })
            .map(|e| (e.x, e.y));
        if let Ok(frame) = terrustia_proto::tile_entity::unshare(id) {
            self.broadcast(frame, None);
        }
        if let Some(at) = pylon {
            // The remembered network, not one read off a tile that is already gone.
            //
            // A miss falls back to 0, Forest, which the client will not match against the pylon it
            // actually has: the removal is silently ignored and the pylon stays on every travel
            // map for the rest of the session (see `pylon_kinds`' own doc). Three hand-written
            // insert sites uphold that invariant and nothing enforces it, so say so out loud
            // rather than letting a permanent failure be a quiet `unwrap_or`.
            let kind = self.pylon_kinds.remove(&at).unwrap_or_else(|| {
                warn!(
                    x = at.0,
                    y = at.1,
                    "no remembered network for a pylon being removed; announcing Forest, which \
                     the client will not match"
                );
                0
            });
            self.broadcast_pylon(
                net_module::PylonMessage::Removed,
                net_module::Pylon {
                    x: at.0,
                    y: at.1,
                    kind,
                },
            );
        }
    }

    /// One tick of the tile entities.
    ///
    /// Only the training dummy does anything: it puts an NPC out when somebody comes near and
    /// takes it away when they leave, which is the only way that NPC ever exists. The rest are
    /// storage and are only there to be remembered.
    pub(super) fn tick_tile_entities(&mut self) {
        use terrustia_proto::tile_entity::EntityKind;
        /// How far a dummy will notice you from.
        const DUMMY_REACH: f32 = 1600.0;
        const DUMMY_NPC: u16 = 488;

        if self.world.tile_entities.is_empty() {
            return;
        }
        let watchers: Vec<(f32, f32)> = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.is_playing())
            .map(|p| p.position)
            .collect();

        // An entity whose tile has gone is gone with it. Without this it is a ghost: it keeps its
        // spot reserved so nothing can be placed there again, and it goes on being ticked forever.
        let mut orphaned = Vec::new();
        for (at, entity) in self.world.tile_entities.iter().enumerate() {
            let tile = self.world.tile(i32::from(entity.x), i32::from(entity.y));
            if !tile.is_active() || tile.block != entity.kind.tile() {
                orphaned.push((at, entity.npc(), entity.id));
            }
        }
        for (at, npc, id) in orphaned.iter().rev() {
            if let Some(index) = npc {
                self.npcs.remove(*index);
                self.broadcast_npc_death(*index);
            }
            self.world.tile_entities.remove(*at);
            // Clients keep their own copy, so one that is not told goes on believing in an item
            // frame nobody can see or take from.
            self.unshare_tile_entity(*id);
        }

        let mut raise = Vec::new();
        let mut lower = Vec::new();
        for (at, entity) in self.world.tile_entities.iter().enumerate() {
            if entity.kind != EntityKind::TrainingDummy {
                continue;
            }
            let here = (
                f32::from(entity.x) * crate::game::npc::TILE,
                f32::from(entity.y) * crate::game::npc::TILE,
            );
            let watched = watchers
                .iter()
                .any(|p| (p.0 - here.0).abs() < DUMMY_REACH && (p.1 - here.1).abs() < DUMMY_REACH);
            // A dummy whose own tile has gone takes its NPC with it.
            let tile = self.world.tile(i32::from(entity.x), i32::from(entity.y));
            let planted = tile.is_active() && tile.block == entity.kind.tile();
            match entity.npc() {
                Some(index) if !watched || !planted => lower.push((at, index)),
                None if watched && planted => raise.push((at, here)),
                _ => {}
            }
        }

        for (at, index) in lower {
            self.npcs.remove(index);
            self.broadcast_npc_death(index);
            if let Some(entity) = self.world.tile_entities.get_mut(at) {
                entity.set_npc(None);
            }
        }
        for (at, here) in raise {
            // It stands on its own tile, and carries where it was planted in its ai so its routine
            // can tell whether it is still there.
            let Some(index) = self.npcs.spawn(DUMMY_NPC, (here.0 + 16.0, here.1 + 48.0)) else {
                continue;
            };
            if let Some(entity) = self.world.tile_entities.get(at)
                && let Some(dummy) = self.npcs.get_mut(index)
            {
                dummy.ai[0] = f32::from(entity.x);
                dummy.ai[1] = f32::from(entity.y);
            }
            if let Some(entity) = self.world.tile_entities.get_mut(at) {
                entity.set_npc(Some(index));
            }
            self.broadcast_npc(index);
        }
    }

    /// Everything a circuit set in motion, including whatever its logic gates went on to do.
    ///
    /// A gate does not act on the world itself: it works out its new state and then starts a
    /// circuit of its own, which can toggle further lamps and run further gates. That cascade is
    /// what makes wiring a machine rather than a switchboard, and it is run here to a ceiling,
    /// because a ring of gates would otherwise go round for ever.
    pub(super) fn apply_circuit(&mut self, fired: crate::world::wiring::Fired, from: (i32, i32)) {
        /// How many rounds of gates one circuit is allowed to set off.
        const MAX_CASCADE: usize = 64;

        let mut pending = vec![fired];
        let mut fired_gates: std::collections::HashSet<(i32, i32)> =
            std::collections::HashSet::new();
        let mut rounds = 0;

        while let Some(fired) = pending.pop() {
            if fired.truncated {
                warn!(
                    x = from.0,
                    y = from.1,
                    reached = fired.reached,
                    "circuit cut short"
                );
            }
            for (cx, cy) in fired.changed {
                self.broadcast_tile(cx, cy);
            }
            for (tx, ty) in fired.traps {
                self.fire_trap(tx, ty);
            }
            // Wired Explosives (tile 141): the flood leaves the tile standing (unlike a land
            // mine), so the caller kills it, resyncs it, and throws the explosion — `Wiring.cs`'s
            // own `case 141`.
            for (mx, my) in fired.mines {
                self.detonate_explosives(mx, my);
            }
            // A buried land mine (tile 210): the flood already cleared the tile and reported the
            // change (`ExplodeMine`'s `KillTile`, mirrored in `wiring.rs`), so all that remains is
            // the explosion projectile.
            for (mx, my) in fired.land_mines {
                self.detonate_land_mine(mx, my);
            }
            for (dx, dy) in fired.doors {
                self.fire_wired_door(dx, dy);
            }
            for (tx, ty) in fired.trapdoors {
                self.fire_wired_trapdoor(tx, ty);
            }
            for (gx, gy) in fired.gates {
                self.fire_wired_gate(gx, gy);
            }
            for (sx, sy) in fired.statues {
                self.run_statue(sx, sy);
            }
            for (bx, by) in fired.boulder_statues {
                self.fire_boulder_statue(bx, by);
            }
            for (cx, cy) in fired.cannons {
                self.fire_cannon(cx, cy);
            }
            for (sx, sy) in fired.snowball_launchers {
                self.fire_snowball_launcher(sx, sy);
            }
            // A sundial or moondial the current reached jumps the world clock, through the same
            // `skip_to` a player right-clicking one already goes through (packet 51, actions 3 and
            // 6, `dispatch.rs`'s own `on_misc_data`). Both dials on one circuit is not a
            // contradiction to resolve: vanilla runs them in the order the flood found them and the
            // last one wins, which is what doing them in this order does.
            if fired.sundial {
                self.skip_to(true);
            }
            if fired.moondial {
                self.skip_to(false);
            }
            // L3-05: each colour's teleporter pair is jumped separately, in the colour order the
            // flood collected them.
            for (a, b) in fired.teleport_pairs {
                self.run_teleporters(a, b);
            }
            // Pumps already moved their water per colour inside the flood; all that is left is to
            // re-settle and broadcast the cells that changed.
            self.broadcast_pump_changes(&fired.pump_changed);
            for (tx, ty) in fired.timers_started {
                self.running_timers.insert((tx, ty), TIMER_WINDOW);
            }
            for (tx, ty) in fired.timers_stopped {
                self.running_timers.remove(&(tx, ty));
            }
            // A pressed Detonator is scheduled to pop back up. Like vanilla's `CheckMech(anchor, 60)`
            // (`Wiring.cs:362`), an anchor already counting down is left alone rather than refreshed,
            // so a second click within the window does not extend it (L3-26).
            for anchor in fired.detonators {
                self.detonator_resets
                    .entry(anchor)
                    .or_insert(DETONATOR_WINDOW);
            }

            if rounds >= MAX_CASCADE {
                if !fired.lamps.is_empty() {
                    warn!(
                        x = from.0,
                        y = from.1,
                        "logic gates went round too many times"
                    );
                }
                continue;
            }
            rounds += 1;
            for (lx, ly) in fired.lamps {
                self.broadcast_tile(lx, ly);
                let result = {
                    let world = &mut self.world;
                    crate::world::wiring::check_logic_gate(
                        world,
                        lx,
                        ly,
                        &fired_gates,
                        &mut self.rng,
                    )
                };
                let Some(result) = result else { continue };
                self.broadcast_tile(result.at.0, result.at.1);
                if !result.fires {
                    continue;
                }
                fired_gates.insert(result.at);
                let onward = {
                    let world = &mut self.world;
                    crate::world::wiring::trip_wire(world, result.at.0, result.at.1)
                };
                pending.push(onward);
            }
        }
    }

    /// Tell everybody about one tile that changed.
    pub(super) fn broadcast_tile(&mut self, x: i32, y: i32) {
        let tile = self.world.tile(x, y);
        let square = TileSquare {
            x: x as i16,
            y: y as i16,
            width: 1,
            height: 1,
            change_type: 0,
            tiles: vec![tile],
        };
        self.broadcast_tile_square(&square, None);
    }

    /// Fire every running timer whose turn it is.
    ///
    /// A timer is the only thing in the wire table that starts a circuit with nobody touching it,
    /// and it is how most contraptions actually run: a farm, a lift, a light that blinks. Each one
    /// counts down from the same window the game uses and fires whenever the count is a multiple
    /// of its own period, which is what keeps two timers of the same kind in step.
    pub(super) fn tick_timers(&mut self) {
        use crate::world::wiring::{timer_is_running, timer_period};

        if self.running_timers.is_empty() {
            return;
        }
        let mut due = Vec::new();
        let mut gone = Vec::new();
        for (&(x, y), left) in &mut self.running_timers {
            let tile = self.world.tile(x, y);
            if !timer_is_running(tile) {
                gone.push((x, y));
                continue;
            }
            *left -= 1;
            if *left <= 0 || (*left).rem_euclid(timer_period(tile.frame_x)) == 0 {
                *left = TIMER_WINDOW;
                due.push((x, y));
            }
        }
        for at in gone {
            self.running_timers.remove(&at);
        }
        for (x, y) in due {
            let fired = {
                let world = &mut self.world;
                crate::world::wiring::trip_wire(world, x, y)
            };
            self.apply_circuit(fired, (x, y));
        }
    }

    /// Fire one trap the current reached, if it is not still cooling down.
    ///
    /// The cooldown is what separates a trap from a machine gun: a pressure plate a slime is
    /// sitting on is hit every tick, and without this every one of those hits would be a dart.
    fn fire_trap(&mut self, x: i32, y: i32) {
        /// The one trap projectile that is rationed by how many are already out.
        const SPIKY_BALL: u16 = 185;

        let tile = self.world.tile(x, y);
        let Some(shot) = crate::world::wiring::trap_shot(tile, x, y, &mut self.rng) else {
            return;
        };
        if self.mech_cooldown.contains_key(&shot.cools_at) {
            return;
        }
        // Spiky balls are also rationed by how many are already lying about, which is what stops
        // a held-down plate burying a corridor in them.
        if shot.projectile_type == SPIKY_BALL {
            let at = (shot.position.0, shot.position.1);
            let allowed = crate::world::wiring::spiky_ball_allowed(
                self.projectiles
                    .iter()
                    .filter(|(_, p)| p.projectile_type == SPIKY_BALL)
                    .map(|(_, p)| {
                        let c = p.center();
                        ((c.0 - at.0).powi(2) + (c.1 - at.1).powi(2)).sqrt()
                    }),
            );
            if !allowed {
                return;
            }
        }
        // The cooldown is taken whether or not a slot was free: a trap that could not fire for
        // want of a projectile slot has still gone off, and should not retry every tick.
        self.mech_cooldown.insert(shot.cools_at, shot.cooldown);
        if let Some(index) = self.projectiles.launch(
            shot.projectile_type,
            shot.position,
            shot.velocity,
            shot.damage,
            0,
        ) {
            self.broadcast_projectile(index);
        }
    }

    /// Wired Explosives (tile 141) going off: `Wiring.cs`'s `case 141` — kill the tile, tell
    /// clients, and throw the explosion projectile (108, 500 damage) from the tile's centre.
    fn detonate_explosives(&mut self, x: i32, y: i32) {
        self.world.set_tile(x, y, Tile::AIR);
        self.broadcast_tile(x, y);
        self.throw_mine_blast(x, y, 108, 500);
    }

    /// A buried land mine (tile 210) going off: `Wiring.cs`'s `ExplodeMine` — the tile is already
    /// gone (the flood cleared it), so this only throws the explosion projectile (164, 250 damage).
    fn detonate_land_mine(&mut self, x: i32, y: i32) {
        self.throw_mine_blast(x, y, 164, 250);
    }

    /// The projectile half both mine detonations share: a still (velocity zero) explosion thrown
    /// from the tile centre, exactly as `Projectile.NewProjectile(..., i*16+8, j*16+8, 0, 0, ...)`.
    fn throw_mine_blast(&mut self, x: i32, y: i32, projectile_type: u16, damage: i32) {
        let position = (x as f32 * 16.0 + 8.0, y as f32 * 16.0 + 8.0);
        if let Some(index) =
            self.projectiles
                .launch(projectile_type, position, (0.0, 0.0), damage, 0)
        {
            self.broadcast_projectile(index);
        }
    }

    /// Run one statue the current reached.
    ///
    /// The spawn point is the middle of the statue's base, which is why a slime statue on a
    /// platform drops its slime onto the platform rather than into the tile it is standing in.
    fn run_statue(&mut self, x: i32, y: i32) {
        use terrustia_proto::statues::{self, Statue};

        let tile = self.world.tile(x, y);
        let (style, _) = statues::style_at(tile.frame_x, tile.frame_y);
        let Some(what) = statues::statue(style) else {
            return;
        };
        if self.mech_cooldown.contains_key(&(x, y)) {
            return;
        }
        let base = ((x * 16 + 16) as f32, ((y + 3) * 16) as f32);

        match what {
            Statue::Npc {
                types,
                offset,
                needs_room,
            } => {
                let npc_type = types[self.rng.random_range(0..types.len())];
                if !self.statue_spawn_allowed(npc_type, base) {
                    // Still take the cooldown: the statue fired, it simply had nothing to give.
                    self.mech_cooldown.insert((x, y), what.cooldown());
                    return;
                }
                self.mech_cooldown.insert((x, y), what.cooldown());
                // Something wide needs the ground around the statue to be clear, or it would
                // appear inside a wall.
                if needs_room && self.solid_tiles(x - 2, x + 3, y, y + 2) {
                    return;
                }
                let at = (base.0 + offset.0 as f32, base.1 + offset.1 as f32);
                if let Some(index) = self.npcs.spawn(npc_type, at) {
                    // A statue's monster is worth nothing and does not count against the spawn
                    // budget, which is what makes a farm a farm rather than a way to stop the
                    // world spawning anything else.
                    if let Some(npc) = self.npcs.get_mut(index) {
                        npc.from_statue = true;
                    }
                    self.broadcast_npc(index);
                }
            }
            Statue::Item { item, offset_y } => {
                let at = (base.0, base.1 + offset_y as f32);
                let crowded = !statues::item_spawn_allowed(
                    self.items
                        .iter()
                        .filter(|(_, w)| w.item.id == item)
                        .map(|(_, w)| {
                            ((w.position.0 - at.0).powi(2) + (w.position.1 - at.1).powi(2)).sqrt()
                        }),
                );
                self.mech_cooldown.insert((x, y), what.cooldown());
                if crowded {
                    return;
                }
                self.spawn_item(ItemStack::new(item, 1, 0), at);
            }
            Statue::Lure { types } => {
                self.mech_cooldown.insert((x, y), what.cooldown());
                let candidates: Vec<u8> = self
                    .npcs
                    .iter()
                    .filter(|(_, n)| types.contains(&n.npc_type) && n.is_alive())
                    .map(|(index, _)| index)
                    .collect();
                if candidates.is_empty() {
                    return;
                }
                let index = candidates[self.rng.random_range(0..candidates.len())];
                if let Some(npc) = self.npcs.get_mut(index) {
                    npc.position = (base.0 - npc.width() / 2.0, base.1 - npc.height() - 1.0);
                    npc.velocity = (0.0, 0.0);
                }
                self.broadcast_npc(index);
            }
            Statue::Becomes { block } => {
                self.mech_cooldown.insert((x, y), what.cooldown());
                for dx in 0..2i32 {
                    for dy in 0..3i32 {
                        let mut tile = self.world.tile(x + dx, y + dy);
                        tile.block = block;
                        tile.frame_x = (dx * 18 + 216) as i16;
                        tile.frame_y = (dy * 18) as i16;
                        self.world.set_tile(x + dx, y + dy, tile);
                    }
                }
                let square = TileSquare {
                    x: x as i16,
                    y: y as i16,
                    width: 2,
                    height: 3,
                    change_type: 0,
                    tiles: (0..6)
                        .map(|i| self.world.tile(x + i % 2, y + i / 2))
                        .collect(),
                };
                self.broadcast_tile_square(&square, None);
            }
        }
    }

    /// Drop a boulder out of a Boulder Statue: `Wiring.cs:1998-2017`.
    ///
    /// Not part of [`Self::run_statue`]'s table, because tile 531 is not tile 105 and produces no
    /// NPC and no item. It is closer to a trap: one `CheckMech(anchor, 900)` on the statue's own
    /// anchor, then a still boulder (projectile 99, 70 damage, 10 knockback) dropped from the middle
    /// of its base, twenty-eight pixels down.
    fn fire_boulder_statue(&mut self, x: i32, y: i32) {
        /// `ProjectileID.Boulder`.
        const BOULDER: u16 = 99;
        /// `CheckMech(num90, num91, 900)` - fifteen seconds between boulders.
        const BOULDER_COOLDOWN: i32 = 900;

        if self.mech_cooldown.contains_key(&(x, y)) {
            return;
        }
        // Taken whether or not a projectile slot was free, exactly as `fire_trap` does and for the
        // same reason: the statue has gone off, and should not retry every tick.
        self.mech_cooldown.insert((x, y), BOULDER_COOLDOWN);
        let position = ((x + 1) as f32 * 16.0, y as f32 * 16.0 + 28.0);
        if let Some(index) = self
            .projectiles
            .launch(BOULDER, position, (0.0, 0.0), 70, 0)
        {
            self.broadcast_projectile(index);
        }
    }

    /// Fire one wired cannon: `Wiring.cs:1306-1342` for the gates, `WorldGen.ShootFromCannon`
    /// (`WorldGen.cs:51041-51156`) for the shot itself.
    ///
    /// Three gates, in vanilla's own order. The world-level lockout for this cannon's own kind comes
    /// first and returns outright; then the cannon's own `CheckMech` window on its anchor; then, for
    /// the Bunny Cannon alone, the population check that stops a wired one filling the world with
    /// explosive bunnies.
    fn fire_cannon(&mut self, x: i32, y: i32) {
        /// `Wiring.cs:1335` and `:1338`: what firing takes off every other cannon of that kind.
        const CANNON_LOCKOUT: i32 = 120;
        const BUNNY_LOCKOUT: i32 = 480;

        let anchor = self.world.tile(x, y);
        let variant = i32::from(anchor.frame_x) / 72;
        // Checked before `CheckMech`, and a plain early return: a cannon inside its world lockout
        // has not fired at all and takes no window of its own.
        match variant {
            0 if self.cannon_cooldown > 0 => return,
            1 if self.bunny_cannon_cooldown > 0 => return,
            _ => {}
        }
        let Some(shot) = crate::world::wiring::cannon_shot(anchor, x, y) else {
            return;
        };
        if self.mech_cooldown.contains_key(&shot.cools_at) {
            return;
        }
        self.mech_cooldown.insert(shot.cools_at, shot.cooldown);
        // `BunnyCannonCanFire` (`WorldGen.cs:51158-51199`) runs inside the shooting function, after
        // the window has already been taken, which is why it is checked here and not above.
        if variant == 1 && !self.bunny_cannon_can_fire() {
            return;
        }
        match variant {
            0 => self.cannon_cooldown = CANNON_LOCKOUT,
            1 => self.bunny_cannon_cooldown = BUNNY_LOCKOUT,
            _ => {}
        }
        if let Some(index) = self.projectiles.launch(
            shot.projectile_type,
            shot.position,
            shot.velocity,
            shot.damage,
            0,
        ) {
            if let Some(projectile) = self.projectiles.get_mut(index) {
                projectile.ai[0] = shot.ai[0];
                projectile.ai[1] = shot.ai[1];
            }
            self.broadcast_projectile(index);
        }
    }

    /// `WorldGen.BunnyCannonCanFire` (`WorldGen.cs:51158-51199`).
    ///
    /// Two ceilings at once, both counted over vanilla's own first hundred NPC slots: no more than
    /// four Explosive Bunnies (NPC 614 and projectile 281 together) may be about, and there must be
    /// at least one free slot left over once each live bunny projectile has claimed one. The odd
    /// shape is vanilla's: the free-slot count is decremented by the *projectiles*, because each one
    /// is about to become an NPC.
    fn bunny_cannon_can_fire(&self) -> bool {
        /// `NPCID.ExplosiveBunny`.
        const EXPLOSIVE_BUNNY: u16 = 614;
        /// `ProjectileID.ExplosiveBunny`.
        const BUNNY_SHOT: u16 = 281;
        /// Vanilla counts over `Main.npc[0..100]`, not the whole table.
        const SLOTS: u8 = 100;
        const MOST_BUNNIES: i32 = 4;

        let mut free = 0i32;
        let mut bunnies = 0i32;
        for slot in 0..SLOTS {
            match self.npcs.get(slot) {
                Some(npc) if npc.is_alive() => {
                    if npc.npc_type == EXPLOSIVE_BUNNY {
                        bunnies += 1;
                        if bunnies >= MOST_BUNNIES {
                            return false;
                        }
                    }
                }
                _ => free += 1,
            }
        }
        for _ in self
            .projectiles
            .iter()
            .filter(|(_, p)| p.projectile_type == BUNNY_SHOT)
        {
            bunnies += 1;
            if bunnies >= MOST_BUNNIES {
                return false;
            }
            free -= 1;
            if free <= 0 {
                return false;
            }
        }
        free >= 1
    }

    /// Fire one wired Snowball Launcher (`Wiring.cs:1391-1417`).
    ///
    /// Two gates like the cannon's, but the world-level one is checked as `== 0` rather than `> 0`
    /// and sits *alongside* the `CheckMech` rather than in front of it, so a launcher inside its
    /// fifteen-frame lockout takes no window either.
    fn fire_snowball_launcher(&mut self, x: i32, y: i32) {
        /// `Wiring.cs:1393`.
        const SNOWBALL_LOCKOUT: i32 = 15;

        if self.snowball_cannon_cooldown != 0 {
            return;
        }
        if self.mech_cooldown.contains_key(&(x, y)) {
            return;
        }
        let anchor = self.world.tile(x, y);
        let shot = crate::world::wiring::snowball_shot(anchor, x, y, &mut self.rng);
        self.mech_cooldown.insert(shot.cools_at, shot.cooldown);
        self.snowball_cannon_cooldown = SNOWBALL_LOCKOUT;
        if let Some(index) = self.projectiles.launch(
            shot.projectile_type,
            shot.position,
            shot.velocity,
            shot.damage,
            0,
        ) {
            self.broadcast_projectile(index);
        }
    }

    /// Swap everything standing on one teleporter with everything standing on the other.
    ///
    /// It is a swap rather than a one-way trip: whatever is on each pad moves by the vector to
    /// the other, so two players on opposite pads change places in one pull.
    fn run_teleporters(&mut self, a: (i32, i32), b: (i32, i32)) {
        use crate::game::ai::{PLAYER_HEIGHT, PLAYER_WIDTH};
        use crate::world::wiring::{TELEPORTER_BOX, teleport_pair_is_useful};

        /// Whether a box overlaps a teleporter's catchment.
        fn overlaps(pad: (f32, f32, f32, f32), at: (f32, f32), size: (f32, f32)) -> bool {
            at.0 < pad.0 + pad.2
                && at.0 + size.0 > pad.0
                && at.1 < pad.1 + pad.3
                && at.1 + size.1 > pad.1
        }

        if !teleport_pair_is_useful(a, b) {
            return;
        }
        // The catchment reaches up from the teleporter's own row, which is why standing on one
        // works and walking past one at head height does not.
        let box_of = |at: (i32, i32)| {
            (
                (at.0 * 16) as f32,
                (at.1 * 16) as f32 - TELEPORTER_BOX,
                TELEPORTER_BOX,
                TELEPORTER_BOX,
            )
        };
        let (pad_a, pad_b) = (box_of(a), box_of(b));
        let hop = (pad_b.0 - pad_a.0, pad_b.1 - pad_a.1);

        // Both directions are worked out before anything moves, so a player who has just arrived
        // on the far pad is not sent straight back.
        let mut moves: Vec<(u8, (f32, f32))> = Vec::new();
        let mut npc_moves: Vec<(u8, (f32, f32))> = Vec::new();
        for (pad, shift) in [(pad_a, hop), (pad_b, (-hop.0, -hop.1))] {
            for slot in 0..self.players.len() {
                let Some(player) = self.players[slot].as_ref() else {
                    continue;
                };
                if !player.is_playing() || player.life <= 0 {
                    continue;
                }
                let slot = player.slot;
                if moves.iter().any(|(s, _)| *s == slot) {
                    continue;
                }
                if overlaps(
                    pad,
                    player.position,
                    (PLAYER_WIDTH as f32, PLAYER_HEIGHT as f32),
                ) {
                    moves.push((
                        slot,
                        (player.position.0 + shift.0, player.position.1 + shift.1),
                    ));
                }
            }
            let riders: Vec<(u8, (f32, f32))> = self
                .npcs
                .iter()
                .filter(|(index, n)| {
                    n.is_alive()
                        && n.life_max > 5
                        && !n.stats.boss
                        && !n.no_tile_collide
                        && !npc_moves.iter().any(|(i, _)| i == index)
                        && overlaps(pad, n.position, (n.width(), n.height()))
                })
                .map(|(index, n)| (index, (n.position.0 + shift.0, n.position.1 + shift.1)))
                .collect();
            npc_moves.extend(riders);
        }

        for (slot, to) in moves {
            if let Some(player) = self.player_mut(slot) {
                player.position = to;
                player.velocity = (0.0, 0.0);
            }
            let mut w = terrustia_proto::PacketWriter::new(id::TELEPORT_ENTITY);
            // Flags zero: a player, moving to a given place, with no extra field.
            w.u8(0).i16(i16::from(slot)).f32(to.0).f32(to.1).u8(0);
            if let Ok(frame) = w.finish() {
                self.broadcast(frame, None);
            }
        }
        for (index, to) in npc_moves {
            if let Some(npc) = self.npcs.get_mut(index) {
                npc.position = to;
                npc.velocity = (0.0, 0.0);
            }
            self.broadcast_npc(index);
        }
    }

    /// Re-settle and broadcast the cells a per-colour pump transfer moved liquid on. The transfer
    /// itself already ran inside the flood (L3-05, `run_from`'s own [`transfer_liquid`] call), so
    /// this only wakes the liquid sim and tells clients.
    fn broadcast_pump_changes(&mut self, changed: &[(i32, i32)]) {
        for &(x, y) in changed {
            // The moved liquid has to settle from where it landed, or it would sit in a column of
            // its own until something else disturbed it.
            self.liquids.disturb(x, y);
            let tile = self.world.tile(x, y);
            let square = TileSquare {
                x: x as i16,
                y: y as i16,
                width: 1,
                height: 1,
                change_type: 0,
                tiles: vec![tile],
            };
            self.broadcast_tile_square(&square, None);
        }
    }

    /// Whether a statue may add another of this type, given how crowded it already is.
    fn statue_spawn_allowed(&self, npc_type: u16, at: (f32, f32)) -> bool {
        terrustia_proto::statues::spawn_allowed(
            self.npcs
                .iter()
                .filter(|(_, n)| {
                    n.is_alive() && terrustia_proto::statues::same_family(npc_type, n.npc_type)
                })
                .map(|(_, n)| {
                    ((n.position.0 - at.0).powi(2) + (n.position.1 - at.1).powi(2)).sqrt()
                }),
        )
    }

    /// Whether any tile in this rectangle is solid.
    fn solid_tiles(&self, from_x: i32, to_x: i32, from_y: i32, to_y: i32) -> bool {
        (from_x..=to_x).any(|x| {
            (from_y..=to_y).any(|y| {
                let tile = self.world.tile(x, y);
                tile.is_active() && terrustia_proto::tile_solid::solid(tile.block)
            })
        })
    }

    /// Count down every trap that has fired recently, and forget the ones that are ready again.
    pub(super) fn tick_mech_cooldowns(&mut self) {
        self.mech_cooldown.retain(|_, ticks| {
            *ticks -= 1;
            *ticks > 0
        });
        // The three world-level cannon lockouts wind down beside the per-tile table, exactly as
        // `UpdateMech` winds them down beside its own (`Wiring.cs:147-158`).
        for lockout in [
            &mut self.cannon_cooldown,
            &mut self.bunny_cannon_cooldown,
            &mut self.snowball_cannon_cooldown,
        ] {
            if *lockout > 0 {
                *lockout -= 1;
            }
        }
    }

    /// Count down every pressed Detonator and pop the ones whose window has run out back up.
    ///
    /// `UpdateMech`'s own type-411 branch (`Wiring.cs:212-244`): when the sixty-frame window a click
    /// registered runs out, the two-by-two frame is shifted back and the change broadcast, and the
    /// entry forgotten. Kept off the trap cooldown map because, unlike a trap, expiry here *does*
    /// something (L3-26). The pressed anchors are collected first so the world can be mutated without
    /// borrowing the map across the loop.
    pub(super) fn tick_detonators(&mut self) {
        if self.detonator_resets.is_empty() {
            return;
        }
        let mut due = Vec::new();
        self.detonator_resets.retain(|&anchor, ticks| {
            *ticks -= 1;
            if *ticks <= 0 {
                due.push(anchor);
                false
            } else {
                true
            }
        });
        for anchor in due {
            let changed = crate::world::wiring::reset_detonator(&mut self.world, anchor);
            for (cx, cy) in changed {
                self.broadcast_tile(cx, cy);
            }
        }
    }

    /// One tick of settling liquid.
    ///
    /// Nothing happens unless something has been disturbed, so a world nobody is digging in costs
    /// nothing here. What moves is sent as tile squares, batched by row, because a flowing pool
    /// changes a run of neighbours at once and one packet each would be a flood of its own.
    pub(super) fn tick_liquids(&mut self) {
        // `Liquid.skipCount` (`WorldGen.cs:72072-72079`): the liquid sim runs only every second
        // tick. `skipCount` counts up and the sim runs (and the count resets) when it passes one,
        // so a settle takes twice as many real ticks — half of the L3-09 slowdown, the per-tile
        // `skipLiquid` flag inside the sim being the other half.
        self.liquid_skip_count += 1;
        if self.liquid_skip_count <= 1 {
            return;
        }
        self.liquid_skip_count = 0;

        if self.liquids.pending() == 0 {
            return;
        }
        // `UpdateLiquid` recomputes its per-pass slice from the player count every pass
        // (`Liquid.cs:993-1012`), so liquid takes less of the frame the busier the server is. See
        // `liquid::budget_for`. Vanilla's own loop only counts the first fifteen slots, and
        // `budget_for` saturates there, so a full server is not a special case.
        let playing = self
            .players
            .iter()
            .filter(|p| p.as_ref().is_some_and(|p| p.is_playing()))
            .count();
        self.liquids.set_player_count(playing);
        let settled = {
            let world = &mut self.world;
            self.liquids.tick(world)
        };
        self.kill_drowned_furniture(&settled.drowned);
        let mut touched: Vec<(i32, i32)> = settled.changed;
        touched.extend(settled.reacted.iter().map(|(x, y, _)| (*x, *y)));
        if touched.is_empty() {
            return;
        }
        touched.sort_unstable();
        touched.dedup();

        // Net module 0, not tile squares. This is the message the client expects for water moving,
        // and it costs six bytes a tile against a square's per-tile flag chain plus a header —
        // which matters because a settling pool dirties a whole stripe of neighbours every tick,
        // and this used to be a flood of its own.
        let changes: Vec<net_module::LiquidChange> = touched
            .iter()
            .map(|&(x, y)| {
                let tile = self.world.tile(x, y);
                net_module::LiquidChange {
                    x,
                    y,
                    amount: tile.liquid,
                    kind: tile.liquid_kind.as_type_byte(),
                }
            })
            .collect();

        // Split rather than truncate: the count is a `u16` and the frame has a size limit, so a
        // large enough disturbance has to go out as several frames or the tail is simply lost.
        for batch in changes.chunks(net_module::MAX_LIQUID_CHANGES) {
            if let Ok(frame) = net_module::liquid_changes(batch) {
                self.broadcast(frame, None);
            }
        }
    }

    /// Kill whatever furniture a liquid just reached, the tiles [`crate::world::liquid::Settled::drowned`]
    /// reports.
    ///
    /// `Liquid.AddWater`'s own inline check (`Liquid.cs:1196-1215`): a lava or water tile
    /// (honey included — the real check is `if (tile.lava()) CheckLavaDeath else
    /// CheckWaterDeath`, with no separate branch for honey or shimmer) spreading onto an active
    /// tile kills it outright when the generated `tile_death::LAVA_DEATH`/`WATER_DEATH` table says
    /// so, via a bare `WorldGen.KillTile(x, y)` — items still drop (`noItem` defaults `false`),
    /// and the kill is told to every client the same way a player's own break is
    /// (`NetMessage.SendData(17, -1, -1, null, 0, x, y)`, packet 17 with no exclusion).
    fn kill_drowned_furniture(&mut self, drowned: &[(i32, i32)]) {
        for &(x, y) in drowned {
            let tile = self.world.tile(x, y);
            if !tile.is_active() {
                continue;
            }
            let table = if tile.liquid_kind == terrustia_proto::tile::Liquid::Lava {
                &terrustia_proto::tile_death::LAVA_DEATH
            } else {
                &terrustia_proto::tile_death::WATER_DEATH
            };
            if !table.get(tile.block as usize).copied().unwrap_or(false) {
                continue;
            }
            let (block, frame_x, frame_y) = (tile.block, tile.frame_x, tile.frame_y);
            let mut cleared = tile;
            cleared.flags.set(TileFlags::ACTIVE, false);
            cleared.block = 0;
            cleared.frame_x = -1;
            cleared.frame_y = -1;
            cleared.slope = 0;
            cleared.flags.set(TileFlags::HALF_BRICK, false);
            self.world.set_tile(x, y, cleared);
            self.spawn_tile_drop(block, frame_x, frame_y, x, y);
            // Plantera's bulb has no crafted summon: breaking it, by whatever means, is the
            // summon (`wake_from_tile`'s own doc comment) — the one tile in either table this
            // project's own machinery gates on a boss, so it is the one special case carried over
            // from the player-edit path's own `broke` handling (`on_tile_manipulation`).
            // `BULB` (238) is `LAVA_DEATH`-true and `WATER_DEATH`-false, verified against the
            // generated table directly, so this is only ever reached on the lava branch above.
            if block == crate::world::bulbs::BULB {
                self.wake_from_tile(x, y, PLANTERA);
                if !self.world.progress.downed_plantera {
                    self.grow_plantera_bulb();
                }
            }
            let edit = TileManipulation {
                action: 0,
                x: x as i16,
                y: y as i16,
                arg: 0,
                style: 0,
            };
            if let Ok(frame) = edit.encode() {
                self.broadcast(frame, None);
            }
        }
    }

    /// One tick of the wind and the rain.
    ///
    /// Both are gated on somebody in the world having found a life crystal, which is the game's
    /// own way of keeping a brand-new world's weather quiet.
    pub(super) fn tick_weather(&mut self) {
        let strong_enough = self
            .players
            .iter()
            .flatten()
            .any(|p| p.is_playing() && p.life_max >= 120);
        let was_raining = self.weather.raining;
        let hard_mode = self.world.progress.hard_mode;
        let sky = crate::game::weather::Sky {
            lantern_night: self.lantern_night.is_up(),
            next_night_is_lantern_night: self.lantern_night.next_night_guaranteed,
            slime_rain: self.slime_rain.is_active(),
            num_clouds: u16::from(self.world.num_clouds),
            day_time: self.world.day_time,
            time: self.world.time,
        };
        let clouds = self.weather.tick(
            strong_enough,
            hard_mode,
            self.journey.freeze_wind,
            self.journey.freeze_rain,
            sky,
            &mut self.rng,
        );
        // The world carries the weather so it goes into the save with everything else.
        self.world.wind = self.weather.wind;
        self.world.raining = self.weather.raining;
        self.world.rain_time = self.weather.rain_time;
        self.world.max_rain = self.weather.max_rain;
        self.world.sandstorm = self.weather.sandstorm;
        self.world.sandstorm_time = self.weather.sandstorm_time;
        self.world.sandstorm_severity = self.weather.severity;
        self.world.sandstorm_intended_severity = self.weather.intended_severity;
        // The cloud count moves on its own timetable (`Main.cs:59939`, republished every 3,600 to
        // 10,800 ticks) and is part of world data, so a change goes out to clients the same way the
        // rain starting does. `NetMessage.SendData(7)` is exactly what vanilla sends here.
        let clouds = clouds.min(u16::from(u8::MAX)) as u8;
        let clouds_changed = clouds != self.world.num_clouds;
        self.world.num_clouds = clouds;
        if was_raining != self.weather.raining {
            self.announce(if self.weather.raining {
                "It has started to rain."
            } else {
                "The rain has stopped."
            });
            self.broadcast_world_data();
        } else if clouds_changed {
            self.broadcast_world_data();
        }
    }

    /// Where each pillar that is still standing is, in [`crate::game::lunar::PILLARS`] order.
    ///
    /// What a real client's `SceneMetrics.ScanNPCPositions` keeps (`SceneMetrics.cs:734-751`), and
    /// the only thing the spawn path needs in order to know it is inside a tower zone. Gathered
    /// once a tick, and only while the event is up: with no apocalypse running this is four
    /// `None`s and no pass over the NPC store at all.
    pub(super) fn standing_pillars(&self) -> [Option<(f32, f32)>; 4] {
        let mut at = [None; 4];
        if !self.lunar.up {
            return at;
        }
        for (_, npc) in self.npcs.iter() {
            if !npc.is_alive() {
                continue;
            }
            if let Some(slot) = crate::game::lunar::PILLARS
                .iter()
                .position(|p| *p == npc.npc_type)
            {
                at[slot] = Some(npc.center());
            }
        }
        at
    }

    /// One tick of the Lunar Apocalypse: the pillars' shields, and the minute after the last one.
    pub(super) fn tick_lunar(&mut self) {
        use crate::game::lunar::{MOON_LORD, PILLARS};
        let standing = self
            .npcs
            .iter()
            .filter(|(_, n)| PILLARS.contains(&n.npc_type))
            .count();
        let here = self.npcs.iter().any(|(_, n)| n.npc_type == MOON_LORD);
        let was_up = self.lunar.up;
        if self.lunar.tick(standing, here) {
            self.summon_moon_lord();
        }
        if was_up && !self.lunar.up {
            self.announce("Impending doom approaches...");
            info!("the last pillar has fallen");
        }
        // The world remembers which pillars are standing, so a save mid-apocalypse comes back to
        // the same fight rather than an empty sky.
        let standing_now = |ty: u16| self.npcs.iter().any(|(_, n)| n.npc_type == ty);
        let towers = (
            standing_now(crate::game::lunar::SOLAR),
            standing_now(crate::game::lunar::VORTEX),
            standing_now(crate::game::lunar::NEBULA),
            standing_now(crate::game::lunar::STARDUST),
        );
        let p = &mut self.world.progress;
        p.lunar_apocalypse_up = self.lunar.up;
        // A pillar that was standing and is not any more has been beaten, and that is permanent.
        p.downed_tower_solar |= p.tower_active_solar && !towers.0;
        p.downed_tower_vortex |= p.tower_active_vortex && !towers.1;
        p.downed_tower_nebula |= p.tower_active_nebula && !towers.2;
        p.downed_tower_stardust |= p.tower_active_stardust && !towers.3;
        (
            p.tower_active_solar,
            p.tower_active_vortex,
            p.tower_active_nebula,
            p.tower_active_stardust,
        ) = towers;
        // Each pillar carries its own shield on itself, so its routine can read it without
        // knowing the event exists.
        if self.lunar.up {
            let shields: Vec<(u8, i32)> = self
                .npcs
                .iter()
                .filter(|(_, n)| PILLARS.contains(&n.npc_type))
                .map(|(index, n)| (index, self.lunar.shield_of(n.npc_type)))
                .collect();
            for (index, shield) in shields {
                if let Some(pillar) = self.npcs.get_mut(index)
                    && pillar.shield != shield
                {
                    pillar.shield = shield;
                    pillar.dirty = true;
                }
            }
        }
        self.broadcast_lunar_state();
    }

    /// Packet `101`: what the four lunar pillar shields read right now.
    ///
    /// Sent both on change and to a joining client, because vanilla does both: `TrySendData(101,
    /// whoAmI)` sits in the join loop (`MessageBuffer.cs:869`, case 8, before `49 InitialSpawn`).
    /// Change-only left a player who joined during a pillar fight looking at four full bars.
    pub(super) fn tower_shield_frame(&self) -> terrustia_proto::Result<Vec<u8>> {
        let mut w = terrustia_proto::PacketWriter::new(id::UPDATE_TOWER_SHIELD_STRENGTHS);
        for shield in self.lunar.shields {
            w.u16(u16::try_from(shield.max(0)).unwrap_or(u16::MAX));
        }
        w.finish()
    }

    /// Tell clients what the four shields read, and how long is left before the Moon Lord.
    ///
    /// Neither was ever sent. The shield is the pillar fight's entire feedback loop — it is what
    /// tells a player their hits are counting and how close the pillar is to becoming killable —
    /// and without it the bar over each pillar sits full while the fight is won underneath it.
    ///
    /// Only when something changed, since these tick every frame and almost never move.
    fn broadcast_lunar_state(&mut self) {
        let shields = self.lunar.shields;
        let countdown = self.lunar.countdown;
        if shields != self.last_sent_shields {
            self.last_sent_shields = shields;
            if let Ok(frame) = self.tower_shield_frame() {
                self.broadcast(frame, None);
            }
        }
        if countdown != self.last_sent_countdown {
            self.last_sent_countdown = countdown;
            let mut w = terrustia_proto::PacketWriter::new(id::MOONLORD_HORROR);
            w.i32(crate::game::lunar::MOON_LORD_COUNTDOWN)
                .i32(countdown);
            if let Ok(frame) = w.finish() {
                self.broadcast(frame, None);
            }
        }
    }

    /// Tell clients how far through an invasion is.
    ///
    /// This is the progress bar. Without it a player has no way to know whether a goblin army is
    /// nearly over or has barely started, which turns a paced event into an indefinite one.
    fn broadcast_invasion_progress(&mut self) {
        let (progress, target, kind, wave) = match &self.invasion {
            Some(invasion) => (
                invasion.started_with - invasion.remaining,
                invasion.started_with,
                invasion.kind as i8,
                0i8,
            ),
            // Zero of zero is how the game says "nothing is happening", which is what takes the
            // bar off the screen.
            None => (0, 0, 0, 0),
        };
        if (progress, target) == self.last_sent_invasion {
            return;
        }
        self.last_sent_invasion = (progress, target);
        let mut w = terrustia_proto::PacketWriter::new(id::INVASION_PROGRESS_REPORT);
        w.i32(progress).i32(target).i8(kind).i8(wave);
        if let Ok(frame) = w.finish() {
            self.broadcast(frame, None);
        }
    }

    /// Tear the sky open. This is what killing the Lunatic Cultist does.
    fn trigger_lunar_apocalypse(&mut self) {
        if self.lunar.up || self.lunar.countdown > 0 {
            return;
        }
        let raised = self.lunar.trigger(
            self.world.width(),
            i32::from(self.world.surface),
            self.world.progress.downed_moon_lord,
            &mut self.rng,
        );
        for (npc_type, x, y) in raised {
            let x = x.clamp(20, self.world.width() - 20);
            let at = (
                x as f32 * crate::game::npc::TILE,
                y as f32 * crate::game::npc::TILE,
            );
            if let Some(index) = self.npcs.spawn(npc_type, at) {
                if let Some(pillar) = self.npcs.get_mut(index) {
                    pillar.shield = self.lunar.shield_of(npc_type);
                }
                self.broadcast_npc(index);
            }
        }
        self.announce("The Lunar Apocalypse is upon us!");
        info!("lunar apocalypse");
    }

    /// He arrives on whoever is nearest the middle of the world, not on whoever killed the last
    /// pillar — which is why standing somewhere sensible during the countdown matters.
    fn summon_moon_lord(&mut self) {
        let middle = (
            self.world.width() as f32 / 2.0 * crate::game::npc::TILE,
            f32::from(self.world.surface) / 2.0 * crate::game::npc::TILE,
        );
        let nearest = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.is_playing() && p.life > 0)
            .min_by(|a, b| {
                let reach = |p: &&crate::game::player::Player| {
                    (p.position.0 - middle.0).hypot(p.position.1 - middle.1)
                };
                reach(a).total_cmp(&reach(b))
            })
            .map(|p| p.slot);
        let Some(slot) = nearest else {
            return;
        };
        self.summon_on_player(slot, crate::game::lunar::MOON_LORD);
        self.announce_key("LegacyMisc.47", Vec::new());
        info!(slot, "moon lord");
    }

    /// The world's own rolls at first light: an eclipse, once a mechanical boss is down.
    pub(super) fn roll_dawn_events(&mut self) {
        if self.world.progress.hard_mode
            && self.world.progress.downed_mech_any
            && !self.world.eclipse
            && rand::Rng::random_range(&mut self.rng, 0..20) == 0
        {
            self.world.eclipse = true;
            self.announce_key("LegacyMisc.20", Vec::new());
            info!("solar eclipse");
        }
        // A meteor is rolled for every dawn once the evil's boss is down, and again whenever one
        // is owed from a kill. It is where meteorite bars come from, and with them the first
        // weapon that does not run out of ammunition.
        let owed = self.world.progress.spawn_meteor;
        if owed
            || (self.world.progress.downed_boss2
                && rand::Rng::random_range(&mut self.rng, 0..50) == 0)
        {
            self.world.progress.spawn_meteor = false;
            self.land_meteor();
        }
        self.roll_angler_quest();
        self.roll_natural_party();
        // `LanternNight::CheckMorning` — a lantern night, genuine or manually forced, never
        // survives past one dawn. No chat announcement in real vanilla either, just the world-flag
        // resync `broadcast_world_data` below already sends.
        if self.lantern_night.end_for_the_morning() {
            self.broadcast_world_data();
        }
    }

    /// `BirthdayParty::NaturalAttempt`, called once at dawn (`Main.UpdateTime_StartDay` calling
    /// `BirthdayParty.CheckMorning`) — see `game/party.rs`'s own module doc for the mechanism.
    fn roll_natural_party(&mut self) {
        use crate::game::party::{PARTY_GIRL, PartyState};
        let party_girl_present = self.npcs.iter().any(|(_, n)| n.npc_type == PARTY_GIRL);
        let eligible: Vec<u8> = self
            .npcs
            .iter()
            .filter(|(_, n)| {
                terrustia_proto::npc_data::npc_stats(n.npc_type)
                    .is_some_and(|s| PartyState::can_party(n.npc_type, s.town_npc, s.ai_style))
            })
            .map(|(index, _)| index)
            .collect();
        let Some(chosen) = self
            .party
            .natural_attempt(party_girl_present, &eligible, &mut self.rng)
        else {
            return;
        };
        let names: Vec<String> = chosen
            .iter()
            .filter_map(|&i| self.npcs.get(i))
            .map(|n| n.given_name.clone())
            .collect();
        // `Game.BirthdayParty_1`/`_2`/`_3` (`Terraria.Localization.Content.en-US.json:825-827`).
        let text = match names.as_slice() {
            [a] => format!("Looks like {a} is throwing a party"),
            [a, b] => format!("Looks like {a} & {b} are throwing a party"),
            [a, b, c, ..] => format!("Looks like {a}, {b}, and {c} are throwing a party"),
            [] => return, // cannot happen: natural_attempt only returns Some with 1-3 names
        };
        self.announce(&text);
        info!(?chosen, "birthday party");
        self.broadcast_world_data();
    }

    /// `BirthdayParty::UpdateTime`'s own per-tick prune: an NPC that stops being eligible mid-day
    /// (killed, evicted, whatever) is dropped from the celebration, and a genuine party with
    /// nobody left to celebrate ends early.
    pub(super) fn tick_party(&mut self) {
        let npcs = &self.npcs;
        let ended = self.party.prune(|index| {
            npcs.get(index).is_some_and(|n| {
                terrustia_proto::npc_data::npc_stats(n.npc_type).is_some_and(|s| {
                    crate::game::party::PartyState::can_party(n.npc_type, s.town_npc, s.ai_style)
                })
            })
        });
        if ended {
            self.announce("Party time's over!");
            self.broadcast_world_data();
        }
    }

    /// Slime Rain's own per-tick countdown, daily roll, and delayed start/stop announcement — see
    /// `crate::game::slime_rain`'s own module doc. Unlike the birthday party's once-per-dawn roll,
    /// `roll`'s own gate (`day_time && before_noon`) needs to catch the exact moment it becomes
    /// true, so this runs every tick rather than only at dawn — the same reason real vanilla's own
    /// `UpdateTime` checks it unconditionally too.
    pub(super) fn tick_slime_rain(&mut self) {
        let rate = self.journey.time_rate();
        self.slime_rain.tick(rate, &mut self.rng);

        let other_events_busy = self.world.blood_moon
            || self.world.eclipse
            || self.moon.running()
            || self.invasion.is_some()
            || self.army.ongoing();
        // `AnyPlayerReadyToFightKingSlime`'s own `statDefense > 8` half is not modelled — this
        // server never tracks a player's own defense stat (only NPC/town-resident defense is
        // server-authoritative; a player's is a client-computed value this project never
        // receives), the same narrowing `start_invasion`'s own `life_max >= 200` qualifying check
        // already made for a different event's readiness gate.
        let someone_ready = self
            .players
            .iter()
            .flatten()
            .any(|p| p.is_playing() && p.life_max > 140);
        self.slime_rain.roll(
            // `Main.raining`, the weather flag (`Main.cs:1282`), which is what
            // `Main.cs:65906`'s `!raining` reads. This passed `self.slime_rain.is_active()`
            // until 2026-08-31: `busy()`, checked inside `roll`, is `timer != 0` and strictly
            // subsumes `is_active()`'s `timer > 0`, so the argument could never change the
            // outcome and the weather half of vanilla's gate was simply missing.
            self.weather.raining,
            self.world.day_time,
            self.world.time < DAY_LENGTH / 2,
            rate,
            other_events_busy,
            self.world.progress.downed_king_slime,
            self.world.progress.hard_mode,
            someone_ready,
            self.is_expert(),
            &mut self.rng,
        );

        if let Some(now_active) = self.slime_rain.tick_warning() {
            self.announce_key(
                if now_active {
                    "LegacyWorldGen.74"
                } else {
                    "LegacyWorldGen.75"
                },
                Vec::new(),
            );
            self.broadcast_world_data();
        }
    }

    /// Pick the fish the Angler wants today, and let everybody try again.
    ///
    /// `Main.AnglerQuestSwap`. It re-rolls until it lands on a fish this world can actually
    /// produce — asking for a hardmode fish in a fresh world would cost the player the whole
    /// day's reward — and clears the list of who has already handed one in.
    pub(super) fn roll_angler_quest(&mut self) {
        use terrustia_proto::angler;
        let p = &self.world.progress;
        let any_boss = p.downed_boss1
            || p.downed_boss2
            || p.downed_boss3
            || p.hard_mode
            || p.downed_king_slime
            || p.downed_queen_bee;
        let (hardmode, crimson) = (p.hard_mode, self.world.crimson);

        let catchable: Vec<usize> = angler::QUESTS
            .iter()
            .enumerate()
            .filter(|(_, q)| angler::available(q, hardmode, crimson, any_boss))
            .map(|(index, _)| index)
            .collect();
        if catchable.is_empty() {
            return; // cannot happen with the shipped table, but guessing is worse than doing nothing
        }
        let at = rand::Rng::random_range(&mut self.rng, 0..catchable.len());
        self.angler_quest = catchable[at] as u8;
        self.angler_finished_today.clear();
        self.broadcast_angler_quest();
    }

    /// Tell each player what the Angler wants, and whether *they* have already handed one in.
    ///
    /// The second half is per-player, which is why this cannot be one broadcast: the packet
    /// carries "have you finished today", and every client needs its own answer.
    fn broadcast_angler_quest(&mut self) {
        let quest = self.angler_quest;
        let names: Vec<(u8, String)> = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.is_playing())
            .map(|p| (p.slot, p.name.clone()))
            .collect();
        for (slot, name) in names {
            let done = self.angler_finished_today.contains(&name);
            let mut w = terrustia_proto::PacketWriter::new(id::ANGLER_QUEST);
            w.u8(quest).bool(done);
            if let Ok(frame) = w.finish() {
                self.send(slot, frame);
            }
        }
    }

    /// Bring a meteor down somewhere out of the way, and tell everyone it happened.
    fn land_meteor(&mut self) {
        // Vanilla refuses a strike whose 35-tile box overlaps any player's spawn-safe zone or any
        // active NPC (`WorldGen.cs:6324-6345`) — a meteor should not bury the player who summoned
        // it, or a town. The boxes are gathered up front because the drop borrows the world and the
        // rng, and the refusal closure must not also reach back into `self`.
        let blockers = self.meteor_entity_boxes();
        let landed = crate::world::meteor::drop_checked(&mut self.world, &mut self.rng, |x, y| {
            // The crater's own box, in world pixels: `num * 2 * 16` on a side, `num == 35`.
            let strike = (
                ((x - 35) * 16) as f32,
                ((y - 35) * 16) as f32,
                ((x + 35) * 16) as f32,
                ((y + 35) * 16) as f32,
            );
            blockers.iter().any(|&b| boxes_overlap(strike, b))
        });
        let Some((x, y)) = landed else {
            debug!("nowhere for a meteor to land");
            return;
        };
        self.announce("A meteorite has landed!");
        info!(x, y, "meteorite landed");
        self.push_region(x, y, METEOR_REACH);
    }

    /// Every playing player's spawn-safe rectangle and every active NPC's hitbox, in world pixels
    /// as `(left, top, right, bottom)` — the boxes a meteor strike is tested against
    /// (`WorldGen.cs:6324-6345`). A player's box is the on-screen spawn area (`NPC.sWidth`/`sHeight`)
    /// widened by `NPC.safeRangeX`/`Y`, centred on the player; an NPC's is simply its own hitbox.
    fn meteor_entity_boxes(&self) -> Vec<(f32, f32, f32, f32)> {
        // `NPC.sWidth => 1920`, `sHeight => 1200`, and `safeRangeX = (int)(sWidth/16 * 0.52) = 62`,
        // `safeRangeY = (int)(sHeight/16 * 0.52) = 39`.
        const S_WIDTH: f32 = 1920.0;
        const S_HEIGHT: f32 = 1200.0;
        const SAFE_X: f32 = 62.0;
        const SAFE_Y: f32 = 39.0;

        let mut boxes = Vec::new();
        for player in self.players.iter().flatten() {
            if !player.is_playing() {
                continue;
            }
            let (px, py) = player.position;
            let left = px + PLAYER_HALF_WIDTH - S_WIDTH / 2.0 - SAFE_X;
            let top = py + PLAYER_HEIGHT / 2.0 - S_HEIGHT / 2.0 - SAFE_Y;
            boxes.push((
                left,
                top,
                left + S_WIDTH + SAFE_X * 2.0,
                top + S_HEIGHT + SAFE_Y * 2.0,
            ));
        }
        for (_, npc) in self.npcs.iter() {
            let (nx, ny) = npc.position;
            boxes.push((nx, ny, nx + npc.width(), ny + npc.height()));
        }
        boxes
    }

    /// Push a square region of the world at every client.
    ///
    /// A client that already holds a section will not ask for it again, so a change this size —
    /// a crater, a hardmode stripe — has to be *sent* or nobody sees it until they rejoin. It
    /// goes as a grid of tile squares, because one square carries at most 255 tiles a side.
    pub(super) fn push_region(&mut self, x: i32, y: i32, reach: i32) {
        const CHUNK: i32 = 50;
        let (from_x, to_x) = ((x - reach).max(0), (x + reach).min(self.world.width() - 1));
        let (from_y, to_y) = ((y - reach).max(0), (y + reach).min(self.world.height() - 1));

        let mut at_x = from_x;
        while at_x <= to_x {
            let width = CHUNK.min(to_x - at_x + 1);
            let mut at_y = from_y;
            while at_y <= to_y {
                let height = CHUNK.min(to_y - at_y + 1);
                let square = TileSquare {
                    x: at_x as i16,
                    y: at_y as i16,
                    width: width as u8,
                    height: height as u8,
                    change_type: 0,
                    // Column-major: all of one column, then the next.
                    tiles: (0..width)
                        .flat_map(|dx| (0..height).map(move |dy| (dx, dy)))
                        .map(|(dx, dy)| self.world.tile(at_x + dx, at_y + dy))
                        .collect(),
                };
                self.broadcast_tile_square(&square, None);
                at_y += height;
            }
            at_x += width;
        }
    }

    /// ...and at nightfall: a blood moon, which will not rise on a new moon and will not rise for
    /// a party of characters who have not found a life crystal between them.
    pub(super) fn roll_dusk_events(&mut self) {
        // `BirthdayParty::CheckNight` — a party, genuine or manually forced, never survives past
        // one day. Called first, matching `Main.UpdateTime_StartNight`'s own order.
        if self.party.end_for_the_night() {
            self.announce("Party time's over!");
            self.broadcast_world_data();
        }
        self.roll_starfall_night();
        self.roll_natural_lantern_night();
        // `WorldGen.mysticLogsEvent.StartNight()`, immediately after `LanternNight.CheckNight()`
        // in `Main.UpdateTime_StartNight` (`Main.cs:66211-66212`) and in that order here for the
        // same reason the three above are: it is the order the game writes.
        self.scan_for_fallen_logs();
        if self.world.blood_moon || self.moon.running() || self.world.moon_phase == 4 {
            return;
        }
        let worth_it = self
            .players
            .iter()
            .flatten()
            .any(|p| p.is_playing() && p.life_max > 120);
        if worth_it && rand::Rng::random_range(&mut self.rng, 0..9) == 0 {
            self.world.blood_moon = true;
            self.announce_key("LegacyMisc.8", Vec::new());
            info!("blood moon");
            self.broadcast_world_data();
        }
    }

    /// `Star.NightSetup`, called once at dusk from `Main.UpdateTime_StartNight`
    /// (`Main.cs:66208` -> `Star.cs:41-60`):
    ///
    /// ```csharp
    /// starfallBoost = 1f;
    /// int maxValue = 10;
    /// int maxValue2 = 3;
    /// if (Main.tenthAnniversaryWorld) { maxValue = 5; maxValue2 = 2; }
    /// if (Main.rand.Next(maxValue) == 0) { starfallBoost = (float)Main.rand.Next(300, 501) * 0.01f; }
    /// else if (Main.rand.Next(maxValue2) == 0) { starfallBoost = (float)Main.rand.Next(100, 151) * 0.01f; }
    /// ```
    ///
    /// Only the first branch can clear the `> 3f` the spawner asks for, and only where it rolled
    /// 301 or above (`300 * 0.01f` is exactly 3 and does not), so a meteor-shower night is a shade
    /// under one in ten. The second branch tops out at 1.5 and so can never clear it; its draw is
    /// not reproduced here, because what it lands on feeds only the star fall that drops Fallen
    /// Stars (`WorldGen.cs:72406`), which this server does not model, and this `rng` is not
    /// vanilla's stream in any case. `Main.tenthAnniversaryWorld` is not modelled anywhere in this
    /// server, so the ordinary 10 is the whole of the outer roll.
    ///
    /// The Enchanted Nightcrawler is the one thing that reads this (`NPC.cs:2409`), and without it
    /// NPC 484 was in no pool and no branch at all.
    fn roll_starfall_night(&mut self) {
        use rand::Rng;
        self.starfall_night =
            self.rng.random_range(0..10) == 0 && self.rng.random_range(300..501) > 300;
    }

    /// `LanternNight::CheckNight`, called once at dusk — see `game/lantern_night.rs`'s own module
    /// doc for the mechanism. `can_start` is real vanilla's own `LanternsCanStart()`, computed
    /// here from every input this project already tracks: no blood moon, no moon event, no real
    /// invasion, no meteor already owed, and nothing boss-shaped currently up — including the
    /// Eater of Worlds' three segments specifically, `NPCID` 13/14/15, none of which carry the
    /// ordinary `.boss` stat (confirmed directly against `npc_data.rs`, the same reason real
    /// vanilla's own `BossIsActive()` needed the same special case).
    fn roll_natural_lantern_night(&mut self) {
        let boss_active = self.npcs.iter().any(|(_, n)| {
            matches!(n.npc_type, 13..=15)
                || terrustia_proto::npc_data::npc_stats(n.npc_type).is_some_and(|s| s.boss)
        });
        let can_start = !self.world.progress.spawn_meteor
            && !self.world.blood_moon
            && !self.world.pumpkin_moon
            && !self.world.snow_moon
            && self.invasion.is_none()
            && self.lunar.countdown == 0
            && !boss_active;
        let was_up = self.lantern_night.is_up();
        self.lantern_night.natural_attempt(
            can_start,
            self.world.progress.downed_moon_lord,
            &mut self.rng,
        );
        // Real vanilla's own `LanternNight::UpdateTime` only resyncs the world flag here
        // (`NetMessage.SendData(7)`) — no chat announcement at all, unlike blood moon/eclipse/the
        // birthday party, each of which really does broadcast a line. Checked directly against
        // source rather than assumed from the pattern those other events set.
        if self.lantern_night.is_up() != was_up {
            self.broadcast_world_data();
        }
    }

    /// `MysticLogFairiesEvent.ScanWholeOverworldForLogs` (`MysticLogFairiesEvent.cs:135-166`),
    /// narrowed to the one bit of it the spawner reads:
    ///
    /// ```csharp
    /// _stumpCoords.Clear();
    /// NPC.Spawner.fairyLog = false;
    /// int num = (int)Main.worldSurface - 10;
    /// int num2 = 100;
    /// int num3 = Main.maxTilesX - 100;
    /// int num4 = 3;
    /// int num5 = 2;
    /// for (int i = 100; i < num3; i += num4)
    ///     for (int num6 = num; num6 >= num2; num6 -= num5)
    ///     {
    ///         Tile tile = Main.tile[i, num6];
    ///         if (tile.active() && tile.type == 488 && tile.liquid == 0)
    ///         {
    ///             list.Add(new Point(i, num6));
    ///             NPC.Spawner.fairyLog = true;
    ///         }
    ///     }
    /// ```
    ///
    /// The strides are the log's own size: a fallen log is three tiles wide and two tall
    /// (`world/worldgen/fallen_logs.rs`), so stepping x by 3 and y by 2 cannot step over one. This
    /// stops at the first hit rather than collecting every stump, because the stump list exists
    /// only for `TrySpawningFairies`, the *surface* half of the event, which this server does not
    /// model: an early return changes nothing about the flag and turns a worst-case million tile
    /// reads into a handful on most worlds.
    ///
    /// The loop order is vanilla's, columns outside and rows in, even though tiles are stored
    /// row-major here (`world/packed.rs`'s own `y * width + x`) and so the sweep misses the cache on
    /// every read. Swapping the two visits exactly the same grid and cannot change a bool over all
    /// of it, so it was tried and measured: it bought 16% and no more, because `World::tile`
    /// reassembling a tile costs more than the miss does. Not enough to write the loop differently
    /// from the game.
    ///
    /// Run at world load and at every dusk, which is where vanilla runs it
    /// (`WorldGen.cs:3272`, `Main.cs:66212`). Not per tick, and never on the spawn path.
    ///
    /// The remix world's own scan window (`MysticLogFairiesEvent.cs:142-146`, which looks below the
    /// rock layer instead) drops with every other `Main.remixWorld` branch in this server.
    pub(super) fn scan_for_fallen_logs(&mut self) {
        const FALLEN_LOG: u16 = 488;
        let bottom = i32::from(self.world.surface) - 10;
        let right = self.world.width() - 100;
        self.fairy_log = (100..right).step_by(3).any(|x| {
            (100..=bottom).rev().step_by(2).any(|y| {
                let tile = self.world.tile(x, y);
                tile.is_active() && tile.block == FALLEN_LOG && tile.liquid == 0
            })
        });
    }

    /// Raise a moon, if it is night.
    ///
    /// The two are exclusive and both cancel a blood moon, but raising one does not *fail* when
    /// the other is up — it replaces it, and the new moon starts again at wave one. Refusing
    /// instead would leave somebody who summoned a frost moon fighting a pumpkin one.
    pub(super) fn start_moon(&mut self, moon: crate::game::moons::Moon, slot: u8) {
        use crate::game::moons::Moon;
        if self.world.day_time {
            return;
        }
        if let Some(was) = self.moon.moon
            && was != moon
        {
            info!(?was, now = ?moon, "one moon replaces the other");
        }
        self.world.blood_moon = false;
        self.moon.start(moon);
        self.world.pumpkin_moon = moon == Moon::Pumpkin;
        self.world.snow_moon = moon == Moon::Frost;
        self.announce_key(
            match moon {
                Moon::Pumpkin => "LegacyMisc.31",
                Moon::Frost => "LegacyMisc.34",
            },
            Vec::new(),
        );
        self.broadcast_world_data();
        info!(slot, ?moon, "moon started");
    }

    /// Put a moon away. Dawn does this, and so does raising the other one.
    pub(super) fn stop_moon(&mut self) {
        let wave = self.moon.wave;
        let Some(moon) = self.moon.stop() else {
            return;
        };
        info!(?moon, wave, "moon over");
        self.world.pumpkin_moon = false;
        self.world.snow_moon = false;
        self.broadcast_world_data();
    }

    /// Count a kill against whatever moon is up.
    ///
    /// Kills are worth points rather than one each — a Pumpking is a third of a wave by itself and
    /// a scarecrow is nothing — so what you choose to fight decides how far the night gets.
    fn note_moon_kill(&mut self, npc_type: u16) {
        // Computed before borrowing `self.moon` mutably below — `is_expert`/`is_master` borrow
        // all of `self`, which would otherwise overlap the `&mut self.moon` the call needs.
        let (expert, master) = (self.is_expert(), self.is_master());
        if let Some(wave) = self.moon.note_kill(npc_type, expert, master) {
            self.announce(&format!("Wave {wave}!"));
        }
    }

    /// Land whatever an enemy leaves behind on the player it just touched.
    ///
    /// The game applies a touch debuff with a local `AddBuff` (`Player.cs:5259`, `StatusFromNPC`),
    /// which runs it through `AddBuff_DetermineBuffTimeToAdd`: in expert mode and up, a debuff in
    /// `BuffID.Sets.BuffTimeIsExtendedWithGameDifficulty` is stretched by the `DebuffTimeMultiplier`
    /// (`Player.cs:5426-5429`). A server that hands the client a raw packet-55 duration skips that
    /// stretch (the client trusts a networked duration as-is), so an expert-world On Fire! or Poison
    /// off a touch lasted half as long as the game intends and a master-world one two fifths. Pre-
    /// multiply here so the wire duration already carries it.
    fn apply_touch_debuffs(&mut self, slot: u8, npc_type: u16, difficulty: f32) {
        // `BuffID.Sets.BuffTimeIsExtendedWithGameDifficulty` (`BuffID.cs:28`): the debuffs whose
        // duration the harder modes stretch. The rest (a cosmetic or a non-scaling status) are sent
        // at their rolled length whatever the difficulty.
        const DIFFICULTY_EXTENDED: &[u16] = &[
            20, 22, 23, 24, 30, 31, 32, 33, 35, 36, 39, 44, 46, 47, 69, 70, 80, 323, 324,
        ];
        let expert = difficulty >= 2.0;
        let stretch = terrustia_proto::difficulty::debuff_time_multiplier(difficulty);
        for rule in terrustia_proto::touch_debuffs::on_touch(npc_type) {
            if rule.expert_only && !expert {
                continue;
            }
            if rule.one_in > 1 && !rand::Rng::random_ratio(&mut self.rng, 1, rule.one_in) {
                continue;
            }
            let mut ticks = if rule.ticks.1 > rule.ticks.0 {
                rand::Rng::random_range(&mut self.rng, rule.ticks.0..=rule.ticks.1)
            } else {
                rule.ticks.0
            };
            if expert && DIFFICULTY_EXTENDED.contains(&rule.buff) {
                ticks = (stretch * ticks as f32) as i32;
            }
            if let Ok(frame) = terrustia_proto::packets::add_player_buff(slot, rule.buff, ticks) {
                self.broadcast(frame, None);
            }
        }
    }

    /// Count one more of something towards its banner, and hand the banner over on the threshold.
    ///
    /// Nothing counted kills at all before this, so the reward never arrived and the world file's
    /// banner section was written as two zeroes. The count lives on the world, so it survives a
    /// restart rather than starting again at nought every session.
    fn note_banner_kill(&mut self, npc_type: u16, at: (f32, f32)) {
        use terrustia_proto::banners;

        let Some(banner) = banners::banner_of(npc_type) else {
            return;
        };
        let item = banners::banner_item(banner);
        let needed = banners::kills_needed(item);

        let count = self.world.banner_kills.entry(banner).or_insert(0);
        *count += 1;
        let reached = (*count).is_multiple_of(needed);
        let total = *count;

        // Tell every client the new count, so the bestiary's counter moves while they watch rather
        // than only on their next join.
        if let Ok(frame) = net_module::banner_kill_count(banner, total) {
            self.broadcast(frame, None);
        }

        if !reached {
            return;
        }

        let name = terrustia_proto::npc_data::npc_stats(npc_type).map_or("them", |s| s.name);
        self.announce(&format!("{total} {name} defeated!"));
        self.spawn_item(ItemStack::new(i32::from(item), 1, 0), at);
    }

    /// Record a boss's death against the world's history, and — real vanilla's own
    /// `OnGameEventClearedForTheFirstTime`, `NPC.cs`'s own `SetEventFlagCleared` calls scattered
    /// through the very dispatcher `note_boss_kill_inner` below already is — guarantee the next
    /// lantern night if any of the flags it sets just flipped false→true for the first time.
    /// Wrapped around the whole existing dispatcher, snapshot-and-diff, rather than touching each
    /// of its match arms individually: every one of vanilla's own real trigger flags this project
    /// tracks at all is covered this way without duplicating that dispatcher's own boss roster by
    /// hand, and any flag `note_boss_kill_inner` gains later is covered automatically too.
    fn note_boss_kill(&mut self, npc_type: u16) {
        let before = self.world.progress;
        self.note_boss_kill_inner(npc_type);
        let p = &self.world.progress;
        let first_time = (!before.downed_boss1 && p.downed_boss1)
            || (!before.downed_boss2 && p.downed_boss2)
            || (!before.downed_boss3 && p.downed_boss3)
            || (!before.downed_king_slime && p.downed_king_slime)
            || (!before.downed_queen_bee && p.downed_queen_bee)
            || (!before.downed_deerclops && p.downed_deerclops)
            || (!before.hard_mode && p.hard_mode)
            || (!before.downed_mech1 && p.downed_mech1)
            || (!before.downed_mech2 && p.downed_mech2)
            || (!before.downed_mech3 && p.downed_mech3)
            || (!before.downed_plantera && p.downed_plantera)
            || (!before.downed_golem && p.downed_golem)
            || (!before.downed_fishron && p.downed_fishron)
            || (!before.downed_queen_slime && p.downed_queen_slime)
            || (!before.downed_empress_of_light && p.downed_empress_of_light)
            || (!before.downed_ancient_cultist && p.downed_ancient_cultist)
            || (!before.downed_moon_lord && p.downed_moon_lord);
        if first_time {
            self.lantern_night.next_night_guaranteed = true;
        }
    }

    /// The actual boss-kill dispatcher — see [`note_boss_kill`](Self::note_boss_kill), its own
    /// thin wrapper, for the lantern-night guarantee this function's own flag transitions feed.
    ///
    /// Nothing in the game reads a boss's death directly — everything reads the flag it sets. A
    /// shop that opens, a spawn pool that widens, an event that becomes possible: all of it hangs
    /// off this, which is why a server that kills bosses without recording them has a world that
    /// never progresses.
    fn note_boss_kill_inner(&mut self, npc_type: u16) {
        use terrustia_proto::npc_params as ids;
        let p = &mut self.world.progress;
        let mut announce: Option<&'static str> = None;
        match npc_type {
            // Pre-hardmode.
            4 => p.downed_boss1 = true,
            13 | 266 => p.downed_boss2 = true,
            35 | 36 => p.downed_boss3 = true,
            50 => p.downed_king_slime = true,
            222 => p.downed_queen_bee = true,
            668 => p.downed_deerclops = true,
            // The wall: the one death that changes the world itself.
            113 => {
                if !p.hard_mode {
                    self.start_hardmode();
                }
                return;
            }
            // The mechanical three. Any one of them is what unlocks the next tier.
            134 => {
                p.downed_mech1 = true;
                p.downed_mech_any = true;
            }
            125 | 126 => {
                // The Twins only count once both eyes are gone.
                if self
                    .npcs
                    .iter()
                    .any(|(_, n)| matches!(n.npc_type, 125 | 126))
                {
                    return;
                }
                p.downed_mech2 = true;
                p.downed_mech_any = true;
            }
            127 => {
                p.downed_mech3 = true;
                p.downed_mech_any = true;
            }
            262 => p.downed_plantera = true,
            245 => p.downed_golem = true,
            370 => p.downed_fishron = true,
            657 => {
                p.downed_queen_slime = true;
                announce = Some("Queen Slime has been defeated!");
            }
            636 => {
                p.downed_empress_of_light = true;
                announce = Some("The Empress of Light has been defeated!");
            }
            // The lunar chain.
            ids::CULTIST => {
                p.downed_ancient_cultist = true;
                self.trigger_lunar_apocalypse();
                return;
            }
            crate::game::lunar::MOON_LORD => {
                self.lunar.stop();
                let p = &mut self.world.progress;
                p.downed_moon_lord = true;
                p.lunar_apocalypse_up = false;
                let who = NetworkText::key("NPCName.MoonLord", Vec::new());
                self.announce_key("Announcement.HasBeenDefeated_Single", vec![who]);
                self.broadcast_world_data();
                return;
            }
            _ => return,
        }
        // All three mechs down is what starts the bulbs growing, and the bulbs are the only way
        // to Plantera. One goes in immediately so the jungle is worth walking into straight away.
        if self.world.progress.downed_mech1
            && self.world.progress.downed_mech2
            && self.world.progress.downed_mech3
        {
            self.grow_plantera_bulb();
        }
        if let Some(text) = announce {
            self.announce(text);
        }
        // The flags reach clients in packet 7 and nowhere else, so every change has to be told.
        self.broadcast_world_data();
    }

    /// The wall has fallen: cut the two stripes through the world and turn hardmode on.
    ///
    /// This is the largest single thing that ever happens to a world, and it happens once. The
    /// stripes are cut immediately rather than in the background — a world of a few million tiles
    /// takes a fraction of a second, and doing it inline means no client can see a half-converted
    /// world.
    fn start_hardmode(&mut self) {
        use crate::world::hardmode;
        use terrustia_proto::convert::Biome;

        if self.world.progress.hard_mode {
            return;
        }
        self.world.progress.hard_mode = true;
        let evil = if self.world.crimson {
            Biome::Crimson
        } else {
            Biome::Corruption
        };
        // The dungeon's side decides which way the stripes lean, so neither lands on it.
        let dungeon_x = self.world.dungeon_x.unwrap_or(self.world.width() / 4);
        let stripes = hardmode::hardmode_stripes(self.world.width(), dungeon_x, &mut self.rng);
        let began = std::time::Instant::now();
        let mut converted = 0usize;
        for ((x, drift), into) in stripes.into_iter().zip([Biome::Hallow, evil]) {
            let changed = {
                let world = &mut self.world;
                hardmode::run_stripe(world, x, drift, into, &mut self.rng)
            };
            converted += changed.len();
        }
        self.announce_key("LegacyMisc.15", Vec::new());
        info!(
            converted,
            took_ms = began.elapsed().as_millis(),
            "hardmode began"
        );
        // Every client's view of the world is now wrong: drop the caches so they re-request.
        self.section_cache.clear();
        self.broadcast_world_data();
    }

    /// One tick of the world updating itself: grass creeping over bare ground, herbs and trees and
    /// cactus growing, vines hanging, sand falling, and - in hardmode - the biomes spreading.
    ///
    /// This is `WorldGen.UpdateWorld`'s per-tick sampling loop (`WorldGen.cs:72082-72172`). Vanilla
    /// samples random tiles across the WHOLE world every tick, scaling the count with the world's
    /// area: `maxTilesX * maxTilesY * 3e-5` overground points and `* 1.5e-5` underground points per
    /// tick (`worldUpdateRate` is 1 in ordinary play - `GetWorldUpdateRate` caps
    /// `desiredWorldTilesUpdateRate` at 24, and it defaults to 1). Rain multiplies the overground
    /// count by 1.5. `hardUpdateWorld` - the biome spread - runs from every one of those same
    /// sampled tiles, on the same budget.
    ///
    /// Re-derived per-tick budgets (`area * 3e-5` overground + `area * 1.5e-5` underground):
    ///   - large  8400x2400 = 20.16M -> 605 over + 302 under = 907/tick
    ///   - medium 6400x1800 = 11.52M -> 346 over + 173 under = 519/tick
    ///   - small  4200x1200 =  5.04M -> 151 over +  76 under = 227/tick
    ///
    /// This replaced a player-local sampler that ran a handful of tiles near each player once every
    /// ten ticks - roughly three-thousand times below vanilla's rate, and only where somebody was
    /// standing - so grass never spread, nothing regrew, and a hardmode infection never crept
    /// (L3-01). The Journey "Stop Biome Spread" power freezes the biomes where they are, which the
    /// persistence lane (L3-15) wired through the world file; the gate lives in `spreading` below.
    /// The loop is bounded and allocation-free on its common (nothing-changed) path: one `changed`
    /// buffer is reused across the whole sweep and both growth and spread push into it.
    pub(super) fn tick_world_update(&mut self) {
        let w = self.world.width();
        let h = self.world.height();
        let surface = i32::from(self.world.surface);
        // A world too small to carry the 10-tile margins has nowhere to sample; the unit-test
        // worlds are this size, and the real ones never are.
        if w <= 20 {
            return;
        }
        let area = f64::from(w) * f64::from(h);
        // Rain multiplies the overground count by 1.5 (`WorldGen.cs:72108`).
        let rain_mult = if self.world.raining { 1.5 } else { 1.0 };
        // `ceil`, not truncation: vanilla's `for (i = 0; (double)i < num5; i++)` runs `ceil(num5)`
        // passes, so a budget of 604.8 is 605 samples, not 604.
        let overground = (area * 3.0e-5 * rain_mult).ceil() as u32;
        let underground = (area * 1.5e-5).ceil() as u32;

        // PlantAlch's world-scaled odds: `num7 = Lerp(151, 151*2.8, clamp(w/4200 - 1, 0, 1))`, and
        // a herb is planted on one sample in `num7 * 100` (`WorldGen.cs:72129-72130,72657`).
        let t = (f64::from(w) / 4200.0 - 1.0).clamp(0.0, 1.0);
        let num7 = (151.0 + t * (151.0 * 2.8 - 151.0)) as u32;
        let herb_plant_odds = num7 * 100;

        // The biomes only creep in hardmode, and Journey mode's "Stop Biome Spread" power freezes
        // them where they are (`AllowedToSpreadInfections`, `WorldGen.cs:72047-72052`; L3-15).
        let hard_mode = self.world.progress.hard_mode;
        let spreading = hard_mode && !self.journey.stop_biome_spread;
        let downed_plantera = self.world.progress.downed_plantera;
        // Crystal shards and chlorophyte regrow in hardmode regardless of the Stop Biome Spread
        // power, because vanilla's `hardUpdateWorld` does them before its own spread gate (L3-14).
        let rock_layer = i32::from(self.world.rock_layer);

        let mut changed: Vec<(i32, i32)> = Vec::new();

        // Overground samples: `Next(10, w-10) x Next(10, worldSurface-1)` (`WorldGen.cs:72135`).
        let og_hi = surface - 1;
        if og_hi > 10 {
            for _ in 0..overground {
                let x = rand::Rng::random_range(&mut self.rng, 10..w - 10);
                let y = rand::Rng::random_range(&mut self.rng, 10..og_hi);
                crate::world::growth::grow_at(
                    &mut self.world,
                    x,
                    y,
                    true,
                    herb_plant_odds,
                    &mut self.rng,
                    &mut changed,
                );
                if hard_mode {
                    changed.extend(crate::world::hardmode::regrow(
                        &mut self.world,
                        x,
                        y,
                        surface,
                        rock_layer,
                        &mut self.rng,
                    ));
                }
                if spreading {
                    changed.extend(crate::world::hardmode::spread(
                        &mut self.world,
                        x,
                        y,
                        downed_plantera,
                        &mut self.rng,
                    ));
                }
            }
        }
        // Underground samples: `Next(10, w-10) x Next(worldSurface-1, h-20)` (`WorldGen.cs:73815`).
        let ug_lo = surface - 1;
        let ug_hi = h - 20;
        if ug_hi > ug_lo && ug_lo >= 10 {
            for _ in 0..underground {
                let x = self.rng.random_range(10..w - 10);
                let y = self.rng.random_range(ug_lo..ug_hi);
                crate::world::growth::grow_at(
                    &mut self.world,
                    x,
                    y,
                    false,
                    herb_plant_odds,
                    &mut self.rng,
                    &mut changed,
                );
                if hard_mode {
                    changed.extend(crate::world::hardmode::regrow(
                        &mut self.world,
                        x,
                        y,
                        surface,
                        rock_layer,
                        &mut self.rng,
                    ));
                }
                if spreading {
                    changed.extend(crate::world::hardmode::spread(
                        &mut self.world,
                        x,
                        y,
                        downed_plantera,
                        &mut self.rng,
                    ));
                }
            }
        }

        // Each changed tile is a tile change like any other. Vanilla pushes these live with
        // `SendTileSquare(-1, ...)` (to every client), so a player watching grass creep or an
        // infection advance sees it happen; the same broadcast wakes the liquid sim on tiles whose
        // occupancy just changed (a fallen sand column, a grown trunk).
        for (x, y) in changed {
            self.liquids.wake(x, y);
            let tile = self.world.tile(x, y);
            let square = TileSquare {
                x: x as i16,
                y: y as i16,
                width: 1,
                height: 1,
                change_type: 0,
                tiles: vec![tile],
            };
            self.broadcast_tile_square(&square, None);
        }
    }

    /// Keep the Old Man standing at the dungeon door until Skeletron is beaten.
    ///
    /// He is not a town NPC and does not move in anywhere: he is a fixture of the dungeon, and he
    /// is the only way to start that fight. If he is missing — a fresh server on a world that
    /// never had him, or one where something killed him — he is put back.
    pub(super) fn tick_old_man(&mut self) {
        const SKELETRON: u16 = 35;

        if !self.ticks.is_multiple_of(OLD_MAN_CHECK_INTERVAL) {
            return;
        }
        if self.world.progress.downed_boss3 {
            return;
        }
        // Not while the fight is on: he has become it.
        if self.npcs.iter().any(|(_, n)| n.npc_type == SKELETRON) {
            return;
        }
        if self.npcs.iter().any(|(_, n)| n.npc_type == OLD_MAN) {
            return;
        }
        let (Some(x), Some(y)) = (self.world.dungeon_x, self.world.dungeon_y) else {
            return;
        };
        // Only bother once somebody is near enough to see him arrive.
        let watched = self.players.iter().flatten().any(|p| {
            p.is_playing()
                && (p.position.0 / crate::game::npc::TILE - x as f32).abs() < OLD_MAN_NOTICE
        });
        if !watched {
            return;
        }
        let at = (x as f32 * 16.0, (y - 3) as f32 * 16.0);
        if let Some(index) = self.npcs.spawn(OLD_MAN, at) {
            self.broadcast_npc(index);
            debug!(x, y, "the old man is back at the dungeon");
        }
    }

    /// Whether the cultist tablet (and its four attendants) may appear at the dungeon entrance —
    /// the periodic, `tick_old_man`-shaped check the Moon Lord acceptance-test bot
    /// (`examples/moonlord.rs`, task #37) found was entirely missing: nothing anywhere placed npc
    /// 437 (`CULTIST_TABLET`), even though its own AI (`ai/boss/tablet.rs`) is real and complete
    /// once it exists — gather four attendants, wait for all four to die, shatter, raise the
    /// Cultist. Without this, the entire post-Golem game (the Lunatic Cultist, the Lunar
    /// Apocalypse, Moon Lord) was unreachable through ordinary play.
    ///
    /// ## What is confirmed vs. reasoned
    ///
    /// **Confirmed**, read directly this session from `terraria.wiki.gg`'s own "Lunatic Cultist"
    /// and "Cultists" pages (no decompiled source tree exists in this environment — see
    /// `secret_seed.rs`'s own module doc for the same standing disclosure this project already
    /// uses when it has to lean on public documentation instead of source): the tablet and its
    /// four Cultists (two Archers, two Devotees) appear at the dungeon's entrance once Golem has
    /// been defeated; they do **not** appear until Skeletron has also been defeated, because the
    /// Old Man takes spawn priority over that exact spot — the same mutual exclusion
    /// `tick_old_man` above already enforces the other way (it stops the moment `downed_boss3` is
    /// set). Killing all four attendants is what raises the Lunatic Cultist, destroying the
    /// tablet — already real and complete in `ai/boss/tablet.rs` and wired through to
    /// `ai/mod.rs`'s `ritual_complete` handling; this function's only job is to put the tablet
    /// itself on the ground so that machinery has something to run.
    ///
    /// **Reasoned, not independently sourced**: that the tablet stops reappearing once the
    /// Lunatic Cultist has actually been killed (`downed_ancient_cultist`). Nothing read this
    /// session states this explicitly, but it matches this file's own standing pattern of a
    /// one-time boss-history flag permanently retiring its own spawn path — the same shape
    /// `downed_boss3` already uses to retire the Old Man above — and the alternative, a tablet
    /// that keeps reappearing at a dungeon whose Cultist fight is already finished, has no
    /// support in anything read either.
    pub(super) fn tick_cultist_tablet(&mut self) {
        use terrustia_proto::npc_params::{
            CULTIST, CULTIST_ARCHER, CULTIST_DEVOTE, CULTIST_TABLET,
        };

        if !self.ticks.is_multiple_of(OLD_MAN_CHECK_INTERVAL) {
            return;
        }
        let progress = self.world.progress;
        if !progress.downed_golem || !progress.downed_boss3 || progress.downed_ancient_cultist {
            return;
        }
        // The tablet, its four attendants, and the boss they raise all occupy the same spot the
        // Old Man used to — never more than one of this whole chain on the ground at a time.
        if self.npcs.iter().any(|(_, n)| {
            matches!(
                n.npc_type,
                CULTIST_TABLET | CULTIST_DEVOTE | CULTIST_ARCHER | CULTIST
            )
        }) {
            return;
        }
        let (Some(x), Some(y)) = (self.world.dungeon_x, self.world.dungeon_y) else {
            return;
        };
        // Only bother once somebody is near enough to see it appear — the same reasoning
        // `tick_old_man` above already uses for the same spot.
        let watched = self.players.iter().flatten().any(|p| {
            p.is_playing()
                && (p.position.0 / crate::game::npc::TILE - x as f32).abs() < OLD_MAN_NOTICE
        });
        if !watched {
            return;
        }
        let at = (x as f32 * 16.0, (y - 3) as f32 * 16.0);
        if let Some(index) = self.npcs.spawn(CULTIST_TABLET, at) {
            self.broadcast_npc(index);
            info!(
                x,
                y, "the cultist tablet has appeared at the dungeon entrance"
            );
        }
    }

    /// Turn the Old Man into Skeletron.
    ///
    /// He is not killed and Skeletron is not summoned beside him — he *becomes* it, which is why
    /// the dungeon has no guardian afterwards. The Clothier will do instead, because he is the
    /// same man once the curse is off him.
    pub(super) fn summon_skeletron(&mut self) {
        const CLOTHIER: u16 = 54;
        const SKELETRON: u16 = 35;

        if self.npcs.iter().any(|(_, n)| n.npc_type == SKELETRON) {
            return;
        }
        let cursed = self
            .npcs
            .iter()
            .find(|(_, n)| matches!(n.npc_type, OLD_MAN | CLOTHIER) && n.is_alive())
            .map(|(index, n)| (index, n.center()));
        let Some((index, at)) = cursed else {
            debug!("nobody at the dungeon to become Skeletron");
            return;
        };

        self.spawn_skeletron_from(index, at, false);
    }

    /// A sundial or moondial: jump the clock to the next dawn or dusk.
    pub(super) fn skip_to(&mut self, dawn: bool) {
        if dawn {
            self.world.day_time = true;
            self.world.time = 0;
        } else {
            self.world.day_time = false;
            self.world.time = 0;
        }
        self.broadcast_world_data();
    }

    /// Set the clock to an exact point and tell everyone — the `/time` admin command's own effect,
    /// pulled out so Journey mode's four time-skip buttons (`StartDayImmediately`/
    /// `StartNoonImmediately`/`StartNightImmediately`/`StartMidnightImmediately`) can share it
    /// rather than re-decide what a client needs to hear about a jumped clock.
    ///
    /// Real vanilla's own equivalent, `Main.SkipToTime`, resyncs with `NetMessage.TrySendData(7)`
    /// — the same `broadcast_world_data` this file's own `skip_to` (the sundial/moondial, right
    /// above) already uses for an identical jumped clock. This used to build and broadcast a
    /// `packets::TimeSet` (message id 18) instead, which is wrong on two counts, not just a style
    /// mismatch: grepping the whole decompiled tree found no call to `SendData(18)` anywhere in
    /// real vanilla's own source, ever, from any code path — and the real client's own receive
    /// side for it (`MessageBuffer.cs`'s `case 18`) does a hard, unconditional assignment with no
    /// interpolation (`Main.dayTime = ...; Main.time = reader.ReadInt32(); ...`), which a real
    /// player watching this exact bug fire live saw as the sky visibly snapping to a different
    /// time of day rather than continuing to flow.
    pub(super) fn set_time(&mut self, day_time: bool, time: i32) {
        self.world.day_time = day_time;
        self.world.time = time;
        self.broadcast_world_data();
    }

    /// `Main.Difficulty`'s own real shape: real vanilla never reads `Main.GameMode` for anything
    /// difficulty-scaled — every such site reads this one float instead, with `expertMode`/
    /// `masterMode` themselves just `Difficulty >= 2`/`>= 3` (`Main.cs`). It is ordinarily
    /// `world.game_mode`-derived, but in a Journey world (`IsJourneyMode`) the `DifficultySlider`
    /// power overrides it to its own continuous value (`Main.cs`'s
    /// `UpdateCreativeGameModeOverride`) — every call site that used to read `world.game_mode`
    /// directly for combat/drop/event scaling should go through this instead, so the slider
    /// actually reaches it. `journey_world` gates (spawning/Godmode/FarPlacementRange/SpawnRate,
    /// which ask "is this a Journey world" rather than "how hard is it") are a different question
    /// and correctly still read `world.game_mode` directly.
    pub(super) fn effective_difficulty(&self) -> f32 {
        if self.world.game_mode == 3 {
            self.journey.difficulty_multiplier()
        } else {
            terrustia_proto::difficulty::of_game_mode(self.world.game_mode)
        }
    }

    /// `Main.expertMode`'s own definition: `Difficulty >= GameDifficultyLevel.Expert` (`2.0`).
    fn is_expert(&self) -> bool {
        self.effective_difficulty() >= 2.0
    }

    /// `Main.masterMode`'s own definition: `Difficulty >= GameDifficultyLevel.Master` (`3.0`).
    fn is_master(&self) -> bool {
        self.effective_difficulty() >= 3.0
    }

    /// The per-tick AI context every NPC's own behaviour reads from — pulled out of `tick_npcs`
    /// into its own method so it can be tested directly (a Journey world's `DifficultySlider`
    /// reaching `expert` here, for instance) rather than only indirectly through a full AI tick.
    fn ai_conditions(&self, biome: crate::game::spawn::Biome) -> crate::game::ai::Conditions {
        crate::game::ai::Conditions {
            blood_moon: self.world.blood_moon,
            day: self.world.day_time,
            eclipse: self.world.eclipse,
            pumpkin_moon: matches!(self.moon.moon, Some(crate::game::moons::Moon::Pumpkin)),
            raining: self.world.raining,
            windy: self.weather.windy(),
            crimson: self.world.crimson,
            snow: biome == crate::game::spawn::Biome::Snow,
            jungle: biome == crate::game::spawn::Biome::Jungle,
            hallow: biome == crate::game::spawn::Biome::Hallow,
            wind: self.weather.wind,
            desert: biome == crate::game::spawn::Biome::Desert,
            sandstorm: self.weather.sandstorm,
            slime_rain: self.slime_rain.is_active(),
            surface_y: f32::from(self.world.surface) * crate::game::npc::TILE,
            // `Main.expertMode` itself — `Difficulty >= Expert`, not a raw game-mode check, so a
            // Journey world's `DifficultySlider` reaches AI branches that ask this too.
            expert: self.is_expert(),
            hardmode: self.world.progress.hard_mode,
            // `Main.getGoodWorld`, the For-the-Worthy secret seed, persisted on the world from
            // worldgen. A handful of routines are genuinely harder here, not merely stat-scaled.
            get_good_world: self.world.secret_seeds.get_good,
            // `Main.tenthAnniversaryWorld`, the celebrationmk10 secret seed. Only the Crimson big
            // mimic's gag state reads it (C7-07).
            tenth_anniversary: self.world.secret_seeds.tenth_anniversary,
            world_size: (self.world.width(), self.world.height()),
        }
    }

    /// The body/tail types and trailing-part count for a worm head, applying the For-the-Worthy
    /// Destroyer variant. WOF-3: `GetDestroyerSegmentsCount` grows the Destroyer from 80 body
    /// segments to 100 in a get-good world (`NPC.cs:51488-51495`), so it runs 101 parts rather than
    /// 81. Every spawn-time path that builds a worm goes through this so the seed is honoured
    /// uniformly.
    pub(super) fn worm_parts(&self, npc_type: u16) -> Option<(u16, u16, usize)> {
        let (body, tail, segments) = terrustia_proto::npc_params::worm_body(npc_type)?;
        let segments = if npc_type == terrustia_proto::npc_params::DESTROYER_HEAD
            && self.world.secret_seeds.get_good
        {
            terrustia_proto::npc_params::DESTROYER_SEGMENTS_GOOD
        } else {
            segments
        };
        Some((body, tail, segments))
    }

    /// Put one Plantera's bulb somewhere in the underground jungle, and tell everyone.
    ///
    /// Called when the third mechanical boss falls, and again whenever the last one is broken —
    /// the jungle is never without one, which is what keeps Plantera reachable.
    pub(super) fn grow_plantera_bulb(&mut self) {
        let grown = {
            let world = &mut self.world;
            crate::world::bulbs::grow(world, &mut self.rng)
        };
        let Some((x, y)) = grown else {
            debug!("nowhere in the jungle to grow a bulb");
            return;
        };
        let square = TileSquare {
            x: x as i16,
            y: (y - 1) as i16,
            width: 2,
            height: 2,
            change_type: 0,
            tiles: (0..4)
                .map(|i| self.world.tile(x + i % 2, y - 1 + i / 2))
                .collect(),
        };
        self.broadcast_tile_square(&square, None);
        debug!(x, y, "a plantera's bulb grew");
    }

    /// Real vanilla's Wall of Flesh trigger: a Guide Voodoo Doll destroyed by lava in the
    /// Underworld while the Guide is alive. Called every tick from `tick_items` — see that
    /// function's own call site.
    ///
    /// The Moon Lord acceptance-test bot (`examples/moonlord.rs`, task #37) found this was
    /// entirely missing: npc 113 (Wall of Flesh) is absent from `npc_params::SUMMONABLE` on
    /// purpose (see below), and nothing else in this file ever spawned it either — grepping for
    /// its id, `voodoo`, `Voodoo` found only the *death*-side hardmode-transition flag already in
    /// `note_boss_kill_inner`. Without a trigger, hardmode — and everything after it — was
    /// unreachable through ordinary play.
    ///
    /// ## What is confirmed vs. what this project narrows
    ///
    /// **Confirmed**, read directly this session from `terraria.wiki.gg`'s own "Wall of Flesh" and
    /// "Guide Voodoo Doll" pages (no decompiled source tree exists in this environment — see
    /// `secret_seed.rs`'s own module doc for the same standing disclosure this project already
    /// uses when it has to lean on public documentation instead of source): the doll must be
    /// destroyed by lava while it is in the Underworld; the Guide must be alive beforehand and
    /// dies as a direct result of the doll burning (not the other way around — the doll does not
    /// need the Guide to be nearby, only alive somewhere); at least one player must be in the
    /// Underworld; the boss then spawns off whichever edge of the map is nearer to where the doll
    /// burned and walks inward. This is also *why* npc 113 is deliberately absent from
    /// `SUMMONABLE`: real vanilla never lets the Wall of Flesh be quick-summoned through the
    /// ordinary boss-item packet the way an Eye of Cthulhu or King Slime can be — adding it there
    /// would be a real behavioural addition vanilla does not have, not a fix. The Guide Voodoo
    /// Doll's own internal item id — 267 — is confirmed the same way (the wiki's own infobox
    /// states it directly; cross-checked against a second, independent page,
    /// `terrariachecklist.com/item/267`, whose own URL slug agrees).
    ///
    /// **Narrowed, and disclosed rather than silently approximated**, because this project's own
    /// item-entity physics (`world/items.rs`) has real position and real gravity, but no liquid
    /// awareness at all — `items::fall`'s own `blocked` test only asks whether a tile is solid, so
    /// an item dropped over lava today falls straight through it with no buoyancy or immersion of
    /// any kind. Building a full float-on-liquid simulation just for this one item would be a
    /// large undertaking disproportionate to this fix (this project's own standing preference —
    /// see `plan.md`'s Tier 2 "narrow, purpose-built implementation" notes — is a narrower,
    /// disclosed trigger over either fabricating physics vanilla's own item class has but this
    /// codebase doesn't, or leaving the progression blocker unfixed):
    /// - "Touches lava" is read the tile at the item's own position, sampled every tick — the same
    ///   shape `tick_shimmer` already uses to ask "is this item in shimmer" for its own liquid,
    ///   the closest existing precedent in this codebase, rather than a dedicated buoyancy
    ///   simulation. An item falling *through* a lava tile on its way to the floor beneath it
    ///   (this generator's own items have no buoyancy to stop them floating on the surface) still
    ///   satisfies this on the tick it passes through, which is a real, if narrower, sense of
    ///   "touches lava" than vanilla's own floating-on-the-surface one.
    /// - "In the Underworld" reuses the exact `height() - 200` boundary this file's own
    ///   `on_server_teleport` and `world/bulbs.rs`'s own `UNDERWORLD` constant already use for the
    ///   same question — not a new threshold invented for this fix.
    /// - "At least one player is in the Underworld" is not tracked as an independent, separate
    ///   check on player position — a player had to physically carry the doll there to drop it in
    ///   the first place, so requiring the *doll itself* to be in the Underworld stands in for it.
    ///   Disclosed as an inferred substitute for vanilla's own distinct check, not a re-derivation
    ///   of it: a doll that somehow ended up in underworld lava with no player nearby (for
    ///   instance, swept there by an unrelated mechanic) would trigger this where real vanilla
    ///   would not.
    /// - Vanilla's exact off-screen spawn distance is not reproduced pixel-for-pixel; this spawns
    ///   the boss from the nearer world edge instead (matching the wiki's own "closer to the left
    ///   edge, comes from the left" rule for *which side*) and lets `ai/boss/wall.rs`'s own AI —
    ///   "its opening direction is toward whoever woke it" — pick which way it walks from there,
    ///   since that already does not depend on the exact vanilla spawn offset to behave correctly.
    pub(super) fn tick_wall_of_flesh_trigger(&mut self) {
        const GUIDE_VOODOO_DOLL: i32 = 267;

        if self.world.progress.hard_mode || self.items.is_empty() {
            return;
        }

        let mut burned: Vec<(i16, (f32, f32))> = Vec::new();
        {
            let world = &self.world;
            let underworld_from = world.height() - 200;
            for (index, item) in self.items.iter() {
                if item.item.id != GUIDE_VOODOO_DOLL {
                    continue;
                }
                let x =
                    ((item.position.0 + crate::world::items::ITEM_SIZE / 2.0) / TILE_SIZE) as i32;
                let y =
                    ((item.position.1 + crate::world::items::ITEM_SIZE / 2.0) / TILE_SIZE) as i32;
                if y < underworld_from {
                    continue;
                }
                let tile = world.tile(x, y);
                if tile.liquid > 0 && tile.liquid_kind == terrustia_proto::Liquid::Lava {
                    burned.push((index, item.position));
                }
            }
        }

        for (index, at) in burned {
            if self.summon_wall_of_flesh(at) {
                self.items.remove(index);
                if let Ok(frame) = terrustia_proto::items::item_despawn(index) {
                    self.broadcast(frame, None);
                }
            }
        }
    }

    /// The Guide dies with the doll, and the Wall rises to replace him — see
    /// `tick_wall_of_flesh_trigger`'s own doc comment for the full disclosure of what is real
    /// vanilla behaviour here versus this project's own narrowing.
    ///
    /// Returns whether it actually happened, so the caller knows whether to consume the doll that
    /// triggered it: real vanilla always destroys a Voodoo Doll that burns in lava, but this
    /// project has no general "items burn in lava" mechanic to fall back on if the trigger did
    /// not actually fire (hardmode already begun, no Guide alive) — so a doll that cannot do
    /// anything is left alone rather than silently vanishing for no visible reason.
    fn summon_wall_of_flesh(&mut self, at: (f32, f32)) -> bool {
        const WALL_OF_FLESH: u16 = 113;

        if self.world.progress.hard_mode {
            return false;
        }
        if self.npcs.iter().any(|(_, n)| n.npc_type == WALL_OF_FLESH) {
            return false;
        }
        let guide = self
            .npcs
            .iter()
            .find(|(_, n)| n.npc_type == GUIDE && n.is_alive())
            .map(|(index, _)| index);
        let Some(guide_index) = guide else {
            debug!("a voodoo doll burned in the underworld, but no guide is alive to die with it");
            return false;
        };

        self.npcs.remove(guide_index);
        self.broadcast_npc_death(guide_index);

        let width = self.world.width() as f32 * TILE_SIZE;
        let spawn_x = if at.0 < width / 2.0 { 0.0 } else { width };
        let spawn_at = (spawn_x, at.1);
        if let Some(index) = self.npcs.spawn(WALL_OF_FLESH, spawn_at) {
            let name = self
                .npcs
                .get(index)
                .map(|n| n.stats.name)
                .unwrap_or("Something");
            let who = NetworkText::key(format!("NPCName.{name}"), Vec::new());
            self.announce_key("Announcement.HasAwoken", vec![who]);
            self.broadcast_npc(index);
            info!(x = spawn_at.0, y = spawn_at.1, "the Wall of Flesh rises");
        }
        true
    }

    /// Wake a boss that has no summon item, from the tile that was its only door.
    ///
    /// A Plantera's bulb and a bee larva are both "break this and it comes" — there is no crafted
    /// summon for either, so without this neither boss can ever appear.
    pub(super) fn wake_from_tile(&mut self, x: i32, y: i32, boss: u16) {
        if self.npcs.iter().any(|(_, n)| n.npc_type == boss) {
            return;
        }
        let at = (x as f32 * 16.0, y as f32 * 16.0);
        let nearest = self
            .players
            .iter()
            .flatten()
            .filter(|player| player.is_playing())
            .min_by(|a, b| {
                let d = |p: &Player| (p.position.0 - at.0).abs() + (p.position.1 - at.1).abs();
                d(a).total_cmp(&d(b))
            })
            .map(|p| p.slot);
        if let Some(slot) = nearest {
            self.summon_on_player(slot, boss);
        }
    }

    /// Break a shadow orb or a crimson heart.
    ///
    /// This is the early game's hinge. The first one in a world always gives a gun, which is what
    /// makes the corruption worth going into at all; every third one wakes the evil's boss, which
    /// is the only way to reach it without crafting a summon. Breaking one also makes a meteor
    /// possible, and that is where the next tier of gear comes from.
    pub(super) fn smash_orb(&mut self, x: i32, y: i32, frame_x: i16) {
        use terrustia_proto::orbs;

        let already = self.world.progress.shadow_orb_smashed;
        let roll = self.rng.random_range(0..5);
        let at = (x as f32 * 16.0, y as f32 * 16.0);
        for reward in orbs::reward(frame_x, already, roll) {
            self.spawn_item(ItemStack::new(reward.item, reward.stack, 0), at);
        }

        let p = &mut self.world.progress;
        p.shadow_orb_smashed = true;
        p.shadow_orb_count = p.shadow_orb_count.saturating_add(1);

        if p.shadow_orb_count >= orbs::ORBS_PER_BOSS {
            p.shadow_orb_count = 0;
            let boss = orbs::boss_for(frame_x);
            self.broadcast_world_data();
            // One at a time: a third orb broken while the boss is already awake is wasted, which
            // is the game's own rule and stops a stack of orbs summoning a stack of bosses.
            if self.npcs.iter().any(|(_, n)| n.npc_type == boss) {
                return;
            }
            // On the nearest player, which is who it holds responsible.
            let nearest = self
                .players
                .iter()
                .flatten()
                .filter(|player| player.is_playing())
                .min_by(|a, b| {
                    let d = |p: &Player| (p.position.0 - at.0).abs() + (p.position.1 - at.1).abs();
                    d(a).total_cmp(&d(b))
                })
                .map(|p| p.slot);
            if let Some(slot) = nearest {
                self.summon_on_player(slot, boss);
            }
            return;
        }
        let omen = orbs::omen(p.shadow_orb_count);
        self.announce(omen);
        self.broadcast_world_data();
    }

    /// Break an altar: seed a tier, spray the ore, and put a wraith on whoever did it.
    pub(super) fn smash_altar(&mut self, x: i32, y: i32, slot: u8) {
        use crate::world::hardmode;

        // The world owns the tiers, so a loaded world that already chose palladium keeps it
        // instead of being re-rolled by the next altar broken here.
        let mut tiers = hardmode::OreTiers::load(&self.world.ore_tiers);
        let Some(smashed) = hardmode::smash(
            self.world.progress.altar_count,
            self.world.progress.hard_mode,
            &mut tiers,
            hardmode::WorldShape {
                width: self.world.width(),
                height: self.world.height(),
                surface: i32::from(self.world.surface),
                rock_layer: i32::from(self.world.rock_layer),
            },
            &mut self.rng,
        ) else {
            return;
        };
        tiers.store(&mut self.world.ore_tiers);
        self.world.progress.altar_count += 1;

        let mut dug = Vec::new();
        for (vx, vy, strength, steps) in smashed.veins {
            let changed = {
                let world = &mut self.world;
                hardmode::run_vein(world, (vx, vy), strength, steps, smashed.ore, &mut self.rng)
            };
            dug.extend(changed);
        }
        // The ore lands all over the world, so the changed tiles go out as whole sections rather
        // than as thousands of squares. Clients re-request what they are near.
        for (dx, dy) in &dug {
            self.liquids.wake(*dx, *dy);
        }

        self.announce_key(smashed.announcement, Vec::new());
        info!(
            x,
            y,
            ore = smashed.ore,
            veins = dug.len(),
            altars = self.world.progress.altar_count,
            "altar smashed"
        );
        if smashed.decided_a_tier {
            self.broadcast_world_data();
        }
        for _ in 0..smashed.wraiths {
            self.summon_on_player(slot, hardmode::WRAITH);
        }
    }

    /// Which tier the world has earned. There is no choosing it: it is whatever the progression
    /// allows, and a fresh world only ever gets tier one.
    pub(super) fn army_tier(&self) -> Option<crate::game::army::Tier> {
        use crate::game::army::Tier;
        let progress = &self.world.progress;
        Some(if progress.hard_mode && progress.downed_golem {
            Tier::Three
        } else if progress.hard_mode && progress.downed_mech_any {
            Tier::Two
        } else {
            Tier::One
        })
    }

    /// Carry out what the event's fixtures decided this tick.
    ///
    /// The crystal and the gates only ever *ask*: they have no way to make an NPC or end an event
    /// themselves. Keeping the decisions in the routines and the consequences here is what lets
    /// both be tested on their own.
    fn apply_army(
        &mut self,
        gates: Vec<(i32, i32, bool)>,
        releases: Vec<((f32, f32), bool)>,
        ended: Option<bool>,
        close_gates: bool,
    ) {
        use terrustia_proto::npc_params::DD2_LANE_PORTAL;

        self.army.tick();

        for (x, y, left) in gates {
            let at = (
                x as f32 * crate::game::npc::TILE,
                (y as f32 + 1.0) * crate::game::npc::TILE,
            );
            if let Some(index) = self.npcs.spawn(DD2_LANE_PORTAL, at)
                && let Some(gate) = self.npcs.get_mut(index)
            {
                // Which side it is on is the one thing a gate cannot work out for itself.
                gate.ai[2] = if left { 0.0 } else { 1.0 };
                gate.position.0 -= gate.width() / 2.0;
                gate.position.1 -= gate.height();
                self.broadcast_npc(index);
            }
        }

        // A gate that has been told to shut goes into its closing phase wherever it is in its
        // cycle, which is what makes the ending look like the whole arena powering down at once.
        if close_gates {
            let closing: Vec<u8> = self
                .npcs
                .iter()
                .filter(|(_, n)| n.npc_type == DD2_LANE_PORTAL && n.ai[1] == 0.0)
                .map(|(index, _)| index)
                .collect();
            for index in closing {
                if let Some(gate) = self.npcs.get_mut(index) {
                    gate.ai[1] = 1.0;
                    gate.ai[0] = 0.0;
                    gate.dirty = true;
                }
            }
        }

        if !releases.is_empty()
            && let Some(tier) = self.army.tier
        {
            let players = self
                .players
                .iter()
                .flatten()
                .filter(|p| p.is_playing() && p.life > 0)
                .count();
            for (bottom, left) in releases {
                // Rebuilt per gate on purpose, and **not** hoistable out of this loop: the body
                // spawns into `self.npcs`, and vanilla calls `NPC.CountNPCS` live at every one of
                // `Difficulty_N_SpawnMonsterFromGate`'s cap checks (`DD2Event.cs:1041-1091` and
                // its tier-2/3 twins), so the second gate to release on a tick must see what the
                // first one just put out. Hoisting it would let two gates spend the same cap.
                let census: Vec<(u16, usize)> = {
                    let mut counts: std::collections::HashMap<u16, usize> =
                        std::collections::HashMap::new();
                    for (_, n) in self.npcs.iter() {
                        *counts.entry(n.npc_type).or_default() += 1;
                    }
                    counts.into_iter().collect()
                };
                let count = |ty: u16| census.iter().find(|(t, _)| *t == ty).map_or(0, |(_, c)| *c);
                let wanted = crate::game::army::from_gate(
                    tier,
                    self.army.wave,
                    left,
                    self.army.kills,
                    &count,
                    players,
                    &mut self.rng,
                );
                for npc_type in wanted {
                    if let Some(index) = self.npcs.spawn(npc_type, bottom)
                        && let Some(spawned) = self.npcs.get_mut(index)
                    {
                        spawned.position.0 -= spawned.width() / 2.0;
                        spawned.position.1 -= spawned.height();
                        self.broadcast_npc(index);
                    }
                }
            }
        }

        if let Some(won) = ended {
            self.announce(if won {
                "The Old One's Army has been defeated!"
            } else {
                "The Eternia Crystal was destroyed!"
            });
            info!(won, wave = self.army.wave, "old one's army over");
            self.army.stop();
            self.army_arena = None;
            self.wipe_army_field();
        }
    }

    /// Clear the field when the Old One's Army ends.
    ///
    /// The event leaves behind its own furniture — the lane portals, whatever was still coming
    /// through them, and the players' towers. None of it belongs to the world afterwards, and a
    /// server that leaves it standing leaves a permanent goblin camp where the arena was.
    ///
    /// The packet tells clients to do the same on their side, since they draw the towers.
    fn wipe_army_field(&mut self) {
        let leftovers: Vec<u8> = self
            .npcs
            .iter()
            .filter(|(_, n)| crate::game::army::belongs(n.npc_type))
            .map(|(index, _)| index)
            .collect();
        for index in leftovers {
            self.npcs.remove(index);
            self.broadcast_npc_death(index);
        }
        if let Ok(frame) = packets::empty(id::CRYSTAL_INVASION_WIPE_ALL_THE_THINGSSS) {
            self.broadcast(frame, None);
        }
    }

    /// Count a kill against the Old One's Army, and advance its waves.
    ///
    /// A goblin also leaves its body where it fell, which is the whole reason a Dark Mage is
    /// dangerous: it turns your own progress back into enemies.
    fn note_army_kill(&mut self, npc_type: u16) {
        if !self.army.ongoing() {
            return;
        }
        // Expert and above count double.
        let expert = self.is_expert();
        if let Some(wave) = self.army.note_kill(npc_type, expert) {
            if self.army.won() {
                // Winning does not end the event here: the crystal plays it out first, and the
                // event ends when the drama does.
                let crystals: Vec<u8> = self
                    .npcs
                    .iter()
                    .filter(|(_, n)| n.npc_type == terrustia_proto::npc_params::DD2_ETERNIA_CRYSTAL)
                    .map(|(index, _)| index)
                    .collect();
                for index in crystals {
                    if let Some(crystal) = self.npcs.get_mut(index) {
                        crystal.ai[1] = 2.0;
                        crystal.ai[0] = 0.0;
                        crystal.dirty = true;
                    }
                }
                self.announce("The Old One's Army has been defeated!");
            } else {
                self.announce(&format!("Old One's Army: wave {} complete!", wave));
                // The wave ended, so the gap begins: clients need the countdown or the pause
                // reads as the event having stopped.
                let left = self.army.hold;
                self.broadcast_army_wait(left);
            }
        }
    }

    /// Send in the next invader, if it is time and there is room.
    ///
    /// Invaders arrive at the invasion's column rather than around a player, which is what makes
    /// one feel like something marching toward you instead of something appearing on top of you.
    ///
    /// The rate and the cap are the game's own invasion override (`NPC.cs:782-786`):
    /// ```csharp
    /// if (invaders) {
    ///     maxSpawns = (int)((double)defaultMaxSpawns * (2.0 + 0.3 * (double)numberOfActivePlayers));
    ///     spawnRate = 20;
    /// }
    /// ```
    /// applied through the same per-player gate every other spawn goes through
    /// (`NPC.cs:312-317`: `nearbyActiveNPCs >= maxSpawns` first, then `rand.Next(spawnRate)`), and
    /// with `SpawnNPC`'s own `break` so at most one arrives server-wide per tick
    /// (`NPC.cs:293-303`).
    ///
    /// This was a hand-rolled fixed cadence with a world-global cap: one invader every 45 ticks
    /// against `used_slots()`, so invasions arrived 2.25 times slower than the game sends them and
    /// stalled outright whenever 13 NPCs existed anywhere in the world, however far away.
    fn spawn_invaders(&mut self, state: InvasionState) {
        let active: Vec<(f32, f32)> = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.is_playing() && p.life > 0)
            .map(|p| p.position)
            .collect();
        if active.is_empty() {
            return;
        }
        let cap = spawn::MAX_SPAWNS * (2.0 + 0.3 * active.len() as f32);

        // `SpawnNPC`'s own shape (`NPC.cs:291-306`): walk the players, and stop at the first one
        // who actually spawns something. A player who fails any gate is passed over rather than
        // ending the tick, exactly as `TrySpawnAnNPC` returning false lets the loop go on.
        for position in active {
            if spawn::nearby_active_npcs(&self.npcs, position) >= cap {
                continue;
            }
            if rand::Rng::random_range(&mut self.rng, 0..INVASION_SPAWN_RATE) != 0 {
                continue;
            }
            // The army only arrives where its front has actually reached.
            let toward = (position.0 / crate::game::npc::TILE) as i32;
            if !state.reaches(toward) {
                continue;
            }

            let present: Vec<u16> = self.npcs.iter().map(|(_, n)| n.npc_type).collect();
            let Some(npc_type) =
                state.next_invader(self.world.progress.hard_mode, &present, &mut self.rng)
            else {
                continue;
            };

            // They arrive around the player rather than at the front itself, which is what puts an
            // invasion in front of somebody instead of over the horizon.
            let side = if state.from_x > state.toward_x { 1 } else { -1 };
            // Just off screen on the side the army is coming from.
            let column = (toward + side * rand::Rng::random_range(&mut self.rng, 40..80))
                .clamp(10, self.world.width() - 10);
            let Some(ground) =
                spawn::find_ground(&self.world, column, i32::from(self.world.spawn_y))
            else {
                continue;
            };
            let at = (
                column as f32 * crate::game::npc::TILE,
                (ground - 1) as f32 * crate::game::npc::TILE,
            );
            if let Some(index) = self.npcs.spawn(npc_type, at) {
                self.broadcast_npc(index);
                break;
            }
        }
    }

    /// Begin an invasion, unless one is already under way or nobody qualifies to be invaded.
    pub(super) fn start_invasion(&mut self, kind: Invasion) {
        if self.invasion.is_some() {
            return;
        }
        // A player qualifies at two hundred maximum life; a world of fresh characters cannot be
        // invaded at all.
        let qualifying = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.is_playing() && p.life_max >= 200)
            .count();
        let Some(state) = InvasionState::begin(
            kind,
            qualifying,
            i32::from(self.world.spawn_x),
            self.world.width(),
            &mut self.rng,
        ) else {
            return;
        };
        let announcement = if kind == Invasion::Martian {
            kind.arrival().to_string()
        } else {
            format!("{} {}!", kind.arrival(), state.side())
        };
        self.announce(&announcement);
        info!(
            invasion = ?kind,
            size = state.started_with,
            from_x = state.from_x,
            "invasion started"
        );
        self.invasion = Some(state);
        // Put the bar on the screen, full, the moment the event begins.
        self.broadcast_invasion_progress();
    }

    /// Count a kill against whatever invasion is running, and end it when the last one falls.
    fn note_invasion_kill(&mut self, npc_type: u16) {
        let Some(state) = self.invasion.as_mut() else {
            return;
        };
        if !crate::game::spawn::belongs_to(state.kind, npc_type) {
            return;
        }
        state.remaining -= 1;
        if state.beaten() {
            let kind = state.kind;
            self.invasion = None;
            self.announce(kind.defeat());
            info!(invasion = ?kind, "invasion defeated");
        }
        // The bar moves on every kill, so it is told on every kill rather than on a timer.
        self.broadcast_invasion_progress();
    }

    /// Try to spawn new NPCs around the players.
    pub(super) fn tick_spawning(&mut self) {
        if self.ticks.is_multiple_of(300) {
            let active = self
                .players
                .iter()
                .flatten()
                .filter(|p| p.is_playing() && p.life > 0)
                .count();
            debug!(
                active,
                npcs = self.npcs.len(),
                slots = self.npcs.used_slots(),
                "spawn tick"
            );
        }
        // While an invasion is running its members replace the ordinary pool. The front closes on
        // the town a tile a tick and they arrive around whoever is near it, so an invasion is
        // something that comes to you rather than something waiting at the edge of the map.
        if let Some(state) = self.invasion.as_mut()
            && state.march()
        {
            let kind = state.kind;
            self.announce(&format!("{} {}", kind.arrival(), "have reached the town!"));
            info!(invasion = ?kind, "the invasion has arrived");
        }
        if let Some(state) = self.invasion {
            self.spawn_invaders(state);
            return;
        }

        // A moon or an eclipse takes the surface pool over entirely, so the census the tables
        // need is built here once rather than per candidate tile.
        let census: std::collections::HashMap<u16, usize> =
            if self.moon.running() || self.world.eclipse {
                let mut counts = std::collections::HashMap::new();
                for (_, n) in self.npcs.iter() {
                    *counts.entry(n.npc_type).or_insert(0) += 1;
                }
                counts
            } else {
                std::collections::HashMap::new()
            };
        let count = |ty: u16| census.get(&ty).copied().unwrap_or(0);
        let progress = &self.world.progress;
        let events = spawn::EventSpawns {
            moon: self.moon.moon.map(|m| (m, self.moon.wave)),
            eclipse: self.world.eclipse,
            // `Sandstorm.Happening`, which the weather tick already keeps.
            sandstorm: self.weather.sandstorm,
            // ...and so are both halves of the windy day: the latch and the target it latched on.
            happy_windy_day: self.weather.happy_windy_day,
            wind_target: self.weather.target,
            // `BirthdayParty.PartyIsUp`, for the bunny that wears a party hat.
            party: self.party.is_up(),
            // The Enchanted Nightcrawler's whole sky gate (`NPC.cs:2409`), resolved once a tick
            // because none of its three clauses varies per candidate tile: tonight's meteor
            // shower, a sky with 55 clouds or fewer in it, and no overcast layer either up or
            // still counting back down toward one.
            starfall_night: self.starfall_night
                && self.world.num_clouds <= 55
                && self.weather.cloud_bg_active == 0.0,
            // `NPC.Spawner.fairyLog`, kept by `scan_for_fallen_logs` at load and at every dusk.
            fairy_log: self.fairy_log,
            downed_plantera: progress.downed_plantera,
            hard_mode: progress.hard_mode,
            downed_mech_any: progress.downed_mech_any,
            downed_all_mechs: progress.downed_mech1
                && progress.downed_mech2
                && progress.downed_mech3,
            // Three of an event's heavies at once is as many as it will put out.
            boss_cap: self.npcs.iter().filter(|(_, n)| n.stats.boss).count() >= 3,
            // `NPC.AnyDanger()` (`NPC.cs:81063-81106`). The invasion half is already decided: this
            // function returns above if one is running, so reaching here means there is none. The
            // Moon Lord's countdown is not modelled as a countdown, but the fight itself is a live
            // boss and so is caught by the same test that catches every other one.
            any_danger: self.moon.running()
                || self.army.ongoing()
                || self.npcs.iter().any(|(_, n)| n.stats.boss && n.is_alive()),
            census: &count,
            cavern_monsters: self.cavern_monsters,
            // Where the four pillars are, gathered once a tick and only while the event is up, so
            // the zone test on the spawn path is four distance comparisons per player rather than
            // another pass over the store. `SceneMetrics.ScanNPCPositions` does the same job on a
            // real client (`SceneMetrics.cs:734-751`).
            towers: self.standing_pillars(),
        };
        self.player_biomes.advance(self.ticks);
        let spawned = spawn::try_spawn(
            &self.world,
            &self.npcs,
            &self.players,
            &events,
            &self.journey,
            &mut self.player_biomes,
            &mut self.rng,
        );
        for (npc_type, position) in spawned {
            if let Some(index) = self.npcs.spawn(npc_type, position) {
                self.broadcast_npc(index);
            }
        }
    }

    /// Advance the world census by one column, and publish it when a sweep completes.
    pub(super) fn tick_census(&mut self) {
        self.census.tick(&self.world);

        if self.census.just_finished
            && let Ok(frame) = packets::world_evil_tally(
                self.census.percent_hallow,
                self.census.percent_corrupt,
                self.census.percent_crimson,
            )
        {
            self.broadcast(frame, None);
        }
    }

    /// Age items, settle falling ones, and hand nearby ones to a player who can pick them up.
    pub(super) fn tick_items(&mut self) {
        // Ages items and lapses reservations. Nothing expires: a Terraria world item never
        // vanishes with age, and the 400-slot table is bounded by the picker's recycling instead.
        self.items.tick();

        // Falling items need their landing broadcast, but nothing in between: a client draws the
        // arc itself once it knows where the item started.
        let mut landed = Vec::new();
        let world = &self.world;
        for (index, item) in self
            .items
            .iter()
            .filter(|(_, i)| !i.resting)
            .map(|(i, item)| (i, *item))
            .collect::<Vec<_>>()
        {
            let mut item = item;
            items::fall(&mut item, |x, y| world.tile(x, y).is_active());
            let settled = item.resting;
            if let Some(slot) = self.items.get_mut(index) {
                *slot = item;
            }
            if settled {
                landed.push(index);
            }
        }
        for index in landed {
            self.broadcast_item(index);
        }
        self.tick_shimmer();
        self.tick_wall_of_flesh_trigger();
        self.correct_item_drift();

        // Offer unreserved items to the nearest player in range. Range is per-player, not one
        // shared constant: Journey mode's `FarPlacementRange` (a misleading name inherited from
        // source — both of its two real vanilla uses, `Player.cs:35212`/`35440`, are about item
        // *pickup* range, not tile placement at all) adds a flat 240 pixels for whichever players
        // have it on, but — matching source's own `difficulty == 3` guard on both sites — only in
        // a Journey-mode world; the power has no effect at all in an ordinary one, even for a
        // player who somehow has it enabled.
        let journey_world = self.world.game_mode == 3;
        let positions: Vec<(u8, (f32, f32), f32)> = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.is_playing())
            .map(|p| {
                let range = if journey_world && self.journey.has_far_placement_range(p.slot) {
                    ITEM_GRAB_RANGE + 240.0
                } else {
                    ITEM_GRAB_RANGE
                };
                (p.slot, p.position, range)
            })
            .collect();
        if positions.is_empty() {
            return;
        }

        let offers: Vec<(i16, u8, (f32, f32))> = self
            .items
            .iter()
            .filter(|(_, item)| !item.is_reserved())
            .filter_map(|(index, item)| {
                positions
                    .iter()
                    .map(|(slot, pos, range)| {
                        let (dx, dy) = (pos.0 - item.position.0, pos.1 - item.position.1);
                        (*slot, dx * dx + dy * dy, range * range)
                    })
                    .filter(|(_, d2, range2)| *d2 <= *range2)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(slot, ..)| (index, slot, item.position))
            })
            .collect();

        for (index, owner, position) in offers {
            if let Some(item) = self.items.get_mut(index) {
                item.owner = owner;
                item.reservation = items::RESERVATION_TICKS;
            }
            if let Ok(frame) = ItemOwner::reserve(index, owner, position).encode() {
                self.broadcast(frame, None);
            }
        }
    }

    /// Tell everyone the current state of one item.
    pub(super) fn broadcast_item(&mut self, index: i16) {
        let Some(item) = self.items.get(index).copied() else {
            return;
        };
        let mut sync = SyncItem::dropped(index, item.position, item.item);
        sync.velocity = item.velocity;
        if let Ok(frame) = sync.encode() {
            self.broadcast(frame, None);
        }
    }

    /// Drop an expert treasure bag: one per interacting player, sent only to them (packet 90).
    ///
    /// The game gives every player who fought the boss their own bag that nobody else sees or can
    /// take (`CommonCode.DropItemLocalPerClientAndSetNPCMoneyTo0`, `WorldItem.MakeInstanced`). This
    /// server does not track which players interacted with a given boss, so it treats every playing
    /// player as an interactor: a documented, strictly-more-generous narrowing. Each bag is a real
    /// item slot at the boss's own position, but announced with `SpawnInstancedItem` to its one
    /// owner rather than broadcast, so the others neither see it nor can race it. With nobody
    /// present it falls back to an ordinary shared drop rather than being silently lost.
    fn drop_instanced_bag(&mut self, item_id: i32, center: (f32, f32)) {
        let owners: Vec<u8> = self
            .players
            .iter()
            .flatten()
            .filter(|p| p.is_playing())
            .map(|p| p.slot)
            .collect();
        if owners.is_empty() {
            self.spawn_item(ItemStack::new(item_id, 1, 0), center);
            return;
        }
        for slot in owners {
            // Not `spawn_item`: this bag is announced to its one owner below, never broadcast.
            // The slot still has to be taken the same way, so whatever it recycled is announced.
            let Some(index) = self.take_item_slot(ItemStack::new(item_id, 1, 0), center) else {
                break;
            };
            // Instance the bag to this one player: vanilla's `WorldItem.MakeInstanced`
            // (`WorldItem.cs:326`) gives the item a real owner so only that client may ever take it.
            // Without it the bag was an un-owned world item merely not broadcast, and the proximity
            // reservation loop would hand each bag to whichever player stood nearest the boss (every
            // bag spawns at the same point), letting one player be reserved bags meant for others.
            // Owning it here keeps the pickup and update gates (`item.owner == slot`) tied to the
            // intended player, and `is_reserved` keeps the proximity loop from ever touching it.
            let Some(item) = self.items.get_mut(index) else {
                continue;
            };
            item.owner = slot;
            item.instanced = true;
            let item = *item;
            let sync = SyncItem::dropped(index, item.position, item.item);
            if let Ok(frame) = sync.encode_instanced() {
                self.send(slot, frame);
            }
        }
    }

    /// Drop whatever a broken tile yields, if it is a block with a simple drop.
    pub(super) fn spawn_tile_drop(
        &mut self,
        tile: u16,
        frame_x: i16,
        frame_y: i16,
        x: i32,
        y: i32,
    ) {
        if is_tree_tile(tile) {
            self.spawn_tree_drop(frame_x, frame_y, x, y);
            return;
        }
        let Some(item_id) = drop_of(tile, frame_x, frame_y) else {
            debug!(tile, "nothing known to drop for this tile type");
            return;
        };
        let position = (x as f32 * 16.0, y as f32 * 16.0);
        self.spawn_item(ItemStack::new(item_id, 1, 0), position);
    }

    /// A tree tile's own drop: kept apart from [`drop_of`]'s static table because vanilla's real
    /// mechanism (`WorldGen.KillTile_GetTreeDrops`) needs live world state a per-tile lookup
    /// cannot see — which biome the tree is rooted in, found by walking down to the ground.
    ///
    /// Faithful to source with one disclosed simplification: vanilla's own "bonus wood" roll also
    /// scales with the chopping player's currently-equipped axe's power (`genRand.Next(35) <=
    /// axe`), which needs per-slot inventory-content tracking this project does not have yet (the
    /// same missing prerequisite `plan.md`'s `RedHatSkeletron` gap already found and disclosed).
    /// Only the roll's own item-independent term (`Main.rand.Next(3) == 0`, real vanilla data, not
    /// invented) is transcribed here; the axe-scaling half is a real, narrower, disclosed gap.
    fn spawn_tree_drop(&mut self, frame_x: i16, frame_y: i16, x: i32, y: i32) {
        let species = self.tree_species_at(x, y);
        // `WorldGen.cs`'s own literal condition, transcribed as-is rather than redesigned: it is
        // the frame range vanilla actually uses to decide "is this the leafy top", quirks and all.
        let is_top = frame_x >= 22 && frame_y >= 198;

        let mut secondary = None;
        if is_top && rand::Rng::random_range(&mut self.rng, 0..2) == 0 && tree_drops_acorns(species)
        {
            secondary = Some(ACORN);
        }

        let primary = match species {
            TreeSpecies::Corrupt => Some(EBONWOOD),
            TreeSpecies::Crimson => Some(SHADEWOOD),
            TreeSpecies::Jungle => Some(RICH_MAHOGANY),
            TreeSpecies::Hallowed => Some(PEARLWOOD),
            TreeSpecies::Snow => Some(BOREAL_WOOD),
            TreeSpecies::Mushroom => {
                (rand::Rng::random_range(&mut self.rng, 0..2) == 0).then_some(GLOWING_MUSHROOM)
            }
            TreeSpecies::Forest | TreeSpecies::None => Some(WOOD),
        };

        let position = (x as f32 * 16.0, y as f32 * 16.0);
        if let Some(item_id) = primary {
            let mut stack: i16 = 1;
            if rand::Rng::random_range(&mut self.rng, 0..3) == 0 {
                stack += 1;
            }
            self.spawn_item(ItemStack::new(item_id, stack, 0), position);
        }
        if let Some(secondary_id) = secondary {
            self.spawn_item(ItemStack::new(secondary_id, 1, 0), position);
        }
    }

    /// Which vanilla species a tree tile belongs to, found by walking down to the ground it is
    /// rooted in — `WorldGen.GetTreeBottom` + `GetTreeType`. The broken tile itself is already
    /// cleared by the time this runs, exactly as in source (`KillTile` clears the tile before
    /// computing its drop): the walk tolerates that by treating "not active" the same as "still a
    /// tree tile, keep walking", the same forgiving condition vanilla's own loop uses.
    ///
    /// Only the ground types this generator's own `trees::fit_for_tree` can actually grow a tree
    /// on ever occur here — vanilla's desert-palm and underworld-ash branches are omitted as
    /// genuinely unreachable rather than transcribed dead, since nothing in this project plants a
    /// tree on sand or ash today.
    fn tree_species_at(&self, x: i32, y: i32) -> TreeSpecies {
        let mut y = y;
        loop {
            let here = self.world.tile(x, y);
            if here.is_active() && !is_tree_tile(here.block) {
                break;
            }
            if !self.world.in_bounds(x, y + 1) {
                break;
            }
            y += 1;
        }
        let ground = self.world.tile(x, y);
        if !ground.is_active() {
            return TreeSpecies::None;
        }
        match ground.block {
            2 => TreeSpecies::Forest,
            23 => TreeSpecies::Corrupt,
            60 => TreeSpecies::Jungle,
            70 => TreeSpecies::Mushroom,
            109 => TreeSpecies::Hallowed,
            147 => TreeSpecies::Snow,
            199 => TreeSpecies::Crimson,
            _ => TreeSpecies::None,
        }
    }

    /// The Travelling Merchant: whether he turns up today, and whether he has gone.
    ///
    /// He is not a resident — he has no house and no permanent slot. He arrives at random during
    /// the first half of a day, provided the town already has two other townsfolk, and he leaves
    /// at dusk whether or not anybody bought anything.
    ///
    /// `Main.UpdateTime`'s own arrangement: the odds are one in `27000 / dayRate * 4` per tick,
    /// which over a morning works out at rather better than it sounds.
    pub(super) fn tick_travelling_merchant(&mut self) {
        let here = self
            .npcs
            .iter()
            .find(|(_, n)| n.npc_type == TRAVELLING_MERCHANT)
            .map(|(index, _)| index);

        // Dusk, or past the hour he leaves at, and he packs up.
        let leaving = !self.world.day_time || self.world.time > MERCHANT_LEAVES_AT;
        if let Some(index) = here {
            if leaving {
                self.npcs.remove(index);
                self.broadcast_npc_death(index);
                self.announce("The Traveling Merchant has departed!");
                info!("the travelling merchant left");
            }
            return;
        }
        if leaving || self.world.time >= MERCHANT_ARRIVES_BEFORE {
            return;
        }
        // Two other townsfolk, not counting the Old Man or the Skeleton Merchant, who are not
        // residents either.
        let townsfolk = self
            .npcs
            .iter()
            .filter(|(_, n)| {
                n.stats.town_npc && n.npc_type != OLD_MAN && n.npc_type != SKELETON_MERCHANT
            })
            .count();
        if townsfolk < 2 {
            return;
        }
        if rand::Rng::random_range(&mut self.rng, 0..MERCHANT_ODDS) != 0 {
            return;
        }

        // He arrives at the world's spawn, since he has no home to arrive at.
        let at = (
            f32::from(self.world.spawn_x) * TILE_SIZE,
            f32::from(self.world.spawn_y) * TILE_SIZE - 48.0,
        );
        let Some(index) = self.npcs.spawn(TRAVELLING_MERCHANT, at) else {
            return;
        };
        self.roll_travel_shop();
        self.broadcast_npc(index);
        self.broadcast_travel_shop();
        self.announce("The Traveling Merchant has arrived!");
        info!("the travelling merchant arrived");
    }

    /// Pick what he is carrying today. `Chest.SetupTravelShop`.
    ///
    /// The stock is a chain of rolls rather than a list: each candidate that comes up overwrites
    /// the last, so the final match wins and rarer things are rarer because their odds are
    /// longer, not because they are drawn from a smaller pool.
    fn roll_travel_shop(&mut self) {
        use terrustia_proto::travel_shop::{Needs, OFFERS, TIER_ODDS};
        let p = &self.world.progress;
        let mut world = 0u16;
        for (yes, flag) in [
            (p.hard_mode, Needs::HARDMODE),
            (p.downed_mech_any, Needs::ANY_MECH),
            (p.downed_mech1, Needs::DESTROYER),
            (p.downed_mech2, Needs::TWINS),
            (p.downed_mech3, Needs::PRIME),
            (p.downed_boss1, Needs::EYE),
            (p.downed_boss3, Needs::SKELETRON),
            (p.shadow_orb_smashed, Needs::ORB_SMASHED),
        ] {
            if yes {
                world |= flag;
            }
        }
        let world = Needs(world);

        // Four to six things, as the game rolls it.
        let wanted = rand::Rng::random_range(&mut self.rng, 4..7);
        self.travel_shop.clear();
        // Fifty passes over the chain is plenty to fill six slots and bounded whatever the odds
        // do; the game's own loop is capped at five thousand for the same reason.
        for _ in 0..MERCHANT_ROLLS {
            if self.travel_shop.len() >= wanted {
                break;
            }
            let mut chosen = None;
            for offer in OFFERS {
                if !offer.needs.met_by(world) {
                    continue;
                }
                let odds = TIER_ODDS
                    .get(offer.tier as usize)
                    .copied()
                    .unwrap_or(1)
                    .max(1);
                if rand::Rng::random_range(&mut self.rng, 0..odds) == 0 {
                    // Later entries overwrite earlier ones, which is what makes the chain work.
                    chosen = Some(offer.item);
                }
            }
            if let Some(item) = chosen
                && !self.travel_shop.contains(&item)
            {
                self.travel_shop.push(item);
            }
        }
        debug!(
            stock = self.travel_shop.len(),
            "travelling merchant's stock"
        );
    }

    /// Tell everyone what he is carrying.
    ///
    /// Forty slots on the wire whatever he actually has, zero-filled — the client reads a fixed
    /// count, so a short packet desynchronises everything after it.
    fn broadcast_travel_shop(&mut self) {
        let mut w = terrustia_proto::PacketWriter::new(id::TRAVEL_MERCHANT_ITEMS);
        for slot in 0..TRAVEL_SHOP_SLOTS {
            w.i16(self.travel_shop.get(slot).copied().unwrap_or(0) as i16);
        }
        if let Ok(frame) = w.finish() {
            self.broadcast(frame, None);
        }
    }

    /// Tell clients where resting items actually are.
    ///
    /// A client simulates a dropped item's fall itself from the moment it is told the item
    /// exists, so the two can end up a few pixels apart — over a slope, in water, or after a
    /// tile is broken out from under it. The gap is invisible until somebody tries to walk over
    /// an item that is not where they see it.
    ///
    /// This is packet 160, which carries a position and nothing else. Sent for a handful of items
    /// per second rather than all of them: the drift is slow, and a full sweep every tick would
    /// cost more than the problem.
    fn correct_item_drift(&mut self) {
        if !self.ticks.is_multiple_of(ITEM_DRIFT_INTERVAL) || self.items.is_empty() {
            return;
        }
        let resting: Vec<(i16, (f32, f32))> = self
            .items
            .iter()
            .filter(|(_, item)| item.resting)
            .map(|(index, item)| (index, item.position))
            .take(ITEMS_PER_SWEEP)
            .collect();
        for (index, at) in resting {
            let mut w = terrustia_proto::PacketWriter::new(id::ITEM_POSITION);
            w.i16(index).f32(at.0).f32(at.1);
            if let Ok(frame) = w.finish() {
                self.broadcast_to_nearby(frame, at);
            }
        }
    }

    /// Sink whatever is lying in shimmer, and transmute what has gone far enough in.
    ///
    /// Shimmer is the 1.4.4 transmutation pool: an item dropped into it becomes another item,
    /// a creature, or — for coins — luck. It does not happen on contact. An item sinks over about
    /// a second and a half and changes at nine tenths, which is what makes the mechanic feel
    /// deliberate rather than punishing: you can pull something back out.
    ///
    /// One branch of the game's shimmer is missing and is a gap rather than a decision: an item
    /// with no transform and no creature falls back to being **decrafted** into its recipe's
    /// ingredients, which needs the whole recipe database. Such an item simply sits in the
    /// shimmer here. See `docs/shimmer.md`.
    fn tick_shimmer(&mut self) {
        use crate::world::items::{Shimmering, shimmer};
        // Almost always nothing is in shimmer, and finding that out should cost one scan of the
        // item table rather than a tile lookup per item.
        if self.items.is_empty() {
            return;
        }

        let mut transmuted: Vec<(i16, ItemStack, (f32, f32))> = Vec::new();
        let mut luck: Vec<((f32, f32), i32)> = Vec::new();
        let mut decrafted: Vec<Decraft> = Vec::new();
        let crimson = self.world.crimson;
        {
            let world = &self.world;
            for (index, item) in self.items.iter_mut() {
                // The game's own test: the tile *above* the item, since it is sinking through a
                // surface rather than standing in a pool.
                let x = (item.position.0 + crate::world::items::ITEM_SIZE / 2.0) / TILE_SIZE;
                let y = item.position.1 / TILE_SIZE - 1.0;
                let tile = world.tile(x as i32, y as i32);
                let in_shimmer =
                    tile.liquid > 0 && tile.liquid_kind == terrustia_proto::Liquid::Shimmer;

                if shimmer(item, in_shimmer) != Shimmering::Transmute {
                    continue;
                }
                let held = item.item;
                let at = item.position;
                let kind = u16::try_from(held.id).unwrap_or(0);

                if terrustia_proto::shimmer::is_coin(kind) {
                    // Coins are not transmuted but spent: they become luck, and are gone.
                    luck.push((
                        at,
                        terrustia_proto::shimmer::coin_luck(kind, i32::from(held.stack)),
                    ));
                    transmuted.push((index, ItemStack::EMPTY, at));
                } else if let Some(into) = terrustia_proto::shimmer::transforms_into(kind) {
                    item.shimmered = true;
                    item.item = ItemStack {
                        id: i32::from(into),
                        stack: held.stack,
                        prefix: 0,
                    };
                    transmuted.push((index, item.item, at));
                } else if let Some(recipe) = terrustia_proto::recipes::decraft_recipe(kind, crimson)
                    && i32::from(held.stack) >= i32::from(recipe.makes)
                {
                    // No transform of its own, so it comes apart into what it was made of. A
                    // stack only decrafts in whole batches: three torches give back one gel, and
                    // the two left over stay torches.
                    let batches = i32::from(held.stack) / i32::from(recipe.makes);
                    let kept = held.stack - (batches * i32::from(recipe.makes)) as i16;
                    item.shimmered = true;
                    item.item.stack = kept;
                    decrafted.push(Decraft {
                        index,
                        at,
                        recipe,
                        batches,
                        kept,
                    });
                } else {
                    // Nothing to become and nothing to come apart into. Mark it done so it stops
                    // asking the same question every tick.
                    item.shimmered = true;
                }
            }
        }

        for (index, now, at) in transmuted {
            if now.is_empty() {
                self.items.remove(index);
                if let Ok(frame) = terrustia_proto::items::item_despawn(index) {
                    self.broadcast(frame, None);
                }
            } else {
                self.broadcast_item(index);
            }
            // The sparkle, which every client draws for itself once it is told where.
            let mut w = terrustia_proto::PacketWriter::new(id::SHIMMER_ACTIONS);
            w.u8(SHIMMER_EFFECT).f32(at.0).f32(at.1);
            if let Ok(frame) = w.finish() {
                self.broadcast(frame, None);
            }
        }
        for (at, amount) in luck {
            let mut w = terrustia_proto::PacketWriter::new(id::SHIMMER_ACTIONS);
            w.u8(SHIMMER_COIN_LUCK).f32(at.0).f32(at.1).i32(amount);
            if let Ok(frame) = w.finish() {
                self.broadcast(frame, None);
            }
            debug!(amount, "coins turned into luck");
        }

        for job in decrafted {
            // Whatever did not make a whole batch stays where it was; the rest comes apart.
            if job.kept > 0 {
                self.broadcast_item(job.index);
            } else {
                self.items.remove(job.index);
                if let Ok(frame) = terrustia_proto::items::item_despawn(job.index) {
                    self.broadcast(frame, None);
                }
            }
            for &(ingredient, per_batch) in job.recipe.ingredients() {
                let mut count = job.batches.saturating_mul(i32::from(per_batch));
                // Alchemy gives back less: each unit has a one-in-three chance of being lost.
                // Without it, potions would be a free material duplicator.
                if job.recipe.alchemy {
                    let mut kept_units = 0;
                    for _ in 0..count {
                        if rand::Rng::random_range(&mut self.rng, 0..3) != 0 {
                            kept_units += 1;
                        }
                    }
                    count = kept_units;
                }
                // Spread across stacks rather than one impossible pile.
                while count > 0 {
                    let stack = count.min(i32::from(MAX_ITEM_STACK));
                    count -= stack;
                    self.spawn_item(
                        ItemStack {
                            id: i32::from(ingredient),
                            stack: stack as i16,
                            prefix: 0,
                        },
                        job.at,
                    );
                }
            }
            let mut w = terrustia_proto::PacketWriter::new(id::SHIMMER_ACTIONS);
            w.u8(SHIMMER_EFFECT).f32(job.at.0).f32(job.at.1);
            if let Ok(frame) = w.finish() {
                self.broadcast(frame, None);
            }
            debug!(
                result = job.recipe.result,
                batches = job.batches,
                "decrafted an item in shimmer"
            );
        }
    }

    /// Drop into the world every pickup a mass-wire run reported — one wire or actuator at each
    /// tile a cut cleared, at that tile's own pixel centre. Vanilla's `KillWire`/`KillActuator`
    /// spawn these the instant they clear a flag; this is the same, batched by the caller. Without
    /// it a mistake with the Grand Design simply destroyed the wire the player paid for.
    pub(super) fn spawn_wire_drops(&mut self, drops: &[(i32, i32, u16)]) {
        for &(x, y, item_id) in drops {
            let position = (x as f32 * 16.0 + 8.0, y as f32 * 16.0 + 8.0);
            self.spawn_item(ItemStack::new(i32::from(item_id), 1, 0), position);
        }
    }

    /// Take an item slot for something new, telling everyone about the item it destroyed.
    ///
    /// The single door every drop on this server goes through, because the `151` that has to
    /// precede the new item's own packet belongs to the act of taking the slot, not to any one
    /// caller. Vanilla sends it from inside `Item.NewItem` for exactly the same reason
    /// (`Item.cs:49725-49730`): the recycled item is gone from the world whether the new one is
    /// broadcast (an ordinary drop), sent to one player (an instanced treasure bag), or relayed
    /// from the client that asked for the slot.
    ///
    /// Returns the slot, or `None` when [`crate::world::items::ItemStore::pick_slot`] could not
    /// find one at all - which now takes 400 items that have not lived a single tick, rather than
    /// merely 400 items.
    pub(super) fn take_item_slot(&mut self, item: ItemStack, position: (f32, f32)) -> Option<i16> {
        let Some((index, recycled)) = self.items.spawn(item, position) else {
            debug!("item slots are full; the drop was discarded");
            return None;
        };
        if recycled && let Ok(frame) = terrustia_proto::items::item_despawn(index) {
            self.broadcast(frame, None);
        }
        Some(index)
    }

    /// Put an item into the world at a place, as a thing that can be picked up.
    pub(super) fn spawn_item(&mut self, item: ItemStack, position: (f32, f32)) -> Option<i16> {
        if item.is_empty() {
            return None;
        }
        let index = self.take_item_slot(item, position)?;
        self.broadcast_item(index);
        Some(index)
    }
}

/// What Soul Drain is allowed to drain, which is vanilla's `realLife` question and not "is this
/// attached to something bigger".
#[cfg(test)]
mod soul_drain_reaches_boss_parts {
    use super::*;
    use crate::game::npc::Npc;

    /// A worm segment shares the head's life, so draining it once per link would drain the same
    /// pool several times over; a boss part holds its own life and is drained like anything else.
    ///
    /// Neutralised by restoring `|| npc.follows_boss.is_some()`: the part assertion fails while the
    /// segment one still passes, which is the exact shape of the bug (one class correctly exempt,
    /// another wrongly exempt beside it).
    #[test]
    fn a_worm_segment_is_exempt_and_a_boss_part_is_not() {
        let mut segment = Npc::new(terrustia_proto::npc_params::GOLEM_HEAD, (0.0, 0.0), 0)
            .expect("a real NPC type");
        segment.follows = Some(3);
        assert!(
            shares_a_life_pool(&segment),
            "a worm segment reads its life from the link ahead of it, so it is vanilla's realLife \
             case and Soul Drain must not tick once per segment"
        );

        let mut part = Npc::new(terrustia_proto::npc_params::GOLEM_HEAD, (0.0, 0.0), 0)
            .expect("a real NPC type");
        part.follows_boss = Some(3);
        assert!(
            !shares_a_life_pool(&part),
            "a boss part steers itself and holds its own life, so vanilla drains it: Skeletron's \
             hands and Golem's fists were silently immune while this said otherwise"
        );

        let ordinary = Npc::new(terrustia_proto::npc_params::GOLEM_HEAD, (0.0, 0.0), 0)
            .expect("a real NPC type");
        assert!(
            !shares_a_life_pool(&ordinary),
            "and an unattached NPC is drained, or the whole debuff does nothing"
        );
    }
}

/// The pillar fight, from the far side of the spawn path: a kill counts against one shield and one
/// only, and the pillar it belongs to is the only one that becomes hittable.
#[cfg(test)]
mod lunar_pillar_fight {
    use super::*;
    use crate::config::Config;
    use crate::game::lunar::{self, PILLARS, SHIELD_STRENGTH};

    /// The Solar Solenian: solar escort, no worm body, and nothing splits off it.
    const SOLENIAN: u16 = 419;

    fn arena() -> GameServer {
        GameServer::new(
            Config::default(),
            crate::world::World::empty(500, 300, "pillar fight probe"),
        )
    }

    /// Kill one of a type, through the server's own death path rather than by poking the state.
    fn kill(server: &mut GameServer, npc_type: u16) {
        let index = server
            .npcs
            .spawn(npc_type, (2000.0, 2000.0))
            .expect("a free NPC slot");
        let center = server.npcs.get(index).expect("just spawned").center();
        server.npc_died(index, npc_type, center, 0.0);
    }

    fn alive(server: &GameServer, npc_type: u16) -> usize {
        server
            .npcs
            .iter()
            .filter(|(_, n)| n.npc_type == npc_type && n.is_alive())
            .count()
    }

    fn invulnerable(server: &GameServer, pillar: u16) -> bool {
        server
            .npcs
            .iter()
            .find(|(_, n)| n.npc_type == pillar)
            .map(|(_, n)| n.invulnerable)
            .expect("the pillar should still be standing")
    }

    /// A hundred kills clear the tower those kills belonged to, and leave the other three exactly
    /// as they were. `NPC.cs:80095-80136` is the game's own credit list, and it is per pillar.
    ///
    /// Neutralised by deleting `self.lunar.note_kill(npc_type)` from `npc_died`: the solar shield
    /// stays at its full hundred and the first assertion after the kills fails.
    #[test]
    fn clearing_one_escort_drops_only_its_own_pillars_shield() {
        let mut server = arena();
        server.trigger_lunar_apocalypse();
        for pillar in PILLARS {
            assert_eq!(server.lunar.shield_of(pillar), SHIELD_STRENGTH);
        }

        for _ in 0..SHIELD_STRENGTH {
            kill(&mut server, SOLENIAN);
        }

        assert_eq!(server.lunar.shield_of(lunar::SOLAR), 0);
        for pillar in [lunar::VORTEX, lunar::NEBULA, lunar::STARDUST] {
            assert_eq!(
                server.lunar.shield_of(pillar),
                SHIELD_STRENGTH,
                "pillar {pillar}'s shield moved for a kill that was not its own",
            );
        }
    }

    /// ...and only then does that pillar take damage. The shield is not a health bar, it is the
    /// gate on the health bar (`ai/hardmode/pillar.rs`, `NPC.cs:39492`).
    ///
    /// Neutralised by deleting the `pillar.shield = shield` write from `tick_lunar`: the pillar's
    /// own copy of the count stays at whatever it was raised with, its routine keeps
    /// `invulnerable` set, and the "the solar pillar should be hittable now" assertion fails.
    #[test]
    fn a_pillar_becomes_damageable_only_once_its_own_escort_is_gone() {
        let mut server = arena();
        server.trigger_lunar_apocalypse();
        server.tick_lunar();
        server.tick_npcs();
        for pillar in PILLARS {
            assert!(
                invulnerable(&server, pillar),
                "pillar {pillar} was hittable with its shield up",
            );
        }

        for _ in 0..SHIELD_STRENGTH {
            kill(&mut server, SOLENIAN);
        }
        server.tick_lunar();
        server.tick_npcs();

        assert!(
            !invulnerable(&server, lunar::SOLAR),
            "the solar pillar should be hittable now its escort is dead",
        );
        for pillar in [lunar::VORTEX, lunar::NEBULA, lunar::STARDUST] {
            assert!(
                invulnerable(&server, pillar),
                "pillar {pillar} became hittable off somebody else's kills",
            );
        }
    }

    /// The two escorts that leave something behind, and the caps that stop a burst becoming a
    /// swarm: `NPC.cs:84381-84403` and `NPC.cs:83981-83994`.
    ///
    /// Neutralised by deleting `self.split_on_death(npc_type, center)` from `npc_died`: nothing is
    /// left behind by either death and the first count in each half reads zero.
    #[test]
    fn a_stardust_cell_bursts_and_a_hornet_queen_leaves_larvae() {
        let mut server = arena();

        // `num172` counts 406 and 405 including the cell that is dying, so the first burst sees
        // one and gives the full four.
        kill(&mut server, 405);
        assert_eq!(alive(&server, 406), 4, "the first cell should give four");
        // Five about: 4 -> three more.
        kill(&mut server, 405);
        assert_eq!(alive(&server, 406), 7);
        // Eight about: 7 -> two more.
        kill(&mut server, 405);
        assert_eq!(alive(&server, 406), 9);
        // Ten about: 10 -> one more, and every burst after this is one.
        kill(&mut server, 405);
        assert_eq!(alive(&server, 406), 10);

        let mut server = arena();
        // `num137` is `CountNPCS(428) + CountNPCS(427) + CountNPCS(426) * 3`, again counting the
        // queen that is dying, so the first death is three against a threshold of twenty.
        kill(&mut server, 426);
        assert_eq!(alive(&server, 428), 3, "a queen should leave three larvae");
        for _ in 0..5 {
            kill(&mut server, 426);
        }
        assert_eq!(alive(&server, 428), 18, "six queens, three larvae each");
        // The seventh finds the swarm at twenty-one and leaves nothing.
        kill(&mut server, 426);
        assert_eq!(alive(&server, 428), 18, "the swarm is capped at twenty");
    }
}

#[cfg(test)]
mod lunar_pillar_persistence {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(400, 300, "lunar pillar persistence probe")
    }

    /// L3-02's headline fixture: a save taken mid-Lunar-Apocalypse round-trips the four standing
    /// pillars, and the event still resolves correctly afterwards — the last pillar falling still
    /// starts the Moon Lord's countdown, rather than every tower reading as defeated the moment
    /// the reloaded world's first tick runs.
    ///
    /// Fails against the bug this exists to catch: reverting `read_town_npcs`/`write_town_npcs`
    /// to drop the second `SaveNPCs` list (so `record_lunar_pillars` has nothing to write and
    /// `restore_lunar_pillars` has nothing to restore) turns this red at the first `tick_lunar`
    /// assertion below — with no pillars restored, `towers.0..3` all read `false` against a
    /// `tower_active_*` of `true`, and every tower is marked downed on the very first tick after
    /// the reload.
    #[test]
    fn a_mid_apocalypse_save_round_trips_the_four_pillars_and_the_event_survives_a_reload() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.trigger_lunar_apocalypse();
        assert_eq!(
            server
                .npcs
                .iter()
                .filter(|(_, n)| crate::game::lunar::PILLARS.contains(&n.npc_type))
                .count(),
            4,
            "the trigger itself must raise all four"
        );
        // `trigger_lunar_apocalypse` only spawns the NPCs; `tower_active_*` is set from the live
        // roster by `tick_lunar` itself, exactly as an ordinary tick during the fight would.
        server.tick_lunar();
        assert!(server.world.progress.tower_active_solar);
        assert!(server.world.progress.tower_active_vortex);
        assert!(server.world.progress.tower_active_nebula);
        assert!(server.world.progress.tower_active_stardust);
        assert!(server.world.progress.lunar_apocalypse_up);

        // What a real save does, in order (`save_world`/`save_world_in_background`).
        server.record_town_npcs();
        server.record_lunar_pillars();
        assert_eq!(
            server.world.saved_npcs.len(),
            4,
            "the live pillars must be captured onto the world before it is serialised"
        );

        let bytes = crate::world::wld_save::serialize(&server.world).expect("serialize");
        let loaded = crate::world::wld::parse(&bytes).expect("parse");

        let mut reloaded = GameServer::new(Config::default(), loaded);
        // What `GameServer::run` does at startup, in order: residents, then the pillars, before
        // the first tick can run `tick_lunar` and misread an empty roster as a defeat.
        reloaded.restore_town_npcs();
        reloaded.restore_lunar_pillars();
        assert_eq!(
            reloaded
                .npcs
                .iter()
                .filter(|(_, n)| crate::game::lunar::PILLARS.contains(&n.npc_type))
                .count(),
            4,
            "all four pillars must come back as live NPCs"
        );
        assert!(
            reloaded.lunar.up,
            "the runtime event tracker must know the fight is still on, or the Moon Lord's \
             countdown branch below never fires"
        );

        reloaded.tick_lunar();
        let p = &reloaded.world.progress;
        assert!(
            !p.downed_tower_solar,
            "a standing tower must not be marked downed by the first tick after a reload"
        );
        assert!(!p.downed_tower_vortex);
        assert!(!p.downed_tower_nebula);
        assert!(!p.downed_tower_stardust);
        assert!(
            p.tower_active_solar
                && p.tower_active_vortex
                && p.tower_active_nebula
                && p.tower_active_stardust,
            "and all four must still read as standing"
        );
        assert!(p.lunar_apocalypse_up, "the apocalypse is still up");

        // The event still functions after the reload: killing the last standing pillar starts
        // the Moon Lord's countdown, which is only possible because `self.lunar.up` came back
        // `true` above rather than staying at `LunarState::default()`'s `false`.
        let indices: Vec<u8> = reloaded
            .npcs
            .iter()
            .filter(|(_, n)| crate::game::lunar::PILLARS.contains(&n.npc_type))
            .map(|(i, _)| i)
            .collect();
        for index in indices {
            reloaded.npcs.remove(index);
        }
        reloaded.tick_lunar();
        assert!(!reloaded.lunar.up, "the last pillar has fallen");
        assert!(
            reloaded.lunar.countdown > 0,
            "the Moon Lord's countdown must have started"
        );
    }

    /// A world with no apocalypse in progress restores nothing and leaves the event tracker
    /// alone — restoring pillars must not be a side effect on every ordinary world.
    #[test]
    fn a_world_with_no_pillars_restores_nothing() {
        let world = tiny_world();
        let mut server = GameServer::new(Config::default(), world);
        server.restore_lunar_pillars();

        assert!(!server.lunar.up);
        assert_eq!(
            server
                .npcs
                .iter()
                .filter(|(_, n)| crate::game::lunar::PILLARS.contains(&n.npc_type))
                .count(),
            0
        );
    }
}

#[cfg(test)]
mod town_npc_persistence {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(400, 300, "town npc persistence probe")
    }

    /// L3-29: the Travelling Merchant is a real, `town_npc: true` NPC type
    /// (`terrustia-proto/src/npc_data.rs`), so without the exclusion he would pass
    /// `record_town_npcs`'s own filter alongside any real resident. Vanilla's own `SaveNPCs`
    /// explicitly skips him (`nPC.type != 368`, `WorldFile.cs:1724`) because he is not a
    /// resident: he arrives and leaves on his own schedule.
    #[test]
    fn the_travelling_merchant_is_not_recorded_as_a_resident() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let guide = server.npcs.spawn(GUIDE, (100.0, 100.0)).expect("a slot");
        let merchant = server
            .npcs
            .spawn(TRAVELLING_MERCHANT, (200.0, 200.0))
            .expect("a slot");
        assert!(server.npcs.get(merchant).unwrap().stats.town_npc);

        server.record_town_npcs();

        let types: Vec<i32> = server.world.town_npcs.iter().map(|n| n.net_id).collect();
        assert_eq!(
            types,
            vec![i32::from(GUIDE)],
            "only the real resident, never the Travelling Merchant"
        );
        let _ = guide;
    }

    /// L3-29's other half: a `homeless_despawn` flag a load decoded must survive
    /// `record_town_npcs`, not be clobbered to `false` on the session's first save.
    #[test]
    fn a_loaded_homeless_despawn_flag_survives_a_re_record() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let index = server.npcs.spawn(GUIDE, (50.0, 50.0)).expect("a slot");
        // What `restore_town_npcs` would have set, from a file that decoded `homelessDespawn`.
        server.npcs.get_mut(index).unwrap().homeless_despawn = true;

        server.record_town_npcs();

        assert!(
            server.world.town_npcs[0].homeless_despawn,
            "a despawn timer a load decoded must round-trip, not reset to false"
        );
    }

    /// L2-17: a hurt townsperson mends over time, the way the game's `CheckLifeRegen` heals a point
    /// each time the regen counter passes 180 (`NPC.cs:93622-93648`). Fails before the fix, when a
    /// town NPC had no regen at all and a resident wounded in a blood moon stayed at a sliver of
    /// health until they died or the world reloaded.
    #[test]
    fn a_hurt_town_npc_regenerates_toward_full_but_not_past_it() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let guide = server.npcs.spawn(GUIDE, (100.0, 100.0)).expect("a slot");
        let max = server.npcs.get(guide).unwrap().life_max;
        assert!(max > 2, "the Guide has room to be hurt");

        // Wound the Guide, then give it a minute of regen ticks.
        server.npcs.get_mut(guide).unwrap().life = 1;
        for _ in 0..600 {
            server.tick_town_regen();
        }
        let healed = server.npcs.get(guide).unwrap().life;
        assert!(
            healed > 1,
            "a hurt town NPC should recover health over a minute, still at {healed}",
        );
        assert!(healed <= max, "regen must never overshoot the maximum");

        // At full health regen does nothing: it neither overshoots nor churns the counter.
        server.npcs.get_mut(guide).unwrap().life = max;
        for _ in 0..600 {
            server.tick_town_regen();
        }
        assert_eq!(
            server.npcs.get(guide).unwrap().life,
            max,
            "a healthy town NPC stays exactly at full",
        );
    }
}

#[cfg(test)]
mod stop_biome_spread_gate {
    use super::*;
    use crate::config::Config;
    use terrustia_proto::Tile;

    /// Corrupt grass (`hardmode::spreads`) and ordinary grass (`hardmode::takeable`), the pair
    /// `hardmode::spread` needs to actually convert something.
    const CORRUPT: u16 = 23;
    const GRASS: u16 = 2;

    /// A checkerboard of corrupt and ordinary grass, large enough that `tick_world_update`'s
    /// world-wide sampler is overwhelmingly likely to land a spread on it within a handful of
    /// ticks with the gate off, and can change nothing at all with it on - the gate is what this
    /// test is about, not `hardmode::spread`'s own odds, which its own module already covers. It
    /// is all grass, so with the biomes frozen the growth half of the tick has nothing to do to
    /// the counted region either (herbs and vines can only appear on its outermost exposed edge,
    /// one tile beyond the square this counts).
    fn world_with_hardmode_and_a_player() -> (crate::world::World, (f32, f32)) {
        let mut world = crate::world::World::empty(600, 600, "stop biome spread probe");
        world.progress.hard_mode = true;
        let (px, py): (i32, i32) = (300, 300);
        for x in (px - 130)..=(px + 130) {
            for y in (py - 130)..=(py + 130) {
                let block = if (x + y).rem_euclid(2) == 0 {
                    CORRUPT
                } else {
                    GRASS
                };
                world.set_tile(x, y, Tile::block(block));
            }
        }
        (
            world,
            (
                px as f32 * crate::game::npc::TILE,
                py as f32 * crate::game::npc::TILE,
            ),
        )
    }

    fn with_a_player_at(mut server: GameServer, position: (f32, f32)) -> GameServer {
        let (tx, _rx) = mpsc::channel(64);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), tx);
        player.state = ConnState::Playing;
        player.position = position;
        server.players[0] = Some(player);
        server
    }

    /// Counts how many tiles in the checkerboard no longer match the pattern it was built with -
    /// the observable evidence that `hardmode::spread` actually ran and converted something.
    fn drift_from_checkerboard(world: &crate::world::World) -> usize {
        let (px, py): (i32, i32) = (300, 300);
        let mut drifted = 0;
        for x in (px - 130)..=(px + 130) {
            for y in (py - 130)..=(py + 130) {
                let want = if (x + y).rem_euclid(2) == 0 {
                    CORRUPT
                } else {
                    GRASS
                };
                if world.tile(x, y).block != want {
                    drifted += 1;
                }
            }
        }
        drifted
    }

    /// L3-15: with the power off, a hardmode world with a player standing in an infected area
    /// spreads — the positive control proving the fixture itself is capable of a change, so the
    /// negative test below is not merely "nothing happens here regardless."
    #[test]
    fn with_the_power_off_the_infection_spreads() {
        let (world, position) = world_with_hardmode_and_a_player();
        let mut server = with_a_player_at(GameServer::new(Config::default(), world), position);
        assert!(!server.journey.stop_biome_spread, "off by default");

        for _ in 0..50 {
            server.tick_world_update();
        }

        assert!(
            drift_from_checkerboard(&server.world) > 0,
            "a hardmode world saturated with corruption, with a player standing in it, must show \
             some spread within fifty ticks"
        );
    }

    /// The fix itself: with `journey.stop_biome_spread` on, the exact same fixture that spreads
    /// above must not change a single tile, matching `AllowedToSpreadInfections = !power.Enabled`
    /// (`WorldGen.cs:72047-72052`).
    #[test]
    fn with_the_power_on_nothing_spreads() {
        let (world, position) = world_with_hardmode_and_a_player();
        let mut server = with_a_player_at(GameServer::new(Config::default(), world), position);
        server.journey.stop_biome_spread = true;

        for _ in 0..50 {
            server.tick_world_update();
        }

        assert_eq!(
            drift_from_checkerboard(&server.world),
            0,
            "Stop Biome Spread must hold the infection completely still"
        );
    }
}

#[cfg(test)]
mod invasion_spawn_tests {
    use super::*;
    use crate::config::Config;
    use crate::game::event::{Invasion, InvasionState};

    /// A server with a floor, one playing player standing on it, and a goblin army whose front has
    /// already reached them.
    fn under_siege() -> GameServer {
        let mut world = crate::world::World::empty(600, 300, "invasion");
        let floor = 150;
        for x in 0..world.width() {
            world.set_tile(x, floor, terrustia_proto::Tile::block(1));
        }
        world.spawn_x = 300;
        world.spawn_y = floor as i16 - 1;

        let mut server = GameServer::new(Config::default(), world);
        let (out_tx, out_rx) = tokio::sync::mpsc::channel(1024);
        // Held open: a dropped receiver would make every broadcast fail, which is not what this
        // test is measuring.
        std::mem::forget(out_rx);
        let mut player =
            crate::game::player::Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = crate::game::ConnState::Playing;
        player.life = 100;
        player.position = (
            300.0 * crate::game::npc::TILE,
            149.0 * crate::game::npc::TILE,
        );
        server.players[0] = Some(player);

        server.invasion = Some(InvasionState {
            kind: Invasion::Goblin,
            remaining: 1000,
            started_with: 1000,
            from_x: 300,
            toward_x: 300,
        });
        server
    }

    /// Run the invasion spawner for a while, clearing the field every tick so the near-player cap
    /// never binds and the count measures the *rate* alone.
    fn arrival_rate_over(server: &mut GameServer, ticks: u64) -> usize {
        let state = server.invasion.expect("an invasion");
        let mut total = 0;
        for _ in 0..ticks {
            server.spawn_invaders(state);
            let arrived: Vec<u8> = server.npcs.iter().map(|(index, _)| index).collect();
            total += arrived.len();
            for index in arrived {
                server.npcs.remove(index);
            }
            server.ticks += 1;
        }
        total
    }

    /// `NPC.cs:782-786`, `spawnRate = 20` during an invasion, rolled per player per tick against
    /// the same one-in-`spawnRate` gate every other spawn goes through (`NPC.cs:316`).
    ///
    /// Fails before the fix, which sent one invader every 45 ticks flat: 2.25 times slower.
    /// 3,000 ticks is about 150 arrivals at the game's pace and exactly 66 at the old fixed
    /// cadence, so the bar sits well clear of both the old number and the new one's spread
    /// (standard deviation about 12, so 100 is more than four below the mean).
    #[test]
    fn an_invasion_arrives_at_the_games_own_rate() {
        let mut server = under_siege();
        let arrived = arrival_rate_over(&mut server, 3_000);
        assert!(
            arrived > 100,
            "3,000 ticks at a one-in-twenty roll should send about 150 invaders, got {arrived}",
        );
    }

    /// The cap is `defaultMaxSpawns * (2 + 0.3 * players)` counted *per player against what is
    /// near them* (`NPC.cs:312-313`, `:783`), not against every NPC in the world.
    ///
    /// Fails before the fix, which compared `used_slots()` against a world-global 13: an invasion
    /// stalled outright whenever thirteen NPCs existed anywhere, however far away.
    #[test]
    fn monsters_on_the_far_side_of_the_world_do_not_stall_an_invasion() {
        let mut server = under_siege();
        // Twenty zombies at the other end of the map, well outside anyone's active box.
        for _ in 0..20 {
            server.npcs.spawn(
                3,
                (
                    10.0 * crate::game::npc::TILE,
                    149.0 * crate::game::npc::TILE,
                ),
            );
        }
        let before = server.npcs.iter().count();
        assert!(before >= 20, "the distant crowd should exist: {before}");

        let state = server.invasion.expect("an invasion");
        for _ in 0..600 {
            server.spawn_invaders(state);
            server.ticks += 1;
        }
        let after = server.npcs.iter().count();
        assert!(
            after > before,
            "an invasion should still arrive past a crowd on the far side: {before} -> {after}",
        );
    }
}

#[cfg(test)]
mod combat_tests {
    use crate::game::projectile::ProjectileStore;

    /// A shot decided by a routine has to become an entity that moves and can hit somebody.
    /// Everything up to this point only produced intentions.
    #[test]
    fn a_launched_shot_flies_and_then_expires() {
        let mut store = ProjectileStore::new();
        let index = store
            .launch(38, (1000.0, 1000.0), (6.0, 0.0), 15, 60)
            .expect("harpy feather");
        let start = store.get(index).unwrap().position;

        struct Sky;
        impl crate::game::npc::TileView for Sky {
            fn tile(&self, _x: i32, _y: i32) -> terrustia_proto::tile::Tile {
                terrustia_proto::tile::Tile::AIR
            }
        }

        let mut ticks = 0;
        loop {
            let done = {
                let p = store.get_mut(index).unwrap();
                crate::game::projectile::step(p, &Sky, &mut Vec::new())
                    == crate::game::projectile::Outcome::Spent
            };
            ticks += 1;
            if done || ticks > 200 {
                break;
            }
        }
        assert_eq!(ticks, 60, "it should live exactly as long as it was given");
        let travelled = store.get(index).unwrap().position.0 - start.0;
        assert!(travelled > 300.0, "and cover ground, got {travelled}");
    }

    #[test]
    fn a_shot_that_hits_a_player_overlaps_their_box() {
        let mut store = ProjectileStore::new();
        let index = store
            .launch(38, (1000.0, 1000.0), (0.0, 0.0), 15, 60)
            .unwrap();
        let p = store.get(index).unwrap();
        let player_box = (
            1000.0 - crate::game::ai::PLAYER_WIDTH as f32 / 2.0,
            1000.0 - crate::game::ai::PLAYER_HEIGHT as f32 / 2.0,
        );
        assert!(p.overlaps(
            player_box,
            (
                crate::game::ai::PLAYER_WIDTH as f32,
                crate::game::ai::PLAYER_HEIGHT as f32
            )
        ));
    }

    #[test]
    fn a_penetrating_shot_survives_more_than_one_hit() {
        let mut store = ProjectileStore::new();
        // The demon scythe passes through everything.
        let scythe = store.launch(44, (0.0, 0.0), (1.0, 0.0), 21, 60).unwrap();
        assert_eq!(store.get(scythe).unwrap().penetrate, -1);
        // The sand ball does too; the eye laser does not.
        let laser = store.launch(83, (0.0, 0.0), (1.0, 0.0), 11, 60).unwrap();
        assert_eq!(store.get(laser).unwrap().penetrate, 3);
    }
}

#[cfg(test)]
mod town_buff_defense {
    use super::*;
    use crate::config::Config;

    const GUIDE: u16 = 22;

    /// A guide standing in an otherwise empty world, so the only thing moving his armour is the
    /// buff under test: no boss is down, so `town_toughness().defense` is zero.
    fn guide_with(mut set: impl FnMut(&mut crate::game::buffs::Flags)) -> i32 {
        let mut server = GameServer::new(
            Config::default(),
            crate::world::World::empty(200, 150, "town buff probe"),
        );
        let index = server
            .npcs
            .spawn(GUIDE, (100.0, 100.0))
            .expect("a slot for the guide");
        set(&mut server
            .npcs
            .get_mut(index)
            .expect("just spawned")
            .buffs
            .flags);
        server.tick_town_casualties();
        server.npcs.get(index).expect("still there").defense
    }

    /// Dryad's Blessing raises the base armour before the world's progression bonus is added:
    /// `defense = (dryadWard ? (defDefense + 20/15/10) : defDefense)` by difficulty
    /// (`NPC.cs:53550-53561`), inside `AI_007_TownEntities` so it reaches town NPCs and critters
    /// and nothing else. Classic is the +10 arm.
    ///
    /// Before 2026-08-31 the `dryad_ward` flag was derived and read nowhere, so a blessed
    /// townsperson was exactly as easy to kill as an unblessed one.
    #[test]
    fn dryads_blessing_is_worth_ten_armour_on_classic() {
        let plain = guide_with(|_| {});
        let blessed = guide_with(|f| f.dryad_ward = true);
        assert_eq!(blessed, plain + 10, "classic's arm of NPC.cs:53560");
    }

    /// Tipsy multiplies the finished figure rather than the base, and truncates:
    /// `defense = (int)((double)defense * 1.1)` (`NPC.cs:53701`), inside `isLikeATownNPC`.
    #[test]
    fn tipsy_multiplies_the_finished_armour_by_a_tenth() {
        let plain = guide_with(|_| {});
        let tipsy = guide_with(|f| f.tipsy = true);
        assert_eq!(tipsy, (f64::from(plain) * 1.1) as i32);
        assert!(tipsy > plain, "the guide has armour to multiply");
    }
}

#[cfg(test)]
mod worm_tests {
    use crate::game::npc::NpcStore;
    use terrustia_proto::npc_params::EATER_OF_WORLDS;

    /// Resolve the chains the way the server does, without needing a running server.
    ///
    /// Kept in step with `GameServer::resolve_worm_chains` by testing the same rules: this is the
    /// decision table, and the server method is that table plus broadcasting.
    fn resolve(store: &mut NpcStore) {
        use terrustia_proto::npc_params::splitting_worm;
        let followed: std::collections::HashSet<u8> =
            store.iter().filter_map(|(_, npc)| npc.follows).collect();
        let mut transformed = Vec::new();
        let mut orphaned = Vec::new();
        for (index, npc) in store.iter() {
            let Some((head, body, tail)) = splitting_worm(npc.npc_type) else {
                continue;
            };
            let has_leader = npc.follows.is_some_and(|l| store.get(l).is_some());
            let has_follower = followed.contains(&index);
            if !has_leader && !has_follower {
                orphaned.push(index);
            } else if npc.npc_type == body && !has_leader {
                transformed.push((index, head));
            } else if npc.npc_type == body && !has_follower {
                transformed.push((index, tail));
            } else if (npc.npc_type == head && !has_follower)
                || (npc.npc_type == tail && !has_leader)
            {
                orphaned.push(index);
            }
        }
        for (index, into) in transformed {
            if let Some(npc) = store.get_mut(index) {
                let follows = npc.follows;
                npc.become_type(into);
                npc.follows = follows;
            }
        }
        for index in orphaned {
            store.remove(index);
        }
    }

    fn eater(store: &mut NpcStore, segments: usize) -> Vec<u8> {
        let (head, body, tail) = EATER_OF_WORLDS;
        store.spawn_worm(head, body, tail, segments, (1000.0, 1000.0));
        store.iter().map(|(index, _)| index).collect()
    }

    /// Cut one in half and you have two worms, not a worm with a hole in it.
    #[test]
    fn cutting_an_eater_of_worlds_makes_two_of_them() {
        let mut store = NpcStore::new();
        let chain = eater(&mut store, 6);
        assert!(chain.len() >= 5);

        // Take a segment out of the middle.
        let cut = chain[3];
        store.remove(cut);
        resolve(&mut store);

        let (head, _, tail) = EATER_OF_WORLDS;
        let heads = store.iter().filter(|(_, n)| n.npc_type == head).count();
        let tails = store.iter().filter(|(_, n)| n.npc_type == tail).count();
        assert_eq!(heads, 2, "the piece behind the wound grows a head");
        assert_eq!(tails, 2, "and the piece ahead of it grows a tail");
    }

    #[test]
    fn a_single_leftover_segment_dies_rather_than_becoming_a_worm() {
        let mut store = NpcStore::new();
        let chain = eater(&mut store, 3);
        // Leave exactly one segment standing.
        for index in chain.iter().skip(1) {
            store.remove(*index);
        }
        resolve(&mut store);
        assert_eq!(store.len(), 0, "one segment is not a worm");
    }

    #[test]
    fn an_intact_worm_is_left_alone() {
        let mut store = NpcStore::new();
        let chain = eater(&mut store, 6);
        let before: Vec<u16> = store.iter().map(|(_, n)| n.npc_type).collect();
        resolve(&mut store);
        let after: Vec<u16> = store.iter().map(|(_, n)| n.npc_type).collect();
        assert_eq!(before, after);
        assert_eq!(store.len(), chain.len());
    }

    /// Only the Eater does this. Cut a giant worm and the pieces simply die.
    #[test]
    fn other_worms_do_not_split() {
        let mut store = NpcStore::new();
        // Giant worm: head 10, body 11, tail 12.
        store.spawn_worm(10, 11, 12, 5, (1000.0, 1000.0));
        let chain: Vec<u8> = store.iter().map(|(index, _)| index).collect();
        store.remove(chain[2]);
        let before = store.len();
        resolve(&mut store);
        assert_eq!(store.len(), before, "nothing should have changed");
        assert!(
            store
                .iter()
                .all(|(_, n)| n.npc_type != 10 || n.follows.is_none())
        );
    }
}

/// The wire-flood consumer resolves buried mines and doors it reaches, not only the traps and
/// statues it always has. Each of these fails on the pre-fix `apply_circuit`, which read
/// `Fired::traps`/`statues`/… but dropped `mines`, `land_mines`, and `doors` on the floor — so a
/// wired mine never went off and a wired door never moved.
#[cfg(test)]
mod wired_mines_and_doors {
    use super::*;
    use crate::config::Config;
    use crate::world::doors::{DOOR_CLOSED, DOOR_OPEN};
    use crate::world::wiring::Fired;

    fn server() -> GameServer {
        GameServer::new(
            Config::default(),
            crate::world::World::empty(200, 150, "wire probe"),
        )
    }

    fn threw(server: &GameServer, projectile_type: u16) -> bool {
        server
            .projectiles
            .iter()
            .any(|(_, p)| p.projectile_type == projectile_type)
    }

    /// A Boulder Statue the current reached drops its boulder, and then does not drop another until
    /// its nine-hundred-frame `CheckMech` runs out (`Wiring.cs:1998-2017`).
    ///
    /// Fails before the fix: `Fired` had no `boulder_statues` at all and `apply_circuit` had nothing
    /// to read, so tile 531 was inert.
    #[test]
    fn a_wired_boulder_statue_drops_one_boulder_and_then_waits() {
        let mut server = server();
        let mut fired = Fired::default();
        fired.boulder_statues.push((100, 100));
        server.apply_circuit(fired, (100, 100));
        assert!(
            threw(&server, 99),
            "a Boulder Statue should throw a boulder"
        );
        let after_one = server.projectiles.iter().count();

        let mut fired = Fired::default();
        fired.boulder_statues.push((100, 100));
        server.apply_circuit(fired, (100, 100));
        assert_eq!(
            server.projectiles.iter().count(),
            after_one,
            "and not another one while it is still cooling down"
        );
    }

    /// A wired cannon fires its shell, and then takes *two* lockouts at once: the world-level one
    /// that stops every other Cannon in the world for 120 frames (`Wiring.cs:1335`), and its own
    /// 480-frame `CheckMech` window (`:1318`).
    ///
    /// Both matter and they are different: without the world lockout a bank of cannons on one
    /// circuit fires as a single volley instead of in sequence.
    ///
    /// Fails before the fix: `Fired` had no `cannons`, so tile 209 was inert; and with only the
    /// per-tile window, the second cannon here fires alongside the first.
    #[test]
    fn a_wired_cannon_locks_out_every_other_cannon_in_the_world() {
        let mut server = server();
        // Two plain Cannons (style 0) aimed level to the right, far apart.
        for ax in [100i32, 140] {
            for dx in 0..4 {
                for dy in 0..3 {
                    server.world.set_tile(
                        ax + dx,
                        100 + dy,
                        Tile::framed(209, (dx * 18) as i16, (dy * 18) as i16),
                    );
                }
            }
        }
        let mut fired = Fired::default();
        fired.cannons.push((100, 100));
        server.apply_circuit(fired, (100, 100));
        assert_eq!(
            server.projectiles.iter().count(),
            1,
            "the first cannon should fire a cannonball"
        );
        assert!(threw(&server, 162), "and it should be projectile 162");

        // The other cannon, which has its own untouched `CheckMech` window, is still locked out by
        // the world-level counter.
        let mut fired = Fired::default();
        fired.cannons.push((140, 100));
        server.apply_circuit(fired, (140, 100));
        assert_eq!(
            server.projectiles.iter().count(),
            1,
            "the second cannon is inside the world lockout"
        );

        // Wind the world lockout out and it fires; the first one is still inside its own window.
        for _ in 0..130 {
            server.tick_mech_cooldowns();
        }
        let mut fired = Fired::default();
        fired.cannons.push((140, 100));
        server.apply_circuit(fired, (140, 100));
        assert_eq!(
            server.projectiles.iter().count(),
            2,
            "and fires once the world lockout has run out"
        );
    }

    /// The Bunny Cannon stops at four Explosive Bunnies about, counting live ones and shells still
    /// in the air together (`WorldGen.BunnyCannonCanFire`, `WorldGen.cs:51158-51199`).
    ///
    /// Without it a Bunny Cannon on a timer fills the world; it is the only cannon with a population
    /// check of its own rather than only a clock.
    ///
    /// Fails before the fix: nothing called it, so a wired Bunny Cannon fired regardless.
    #[test]
    fn a_bunny_cannon_stops_at_four_bunnies() {
        /// `NPCID.ExplosiveBunny`.
        const EXPLOSIVE_BUNNY: u16 = 614;

        let mut server = server();
        assert!(
            server.bunny_cannon_can_fire(),
            "an empty world lets one through"
        );
        for n in 0..3 {
            server
                .npcs
                .spawn(EXPLOSIVE_BUNNY, (1000.0 + n as f32 * 32.0, 1000.0))
                .expect("a free slot");
        }
        assert!(
            server.bunny_cannon_can_fire(),
            "three is still under the ceiling"
        );
        server
            .npcs
            .spawn(EXPLOSIVE_BUNNY, (1200.0, 1000.0))
            .expect("a free slot");
        assert!(!server.bunny_cannon_can_fire(), "four is the ceiling");
    }

    /// A sundial and a moondial the current reached jump the world clock, through the same
    /// `skip_to` a direct click already goes through (`Wiring.cs:1137-1176`).
    ///
    /// Fails before the fix: `Fired` had no `sundial`/`moondial` flags, so a wired dial did nothing
    /// at all and the clock stayed where it was.
    #[test]
    fn a_wired_sundial_and_moondial_jump_the_clock() {
        let mut server = server();
        server.world.day_time = false;
        server.world.time = 12_345;
        let mut fired = Fired::default();
        fired.sundial = true;
        server.apply_circuit(fired, (100, 100));
        assert!(server.world.day_time, "the sundial should bring the day");
        assert_eq!(server.world.time, 0, "and start it at dawn");

        server.world.time = 6_789;
        let mut fired = Fired::default();
        fired.moondial = true;
        server.apply_circuit(fired, (100, 100));
        assert!(
            !server.world.day_time,
            "the moondial should bring the night"
        );
        assert_eq!(server.world.time, 0, "and start it at dusk");
    }

    /// The real world hands the flood its own surface line and Plantera flag, so
    /// `actuation_allowed`'s Lihzahrd guard actually lifts once the boss is down.
    ///
    /// Fails before the fix: `impl WiredWorld for World` implemented only the four required methods
    /// and took the trait's own deliberately-conservative defaults for the other two (surface at row
    /// zero, Plantera never down). The guard was therefore permanently shut on every real server: a
    /// temple wall stayed unactuatable for ever, including long after Plantera had fallen. The
    /// trait's doc had said all along that a real implementation should override them; the only real
    /// implementation never did.
    #[test]
    fn a_real_world_lifts_the_lihzahrd_guard_once_plantera_is_down() {
        /// `TileID.LihzahrdBrick`.
        const LIHZAHRD_BRICK: u16 = 226;

        let mut server = server();
        // Well below the surface, which a real world reports and a defaulted one calls row zero.
        let y = i32::from(server.world.surface) + 20;
        let lay = |server: &mut GameServer| {
            server.world.set_tile(100, y, wiring_switch());
            for x in 101..105 {
                let mut wire = Tile::AIR;
                wire.flags.set(TileFlags::WIRE_RED, true);
                server.world.set_tile(x, y, wire);
            }
            let mut wall = Tile::block(LIHZAHRD_BRICK);
            wall.flags.set(TileFlags::WIRE_RED, true);
            wall.flags.set(TileFlags::ACTUATOR, true);
            server.world.set_tile(105, y, wall);
        };

        lay(&mut server);
        crate::world::wiring::hit_switch(&mut server.world, 100, y);
        assert!(
            !server.world.tile(105, y).flags.has(TileFlags::ACTUATED),
            "the temple's wall should hold while Plantera is up"
        );

        server.world.progress.downed_plantera = true;
        lay(&mut server);
        crate::world::wiring::hit_switch(&mut server.world, 100, y);
        assert!(
            server.world.tile(105, y).flags.has(TileFlags::ACTUATED),
            "and give way once she is down"
        );
    }

    /// A plain red-wired Switch (tile 136), which is what every board here starts a circuit with.
    fn wiring_switch() -> Tile {
        let mut switch = Tile::framed(136, 0, 0);
        switch.flags.set(TileFlags::WIRE_RED, true);
        switch
    }

    #[test]
    fn a_wired_land_mine_throws_its_explosion() {
        // The flood has already cleared the mine tile, so the consumer owes only the projectile —
        // `ExplodeMine`'s type 164.
        let mut server = server();
        let mut fired = Fired::default();
        fired.land_mines.push((100, 100));
        server.apply_circuit(fired, (100, 100));
        assert!(
            threw(&server, 164),
            "a land mine a circuit reaches should throw projectile 164"
        );
    }

    #[test]
    fn a_wired_explosive_kills_its_tile_and_throws_a_bomb() {
        // Unlike a land mine, the flood leaves the Explosives tile (141) standing, so the consumer
        // both kills it and throws projectile 108 — `Wiring.cs`'s `case 141`.
        let mut server = server();
        server.world.set_tile(100, 100, Tile::framed(141, 0, 0));
        let mut fired = Fired::default();
        fired.mines.push((100, 100));
        server.apply_circuit(fired, (100, 100));
        assert!(
            !server.world.tile(100, 100).is_active(),
            "the Explosives tile should be gone (case 141 KillTile)"
        );
        assert!(threw(&server, 108), "and it should throw projectile 108");
    }

    #[test]
    fn a_wired_door_opens_then_shuts_on_the_next_pulse() {
        let mut server = server();
        for dy in 0..3 {
            server.world.set_tile(
                100,
                100 + dy,
                Tile::framed(DOOR_CLOSED, 0, (dy as i16) * 18),
            );
        }
        // First pulse: the shut door swings open.
        let mut fired = Fired::default();
        fired.doors.push((100, 101));
        server.apply_circuit(fired, (100, 101));
        assert_eq!(
            server.world.tile(100, 100).block,
            DOOR_OPEN,
            "a circuit reaching a shut door opens it"
        );
        // Second pulse: the now-open door is forced shut.
        let mut fired = Fired::default();
        fired.doors.push((100, 101));
        server.apply_circuit(fired, (100, 101));
        assert_eq!(
            server.world.tile(100, 100).block,
            DOOR_CLOSED,
            "and a circuit reaching it again forces it shut"
        );
    }

    #[test]
    fn a_resident_will_not_shut_a_door_on_someone_standing_in_it() {
        let mut server = server();
        // A shut door, opened so there is an open door to close again.
        for dy in 0..3 {
            server
                .world
                .set_tile(100, 50 + dy, Tile::framed(DOOR_CLOSED, 0, (dy as i16) * 18));
        }
        assert!(
            crate::world::doors::open(&mut server.world, 100, 50, 1),
            "the door should open"
        );
        // Someone stands in the doorway.
        server
            .npcs
            .spawn(1, (100.0 * 16.0, 50.0 * 16.0))
            .expect("a slime should spawn");

        // The town-NPC path is unforced, so `Collision.EmptyTile` refuses while the slime is there.
        server.close_door(100, 50, false);
        assert!(
            server.world.tile(100, 50).block == DOOR_OPEN
                || server.world.tile(101, 50).block == DOOR_OPEN,
            "an unforced close is blocked by the occupant"
        );

        // A wire signal forces it shut regardless.
        server.close_door(100, 50, true);
        assert!(
            server.world.tile(100, 50).block != DOOR_OPEN
                && server.world.tile(101, 50).block != DOOR_OPEN,
            "a forced (wired) close ignores the occupant"
        );
    }

    #[test]
    fn cut_wire_drops_become_world_pickups() {
        // What a mass-wire cut reports back: a wire at one tile, an actuator at the next. The
        // pre-fix `on_mass_wire` ignored `Outcome::drops`, so a mistake with the Grand Design
        // destroyed the materials outright.
        let mut server = server();
        server.spawn_wire_drops(&[(50, 50, WIRE_ITEM as u16), (51, 50, ACTUATOR_ITEM as u16)]);
        let mut found: Vec<(i32, (f32, f32))> = server
            .items
            .iter()
            .map(|(_, it)| (it.item.id, it.position))
            .collect();
        found.sort_by_key(|(id, _)| *id);
        assert_eq!(found.len(), 2, "both cuts should drop a pickup");
        assert_eq!(found[0].0, i32::from(WIRE_ITEM));
        assert_eq!(found[1].0, i32::from(ACTUATOR_ITEM));
        assert_eq!(
            found[0].1,
            (50.0 * 16.0 + 8.0, 50.0 * 16.0 + 8.0),
            "dropped at the cut tile's own centre"
        );
    }

    /// A clicked Detonator is momentary: it presses down, and pops back up once its window runs
    /// out, driven end to end through `hit_switch` (the press and the report), `apply_circuit` (the
    /// registration) and `tick_detonators` (the `UpdateMech` reset) (L3-26).
    ///
    /// Fails before the fix: the Detonator latched like a Lever with nothing to release it, so it
    /// stayed pressed forever — `tick_detonators` and the report it consumes did not exist.
    #[test]
    fn a_wired_detonator_pops_back_up_after_its_window() {
        let mut server = server();
        // An unpressed 2x2 Detonator, anchor at (100,100): frameX is the column, frameY the row.
        for dx in 0..2i32 {
            for dy in 0..2i32 {
                let mut cell = Tile::framed(411, (dx * 18) as i16, (dy * 18) as i16);
                cell.flags.set(TileFlags::WIRE_RED, true);
                server.world.set_tile(100 + dx, 100 + dy, cell);
            }
        }
        let fired = crate::world::wiring::hit_switch(&mut server.world, 100, 100);
        server.apply_circuit(fired, (100, 100));
        assert_eq!(
            server.world.tile(100, 100).frame_x,
            36,
            "the click presses it down"
        );

        for _ in 0..DETONATOR_WINDOW - 1 {
            server.tick_detonators();
        }
        assert_eq!(
            server.world.tile(100, 100).frame_x,
            36,
            "still down inside its window"
        );
        server.tick_detonators();
        assert_eq!(
            server.world.tile(100, 100).frame_x,
            0,
            "and pops back up when the window runs out"
        );
    }
}

/// Trapdoor and tall-gate wiring (C1-b item 4): `Fired::trapdoors`/`Fired::gates` used to be
/// reported by the flood and then dropped on the floor here, the same way `doors` once was — see
/// `wired_mines_and_doors`'s own doc comment for that precedent. `world::trapdoors` now has the
/// real `ShiftTrapdoor`/`ShiftTallGate` logic; these drive it end to end through `apply_circuit`.
#[cfg(test)]
mod wired_trapdoors_and_gates {
    use super::*;
    use crate::config::Config;
    use crate::world::trapdoors::{
        TALL_GATE_CLOSED, TALL_GATE_OPEN, TRAPDOOR_CLOSED, TRAPDOOR_OPEN,
    };
    use crate::world::wiring::Fired;

    fn server() -> GameServer {
        GameServer::new(
            Config::default(),
            crate::world::World::empty(200, 150, "trapdoor/gate wire probe"),
        )
    }

    fn shut_trapdoor(server: &mut GameServer, x: i32, y: i32) {
        for dx in 0..2i32 {
            for dy in 0..2i32 {
                server.world.set_tile(
                    x + dx,
                    y + dy,
                    Tile::framed(TRAPDOOR_CLOSED, dx as i16 * 18, dy as i16 * 18),
                );
            }
        }
    }

    #[test]
    fn a_wired_trapdoor_opens_then_shuts_on_the_next_pulse() {
        let mut server = server();
        shut_trapdoor(&mut server, 100, 100);

        let mut fired = Fired::default();
        fired.trapdoors.push((100, 100));
        server.apply_circuit(fired, (100, 100));
        assert!(
            server.world.tile(100, 100).block == TRAPDOOR_OPEN
                || server.world.tile(100, 101).block == TRAPDOOR_OPEN,
            "a circuit reaching a shut trapdoor should open it"
        );

        let opened_at_row = if server.world.tile(100, 100).block == TRAPDOOR_OPEN {
            100
        } else {
            101
        };
        let mut fired = Fired::default();
        fired.trapdoors.push((100, opened_at_row));
        server.apply_circuit(fired, (100, opened_at_row));
        assert_eq!(
            server.world.tile(100, opened_at_row).block,
            TRAPDOOR_CLOSED,
            "and a circuit reaching it again shuts it"
        );
    }

    /// The fail-then-pass case: nothing shifts unless the circuit reaches whichever tile actually
    /// holds the trapdoor, matching real vanilla's own retry logic rather than always succeeding.
    #[test]
    fn nothing_happens_where_there_is_no_trapdoor() {
        let mut server = server();
        let mut fired = Fired::default();
        fired.trapdoors.push((100, 100));
        server.apply_circuit(fired, (100, 100));
        assert!(!server.world.tile(100, 100).is_active());
    }

    /// Someone standing where the doorway would open blocks a wired trapdoor from opening at all
    /// — `Collision.EmptyTile(..., ignoreTiles: true)`'s own entity-only check, which has no
    /// `forced` override the way a door's own wired close does.
    #[test]
    fn a_wired_trapdoor_will_not_open_onto_someone_standing_there() {
        let mut server = server();
        shut_trapdoor(&mut server, 100, 100);
        // Both possible doorway rows (100 and 101) are occupied, so neither of ShiftTrapdoor's
        // two attempts (`player_above: true` then `false`) can succeed.
        server
            .npcs
            .spawn(1, (100.0 * 16.0, 100.0 * 16.0))
            .expect("a slime should spawn");
        server
            .npcs
            .spawn(1, (100.0 * 16.0, 101.0 * 16.0))
            .expect("a slime should spawn");

        let mut fired = Fired::default();
        fired.trapdoors.push((100, 100));
        server.apply_circuit(fired, (100, 100));

        assert_eq!(
            server.world.tile(100, 100).block,
            TRAPDOOR_CLOSED,
            "still shut: both landing rows are occupied"
        );
    }

    fn tall_gate(server: &mut GameServer, x: i32, y: i32, block: u16) {
        let heights = [18i16, 16, 16, 16, 18];
        let mut frame_y = 0i16;
        for (row, &height) in heights.iter().enumerate() {
            server
                .world
                .set_tile(x, y + row as i32, Tile::framed(block, 0, frame_y));
            frame_y += height;
        }
    }

    #[test]
    fn a_wired_tall_gate_opens_then_shuts_on_the_next_pulse() {
        let mut server = server();
        tall_gate(&mut server, 100, 100, TALL_GATE_CLOSED);

        let mut fired = Fired::default();
        fired.gates.push((100, 102)); // hitting a middle row of the five
        server.apply_circuit(fired, (100, 102));
        for row in 0..5 {
            assert_eq!(server.world.tile(100, 100 + row).block, TALL_GATE_OPEN);
        }

        let mut fired = Fired::default();
        fired.gates.push((100, 100));
        server.apply_circuit(fired, (100, 100));
        for row in 0..5 {
            assert_eq!(server.world.tile(100, 100 + row).block, TALL_GATE_CLOSED);
        }
    }

    /// An unforced wired close still refuses while something stands in the gate's own column —
    /// real vanilla's own wire trigger never passes `forced` for a tall gate either.
    #[test]
    fn a_wired_tall_gate_will_not_close_on_someone_standing_in_it() {
        let mut server = server();
        tall_gate(&mut server, 100, 100, TALL_GATE_OPEN);
        server
            .npcs
            .spawn(1, (100.0 * 16.0, 102.0 * 16.0))
            .expect("a slime should spawn");

        let mut fired = Fired::default();
        fired.gates.push((100, 100));
        server.apply_circuit(fired, (100, 100));

        assert_eq!(
            server.world.tile(100, 100).block,
            TALL_GATE_OPEN,
            "still open: something stands in the column"
        );
    }
}

/// A meteor refuses a landing site that overlaps a player or an NPC — the `blocked_by_entity`
/// closure `land_meteor` now feeds `meteor::drop_checked`, which the pre-fix code (a bare
/// `meteor::drop`) had no way to express, so a meteor could bury the player who summoned it.
#[cfg(test)]
mod meteor_entity_safety {
    use super::*;
    use crate::config::Config;

    fn server() -> GameServer {
        GameServer::new(
            Config::default(),
            crate::world::World::empty(400, 300, "meteor probe"),
        )
    }

    /// The open-interval overlap: a shared edge is not an overlap.
    #[test]
    fn touching_boxes_do_not_count_as_overlap() {
        assert!(boxes_overlap(
            (0.0, 0.0, 10.0, 10.0),
            (5.0, 5.0, 15.0, 15.0)
        ));
        assert!(!boxes_overlap(
            (0.0, 0.0, 10.0, 10.0),
            (10.0, 0.0, 20.0, 10.0)
        ));
        assert!(!boxes_overlap(
            (0.0, 0.0, 10.0, 10.0),
            (20.0, 20.0, 30.0, 30.0)
        ));
    }

    #[test]
    fn a_meteor_will_not_land_on_an_npc() {
        let mut server = server();
        let at = (2000.0, 2000.0);
        server.npcs.spawn(1, at).expect("a slime should spawn");

        let boxes = server.meteor_entity_boxes();
        assert_eq!(boxes.len(), 1, "one NPC, no players");
        let npc_box = boxes[0];
        assert_eq!((npc_box.0, npc_box.1), at, "the box starts at the NPC");

        // The 35-tile strike box centred on the slime's tile overlaps it and is refused...
        let (tx, ty) = ((at.0 / 16.0) as i32, (at.1 / 16.0) as i32);
        let strike = |cx: i32, cy: i32| {
            (
                ((cx - 35) * 16) as f32,
                ((cy - 35) * 16) as f32,
                ((cx + 35) * 16) as f32,
                ((cy + 35) * 16) as f32,
            )
        };
        assert!(
            boxes_overlap(strike(tx, ty), npc_box),
            "a strike on the slime is blocked"
        );
        // ...but one a couple of hundred tiles away is fine.
        assert!(
            !boxes_overlap(strike(tx + 200, ty), npc_box),
            "a strike far from the slime is allowed"
        );
    }
}

/// Server MINOR (C1-b item 5): `broadcast_npc_buffs` used to go through `broadcast_near`, the
/// same distance-gated path an ordinary position sync takes, and could withhold the buff list
/// from a distant player for several ticks running (`MAX_NPC_SYNC_SKIPS`). Every real send site
/// for this packet (`NPC.cs:81959`, `91090`, `91130`, `93029`) is `SendData(54, -1, -1, ...)` —
/// no proximity check anywhere in source — so this is a real, unconditional broadcast now.
#[cfg(test)]
mod npc_buff_broadcast_scope {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(4000, 1200, "npc buff broadcast probe")
    }

    #[test]
    fn a_far_away_player_still_hears_an_npcs_buff_change_immediately() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        const ZOMBIE: u16 = 3;
        let index = server
            .npcs
            .spawn(ZOMBIE, (0.0, 0.0))
            .expect("a slot for a zombie");

        let (out_tx, mut out_rx) = mpsc::channel(16);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = ConnState::Playing;
        // Comfortably outside `SECTION_REACH`'s one-section radius of the NPC at the origin.
        player.position = (3000.0 * 16.0, 0.0);
        server.players[0] = Some(player);

        let npc = server.npcs.get_mut(index).expect("just spawned");
        assert!(
            npc.buffs.add(ZOMBIE, 20, 600),
            "adding Poisoned should succeed"
        );

        server.broadcast_npc_buffs(index);

        let frame = out_rx.try_recv();
        assert!(
            frame.is_ok(),
            "a distant player should still hear about the buff on the very first call, \
             not have it withheld the way an ordinary position sync would be"
        );
        assert_eq!(frame.unwrap()[2], terrustia_proto::id::N_P_C_BUFFS);
    }
}

/// The area-of-interest cull, on the two things that arrive every tick rather than every sixth.
///
/// Vanilla relays a player's movement to every other player and a client's projectile syncs the
/// same way, which is `max_players - 1` sends per source per tick: the quadratic fan-out that fills
/// outbound queues and gets slow clients dropped. Culling by loaded section is a deliberate
/// departure, bounded by a skip budget so nothing distant freezes outright.
#[cfg(test)]
mod area_of_interest_cull {
    use super::*;
    use crate::config::Config;

    fn world() -> crate::world::World {
        crate::world::World::empty(4000, 1200, "area of interest probe")
    }

    /// Seat a playing player and hand back the channel its frames land in. The receiver must be
    /// kept alive by the caller: a dropped one closes the channel, and `send_bytes` removes a
    /// player whose channel has closed.
    fn seat(server: &mut GameServer, slot: u8, position: (f32, f32)) -> mpsc::Receiver<Bytes> {
        let (out_tx, out_rx) = mpsc::channel(256);
        let addr = format!("127.0.0.1:{}", u16::from(slot) + 1);
        let mut player = Player::new(
            slot,
            addr.parse().expect("a valid loopback address"),
            out_tx,
        );
        player.state = ConnState::Playing;
        player.position = position;
        server.players[slot as usize] = Some(player);
        out_rx
    }

    /// One section is 200 by 150 tiles and `SECTION_REACH` is one, so this is comfortably outside.
    const FAR: (f32, f32) = (3000.0 * 16.0, 0.0);
    const ORIGIN: (f32, f32) = (0.0, 0.0);

    #[test]
    fn a_player_in_the_same_part_of_the_world_gets_every_movement_update() {
        let mut server = GameServer::new(Config::default(), world());
        let _mover = seat(&mut server, 0, ORIGIN);
        let mut near = seat(&mut server, 1, ORIGIN);

        for _ in 0..10 {
            server.broadcast_near(
                vec![0, 0, terrustia_proto::id::PLAYER_CONTROLS],
                ORIGIN,
                Withheld::Player(0),
                MAX_PLAYER_SYNC_SKIPS,
                Some(0),
            );
        }

        let mut got = 0;
        while near.try_recv().is_ok() {
            got += 1;
        }
        assert_eq!(
            got, 10,
            "somebody standing next to the mover is never culled"
        );
    }

    #[test]
    fn a_distant_player_is_not_sent_every_movement_update() {
        let mut server = GameServer::new(Config::default(), world());
        let _mover = seat(&mut server, 0, ORIGIN);
        let mut far = seat(&mut server, 1, FAR);

        // One full budget's worth. Every one of these is withheld.
        for _ in 0..MAX_PLAYER_SYNC_SKIPS {
            assert!(
                server.broadcast_near(
                    vec![0, 0, terrustia_proto::id::PLAYER_CONTROLS],
                    ORIGIN,
                    Withheld::Player(0),
                    MAX_PLAYER_SYNC_SKIPS,
                    Some(0),
                ),
                "a player nowhere near the mover is skipped, and the caller is told so"
            );
        }
        assert!(
            far.try_recv().is_err(),
            "nothing should have reached a player {} sections away yet",
            (FAR.0 / 16.0) as i32 / terrustia_proto::section::SECTION_WIDTH
        );
    }

    #[test]
    fn a_distant_player_is_not_left_frozen() {
        let mut server = GameServer::new(Config::default(), world());
        let _mover = seat(&mut server, 0, ORIGIN);
        let mut far = seat(&mut server, 1, FAR);

        for _ in 0..MAX_PLAYER_SYNC_SKIPS {
            server.broadcast_near(
                vec![0, 0, terrustia_proto::id::PLAYER_CONTROLS],
                ORIGIN,
                Withheld::Player(0),
                MAX_PLAYER_SYNC_SKIPS,
                Some(0),
            );
        }
        // The budget is spent, so this one goes out: the marker on a distant player's map keeps
        // moving, it just moves in steps.
        server.broadcast_near(
            vec![0, 0, terrustia_proto::id::PLAYER_CONTROLS],
            ORIGIN,
            Withheld::Player(0),
            MAX_PLAYER_SYNC_SKIPS,
            Some(0),
        );
        let frame = far.try_recv();
        assert!(
            frame.is_ok(),
            "after {MAX_PLAYER_SYNC_SKIPS} withheld updates the next one is sent anyway"
        );
        assert_eq!(frame.unwrap()[2], terrustia_proto::id::PLAYER_CONTROLS);
    }

    #[test]
    fn a_projectile_is_culled_on_the_npc_budget_not_the_player_one() {
        let mut server = GameServer::new(Config::default(), world());
        let _owner = seat(&mut server, 0, ORIGIN);
        let mut far = seat(&mut server, 1, FAR);

        let key = terrustia_proto::projectile::ProjectileKey {
            owner: 0,
            index: 7,
            generation: 1,
        };
        for _ in 0..MAX_NPC_SYNC_SKIPS {
            server.broadcast_near(
                vec![0, 0, terrustia_proto::id::SYNC_PROJECTILE],
                ORIGIN,
                Withheld::Projectile(key.pack()),
                MAX_NPC_SYNC_SKIPS,
                Some(0),
            );
        }
        assert!(far.try_recv().is_err(), "the budget is not spent yet");

        server.broadcast_near(
            vec![0, 0, terrustia_proto::id::SYNC_PROJECTILE],
            ORIGIN,
            Withheld::Projectile(key.pack()),
            MAX_NPC_SYNC_SKIPS,
            Some(0),
        );
        assert!(
            far.try_recv().is_ok(),
            "a projectile uses the game's own four-skip rule, not the movement budget"
        );
    }

    #[test]
    fn a_dead_projectiles_skip_runs_are_not_kept_for_ever() {
        let mut server = GameServer::new(Config::default(), world());
        let _owner = seat(&mut server, 0, ORIGIN);
        let _far = seat(&mut server, 1, FAR);

        let key = terrustia_proto::projectile::ProjectileKey {
            owner: 0,
            index: 7,
            generation: 1,
        };
        let what = Withheld::Projectile(key.pack());
        server.broadcast_near(
            vec![0, 0, terrustia_proto::id::SYNC_PROJECTILE],
            ORIGIN,
            what,
            MAX_NPC_SYNC_SKIPS,
            Some(0),
        );
        assert!(
            !server.skips.is_empty(),
            "the distant player's skip run was recorded"
        );

        server.forget_skips(what);
        assert!(
            server.skips.is_empty(),
            "a projectile's identity carries a climbing generation, so its entries have to go \
             when it dies or the ledger grows for the life of the server"
        );
    }

    #[test]
    fn a_departing_player_leaves_no_skip_run_behind_for_the_next_occupant() {
        let mut server = GameServer::new(Config::default(), world());
        let _mover = seat(&mut server, 0, ORIGIN);
        let far = seat(&mut server, 1, FAR);

        server.broadcast_near(
            vec![0, 0, terrustia_proto::id::PLAYER_CONTROLS],
            ORIGIN,
            Withheld::Player(0),
            MAX_PLAYER_SYNC_SKIPS,
            Some(0),
        );
        assert!(!server.skips.is_empty(), "slot 1 has a run against it");

        drop(far);
        server.remove_player(1);
        assert!(
            server.skips.is_empty(),
            "slots are reused, so whoever joins as slot 1 next must not inherit a spent budget"
        );
    }
}

/// Journey mode's `Godmode` actually blocks the one damage path this server decides on a
/// player's behalf — `hurt_player`'s own gate, mirroring the effect (not the client-side
/// mechanism) of `creativeGodMode` in source. Unconditional on the world's own difficulty,
/// deliberately unlike `FarPlacementRange`/`SpawnRate` below: `Player.cs`'s own
/// `creativeGodMode = true;` assignment has no `difficulty == 3` guard around it at all.
#[cfg(test)]
mod godmode {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "godmode probe")
    }

    /// The receiver has to stay alive for as long as the caller keeps using `server` — `broadcast`
    /// (`hurt_player`'s own `PlayerHurt`/`PlayerDeath`) removes a player whose send fails, and a
    /// dropped receiver closes the channel immediately regardless of its buffer size, not merely
    /// once that buffer fills. Returned rather than silently kept alive inside this function,
    /// which would only postpone the drop to *this* function's own return, not the caller's use.
    fn with_one_player(mut server: GameServer) -> (GameServer, mpsc::Receiver<Bytes>) {
        let (out_tx, out_rx) = mpsc::channel(16);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = ConnState::Playing;
        player.life = 100;
        player.life_max = 100;
        server.players[0] = Some(player);
        (server, out_rx)
    }

    #[test]
    fn godmode_takes_no_damage() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), tiny_world()));
        server.journey.set_godmode(0, true);

        server.hurt_player(0, 9999, 1, terrustia_proto::hurt::DeathReason::from_npc(0));

        assert_eq!(
            server.players[0].as_ref().unwrap().life,
            100,
            "life should be untouched while godmode is on"
        );
    }

    #[test]
    fn an_ordinary_player_takes_damage_normally() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), tiny_world()));
        // godmode left off — the control case, so the test above is proving something rather
        // than passing regardless of whether the gate exists at all.

        server.hurt_player(0, 30, 1, terrustia_proto::hurt::DeathReason::from_npc(0));

        assert_eq!(server.players[0].as_ref().unwrap().life, 70);
    }

    #[test]
    fn turning_godmode_off_again_lets_damage_through() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), tiny_world()));
        server.journey.set_godmode(0, true);
        server.hurt_player(0, 9999, 1, terrustia_proto::hurt::DeathReason::from_npc(0));
        assert_eq!(server.players[0].as_ref().unwrap().life, 100, "still on");

        server.journey.set_godmode(0, false);
        server.hurt_player(0, 30, 1, terrustia_proto::hurt::DeathReason::from_npc(0));
        assert_eq!(server.players[0].as_ref().unwrap().life, 70);
    }

    #[test]
    fn godmode_for_one_player_does_not_protect_another() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), tiny_world()));
        let (out_tx, _other_rx) = mpsc::channel(16);
        let mut other = Player::new(1, "127.0.0.1:2".parse().unwrap(), out_tx);
        other.state = ConnState::Playing;
        other.life = 100;
        other.life_max = 100;
        server.players[1] = Some(other);

        server.journey.set_godmode(0, true);
        server.hurt_player(1, 30, 1, terrustia_proto::hurt::DeathReason::from_npc(0));

        assert_eq!(
            server.players[1].as_ref().unwrap().life,
            70,
            "slot 1 was never given godmode"
        );
    }

    /// A hostile shot lands `base * hostileDamageScaling(difficulty) * 2` where the game lands it,
    /// at impact (`Projectile.cs:14916-14919`), and opens the full forty-tick window. The old path
    /// pre-scaled the projectile at launch and omitted the flat doubling, so a base-one classic shot
    /// delivered one where the game delivers two, and the immune window was a flat thirty.
    #[test]
    fn a_hostile_shot_does_double_damage_and_opens_the_full_window() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), tiny_world()));
        let pos = (1000.0, 1000.0);
        {
            let p = server.players[0].as_mut().unwrap();
            p.position = pos;
            p.immune_ticks = 0;
            p.life = 200;
            p.life_max = 200;
        }
        // A hostile Harpy Feather carrying one base damage, centred on the player.
        let centre = (
            pos.0 + crate::game::ai::PLAYER_WIDTH as f32 / 2.0,
            pos.1 + crate::game::ai::PLAYER_HEIGHT as f32 / 2.0,
        );
        server.projectiles.launch(38, centre, (0.0, 0.0), 1, 0);
        server.tick_contact_damage();
        let p = server.players[0].as_ref().unwrap();
        assert_eq!(p.life, 198, "classic: base 1 * difficulty 1 * flat 2 = 2");
        assert_eq!(p.immune_ticks, 40, "a real hit opens the full forty ticks");
    }

    /// The immune window follows the damage: a bare one-damage hit only opens twenty ticks, and a
    /// godmoded hit reports no strike at all so no touch debuff can follow it (`Player.cs:38672`,
    /// and the `StatusFromNPC` gate at `Player.cs:31659-31661`).
    #[test]
    fn the_immune_window_and_the_strike_report_follow_the_damage() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), tiny_world()));
        let chip = server.hurt_player(0, 1, 1, terrustia_proto::hurt::DeathReason::from_npc(0));
        assert!(chip, "the chip hit still landed");
        assert_eq!(server.players[0].as_ref().unwrap().immune_ticks, 20);

        server.journey.set_godmode(0, true);
        let blocked = server.hurt_player(0, 30, 1, terrustia_proto::hurt::DeathReason::from_npc(0));
        assert!(
            !blocked,
            "a godmoded hit reports no strike, so no debuff follows"
        );
    }

    /// BS3-B1: contact damage is the *live* number, so the phase multiplier a routine wrote
    /// actually lands. `Player.cs:31623` reads `Main.npc[i].damage`, which is exactly the field
    /// `NPC.cs:27938-27939` has just doubled for the Prime's spin, and Mothron's chase has just
    /// halved (`NPC.cs:38387`). This server keeps the fixed half in `stats.damage` and the routine's
    /// half in `damage_bonus`, and read only the first: every one of the thirteen production writes
    /// of `damage_bonus` was discarded. Reverting `contact_damage()` to `npc.stats.damage` turns
    /// both halves of this red - the doubled hit reads as a plain one, and the given-up Big Mimic
    /// still hits for full.
    #[test]
    fn contact_damage_carries_the_routines_phase_multiplier() {
        let (mut server, _rx) = with_one_player(GameServer::new(Config::default(), tiny_world()));
        let pos = (1000.0, 1000.0);
        server.players[0].as_mut().unwrap().position = pos;

        // A Demon Eye sitting on the player, mid-way through some phase that hits twice as hard.
        let index = server.npcs.spawn(2, pos).expect("a slot");
        let base = {
            let npc = server.npcs.get_mut(index).expect("just spawned");
            npc.damage_bonus = 2.0;
            npc.stats.damage
        };
        server.tick_contact_damage();
        let after_double = server.players[0].as_ref().unwrap().life;
        assert_eq!(
            100 - after_double,
            (base * 2) as i16,
            "a doubled phase has to hit for double"
        );

        // And a routine that has given up hits for nothing at all, the way a Big Mimic that has
        // stopped fighting does (`big_mimic.rs`, `damage_bonus = 0.0`).
        {
            let p = server.players[0].as_mut().unwrap();
            p.life = 100;
            p.immune_ticks = 0;
        }
        server
            .npcs
            .get_mut(index)
            .expect("still there")
            .damage_bonus = 0.0;
        server.tick_contact_damage();
        assert_eq!(
            server.players[0].as_ref().unwrap().life,
            100,
            "a zeroed phase does no damage"
        );
    }

    /// BS3-B1, the other half: a routine that names an absolute vanilla-normal figure is naming a
    /// *pre-scaling* one, because vanilla writes those through `GetAttackDamage_ScaledByDifficulty`
    /// (`NPC.cs:7063`), which applies the difficulty multiplier itself. Plantera's second form
    /// "hits for 70" (`NPC.cs:32209`), so an expert one hits for 140. Measuring the bonus against
    /// the already-scaled `stats.damage` instead of the table's raw damage cancels the world's
    /// difficulty straight back out and leaves it at a classic 70.
    #[test]
    fn an_absolute_phase_damage_still_scales_with_the_worlds_difficulty() {
        let mut classic = GameServer::new(Config::default(), tiny_world());
        let mut expert = GameServer::new(Config::default(), tiny_world());
        expert.world.game_mode = 1;
        for server in [&mut classic, &mut expert] {
            let difficulty = server.effective_difficulty();
            // Only difficulty and player count matter here; spread the rest so a later field
            // added to Scaling does not break a test that does not care about it.
            server.npcs.set_scaling(crate::game::npc::Scaling {
                difficulty,
                players: 1,
                ..Default::default()
            });
        }

        let hits = |server: &mut GameServer| {
            let index = server.npcs.spawn(262, (1000.0, 1000.0)).expect("Plantera");
            let npc = server.npcs.get_mut(index).expect("just spawned");
            npc.set_contact_damage(terrustia_proto::npc_params::PLANTERA_SECOND_DAMAGE);
            npc.contact_damage()
        };
        assert_eq!(hits(&mut classic), 70, "classic: the plain figure");
        assert_eq!(hits(&mut expert), 140, "expert: doubled by the difficulty");
    }

    /// An expert boss hands every player its own bag over packet 90 and drops no loose coins (they
    /// ride inside the bag). The old path broadcast one shared bag as an ordinary item and paid the
    /// coins on top, so on a two-player server one player got the bag and the boss double-paid.
    #[test]
    fn an_expert_boss_instances_a_bag_per_player_and_drops_no_loose_coins() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.world.game_mode = 1; // expert
        let mut rxs = Vec::new();
        for slot in 0u8..2 {
            let (tx, rx) = mpsc::channel(64);
            let mut p = Player::new(slot, format!("127.0.0.1:{}", slot + 1).parse().unwrap(), tx);
            p.state = ConnState::Playing;
            server.players[slot as usize] = Some(p);
            rxs.push(rx);
        }
        // Eye of Cthulhu (type 4, bag 3319), killed while carrying a fat coin value.
        let index = server
            .npcs
            .spawn(4, (1000.0, 1000.0))
            .expect("the eye spawns");
        server.npc_died(index, 4, (1000.0, 1000.0), 10_000.0);

        for (slot, rx) in rxs.iter_mut().enumerate() {
            let mut bag = false;
            let mut coin = false;
            while let Ok(frame) = rx.try_recv() {
                if frame.len() < 3 {
                    continue;
                }
                let item_id = i16::from_le_bytes([frame[frame.len() - 2], frame[frame.len() - 1]]);
                if frame[2] == terrustia_proto::id::SPAWN_INSTANCED_ITEM && item_id == 3319 {
                    bag = true;
                }
                if frame[2] == terrustia_proto::id::SYNC_ITEM && (71..=74).contains(&item_id) {
                    coin = true;
                }
            }
            assert!(
                bag,
                "player {slot} should be sent its own instanced treasure bag"
            );
            assert!(!coin, "an expert boss with a bag drops no loose coins");
        }
    }

    /// m5: an expert or master treasure bag is instanced to its one intended player, the way
    /// vanilla's `WorldItem.MakeInstanced` (`WorldItem.cs:326`) reserves it, so no other client can
    /// take it. Fails before the fix, when each bag was an un-owned world item and the proximity
    /// reservation loop handed it to whoever stood nearest the boss (`WorldItem.FindOwner` skips only
    /// instanced items, `WorldItem.cs:195`) - so a player parked on the drop point could be reserved
    /// every bag, including ones meant for others.
    #[test]
    fn an_instanced_bag_stays_owned_by_its_player_and_a_bystander_cannot_take_it() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.world.game_mode = 1; // expert
        let center = (1000.0, 1000.0);
        for slot in 0u8..2 {
            let (tx, _rx) = mpsc::channel(64);
            let mut p = Player::new(slot, format!("127.0.0.1:{}", slot + 1).parse().unwrap(), tx);
            p.state = ConnState::Playing;
            // Park player 1 right on the drop point and player 0 far away: were the bags un-owned,
            // the proximity loop would reserve every bag to the nearest player (player 1).
            p.position = if slot == 1 { center } else { (2800.0, 1000.0) };
            server.players[slot as usize] = Some(p);
        }

        server.drop_instanced_bag(3319, center);

        // One bag per player, each instanced to a distinct real owner, none left un-owned.
        let mut owners: Vec<u8> = server.items.iter().map(|(_, i)| i.owner).collect();
        owners.sort_unstable();
        assert_eq!(owners, vec![0, 1], "one bag instanced to each player");
        assert!(
            server.items.iter().all(|(_, i)| i.instanced),
            "every treasure bag must be instanced to its owner"
        );

        // The proximity reservation loop, with player 1 sitting on both bags, must not steal
        // player 0's bag: an instanced item is never re-offered to whoever is nearest.
        server.tick_items();
        let mut owners_after: Vec<u8> = server.items.iter().map(|(_, i)| i.owner).collect();
        owners_after.sort_unstable();
        assert_eq!(
            owners_after,
            vec![0, 1],
            "a bystander on the drop point must not be reserved another player's instanced bag"
        );
    }

    /// Coins are varied per roll, not paid at face (`NPC.NPCLoot_DropMoney`, `NPC.cs:80436`). The
    /// old path paid the exact value as one pile every time; the game rolls a -20%..+75% base plus
    /// jackpots, so different rolls of the same value pay different amounts.
    #[test]
    fn coin_drops_are_varied_not_paid_at_face() {
        use rand::SeedableRng;
        const FACE: f32 = 1000.0;
        let total_for = |seed: u64| -> i64 {
            let mut server = GameServer::new(Config::default(), tiny_world());
            let (tx, mut rx) = mpsc::channel(256);
            let mut p = Player::new(0, "127.0.0.1:1".parse().unwrap(), tx);
            p.state = ConnState::Playing;
            server.players[0] = Some(p);
            server.rng = rand::rngs::SmallRng::seed_from_u64(seed);
            server.drop_coins(FACE, (1000.0, 1000.0), false);
            let mut total = 0i64;
            while let Ok(frame) = rx.try_recv() {
                if frame.len() >= 23 && frame[2] == terrustia_proto::id::SYNC_ITEM {
                    let id = i16::from_le_bytes([frame[frame.len() - 2], frame[frame.len() - 1]]);
                    let stack = i64::from(i16::from_le_bytes([frame[21], frame[22]]));
                    let unit = match id {
                        71 => 1,
                        72 => 100,
                        73 => 10_000,
                        74 => 1_000_000,
                        _ => 0,
                    };
                    total += stack * unit;
                }
            }
            total
        };
        let (a, b, c) = (total_for(1), total_for(2), total_for(3));
        assert!(a > 0 && b > 0 && c > 0, "coins should drop");
        assert!(
            a != b || b != c,
            "the value is varied per roll, not paid at face"
        );
        for t in [a, b, c] {
            let t = t as f32;
            assert!(
                (FACE * 0.5..=FACE * 5.0).contains(&t),
                "within a sane band: {t}"
            );
        }
    }
}

/// A latched nebula headcrab (`ai_style` 85, `ai[0] == 5.0`, `hunter::path::LATCHED`) keeps
/// putting `Obstructed` (buff 163) on the player it is riding, every tick, per
/// `NPC.cs:37508-37526` (`player22.AddBuff(163, 59)`) — exercising the whole channel end to end:
/// `ai::run`'s `85 =>` arm sets `Effects::player_buff`, `npc_ai::update_with` carries it into
/// `AiOutput`, and `tick_npcs` broadcasts it as packet 55 the same way the roar/aura/touch-debuff
/// buffs already do.
#[cfg(test)]
mod nebula_headcrab_buff {
    use super::*;
    use crate::config::Config;
    use terrustia_proto::npc_params::HEADCRAB;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "nebula headcrab buff probe")
    }

    fn with_one_player(mut server: GameServer) -> (GameServer, mpsc::Receiver<Bytes>) {
        let (out_tx, out_rx) = mpsc::channel(16);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = ConnState::Playing;
        player.life = 100;
        player.life_max = 100;
        server.players[0] = Some(player);
        (server, out_rx)
    }

    /// Whether an `ADD_PLAYER_BUFF_PV_P` (packet 55) frame for buff 163 sits anywhere in the
    /// queue. `tick_npcs` also runs `tick_npc_syncs` at its own end, which broadcasts an ordinary
    /// NPC sync for the headcrab on every call (`hunter::pathfinder` sets `npc.dirty = true`
    /// unconditionally) — draining and filtering is what keeps that unrelated packet from being
    /// mistaken for the buff this module is actually testing.
    fn obstructed_was_sent(rx: &mut mpsc::Receiver<Bytes>) -> bool {
        let mut found = false;
        while let Ok(frame) = rx.try_recv() {
            if frame.len() >= 6
                && frame[2] == terrustia_proto::id::ADD_PLAYER_BUFF_PV_P
                && u16::from_le_bytes([frame[4], frame[5]]) == 163
            {
                found = true;
            }
        }
        found
    }

    #[test]
    fn a_latched_headcrab_puts_obstructed_on_its_rider() {
        let (mut server, mut rx) =
            with_one_player(GameServer::new(Config::default(), tiny_world()));
        let index = server
            .npcs
            .spawn(HEADCRAB, (0.0, 0.0))
            .expect("a slot for the headcrab");
        server.npcs.get_mut(index).expect("just spawned").ai[0] = 5.0;

        server.tick_npcs();

        // `tick_npcs` also runs `tick_npc_syncs` at its own end, which sends an ordinary NPC sync
        // for the headcrab too (`hunter::pathfinder` sets `npc.dirty = true` unconditionally) — so
        // the buff frame is found among the queue rather than assumed to be the only one in it.
        let frame = std::iter::from_fn(|| rx.try_recv().ok())
            .find(|f| f.len() >= 6 && f[2] == terrustia_proto::id::ADD_PLAYER_BUFF_PV_P)
            .expect("a buff packet should have been sent");
        assert_eq!(frame[3], 0, "aimed at player slot 0");
        assert_eq!(u16::from_le_bytes([frame[4], frame[5]]), 163, "Obstructed");
        assert_eq!(
            i32::from_le_bytes([frame[6], frame[7], frame[8], frame[9]]),
            59
        );
    }

    /// The control case: a headcrab that has not latched on (`ai[0]` at its default, `DECIDING`)
    /// sends nothing, so the test above is proving the latch check does something.
    #[test]
    fn an_unlatched_headcrab_sends_nothing() {
        let (mut server, mut rx) =
            with_one_player(GameServer::new(Config::default(), tiny_world()));
        server
            .npcs
            .spawn(HEADCRAB, (0.0, 0.0))
            .expect("a slot for the headcrab");

        server.tick_npcs();

        assert!(
            !obstructed_was_sent(&mut rx),
            "a headcrab that has not latched on should not be buffing anyone"
        );
    }

    /// `NPC.cs:37522`'s own `!player22.creativeGodMode` gate, mirrored the same way `hurt_player`
    /// mirrors it for damage — see the `godmode` module above.
    #[test]
    fn a_player_in_creative_godmode_is_not_buffed() {
        let (mut server, mut rx) =
            with_one_player(GameServer::new(Config::default(), tiny_world()));
        server.journey.set_godmode(0, true);
        let index = server
            .npcs
            .spawn(HEADCRAB, (0.0, 0.0))
            .expect("a slot for the headcrab");
        server.npcs.get_mut(index).expect("just spawned").ai[0] = 5.0;

        server.tick_npcs();

        assert!(
            !obstructed_was_sent(&mut rx),
            "creative god mode should have gated this"
        );
    }
}

/// Journey mode's `Difficulty` — real vanilla's `Main.Difficulty` is the single float every
/// difficulty-scaled system in source actually reads (`effective_difficulty`'s own doc), so this
/// module pins that a Journey world's slider actually reaches every one of the call sites that
/// used to read `world.game_mode` directly, one test per site — not just that the accessor itself
/// computes the right number.
///
/// A genuine side-finding along the way, not something introduced by this change: five of those
/// call sites (`ai_conditions`'s `expert`, `drop_loot`'s `expert`/`master`, `apply_touch_debuffs`,
/// `note_army_kill`, `note_moon_kill`) read `world.game_mode >= 1`/`>= 2` directly — and `3 >= 1`
/// and `3 >= 2` are both true, so a Journey world (`game_mode == 3`) was *already* silently read as
/// full expert-and-master for every one of these before this module existed at all, regardless of
/// the gentler `0.5` difficulty `of_game_mode` correctly gave it for NPC life/damage. Real vanilla
/// never has this inconsistency, because `Main.Difficulty` is the one thing everything reads —
/// `expertMode`/`masterMode` are just `Difficulty >= 2`/`>= 3` on it, and a Journey world's
/// `Difficulty` (0.5 by default, whether or not the slider override is even active — `GameMode ==
/// 3` matches neither of `Main.Difficulty`'s own `GameMode == 1`/`== 2` fallback branches) is below
/// both thresholds. Routing every site through `effective_difficulty`/`is_expert`/`is_master`
/// fixes this as a side effect of giving the slider anywhere to reach at all.
#[cfg(test)]
mod difficulty_slider {
    use super::*;
    use crate::config::Config;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "difficulty slider probe")
    }

    fn journey_at(slider: f32) -> GameServer {
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.world.game_mode = 3;
        server.journey.difficulty_slider = slider;
        server
    }

    fn with_one_player(mut server: GameServer) -> (GameServer, mpsc::Receiver<Bytes>) {
        let (out_tx, out_rx) = mpsc::channel(16);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = ConnState::Playing;
        server.players[0] = Some(player);
        (server, out_rx)
    }

    /// WOF-3: a For-the-Worthy world grows the Destroyer's body. Its head is built with 101 trailing
    /// parts (100 body + tail) in a get-good world against 81 (80 + tail) otherwise
    /// (`GetDestroyerSegmentsCount`, `NPC.cs:51488-51495`). Only the get-good flag changes it, and
    /// only for the Destroyer. Before the fix `worm_body` gave 81 whatever the seed.
    #[test]
    fn a_for_the_worthy_destroyer_grows_a_longer_body() {
        use terrustia_proto::npc_params::{
            DESTROYER_HEAD, DESTROYER_SEGMENTS, DESTROYER_SEGMENTS_GOOD,
        };
        let mut server = GameServer::new(Config::default(), tiny_world());

        let (_, _, ordinary) = server
            .worm_parts(DESTROYER_HEAD)
            .expect("the Destroyer is a worm");
        assert_eq!(
            ordinary, DESTROYER_SEGMENTS,
            "an ordinary world keeps 81 parts"
        );

        server.world.secret_seeds.get_good = true;
        let (_, _, good) = server.worm_parts(DESTROYER_HEAD).expect("still a worm");
        assert_eq!(
            good, DESTROYER_SEGMENTS_GOOD,
            "a For-the-Worthy world grows it to 101"
        );
        assert!(good > ordinary, "the seed makes it longer, not shorter");

        // The seed lengthens only the Destroyer: the Eater of Worlds keeps its count either way.
        let (_, _, eater) = server.worm_parts(13).expect("the Eater is a worm");
        assert_eq!(
            eater, 20,
            "an Eater of Worlds is the same in a get-good world"
        );
    }

    #[test]
    fn outside_a_journey_world_the_slider_is_ignored() {
        for game_mode in [0u8, 1, 2] {
            let mut server = GameServer::new(Config::default(), tiny_world());
            server.world.game_mode = game_mode;
            server.journey.difficulty_slider = 1.0; // set, but should never be read
            assert_eq!(
                server.effective_difficulty(),
                terrustia_proto::difficulty::of_game_mode(game_mode),
                "game_mode {game_mode} must ignore the slider entirely"
            );
        }
    }

    #[test]
    fn an_untouched_journey_world_keeps_journeys_own_old_fixed_difficulty() {
        let server = journey_at(0.0);
        assert_eq!(server.effective_difficulty(), 0.5);
        assert!(!server.is_expert(), "a fresh Journey world is not expert");
        assert!(!server.is_master(), "a fresh Journey world is not master");
    }

    #[test]
    fn moving_the_slider_to_its_top_makes_a_journey_world_read_as_master() {
        let server = journey_at(1.0);
        assert_eq!(server.effective_difficulty(), 3.0);
        assert!(server.is_expert());
        assert!(server.is_master());
    }

    /// The main chokepoint (`tick()`'s own `let difficulty = self.effective_difficulty();`):
    /// NPC life scaling. A zombie's `life_max` should reflect the slider's own continuous value,
    /// not the fixed `0.5` a Journey world always used to be stuck with. `life_multiplier` is a
    /// single linear segment from `(0.5, 0.5)` to `(4.0, 4.0)` — i.e. the identity function on
    /// this range — so 0.5x to 3.0x should be a clean 6x, modulo the `as i32` truncation each
    /// scaling step already applies (real vanilla's own `NPC.ScaleStats` truncates too), which is
    /// why this checks a wide, unambiguous margin rather than an exact ratio.
    #[test]
    fn the_npc_scaling_chokepoint_reflects_a_moved_slider() {
        const ZOMBIE: u16 = 3;
        let mut gentle = journey_at(0.0);
        gentle.tick();
        let index = gentle.npcs.spawn(ZOMBIE, (0.0, 0.0)).expect("a slot");
        let gentle_life = gentle.npcs.get(index).unwrap().life_max;

        let mut fierce = journey_at(1.0);
        fierce.tick();
        let index = fierce.npcs.spawn(ZOMBIE, (0.0, 0.0)).expect("a slot");
        let fierce_life = fierce.npcs.get(index).unwrap().life_max;

        assert!(
            fierce_life >= gentle_life * 5,
            "0.5x to 3.0x should be roughly a 6x jump: gentle={gentle_life}, fierce={fierce_life}"
        );
    }

    /// Dryad's Bane borrows the same difficulty curve as town NPC damage
    /// (`dryad_bane_rate`/`buffs::town_npc_damage_multiplier`) — a separate call site from the NPC
    /// scaling chokepoint above, reached through `self.effective_difficulty()` directly rather
    /// than through `self.npcs.set_scaling`. Comparing classic-equivalent (slider 0.33) against
    /// master (slider 1.0) rather than journey's own default (slider 0.0) against master: the
    /// curve's own real shape (`the_difficulty_curve_hits_its_keys` in `buffs.rs`) peaks at
    /// journey (2.0x) and dips before master (1.75x) — a real, pre-existing, already-pinned curve
    /// shape, not something this change introduced, but the wrong pair to assert "goes up" on.
    #[test]
    fn dryad_banes_rate_reflects_a_moved_slider() {
        let classic_equivalent = journey_at(0.33).dryad_bane_rate();
        let master = journey_at(1.0).dryad_bane_rate();
        assert_eq!(
            classic_equivalent, 4,
            "difficulty 1.0: base 4 * multiplier 1.0"
        );
        assert_eq!(
            master, 7,
            "difficulty 3.0: base 4 * multiplier 1.75, truncated"
        );
        assert!(master > classic_equivalent);
    }

    /// `ai_conditions`'s own `expert` field, which town NPC combat (and every other AI branch that
    /// asks) reads instead of a raw game-mode check.
    #[test]
    fn ai_conditions_expert_reflects_a_moved_slider() {
        let biome = crate::game::spawn::Biome::Forest;
        assert!(!journey_at(0.0).ai_conditions(biome).expert);
        assert!(journey_at(1.0).ai_conditions(biome).expert);
    }

    /// `drop_loot`'s `Conditions.expert` — a King Slime's treasure bag (item 3318) is an
    /// unconditional expert-only drop (`conditional_drops::conditional`'s own `always(bag)`), so
    /// its presence or absence is a clean, RNG-free signal.
    #[test]
    fn drop_loots_expert_condition_reflects_a_moved_slider() {
        const KING_SLIME: u16 = 50;
        const TREASURE_BAG: i32 = 3318;

        let mut gentle = journey_at(0.0);
        gentle.drop_loot(KING_SLIME, (0.0, 0.0), DeadNpc::default());
        assert!(
            !gentle
                .items
                .iter()
                .any(|(_, it)| it.item.id == TREASURE_BAG),
            "a fresh Journey world is not expert, so no bag"
        );

        let mut fierce = journey_at(1.0);
        fierce.drop_loot(KING_SLIME, (0.0, 0.0), DeadNpc::default());
        assert!(
            fierce
                .items
                .iter()
                .any(|(_, it)| it.item.id == TREASURE_BAG),
            "the slider at its top is expert, so the bag should drop"
        );
    }

    /// `apply_touch_debuffs`'s own `expert` gate — npc 222 (Queen Bee) always (`one_in: 1`) lands
    /// an expert-only Poisoned on touch (`touch_debuffs::POISONED_IN_EXPERT`).
    #[test]
    fn apply_touch_debuffs_expert_gate_reflects_a_moved_slider() {
        const QUEEN_BEE: u16 = 222;

        let (mut gentle, mut gentle_rx) = with_one_player(journey_at(0.0));
        let gentle_difficulty = gentle.effective_difficulty();
        gentle.apply_touch_debuffs(0, QUEEN_BEE, gentle_difficulty);
        assert!(
            gentle_rx.try_recv().is_err(),
            "a fresh Journey world is not expert, so no buff should be sent"
        );

        let (mut fierce, mut fierce_rx) = with_one_player(journey_at(1.0));
        let fierce_difficulty = fierce.effective_difficulty();
        fierce.apply_touch_debuffs(0, QUEEN_BEE, fierce_difficulty);
        assert!(
            fierce_rx.try_recv().is_ok(),
            "the slider at its top is expert, so the buff should be sent"
        );
    }

    /// A touch debuff in `BuffID.Sets.BuffTimeIsExtendedWithGameDifficulty` lasts longer in expert.
    /// NPC 141 lands a fixed six-hundred-tick Poisoned (buff 20, in that set) one touch in two, and
    /// `DebuffTimeMultiplier` stretches it x2 in expert. The server sends packet 55 with a raw
    /// duration the client trusts as-is, so it must pre-multiply: the same base roll must go out at
    /// six hundred in classic and twelve hundred in expert. Before this the wire duration was raw,
    /// so an expert-world poison off a touch ran half as long as the game intends.
    #[test]
    fn a_difficulty_extended_touch_debuff_is_stretched_on_the_wire_in_expert() {
        use rand::SeedableRng;
        const POISONER: u16 = 141;
        let ticks = |f: &[u8]| i32::from_le_bytes([f[6], f[7], f[8], f[9]]);
        for seed in 0..64u64 {
            let (mut classic, mut classic_rx) =
                with_one_player(GameServer::new(Config::default(), tiny_world()));
            classic.rng = rand::rngs::SmallRng::seed_from_u64(seed);
            classic.apply_touch_debuffs(0, POISONER, 1.0);

            let (mut expert, mut expert_rx) =
                with_one_player(GameServer::new(Config::default(), tiny_world()));
            expert.rng = rand::rngs::SmallRng::seed_from_u64(seed);
            expert.apply_touch_debuffs(0, POISONER, 2.0);

            // The one-in-two roll consumes the rng identically under the same seed, so either both
            // land the debuff or neither does.
            if let Ok(classic_frame) = classic_rx.try_recv() {
                let expert_frame = expert_rx
                    .try_recv()
                    .expect("the same seed lands the same one-in-two roll");
                assert_eq!(
                    ticks(&classic_frame),
                    600,
                    "classic keeps the base duration"
                );
                assert_eq!(ticks(&expert_frame), 1200, "expert doubles a Poisoned");
                return;
            }
        }
        panic!("no seed in the range landed the one-in-two poison");
    }

    /// `note_army_kill`'s own `expert` local — a plain Old One's Army goblin (any id in
    /// `army::belongs`'s range) is worth double the kill points once expert.
    #[test]
    fn note_army_kill_expert_doubling_reflects_a_moved_slider() {
        const A_PLAIN_ARMY_ENEMY: u16 = 552;

        let mut gentle = journey_at(0.0);
        gentle.army.start(crate::game::army::Tier::One);
        gentle.note_army_kill(A_PLAIN_ARMY_ENEMY);
        assert_eq!(gentle.army.kills, 1, "not expert, so one plain kill");

        let mut fierce = journey_at(1.0);
        fierce.army.start(crate::game::army::Tier::One);
        fierce.note_army_kill(A_PLAIN_ARMY_ENEMY);
        assert_eq!(fierce.army.kills, 2, "expert doubles a plain kill");
    }

    /// `note_moon_kill`'s own `is_expert()`/`is_master()` — the top of the slider is master, worth
    /// 2.5x a kill rather than the classic 1x a fresh Journey world reads as.
    #[test]
    fn note_moon_kill_scaling_reflects_a_moved_slider() {
        const A_PUMPKIN_MOON_SCARECROW: u16 = 305; // worth 1 point, from moons.rs's own table

        let mut gentle = journey_at(0.0);
        gentle.moon.start(crate::game::moons::Moon::Pumpkin);
        gentle.note_moon_kill(A_PUMPKIN_MOON_SCARECROW);
        assert_eq!(gentle.moon.points, 1.0, "classic scale is 1x");

        let mut fierce = journey_at(1.0);
        fierce.moon.start(crate::game::moons::Moon::Pumpkin);
        fierce.note_moon_kill(A_PUMPKIN_MOON_SCARECROW);
        assert_eq!(fierce.moon.points, 2.5, "master scale is 2.5x");
    }
}

/// The birthday party — see `game/party.rs`'s own module doc for the real vanilla mechanism this
/// wires up: `roll_dawn_events`'s own natural roll, `roll_dusk_events`'s own end-of-day clear,
/// `tick_party`'s own mid-day prune, and `on_hit_switch`'s own reaction to a Party Monolith.
#[cfg(test)]
mod party {
    use super::*;
    use crate::config::Config;
    use rand::SeedableRng;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "party probe")
    }

    /// Real town NPC types (`npc_data.rs`'s own table) — five ordinary residents plus Party Girl,
    /// which is exactly enough for a natural party to have somewhere to start.
    const A_TOWN: [u16; 5] = [17, 18, 19, 20, 22];

    fn a_town_and_party_girl(server: &mut GameServer) {
        for npc_type in A_TOWN {
            server.npcs.spawn(npc_type, (0.0, 0.0)).expect("a slot");
        }
        server
            .npcs
            .spawn(crate::game::party::PARTY_GIRL, (0.0, 0.0))
            .expect("a slot");
    }

    /// `roll_dawn_events`'s own `roll_natural_party` call, run against a real `NpcStore` rather
    /// than a hand-built eligible list — proves the real `npc_data` lookup and the exclusion
    /// list actually connect to a live server, not just `PartyState`'s own already-tested logic.
    #[test]
    fn a_natural_party_eventually_starts_with_a_real_town_present() {
        for seed in 0..500u64 {
            let mut server = GameServer::new(Config::default(), tiny_world());
            server.rng = SmallRng::seed_from_u64(seed);
            a_town_and_party_girl(&mut server);
            server.roll_natural_party();
            if server.party.genuine {
                assert!(!server.party.celebrating.is_empty());
                assert!(server.party.celebrating.len() <= 3);
                return;
            }
        }
        panic!("a party should have started at least once across 500 seeds");
    }

    /// Without Party Girl having moved in, no amount of trying starts a natural party — real
    /// vanilla's own `NPC.AnyNPCs(208)` gate.
    #[test]
    fn no_party_girl_means_no_natural_party_at_the_server_level() {
        for seed in 0..500u64 {
            let mut server = GameServer::new(Config::default(), tiny_world());
            server.rng = SmallRng::seed_from_u64(seed);
            for npc_type in A_TOWN {
                server.npcs.spawn(npc_type, (0.0, 0.0)).expect("a slot");
            }
            server.roll_natural_party();
            assert!(!server.party.genuine);
        }
    }

    #[test]
    fn a_party_ends_at_dusk() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.party.manual = true;
        server.roll_dusk_events();
        assert!(!server.party.is_up(), "manual parties end at night too");
    }

    /// A celebrating NPC that stops being eligible (evicted, its slot reused by something else)
    /// is pruned on the next tick, and the party ends once none are left.
    #[test]
    fn a_party_ends_early_once_its_last_celebrant_is_gone() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let index = server
            .npcs
            .spawn(crate::game::party::PARTY_GIRL, (0.0, 0.0))
            .expect("a slot");
        server.party.genuine = true;
        server.party.celebrating = vec![index];

        server.npcs.remove(index);
        server.tick_party();

        assert!(!server.party.genuine, "nobody left to celebrate");
        assert!(server.party.celebrating.is_empty());
    }

    /// A direct click on a Party Monolith toggles the world's manually-forced party and resyncs
    /// world data — `on_hit_switch`'s own reaction to `Fired::party_monolith`.
    #[test]
    fn clicking_a_party_monolith_toggles_the_manual_party() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let (out_tx, _out_rx) = mpsc::channel(16);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = ConnState::Playing;
        server.players[0] = Some(player);

        server
            .world
            .set_tile(50, 50, terrustia_proto::Tile::framed(455, 0, 0));

        let mut payload = Vec::new();
        payload.extend_from_slice(&50i16.to_le_bytes());
        payload.extend_from_slice(&50i16.to_le_bytes());

        assert!(!server.party.manual);
        server.on_hit_switch(0, &payload).unwrap();
        assert!(server.party.manual, "the click should have toggled it on");

        server.on_hit_switch(0, &payload).unwrap();
        assert!(!server.party.manual, "and off again");
    }
}

#[cfg(test)]
mod slime_rain {
    use super::*;
    use crate::config::Config;
    use rand::SeedableRng;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "slime rain probe")
    }

    /// `world_data`'s own `WorldFlag::SlimeRain` patch — real state, not left unwired the way
    /// `PartyIsUp` sat before the birthday party landed.
    #[test]
    fn world_data_reflects_whether_a_rain_is_active() {
        // `WorldFlag::SlimeRain` is byte 2, bit 2 (`packets.rs`'s own `position()`, private to
        // that module) — read directly rather than via a setter-only API that has no matching
        // getter.
        let has_flag = |server: &GameServer| server.world_data().flags.0[2] & (1 << 2) != 0;
        let mut server = GameServer::new(Config::default(), tiny_world());
        assert!(!has_flag(&server));
        server.slime_rain.timer = 100;
        assert!(has_flag(&server));
    }

    /// `tick_slime_rain`'s own daily-roll wiring, driven through a real server rather than
    /// `SlimeRainState::roll` in isolation — proves `effective_difficulty`/`journey.time_rate`/
    /// the world's own day-time fields actually connect, not just the state machine's own
    /// already-tested logic. Expert mode alone is enough to let it fire (no ready player needed),
    /// matching `slime_rain.rs`'s own `expert_mode_alone_can_still_start_a_rain` test — and
    /// Journey's fastest clock (`time_rate_slider = 1.0`, 24x) keeps the odds (`9375`, per that
    /// same test's own comment) small enough for a real server loop to observe within a test.
    #[test]
    fn the_daily_roll_eventually_starts_a_rain_with_a_real_server() {
        for seed in 0..30u64 {
            let mut server = GameServer::new(Config::default(), tiny_world());
            server.rng = SmallRng::seed_from_u64(seed);
            server.world.game_mode = 2; // expert
            server.journey.time_rate_slider = 1.0;
            server.world.day_time = true;
            server.world.time = 0;
            for _ in 0..50_000 {
                server.tick_slime_rain();
                if server.slime_rain.is_active() {
                    return;
                }
            }
        }
        panic!("a rain should have started at least once across 30 seeds");
    }

    /// Real rain blocks the roll outright: `Main.cs:65906`'s gate opens with `!raining`, where
    /// `raining` is `Main.raining` (`Main.cs:1282`), the weather flag, and not anything to do
    /// with a slime rain (that half is `NPC.BusyWithAnyInvasionOfSorts`'s `slimeRainTime == 0.0`,
    /// `NPC.cs:7051`).
    ///
    /// Same setup as the roll test above, which fires within 50,000 ticks on every one of these
    /// seeds. Before 2026-08-31 the call site passed `slime_rain.is_active()` here, which `roll`'s
    /// own `busy()` already subsumed, so the weather gate did nothing and this failed on seed 0.
    #[test]
    fn weather_rain_blocks_the_daily_roll() {
        for seed in 0..30u64 {
            let mut server = GameServer::new(Config::default(), tiny_world());
            server.rng = SmallRng::seed_from_u64(seed);
            server.world.game_mode = 2; // expert
            server.journey.time_rate_slider = 1.0;
            server.world.day_time = true;
            server.world.time = 0;
            server.weather.raining = true;
            for _ in 0..50_000 {
                server.tick_slime_rain();
                assert!(
                    !server.slime_rain.is_active(),
                    "seed {seed}: a rain started while it was already raining"
                );
            }
        }
    }

    /// A hundred and fifty Blue Slime kills during a rain summons King Slime at the *closest*
    /// player to the last kill — `DoDeathEvents_AdvanceSlimeRain`'s own real choice, not a random
    /// one, and not just "some player" the way a first draft might assume.
    #[test]
    fn one_hundred_and_fifty_blue_slime_kills_summons_king_slime_near_the_closest_player() {
        use crate::game::slime_rain::{BLUE_SLIME, KING_SLIME};
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.slime_rain.timer = 100;

        let (near_tx, _near_rx) = mpsc::channel(16);
        let mut near = Player::new(0, "127.0.0.1:1".parse().unwrap(), near_tx);
        near.state = ConnState::Playing;
        near.position = (10.0, 10.0);
        server.players[0] = Some(near);

        let (far_tx, _far_rx) = mpsc::channel(16);
        let mut far = Player::new(1, "127.0.0.1:2".parse().unwrap(), far_tx);
        far.state = ConnState::Playing;
        far.position = (10_000.0, 10_000.0);
        server.players[1] = Some(far);

        for _ in 0..149 {
            server.note_slime_rain_kill(BLUE_SLIME, (10.0, 10.0));
        }
        assert!(
            !server.npcs.iter().any(|(_, n)| n.npc_type == KING_SLIME),
            "not yet — only 149 kills"
        );

        server.note_slime_rain_kill(BLUE_SLIME, (10.0, 10.0));

        assert!(
            server.npcs.iter().any(|(_, n)| n.npc_type == KING_SLIME),
            "the 150th kill should have summoned him"
        );
    }

    /// A kill while no rain is active does nothing at all — `note_kill`'s own `!is_active()`
    /// guard, proven connected through the real death path rather than assumed from the isolated
    /// state-machine test.
    #[test]
    fn a_blue_slime_kill_with_no_rain_active_summons_nothing() {
        use crate::game::slime_rain::{BLUE_SLIME, KING_SLIME};
        let mut server = GameServer::new(Config::default(), tiny_world());
        let (out_tx, _out_rx) = mpsc::channel(16);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = ConnState::Playing;
        server.players[0] = Some(player);

        for _ in 0..200 {
            server.note_slime_rain_kill(BLUE_SLIME, (10.0, 10.0));
        }
        assert!(!server.npcs.iter().any(|(_, n)| n.npc_type == KING_SLIME));
    }
}

#[cfg(test)]
mod lantern_night {
    use super::*;
    use crate::config::Config;
    use rand::SeedableRng;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "lantern night probe")
    }

    /// `world_data`'s own `WorldFlag::LanternNight` patch — real state, the same wiring
    /// `WorldFlag::SlimeRain`/`PartyIsUp` already got for their own events.
    #[test]
    fn world_data_reflects_whether_a_lantern_night_is_up() {
        // `WorldFlag::LanternNight` is byte 5, bit 1 (`packets.rs`'s own `position()`, private to
        // that module) — read directly, the same way the slime-rain flag test above does.
        let has_flag = |server: &GameServer| server.world_data().flags.0[5] & (1 << 1) != 0;
        let mut server = GameServer::new(Config::default(), tiny_world());
        assert!(!has_flag(&server));
        server.lantern_night.genuine = true;
        assert!(has_flag(&server));
    }

    /// `roll_natural_lantern_night`'s own daily-roll wiring, driven through a real server —
    /// proves `world.progress.downed_moon_lord`/the busy-gate computation actually connect, not
    /// just `LanternNightState::natural_attempt`'s own already-tested logic in isolation.
    #[test]
    fn a_natural_lantern_night_eventually_starts_with_moon_lord_downed() {
        for seed in 0..2000u64 {
            let mut server = GameServer::new(Config::default(), tiny_world());
            server.rng = SmallRng::seed_from_u64(seed);
            server.world.progress.downed_moon_lord = true;
            server.roll_natural_lantern_night();
            if server.lantern_night.genuine {
                return;
            }
        }
        panic!("a lantern night should have started at least once across 2000 seeds");
    }

    /// Without Moon Lord ever downed, no amount of trying starts a natural lantern night — real
    /// vanilla's own one real gate on the roll firing at all.
    #[test]
    fn no_lantern_night_without_moon_lord_downed() {
        for seed in 0..500u64 {
            let mut server = GameServer::new(Config::default(), tiny_world());
            server.rng = SmallRng::seed_from_u64(seed);
            server.roll_natural_lantern_night();
            assert!(!server.lantern_night.genuine);
        }
    }

    /// `note_boss_kill`'s own snapshot-and-diff guarantee wiring: killing King Slime for the
    /// first time (a `downed_king_slime` false→true transition) arms
    /// `lantern_night.next_night_guaranteed`, and the very next `roll_natural_lantern_night` call
    /// fires a lantern night outright — deterministic, since the guarantee bypasses the daily
    /// roll's own odds entirely, unlike the statistical test above.
    #[test]
    fn killing_a_boss_for_the_first_time_guarantees_the_next_lantern_night() {
        const KING_SLIME: u16 = crate::game::slime_rain::KING_SLIME;
        let mut server = GameServer::new(Config::default(), tiny_world());
        assert!(!server.lantern_night.next_night_guaranteed);

        server.note_boss_kill(KING_SLIME);
        assert!(server.world.progress.downed_king_slime);
        assert!(
            server.lantern_night.next_night_guaranteed,
            "a first-time boss kill should have armed the guarantee"
        );

        server.roll_natural_lantern_night();
        assert!(
            server.lantern_night.genuine,
            "the armed guarantee should have fired the very next roll"
        );
    }

    /// Killing the *same* boss again (already downed before this kill) does not re-arm the
    /// guarantee — `note_boss_kill`'s own diff is against the flag's own transition, not the kill
    /// event itself.
    #[test]
    fn killing_an_already_downed_boss_again_does_not_rearm_the_guarantee() {
        const KING_SLIME: u16 = crate::game::slime_rain::KING_SLIME;
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.world.progress.downed_king_slime = true;

        server.note_boss_kill(KING_SLIME);
        assert!(
            !server.lantern_night.next_night_guaranteed,
            "already downed before this kill — no false→true transition to guarantee off of"
        );
    }

    /// `roll_dawn_events`'s own `LanternNight::CheckMorning` hook — a lantern night never
    /// survives past one dawn, genuine or manually forced alike, matching the birthday party's
    /// own analogous dawn-end rule.
    #[test]
    fn a_lantern_night_ends_at_dawn() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.lantern_night.manual = true;
        server.roll_dawn_events();
        assert!(
            !server.lantern_night.is_up(),
            "manual nights end at dawn too"
        );
    }
}

/// The Lunatic Cultist's tablet (npc 437) — real vanilla places it at the dungeon entrance once
/// Golem is down, the same "periodic server-side check" shape `tick_old_man` already uses to keep
/// Skeletron reachable. Before this fix, nothing anywhere ever called `self.npcs.spawn` with npc
/// 437 at all — the Moon Lord acceptance-test bot's own finding (task #37), confirmed by direct
/// inspection: `CULTIST_TABLET` was a named constant nothing referenced as a spawn trigger. Every
/// test below fails on the unfixed code (no `tick_cultist_tablet` to call at all) and passes once
/// it exists and is wired into the tick loop.
#[cfg(test)]
mod cultist_tablet_trigger {
    use super::*;
    use crate::config::Config;
    use terrustia_proto::npc_params::CULTIST_TABLET;

    fn dungeon_world() -> crate::world::World {
        let mut world = crate::world::World::empty(200, 150, "cultist tablet probe");
        world.dungeon_x = Some(100);
        world.dungeon_y = Some(50);
        world
    }

    /// A playing player standing right at the dungeon entrance — near enough for
    /// `tick_cultist_tablet`'s own "somebody has to be there to see it" check, the same reasoning
    /// `tick_old_man` already uses for the same spot.
    fn seat_player_at_the_dungeon(server: &mut GameServer) {
        let (tx, _rx) = mpsc::channel(4);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), tx);
        player.state = ConnState::Playing;
        player.position = (100.0 * TILE_SIZE, 50.0 * TILE_SIZE);
        server.players[0] = Some(player);
    }

    #[test]
    fn the_tablet_appears_once_golem_and_skeletron_are_both_down() {
        let mut server = GameServer::new(Config::default(), dungeon_world());
        seat_player_at_the_dungeon(&mut server);
        server.world.progress.downed_golem = true;
        server.world.progress.downed_boss3 = true;

        server.tick_cultist_tablet();

        assert!(
            server
                .npcs
                .iter()
                .any(|(_, n)| n.npc_type == CULTIST_TABLET),
            "the tablet should have appeared at the dungeon entrance"
        );
    }

    /// Golem alone is not enough — real vanilla's own documented gate (`terraria.wiki.gg`'s
    /// "Cultists" page: the Old Man takes spawn priority over the same spot until Skeletron is
    /// down), and the same mutual exclusion `tick_old_man` above already enforces the other way.
    #[test]
    fn no_tablet_while_skeletron_is_still_undefeated() {
        let mut server = GameServer::new(Config::default(), dungeon_world());
        seat_player_at_the_dungeon(&mut server);
        server.world.progress.downed_golem = true;
        server.world.progress.downed_boss3 = false;

        server.tick_cultist_tablet();

        assert!(
            !server
                .npcs
                .iter()
                .any(|(_, n)| n.npc_type == CULTIST_TABLET),
            "Golem alone should not be enough"
        );
    }

    #[test]
    fn no_tablet_before_golem_is_down() {
        let mut server = GameServer::new(Config::default(), dungeon_world());
        seat_player_at_the_dungeon(&mut server);
        server.world.progress.downed_boss3 = true;
        server.world.progress.downed_golem = false;

        server.tick_cultist_tablet();

        assert!(
            !server
                .npcs
                .iter()
                .any(|(_, n)| n.npc_type == CULTIST_TABLET)
        );
    }

    /// Once the Lunatic Cultist has actually been beaten, the tablet does not return — this
    /// project's own reasoned assumption (disclosed in `tick_cultist_tablet`'s own doc comment),
    /// mirroring how `downed_boss3` already permanently retires the Old Man above.
    #[test]
    fn no_tablet_once_the_cultist_is_already_downed() {
        let mut server = GameServer::new(Config::default(), dungeon_world());
        seat_player_at_the_dungeon(&mut server);
        server.world.progress.downed_golem = true;
        server.world.progress.downed_boss3 = true;
        server.world.progress.downed_ancient_cultist = true;

        server.tick_cultist_tablet();

        assert!(
            !server
                .npcs
                .iter()
                .any(|(_, n)| n.npc_type == CULTIST_TABLET)
        );
    }

    /// Nobody standing nearby to see it appear — the same reasoning `tick_old_man` already
    /// applies to the Old Man's own arrival.
    #[test]
    fn no_tablet_with_nobody_watching() {
        let mut server = GameServer::new(Config::default(), dungeon_world());
        server.world.progress.downed_golem = true;
        server.world.progress.downed_boss3 = true;

        server.tick_cultist_tablet();

        assert!(
            !server
                .npcs
                .iter()
                .any(|(_, n)| n.npc_type == CULTIST_TABLET)
        );
    }
}

/// The Wall of Flesh's real vanilla trigger: a Guide Voodoo Doll destroyed by lava in the
/// Underworld while the Guide is alive. Before this fix, no packet a real client could ever send
/// spawned npc 113 at all — the Moon Lord acceptance-test bot's own finding (task #37), confirmed
/// by direct inspection: npc 113 is (deliberately) absent from `npc_params::SUMMONABLE`, and
/// nothing else in this file spawned it either. Every test below fails on the unfixed code (no
/// `tick_wall_of_flesh_trigger` to call at all) and passes once it exists and is wired into
/// `tick_items`.
#[cfg(test)]
mod wall_of_flesh_trigger {
    use super::*;
    use crate::config::Config;

    /// Real vanilla's Guide Voodoo Doll item id, confirmed via `terraria.wiki.gg`'s own infobox —
    /// see `tick_wall_of_flesh_trigger`'s own doc comment for the full citation.
    const GUIDE_VOODOO_DOLL: i32 = 267;
    const GUIDE: u16 = 22;
    const WALL_OF_FLESH: u16 = 113;

    fn underworld_world() -> crate::world::World {
        crate::world::World::empty(200, 400, "wall of flesh probe")
    }

    /// A tile of lava well within the underworld's own `height() - 200` band — the same threshold
    /// `bulbs.rs`'s own `UNDERWORLD` constant and `on_server_teleport`'s own inline arithmetic
    /// already use for the same question.
    fn put_lava_in_the_underworld(server: &mut GameServer) -> (i32, i32) {
        let x = 50;
        let y = server.world.height() - 50;
        server.world.set_tile(
            x,
            y,
            Tile::AIR.with_liquid(terrustia_proto::Liquid::Lava, 255),
        );
        (x, y)
    }

    #[test]
    fn a_guide_voodoo_doll_burning_in_underworld_lava_spawns_the_wall_and_kills_the_guide() {
        let mut server = GameServer::new(Config::default(), underworld_world());
        server.npcs.spawn(GUIDE, (500.0, 500.0)).expect("a slot");
        let (x, y) = put_lava_in_the_underworld(&mut server);
        server
            .items
            .spawn(
                ItemStack::new(GUIDE_VOODOO_DOLL, 1, 0),
                (x as f32 * TILE_SIZE, y as f32 * TILE_SIZE),
            )
            .expect("an item slot");

        server.tick_wall_of_flesh_trigger();

        assert!(
            server.npcs.iter().any(|(_, n)| n.npc_type == WALL_OF_FLESH),
            "the Wall of Flesh should have risen"
        );
        assert!(
            !server
                .npcs
                .iter()
                .any(|(_, n)| n.npc_type == GUIDE && n.is_alive()),
            "the guide should have died with the doll"
        );
        assert!(
            server.items.is_empty(),
            "the doll should have burned up in the process"
        );
    }

    /// The Guide must be alive beforehand — real vanilla's own confirmed requirement. Left
    /// narrowly disclosed here: without a general "items burn in lava" mechanic, a doll that
    /// cannot trigger anything is left alone rather than silently destroyed for no visible
    /// reason — see `summon_wall_of_flesh`'s own doc comment.
    #[test]
    fn nothing_happens_without_a_guide_alive() {
        let mut server = GameServer::new(Config::default(), underworld_world());
        let (x, y) = put_lava_in_the_underworld(&mut server);
        let (index, _) = server
            .items
            .spawn(
                ItemStack::new(GUIDE_VOODOO_DOLL, 1, 0),
                (x as f32 * TILE_SIZE, y as f32 * TILE_SIZE),
            )
            .expect("an item slot");

        server.tick_wall_of_flesh_trigger();

        assert!(!server.npcs.iter().any(|(_, n)| n.npc_type == WALL_OF_FLESH));
        assert!(
            server.items.get(index).is_some(),
            "left alone, since it could not trigger anything"
        );
    }

    #[test]
    fn an_ordinary_item_burning_in_the_same_lava_does_nothing() {
        let mut server = GameServer::new(Config::default(), underworld_world());
        server.npcs.spawn(GUIDE, (500.0, 500.0)).expect("a slot");
        let (x, y) = put_lava_in_the_underworld(&mut server);
        server
            .items
            .spawn(
                ItemStack::new(1, 1, 0), // not the doll
                (x as f32 * TILE_SIZE, y as f32 * TILE_SIZE),
            )
            .expect("an item slot");

        server.tick_wall_of_flesh_trigger();

        assert!(!server.npcs.iter().any(|(_, n)| n.npc_type == WALL_OF_FLESH));
    }

    /// The doll has to be in the Underworld itself, not merely in lava somewhere else in the
    /// world — matching real vanilla's own location requirement.
    #[test]
    fn a_doll_burning_outside_the_underworld_does_nothing() {
        let mut server = GameServer::new(Config::default(), underworld_world());
        server.npcs.spawn(GUIDE, (500.0, 500.0)).expect("a slot");
        let (x, y) = (50, 10); // the surface, not the underworld
        server.world.set_tile(
            x,
            y,
            Tile::AIR.with_liquid(terrustia_proto::Liquid::Lava, 255),
        );
        server
            .items
            .spawn(
                ItemStack::new(GUIDE_VOODOO_DOLL, 1, 0),
                (x as f32 * TILE_SIZE, y as f32 * TILE_SIZE),
            )
            .expect("an item slot");

        server.tick_wall_of_flesh_trigger();

        assert!(!server.npcs.iter().any(|(_, n)| n.npc_type == WALL_OF_FLESH));
    }

    /// Once hardmode has already begun there is nothing left for this trigger to do — matching
    /// `note_boss_kill_inner`'s own `if !p.hard_mode` guard on the death side.
    #[test]
    fn nothing_happens_once_hardmode_has_already_begun() {
        let mut server = GameServer::new(Config::default(), underworld_world());
        server.npcs.spawn(GUIDE, (500.0, 500.0)).expect("a slot");
        server.world.progress.hard_mode = true;
        let (x, y) = put_lava_in_the_underworld(&mut server);
        server
            .items
            .spawn(
                ItemStack::new(GUIDE_VOODOO_DOLL, 1, 0),
                (x as f32 * TILE_SIZE, y as f32 * TILE_SIZE),
            )
            .expect("an item slot");

        server.tick_wall_of_flesh_trigger();

        assert!(!server.npcs.iter().any(|(_, n)| n.npc_type == WALL_OF_FLESH));
    }
}

/// Real server-side coverage for the seven boss-drop-table bugs a parallel audit found this
/// session, cross-referenced against `ItemDropDatabase.cs` and fixed in `conditional_drops.rs`
/// (`conditional_chains`, `moon_lord_weapons`, `bundled_with`, the Creeper npc-id fix, the Queen
/// Slime trophy fix).
///
/// `conditional_drops.rs`'s own test module pins the *data* — the exact item ids, rates, and
/// which npc a rule is wired to. It cannot pin the *algorithms* that actually roll that data,
/// because those live here, in `drop_loot`: break-on-first-success for a chain, draw-without-
/// replacement for Moon Lord's pair, and "spawn a companion item" for Golem's bundle. These tests
/// drive the real consumer end to end instead.
#[cfg(test)]
mod boss_drop_table_fixes {
    use super::*;
    use crate::config::Config;
    use crate::world::items::ItemStore;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "boss drop table probe")
    }

    /// Every `(item id, stack size)` this one kill spawned, in allocation order.
    /// `ItemStore::spawn` always fills the lowest free slot, and nothing here ever removes an
    /// item, so resetting the store immediately before each kill means everything found
    /// afterward belongs to that kill alone — no need to diff against a running total.
    fn kill_and_collect(server: &mut GameServer, npc_type: u16) -> Vec<(i32, i16)> {
        server.items = ItemStore::new();
        server.drop_loot(npc_type, (0.0, 0.0), DeadNpc::default());
        let mut items: Vec<(i16, i32, i16)> = server
            .items
            .iter()
            .map(|(index, it)| (index, it.item.id, it.item.stack))
            .collect();
        items.sort_unstable_by_key(|(index, _, _)| *index);
        items
            .into_iter()
            .map(|(_, id, stack)| (id, stack))
            .collect()
    }

    /// Bug #1, driven end to end: Moon Lord must hand back exactly two items from his real
    /// ten-weapon pool, and never the same one twice. The unfixed code had no case for npc 398 at
    /// all, so this pool never dropped anything; a naive fix drawing from `one_from`'s own
    /// independent-per-pool mechanism could still repeat the same weapon (~1-in-10 per kill) —
    /// this pins the actual without-replacement algorithm in `drop_loot`.
    #[test]
    fn moon_lord_always_drops_two_distinct_signature_weapons() {
        const MOON_LORD: u16 = 398;
        const POOL: [i32; 10] = [3063, 3389, 3065, 1553, 3930, 3541, 3570, 3571, 3569, 5480];
        let mut server = GameServer::new(Config::default(), tiny_world());
        for trial in 0..60 {
            let dropped = kill_and_collect(&mut server, MOON_LORD);
            let picked: Vec<i32> = dropped
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| POOL.contains(id))
                .collect();
            assert_eq!(
                picked.len(),
                2,
                "trial {trial}: exactly two signature weapons, got {picked:?}"
            );
            assert_ne!(
                picked[0], picked[1],
                "trial {trial}: never the same weapon twice"
            );
        }
    }

    /// Bug #1, expert side: expert mode replaces this pool with nothing at all — the treasure bag
    /// carries it instead, same as every other boss's ordinary loot.
    #[test]
    fn moon_lord_gives_none_of_his_signature_weapons_in_expert() {
        const MOON_LORD: u16 = 398;
        const POOL: [i32; 10] = [3063, 3389, 3065, 1553, 3930, 3541, 3570, 3571, 3569, 5480];
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.world.game_mode = 1; // expert
        for trial in 0..10 {
            let dropped = kill_and_collect(&mut server, MOON_LORD);
            assert!(
                !dropped.iter().any(|(id, _)| POOL.contains(id)),
                "trial {trial}: expert should give none of these: {dropped:?}"
            );
        }
    }

    /// Bug #2, driven end to end: Queen Bee must never hand back both the Hive Wand and a piece
    /// of Bee armor in the same kill. The unfixed code rolled 1129 as an independent
    /// `classic_only` entry and spawned an armor piece from `one_from` *unconditionally* — so
    /// both a guaranteed armor piece and a possible wand could land together.
    #[test]
    fn queen_bee_never_gives_the_wand_and_armor_together() {
        const QUEEN_BEE: u16 = 222;
        const BEE_STUFF: [i32; 4] = [1129, 842, 843, 844];
        let mut server = GameServer::new(Config::default(), tiny_world());
        let mut saw_wand = false;
        let mut saw_armor = false;
        for trial in 0..150 {
            let dropped = kill_and_collect(&mut server, QUEEN_BEE);
            let hits: Vec<i32> = dropped
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| BEE_STUFF.contains(id))
                .collect();
            assert!(
                hits.len() <= 1,
                "trial {trial}: at most one of the wand/armor, got {hits:?}"
            );
            match hits.first() {
                Some(1129) => saw_wand = true,
                Some(_) => saw_armor = true,
                None => {}
            }
        }
        assert!(
            saw_wand,
            "150 trials never landed the wand — check the odds"
        );
        assert!(
            saw_armor,
            "150 trials never landed any armor piece — check the odds"
        );
    }

    /// Bug #3, driven end to end: Skeletron must never hand back more than one of its three
    /// weapons in the same kill — the unfixed code rolled all three as independent `classic_only`
    /// entries, so a single kill could give 0, 1, 2 or all 3.
    #[test]
    fn skeletron_never_gives_more_than_one_weapon_per_kill() {
        const SKELETRON: u16 = 35;
        const WEAPONS: [i32; 3] = [1281, 1273, 1313];
        let mut server = GameServer::new(Config::default(), tiny_world());
        let mut saw_any = false;
        for trial in 0..250 {
            let dropped = kill_and_collect(&mut server, SKELETRON);
            let hits: Vec<i32> = dropped
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| WEAPONS.contains(id))
                .collect();
            assert!(
                hits.len() <= 1,
                "trial {trial}: at most one weapon, got {hits:?}"
            );
            saw_any |= !hits.is_empty();
        }
        assert!(
            saw_any,
            "250 trials never landed any weapon — check the odds"
        );
    }

    /// Bug #5, driven end to end: King Slime must get exactly one of the Slime Hook or Slime Gun
    /// every single kill — never both, and, critically, never neither. The unfixed code only ever
    /// had the 1/3 Slime Hook roll, so roughly two kills in three gave neither item.
    #[test]
    fn king_slime_always_gets_exactly_one_of_hook_or_gun() {
        const KING_SLIME: u16 = 50;
        let mut server = GameServer::new(Config::default(), tiny_world());
        let mut saw_hook = false;
        let mut saw_gun = false;
        for trial in 0..60 {
            let dropped = kill_and_collect(&mut server, KING_SLIME);
            let hook = dropped.iter().filter(|(id, _)| *id == 2585).count();
            let gun = dropped.iter().filter(|(id, _)| *id == 2610).count();
            assert_eq!(
                hook + gun,
                1,
                "trial {trial}: exactly one of the two, got {dropped:?}"
            );
            saw_hook |= hook == 1;
            saw_gun |= gun == 1;
        }
        assert!(saw_hook, "60 trials never landed the Slime Hook");
        assert!(saw_gun, "60 trials never landed the Slime Gun");
    }

    /// Bug #7, driven end to end: whenever Golem's pool draw is the Stynger, the same kill must
    /// also carry 60-180 of its own Stynger Bolt — and no other pick brings anything extra. The
    /// unfixed code spawned only whatever `one_from` picked, with no notion of a bundled item, so
    /// item 1261 never dropped from anywhere.
    #[test]
    fn golems_stynger_pick_always_brings_its_own_bolts() {
        const GOLEM: u16 = 245;
        let mut server = GameServer::new(Config::default(), tiny_world());
        let mut saw_stynger = false;
        for trial in 0..300 {
            let dropped = kill_and_collect(&mut server, GOLEM);
            let stynger = dropped.iter().filter(|(id, _)| *id == 1258).count();
            let bolts = dropped.iter().find(|(id, _)| *id == 1261);
            assert!(
                stynger <= 1,
                "trial {trial}: the pool draws exactly one item"
            );
            if stynger == 1 {
                saw_stynger = true;
                let (_, stack) = *bolts.unwrap_or_else(|| {
                    panic!("trial {trial}: Stynger without its own bolts: {dropped:?}")
                });
                assert!(
                    (60..=180).contains(&stack),
                    "trial {trial}: bolt stack {stack} out of the real 60-180 range"
                );
            } else {
                assert!(
                    bolts.is_none(),
                    "trial {trial}: bolts without Stynger: {dropped:?}"
                );
            }
        }
        assert!(
            saw_stynger,
            "300 trials never drew the Stynger — check the odds"
        );
    }

    /// The four AI-state drop gaps (C1-b item 2): Pumpking's own pool now brings the Stake
    /// Launcher's ammunition along the same way Golem's Stynger does — the fix threading
    /// `bundled_with` into `chance_pools`'s own consumer, not just `one_from`'s, alongside the
    /// weapon pool actually existing for npc 325 at all.
    #[test]
    fn pumpkings_stake_launcher_pick_always_brings_its_own_stakes() {
        const PUMPKING: u16 = 325;
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.moon.moon = Some(crate::game::moons::Moon::Pumpkin);
        server.moon.wave = 1;
        let mut saw_launcher = false;
        for trial in 0..300 {
            let dropped = kill_and_collect(&mut server, PUMPKING);
            let launcher = dropped.iter().filter(|(id, _)| *id == 1835).count();
            let stakes = dropped.iter().find(|(id, _)| *id == 1836);
            assert!(launcher <= 1, "trial {trial}: the pool draws one item");
            if launcher == 1 {
                saw_launcher = true;
                let (_, stack) = *stakes.unwrap_or_else(|| {
                    panic!("trial {trial}: Stake Launcher without its own Stakes: {dropped:?}")
                });
                assert!(
                    (30..=60).contains(&stack),
                    "trial {trial}: stake stack {stack} out of the real 30-60 range"
                );
            } else {
                assert!(
                    stakes.is_none(),
                    "trial {trial}: stakes without the launcher: {dropped:?}"
                );
            }
        }
        assert!(saw_launcher, "300 trials never drew the Stake Launcher");
    }

    /// The RedHatSkeletron seam (C1-b item 2): `npc_died` has to read `ai[3]` off the exact NPC
    /// instance that just died, before it removes it from the table, and carry that into
    /// `drop_loot`'s own `Conditions.red_hat_skeletron` — driven through the real `npc_died` entry
    /// point rather than `drop_loot` directly, since that is where the plumbing this item added
    /// actually lives.
    #[test]
    fn a_red_hat_skeletron_kill_drops_its_own_vanity_set_through_npc_died() {
        const SKELETRON: u16 = 35;
        const RED_HAT_SET: [i32; 5] = [5624, 5625, 5626, 5628, 5737];

        let mut server = GameServer::new(Config::default(), tiny_world());
        let index = server
            .npcs
            .spawn(SKELETRON, (0.0, 0.0))
            .expect("a slot for Skeletron");
        server.npcs.get_mut(index).expect("just spawned").ai[3] = 1.0;
        server.items = ItemStore::new();

        server.npc_died(index, SKELETRON, (0.0, 0.0), 0.0);

        let dropped: Vec<i32> = server.items.iter().map(|(_, it)| it.item.id).collect();
        for item in RED_HAT_SET {
            assert!(
                dropped.contains(&item),
                "item {item} missing from a red-hat Skeletron kill: {dropped:?}"
            );
        }
    }

    /// GOL-1: the Golem's head comes off when it dies and flies on its own
    /// (`NPC.cs:85913-85918`).
    ///
    /// Nothing in production ever spawned type 249: `GOLEM_HEAD_FREE` appeared only in
    /// `npc_params.rs` and in the golem tests, so the whole style-48 free-head routine was
    /// unreachable code. It has to inherit the dead head's link to the body, because every
    /// threshold in that style keys on the body's health rather than its own.
    #[test]
    fn the_golems_head_comes_off_when_it_dies() {
        use terrustia_proto::npc_params::{GOLEM_BODY, GOLEM_HEAD, GOLEM_HEAD_FREE};

        let mut server = GameServer::new(Config::default(), tiny_world());
        let body = server
            .npcs
            .spawn(GOLEM_BODY, (1_000.0, 1_000.0))
            .expect("a slot for the Golem");
        let head = server
            .npcs
            .spawn(GOLEM_HEAD, (1_000.0, 900.0))
            .expect("a slot for its head");
        let bottom = {
            let attached = server.npcs.get_mut(head).expect("just spawned");
            attached.follows_boss = Some(body);
            (attached.center().0, attached.position.1 + attached.height())
        };

        server.npc_died(head, GOLEM_HEAD, (1_000.0, 900.0), 0.0);

        let freed: Vec<_> = server
            .npcs
            .iter()
            .filter(|(_, n)| n.npc_type == GOLEM_HEAD_FREE)
            .collect();
        assert_eq!(freed.len(), 1, "the head should have been freed");
        let (_, free) = freed[0];
        assert_eq!(
            free.follows_boss,
            Some(body),
            "and it has to know which body to read"
        );
        // Vanilla hands `NewNPC` a bottom centre; ours is a top-left.
        assert!((free.center().0 - bottom.0).abs() < 1.0);
        assert!((free.position.1 + free.height() - bottom.1).abs() < 1.0);
    }

    /// ...and nothing else sheds a head: an ordinary kill leaves no free head behind.
    #[test]
    fn an_ordinary_kill_frees_no_golem_head() {
        use terrustia_proto::npc_params::GOLEM_HEAD_FREE;

        let mut server = GameServer::new(Config::default(), tiny_world());
        let index = server
            .npcs
            .spawn(1, (0.0, 0.0))
            .expect("a slot for a slime");
        server.npc_died(index, 1, (0.0, 0.0), 0.0);
        assert!(
            !server
                .npcs
                .iter()
                .any(|(_, n)| n.npc_type == GOLEM_HEAD_FREE)
        );
    }

    /// The Terraprisma, driven through the real `npc_died` entry point.
    ///
    /// `RegisterBoss_HallowBoss` hangs item 5005 off a single gate,
    /// `Conditions.EmpressOfLightIsGenuinelyEnraged` (`ItemDropDatabase.cs:333-334`), which asks
    /// *this* Empress whether her fight was begun in daylight -
    /// `AI_120_HallowBoss_IsGenuinelyEnraged` (`NPC.cs:46321-46328`), her own `ai[3]` being 2 or 3.
    /// Her ai style already keeps that mark; the drop was simply absent from the tables, so the
    /// hardest thing in the game could not be obtained at all. Dropping the `npc_type == 636` arm
    /// in `conditional_drops` turns the first half of this red, and dropping the `ai[3]` read in
    /// `npc_died` turns it red as well.
    #[test]
    fn a_daylight_empress_kill_drops_the_terraprisma_and_a_night_one_does_not() {
        const EMPRESS: u16 = 636;
        const TERRAPRISMA: i32 = 5005;

        let dropped_with = |ai3: f32| {
            let mut server = GameServer::new(Config::default(), tiny_world());
            let index = server
                .npcs
                .spawn(EMPRESS, (0.0, 0.0))
                .expect("a slot for the Empress");
            server.npcs.get_mut(index).expect("just spawned").ai[3] = ai3;
            server.items = ItemStore::new();
            server.npc_died(index, EMPRESS, (0.0, 0.0), 0.0);
            let ids: Vec<i32> = server.items.iter().map(|(_, it)| it.item.id).collect();
            ids.contains(&TERRAPRISMA)
        };

        // 2 is "enraged from full health", 3 the same after she has turned into her second phase.
        assert!(dropped_with(2.0), "a daylight kill earns it");
        assert!(dropped_with(3.0), "and so does one after the turn");
        // 0 is an ordinary night fight, 1 the same after the turn. Neither is the daylight fight.
        assert!(!dropped_with(0.0), "a night fight does not");
        assert!(!dropped_with(1.0), "nor a night fight past its turn");
    }

    /// Killing a Prismatic Lacewing is the Empress of Light's only summon
    /// (`NPC.cs:80309-80319`), and it is what makes her reachable at all: with the spawn arm and
    /// this event both absent, nothing this server could do put a 636 in a world by itself.
    ///
    /// The three gates are asserted separately because each one is a different failure. Vanilla's
    /// `GetWereThereAnyInteractions()` is [`Npc::hit_by_player`], so a Lacewing a townsperson cut
    /// down is not a summon; `!AnyNPCs(636)` stops a second Lacewing stacking a second Empress; and
    /// the type gate stops every other corpse in the world doing it.
    ///
    /// Neutralised arm by arm, each run and each failing its own assertion:
    ///
    /// * deleting the `wake_the_empress` call from `npc_died`: "a killed Lacewing woke no Empress".
    /// * dropping `|| !hit_by_player`: "a Lacewing nobody hit woke the Empress".
    /// * dropping the `AnyNPCs` guard: "a second Lacewing stacked a second Empress: 2".
    #[test]
    fn a_killed_prismatic_lacewing_wakes_the_empress() {
        use terrustia_proto::npc_params::{EMPRESS_OF_LIGHT, PRISMATIC_LACEWING};

        let empresses = |server: &GameServer| {
            server
                .npcs
                .iter()
                .filter(|(_, n)| n.npc_type == EMPRESS_OF_LIGHT)
                .count()
        };
        let kill = |server: &mut GameServer, hit_by_player: bool| {
            let at = (5_000.0, 2_000.0);
            let index = server
                .npcs
                .spawn(PRISMATIC_LACEWING, at)
                .expect("a slot for the Lacewing");
            server
                .npcs
                .get_mut(index)
                .expect("just spawned")
                .hit_by_player = hit_by_player;
            server.npc_died(index, PRISMATIC_LACEWING, at, 0.0);
        };

        let mut server = GameServer::new(Config::default(), tiny_world());
        kill(&mut server, true);
        assert_eq!(empresses(&server), 1, "a killed Lacewing woke no Empress");

        // ...and only one of her, however many Lacewings die.
        kill(&mut server, true);
        assert_eq!(
            empresses(&server),
            1,
            "a second Lacewing stacked a second Empress: {}",
            empresses(&server)
        );

        // A Lacewing that no player ever touched is not a summon.
        let mut untouched = GameServer::new(Config::default(), tiny_world());
        kill(&mut untouched, false);
        assert_eq!(
            empresses(&untouched),
            0,
            "a Lacewing nobody hit woke the Empress"
        );

        // ...and nothing else does it at all.
        let mut other = GameServer::new(Config::default(), tiny_world());
        let index = other.npcs.spawn(1, (5_000.0, 2_000.0)).expect("a slot");
        other
            .npcs
            .get_mut(index)
            .expect("just spawned")
            .hit_by_player = true;
        other.npc_died(index, 1, (5_000.0, 2_000.0), 0.0);
        assert_eq!(empresses(&other), 0, "a slime woke the Empress");
    }

    /// The control case: an ordinary Skeletron kill (`ai[3]` left at its default) must not carry
    /// the vanity set — otherwise every Skeletron kill would hand it out, which real vanilla never
    /// does outside the Clothier's own repeatable re-fight.
    #[test]
    fn an_ordinary_skeletron_kill_does_not_drop_the_red_hat_set_through_npc_died() {
        const SKELETRON: u16 = 35;
        const RED_HAT_SET: [i32; 5] = [5624, 5625, 5626, 5628, 5737];

        let mut server = GameServer::new(Config::default(), tiny_world());
        let index = server
            .npcs
            .spawn(SKELETRON, (0.0, 0.0))
            .expect("a slot for Skeletron");
        server.items = ItemStore::new();

        server.npc_died(index, SKELETRON, (0.0, 0.0), 0.0);

        let dropped: Vec<i32> = server.items.iter().map(|(_, it)| it.item.id).collect();
        for item in RED_HAT_SET {
            assert!(
                !dropped.contains(&item),
                "an ordinary Skeletron kill should not carry item {item}: {dropped:?}"
            );
        }
    }

    /// The bound Purple Slime does not die when it runs out of life: it becomes a Purple Slime
    /// where it stood (`NPC.HitEffect`, `NPC.cs:82596-82627`).
    ///
    /// The guard is at the head of `npc_died` rather than at one hit path because vanilla's own
    /// `HitEffect` runs from `StrikeNPC` (`NPC.cs:82323`) before anything reaps the corpse, so
    /// every way of running it out of life frees it. Fails before the fix, when 686 was reaped like
    /// any other corpse: the slot came back empty, no resident appeared, and nothing was recorded.
    #[test]
    fn a_beaten_bound_purple_slime_becomes_a_resident_rather_than_a_corpse() {
        use crate::game::spawn::BOUND_TOWN_SLIME_PURPLE;

        let mut server = GameServer::new(Config::default(), tiny_world());
        let at = (1_000.0, 500.0);
        let index = server
            .npcs
            .spawn(BOUND_TOWN_SLIME_PURPLE, at)
            .expect("a slot for the bound slime");
        let (width, height) = {
            let npc = server.npcs.get(index).expect("just spawned");
            (npc.width(), npc.height())
        };

        server.npc_died(index, BOUND_TOWN_SLIME_PURPLE, at, 0.0);

        let npc = server
            .npcs
            .get(index)
            .expect("a freed slime is still in the world, not a removed corpse");
        assert_eq!(npc.npc_type, crate::game::server::PURPLE_SLIME);
        assert!(
            npc.stats.town_npc,
            "the freed form has to be a resident, or no house will ever take it"
        );
        // `position = base.Bottom + new Vector2(0f, 48f)` transcribed exactly, quirk included:
        // `Bottom` carries the old half-width into what is then read as a top-left.
        assert_eq!(npc.position, (at.0 + width / 2.0, at.1 + height + 48.0));
        assert!(
            server.world.progress.unlocked_slime_purple,
            "without the flag the sky would keep offering another bound one"
        );
    }

    /// Every other death still goes through the ordinary path. The guard above sits at the head of
    /// the one function four different callers route deaths through, so a mistake in it would
    /// quietly stop reaping the whole bestiary.
    #[test]
    fn an_ordinary_kill_is_still_reaped() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let index = server
            .npcs
            .spawn(1, (0.0, 0.0))
            .expect("a slot for a blue slime");
        server.npc_died(index, 1, (0.0, 0.0), 0.0);
        assert!(
            server.npcs.get(index).is_none(),
            "a slime is still a corpse"
        );
    }
}

/// Real server-side coverage for the numerator fix in `conditional_drops.rs`: `Conditional` used
/// to have no way to represent a real vanilla `chanceNumerator` other than `1`, so every rule with
/// a real rate of `M`-in-`N` (`M != 1`) was modelled at the wrong, too-low `1`-in-`N` instead. The
/// unit tests in `conditional_drops.rs` pin the exact numerator/denominator this module now
/// carries; these drive the real `drop_loot` consumer over many trials to prove the roll it
/// actually performs lands at the *real* rate rather than the old one — the same lesson the Queen
/// Bee test in `boss_drop_table_fixes` already taught this project: a correct-looking data table
/// can still be wrong if the consumer never reads the field.
///
/// Every trial count below is chosen so the real rate and the old (pre-fix) rate are each roughly
/// ten standard deviations from the threshold — at that separation a false result from ordinary RNG
/// variance is not a realistic concern.
#[cfg(test)]
mod conditional_numerator_fixes {
    use super::*;
    use crate::config::Config;
    use crate::world::items::ItemStore;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "numerator fix probe")
    }

    /// Every item id this one kill spawned, in allocation order — see the identical helper in
    /// `boss_drop_table_fixes` for why resetting the store first makes this exact.
    fn kill_and_collect_ids(server: &mut GameServer, npc_type: u16) -> Vec<i32> {
        server.items = ItemStore::new();
        server.drop_loot(npc_type, (0.0, 0.0), DeadNpc::default());
        let mut items: Vec<(i16, i32)> = server
            .items
            .iter()
            .map(|(index, it)| (index, it.item.id))
            .collect();
        items.sort_unstable_by_key(|(index, _)| *index);
        items.into_iter().map(|(_, id)| id).collect()
    }

    /// The Creeper's Tissue Sample (1329) and Crimtane Ore (880): real vanilla rolls both at
    /// 2-in-3 in classic (`ItemDropDatabase.cs:502-503`), not the 1-in-3 this project modelled
    /// before `Conditional` had a numerator field. 300 trials: 2-in-3 has a mean of 200 (sd ~8.2),
    /// 1-in-3 a mean of 100 (sd ~8.2) — the 150 threshold below sits about six standard deviations
    /// from either, so this distinguishes the two rates rather than just checking "something
    /// dropped."
    #[test]
    fn the_creeper_drops_tissue_sample_and_crimtane_at_two_in_three_not_one_in_three() {
        const CREEPER: u16 = 267;
        const TRIALS: usize = 300;
        let mut server = GameServer::new(Config::default(), tiny_world());
        let mut tissue_hits = 0usize;
        let mut crimtane_hits = 0usize;
        for _ in 0..TRIALS {
            let dropped = kill_and_collect_ids(&mut server, CREEPER);
            tissue_hits += dropped.iter().filter(|&&id| id == 1329).count();
            crimtane_hits += dropped.iter().filter(|&&id| id == 880).count();
        }
        assert!(
            tissue_hits > 150,
            "tissue sample landed {tissue_hits}/{TRIALS} — real rate is 2-in-3 (~200), not 1-in-3 (~100)"
        );
        assert!(
            crimtane_hits > 150,
            "crimtane landed {crimtane_hits}/{TRIALS} — real rate is 2-in-3 (~200), not 1-in-3 (~100)"
        );
    }

    /// Queen Bee's own `ByCondition(condition, 1130, 4, 10, 30, 3)` (`ItemDropDatabase.cs:551`):
    /// real rate is 3-in-4 (mean 225 of 300, sd ~7.5), not the 1-in-4 this project modelled before
    /// (mean 75). The 150 threshold sits ten standard deviations from either.
    #[test]
    fn queen_bee_drops_item_1130_at_three_in_four_not_one_in_four() {
        const QUEEN_BEE: u16 = 222;
        const TRIALS: usize = 300;
        let mut server = GameServer::new(Config::default(), tiny_world());
        let mut hits = 0usize;
        for _ in 0..TRIALS {
            let dropped = kill_and_collect_ids(&mut server, QUEEN_BEE);
            hits += dropped.iter().filter(|&&id| id == 1130).count();
        }
        assert!(
            hits > 150,
            "item 1130 landed {hits}/{TRIALS} — real rate is 3-in-4 (~225), not 1-in-4 (~75)"
        );
    }

    /// The hornet family's own `DropBasedOnExpertMode(CommonDrop(209, 3, 1, 1, 2), Common(209))`
    /// (`ItemDropDatabase.cs:1170`): classic's real rate is 2-in-3 (mean 200 of 300, sd ~8.2), not
    /// the 1-in-3 this project modelled before — a gap this numerator audit found fresh, not one of
    /// the two already known when it started. Expert stays unconditional (100%), unaffected by this
    /// fix and checked here as a same-test regression guard.
    #[test]
    fn hornet_stinger_drops_at_two_in_three_in_classic_not_one_in_three() {
        const HORNET: u16 = 42;
        const TRIALS: usize = 300;
        let mut server = GameServer::new(Config::default(), tiny_world());
        let mut hits = 0usize;
        for _ in 0..TRIALS {
            let dropped = kill_and_collect_ids(&mut server, HORNET);
            hits += dropped.iter().filter(|&&id| id == 209).count();
        }
        assert!(
            hits > 150,
            "stinger landed {hits}/{TRIALS} — real classic rate is 2-in-3 (~200), not 1-in-3 (~100)"
        );

        server.world.game_mode = 1; // expert
        for _ in 0..30 {
            let dropped = kill_and_collect_ids(&mut server, HORNET);
            assert!(
                dropped.contains(&209),
                "expert's stinger roll is unconditional (chanceDenominator: 1)"
            );
        }
    }

    /// The Black Recluse's own `DropBasedOnExpertMode(Common(2607, 2, 1, 3), CommonDrop(2607, 10,
    /// 1, 3, 9))` (`ItemDropDatabase.cs:959`): before this fix, every mode gave the same flat
    /// 1-in-2 (mean 150 of 300) because the rule was never mode-branched at all — real expert is
    /// 9-in-10 (mean 270, sd ~5.2), a real, material difference from classic this test proves the
    /// consumer now actually rolls, not just that `conditional_drops.rs`'s own data table has two
    /// different numbers in it.
    #[test]
    fn black_recluse_drops_its_own_item_far_more_often_in_expert_than_classic() {
        const BLACK_RECLUSE: u16 = 163;
        const TRIALS: usize = 300;
        let mut server = GameServer::new(Config::default(), tiny_world());

        let mut classic_hits = 0usize;
        for _ in 0..TRIALS {
            let dropped = kill_and_collect_ids(&mut server, BLACK_RECLUSE);
            classic_hits += dropped.iter().filter(|&&id| id == 2607).count();
        }

        server.world.game_mode = 1; // expert
        let mut expert_hits = 0usize;
        for _ in 0..TRIALS {
            let dropped = kill_and_collect_ids(&mut server, BLACK_RECLUSE);
            expert_hits += dropped.iter().filter(|&&id| id == 2607).count();
        }

        assert!(
            (100..200).contains(&classic_hits),
            "classic landed {classic_hits}/{TRIALS} — real classic rate is 1-in-2 (~150)"
        );
        assert!(
            expert_hits > 230,
            "expert landed {expert_hits}/{TRIALS} — real expert rate is 9-in-10 (~270), not the classic 1-in-2 (~150)"
        );
    }
}

/// Journey mode's `FarPlacementRange` — a misleading name inherited from source; both of its two
/// real vanilla uses (`Player.cs:35212`/`35440`) are about item *pickup* range, not tile placement
/// at all (see `tick_items`'s own comment) — extends how far an item can be reserved for a player,
/// by exactly 240 pixels, and only in a world whose own difficulty is literally Journey
/// (`world.game_mode == 3`), matching source's own `difficulty == 3` guard on both sites.
#[cfg(test)]
mod far_placement_range {
    use super::*;
    use crate::config::Config;
    use terrustia_proto::ItemStack;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "far placement probe")
    }

    /// Same shape as `godmode`'s own `with_one_player` — the receiver has to outlive the tick
    /// call, for the same reason (`broadcast` removes a player whose send fails).
    fn with_one_player_at(
        mut server: GameServer,
        position: (f32, f32),
    ) -> (GameServer, mpsc::Receiver<Bytes>) {
        let (out_tx, out_rx) = mpsc::channel(16);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = ConnState::Playing;
        player.position = position;
        server.players[0] = Some(player);
        (server, out_rx)
    }

    fn a_coin() -> ItemStack {
        ItemStack {
            id: 71, // Copper Coin
            prefix: 0,
            stack: 1,
        }
    }

    #[test]
    fn extends_pickup_range_by_exactly_two_hundred_forty_pixels_in_a_journey_world() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.world.game_mode = 3; // Journey
        let (mut server, _rx) = with_one_player_at(server, (0.0, 0.0));
        server.journey.set_far_placement_range(0, true);
        // Within the boosted range (400 + 240 = 640) but outside the ordinary one — the only
        // distance this test is actually about.
        let (index, _) = server.items.spawn(a_coin(), (500.0, 0.0)).unwrap();

        server.tick_items();

        assert!(
            server.items.get(index).unwrap().is_reserved(),
            "should have been reserved for the player once the range was extended"
        );
    }

    #[test]
    fn does_not_extend_range_without_the_power_enabled() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.world.game_mode = 3;
        let (mut server, _rx) = with_one_player_at(server, (0.0, 0.0));
        // far_placement_range left off — the control case, so the test above is proving
        // something rather than passing regardless of whether the extension exists at all.
        let (index, _) = server.items.spawn(a_coin(), (500.0, 0.0)).unwrap();

        server.tick_items();

        assert!(
            !server.items.get(index).unwrap().is_reserved(),
            "at ordinary range this item should never have been reserved at all"
        );
    }

    /// The power has no effect at all outside a Journey world — `Player.cs`'s own two real uses
    /// both gate on `difficulty == 3` before ever reading it, so an implementation that skipped
    /// that gate would extend pickup range on every world, not just Journey ones.
    #[test]
    fn has_no_effect_outside_a_journey_world() {
        let server = GameServer::new(Config::default(), tiny_world()); // game_mode 0: ordinary
        let (mut server, _rx) = with_one_player_at(server, (0.0, 0.0));
        server.journey.set_far_placement_range(0, true);
        let (index, _) = server.items.spawn(a_coin(), (500.0, 0.0)).unwrap();

        server.tick_items();

        assert!(
            !server.items.get(index).unwrap().is_reserved(),
            "an ordinary-difficulty world should use the plain range regardless of the power"
        );
    }

    #[test]
    fn an_item_beyond_even_the_extended_range_is_still_out_of_reach() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        server.world.game_mode = 3;
        let (mut server, _rx) = with_one_player_at(server, (0.0, 0.0));
        server.journey.set_far_placement_range(0, true);
        let (index, _) = server.items.spawn(a_coin(), (700.0, 0.0)).unwrap(); // past 400 + 240

        server.tick_items();

        assert!(!server.items.get(index).unwrap().is_reserved());
    }
}

/// The wire half of item-slot recycling. Vanilla's `Item.NewItem` sends a `151` for a slot it is
/// about to overwrite before it sends the `21` for what is going in it (`Item.cs:49725-49730`),
/// because both packets address the slot by the same index: a client told only about the new item
/// would silently swap one item for another on its own screen, with no pickup ever having happened
/// and the old item's disappearance never explained.
#[cfg(test)]
mod item_slot_recycling {
    use super::*;
    use crate::config::Config;
    use terrustia_proto::items::MAX_ITEMS;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "item recycling probe")
    }

    /// Same shape as `far_placement_range`'s own helper: the receiver has to outlive the call,
    /// because `broadcast` removes a player whose send fails.
    fn with_one_player(mut server: GameServer) -> (GameServer, mpsc::Receiver<Bytes>) {
        let (out_tx, out_rx) = mpsc::channel(16);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = ConnState::Playing;
        server.players[0] = Some(player);
        (server, out_rx)
    }

    /// Every slot taken, all the same age but for one older one the picker should settle on.
    fn a_full_world(server: &mut GameServer, oldest: i16) {
        for _ in 0..MAX_ITEMS {
            let (index, _) = server
                .items
                .spawn(ItemStack::new(3, 1, 0), (0.0, 0.0))
                .expect("a slot");
            server.items.get_mut(index).expect("the item").age = 5_000;
        }
        server.items.get_mut(oldest).expect("the item").age = 9_000;
    }

    /// The packet ids the one connected player was sent, in order.
    fn frames(rx: &mut mpsc::Receiver<Bytes>) -> Vec<Bytes> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[test]
    fn a_recycled_slot_is_announced_as_despawned_before_the_new_item_arrives_in_it() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        a_full_world(&mut server, 7);
        let (mut server, mut rx) = with_one_player(server);

        let index = server
            .spawn_item(ItemStack::new(9, 1, 0), (16.0, 16.0))
            .expect("a full world should still find a slot by recycling one");
        assert_eq!(index, 7, "the oldest item's slot");

        let sent = frames(&mut rx);
        assert_eq!(
            sent.iter().map(|f| f[2]).collect::<Vec<_>>(),
            vec![id::SYNC_ITEM_DESPAWN, id::SYNC_ITEM],
            "the despawn has to come first, or a client swaps one item for the other in silence"
        );
        assert_eq!(
            terrustia_proto::items::decode_item_despawn(&sent[0][3..]).unwrap(),
            7,
            "and it has to name the slot being reused"
        );
        let spawned = SyncItem::decode(&sent[1][3..]).unwrap();
        assert_eq!((spawned.index, spawned.item.id), (7, 9));
    }

    /// The control: an ordinary drop into a world with room still sends exactly one packet, so the
    /// test above is proving the recycle path rather than a `151` on every single drop.
    #[test]
    fn an_ordinary_drop_into_a_free_slot_sends_no_despawn() {
        let server = GameServer::new(Config::default(), tiny_world());
        let (mut server, mut rx) = with_one_player(server);

        server.spawn_item(ItemStack::new(9, 1, 0), (16.0, 16.0));

        assert_eq!(
            frames(&mut rx).iter().map(|f| f[2]).collect::<Vec<_>>(),
            vec![id::SYNC_ITEM]
        );
    }

    /// A treasure bag goes to one player over `90` rather than being broadcast, but it takes a
    /// slot the same way, so the item it destroys is still owed to everybody.
    #[test]
    fn an_instanced_bag_announces_the_item_it_recycled_too() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        a_full_world(&mut server, 7);
        let (mut server, mut rx) = with_one_player(server);

        server.drop_instanced_bag(3318, (16.0, 16.0)); // Eye of Cthulhu's Treasure Bag

        assert_eq!(
            frames(&mut rx).iter().map(|f| f[2]).collect::<Vec<_>>(),
            vec![id::SYNC_ITEM_DESPAWN, id::SPAWN_INSTANCED_ITEM]
        );
    }
}

/// Wood, closed as a real gap this session: a freshly generated world had trees but no drop
/// mapping for tile 5 at all — chopping one gave nothing, silently, the first material every
/// crafting recipe in the game starts from. `moonlord.rs`'s own doc comment first found and
/// disclosed this live. Fixed by [`GameServer::spawn_tree_drop`], transcribed from
/// `WorldGen.KillTile_GetTreeDrops`.
#[cfg(test)]
mod wood_from_trees {
    use super::*;
    use crate::config::Config;
    use crate::world::items::ItemStore;

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(200, 150, "tree drop probe")
    }

    /// Plants a one-tile trunk on top of `ground_block` at `(10, 100)` and returns a server ready
    /// to break it — the trunk's own position, `(10, 99)`, is what `spawn_tile_drop` is called
    /// with in each test below.
    fn planted(ground_block: u16) -> GameServer {
        let mut world = tiny_world();
        world.set_tile(10, 100, Tile::block(ground_block));
        world.set_tile(10, 99, Tile::framed(5, 0, 0));
        GameServer::new(Config::default(), world)
    }

    fn broken_ids(server: &mut GameServer, frame_x: i16, frame_y: i16) -> Vec<i32> {
        server.items = ItemStore::new();
        server.spawn_tile_drop(5, frame_x, frame_y, 10, 99);
        let mut items: Vec<(i16, i32)> = server
            .items
            .iter()
            .map(|(index, it)| (index, it.item.id))
            .collect();
        items.sort_unstable_by_key(|(index, _)| *index);
        items.into_iter().map(|(_, id)| id).collect()
    }

    /// Ordinary forest grass (block 2, `GetTreeType`'s default) gives plain Wood (item 9), not
    /// nothing — the exact gap this test closes.
    #[test]
    fn a_tree_rooted_in_forest_grass_drops_wood() {
        let mut server = planted(2);
        let dropped = broken_ids(&mut server, 0, 0);
        assert!(
            dropped.contains(&9),
            "expected Wood (9) from a plain trunk segment, got {dropped:?}"
        );
    }

    /// The five other real biome ground types each give their own named wood
    /// (`WorldGen.GetTreeType`'s switch), not the plain forest item.
    #[test]
    fn each_biomes_ground_gives_that_biomes_own_wood() {
        for (ground, expected, name) in [
            (23u16, 619i32, "Ebonwood from Corruption grass"),
            (199, 911, "Shadewood from Crimson grass"),
            (60, 620, "Rich Mahogany from Jungle grass"),
            (109, 621, "Pearlwood from Hallowed grass"),
            (147, 2503, "Boreal Wood from Snow"),
        ] {
            let mut server = planted(ground);
            let dropped = broken_ids(&mut server, 0, 0);
            assert!(
                dropped.contains(&expected),
                "{name}: expected item {expected}, got {dropped:?}"
            );
        }
    }

    /// A tree with no resolvable ground under it (nothing planted below the trunk at all) still
    /// gives plain Wood — vanilla's own fallback (`GetTreeType`'s `default: return TreeTypes.None`
    /// still reaches `KillTile_GetTreeDrops`'s unconditional `dropItem = 9` before the species
    /// switch), not a silently discarded drop.
    #[test]
    fn a_tree_with_unresolvable_ground_still_drops_plain_wood() {
        let mut world = tiny_world();
        // No ground tile placed at all under the trunk.
        world.set_tile(10, 99, Tile::framed(5, 0, 0));
        let mut server = GameServer::new(Config::default(), world);
        let dropped = broken_ids(&mut server, 0, 0);
        assert!(
            dropped.contains(&9),
            "expected the plain-Wood fallback, got {dropped:?}"
        );
    }

    /// A Mushroom-grass-rooted tree gives a Glowing Mushroom about half the time and nothing the
    /// other half, never wood (`KillTile_GetTreeDrops`'s `TreeTypes.Mushroom` arm: `dropItem =
    /// (genRand.Next(2)==0) ? 183 : 0`). 300 trials, mean 150 (sd ~8.7) if the roll is real.
    #[test]
    fn a_tree_rooted_in_mushroom_grass_sometimes_gives_a_glowing_mushroom_never_wood() {
        const TRIALS: usize = 300;
        let mut server = planted(70);
        let mut hits = 0usize;
        for _ in 0..TRIALS {
            let dropped = broken_ids(&mut server, 0, 0);
            assert!(
                !dropped.contains(&9),
                "a Mushroom-biome tree should never give plain Wood, got {dropped:?}"
            );
            hits += dropped.iter().filter(|&&id| id == 183).count();
        }
        assert!(
            (100..200).contains(&hits),
            "glowing mushroom landed {hits}/{TRIALS} — real rate is 1-in-2 (~150)"
        );
    }

    /// Breaking the canopy-top frame (`frameX >= 22 && frameY >= 198`, vanilla's own literal
    /// condition) on acorn-capable ground gives an Acorn about half the time, alongside the wood —
    /// not instead of it. 300 trials, mean 150 (sd ~8.7).
    #[test]
    fn the_canopy_top_sometimes_also_drops_an_acorn() {
        const TRIALS: usize = 300;
        let mut server = planted(2);
        let mut acorns = 0usize;
        for _ in 0..TRIALS {
            let dropped = broken_ids(&mut server, 22, 198);
            assert!(
                dropped.contains(&9),
                "the canopy should still give Wood alongside any acorn, got {dropped:?}"
            );
            acorns += dropped.iter().filter(|&&id| id == 27).count();
        }
        assert!(
            (100..200).contains(&acorns),
            "acorn landed {acorns}/{TRIALS} — real rate off the canopy top is 1-in-2 (~150)"
        );
    }

    /// Jungle trees never give an acorn even off the canopy top — `TreeTypeDropsAcorns` excludes
    /// Jungle by name, since Rich Mahogany propagates by a sapling players plant, not a bonus item.
    #[test]
    fn jungle_trees_never_drop_an_acorn_even_off_the_canopy() {
        let mut server = planted(60);
        for _ in 0..40 {
            let dropped = broken_ids(&mut server, 22, 198);
            assert!(
                !dropped.contains(&27),
                "a Jungle tree's canopy should never give an acorn, got {dropped:?}"
            );
        }
    }

    /// A non-canopy frame never gives an acorn, on any ground — only the leafy top can.
    #[test]
    fn a_trunk_segment_never_drops_an_acorn() {
        let mut server = planted(2);
        for _ in 0..40 {
            let dropped = broken_ids(&mut server, 0, 0);
            assert!(
                !dropped.contains(&27),
                "a plain trunk segment should never give an acorn, got {dropped:?}"
            );
        }
    }

    /// "Bonus wood" (a second Wood in the same stack) lands about a third of the time — the one
    /// real, item-independent term in vanilla's own roll (`Main.rand.Next(3) == 0`) this fix
    /// transcribes; the axe-power-scaled term is a disclosed, separate gap (see
    /// `spawn_tree_drop`'s own doc comment). 300 trials, mean 100 (sd ~8.2).
    #[test]
    fn bonus_wood_lands_about_a_third_of_the_time() {
        const TRIALS: usize = 300;
        let mut server = planted(2);
        let mut bonus = 0usize;
        for _ in 0..TRIALS {
            server.items = ItemStore::new();
            server.spawn_tile_drop(5, 0, 0, 10, 99);
            let wood_stack: i16 = server
                .items
                .iter()
                .filter(|(_, it)| it.item.id == 9)
                .map(|(_, it)| it.item.stack)
                .sum();
            assert!(wood_stack == 1 || wood_stack == 2, "got stack {wood_stack}");
            if wood_stack == 2 {
                bonus += 1;
            }
        }
        assert!(
            (70..130).contains(&bonus),
            "bonus wood landed {bonus}/{TRIALS} — real item-independent rate is 1-in-3 (~100)"
        );
    }
}

/// L2, the liquid-destroys-furniture consumer: `tick_liquids` now resolves
/// `crate::world::liquid::Settled::drowned` against the generated `tile_death` table and
/// actually kills what needs killing — `Liquid.AddWater`'s own inline
/// `CheckLavaDeath`/`CheckWaterDeath` check (`Liquid.cs:1196-1215`), which the table alone,
/// landed separately, was deliberately left unwired until this item.
#[cfg(test)]
mod liquid_furniture_death {
    use super::*;
    use crate::config::Config;
    use terrustia_proto::tile::{Liquid, Tile};

    fn tiny_world() -> crate::world::World {
        crate::world::World::empty(50, 50, "liquid death probe")
    }

    // `WATER_DEATH[4] == true`, `LAVA_DEATH[4] == false` — a torch goes out in a flood but does
    // not burn any further in a fire it is already part of.
    const TORCH: u16 = 4;

    fn full_of(kind: Liquid) -> Tile {
        let mut t = Tile::AIR;
        t.liquid = 255;
        t.liquid_kind = kind;
        t
    }

    /// A torch sitting under a full column of water is killed the moment the water reaches it:
    /// the tile is cleared and every client hears about it as packet 17, the same shape
    /// `on_tile_manipulation`'s own player-break path already uses.
    #[test]
    fn water_reaching_a_torch_kills_it_and_tells_every_client() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let (x, y) = (10, 10);
        server.world.set_tile(x, y, Tile::framed(TORCH, 0, 0));
        server.world.set_tile(x, y - 1, full_of(Liquid::Water));

        let (out_tx, mut out_rx) = mpsc::channel(16);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), out_tx);
        player.state = ConnState::Playing;
        server.players[0] = Some(player);

        server.liquids.wake(x, y - 1);
        for _ in 0..5 {
            server.tick_liquids();
        }

        let tile = server.world.tile(x, y);
        assert!(!tile.is_active(), "the torch should have been killed");
        assert_eq!(tile.block, 0, "and the tile cleared, not merely flagged");

        let told_every_client = std::iter::from_fn(|| out_rx.try_recv().ok())
            .any(|frame| frame.len() > 2 && frame[2] == terrustia_proto::id::TILE_MANIPULATION);
        assert!(
            told_every_client,
            "packet 17 should have gone out, matching NetMessage.SendData(17, -1, -1, ...)"
        );
    }

    /// L3-09: the liquid simulation runs only every second `tick_liquids` call — the
    /// `Liquid.skipCount` gate (`WorldGen.cs:72072-72079`), half of what keeps liquid from running
    /// roughly four times too fast.
    ///
    /// Fails before the fix: `tick_liquids` ran the sim every tick, so a single call already
    /// carried the water down a tile.
    #[test]
    fn liquid_runs_only_every_second_tick() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        // A single tile of water above open air, so one settle pass visibly drops it a tile.
        server.world.set_tile(10, 8, full_of(Liquid::Water));
        server.liquids.wake(10, 8);

        server.tick_liquids();
        assert_eq!(
            server.world.tile(10, 8).liquid,
            255,
            "the first call is skipped, so nothing has moved yet"
        );

        server.tick_liquids();
        assert!(
            server.world.tile(10, 9).liquid > 0,
            "the second call runs the sim and the water falls a tile"
        );
    }

    /// The control case: the same torch under lava (`LAVA_DEATH[4] == false`) survives — proving
    /// the fix reads the right table for the liquid that actually arrived, not merely "any liquid
    /// kills anything".
    #[test]
    fn lava_reaching_a_torch_leaves_it_alone() {
        let mut server = GameServer::new(Config::default(), tiny_world());
        let (x, y) = (10, 10);
        server.world.set_tile(x, y, Tile::framed(TORCH, 0, 0));
        server.world.set_tile(x, y - 1, full_of(Liquid::Lava));

        server.liquids.wake(x, y - 1);
        for _ in 0..40 {
            server.tick_liquids();
        }

        let tile = server.world.tile(x, y);
        assert!(tile.is_active(), "a torch should survive a lava flow");
        assert_eq!(tile.block, TORCH);
    }
}

/// L3-01: the world-update sampler runs over the WHOLE world every tick, with nobody watching.
///
/// The old sampler grew grass and crept biomes only near a connected player, and only once every
/// ten ticks - roughly three-thousand times below vanilla's `UpdateWorld` rate. These two prove
/// the new one both samples world-wide and no longer needs a player at all: they connect nobody,
/// and still see grass creep and an infection spread far from any spawn.
#[cfg(test)]
mod world_update {
    use super::*;

    /// A 500x300 world - big enough that the area-scaled budget (`500*300*3e-5` ~= 5 overground
    /// samples a tick) actually samples, small enough to tick thousands of times cheaply.
    fn probe_world() -> crate::world::World {
        crate::world::World::empty(500, 300, "world update probe")
    }

    /// Grass creeps across a strip of exposed dirt with no player connected. Before the fix the
    /// growth pass returned the moment `around.is_empty()`, so an empty server grew nothing.
    #[test]
    fn grass_spreads_world_wide_with_no_players() {
        let mut server = GameServer::new(Config::default(), probe_world());
        // A strip in the overground band (surface is 100): dirt with air above and below, seeded
        // with plain grass every third tile so most of the strip is exposed dirt beside grass.
        for x in 100..400 {
            let block = if x % 3 == 0 {
                2
            } else {
                crate::world::growth::DIRT
            };
            server.world.set_tile(x, 50, Tile::block(block));
        }
        assert!(
            server.players.iter().flatten().next().is_none(),
            "this test's whole point is that nobody is connected"
        );

        for _ in 0..4000 {
            server.tick_world_update();
        }

        let grew = (100..400)
            .filter(|&x| server.world.tile(x, 50).block == 2)
            .count();
        // Seeded ~100 grass tiles; anything above that is dirt that turned to grass on its own.
        assert!(
            grew > 100,
            "grass did not creep across the strip with nobody watching (only {grew} grass tiles, \
             started with ~100)"
        );
    }

    /// An infection eats the stone around it with no player connected. Before the fix the spread
    /// pass returned the moment `here.is_empty()`, so an empty hardmode server never crept.
    #[test]
    fn corruption_spreads_world_wide_with_no_players() {
        let mut server = GameServer::new(Config::default(), probe_world());
        server.world.progress.hard_mode = true;
        // A field of stone with ebonstone cores threaded through it, in the overground band. Every
        // core has takeable stone within reach, so a sampled core has somewhere to spread.
        for x in 100..180 {
            for y in 40..80 {
                let block = if (x + y) % 2 == 0 { 25 } else { 1 };
                server.world.set_tile(x, y, Tile::block(block));
            }
        }
        let corrupt_before = count_ebonstone(&server.world);

        for _ in 0..4000 {
            server.tick_world_update();
        }

        let corrupt_after = count_ebonstone(&server.world);
        assert!(
            corrupt_after > corrupt_before,
            "the infection never crept with nobody watching ({corrupt_before} -> {corrupt_after} \
             ebonstone)"
        );
    }

    /// With Journey mode's "Stop Biome Spread" power on, the same field stays clean - the runtime
    /// half of L3-15, wired through the same `AllowedToSpreadInfections` gate vanilla uses.
    #[test]
    fn stop_biome_spread_freezes_the_infection() {
        let mut server = GameServer::new(Config::default(), probe_world());
        server.world.progress.hard_mode = true;
        server.journey.stop_biome_spread = true;
        for x in 100..180 {
            for y in 40..80 {
                let block = if (x + y) % 2 == 0 { 25 } else { 1 };
                server.world.set_tile(x, y, Tile::block(block));
            }
        }
        let before = count_ebonstone(&server.world);
        for _ in 0..4000 {
            server.tick_world_update();
        }
        assert_eq!(
            before,
            count_ebonstone(&server.world),
            "the infection crept while Stop Biome Spread was on"
        );
    }

    fn count_ebonstone(world: &crate::world::World) -> usize {
        let mut n = 0;
        for x in 90..190 {
            for y in 30..90 {
                if world.tile(x, y).block == 25 {
                    n += 1;
                }
            }
        }
        n
    }
}

/// Sections stream to a player as they move, not only at their join.
///
/// `Main.cs:65601` calls `RemoteClient.CheckSection(k, player[k].position)` for every active
/// player on every server tick, and `CheckSection_ForClient` (`RemoteClient.cs:152-190`) sends the
/// 3x3 block around wherever that player now is. This server had no per-tick push at all: the only
/// place `pending_sections` was ever written was the join. A player who walked out of the block
/// sent at their join saw sky forever and could not build there either, because
/// `on_tile_manipulation` and `on_place_object` both refuse an edit in a section the client was
/// never sent.
#[cfg(test)]
mod section_streaming_as_players_move {
    use super::*;

    /// Twelve sections by six, so a player can be put well outside their own starting block
    /// without leaving the world. Empty rather than generated: what is being proved is which
    /// sections go out, not what is in them, and an empty world ticks in a fraction of the time.
    fn one_player_at(at: (f32, f32)) -> (GameServer, mpsc::Receiver<Bytes>) {
        let mut server = GameServer::new(
            Config::default(),
            crate::world::World::empty(2400, 900, "section streaming probe"),
        );
        let (tx, rx) = mpsc::channel(100_000);
        let mut player = Player::new(0, "127.0.0.1:1".parse().unwrap(), tx);
        player.state = ConnState::Playing;
        player.position = at;
        server.players[0] = Some(player);
        (server, rx)
    }

    /// A world position a little way inside the named section.
    fn in_section(sx: i32, sy: i32) -> (f32, f32) {
        (
            (sx * terrustia_proto::section::SECTION_WIDTH * 16 + 16) as f32,
            (sy * terrustia_proto::section::SECTION_HEIGHT * 16 + 16) as f32,
        )
    }

    /// How many frames of one packet id have reached this client.
    fn count(rx: &mut mpsc::Receiver<Bytes>, id: u8) -> usize {
        let mut n = 0;
        while let Ok(frame) = rx.try_recv() {
            if frame.get(2) == Some(&id) {
                n += 1;
            }
        }
        n
    }

    /// Tick until nothing is queued, or give up.
    ///
    /// `drain_section_streams` is bounded by wall clock (`SECTION_STREAM_BUDGET`) and guarantees
    /// only one section a tick, so how many of a block go out on any single tick depends on how
    /// busy the machine is. Anything that asserts on a *whole* block has to settle first or it is
    /// a test that passes alone and fails under a loaded CI run.
    fn settle(server: &mut GameServer) {
        for _ in 0..200 {
            server.tick();
            if server
                .player(0)
                .is_some_and(|p| p.pending_sections.is_empty())
            {
                return;
            }
        }
        panic!("a section queue never drained");
    }

    /// The bug itself: walk past the join block and the world is still sent.
    #[test]
    fn walking_into_a_new_section_streams_the_block_around_it() {
        let (mut server, mut rx) = one_player_at(in_section(1, 1));
        settle(&mut server);
        assert!(
            server.player(0).unwrap().sent_sections.contains(&(1, 1)),
            "a standing player should be sent the section they are standing in"
        );
        let _ = count(&mut rx, id::TILE_SECTION);

        // Eight sections away: 1,600 tiles, far outside anything a join could have covered.
        server.player_mut(0).unwrap().position = in_section(9, 4);
        settle(&mut server);

        let sent = &server.player(0).unwrap().sent_sections;
        for sx in 8..=10 {
            for sy in 3..=5 {
                assert!(
                    sent.contains(&(sx, sy)),
                    "section ({sx},{sy}) never reached the player standing in it: past their join \
                     block the world is sky, and they cannot build there either"
                );
            }
        }
        assert_eq!(
            count(&mut rx, id::TILE_SECTION),
            9,
            "the nine sections of the new block should have gone out as real frames"
        );
    }

    /// The cost bound, and the guard that comes with reusing the join's own queue: a player who
    /// has not crossed a boundary queues nothing, and draining a walker's queue must not replay
    /// the join tail (`finish_join_stream`) and re-send `InitialSpawn`, which would respawn a
    /// client that was only walking.
    #[test]
    fn standing_still_queues_nothing_and_never_replays_the_join() {
        let (mut server, mut rx) = one_player_at(in_section(3, 2));
        settle(&mut server);
        let settled = server.player(0).unwrap().sent_sections.len();
        assert_eq!(settled, 9, "a settled player holds their own 3x3 block");
        let _ = count(&mut rx, id::TILE_SECTION);

        for _ in 0..120 {
            server.tick();
        }

        assert_eq!(
            server.player(0).unwrap().sent_sections.len(),
            settled,
            "standing still re-sent sections"
        );
        assert!(
            server.player(0).unwrap().pending_sections.is_empty(),
            "standing still left work queued"
        );
        assert_eq!(
            count(&mut rx, id::TILE_SECTION),
            0,
            "standing still cost section frames"
        );
    }

    /// Every server-side relocation rides the same per-tick check, because each of them moves
    /// `Player::position`: a pylon (`TeleportPylonsSystem.cs:199` makes the same call inline), a
    /// Teleportation Potion, a magic mirror, a respawn. Landing in sky is the failure this rules
    /// out.
    #[test]
    fn a_teleport_streams_where_the_player_lands() {
        let (mut server, mut rx) = one_player_at(in_section(1, 1));
        settle(&mut server);
        let _ = count(&mut rx, id::INITIAL_SPAWN);

        server.player_mut(0).unwrap().position = in_section(10, 1);
        settle(&mut server);

        assert!(
            server.player(0).unwrap().sent_sections.contains(&(10, 1)),
            "a teleported player landed in a section they were never sent"
        );
        assert_eq!(
            count(&mut rx, id::INITIAL_SPAWN),
            0,
            "a walker's section stream replayed the join tail and respawned the client"
        );
    }
}

/// Packet 20 goes to the clients that hold the ground it patches, and to nobody else.
///
/// Vanilla gates this one packet that way and no other (`NetMessage.cs:1721-1731`): its case 20
/// loop adds `Netplay.Clients[i].SectionRange(Math.Max(width, height), x, y)` to the ordinary
/// connected-and-broadcasting test. We were sending every square to every player, so grass
/// spreading in one corner of a full world cost bandwidth in every other, and the square described
/// ground the receiving client had never been sent.
#[cfg(test)]
mod tile_squares_only_reach_who_can_see_them {
    use super::*;

    fn server_with_two_players() -> (GameServer, mpsc::Receiver<Bytes>, mpsc::Receiver<Bytes>) {
        let mut server = GameServer::new(
            Config::default(),
            crate::world::World::empty(2400, 900, "tile square fanout probe"),
        );
        let (tx0, rx0) = mpsc::channel(100_000);
        let (tx1, rx1) = mpsc::channel(100_000);
        for (slot, tx) in [(0u8, tx0), (1u8, tx1)] {
            let mut player = Player::new(slot, "127.0.0.1:1".parse().unwrap(), tx);
            player.state = ConnState::Playing;
            server.players[slot as usize] = Some(player);
        }
        (server, rx0, rx1)
    }

    fn squares(rx: &mut mpsc::Receiver<Bytes>) -> usize {
        let mut n = 0;
        while let Ok(frame) = rx.try_recv() {
            if frame.get(2) == Some(&terrustia_proto::id::AREA_TILE_CHANGE) {
                n += 1;
            }
        }
        n
    }

    /// Slot 0 holds the section the square is in; slot 1 holds one far away.
    #[test]
    fn a_square_reaches_only_the_client_holding_its_section() {
        let (mut server, mut rx0, mut rx1) = server_with_two_players();
        server.player_mut(0).unwrap().sent_sections.insert((0, 0));
        server.player_mut(1).unwrap().sent_sections.insert((9, 4));

        let square = TileSquare {
            x: 10,
            y: 10,
            width: 1,
            height: 1,
            change_type: 0,
            tiles: vec![server.world.tile(10, 10)],
        };
        server.broadcast_tile_square(&square, None);

        assert_eq!(
            squares(&mut rx0),
            1,
            "the client holding that section must receive it"
        );
        assert_eq!(
            squares(&mut rx1),
            0,
            "a client eight sections away must not: it has never been sent the ground this \
             patches, so the packet describes tiles its client does not have"
        );
    }

    /// `SectionRange` tests four corners, so a square straddling a boundary reaches a client that
    /// holds either side. `RemoteClient.cs:192-215`.
    #[test]
    fn a_square_straddling_a_boundary_reaches_both_sides() {
        let (mut server, mut rx0, mut rx1) = server_with_two_players();
        // Section 0 spans x 0..199, so a square at 195 wide enough to cross touches section 1 too.
        server.player_mut(0).unwrap().sent_sections.insert((0, 0));
        server.player_mut(1).unwrap().sent_sections.insert((1, 0));

        let square = TileSquare {
            x: 195,
            y: 10,
            width: 10,
            height: 1,
            change_type: 0,
            tiles: (0..10).map(|i| server.world.tile(195 + i, 10)).collect(),
        };
        server.broadcast_tile_square(&square, None);

        assert_eq!(squares(&mut rx0), 1, "the section it starts in");
        assert_eq!(squares(&mut rx1), 1, "and the one it runs into");
    }

    /// The sender is still excluded, because vanilla tests `num23 != ignoreClient` and
    /// `SectionRange` together rather than one instead of the other.
    #[test]
    fn the_sending_client_is_still_excluded() {
        let (mut server, mut rx0, mut rx1) = server_with_two_players();
        for slot in [0u8, 1u8] {
            server
                .player_mut(slot)
                .unwrap()
                .sent_sections
                .insert((0, 0));
        }

        let square = TileSquare {
            x: 10,
            y: 10,
            width: 1,
            height: 1,
            change_type: 0,
            tiles: vec![server.world.tile(10, 10)],
        };
        server.broadcast_tile_square(&square, Some(0));

        assert_eq!(squares(&mut rx0), 0, "not echoed back to whoever sent it");
        assert_eq!(squares(&mut rx1), 1, "but it does reach the other client");
    }
}

/// The Terraria x Palworld encounter end to end, through the server's own tick rather than through
/// the routine on its own: a distressed pet raises two Goblin Archers to guard it, will not be
/// collected while either is alive, and hands over its Palworld Minion item when it is
/// (`NPC.cs:43379-43489`).
#[cfg(test)]
mod a_distressed_pal_and_its_guard {
    use super::*;
    use crate::config::Config;
    use terrustia_proto::npc_params::{
        PAL_CATTIVA, PAL_ESCORT, PAL_PAYOUT_TICKS, PAL_REWARD_CATTIVA,
    };

    /// Where the pet stands, on a floor with room either side.
    const FLOOR_ROW: i32 = 100;
    const PAL_COLUMN: i32 = 250;

    /// A world that is solid from `FLOOR_ROW` down and open above it, with one pet on the floor.
    fn encounter() -> (GameServer, u8) {
        let mut world = crate::world::World::empty(500, 300, "pal encounter probe");
        for x in 0..world.width() {
            for y in FLOOR_ROW..world.height() {
                world.set_tile(x, y, terrustia_proto::Tile::block(1));
            }
        }
        let mut server = GameServer::new(Config::default(), world);
        let index = server
            .npcs
            .spawn(PAL_CATTIVA, (PAL_COLUMN as f32 * 16.0, 97.0 * 16.0))
            .expect("a free NPC slot");
        (server, index)
    }

    fn guards(server: &GameServer) -> Vec<u8> {
        server
            .npcs
            .iter()
            .filter(|(_, n)| n.npc_type == PAL_ESCORT && n.is_alive())
            .map(|(slot, _)| slot)
            .collect()
    }

    /// Its first tick raises two archers, each back-linked to the pet's own slot.
    ///
    /// Neutralised by dropping the `for spot in spots` loop from `fixtures::pal`: the count
    /// assertion fails and the pet stands alone. Neutralised again by deleting the
    /// `effects.spawn.extend(out.spawn)` line from the `127 =>` arm of `ai::run`: the same
    /// assertion fails, with the routine asking for guards that nothing raises.
    #[test]
    fn a_pal_raises_its_own_guard_on_its_first_tick() {
        let (mut server, pal) = encounter();
        server.tick_npcs();
        let raised = guards(&server);
        assert_eq!(raised.len(), 2, "a pal is guarded by two Goblin Archers");
        for slot in raised {
            let archer = server.npcs.get(slot).expect("just raised");
            assert_eq!(
                archer.ai[3],
                -(f32::from(pal) + 1.0),
                "each guard is back-linked to the pal it stands over"
            );
        }
    }

    /// The whole encounter, in order: guarded, then collectible, then paid out.
    ///
    /// Neutralised by putting `world.count(PAL_ESCORT)` back in place of `world.own_escorts` in the
    /// `127 =>` arm of `ai::run` and standing an unrelated Goblin Archer somewhere else in the
    /// world (which this test does): the "still guarded" half passes but the "released" half fails,
    /// the pal held forever by an archer that is nothing to do with it. Neutralised again by
    /// deleting the `for (item, at) in rewards` loop from `tick_npcs`: the reward assertion fails
    /// and a collected pet leaves nothing behind.
    #[test]
    fn the_pet_is_collected_only_once_both_guards_are_dead_and_leaves_its_item() {
        let (mut server, pal) = encounter();
        // A Goblin Archer of nobody's, on the other side of the world. It must not hold the pal.
        server
            .npcs
            .spawn(PAL_ESCORT, (50.0 * 16.0, 97.0 * 16.0))
            .expect("a free NPC slot");
        // ...and a player standing right on the pet, so the only thing left to wait for is the
        // guard. Well inside `PAL_APPROACH`, and inside `PAL_ESCORT_WAKE` of the guards too.
        let pal_at = server.npcs.get(pal).expect("just spawned").center();
        let (out_tx, _out_rx) = tokio::sync::mpsc::channel(256);
        let mut player = Player::new(0, "127.0.0.1:1".parse().expect("loopback"), out_tx);
        player.state = ConnState::Playing;
        player.position = (pal_at.0, pal_at.1 - 20.0);
        server.players[0] = Some(player);

        server.tick_npcs();
        let raised = guards(&server);
        assert_eq!(raised.len(), 3, "its own two, plus the stranger");

        for _ in 0..40 {
            server.tick_npcs();
        }
        assert_eq!(
            server.npcs.get(pal).expect("still guarded").ai[0],
            0.0,
            "a guarded pal stays in its waiting state however close you stand"
        );

        // Kill its own two, which by now have woken and cleared their own back-links: the pal names
        // them in `ai[1]`/`ai[2]` and that is what still holds it. The stranger stays.
        let held = server.npcs.get(pal).expect("still there");
        let mine = [held.ai[1], held.ai[2]];
        assert!(
            mine.iter().all(|h| *h > 0.0),
            "the pal should be holding a handle to each of its own guards: {mine:?}"
        );
        for handle in mine {
            let slot = u8::try_from(handle as i32 - 1).expect("a real slot");
            assert!(
                server
                    .npcs
                    .get(slot)
                    .is_some_and(|n| n.npc_type == PAL_ESCORT),
                "handle {handle} should name one of its own Goblin Archers"
            );
            server.npcs.remove(slot);
        }
        assert_eq!(guards(&server).len(), 1, "the stranger is still out there");

        // With both its own guards dead and a player already inside `PAL_APPROACH`, the pal moves
        // straight to its payout countdown next tick and pays out `PAL_PAYOUT_TICKS` (120) ticks
        // later. A window well past that but nowhere near the old 400-tick bound catches the payout
        // happening on schedule rather than eventually, by luck, once some unrelated condition frees
        // the slot.
        let mut collected_at = None;
        for tick in 0..150 {
            server.tick_npcs();
            if server.npcs.get(pal).is_none() {
                collected_at = Some(tick);
                break;
            }
        }
        assert_eq!(
            collected_at,
            // One tick to notice both guards are gone and move from state 0 to state 1 (already
            // inside PAL_APPROACH, so this same tick also moves it straight to state 2), then
            // PAL_PAYOUT_TICKS of counting before the payout tick itself.
            Some(PAL_PAYOUT_TICKS as usize + 1),
            "with its guard dead and you on top of it, the payout should land on the tick vanilla's \
             own PAL_PAYOUT_TICKS names, not merely sometime before this loop gives up"
        );
        // Exactly one reward, not "at least one": the payout used to route through the same
        // same-tick expiry `tick_life`'s despawn-radius refresh immediately overwrote, so the pal
        // was never actually removed, kept re-entering its payout arm, and re-queued the reward
        // item every tick until the collecting player happened to fall out of range. A loose
        // `count() > before` assertion passed on that broken behaviour as readily as on the fix.
        let dropped: Vec<i32> = server.items.iter().map(|(_, item)| item.item.id).collect();
        assert_eq!(
            dropped
                .iter()
                .filter(|&&id| id == i32::from(PAL_REWARD_CATTIVA))
                .count(),
            1,
            "a collected Cattiva should leave exactly one item {PAL_REWARD_CATTIVA} behind, not \
             one per tick it stayed alive after paying out: {dropped:?}"
        );
    }
}

/// A blood moon turns the world's critters evil, which is the only way a Corrupt Bunny exists.
///
/// `NPC.UpdateNPC_BloodMoonTransformations` (`NPC.cs:93033-93048`). Our surface-corruption spawn
/// pool used to list type 47 outright, so corrupt bunnies appeared out of nothing in daylight
/// while vanilla's `NPC.Spawner` never spawns one at any depth.
#[cfg(test)]
mod blood_moon_turns_critters_evil {
    use super::*;

    fn server_with_a_bunny(crimson: bool, blood_moon: bool) -> (GameServer, u8) {
        let mut world = crate::world::World::empty(500, 300, "blood moon probe");
        world.crimson = crimson;
        world.blood_moon = blood_moon;
        let mut server = GameServer::new(Config::default(), world);
        let index = server
            .npcs
            .spawn(46, (1000.0, 1000.0))
            .expect("a bunny in the world");
        (server, index)
    }

    #[test]
    fn a_blood_moon_over_a_corruption_world_turns_the_bunny() {
        let (mut server, index) = server_with_a_bunny(false, true);
        server.tick_npcs();
        assert_eq!(
            server.npcs.get(index).expect("still in its slot").npc_type,
            47,
            "the bunny should have turned corrupt where it stood"
        );
    }

    #[test]
    fn a_blood_moon_over_a_crimson_world_turns_it_the_other_way() {
        let (mut server, index) = server_with_a_bunny(true, true);
        server.tick_npcs();
        assert_eq!(server.npcs.get(index).expect("still there").npc_type, 464);
    }

    /// The flag is the gate. An ordinary night leaves the world's bunnies alone.
    #[test]
    fn an_ordinary_night_leaves_the_bunny_alone() {
        let (mut server, index) = server_with_a_bunny(false, false);
        for _ in 0..10 {
            server.tick_npcs();
        }
        assert_eq!(
            server.npcs.get(index).expect("still there").npc_type,
            46,
            "nothing but a blood moon converts anything"
        );
    }

    /// And it stays converted rather than flickering: the second tick finds a Corrupt Bunny, which
    /// has no evil form of its own, so nothing happens to it.
    #[test]
    fn a_converted_critter_is_not_converted_again() {
        let (mut server, index) = server_with_a_bunny(false, true);
        server.tick_npcs();
        server.tick_npcs();
        assert_eq!(server.npcs.get(index).expect("still there").npc_type, 47);
    }
}

/// Purification Powder turning a Tortured Soul into the Tax Collector, the one recruitment in the
/// game that is a thrown item rather than a conversation.
#[cfg(test)]
mod purification_powder_turns_the_tortured_soul {
    use super::*;
    use crate::game::projectile::{POWDER_SIZE, PURIFICATION_POWDER};
    use crate::game::spawn::TORTURED_SOUL;

    /// Where the soul stands, and the two corners its 18-by-40 hitbox spans.
    const SOUL: (f32, f32) = (1000.0, 500.0);

    fn server_with_a_soul() -> (GameServer, u8, mpsc::Receiver<Bytes>) {
        let world = crate::world::World::empty(500, 300, "powder probe");
        let mut server = GameServer::new(Config::default(), world);
        let (out_tx, out_rx) = mpsc::channel(256);
        let mut player = Player::new(0, "127.0.0.1:1".parse().expect("loopback"), out_tx);
        player.state = ConnState::Playing;
        player.position = SOUL;
        server.players[0] = Some(player);
        let index = server
            .npcs
            .spawn(TORTURED_SOUL, SOUL)
            .expect("a Tortured Soul in the world");
        (server, index, out_rx)
    }

    /// Hand the server a packet-27 the way a real client's throw arrives: the projectile's own
    /// top-left corner and the velocity it left with, nothing else.
    fn throw(server: &mut GameServer, at: (f32, f32), velocity: (f32, f32)) {
        let sync = terrustia_proto::projectile::SyncProjectile {
            key: terrustia_proto::projectile::ProjectileKey {
                owner: 0,
                index: 3,
                generation: 1,
            },
            position: at,
            velocity,
            projectile_type: PURIFICATION_POWDER as i16,
            ai: [0.0; terrustia_proto::projectile::MAX_AI],
            banner: 0,
            damage: 0,
            knockback: 0.0,
            original_damage: 0,
        };
        let frame = sync.encode().expect("a powder encodes");
        server
            .on_client_projectile(0, &frame[3..])
            .expect("the server takes the throw");
    }

    /// The whole mechanism, end to end: a throw that lands short, drifts on, and turns the soul.
    ///
    /// The powder starts clear of the soul on purpose. Its box is 64 wide with its right edge at
    /// 980 against a soul beginning at 1000, so nothing overlaps at the moment the packet arrives:
    /// a check done at throw time would find nothing here for ever, which is the whole reason the
    /// cloud is followed for its 180 ticks (`Projectile.cs:24366-24372`, `aiStyle == 6`) instead.
    ///
    /// Fails before the fix in the only way it could: nothing in this server transformed a 534 at
    /// all, so the assert below found a Tortured Soul still standing there.
    #[test]
    fn a_powder_thrown_at_a_tortured_soul_turns_him_into_the_tax_collector() {
        let (mut server, index, _out) = server_with_a_soul();
        let short = (SOUL.0 - POWDER_SIZE.0 - 20.0, SOUL.1 - 12.0);
        assert!(
            short.0 + POWDER_SIZE.0 < SOUL.0,
            "the throw has to start clear of the soul or this proves nothing"
        );
        throw(&mut server, short, (4.0, 0.0));

        server.tick_powders();
        assert_eq!(
            server.npcs.get(index).expect("still there").npc_type,
            TORTURED_SOUL,
            "one tick of drift is not enough to reach him yet"
        );

        for _ in 0..30 {
            server.tick_powders();
        }
        assert_eq!(
            server.npcs.get(index).expect("still there").npc_type,
            crate::game::server::TAX_COLLECTOR,
            "the drifting cloud never reached the soul"
        );
        assert!(
            server.world.progress.saved_tax_collector,
            "without the flag the underworld would keep offering Tortured Souls, and his arrival \
             would never fire"
        );
    }

    /// A powder thrown the other way is just dust. Vanilla's test is a rectangle intersection and
    /// nothing else, so a miss has to stay a miss for the whole three seconds the cloud lasts.
    #[test]
    fn a_powder_thrown_away_from_him_leaves_him_alone() {
        let (mut server, index, _out) = server_with_a_soul();
        throw(
            &mut server,
            (SOUL.0 - POWDER_SIZE.0 - 20.0, SOUL.1 - 12.0),
            (-4.0, 0.0),
        );

        for _ in 0..200 {
            server.tick_powders();
        }
        assert_eq!(
            server.npcs.get(index).expect("still there").npc_type,
            TORTURED_SOUL,
        );
        assert!(!server.world.progress.saved_tax_collector);
        assert!(
            server.powders.is_empty(),
            "the cloud dies at 180 ticks (`Projectile.cs:24372`), it does not accumulate"
        );
    }

    /// Nothing to purify, nothing to follow: the server does not carry a cloud around a world with
    /// no Tortured Soul in it, which is every world that has ever had its Tax Collector.
    #[test]
    fn no_soul_means_no_tracking_at_all() {
        let (mut server, index, _out) = server_with_a_soul();
        server.npcs.remove(index);
        throw(&mut server, SOUL, (0.0, 0.0));
        assert!(server.powders.is_empty());
    }

    /// The same cloud frees a bound Yellow Slime, which is the other half of vanilla's own routine
    /// (`Projectile.cs:14806-14824`): `Transform(683)` and `unlockedSlimeYellowSpawn = true`.
    ///
    /// Fails before the fix twice over, and the second failure is the interesting one. The
    /// transform did not exist, so 687 stayed 687; and even with a transform it would never have
    /// run, because `on_client_projectile` only followed a cloud while a *Tortured Soul* was alive,
    /// so no powder thrown at a slime was ever tracked in the first place.
    #[test]
    fn a_powder_thrown_at_a_bound_yellow_slime_frees_it() {
        let (mut server, index, _out) = server_with_a_soul();
        server.npcs.remove(index);
        let slime = server
            .npcs
            .spawn(crate::game::spawn::BOUND_TOWN_SLIME_YELLOW, SOUL)
            .expect("a bound Yellow Slime in the world");

        let short = (SOUL.0 - POWDER_SIZE.0 - 20.0, SOUL.1 - 12.0);
        throw(&mut server, short, (4.0, 0.0));
        assert!(
            !server.powders.is_empty(),
            "a slime worth purifying must make the server follow the cloud"
        );

        for _ in 0..30 {
            server.tick_powders();
        }
        assert_eq!(
            server.npcs.get(slime).expect("still there").npc_type,
            crate::game::server::YELLOW_SLIME,
            "the drifting cloud never freed the slime"
        );
        assert!(
            server.world.progress.unlocked_slime_yellow,
            "without the flag every frog draw would keep offering another bound slime"
        );
    }

    /// A world with nothing left to purify follows no cloud at all, slime included.
    #[test]
    fn a_freed_world_follows_nothing() {
        let (mut server, index, _out) = server_with_a_soul();
        server.npcs.remove(index);
        throw(&mut server, SOUL, (0.0, 0.0));
        assert!(server.powders.is_empty());
    }

    /// An ordinary arrow is relayed and forgotten, as every other client projectile is.
    #[test]
    fn only_the_powder_is_followed() {
        let (mut server, _index, _out) = server_with_a_soul();
        let sync = terrustia_proto::projectile::SyncProjectile {
            key: terrustia_proto::projectile::ProjectileKey {
                owner: 0,
                index: 3,
                generation: 1,
            },
            position: SOUL,
            velocity: (0.0, 0.0),
            projectile_type: 1, // WoodenArrowFriendly
            ai: [0.0; terrustia_proto::projectile::MAX_AI],
            banner: 0,
            damage: 5,
            knockback: 0.0,
            original_damage: 5,
        };
        let frame = sync.encode().expect("an arrow encodes");
        server
            .on_client_projectile(0, &frame[3..])
            .expect("the server takes the shot");
        assert!(server.powders.is_empty());
    }
}

/// `NPC.Spawner.fairyLog`, the one thing the spawner reads of the whole mystic-log business
/// (`MysticLogFairiesEvent.ScanWholeOverworldForLogs`).
#[cfg(test)]
mod fallen_log_scan {
    use super::*;

    const FALLEN_LOG: u16 = 488;

    /// A world big enough for the scan's own window to be non-empty. `World::empty` puts the
    /// surface at a third of the height, so this one's is row 300, and the scan runs from row 290
    /// (`worldSurface - 10`) up to row 100, across columns 100 to 699.
    fn logging_country() -> crate::world::World {
        crate::world::World::empty(800, 900, "fallen log probe")
    }

    /// Lay a real log down: three tiles wide and two tall, framed the way `place_object` frames
    /// one, eighteen pixels to a tile.
    fn place_log(world: &mut crate::world::World, x: i32, y: i32) {
        for dx in 0..3i32 {
            for dy in 0..2i32 {
                let tile =
                    terrustia_proto::Tile::framed(FALLEN_LOG, (dx * 18) as i16, (dy * 18) as i16);
                world.set_tile(x + dx, y + dy, tile);
            }
        }
    }

    /// The scan's strides are the log's own footprint, so a log lands on the grid wherever it is
    /// put: `i += 3` across a three-wide log and `num6 -= 2` down a two-tall one
    /// (`MysticLogFairiesEvent.cs:147-161`).
    ///
    /// Every offset is tried rather than one, because a stride that was one out (4 and 3, say)
    /// still finds a log at some offsets and misses it at others, and a single placement would
    /// pass against the wrong constant more often than not.
    ///
    /// Neutralised by widening the strides to `step_by(4)` and `step_by(3)`: "a log at (300, 201)
    /// went unseen on load".
    #[test]
    fn a_log_anywhere_in_the_overworld_is_found() {
        for dx in 0..6 {
            for dy in 0..6 {
                let mut world = logging_country();
                let (x, y) = (300 + dx, 200 + dy);
                place_log(&mut world, x, y);
                let mut server = GameServer::new(Config::default(), world);
                assert!(server.fairy_log, "a log at ({x}, {y}) went unseen on load");

                // ...and again at dusk, which is the other moment vanilla rescans
                // (`Main.cs:66212`).
                server.fairy_log = false;
                server.roll_dusk_events();
                assert!(server.fairy_log, "a log at ({x}, {y}) went unseen at dusk");
            }
        }
    }

    /// A world with no log in it, and a world whose log is under water, both leave the flag down.
    ///
    /// The flooded case is `tile.liquid == 0` (`MysticLogFairiesEvent.cs:155`), which is the one
    /// clause of the three that is easy to drop as decoration.
    ///
    /// Neutralised by dropping `tile.liquid == 0` from the scan: the flooded assertion fails, "a
    /// drowned log still counted".
    #[test]
    fn no_log_and_a_drowned_log_both_leave_the_flag_down() {
        let bare = GameServer::new(Config::default(), logging_country());
        assert!(!bare.fairy_log, "a world with no logs in it claimed one");

        let mut flooded = logging_country();
        place_log(&mut flooded, 300, 200);
        for dx in 0..3 {
            for dy in 0..2 {
                let tile = flooded
                    .tile(300 + dx, 200 + dy)
                    .with_liquid(terrustia_proto::tile::Liquid::Water, 255);
                flooded.set_tile(300 + dx, 200 + dy, tile);
            }
        }
        let drowned = GameServer::new(Config::default(), flooded);
        assert!(!drowned.fairy_log, "a drowned log still counted");
    }

    /// The window is the overworld and only the overworld: the hundred columns at each edge and
    /// everything below `worldSurface - 10` are outside it (`MysticLogFairiesEvent.cs:139-152`).
    ///
    /// Neutralised by starting the column loop at 0 instead of 100: the left-margin assertion
    /// fails. Neutralised again by dropping the `- 10` from the row start: the below-surface
    /// assertion fails, a log at row 295 counting when it should not.
    #[test]
    fn the_margins_and_the_underground_are_outside_the_window() {
        for (x, y, what) in [
            (20, 200, "in the left margin"),
            (750, 200, "in the right margin"),
            (300, 295, "below the surface line"),
            (300, 40, "above the scan's ceiling"),
        ] {
            let mut world = logging_country();
            place_log(&mut world, x, y);
            let server = GameServer::new(Config::default(), world);
            assert!(!server.fairy_log, "a log {what} counted");
        }
    }

    /// What the sweep costs, on the largest world the game makes and in its worst case.
    ///
    /// The worst case is a world with no log at all: the early return that makes a logged world
    /// cheap never fires, so every column and every second row is read. A large world is 8400 by
    /// 2400 with its surface around row 800, which is 2767 columns by 346 rows, so roughly 957,000
    /// tile reads.
    ///
    /// It is not on a per-tick path. It runs once at world load and once at each dusk, which at the
    /// game's own day length is one sweep per 24 minutes of play.
    ///
    /// Measured on this machine, release build, with several build lanes running beside it: 3.2 to
    /// 4.9 ms a sweep across runs. Against a 16.67 ms frame that is a fifth to a third of the one
    /// tick a night that pays it, and a world that has a log in it, which is nearly all of them,
    /// returns at the first one and pays a fraction of that.
    #[test]
    #[ignore]
    fn measure_the_fallen_log_sweep() {
        let mut world = crate::world::World::empty(8400, 2400, "sweep bench");
        world.surface = 800;
        let mut server = GameServer::new(Config::default(), world);

        const SWEEPS: u32 = 10;
        let start = std::time::Instant::now();
        for _ in 0..SWEEPS {
            server.scan_for_fallen_logs();
            assert!(!std::hint::black_box(server.fairy_log));
        }
        let each = start.elapsed().as_secs_f64() / f64::from(SWEEPS) * 1e3;
        println!("scan_for_fallen_logs, no log in a large world: {each:.3} ms/sweep");
    }

    /// What the pal wiring costs a tick of the whole NPC table.
    ///
    /// Four things were added to the per-NPC path and none of them is a scan:
    ///
    /// * `own_escorts` is `npc.stats.ai_style == 127`, and the two-handle unpack behind it runs
    ///   only for a pal, of which a world holds at most one.
    /// * the `parent` resolution gained an `or_else` that runs only when `follows_boss` is `None`
    ///   and is then `npc.npc_type == 111 && npc.ai[3] < 0.0`.
    /// * the style-3 arm gained `npc.npc_type == 111`, paid only by fighters.
    /// * `ai_out.reward.take()`, one `Option` read, beside the twenty that were already there.
    ///
    /// The claim being measured is that this is not visible against the tick it sits in, which
    /// already runs a zone scan, a physics step and a routine per NPC. Two hundred slots filled with
    /// ordinary fighters, release build, a thousand ticks, three runs of each arm:
    ///
    /// * with the wiring in place: 58.8, 59.2, 62.3 us/tick
    /// * with all four removed by hand and rerun in the same build: 57.5, 58.0, 61.4 us/tick
    ///
    /// The gap between the two medians is 1.2 us and the spread *within* each arm is 3.5, so the
    /// honest reading is "no measurable cost" rather than any number. Against a 16.67 ms frame the
    /// whole two-hundred-NPC tick is a third of one per cent.
    #[test]
    #[ignore]
    fn measure_what_the_pal_wiring_costs_a_tick() {
        let mut world = crate::world::World::empty(2000, 800, "npc tick bench");
        world.surface = 200;
        for x in 0..world.width() {
            for y in 300..world.height() {
                world.set_tile(x, y, terrustia_proto::Tile::block(1));
            }
        }
        let mut server = GameServer::new(Config::default(), world);
        let (out_tx, _out_rx) = tokio::sync::mpsc::channel(4096);
        let mut player = Player::new(0, "127.0.0.1:1".parse().expect("loopback"), out_tx);
        player.state = ConnState::Playing;
        player.position = (1000.0 * 16.0, 299.0 * 16.0);
        server.players[0] = Some(player);

        // Zombies: `ai_style` 3, which is the arm the guard branch was added to.
        let mut filled = 0;
        while server
            .npcs
            .spawn(3, ((900 + filled % 200) as f32 * 16.0, 299.0 * 16.0))
            .is_some()
        {
            filled += 1;
        }

        const TICKS: u32 = 1_000;
        // One pass to warm whatever the first tick builds, then the measured run.
        server.tick_npcs();
        let start = std::time::Instant::now();
        for _ in 0..TICKS {
            server.tick_npcs();
        }
        let each = start.elapsed().as_secs_f64() / f64::from(TICKS) * 1e6;
        println!("tick_npcs, {filled} fighters: {each:.1} us/tick");
    }
}
