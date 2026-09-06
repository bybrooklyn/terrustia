//! Style 22 — the hauntings.
//!
//! Ghosts and dripplers do not fly at you so much as *hang over* you. The routine feels ahead and
//! down for something to float above: finding nothing, it sinks; finding something, it pushes back
//! up off it. That is the whole of the bobbing, and it is why one will follow you along a cave roof
//! and then drop through a doorway.
//!
//! The other half is the anti-stall. A ghost that has spent half a second going nowhere — pinned in
//! a corner, or grinding against a ledge — decides it is stuck, turns around, and spends three full
//! seconds deliberately walking *away* from its target before trying again.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    HAUNT_BACK_OFF, HAUNT_RELOAD, HAUNT_STUCK_OVER, HAUNT_WINDUP, ICHOR_STICKER_FEEL,
    STICKER_HIT_PENALTY, STICKER_RELOAD, haunt, haunt_feels_by_distance, haunt_flees_daylight,
    haunt_gives_up_at_range, haunt_release_at, haunt_shot,
};
use terrustia_proto::tile_solid::{solid, solid_top};

use super::{Shot, World, can_see, face, sight::within_firing_range, steer_axis};
use crate::game::npc::{Npc, TILE, TileView};

/// How far from where it was it has to have got, in pixels, to count as having moved.
const STIRRED_X: f32 = 16.0;
const STIRRED_Y: f32 = 40.0;

/// Gastropod, Ice Elemental, Ichor Sticker, Reaper, Poltergeist.
const GASTROPOD: u16 = 122;
const ICE_ELEMENTAL: u16 = 169;
const ICHOR_STICKER: u16 = 268;
const REAPER: u16 = 253;
const POLTERGEIST: u16 = 330;

/// How long anything this style throws lives, matching the other ported routines.
const SHOT_LIFETIME: u16 = 300;

/// Whether the hour, the event or the target is telling it to leave (`flag31`,
/// `NPC.cs:24793-24806`).
fn leaving<T: TileView>(npc: &Npc, world: &World<'_, T>) -> bool {
    match npc.npc_type {
        // A Poltergeist belongs to the Pumpkin Moon and a Reaper to the Solar Eclipse; outside
        // their own event both go home rather than wandering the world.
        POLTERGEIST => return !world.conditions.pumpkin_moon,
        REAPER => return !world.conditions.eclipse,
        _ => {}
    }
    if haunt_flees_daylight(npc.npc_type) && world.conditions.day {
        return true;
    }
    if let Some(limit) = haunt_gives_up_at_range(npc.npc_type) {
        return match world.target {
            None => true,
            Some(t) => {
                let (cx, cy) = npc.center();
                !t.alive || ((t.center.0 - cx).powi(2) + (t.center.1 - cy).powi(2)).sqrt() > limit
            }
        };
    }
    false
}

/// The attack, `NPC.cs:24924-25097`. Three of the style's types shoot and the rest only haunt.
///
/// The Gastropod and the Ice Elemental share a cycle: a 120-tick reload, then a 64-tick wind-up
/// whose sixteenth or thirty-second tick is the shot. The Ichor Sticker instead counts up on its
/// own and lobs a glob with an upward lead the moment its timer runs out.
fn attack<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) -> Option<Shot> {
    let (projectile, damage, speed) = haunt_shot(npc.npc_type)?;
    let t = world.target?;
    let in_range = within_firing_range(npc.center(), t.center);
    let visible = can_see(world.tiles, npc, t);
    let mut shot = None;

    if npc.npc_type == ICHOR_STICKER {
        npc.ai[3] += 1.0;
        if world.was_hurt {
            npc.ai[3] = STICKER_HIT_PENALTY;
            npc.local_ai[1] = 0.0;
        }
        if npc.ai[3] >= (STICKER_RELOAD + rng.random_range(0..STICKER_RELOAD)) as f32 {
            npc.ai[3] = 0.0;
            if in_range && visible {
                // `NPC.cs:25086-25093`: it aims high by a tenth of the horizontal gap, and scatters
                // wide rather than tall.
                let from = (
                    npc.position.0 + npc.width() * 0.5 - 4.0,
                    npc.position.1 + npc.height() * 0.7,
                );
                let mut dx = t.center.0 - from.0;
                let lift = dx.abs() * 0.1;
                let mut dy = t.center.1 - from.1 - lift;
                dx += rng.random_range(-10..=10) as f32;
                dy += rng.random_range(-30..=20) as f32;
                let d = (dx * dx + dy * dy).sqrt().max(0.001);
                shot = Some(Shot {
                    projectile,
                    damage,
                    position: from,
                    velocity: (dx * speed / d, dy * speed / d),
                    time_left: SHOT_LIFETIME,
                    ai: [0.0; 3],
                });
            }
        }
        return shot;
    }

    if world.was_hurt {
        npc.ai[3] = 0.0;
        npc.local_ai[1] = 0.0;
    }
    if npc.ai[3] == haunt_release_at(npc.npc_type) {
        let from = (
            npc.position.0 + npc.width() * 0.5,
            npc.position.1 + npc.height() * 0.5,
        );
        let (dx, dy) = (t.center.0 - from.0, t.center.1 - from.1);
        let d = (dx * dx + dy * dy).sqrt().max(0.001);
        let (mut vx, mut vy) = (dx * speed / d, dy * speed / d);
        if npc.npc_type == GASTROPOD {
            // `RotatedByRandom(0.0125f * 2PI)`, a little under a degree either way.
            let spread = 0.0125 * std::f32::consts::TAU;
            let angle = rng.random::<f32>() * spread - rng.random::<f32>() * spread;
            let (s, c) = angle.sin_cos();
            (vx, vy) = (vx * c - vy * s, vx * s + vy * c);
        }
        shot = Some(Shot {
            projectile,
            damage,
            position: from,
            velocity: (vx, vy),
            time_left: SHOT_LIFETIME,
            ai: [0.0; 3],
        });
    }
    if npc.ai[3] > 0.0 {
        npc.ai[3] += 1.0;
        if npc.ai[3] >= HAUNT_WINDUP {
            npc.ai[3] = 0.0;
        }
    }
    if npc.ai[3] == 0.0 {
        npc.local_ai[1] += 1.0;
        // The Gastropod checks its range and sight at the end of the reload; the Ice Elemental
        // only its sight, and it keeps the reload running until it has it.
        let ready = if npc.npc_type == GASTROPOD {
            in_range && visible
        } else {
            visible
        };
        if npc.local_ai[1] > HAUNT_RELOAD {
            if npc.npc_type == GASTROPOD {
                npc.local_ai[1] = 0.0;
            }
            if ready {
                npc.local_ai[1] = 0.0;
                npc.ai[3] = 1.0;
                npc.dirty = true;
            }
        }
    }
    shot
}

/// Drive one haunting for a tick. Returns whatever it threw.
pub fn update<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    drift: f32,
    rng: &mut SmallRng,
) -> Option<Shot> {
    let params = haunt(npc.npc_type);

    if leaving(npc, world) {
        // On its way out: it keeps whatever course it has, and picks one if it has none.
        if npc.velocity.0 == 0.0 {
            npc.velocity.0 = drift;
            npc.dirty = true;
        }
        npc.time_left = npc.time_left.min(10);
    } else if npc.ai[2] >= 0.0 {
        // `ai[0..1]` remember where it was; if it is still about there, the counter climbs.
        let held_x = (npc.position.0 > npc.ai[0] - STIRRED_X
            && npc.position.0 < npc.ai[0] + STIRRED_X)
            || (npc.velocity.0 < 0.0 && npc.direction > 0)
            || (npc.velocity.0 > 0.0 && npc.direction < 0);
        let held_y =
            npc.position.1 > npc.ai[1] - STIRRED_Y && npc.position.1 < npc.ai[1] + STIRRED_Y;
        if held_x && held_y {
            npc.ai[2] += 1.0;
            // The game also tests `ai[2] >= 30f && num309 == 16` here (`NPC.cs:24855-24858`), but
            // `num309` was raised to 40 twelve lines earlier, so that arm is dead in vanilla and is
            // deliberately not transcribed. What actually sets the game's `flag30` is the ceiling
            // probe below.
            if npc.ai[2] >= HAUNT_STUCK_OVER {
                npc.ai[2] = -HAUNT_BACK_OFF;
                npc.direction = -npc.direction;
                npc.velocity.0 = -npc.velocity.0;
                npc.collide_x = false;
                npc.dirty = true;
            }
        } else {
            npc.ai[0] = npc.position.0;
            npc.ai[1] = npc.position.1;
            npc.ai[2] = 0.0;
            npc.dirty = true;
        }
        if let Some(t) = world.target {
            face(npc, t);
        }
    } else if npc.npc_type == REAPER {
        // `NPC.cs:24888-24892`: a Reaper backs off twice as fast and never turns its back on you.
        npc.ai[2] += 2.0;
        if let Some(t) = world.target {
            face(npc, t);
        }
    } else {
        // Backing off: it deliberately faces away from whoever it was chasing. A Poltergeist takes
        // ten times as long over it as anything else (`NPC.cs:24895-24902`).
        npc.ai[2] += if npc.npc_type == POLTERGEIST {
            0.1
        } else {
            1.0
        };
        if let Some(t) = world.target {
            npc.direction = if t.center.0 > npc.center().0 { -1 } else { 1 };
        }
    }

    // How far ahead and down to feel. A drippler reaches further the further off its target is,
    // and an Ichor Sticker twice as far when its target is above it (`NPC.cs:25066`).
    let mut feel = params.feel;
    if npc.npc_type == ICHOR_STICKER {
        let above = world.target.is_some_and(|t| t.center.1 < npc.center().1);
        feel = if above {
            ICHOR_STICKER_FEEL.1
        } else {
            ICHOR_STICKER_FEEL.0
        };
    }
    if haunt_feels_by_distance(npc.npc_type)
        && let Some(t) = world.target
    {
        let (cx, cy) = npc.center();
        let reach = ((t.center.0 - cx).powi(2) + (t.center.1 - cy).powi(2)).sqrt() / 70.0;
        feel += reach.min(8.0) as i32;
    }

    let probe_x = (npc.center().0 / TILE) as i32 + i32::from(npc.direction) * 2;
    let probe_y = ((npc.position.1 + npc.height()) / TILE) as i32;
    let mut nothing_below = true;
    let mut right_on_it = false;
    // It only bothers looking while it is below its target's head; above that it just floats.
    let below_target = world.target.is_some_and(|t| {
        npc.position.1 + npc.height() > t.center.1 - super::PLAYER_HEIGHT as f32 / 2.0
    });
    if below_target {
        for step in 0..feel {
            let tile = world.tiles.tile(probe_x, probe_y + step);
            if (tile.is_active() && solid(tile.block)) || tile.liquid > 0 {
                if step <= 1 {
                    right_on_it = true;
                }
                nothing_below = false;
                break;
            }
        }
    }
    // `NPC.cs:25156-25178`, the game's `flag30`. Only the Ice Elemental and the Ichor Sticker look
    // *up*: finding anything solid in the three tiles over their heads, they stop pushing off the
    // floor and sink out from under it, and the sticker gets a hard shove down as well.
    if matches!(npc.npc_type, ICE_ELEMENTAL | ICHOR_STICKER) {
        let ceiling = (probe_y - 3..probe_y).any(|y| {
            let tile = world.tiles.tile(probe_x, y);
            (tile.is_active() && solid(tile.block) && !solid_top(tile.block)) || tile.liquid > 0
        });
        if ceiling {
            right_on_it = false;
            nothing_below = true;
            if npc.npc_type == ICHOR_STICKER {
                npc.velocity.1 += 2.0;
            }
        }
    }

    if nothing_below {
        npc.velocity.1 += params.sink;
        if npc.velocity.1 > params.sink_cap {
            npc.velocity.1 = params.sink_cap;
        }
    } else {
        if (npc.direction_y < 0 && npc.velocity.1 > 0.0) || right_on_it {
            npc.velocity.1 -= params.lift;
        }
        if let Some(cap) = params.lift_cap
            && npc.velocity.1 < -cap
        {
            npc.velocity.1 = -cap;
        }
        if npc.velocity.1 < -4.0 {
            npc.velocity.1 = -4.0;
        }
    }

    // A soft rebound: it drifts off terrain rather than bouncing away from it.
    if npc.collide_x {
        npc.velocity.0 = npc.old_velocity.0 * -0.4;
        if npc.direction == -1 && npc.velocity.0 > 0.0 && npc.velocity.0 < 1.0 {
            npc.velocity.0 = 1.0;
        }
        if npc.direction == 1 && npc.velocity.0 < 0.0 && npc.velocity.0 > -1.0 {
            npc.velocity.0 = -1.0;
        }
    }
    if npc.collide_y {
        npc.velocity.1 = npc.old_velocity.1 * -0.25;
        if npc.velocity.1 > 0.0 && npc.velocity.1 < 1.0 {
            npc.velocity.1 = 1.0;
        }
        if npc.velocity.1 < 0.0 && npc.velocity.1 > -1.0 {
            npc.velocity.1 = -1.0;
        }
    }

    steer_axis(&mut npc.velocity.0, npc.direction, params.steering.x);
    steer_axis(&mut npc.velocity.1, npc.direction_y, params.steering.y);
    if npc.npc_type == ICHOR_STICKER {
        npc.rotation = npc.velocity.0 * 0.1;
    }
    npc.dirty = true;
    attack(npc, world, rng)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(13)
    }

    #[derive(Default)]
    struct Crypt(HashMap<(i32, i32), Tile>);

    impl TileView for Crypt {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn floor(top: i32) -> Crypt {
        let mut c = Crypt::default();
        for x in 0..2000 {
            for y in top..top + 10 {
                c.0.insert((x, y), Tile::block(1));
            }
        }
        c
    }

    fn ghost(npc_type: u16, tile_x: i32, tile_y: i32) -> Npc {
        Npc::new(npc_type, (tile_x as f32 * TILE, tile_y as f32 * TILE), 1)
            .expect("a style 22 type")
    }

    fn night<'a>(tiles: &'a Crypt, target: Option<Target>) -> World<'a, Crypt> {
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

    #[test]
    fn a_ghost_sinks_over_a_drop_and_lifts_over_ground() {
        let empty = Crypt::default();
        let mut falling = ghost(316, 100, 100);
        falling.direction = 1;
        falling.direction_y = -1;
        let (cx, cy) = falling.center();
        // Level with it: the ground probe only runs while the haunting is at or below its
        // target's head, which is what lets one drift over your head and then drop on you.
        let t = Some(player_at(cx + 100.0, cy));
        update(&mut falling, &night(&empty, t), 1.5, &mut rng());
        assert!(falling.velocity.1 > 0.0, "should sink into the drop");

        let solid = floor(102);
        let mut hovering = ghost(316, 100, 100);
        hovering.direction = 1;
        hovering.direction_y = -1;
        hovering.velocity.1 = 1.0;
        update(&mut hovering, &night(&solid, t), 1.5, &mut rng());
        assert!(
            hovering.velocity.1 < 1.0,
            "should push off it, got {}",
            hovering.velocity.1
        );
    }

    #[test]
    fn a_drippler_hangs_lower_and_slower_than_a_ghost() {
        assert!(haunt(490).steering.x.max < haunt(316).steering.x.max);
        assert!(haunt(490).sink < haunt(316).sink);
        assert!(haunt_feels_by_distance(490));
        assert!(!haunt_feels_by_distance(316));
    }

    #[test]
    fn a_drippler_leaves_at_dawn_and_a_ghost_does_not() {
        assert!(haunt_flees_daylight(490));
        assert!(!haunt_flees_daylight(316));

        let tiles = floor(200);
        let mut d = ghost(490, 100, 100);
        let (cx, cy) = d.center();
        let mut w = night(&tiles, Some(player_at(cx + 100.0, cy)));
        w.conditions.day = true;
        update(&mut d, &w, 1.5, &mut rng());
        assert!(d.time_left <= 10, "should be leaving, got {}", d.time_left);
    }

    #[test]
    fn a_ghost_gives_up_on_a_target_across_the_world() {
        let tiles = floor(200);
        let mut g = ghost(316, 100, 100);
        let (cx, cy) = g.center();
        update(
            &mut g,
            &night(&tiles, Some(player_at(cx + 5000.0, cy))),
            1.5,
            &mut rng(),
        );
        assert!(g.time_left <= 10, "should be leaving");
    }

    #[test]
    fn a_ghost_going_nowhere_backs_away_and_then_comes_back() {
        let tiles = floor(200);
        let mut g = ghost(316, 100, 100);
        g.direction = 1;
        g.ai[0] = g.position.0;
        g.ai[1] = g.position.1;
        let (cx, cy) = g.center();
        // A target off to the right, which it will not be making progress toward.
        let t = Some(player_at(cx + 400.0, cy));

        for _ in 0..(HAUNT_STUCK_OVER as i32 + 1) {
            update(&mut g, &night(&tiles, t), 1.5, &mut rng());
        }
        assert!(g.ai[2] < 0.0, "should have decided it is stuck");
        assert_eq!(g.direction, -1, "and turned away");

        // Facing away from the target while it backs off, rather than turning straight back.
        update(&mut g, &night(&tiles, t), 1.5, &mut rng());
        assert_eq!(g.direction, -1);

        for _ in 0..(HAUNT_BACK_OFF as i32 + 2) {
            update(&mut g, &night(&tiles, t), 1.5, &mut rng());
        }
        assert!(g.ai[2] >= 0.0, "should be hunting again");
    }

    #[test]
    fn moving_along_resets_the_stuck_counter() {
        let tiles = floor(200);
        let mut g = ghost(316, 100, 100);
        g.direction = 1;
        let (cx, cy) = g.center();
        let t = Some(player_at(cx + 400.0, cy));
        for _ in 0..40 {
            update(&mut g, &night(&tiles, t), 1.5, &mut rng());
            // Actually getting somewhere.
            g.position.0 += 40.0;
        }
        assert_eq!(g.ai[2], 0.0, "it is not stuck if it is moving");
    }

    /// The three shooters, none of which could emit anything at all before: `update` returned `()`
    /// and the dispatch pushed no shot. The Gastropod's laser is its entire threat.
    #[test]
    fn the_three_shooters_of_the_style_actually_shoot() {
        let tiles = Crypt::default();
        for (npc_type, projectile, damage, speed) in [
            (122u16, 84u16, 25, 7.0f32),
            (169, 128, 45, 5.0),
            (268, 288, 40, 10.0),
        ] {
            let mut g = ghost(npc_type, 500, 500);
            let (cx, cy) = g.center();
            let t = Some(player_at(cx + 200.0, cy + 40.0));
            let w = night(&tiles, t);
            let mut r = rng();
            let mut shot = None;
            for _ in 0..600 {
                if let Some(s) = update(&mut g, &w, 1.5, &mut r) {
                    shot = Some(s);
                    break;
                }
                // Hold it still so the probe and the timers are all that vary.
                g.position = (500.0 * TILE, 500.0 * TILE);
                g.velocity = (0.0, 0.0);
            }
            let s = shot.unwrap_or_else(|| panic!("type {npc_type} never fired"));
            assert_eq!(s.projectile, projectile, "type {npc_type}");
            assert_eq!(s.damage, damage, "type {npc_type}");
            let magnitude = (s.velocity.0.powi(2) + s.velocity.1.powi(2)).sqrt();
            assert!(
                (magnitude - speed).abs() < 1e-3,
                "type {npc_type} leaves at {speed}, got {magnitude}"
            );
            assert!(
                s.velocity.0 > 0.0,
                "type {npc_type} should aim at the player"
            );
        }
    }

    /// `NPC.cs:24954-24975`: a 64-tick wind-up whose 32nd tick is the shot, and then 120 ticks of
    /// reload before the next one starts, so one laser every three seconds and no faster.
    #[test]
    fn a_gastropod_fires_on_a_fixed_cadence() {
        let tiles = Crypt::default();
        let mut g = ghost(122, 500, 500);
        let (cx, cy) = g.center();
        let w = night(&tiles, Some(player_at(cx + 200.0, cy + 40.0)));
        let mut r = rng();
        let mut at = Vec::new();
        for tick in 0..600 {
            if update(&mut g, &w, 1.5, &mut r).is_some() {
                at.push(tick);
            }
            g.position = (500.0 * TILE, 500.0 * TILE);
            g.velocity = (0.0, 0.0);
        }
        assert_eq!(at.len(), 3, "three in ten seconds, got {at:?}");
        for pair in at.windows(2) {
            // The rest of the 64-tick wind-up, then the 121-tick reload, then 31 more ticks back
            // up to the release.
            assert_eq!(pair[1] - pair[0], 183, "one every three seconds");
        }
    }

    /// `NPC.cs:24795-24802`: both event ghosts leave when their event is not running, and the
    /// Reaper is the Solar Eclipse's most dangerous spawn.
    #[test]
    fn the_reaper_and_the_poltergeist_go_home_after_their_event() {
        let tiles = floor(200);
        for npc_type in [REAPER, POLTERGEIST] {
            let mut g = ghost(npc_type, 100, 100);
            let (cx, cy) = g.center();
            let mut w = night(&tiles, Some(player_at(cx + 100.0, cy)));
            update(&mut g, &w, 1.5, &mut rng());
            assert!(g.time_left <= 10, "type {npc_type} should be leaving");

            if npc_type == REAPER {
                w.conditions.eclipse = true;
            } else {
                w.conditions.pumpkin_moon = true;
            }
            let mut on_duty = ghost(npc_type, 100, 100);
            update(&mut on_duty, &w, 1.5, &mut rng());
            assert!(
                on_duty.time_left > 10,
                "type {npc_type} should stay while its event runs"
            );
        }
    }

    /// `NPC.cs:25156-25178`: the two types that look up stop pushing off the floor when they find a
    /// ceiling, and the Ichor Sticker gets a hard shove down with it.
    #[test]
    fn a_ceiling_drops_an_ichor_sticker_out_from_under_it() {
        let mut roofed = floor(200);
        let sample = ghost(268, 100, 100);
        let probe_x = (sample.center().0 / TILE) as i32 + 2;
        let probe_y = ((sample.position.1 + sample.height()) / TILE) as i32;
        for y in probe_y - 3..probe_y {
            roofed.0.insert((probe_x, y), Tile::block(1));
        }

        let mut s = ghost(268, 100, 100);
        s.direction = 1;
        let (cx, cy) = s.center();
        let t = Some(player_at(cx + 200.0, cy + 40.0));
        update(&mut s, &night(&roofed, t), 1.5, &mut rng());
        assert!(
            s.velocity.1 > 1.5,
            "should be shoved down, got {}",
            s.velocity.1
        );
    }

    /// `NPC.cs:24953`, `:25066`. A Gastropod feels eight tiles ahead rather than three, and an
    /// Ichor Sticker twice as far when it has to climb to you.
    #[test]
    fn the_probe_depths_are_per_type() {
        assert_eq!(haunt(122).feel, 8, "gastropod");
        assert_eq!(haunt(169).feel, 10, "ice elemental");
        assert_eq!(haunt(316).feel, 3, "an ordinary ghost");
        assert_eq!(ICHOR_STICKER_FEEL, (6, 12));
    }

    #[test]
    fn a_haunting_rebounds_softly_rather_than_bouncing() {
        let tiles = floor(200);
        let mut g = ghost(316, 100, 100);
        g.direction = 1;
        g.velocity = (2.0, 0.0);
        g.old_velocity = (2.0, 0.0);
        g.collide_x = true;
        let (cx, cy) = g.center();
        update(
            &mut g,
            &night(&tiles, Some(player_at(cx + 400.0, cy))),
            1.5,
            &mut rng(),
        );
        assert!(
            g.velocity.0 < 0.0 && g.velocity.0 > -2.0,
            "should ease back, got {}",
            g.velocity.0
        );
    }
}
