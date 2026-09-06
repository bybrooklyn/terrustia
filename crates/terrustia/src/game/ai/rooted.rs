//! Style 13 — the rooted plants, and style 17, the perchers.
//!
//! A **plant** (13) is tethered to the tile it grew from: it lunges toward whoever comes near,
//! never further than its reach, and dies the moment that tile is mined out from under it. Its
//! reach is not constant — over a 450-tick cycle the last third stretches it by 30%, which is the
//! slow breathing motion a man eater makes when nobody is in range.
//!
//! A **vulture** (17) sits on the sand with gravity on until something disturbs it, then kicks
//! itself into the air and circles, preferring to hang a hundred pixels above its target when it
//! is not already on top of it.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    PERCH_LAUNCH, PERCH_STARTLE, ROOTED_CYCLE, ROOTED_STRETCH, ROOTED_STRETCH_AT, VULTURE_CEILING,
    VULTURE_CLIMB_AT, rooted as rooted_params,
};

use super::{
    Shot, World, bounce, can_see, face, rise_out_of_water, sight::solid_collision,
    sight::within_firing_range,
};
use crate::game::npc::{Npc, TILE, TileView};
use crate::game::npc_ai::Spawn;

/// Clinger and Giant Fungi Bulb, the two style-13 types that do more than bite.
const CLINGER: u16 = 101;
const FUNGI_BULB: u16 = 260;
/// What a Giant Fungi Bulb launches (`NPCID.FungiSpore`).
const FUNGI_SPORE: u16 = 261;
/// How long a Clinger's ichor lives (`Main.projectile[num226].timeLeft = 300`).
const SHOT_LIFETIME: u16 = 300;
/// How soon each tries again after a blocked shot (`NPC.cs:22908`, `:22948`).
const CLINGER_RETRY_AT: f32 = 100.0;
const BULB_RETRY_AT: f32 = 130.0;

/// What a plant's tick concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Alive,
    /// The tile it grew from is gone, so it is too.
    Uprooted,
}

/// Everything a plant's tick produced.
#[derive(Debug, Default)]
pub struct Growth {
    pub uprooted: bool,
    /// A Clinger's ichor.
    pub shot: Option<Shot>,
    /// A Giant Fungi Bulb's spore, which is an NPC rather than a projectile.
    pub spawn: Option<Spawn>,
}

/// Drive one rooted plant for a tick.
pub fn plant<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) -> Growth {
    let mut out = Growth::default();
    if plant_move(npc, world, rng, &mut out) == Outcome::Uprooted {
        out.uprooted = true;
    }
    out
}

/// The movement half, which is also where the aim offset the attack reads is worked out.
fn plant_move<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    rng: &mut SmallRng,
    out: &mut Growth,
) -> Outcome {
    // `ai[0..1]` hold the anchor tile. The game's world generator writes it when it places the
    // plant, and so does the spawner, from the solid ground row it chose (`NPC.cs:3937` for the Man
    // Eater, `:3649`, `:3653`, `:3688` and `:3692` for the Fungi Bulb pair, all of the shape
    // `SpawnNPC(..., 43, 0, spawnTileX, spawnTileY)`). Anything that arrives without one takes root
    // in the first solid tile under it.
    //
    // Under it, not the tile its own centre falls in. A spawned plant stands in open air by
    // construction, so a centre-tile anchor is not active and the check below uproots it on its
    // very first tick: every Man Eater this server put in a jungle died before it was drawn, and
    // the Fungi Bulb pair would have done the same. Only the Giant Fungi Bulb happened to survive,
    // and only because it is 36 pixels tall, so its centre fell into the ground tile by accident.
    if npc.ai[0] == 0.0 && npc.ai[1] == 0.0 {
        let (cx, _) = npc.center();
        let column = (cx / TILE).floor() as i32;
        let top = (npc.position.1 / TILE).floor() as i32;
        // Its own rows plus one, which is where the ground is: the spawner puts a plant's top-left
        // on the open row above the tile it chose.
        let rows = (npc.height() / TILE).ceil() as i32 + 1;
        let root = (top..=top + rows)
            .find(|row| world.tiles.tile(column, *row).is_active())
            .unwrap_or(top + 1);
        npc.ai[0] = column as f32;
        npc.ai[1] = root as f32;
    }
    let (anchor_x, anchor_y) = (npc.ai[0] as i32, npc.ai[1] as i32);
    if !world.tiles.tile(anchor_x, anchor_y).is_active() {
        return Outcome::Uprooted;
    }

    if let Some(t) = world.target {
        face(npc, t);
    }
    let params = rooted_params(npc.npc_type);

    // The stretch cycle: for the last third of it the plant reaches half again as far.
    npc.ai[2] += 1.0;
    let mut reach = params.reach;
    if npc.ai[2] > ROOTED_STRETCH_AT {
        reach = (f64::from(reach) * f64::from(ROOTED_STRETCH)) as i32 as f32;
        if npc.ai[2] > ROOTED_CYCLE {
            npc.ai[2] = 0.0;
        }
    }

    let root = ((anchor_x * 16 + 8) as f32, (anchor_y * 16 + 8) as f32);
    let (mut dx, mut dy) = match world.target {
        Some(t) => (
            t.center.0 - npc.width() / 2.0 - root.0,
            t.center.1 - npc.height() / 2.0 - root.1,
        ),
        None => (0.0, 0.0),
    };
    let span = (dx * dx + dy * dy).sqrt();
    if span > reach {
        let k = reach / span;
        dx *= k;
        dy *= k;
    }

    // Pull toward the aim point, with an extra shove while still moving the wrong way.
    if npc.position.0 < root.0 + dx {
        npc.velocity.0 += params.pull;
        if npc.velocity.0 < 0.0 && dx > 0.0 {
            npc.velocity.0 += params.pull * 1.5;
        }
    } else if npc.position.0 > root.0 + dx {
        npc.velocity.0 -= params.pull;
        if npc.velocity.0 > 0.0 && dx < 0.0 {
            npc.velocity.0 -= params.pull * 1.5;
        }
    }
    if npc.position.1 < root.1 + dy {
        npc.velocity.1 += params.pull;
        if npc.velocity.1 < 0.0 && dy > 0.0 {
            npc.velocity.1 += params.pull * 1.5;
        }
    } else if npc.position.1 > root.1 + dy {
        npc.velocity.1 -= params.pull;
        if npc.velocity.1 > 0.0 && dy < 0.0 {
            npc.velocity.1 -= params.pull * 1.5;
        }
    }
    npc.velocity.0 = npc.velocity.0.clamp(-params.cap, params.cap);
    npc.velocity.1 = npc.velocity.1.clamp(-params.cap, params.cap);

    npc.sprite_direction = if dx > 0.0 { 1 } else { -1 };
    npc.rotation = dy.atan2(dx);

    // A plant that clips terrain rebounds hard rather than grinding along it.
    if npc.collide_x {
        npc.velocity.0 = npc.old_velocity.0 * -0.7;
        if npc.velocity.0 > 0.0 && npc.velocity.0 < 2.0 {
            npc.velocity.0 = 2.0;
        }
        if npc.velocity.0 < 0.0 && npc.velocity.0 > -2.0 {
            npc.velocity.0 = -2.0;
        }
        npc.dirty = true;
    }
    if npc.collide_y {
        npc.velocity.1 = npc.old_velocity.1 * -0.7;
        if npc.velocity.1 > 0.0 && npc.velocity.1 < 2.0 {
            npc.velocity.1 = 2.0;
        }
        if npc.velocity.1 < 0.0 && npc.velocity.1 > -2.0 {
            npc.velocity.1 = -2.0;
        }
        npc.dirty = true;
    }

    npc.dirty = true;
    attack(npc, world, rng, dy, out);
    Outcome::Alive
}

/// The two style-13 attacks, `NPC.cs:22883-22950`.
///
/// Both are the same shape: a counter that a hit resets, and on running out either the shot goes
/// or the counter is set back near the top so it tries again in twenty ticks. `aim_y` is the
/// vertical part of the plant's own reach, which is what decides whether a bulb aims high.
fn attack<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    rng: &mut SmallRng,
    aim_y: f32,
    out: &mut Growth,
) {
    let (reload, retry) = match npc.npc_type {
        CLINGER => (120.0, CLINGER_RETRY_AT),
        FUNGI_BULB => (150.0, BULB_RETRY_AT),
        _ => return,
    };
    let Some(t) = world.target.filter(|t| t.alive) else {
        return;
    };
    if world.was_hurt {
        npc.local_ai[0] = 0.0;
    }
    npc.local_ai[0] += 1.0;
    if npc.local_ai[0] < reload {
        return;
    }
    // It will not fire out of a wall, out of range, or through terrain.
    let clear = !solid_collision(
        world.tiles,
        npc.position,
        (npc.width() as i32, npc.height() as i32),
    ) && within_firing_range(npc.center(), t.center)
        && can_see(world.tiles, npc, t);
    if !clear {
        npc.local_ai[0] = retry;
        return;
    }

    let from = (
        npc.position.0 + npc.width() * 0.5,
        npc.position.1 + npc.height() * 0.5,
    );
    let mut dx = t.center.0 - from.0 + rng.random_range(-10..=10) as f32;
    let mut dy = t.center.1 - from.1 + rng.random_range(-10..=10) as f32;
    let speed = if npc.npc_type == CLINGER { 10.0 } else { 14.0 };
    if npc.npc_type == FUNGI_BULB {
        // `NPC.cs:22935-22939`: it lobs high by a tenth of the horizontal gap, but only when its
        // target is not already below it (`num222` there is this tick's vertical reach).
        if aim_y <= 0.0 {
            dy -= (dx * 0.1).abs();
        }
    }
    let d = (dx * dx + dy * dy).sqrt().max(0.001);
    dx *= speed / d;
    dy *= speed / d;
    npc.local_ai[0] = 0.0;
    npc.dirty = true;

    if npc.npc_type == CLINGER {
        out.shot = Some(Shot {
            projectile: 96,
            // `GetAttackDamage_ForProjectiles(22f, 17.6f)`.
            damage: if world.conditions.expert { 17 } else { 22 },
            position: from,
            velocity: (dx, dy),
            time_left: SHOT_LIFETIME,
            ai: [0.0; 3],
        });
    } else {
        out.spawn = Some(Spawn {
            handle: None,
            npc_type: FUNGI_SPORE,
            position: from,
            velocity: (dx, dy),
            parent: None,
            ai: [None; 4],
        });
    }
}

/// Drive one vulture for a tick.
pub fn vulture<T: TileView>(npc: &mut Npc, world: &World<'_, T>) {
    npc.no_gravity = true;
    if npc.ai[0] == 0.0 {
        // Perched, and therefore heavy.
        npc.no_gravity = false;
        if let Some(t) = world.target {
            face(npc, t);
        }
        let jostled = npc.velocity.0 != 0.0 || npc.velocity.1 < 0.0 || npc.velocity.1 > 0.3;
        if jostled {
            npc.ai[0] = 1.0;
            npc.dirty = true;
        } else {
            let disturbed = npc.life < npc.life_max
                || world.target.is_some_and(|t| {
                    let (cx, cy) = npc.center();
                    (t.center.0 - cx).abs() < PERCH_STARTLE + npc.width()
                        && (t.center.1 - cy).abs() < PERCH_STARTLE + npc.height()
                });
            if disturbed {
                npc.ai[0] = 1.0;
                npc.velocity.1 -= PERCH_LAUNCH;
                npc.dirty = true;
            }
        }
    } else if let Some(t) = world.target.filter(|t| t.alive) {
        bounce(npc);
        face(npc, t);

        // Level flight toward the target, with the usual brake against the turn.
        if npc.direction == -1 && npc.velocity.0 > -3.0 {
            npc.velocity.0 -= 0.1;
            if npc.velocity.0 > 3.0 {
                npc.velocity.0 -= 0.1;
            } else if npc.velocity.0 > 0.0 {
                npc.velocity.0 -= 0.05;
            }
            npc.velocity.0 = npc.velocity.0.max(-3.0);
        } else if npc.direction == 1 && npc.velocity.0 < 3.0 {
            npc.velocity.0 += 0.1;
            if npc.velocity.0 < -3.0 {
                npc.velocity.0 += 0.1;
            } else if npc.velocity.0 < 0.0 {
                npc.velocity.0 += 0.05;
            }
            npc.velocity.0 = npc.velocity.0.min(3.0);
        }

        // Hold a hundred pixels above, unless it is nearly overhead already.
        let across = (npc.position.0 + npc.width() / 2.0 - t.center.0).abs();
        let mut wanted = t.center.1 - super::PLAYER_HEIGHT as f32 / 2.0 - npc.height() / 2.0;
        if across > VULTURE_CLIMB_AT {
            wanted -= VULTURE_CEILING;
        }
        if npc.position.1 < wanted {
            npc.velocity.1 += 0.05;
            if npc.velocity.1 < 0.0 {
                npc.velocity.1 += 0.01;
            }
        } else {
            npc.velocity.1 -= 0.05;
            if npc.velocity.1 > 0.0 {
                npc.velocity.1 -= 0.01;
            }
        }
        npc.velocity.1 = npc.velocity.1.clamp(-3.0, 3.0);
    }

    if world.wet {
        rise_out_of_water(npc);
        if let Some(t) = world.target {
            face(npc, t);
        }
    }
    npc.sprite_direction = npc.direction;
    npc.dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(5)
    }

    #[derive(Default)]
    struct Jungle(HashMap<(i32, i32), Tile>);

    impl TileView for Jungle {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn world<'a>(tiles: &'a Jungle, target: Option<Target>) -> World<'a, Jungle> {
        crate::game::ai::calm(tiles, target)
    }

    fn player_at(x: f32, y: f32) -> Target {
        Target {
            slot: 0,
            center: (x, y),
            velocity: (0.0, 0.0),
            alive: true,
        }
    }

    fn rooted_at(npc_type: u16, tile_x: i32, tile_y: i32) -> (Npc, Jungle) {
        let mut n = Npc::new(npc_type, (tile_x as f32 * TILE, tile_y as f32 * TILE), 1)
            .expect("a style 13 type");
        n.ai[0] = tile_x as f32;
        n.ai[1] = tile_y as f32;
        let mut t = Jungle::default();
        t.0.insert((tile_x, tile_y), Tile::block(1));
        (n, t)
    }

    #[test]
    fn a_plant_dies_when_its_tile_is_mined_out() {
        let (mut p, tiles) = rooted_at(43, 500, 500);
        assert!(!plant(&mut p, &world(&tiles, None), &mut rng()).uprooted);
        let bare = Jungle::default();
        assert!(plant(&mut p, &world(&bare, None), &mut rng()).uprooted);
    }

    /// A plant the spawner placed arrives with no anchor at all, and it has to root itself in the
    /// ground under its feet rather than in the air its body occupies.
    ///
    /// Vanilla never has to decide this, because `NPC.Spawner` passes the ground row it chose
    /// (`NPC.cs:3937` for the Man Eater, `:3649` and `:3688` for the Fungi Bulb pair). This server's
    /// spawner has no ai arguments to pass, so the fallback is what stands in for it, and the
    /// fallback used to be the tile the plant's own centre falls in. `try_spawn` puts a spawn on an
    /// open row by construction, so that tile is never active: `plant` uprooted it on tick one and
    /// every Man Eater the jungle produced died before a client ever drew it.
    ///
    /// Neutralised by restoring the centre-tile anchor (`npc.ai[1] = (cy / TILE).floor()`): both
    /// the Man Eater and the Fungi Bulb assertions fail with "uprooted itself on its first tick".
    #[test]
    fn a_plant_spawned_with_no_anchor_roots_in_the_ground_under_it() {
        // The spawner's own shape: solid floor at row 501, the plant's top-left on row 500.
        let mut tiles = Jungle::default();
        for x in 498..=503 {
            tiles.0.insert((x, 501), Tile::block(1));
        }
        // 30x30, so its centre lands in the open row; and 20x20, likewise.
        for npc_type in [43u16, 259] {
            let mut p = Npc::new(npc_type, (500.0 * TILE, 500.0 * TILE), 1).expect("a plant");
            assert_eq!(p.ai[0], 0.0, "the spawner passes no anchor");
            assert!(
                !plant(&mut p, &world(&tiles, None), &mut rng()).uprooted,
                "{npc_type} uprooted itself on its first tick",
            );
            assert_eq!((p.ai[0], p.ai[1]), (500.0, 501.0), "{npc_type} anchor");
        }
        // And it is still the tile it rooted in that kills it, not merely any tile.
        let mut p = Npc::new(43, (500.0 * TILE, 500.0 * TILE), 1).expect("a plant");
        plant(&mut p, &world(&tiles, None), &mut rng());
        tiles.0.remove(&(500, 501));
        assert!(plant(&mut p, &world(&tiles, None), &mut rng()).uprooted);
    }

    #[test]
    fn a_plant_lunges_toward_a_player_within_reach() {
        let (mut p, tiles) = rooted_at(43, 500, 500);
        let (cx, cy) = p.center();
        let t = Some(player_at(cx + 150.0, cy));
        for _ in 0..200 {
            plant(&mut p, &world(&tiles, t), &mut rng());
        }
        assert!(p.velocity.0 > 0.0, "should reach out, got {}", p.velocity.0);
    }

    #[test]
    fn a_plant_will_not_reach_past_its_own_length() {
        let (mut p, tiles) = rooted_at(56, 500, 500);
        let root = (500.0 * 16.0 + 8.0, 500.0 * 16.0 + 8.0);
        let (cx, cy) = p.center();
        // Someone far out of reach; the plant should still stop at its own limit.
        let t = Some(player_at(cx + 4000.0, cy));
        let mut furthest: f32 = 0.0;
        for _ in 0..2000 {
            plant(&mut p, &world(&tiles, t), &mut rng());
            p.position.0 += p.velocity.0;
            p.position.1 += p.velocity.1;
            furthest = furthest.max(p.position.0 - root.0);
        }
        // It has no brake beyond the same pull it accelerates with, so it coasts past its limit
        // by exactly the distance it takes to shed its top speed.
        let params = rooted_params(56);
        let reach = params.reach * ROOTED_STRETCH;
        let coast = params.cap.powi(2) / (2.0 * params.pull);
        assert!(
            furthest < reach + coast + 10.0,
            "a snatcher reached {furthest}, past its {reach} plus {coast} of coasting"
        );
    }

    #[test]
    fn each_plant_has_its_own_reach() {
        assert_eq!(rooted_params(43).reach, 250.0, "man eater");
        assert_eq!(rooted_params(56).reach, 150.0, "snatcher");
        assert_eq!(rooted_params(259).reach, 100.0, "fungi bulb");
        assert_eq!(rooted_params(43).cap, 3.0, "and the man eater is quicker");
    }

    #[test]
    fn a_plant_stretches_for_the_last_third_of_its_cycle() {
        let (mut p, tiles) = rooted_at(56, 500, 500);
        let (cx, cy) = p.center();
        let t = Some(player_at(cx + 4000.0, cy));
        // Just before the stretch.
        p.ai[2] = ROOTED_STRETCH_AT - 1.0;
        plant(&mut p, &world(&tiles, t), &mut rng());
        let short = p.velocity.0;
        p.velocity = (0.0, 0.0);
        p.ai[2] = ROOTED_STRETCH_AT + 1.0;
        plant(&mut p, &world(&tiles, t), &mut rng());
        assert_eq!(
            short, p.velocity.0,
            "the pull is the same either way; only the limit moves"
        );
        assert!(p.ai[2] > ROOTED_STRETCH_AT, "and the cycle keeps running");
    }

    /// `NPC.cs:22883-22950`: two of the style's types attack at range, and neither could emit
    /// anything before because `plant` returned nothing but an `Outcome`.
    #[test]
    fn the_two_armed_plants_actually_attack() {
        // Hanging clear of the block it grew from, as one does in a real world: the game refuses
        // the shot while the plant's own body is inside terrain.
        let clear = (503.0 * TILE, 500.0 * TILE);
        let (mut clinger, tiles) = rooted_at(CLINGER, 500, 500);
        clinger.position = clear;
        let (cx, cy) = clinger.center();
        let t = Some(player_at(cx + 200.0, cy));
        let mut r = rng();
        let mut ichor = None;
        for _ in 0..400 {
            let out = plant(&mut clinger, &world(&tiles, t), &mut r);
            if out.shot.is_some() {
                ichor = out.shot;
                break;
            }
            clinger.position = clear;
        }
        let s = ichor.expect("a clinger should spit ichor");
        assert_eq!(s.projectile, 96);
        assert_eq!(s.damage, 22);
        let magnitude = (s.velocity.0.powi(2) + s.velocity.1.powi(2)).sqrt();
        assert!((magnitude - 10.0).abs() < 1e-3, "at 10, got {magnitude}");

        let (mut bulb, tiles) = rooted_at(FUNGI_BULB, 500, 500);
        bulb.position = clear;
        let (cx, cy) = bulb.center();
        let t = Some(player_at(cx + 200.0, cy));
        let mut spore = None;
        for _ in 0..500 {
            let out = plant(&mut bulb, &world(&tiles, t), &mut r);
            if out.spawn.is_some() {
                spore = out.spawn;
                break;
            }
            bulb.position = clear;
        }
        let s = spore.expect("a bulb should launch a spore");
        assert_eq!(s.npc_type, FUNGI_SPORE);
        let magnitude = (s.velocity.0.powi(2) + s.velocity.1.powi(2)).sqrt();
        assert!((magnitude - 14.0).abs() < 1e-3, "at 14, got {magnitude}");
        assert!(s.velocity.1 < 0.0, "and lobbed upward at a level target");
    }

    /// `NPC.cs:22908`, `:22948`: a blocked shot does not cost a full reload.
    #[test]
    fn a_walled_in_plant_retries_soon_rather_than_waiting_again() {
        let (mut c, mut tiles) = rooted_at(CLINGER, 500, 500);
        // Bury it, so `SolidCollision` refuses the shot.
        for x in 499..503 {
            for y in 499..503 {
                tiles.0.insert((x, y), Tile::block(1));
            }
        }
        let (cx, cy) = c.center();
        let t = Some(player_at(cx + 200.0, cy));
        let mut r = rng();
        for _ in 0..130 {
            assert!(plant(&mut c, &world(&tiles, t), &mut r).shot.is_none());
            c.position = (500.0 * TILE, 500.0 * TILE);
        }
        // 120 ticks to the first blocked attempt, reset to 100, then ten more.
        assert_eq!(c.local_ai[0], CLINGER_RETRY_AT + 10.0);
    }

    #[test]
    fn a_perched_vulture_stays_put_until_something_disturbs_it() {
        let tiles = Jungle::default();
        let mut v = Npc::new(61, (10_000.0, 10_000.0), 1).expect("vulture");
        let (cx, cy) = v.center();
        vulture(&mut v, &world(&tiles, Some(player_at(cx + 900.0, cy))));
        assert_eq!(v.ai[0], 0.0, "still perched");
        assert!(!v.no_gravity, "and heavy");

        vulture(&mut v, &world(&tiles, Some(player_at(cx + 50.0, cy))));
        assert_eq!(v.ai[0], 1.0, "should have taken off");
        assert!(v.velocity.1 < 0.0, "with a kick, got {}", v.velocity.1);
    }

    #[test]
    fn a_hurt_vulture_takes_off_too() {
        let tiles = Jungle::default();
        let mut v = Npc::new(61, (10_000.0, 10_000.0), 1).expect("vulture");
        v.life -= 1;
        let (cx, cy) = v.center();
        vulture(&mut v, &world(&tiles, Some(player_at(cx + 900.0, cy))));
        assert_eq!(v.ai[0], 1.0);
    }

    #[test]
    fn an_airborne_vulture_circles_above_its_target() {
        let tiles = Jungle::default();
        let mut v = Npc::new(61, (10_000.0, 10_000.0), 1).expect("vulture");
        v.ai[0] = 1.0;
        let (cx, cy) = v.center();
        // Well off to the side, so it should climb to its preferred ceiling.
        let t = Some(player_at(cx + 600.0, cy));
        // It hunts rather than settles: it climbs past its preferred ceiling, falls back through
        // it, and circles. So what to check is the height it reaches and where it spends its time,
        // not where it happens to be on the last tick.
        let mut highest = v.center().1;
        let mut above = 0;
        for _ in 0..300 {
            vulture(&mut v, &world(&tiles, t));
            v.position.0 += v.velocity.0;
            v.position.1 += v.velocity.1;
            highest = highest.min(v.center().1);
            if v.center().1 < cy {
                above += 1;
            }
        }
        assert!(v.no_gravity, "flying vultures are weightless");
        assert!(
            highest < cy - VULTURE_CEILING,
            "should have climbed above its target, got {highest} against {cy}"
        );
        assert!(above > 150, "and stayed up there most of the time: {above}");
        assert!(v.center().0 > cx, "and closed the gap");
    }
}
