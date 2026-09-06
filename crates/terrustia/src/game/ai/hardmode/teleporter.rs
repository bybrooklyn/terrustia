//! The nebula brain: style 97.
//!
//! A brain does not close on you and it does not run away. It drifts to four hundred pixels, holds
//! there, and every eight seconds it simply *is somewhere else* — a spot within twenty tiles of
//! you that is not within twelve tiles of anybody, so it can never land on top of a player. That
//! relocation is the fight: you cannot corner one, and the floaters it puts out in its first three
//! seconds keep coming at you from wherever it has just gone.
//!
//! Each teleport also hurries its floaters along by half a second, so relocating is not a retreat
//! — it speeds the attack up.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    BRAIN_APPROACH, BRAIN_APPROACH_SMOOTH, BRAIN_DRIFT_DRAG, BRAIN_FLOATER_EVERY,
    BRAIN_FLOATER_HURRY, BRAIN_FLOATER_SPEED, BRAIN_FLOATER_WINDOW, BRAIN_STANDOFF,
    BRAIN_TELEPORT_CLEARANCE, BRAIN_TELEPORT_EVERY, BRAIN_TELEPORT_RANGE,
};
use terrustia_proto::projectile::ids::NEBULA_FLOATER;

use super::drifters::Outcome;
use crate::game::ai::{Shot, World, face, sight};
use crate::game::npc::{Npc, TILE, TileView};

/// What a brain did this tick, beyond moving.
#[derive(Debug, Default)]
pub struct BrainOutcome {
    pub base: Outcome,
    /// Set on the tick it relocates, so its floaters can be hurried along.
    pub hurried_floaters: bool,
}

/// Style 97.
pub fn nebula_brain(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    rng: &mut SmallRng,
) -> BrainOutcome {
    let mut out = BrainOutcome::default();
    npc.dirty = true;

    // The floaters, three of them, one a second, in its first three seconds.
    if npc.local_ai[2] < BRAIN_FLOATER_WINDOW {
        npc.local_ai[2] += 1.0;
        if npc.local_ai[2] % BRAIN_FLOATER_EVERY == 0.0 {
            out.base.shots.push(Shot {
                projectile: NEBULA_FLOATER,
                damage: 0,
                position: npc.center(),
                velocity: floater_launch(rng),
                time_left: 1800,
                // `NewProjectile(..., 0f, whoAmI)` (`NPC.cs:39879`). An eye is the Brain's, and
                // `aiStyle 102` reads `ai[1]` for three things: what to hover beside, whose target
                // to shoot at, and when to stop existing. Without it the eyes drifted off and
                // outlived the Brain by half a minute.
                ai: [0.0, f32::from(world.slot), 0.0],
            });
        }
    }

    let Some(target) = world.target.filter(|t| t.alive) else {
        // Nobody left: it winds down rather than hanging about.
        npc.velocity.0 *= BRAIN_DRIFT_DRAG;
        npc.velocity.1 *= BRAIN_DRIFT_DRAG;
        npc.ai[0] = 0.0;
        return out;
    };
    face(npc, target);
    npc.rotation = npc.velocity.0.abs() * f32::from(npc.direction) * 0.1;
    npc.sprite_direction = -npc.direction;

    // It aims from a point off to the side, which is where its eye is.
    let (cx, cy) = npc.center();
    let eye = (cx + f32::from(npc.direction) * 20.0, cy + 6.0);
    let to_player = (target.center.0 - eye.0, target.center.1 - eye.1);
    let seen = sight::can_hit(world.tiles, npc.center(), (1, 1), target.center, (1, 1));

    if to_player.0.hypot(to_player.1) > BRAIN_STANDOFF || !seen {
        // Too far or blocked: drift closer, gently.
        let mut wanted = to_player;
        let length = wanted.0.hypot(wanted.1);
        if length > BRAIN_APPROACH {
            wanted = (
                wanted.0 / length * BRAIN_APPROACH,
                wanted.1 / length * BRAIN_APPROACH,
            );
        }
        npc.velocity.0 =
            (npc.velocity.0 * (BRAIN_APPROACH_SMOOTH - 1.0) + wanted.0) / BRAIN_APPROACH_SMOOTH;
        npc.velocity.1 =
            (npc.velocity.1 * (BRAIN_APPROACH_SMOOTH - 1.0) + wanted.1) / BRAIN_APPROACH_SMOOTH;
    } else {
        npc.velocity.0 *= BRAIN_DRIFT_DRAG;
        npc.velocity.1 *= BRAIN_DRIFT_DRAG;
    }

    // Being hit has a one-in-six chance of forcing an early relocation, rather than always
    // waiting out the full cycle — so hitting one is not simply free, safe damage.
    if world.was_hurt && rng.random_range(0..6) == 0 {
        npc.ai[0] = BRAIN_TELEPORT_EVERY;
    }
    npc.ai[0] += 1.0;
    if npc.ai[0] >= BRAIN_TELEPORT_EVERY {
        npc.ai[0] = 0.0;
        let here = ((cx / TILE) as i32, (cy / TILE) as i32);
        let player_tile = (
            (target.center.0 / TILE) as i32,
            (target.center.1 / TILE) as i32,
        );
        if let Some((tx, ty)) = find_spot(world, here, player_tile, target.center, rng) {
            npc.position = (
                tx as f32 * TILE - npc.width() / 2.0,
                ty as f32 * TILE - npc.height() / 2.0,
            );
            npc.velocity = (0.0, 0.0);
            out.hurried_floaters = true;
        }
    }
    out
}

/// A floater's launch velocity: downward, spread up to a quarter turn either way, and never so
/// nearly vertical that it just falls on the brain's own head.
fn floater_launch(rng: &mut SmallRng) -> (f32, f32) {
    for _ in 0..32 {
        let angle = rng.random_range(-1.5707964f32..1.5707964);
        let (sin, cos) = angle.sin_cos();
        // Straight down, rotated, then squashed into an ellipse.
        let v = (-sin * BRAIN_FLOATER_SPEED.0, cos * BRAIN_FLOATER_SPEED.1);
        if v.0.abs() >= 1.5 {
            return v;
        }
    }
    // Vanishingly unlikely; a sideways launch is the safe answer.
    (BRAIN_FLOATER_SPEED.0, 0.0)
}

/// Somewhere within `range` tiles of the player that is clear, and not within
/// [`BRAIN_TELEPORT_CLEARANCE`] tiles of anyone.
///
/// The clearance is what stops a brain relocating on top of you, and it is measured against where
/// a player is *going* as well as where they are, so running does not walk you into one.
fn find_spot(
    world: &World<'_, impl TileView>,
    here: (i32, i32),
    player: (i32, i32),
    player_center: (f32, f32),
    rng: &mut SmallRng,
) -> Option<(i32, i32)> {
    // Too far away to relocate at all.
    if (here.0 - player.0).abs() * 16 + (here.1 - player.1).abs() * 16 > 2000 {
        return None;
    }
    let range = BRAIN_TELEPORT_RANGE;
    for _ in 0..100 {
        let x = rng.random_range(player.0 - range..=player.0 + range);
        let mut y = rng.random_range(player.1 - range..=player.1 + range);
        while y < player.1 + range {
            // Never right where it already is.
            let same_place = (y - here.1).abs() <= 1 && (x - here.0).abs() <= 1;
            if same_place {
                y += 1;
                continue;
            }
            // It teleports into open air, so what it needs is room rather than ground.
            if solid_around(world, x, y, 1) {
                y += 1;
                continue;
            }
            // Far enough from every player, allowing for where they are heading.
            let ahead = (
                player_center.0 + world.target_velocity.0 * 20.0,
                player_center.1 + world.target_velocity.1 * 20.0,
            );
            let spot = (x as f32 * TILE, y as f32 * TILE);
            let clearance = BRAIN_TELEPORT_CLEARANCE as f32 * TILE;
            let too_near = (spot.0 - player_center.0).abs() < clearance
                && (spot.1 - player_center.1).abs() < clearance;
            let too_near_soon =
                (spot.0 - ahead.0).abs() < clearance && (spot.1 - ahead.1).abs() < clearance;
            if too_near || too_near_soon {
                break;
            }
            return Some((x, y));
        }
    }
    None
}

/// Whether any tile within `fluff` of `(x, y)` is solid.
fn solid_around(world: &World<'_, impl TileView>, x: i32, y: i32, fluff: i32) -> bool {
    for dy in -fluff..=fluff {
        for dx in -fluff..=fluff {
            let tile = world.tiles.tile(x + dx, y + dy);
            if tile.is_active() && terrustia_proto::tile_solid::solid(tile.block) {
                return true;
            }
        }
    }
    false
}

/// How much to hurry a floater along when its brain relocates.
pub const FLOATER_HURRY: f32 = BRAIN_FLOATER_HURRY;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    struct Sky(HashMap<(i32, i32), Tile>);

    impl TileView for Sky {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn world<'a>(tiles: &'a Sky, target: Option<(f32, f32)>) -> World<'a, Sky> {
        crate::game::ai::calm(
            tiles,
            target.map(|center| Target {
                slot: 0,
                center,
                velocity: (0.0, 0.0),
                alive: true,
            }),
        )
    }

    const NEBULA_BRAIN: u16 = 507;

    fn brain(x: f32, y: f32) -> Npc {
        Npc::new(NEBULA_BRAIN, (x, y), 1).expect("nebula brain")
    }

    /// Three floaters in the first three seconds, and none after.
    #[test]
    fn a_brain_puts_out_three_floaters_and_then_stops() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(97);
        let mut b = brain(0.0, 0.0);
        let w = world(&tiles, Some((300.0, 0.0)));

        let mut early = 0;
        for _ in 0..BRAIN_FLOATER_WINDOW as i32 {
            early += nebula_brain(&mut b, &w, &mut rng).base.shots.len();
        }
        assert_eq!(early, 3, "three floaters");

        let mut late = 0;
        for _ in 0..600 {
            late += nebula_brain(&mut b, &w, &mut rng).base.shots.len();
        }
        assert_eq!(late, 0, "and no more after that");
    }

    /// A floater is thrown out sideways enough to matter rather than dropped straight down.
    #[test]
    fn floaters_are_thrown_wide() {
        let mut rng = SmallRng::seed_from_u64(4);
        for _ in 0..200 {
            let v = floater_launch(&mut rng);
            assert!(v.0.abs() >= 1.5, "a floater should go sideways, got {v:?}");
        }
    }

    /// It relocates on a fixed cycle, and lands somewhere else.
    #[test]
    fn a_brain_relocates_every_eight_seconds() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(1);
        let mut b = brain(0.0, 0.0);
        // Close enough to be within relocating distance of.
        let w = world(&tiles, Some((600.0, 0.0)));

        let mut moved = None;
        for tick in 0..1200 {
            let before = b.position;
            let out = nebula_brain(&mut b, &w, &mut rng);
            if out.hurried_floaters {
                moved = Some((tick, before, b.position));
                break;
            }
            b.position.0 += b.velocity.0;
            b.position.1 += b.velocity.1;
        }
        let (tick, before, after) = moved.expect("it should have relocated");
        assert!(
            tick >= BRAIN_TELEPORT_EVERY as i32 - 1,
            "not before its cycle is up, went at {tick}"
        );
        let jump = (after.0 - before.0).hypot(after.1 - before.1);
        assert!(jump > 100.0, "and it should be a jump, not a step: {jump}");
    }

    /// Being hit gives a real one-in-six chance of forcing a relocation early, rather than always
    /// waiting out the full eight-second cycle — hitting one is not free.
    #[test]
    fn a_brain_can_relocate_early_when_hit() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(11);
        let mut b = brain(0.0, 0.0);
        let mut w = world(&tiles, Some((600.0, 0.0)));
        w.was_hurt = true;

        let mut relocated_early = false;
        for _ in 0..(BRAIN_TELEPORT_EVERY as i32 - 10) {
            if nebula_brain(&mut b, &w, &mut rng).hurried_floaters {
                relocated_early = true;
                break;
            }
        }
        assert!(
            relocated_early,
            "constant hits should eventually force a relocation well before the cycle is up"
        );
    }

    /// It never lands on top of a player.
    #[test]
    fn a_brain_keeps_its_distance_when_it_relocates() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(6);
        let player = (600.0, 0.0);
        let w = world(&tiles, Some(player));

        for seed in 0..40 {
            let mut b = brain(0.0, 0.0);
            b.ai[0] = BRAIN_TELEPORT_EVERY - 1.0;
            let mut rng = SmallRng::seed_from_u64(seed);
            let out = nebula_brain(&mut b, &w, &mut rng);
            if !out.hurried_floaters {
                continue;
            }
            let (cx, cy) = b.center();
            let clearance = BRAIN_TELEPORT_CLEARANCE as f32 * TILE;
            assert!(
                (cx - player.0).abs() >= clearance || (cy - player.1).abs() >= clearance,
                "it landed on the player: {cx},{cy}"
            );
        }
        let _ = &mut rng;
    }

    /// It will not relocate into rock.
    #[test]
    fn a_brain_does_not_relocate_into_the_ground() {
        // Solid everywhere except a pocket well away from the player.
        let mut tiles = HashMap::new();
        for x in -80..80 {
            for y in -80..80 {
                tiles.insert((x, y), Tile::block(1));
            }
        }
        for x in 50..60 {
            for y in -40..-30 {
                tiles.remove(&(x, y));
            }
        }
        let tiles = Sky(tiles);
        let w = world(&tiles, Some((40.0 * TILE, -35.0 * TILE)));

        for seed in 0..20 {
            let mut rng = SmallRng::seed_from_u64(seed);
            let mut b = brain(30.0 * TILE, -35.0 * TILE);
            b.ai[0] = BRAIN_TELEPORT_EVERY - 1.0;
            if !nebula_brain(&mut b, &w, &mut rng).hurried_floaters {
                continue;
            }
            let (cx, cy) = b.center();
            let (tx, ty) = ((cx / TILE) as i32, (cy / TILE) as i32);
            assert!(
                !solid_around(&w, tx, ty, 1),
                "it should not be inside rock at {tx},{ty}"
            );
        }
    }
    /// An eye is told which Brain made it.
    ///
    /// `NewProjectile(..., 0f, whoAmI)` (`NPC.cs:39879`). `aiStyle 102` reads `ai[1]` for three
    /// separate things - what to hover beside, whose target to shoot at, and when to stop existing
    /// - so without it the eyes drifted off and outlived the Brain by half a minute.
    #[test]
    fn a_floater_carries_its_brains_slot() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(97);
        let mut b = brain(0.0, 0.0);
        let mut w = world(&tiles, Some((300.0, 0.0)));
        w.slot = 5;
        let mut floaters = Vec::new();
        for _ in 0..(BRAIN_FLOATER_WINDOW as i32 + 2) {
            floaters.extend(nebula_brain(&mut b, &w, &mut rng).base.shots);
        }
        assert!(!floaters.is_empty(), "it should have put some out");
        assert!(
            floaters.iter().all(|s| s.ai[1] == 5.0),
            "every one carries the slot the caller filled in, not zero"
        );
    }
}
