//! Style 43 — the Queen Bee.
//!
//! Three attacks, picked at random and never the same one twice running: she **charges** you along
//! a level line, she **calls bees** from directly above, and she **spits stingers** from higher
//! still. Between them she returns to a chooser state, which is the only place the next attack is
//! decided — so the fight has a rhythm you can read but not predict.
//!
//! In Expert Mode, everything scales twice over. As her health falls her charges get faster, she
//! strings more of them together, and her stingers come three times as often — none of which
//! Normal mode's health ever moves; only the geography penalty below always applies. And she is
//! furious about geography in either mode: fight her above ground or drag her out of the jungle
//! and every one of those numbers jumps again. That penalty is the game telling you where the
//! fight is supposed to happen.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    BEE, BEE_STRONG, QUEEN_BEE_SPEED, QUEEN_CHARGE, QUEEN_CHARGE_ALIGN, QUEEN_CHARGE_EXPERT,
    QUEEN_CHARGE_LIMIT, QUEEN_CHARGE_RAGE, QUEEN_CHARGE_SPEED_RAGE_EXPERT, QUEEN_CHARGES,
    QUEEN_CHARGING, QUEEN_CHASING, QUEEN_CHOOSING, QUEEN_CLIMB_ACCEL_RAGE_EXPERT, QUEEN_CLIMBING,
    QUEEN_CLIMBING_HOVER_ACCEL_EXPERT, QUEEN_DEFENSE_RAMP, QUEEN_EXPERT_STEPS, QUEEN_GIVE_UP,
    QUEEN_HOVER, QUEEN_HOVER_ACCEL, QUEEN_LEAVING, QUEEN_STANDOFF, QUEEN_STING_ABOVE,
    QUEEN_STING_EVERY, QUEEN_STING_EVERY_ENRAGED, QUEEN_STING_EVERY_EXPERT,
    QUEEN_STING_SPEED_EXPERT, QUEEN_STING_SPEED_EXPERT_ENRAGED, QUEEN_STINGING, QUEEN_SUMMON_ABOVE,
    QUEEN_SUMMON_CADENCE_RAGE_EXPERT, QUEEN_SUMMON_EVERY, QUEEN_SUMMONING, QUEEN_SUMMONS,
    STINGER_DAMAGE, STINGER_SPEED,
};
use terrustia_proto::projectile::ids::STINGER;

use crate::game::ai::{PLAYER_HEIGHT, PLAYER_WIDTH, Shot, World, can_see, sight};
use crate::game::npc::{Npc, TileView};
use crate::game::npc_ai::{Spawn, Target};

/// What a tick of the fight produced.
#[derive(Debug, Default)]
pub struct Hive {
    pub bees: Vec<Spawn>,
    pub stingers: Vec<Shot>,
}

/// How out of place she is: above ground, or outside the jungle.
///
/// Every speed and cadence in the fight reads this, which is why dragging her out of the hive turns
/// her from a boss into a blender.
fn displeasure<T: TileView>(npc: &Npc, world: &World<'_, T>) -> f32 {
    let mut n = 0.0;
    if npc.position.1 < world.conditions.surface_y {
        n += 1.0;
    }
    if !world.conditions.jungle {
        n += 1.0;
    }
    n
}

/// How many of a set of health thresholds she has passed, and what they add up to.
///
/// Every one of these tables is Expert Mode only in real vanilla (`aiStyle==43`'s dozen or so
/// `Main.expertMode` blocks) — Normal mode never reads any of them, so callers gate on `expert`
/// themselves rather than this function assuming it.
fn wounds(health: f32, table: &[(f32, f32)]) -> f32 {
    table
        .iter()
        .filter(|(threshold, _)| health < *threshold)
        .map(|(_, extra)| extra)
        .sum()
}

/// How many of the three Expert-only steps (1/2, 1/3, 1/5 health) she has passed — real vanilla's
/// `life < lifeMax / 2`, `/ 3`, `/ 5`, reused for both an extra charge and a harder brake.
fn expert_steps(health: f32) -> usize {
    QUEEN_EXPERT_STEPS.iter().filter(|t| health < **t).count()
}

/// Edge one axis toward a wanted velocity, doubling the push while still going the wrong way.
fn close_on(velocity: &mut f32, wanted: f32, accel: f32) {
    if *velocity < wanted {
        *velocity += accel;
        if *velocity < 0.0 && wanted > 0.0 {
            *velocity += accel;
        }
    } else if *velocity > wanted {
        *velocity -= accel;
        if *velocity > 0.0 && wanted < 0.0 {
            *velocity -= accel;
        }
    }
}

/// As [`close_on`], but the wrong-way correction is a *doubled* extra push (`num689 * 2f`) rather
/// than a repeat of the base one. The stinging approach's clear-line branch uses this harder pull
/// (`NPC.cs:31011-31042`).
fn close_on_hard(velocity: &mut f32, wanted: f32, accel: f32) {
    if *velocity < wanted {
        *velocity += accel;
        if *velocity < 0.0 && wanted > 0.0 {
            *velocity += accel * 2.0;
        }
    } else if *velocity > wanted {
        *velocity -= accel;
        if *velocity > 0.0 && wanted < 0.0 {
            *velocity -= accel * 2.0;
        }
    }
}

/// Whether a 1x1 muzzle point has a clear line to the player's box.
///
/// This is vanilla's `Collision.CanHit(muzzle, 1, 1, player.position, player.width, player.height)`,
/// the gate the game puts in front of both the bee call and the stinger spit so she never fires
/// blind through solid ground.
fn muzzle_can_hit<T: TileView>(tiles: &T, muzzle: (f32, f32), target: Target) -> bool {
    sight::can_hit(
        tiles,
        muzzle,
        (1, 1),
        (
            target.center.0 - PLAYER_WIDTH as f32 / 2.0,
            target.center.1 - PLAYER_HEIGHT as f32 / 2.0,
        ),
        (PLAYER_WIDTH, PLAYER_HEIGHT),
    )
}

/// Drive the Queen Bee for a tick.
pub fn update<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) -> Hive {
    let mut hive = Hive::default();
    let expert = world.conditions.expert;
    let health = npc.life as f32 / npc.life_max as f32;

    // Her defence climbs as her health falls: the fight gets harder as you win it — Expert Mode
    // only (`if (Main.expertMode) { defense = defDefense + num657; }`). Written to the live
    // `defense` field combat actually reads, from the type's own baseline each tick rather than
    // compounding on whatever the field already held.
    if expert {
        npc.defense = npc.stats.defense + (QUEEN_DEFENSE_RAMP * (1.0 - health)) as i32;
    }

    let Some(target) = world.target else {
        npc.ai[0] = QUEEN_LEAVING;
        npc.time_left = npc.time_left.min(10);
        return hive;
    };
    let cross = displeasure(npc, world);
    let (cx, cy) = npc.center();
    let reach = ((target.center.0 - cx).powi(2) + (target.center.1 - cy).powi(2)).sqrt();

    if npc.ai[0] != QUEEN_LEAVING {
        npc.time_left = npc.time_left.max(60);
        if reach > QUEEN_GIVE_UP {
            npc.ai[0] = QUEEN_CHASING;
            npc.dirty = true;
        }
    }
    if !target.alive {
        npc.ai[0] = QUEEN_LEAVING;
        npc.dirty = true;
    }

    if npc.ai[0] == QUEEN_CHASING {
        // QB-1: out-running her (past 3000) does not despawn her. She gives chase, flying toward
        // the player at 14 with heavy smoothing, and drops back into the chooser the instant they
        // are within 2000 again (`NPC.cs:31053-31076`). Only a dead player (`QUEEN_LEAVING`)
        // actually ends the fight. The old code lumped this state in with leaving and capped
        // time_left to 10, so a player who briefly outran her killed the fight outright.
        let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
        let len = (dx * dx + dy * dy).sqrt().max(f32::MIN_POSITIVE);
        let toward = (dx / len * 14.0, dy / len * 14.0);
        npc.velocity.0 = (npc.velocity.0 * 14.0 + toward.0) / 15.0;
        npc.velocity.1 = (npc.velocity.1 * 14.0 + toward.1) / 15.0;
        npc.direction = if npc.velocity.0 < 0.0 { -1 } else { 1 };
        npc.sprite_direction = npc.direction;
        if reach < 2000.0 {
            npc.ai[0] = QUEEN_CHOOSING;
        }
        npc.dirty = true;
        return hive;
    }

    if npc.ai[0] == QUEEN_LEAVING {
        // Everyone is dead: she leaves the field for good (EncourageDespawn, `NPC.cs:30434-30471`).
        npc.velocity.1 *= 0.98;
        npc.direction = if npc.velocity.0 < 0.0 { -1 } else { 1 };
        npc.velocity.0 += 0.08 * f32::from(npc.direction);
        npc.time_left = npc.time_left.min(10);
        npc.sprite_direction = npc.direction;
        npc.dirty = true;
        return hive;
    }

    if npc.ai[0] == QUEEN_CHOOSING {
        // A new attack, and never the one she just finished.
        let last = npc.ai[1];
        let choices = [QUEEN_CHARGING, QUEEN_CLIMBING, QUEEN_STINGING];
        let mut pick = choices[rng.random_range(0..choices.len())];
        let mut guard = 0;
        while pick == last && guard < 8 {
            pick = choices[rng.random_range(0..choices.len())];
            guard += 1;
        }
        npc.ai[0] = pick;
        npc.ai[1] = 0.0;
        npc.ai[2] = 0.0;
        npc.dirty = true;
        return hive;
    }

    if npc.ai[0] == QUEEN_CHARGING {
        // Two charges, one more at each of three health steps in Expert Mode, and one more again
        // for the biome penalty, in any mode.
        let extra_charges = if expert {
            expert_steps(health) as i32
        } else {
            0
        };
        let runs = QUEEN_CHARGES + extra_charges + cross as i32;
        if npc.ai[1] > (2 * runs) as f32 && npc.ai[1] % 2.0 == 0.0 {
            npc.ai[0] = QUEEN_CHOOSING;
            npc.ai[1] = 0.0;
            npc.ai[2] = 0.0;
            npc.dirty = true;
            return hive;
        }

        if npc.ai[1] % 2.0 == 0.0 {
            // Lining up. Level with her target, she commits.
            let align = QUEEN_CHARGE_ALIGN + QUEEN_CHARGE_ALIGN * cross;
            if (cy - target.center.1).abs() < align {
                npc.ai[1] += 1.0;
                npc.ai[2] = 0.0;
                // Expert Mode's own, higher base and its own, larger bonus at the same four
                // thresholds; Normal mode's charge speed never moves with health at all.
                let speed = if expert {
                    QUEEN_CHARGE_EXPERT + wounds(health, &QUEEN_CHARGE_SPEED_RAGE_EXPERT)
                } else {
                    QUEEN_CHARGE
                } + 7.0 * cross;
                let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
                let k = speed / (dx * dx + dy * dy).sqrt().max(0.01);
                npc.velocity = (dx * k, dy * k);
                npc.sprite_direction = npc.direction;
                npc.dirty = true;
                return hive;
            }
            // Not level yet: climb or dive to their line, holding a standoff. Expert Mode's own
            // bonus to both, at the same four thresholds again; Normal mode gets neither.
            let (climb_bonus, accel_bonus) = if expert {
                (
                    wounds(health, &QUEEN_CHARGE_RAGE),
                    wounds(health, &QUEEN_CLIMB_ACCEL_RAGE_EXPERT),
                )
            } else {
                (0.0, 0.0)
            };
            let climb = QUEEN_HOVER + climb_bonus + 3.0 * cross;
            let accel = 0.15 + accel_bonus + 0.5 * cross;
            if cy < target.center.1 {
                npc.velocity.1 += accel;
            } else {
                npc.velocity.1 -= accel;
            }
            npc.velocity.1 = npc.velocity.1.clamp(-climb, climb);

            let across = (cx - target.center.0).abs();
            if across > QUEEN_STANDOFF.1 {
                npc.velocity.0 += 0.15 * f32::from(npc.direction);
            } else if across < QUEEN_STANDOFF.0 {
                npc.velocity.0 -= 0.15 * f32::from(npc.direction);
            } else {
                npc.velocity.0 *= 0.8;
            }
            npc.velocity.0 = npc.velocity.0.clamp(-16.0, 16.0);
            npc.sprite_direction = npc.direction;
            npc.dirty = true;
            return hive;
        }

        // Mid-charge: it ends when she has overshot far enough, and then she brakes to a stop.
        // The tighter, health-tiered leash below is Expert Mode only; Normal always uses the flat
        // one.
        npc.direction = if npc.velocity.0 < 0.0 { -1 } else { 1 };
        npc.sprite_direction = npc.direction;
        let mut limit = if expert {
            match health {
                h if h < 0.1 => 300.0,
                h if h < 0.25 => 450.0,
                h if h < 0.5 => 500.0,
                h if h < 0.75 => 550.0,
                _ => QUEEN_CHARGE_LIMIT,
            }
        } else {
            QUEEN_CHARGE_LIMIT
        };
        limit -= 100.0 * cross;
        let away = if cx < target.center.0 { -1 } else { 1 };
        if (npc.direction == away && (cx - target.center.0).abs() > limit)
            || (cy - target.center.1).abs() > limit * 1.5
        {
            npc.ai[2] = 1.0;
        }
        if npc.ai[2] == 1.0 {
            npc.velocity.0 *= 0.9;
            npc.velocity.1 *= 0.9;
            // Expert Mode brakes harder — one more `*0.9` per step she has passed — and needs to
            // slow further before she counts as stopped.
            let mut stop = 0.1;
            if expert {
                for _ in 0..expert_steps(health) {
                    npc.velocity.0 *= 0.9;
                    npc.velocity.1 *= 0.9;
                    stop += 0.05;
                }
            }
            // Out of her jungle, or above ground, she brakes harder still, in any mode.
            if cross > 0.0 {
                npc.velocity.0 *= 0.7;
                npc.velocity.1 *= 0.7;
            }
            if npc.velocity.0.abs() + npc.velocity.1.abs() < stop {
                npc.ai[2] = 0.0;
                npc.ai[1] += 1.0;
                npc.dirty = true;
            }
        }
        npc.dirty = true;
        return hive;
    }

    if npc.ai[0] == QUEEN_CLIMBING {
        // Getting into position above them to call her bees. Expert Mode accelerates faster.
        let (dx, dy) = (
            target.center.0 - cx,
            target.center.1 - QUEEN_SUMMON_ABOVE - cy,
        );
        let gap = (dx * dx + dy * dy).sqrt();
        if gap < QUEEN_SUMMON_ABOVE {
            npc.ai[0] = QUEEN_SUMMONING;
            npc.ai[1] = 0.0;
            npc.dirty = true;
            return hive;
        }
        let hover_accel = if expert {
            QUEEN_CLIMBING_HOVER_ACCEL_EXPERT
        } else {
            QUEEN_HOVER_ACCEL
        };
        // QB-4: vanilla computes `num677 = num673 / num677` here (a normalised 12-hover speed) but
        // never reads it again (`NPC.cs:30749`) - the accel below closes on the *raw* offset, so
        // there is no 12-cap on her climb. The old code multiplied the target by that dead `k` and
        // so plateaued her at 12.
        close_on(&mut npc.velocity.0, dx, hover_accel);
        close_on(&mut npc.velocity.1, dy, hover_accel);
        npc.sprite_direction = npc.direction;
        npc.dirty = true;
        return hive;
    }

    if npc.ai[0] == QUEEN_SUMMONING {
        // A bee roughly every forty ticks, sooner the more hurt she is — Expert Mode only.
        let cadence_bonus = if expert {
            wounds(health, &QUEEN_SUMMON_CADENCE_RAGE_EXPERT)
        } else {
            0.0
        };
        // Her muzzle, jittered along her facing and rebuilt every tick as vanilla does (`vector77`,
        // `NPC.cs:30787`).
        let muzzle = (
            cx + (rng.random_range(0..20) * i32::from(npc.direction)) as f32,
            npc.position.1 + npc.height() * 0.8,
        );
        npc.ai[1] += 1.0 + cadence_bonus;
        let every = QUEEN_SUMMON_EVERY - 18.0 * cross;
        // The cadence timer counts and the tally climbs whether or not she has a clear line
        // (`flag52`, `ai[2]++`, `NPC.cs:30821-30828`); only the bee itself is line-of-sight gated.
        let mut called = false;
        if npc.ai[1] > every.max(1.0) {
            npc.ai[1] = 0.0;
            npc.ai[2] += 1.0;
            called = true;
        }
        // QB-5: the bee only appears when she can see the player from that muzzle
        // (`Collision.CanHit(vector77, ...) && flag52`, `NPC.cs:30829`). The old code called bees
        // blind through walls.
        if called && muzzle_can_hit(world.tiles, muzzle, target) {
            let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
            let k = QUEEN_BEE_SPEED / (dx * dx + dy * dy).sqrt().max(0.01);
            hive.bees.push(Spawn {
                handle: None,
                npc_type: if rng.random_ratio(1, 2) {
                    BEE
                } else {
                    BEE_STRONG
                },
                position: muzzle,
                velocity: (dx * k, dy * k),
                parent: None,
                ai: [None; 4],
            });
            npc.dirty = true;
        }
        // QB-3: she keeps station on the player while calling her bees. Too far (over 400) or out
        // of sight, she flies back toward them at 14 (`NPC.cs:30845-30890`); close and in view, she
        // holds. The old code only ever held, so a player who walked off during the summon was
        // never followed and the bees came out of empty air behind her.
        let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
        let reach_player = (dx * dx + dy * dy).sqrt();
        if reach_player > 400.0 || !can_see(world.tiles, npc, target) {
            let k = 14.0 / reach_player.max(0.01);
            close_on(&mut npc.velocity.0, dx * k, 0.1);
            close_on(&mut npc.velocity.1, dy * k, 0.1);
        } else {
            npc.velocity.0 *= 0.9;
            npc.velocity.1 *= 0.9;
        }
        if npc.ai[2] > QUEEN_SUMMONS {
            npc.ai[0] = QUEEN_CHOOSING;
            npc.ai[1] = QUEEN_SUMMONING;
            npc.dirty = true;
        }
        npc.sprite_direction = npc.direction;
        npc.dirty = true;
        return hive;
    }

    // Stinging: she hangs high and spits, and only downward.
    //
    // Her muzzle, jittered along her facing and rebuilt every tick (`vector79`, `NPC.cs:30910`).
    let muzzle = (
        cx + (rng.random_range(0..20) * i32::from(npc.direction)) as f32,
        npc.position.1 + npc.height() * 0.8,
    );
    // Offsets from her centre: dx to the player, dy to her sting height (300 above them). Her
    // approach acceleration toward that height is its own, higher, flat value in Expert Mode, plus
    // the biome penalty in any mode. (Vanilla's paired `num688` approach *speed* is dead: it only
    // ever feeds a normalise, `num692 = num688 / num692`, that is never read - which is QB-4.)
    let (dx, dy) = (
        target.center.0 - cx,
        target.center.1 - QUEEN_STING_ABOVE - cy,
    );
    let gap = (dx * dx + dy * dy).sqrt();
    let accel = if expert { 0.075 } else { 0.05 } + 0.2 * cross;

    npc.ai[1] += 1.0;
    // Cadence: flat in Normal mode; a health-tiered ladder, faster throughout, in Expert.
    let mut every = if expert {
        match health {
            h if h < 0.1 => QUEEN_STING_EVERY_ENRAGED,
            h if h < 1.0 / 3.0 => 25.0,
            h if h < 0.5 => 30.0,
            _ => QUEEN_STING_EVERY_EXPERT,
        }
    } else {
        QUEEN_STING_EVERY
    };
    every -= 5.0 * cross;
    let every = every.max(1.0);
    // QB-5: she only spits when she has a clear line to the player from the muzzle
    // (`Collision.CanHit(vector79, ...)`, `NPC.cs:30923`). The old code fired blind through walls.
    if npc.ai[1] % every == every - 1.0
        && npc.position.1 + npc.height() < target.center.1
        && muzzle_can_hit(world.tiles, muzzle, target)
    {
        // The horizontal and vertical scatter are not the same width in real vanilla.
        let scatter_x = (80.0 - 39.0 * cross).max(1.0) as i32;
        let scatter_y = (40.0 - 19.0 * cross).max(1.0) as i32;
        let aim = (
            target.center.0 - muzzle.0 + rng.random_range(-scatter_x..=scatter_x) as f32,
            target.center.1 - muzzle.1 + rng.random_range(-scatter_y..=scatter_y) as f32,
        );
        // Expert Mode's own bonus, and the extra it adds again below a tenth health; Normal mode
        // gains neither.
        let speed_bonus = if expert {
            QUEEN_STING_SPEED_EXPERT
        } else {
            0.0
        } + if expert && health < 0.1 {
            QUEEN_STING_SPEED_EXPERT_ENRAGED
        } else {
            0.0
        };
        let shot_speed = STINGER_SPEED + speed_bonus + 7.0 * cross;
        let k = shot_speed / (aim.0 * aim.0 + aim.1 * aim.1).sqrt().max(0.01);
        hive.stingers.push(Shot {
            projectile: STINGER,
            damage: STINGER_DAMAGE,
            position: muzzle,
            velocity: (aim.0 * k, aim.1 * k),
            time_left: 300,
            ai: [0.0; 3],
        });
        npc.dirty = true;
    }

    // Approach. QB-4: vanilla closes on the *raw* offset (its `num692 = num688 / num692` normalise
    // at `NPC.cs:30972,31010` is computed and never read), so nothing caps her approach speed. The
    // old code multiplied the target by that dead `k` and plateaued her at 4 (6 in Expert).
    // QB-5: which offset she chases, and how hard, turns on her line to the player.
    if !muzzle_can_hit(world.tiles, (muzzle.0, muzzle.1 - 30.0), target) {
        // Line blocked: fly straight at the player from the muzzle, faster outside her jungle
        // (`NPC.cs:30960-31005`, `num689 = 0.5f` when displeased else `0.1f`).
        let hard_accel = if cross > 0.0 { 0.5 } else { 0.1 };
        close_on(&mut npc.velocity.0, target.center.0 - muzzle.0, hard_accel);
        close_on(&mut npc.velocity.1, target.center.1 - muzzle.1, hard_accel);
    } else if gap > 100.0 {
        // Clear line but still short of her sting height: close on it, pulling harder when she is
        // going the wrong way (`NPC.cs:31006-31043`, `num689 * 2f`). A clear line within 100px and
        // she simply coasts.
        close_on_hard(&mut npc.velocity.0, dx, accel);
        close_on_hard(&mut npc.velocity.1, dy, accel);
        npc.sprite_direction = npc.direction;
    }

    // QB-2: the sting phase runs for `cadence * (20 - 5*cross)` ticks (`NPC.cs:31044-31051`,
    // `ai[1] > num693 * num703`), so she fires roughly twenty stingers (fewer out of her jungle)
    // whatever the cadence. In Normal mode with no penalty that is 40 * 20 = 800 ticks, not the
    // flat 300 the old code used, which cut the volley to about a third of its length.
    let phase_ticks = every * (20.0 - 5.0 * cross);
    if npc.ai[1] > phase_ticks {
        npc.ai[0] = QUEEN_CHOOSING;
        npc.ai[1] = QUEEN_STINGING;
        npc.dirty = true;
    }
    npc.sprite_direction = npc.direction;
    npc.dirty = true;
    hive
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::ai::Conditions;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use terrustia_proto::tile::Tile;

    struct Hollow;

    impl TileView for Hollow {
        fn tile(&self, _x: i32, _y: i32) -> Tile {
            Tile::AIR
        }
    }

    /// A solid horizontal band of tiles spanning every column, so a line crossing it is broken.
    struct Walled {
        band: std::ops::Range<i32>,
    }

    impl TileView for Walled {
        fn tile(&self, _x: i32, y: i32) -> Tile {
            if self.band.contains(&y) {
                Tile::block(1)
            } else {
                Tile::AIR
            }
        }
    }

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(43)
    }

    fn queen() -> Npc {
        Npc::new(222, (10_000.0, 10_000.0), 1).expect("queen bee")
    }

    fn hive_world<'a>(tiles: &'a Hollow, target: Option<Target>) -> World<'a, Hollow> {
        World {
            conditions: Conditions {
                jungle: true,
                surface_y: 0.0,
                ..Conditions::default()
            },
            ..crate::game::ai::calm(tiles, target)
        }
    }

    fn walled_world<'a>(tiles: &'a Walled, target: Option<Target>) -> World<'a, Walled> {
        World {
            conditions: Conditions {
                jungle: true,
                surface_y: 0.0,
                ..Conditions::default()
            },
            ..crate::game::ai::calm(tiles, target)
        }
    }

    fn player_at(x: f32, y: f32) -> Target {
        Target {
            slot: 0,
            center: (x, y),
            velocity: (0.0, 0.0),
            alive: true,
        }
    }

    #[test]
    fn she_never_picks_the_same_attack_twice_running() {
        let tiles = Hollow;
        let mut q = queen();
        let t = Some(player_at(10_400.0, 10_000.0));
        let mut r = rng();
        for last in [QUEEN_CHARGING, QUEEN_CLIMBING, QUEEN_STINGING] {
            for _ in 0..40 {
                q.ai[0] = QUEEN_CHOOSING;
                q.ai[1] = last;
                update(&mut q, &hive_world(&tiles, t), &mut r);
                assert_ne!(q.ai[0], last, "she should not repeat herself");
            }
        }
    }

    #[test]
    fn she_charges_only_once_she_is_level_with_you() {
        let tiles = Hollow;
        let mut q = queen();
        q.ai[0] = QUEEN_CHARGING;
        // Well above them: she should be closing the gap rather than charging.
        let below = Some(player_at(10_400.0, 11_000.0));
        update(&mut q, &hive_world(&tiles, below), &mut rng());
        assert_eq!(q.ai[1] % 2.0, 0.0, "still lining up");

        // Level: she commits.
        let (cx, cy) = q.center();
        let level = Some(player_at(cx + 400.0, cy));
        update(&mut q, &hive_world(&tiles, level), &mut rng());
        assert_eq!(q.ai[1] % 2.0, 1.0, "should have committed");
        let speed = q.velocity.0.hypot(q.velocity.1);
        assert!(
            (speed - QUEEN_CHARGE).abs() < 0.5,
            "at her charge speed, got {speed}"
        );
    }

    /// Real vanilla (`NPC.cs`, `aiStyle==43`): `num664` only gains its four health-tiered bonuses
    /// — and only replaces its 12-baseline with 16 — under `Main.expertMode`. On the unfixed code
    /// (which read `wounds(npc)` unconditionally, in every difficulty) this would have passed for
    /// the wrong reason; splitting it into these two proves the gating is real.
    #[test]
    fn a_wounded_queen_charges_faster_in_expert_mode() {
        let tiles = Hollow;
        let charge_speed = |life_fraction: f32| {
            let mut q = queen();
            q.life = (q.life_max as f32 * life_fraction) as i32;
            q.ai[0] = QUEEN_CHARGING;
            let (cx, cy) = q.center();
            let level = Some(player_at(cx + 400.0, cy));
            let mut w = hive_world(&tiles, level);
            w.conditions.expert = true;
            update(&mut q, &w, &mut rng());
            q.velocity.0.hypot(q.velocity.1)
        };
        assert!(
            charge_speed(0.05) > charge_speed(1.0),
            "she should speed up as she dies, in Expert Mode"
        );
    }

    #[test]
    fn normal_mode_charge_speed_never_scales_with_health() {
        let tiles = Hollow;
        let charge_speed = |life_fraction: f32| {
            let mut q = queen();
            q.life = (q.life_max as f32 * life_fraction) as i32;
            q.ai[0] = QUEEN_CHARGING;
            let (cx, cy) = q.center();
            let level = Some(player_at(cx + 400.0, cy));
            update(&mut q, &hive_world(&tiles, level), &mut rng());
            q.velocity.0.hypot(q.velocity.1)
        };
        assert!(
            (charge_speed(0.05) - charge_speed(1.0)).abs() < 1e-4,
            "Normal mode's charge speed should never move with health"
        );
    }

    #[test]
    fn expert_mode_strings_more_charges_together_at_low_health() {
        let tiles = Hollow;
        // At Normal's own count (2 + the biome penalty), still charging means Expert strung on
        // another; moved on to choosing means it did not.
        let still_charging = |expert: bool| {
            let mut q = queen();
            q.life = (q.life_max as f32 * 0.03) as i32; // below all three Expert steps
            q.ai[0] = QUEEN_CHARGING;
            q.ai[1] = 2.0 * QUEEN_CHARGES as f32 + 2.0; // one cycle past Normal's own count
            let (cx, cy) = q.center();
            // Not level with her, so this tick cannot commit to yet another charge itself.
            let t = Some(player_at(cx + 400.0, cy + 2000.0));
            let mut w = hive_world(&tiles, t);
            w.conditions.expert = expert;
            update(&mut q, &w, &mut rng());
            q.ai[0] == QUEEN_CHARGING
        };
        assert!(
            !still_charging(false),
            "Normal mode should have moved on to choosing by now"
        );
        assert!(
            still_charging(true),
            "Expert Mode should still have another charge to string on"
        );
    }

    #[test]
    fn expert_mode_climbs_faster_toward_the_charge_line() {
        let tiles = Hollow;
        let velocity_after_one_tick = |expert: bool| {
            let mut q = queen();
            q.life = (q.life_max as f32 * 0.05) as i32;
            q.ai[0] = QUEEN_CHARGING;
            let (cx, cy) = q.center();
            // Far below her, well outside the alignment window: she should be climbing.
            let t = Some(player_at(cx, cy + 2000.0));
            let mut w = hive_world(&tiles, t);
            w.conditions.expert = expert;
            update(&mut q, &w, &mut rng());
            q.velocity.1
        };
        assert!(
            velocity_after_one_tick(true) > velocity_after_one_tick(false) * 1.5,
            "Expert's steeper acceleration at low health should show up after a single tick"
        );
    }

    #[test]
    fn expert_mode_tightens_the_charge_overshoot_leash() {
        let tiles = Hollow;
        let overshot = |expert: bool| {
            let mut q = queen();
            q.life = (q.life_max as f32 * 0.05) as i32; // Expert's own 300px tier
            q.ai[0] = QUEEN_CHARGING;
            q.ai[1] = 1.0; // mid-charge
            q.velocity.0 = -5.0; // already moving away from where the target is
            let (cx, cy) = q.center();
            let t = Some(player_at(cx + 450.0, cy));
            let mut w = hive_world(&tiles, t);
            w.conditions.expert = expert;
            update(&mut q, &w, &mut rng());
            q.ai[2] == 1.0
        };
        assert!(
            !overshot(false),
            "450px is well inside Normal's flat 600px leash"
        );
        assert!(
            overshot(true),
            "but well outside Expert's own 300px leash at 5% health"
        );
    }

    #[test]
    fn expert_mode_brakes_harder_out_of_a_charge() {
        let tiles = Hollow;
        let exited_braking = |expert: bool| {
            let mut q = queen();
            q.life = (q.life_max as f32 * 0.05) as i32; // all three Expert brake steps
            q.ai[0] = QUEEN_CHARGING;
            q.ai[1] = 1.0;
            q.ai[2] = 1.0; // already braking
            q.velocity = (0.2, 0.0);
            let (cx, cy) = q.center();
            let t = Some(player_at(cx, cy)); // on top of her: no overshoot re-trigger this tick
            let mut w = hive_world(&tiles, t);
            w.conditions.expert = expert;
            update(&mut q, &w, &mut rng());
            q.ai[2] == 0.0
        };
        assert!(
            !exited_braking(false),
            "Normal's single *0.9 should not have slowed her enough yet"
        );
        assert!(
            exited_braking(true),
            "Expert's extra compounding *0.9 per step should finish the brake"
        );
    }

    #[test]
    fn expert_mode_climbs_to_summon_faster() {
        let tiles = Hollow;
        let velocity_after_one_tick = |expert: bool| {
            let mut q = queen();
            q.ai[0] = QUEEN_CLIMBING;
            let (cx, cy) = q.center();
            // Far below where she wants to hover, so she accelerates upward this tick.
            let t = Some(player_at(cx, cy + 2000.0));
            let mut w = hive_world(&tiles, t);
            w.conditions.expert = expert;
            update(&mut q, &w, &mut rng());
            q.velocity.1.abs()
        };
        assert!(
            velocity_after_one_tick(true) > velocity_after_one_tick(false),
            "Expert's 0.1 acceleration should out-pace Normal's 0.07"
        );
    }

    #[test]
    fn she_calls_bees_from_above() {
        let tiles = Hollow;
        let mut q = queen();
        q.ai[0] = QUEEN_SUMMONING;
        let (cx, cy) = q.center();
        let t = Some(player_at(cx, cy + 300.0));
        let mut r = rng();
        let mut called = Vec::new();
        for _ in 0..200 {
            called.extend(update(&mut q, &hive_world(&tiles, t), &mut r).bees);
        }
        assert!(!called.is_empty(), "she should have called bees");
        assert!(
            called
                .iter()
                .all(|b| b.npc_type == BEE || b.npc_type == BEE_STRONG)
        );
        assert!(
            called.iter().all(|b| b.velocity.1 > 0.0),
            "and sent them down at the player"
        );
    }

    #[test]
    fn she_gives_up_calling_after_a_handful() {
        let tiles = Hollow;
        let mut q = queen();
        q.ai[0] = QUEEN_SUMMONING;
        let (cx, cy) = q.center();
        let t = Some(player_at(cx, cy + 300.0));
        let mut r = rng();
        for _ in 0..2000 {
            update(&mut q, &hive_world(&tiles, t), &mut r);
            if q.ai[0] == QUEEN_CHOOSING {
                break;
            }
        }
        assert_eq!(q.ai[0], QUEEN_CHOOSING, "and then move on");
    }

    /// Real vanilla (`num681`/health-tier additions to `ai[1]`) is Expert Mode only; the unfixed
    /// code added the health-tier bonus in every difficulty.
    #[test]
    fn expert_mode_calls_bees_faster_at_low_health() {
        let tiles = Hollow;
        let cadence = |expert: bool| {
            let mut q = queen();
            q.life = (q.life_max as f32 * 0.05) as i32; // all four thresholds crossed
            q.ai[0] = QUEEN_SUMMONING;
            let t = Some(player_at(10_000.0, 10_000.0));
            let mut w = hive_world(&tiles, t);
            w.conditions.expert = expert;
            update(&mut q, &w, &mut rng());
            q.ai[1]
        };
        assert!(
            cadence(true) > cadence(false),
            "Expert's health-scaled bonus should push ai[1] up faster in a single tick"
        );
    }

    #[test]
    fn she_spits_stingers_downward_only() {
        let tiles = Hollow;
        let mut q = queen();
        q.ai[0] = QUEEN_STINGING;
        let (cx, cy) = q.center();
        let below = Some(player_at(cx + 50.0, cy + 500.0));
        let mut r = rng();
        let mut spat = Vec::new();
        for _ in 0..200 {
            spat.extend(update(&mut q, &hive_world(&tiles, below), &mut r).stingers);
        }
        assert!(!spat.is_empty(), "she should have spat");
        assert_eq!(spat[0].projectile, STINGER);
        assert!(spat.iter().all(|s| s.velocity.1 > 0.0));

        // With the player above her, nothing leaves.
        let mut high = queen();
        high.ai[0] = QUEEN_STINGING;
        let above = Some(player_at(cx + 50.0, cy - 500.0));
        for _ in 0..200 {
            assert!(
                update(&mut high, &hive_world(&tiles, above), &mut r)
                    .stingers
                    .is_empty()
            );
        }
    }

    #[test]
    fn expert_mode_approaches_its_stinging_height_faster() {
        let tiles = Hollow;
        let velocity_after_one_tick = |expert: bool| {
            let mut q = queen();
            q.ai[0] = QUEEN_STINGING;
            let (cx, cy) = q.center();
            let t = Some(player_at(cx, cy + 2000.0)); // far below her sting height
            let mut w = hive_world(&tiles, t);
            w.conditions.expert = expert;
            update(&mut q, &w, &mut rng());
            q.velocity.1.abs()
        };
        assert!(
            velocity_after_one_tick(true) > velocity_after_one_tick(false),
            "Expert's 0.075 acceleration should out-pace Normal's 0.05"
        );
    }

    /// Real vanilla's cadence (`num693`) and speed (`num694`) bonuses are both Expert Mode only.
    /// The unfixed code applied the maximum speed bonus (`+5.0`) in every difficulty and used the
    /// wrong, always-on cadence table, so this fails on it.
    #[test]
    fn expert_mode_spits_stingers_faster_and_harder_at_low_health() {
        let tiles = Hollow;
        let spat_at = |expert: bool| {
            let mut q = queen();
            q.ai[0] = QUEEN_STINGING;
            q.life = (q.life_max as f32 * 0.05) as i32; // Expert's own enraged tier
            let (cx, cy) = q.center();
            let below = Some(player_at(cx + 50.0, cy + 500.0));
            let mut w = hive_world(&tiles, below);
            w.conditions.expert = expert;
            let mut r = rng();
            let mut spat = Vec::new();
            for _ in 0..200 {
                spat.extend(update(&mut q, &w, &mut r).stingers);
            }
            spat
        };
        let normal = spat_at(false);
        let expert = spat_at(true);
        assert!(
            expert.len() > normal.len(),
            "Expert's 15-tick cadence should fire more often than Normal's flat 40 in 200 ticks"
        );
        assert!(
            expert[0].velocity.0.hypot(expert[0].velocity.1)
                > normal[0].velocity.0.hypot(normal[0].velocity.1),
            "and each shot should leave faster too"
        );
    }

    /// Drag her out of the jungle and every number in the fight moves against you.
    #[test]
    fn taking_her_out_of_the_jungle_enrages_her() {
        let tiles = Hollow;
        let charge = |jungle: bool| {
            let mut q = queen();
            q.ai[0] = QUEEN_CHARGING;
            let (cx, cy) = q.center();
            let level = Some(player_at(cx + 400.0, cy));
            let mut w = hive_world(&tiles, level);
            w.conditions.jungle = jungle;
            update(&mut q, &w, &mut rng());
            q.velocity.0.hypot(q.velocity.1)
        };
        assert!(
            charge(false) > charge(true) + 5.0,
            "she should be much faster outside her hive"
        );
    }

    /// Real vanilla (`if (Main.expertMode) { defense = defDefense + num657; }`) only ever writes
    /// this in Expert Mode, and to the live `defense` field, not the type's own baseline stats.
    /// The unfixed code raised `stats.defense` — a field combat never reads — unconditionally and
    /// cumulatively, so this fails on it both by mode and by field.
    #[test]
    fn her_defence_climbs_as_she_is_worn_down_in_expert_mode() {
        let tiles = Hollow;
        let defence = |life_fraction: f32| {
            let mut q = queen();
            q.life = (q.life_max as f32 * life_fraction) as i32;
            let t = Some(player_at(10_400.0, 10_000.0));
            let mut w = hive_world(&tiles, t);
            w.conditions.expert = true;
            update(&mut q, &w, &mut rng());
            q.defense
        };
        assert!(defence(0.05) > defence(1.0));
    }

    #[test]
    fn normal_mode_defence_never_climbs() {
        let tiles = Hollow;
        let mut q = queen();
        let base = q.defense;
        q.life = (q.life_max as f32 * 0.05) as i32;
        let t = Some(player_at(10_400.0, 10_000.0));
        update(&mut q, &hive_world(&tiles, t), &mut rng());
        assert_eq!(q.defense, base, "Normal mode's defence should never move");
    }

    /// QB-1: out-running her does not despawn her. Past 3000 she gives chase (state 4) rather than
    /// leaving, flies toward the player and is not marked for despawn, and recovers to the chooser
    /// the moment the player is back within 2000 (`NPC.cs:31053-31076`). Only a dead player ends it.
    #[test]
    fn out_running_her_makes_her_give_chase_not_despawn() {
        let tiles = Hollow;
        let mut q = queen();
        let far = Some(player_at(10_000.0 + QUEEN_GIVE_UP + 100.0, 10_000.0));
        update(&mut q, &hive_world(&tiles, far), &mut rng());
        assert_eq!(q.ai[0], QUEEN_CHASING, "she chases rather than leaving");
        update(&mut q, &hive_world(&tiles, far), &mut rng());
        assert!(q.time_left > 10, "and is not marked for despawn");
        assert!(
            q.velocity.0 > 0.0,
            "she closes the distance, flying toward the player"
        );

        // Back within 2000: she resumes the fight rather than leaving.
        let (cx, cy) = q.center();
        let near = Some(player_at(cx + 1500.0, cy));
        update(&mut q, &hive_world(&tiles, near), &mut rng());
        assert_eq!(q.ai[0], QUEEN_CHOOSING, "recovered to the chooser");
    }

    /// A dead player, on the other hand, does end it: she leaves for good and is marked to despawn.
    #[test]
    fn a_dead_player_sends_her_home_for_good() {
        let tiles = Hollow;
        let mut q = queen();
        let dead = Some(Target {
            slot: 0,
            center: (10_400.0, 10_000.0),
            velocity: (0.0, 0.0),
            alive: false,
        });
        update(&mut q, &hive_world(&tiles, dead), &mut rng());
        assert_eq!(q.ai[0], QUEEN_LEAVING);
        assert!(q.time_left <= 10, "and marked for despawn");
    }

    /// QB-2: the sting phase runs about 800 ticks in Normal mode, not the old flat 300, so she
    /// fires roughly twenty stingers a volley (`NPC.cs:31044-31051`, `ai[1] > 40 * 20`).
    #[test]
    fn the_sting_phase_lasts_its_full_length() {
        let tiles = Hollow;
        let mut q = queen();
        q.ai[0] = QUEEN_STINGING;
        let (cx, cy) = q.center();
        let below = Some(player_at(cx + 50.0, cy + 500.0));
        let mut r = rng();
        // At 300 ticks (the old cut-off) she is still stinging.
        for _ in 0..300 {
            update(&mut q, &hive_world(&tiles, below), &mut r);
        }
        assert_eq!(
            q.ai[0], QUEEN_STINGING,
            "still stinging past the old 300-tick cut-off"
        );
        // She keeps stinging until close to 800, then returns to the chooser.
        let mut ended_at = None;
        for tick in 300..900 {
            update(&mut q, &hive_world(&tiles, below), &mut r);
            if q.ai[0] == QUEEN_CHOOSING {
                ended_at = Some(tick);
                break;
            }
        }
        let ended_at = ended_at.expect("the sting phase should end");
        assert!(
            (750..=800).contains(&ended_at),
            "the phase should run its full ~800 ticks, ended at {ended_at}"
        );
    }

    /// QB-3: she keeps station on the player while summoning. A player who walks off during the
    /// call is chased at 14 toward them (`NPC.cs:30845-30890`), not left behind while she holds.
    #[test]
    fn she_repositions_toward_a_fleeing_player_during_the_summon() {
        let tiles = Hollow;
        let mut q = queen();
        q.ai[0] = QUEEN_SUMMONING;
        let (cx, cy) = q.center();
        // Well beyond the 400px hold radius, off to the right.
        let far = Some(player_at(cx + 1200.0, cy));
        let mut r = rng();
        for _ in 0..12 {
            update(&mut q, &hive_world(&tiles, far), &mut r);
        }
        assert!(
            q.velocity.0 > 0.5,
            "she flies toward the fleeing player, got {}",
            q.velocity.0
        );
    }

    /// QB-4: state 2's `num677 = num673 / num677` normalise (`NPC.cs:30749`) is dead, so her climb
    /// closes on the raw offset and accelerates past the 12 the old capped code plateaued at. The
    /// player is kept just inside her give-up radius so she stays in the climb.
    #[test]
    fn her_climb_has_no_hover_speed_cap() {
        let tiles = Hollow;
        let mut q = queen();
        q.ai[0] = QUEEN_CLIMBING;
        let (cx, cy) = q.center();
        let t = Some(player_at(cx, cy + 2900.0));
        let mut r = rng();
        for _ in 0..250 {
            update(&mut q, &hive_world(&tiles, t), &mut r);
            assert_eq!(q.ai[0], QUEEN_CLIMBING, "she should still be climbing");
        }
        assert!(
            q.velocity.1 > QUEEN_HOVER + 1.0,
            "her climb should build past the old 12 cap, got {}",
            q.velocity.1
        );
    }

    /// QB-4: state 3's clear-line branch has the same dead normalise (`num692 = num688 / num692`,
    /// `NPC.cs:31010`), so her sting approach closes on the raw offset and outruns the old 4 cap.
    #[test]
    fn her_sting_approach_has_no_speed_cap() {
        let tiles = Hollow;
        let mut q = queen();
        q.ai[0] = QUEEN_STINGING;
        let (cx, cy) = q.center();
        // Far below, in clear air: the clear-line branch, well past its 100px coast window.
        let t = Some(player_at(cx, cy + 2900.0));
        let mut r = rng();
        for _ in 0..150 {
            update(&mut q, &hive_world(&tiles, t), &mut r);
        }
        assert!(
            q.velocity.1 > 5.0,
            "her sting approach should outrun the old ~4 cap, got {}",
            q.velocity.1
        );
    }

    /// QB-5: the bee is gated on `Collision.CanHit` from the muzzle (`NPC.cs:30829`). Behind a wall
    /// she calls none, though the tally still climbs; in clear air she calls plenty.
    #[test]
    fn she_will_not_call_bees_through_a_wall() {
        let summon = |walled: bool| {
            let mut q = queen();
            q.ai[0] = QUEEN_SUMMONING;
            let (cx, cy) = q.center();
            let t = Some(player_at(cx, cy + 300.0));
            let mut r = rng();
            let mut bees = Vec::new();
            for _ in 0..200 {
                if walled {
                    let tiles = Walled { band: 634..637 };
                    bees.extend(update(&mut q, &walled_world(&tiles, t), &mut r).bees);
                } else {
                    let tiles = Hollow;
                    bees.extend(update(&mut q, &hive_world(&tiles, t), &mut r).bees);
                }
            }
            bees.len()
        };
        assert_eq!(
            summon(true),
            0,
            "no bee should reach the player through a wall"
        );
        assert!(summon(false) > 0, "in clear air she calls her bees");
    }

    /// QB-5: the stinger is gated on `Collision.CanHit` from the muzzle (`NPC.cs:30923`). Behind a
    /// wall she spits nothing; in clear air she spits freely.
    #[test]
    fn she_will_not_spit_stingers_through_a_wall() {
        let sting = |walled: bool| {
            let mut q = queen();
            q.ai[0] = QUEEN_STINGING;
            let (cx, cy) = q.center();
            let t = Some(player_at(cx, cy + 500.0));
            let mut r = rng();
            let mut spat = Vec::new();
            for _ in 0..200 {
                if walled {
                    let tiles = Walled { band: 634..637 };
                    spat.extend(update(&mut q, &walled_world(&tiles, t), &mut r).stingers);
                } else {
                    let tiles = Hollow;
                    spat.extend(update(&mut q, &hive_world(&tiles, t), &mut r).stingers);
                }
            }
            spat.len()
        };
        assert_eq!(
            sting(true),
            0,
            "no stinger should reach the player through a wall"
        );
        assert!(sting(false) > 0, "in clear air she spits her stingers");
    }
}
