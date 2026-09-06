//! Style 14 — the bats, and the demons, harpies and flying snakes that share their routine.
//!
//! Ported from the `aiStyle == 14` block. The shape is a weightless flier that steers toward its
//! target on both axes at once, bounces off anything it clips, and gives up after two hundred
//! ticks of not being able to see anyone — at which point it drifts along a slow sawtooth until
//! the player comes back into view.
//!
//! Every number that varies by type is in [`terrustia_proto::npc_params`]: the game writes the
//! same steering four times over with different constants, and once even lifts those constants
//! into named locals, which is as clear a statement as any that they are data.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    bat_drift, bat_extra_steering, bat_flees_daylight, bat_holds_course_when_blind,
    bat_rises_in_water, bat_shot, bat_steering,
};

use super::{Shot, World, bounce, can_see, face, rise_out_of_water, steer};
use crate::game::npc::{Npc, TileView};

/// How long an NPC goes without seeing anyone before it stops chasing, in ticks.
pub const PATIENCE: f32 = 200.0;

/// The drift's sawtooth runs between these, changing the vertical direction at zero.
pub const DRIFT_PERIOD: f32 = 300.0;
/// ...and the horizontal one at this offset either side of zero.
pub const DRIFT_TURN: f32 = 150.0;

/// After this long adrift the patience timer wraps, so an NPC that never regains sight of anyone
/// keeps cycling rather than sticking.
pub const PATIENCE_LIMIT: f32 = 1000.0;

/// Every projectile a flier throws lives for five seconds.
pub const SHOT_LIFETIME: u16 = 300;

/// Vampire Bat and the Vampire it becomes (`NPCID.VampireBat`, `NPCID.Vampire`).
const VAMPIRE_BAT: u16 = 158;
const VAMPIRE: u16 = 159;

/// How close a Vampire Bat has to get before it drops the disguise (`NPC.cs:23459`).
const UNMASK_RANGE: f32 = 200.0;

/// What a tick of this style did beyond moving its NPC.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub shot: Option<Shot>,
    /// A type this one turned into: only the Vampire Bat, which becomes a Vampire.
    pub became: Option<u16>,
}

/// Which way a velocity points, treating a standstill as positive the way the game does.
fn course(v: f32) -> i8 {
    if v < 0.0 { -1 } else { 1 }
}

/// Wander when there is nobody worth chasing.
///
/// `ai[2]` is a sawtooth from -300 to 300: its sign picks the vertical direction and its
/// magnitude the horizontal one, so the path is a long slow zig-zag rather than a circle.
fn drift(npc: &mut Npc) {
    let d = bat_drift(npc.npc_type);
    npc.ai[2] += 1.0;
    if npc.ai[2] > 0.0 {
        if npc.velocity.1 < d.max_y {
            npc.velocity.1 += d.accel_y;
        }
    } else if npc.velocity.1 > -d.max_y {
        npc.velocity.1 -= d.accel_y;
    }
    if npc.ai[2] < -DRIFT_TURN || npc.ai[2] > DRIFT_TURN {
        if npc.velocity.0 < d.max_x {
            npc.velocity.0 += d.accel_x;
        }
    } else if npc.velocity.0 > -d.max_x {
        npc.velocity.0 -= d.accel_x;
    }
    if npc.ai[2] > DRIFT_PERIOD {
        npc.ai[2] = -DRIFT_PERIOD;
    }
}

/// Run the fire timer, returning a shot when one leaves the muzzle this tick.
///
/// The reload is re-rolled every tick rather than fixed when the volley ends, so the gap between
/// volleys is a distribution, not a constant: somewhere between the base and twice it. That is the
/// game's own arithmetic, not a simplification of it.
fn fire<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    rng: &mut SmallRng,
    in_range: bool,
) -> Option<Shot> {
    let spec = bat_shot(npc.npc_type)?;
    let target = world.target?;
    npc.ai[0] += 1.0;

    let tick = npc.ai[0];
    if spec.cadence.iter().any(|&t| f32::from(t) == tick) {
        if !can_see(world.tiles, npc, target) {
            return None;
        }
        let muzzle = (
            npc.position.0 + npc.width() * 0.5,
            npc.position.1 + npc.height() * 0.5,
        );
        let scatter = spec.scatter;
        let mut aim = (
            target.center.0 - muzzle.0 + rng.random_range(-scatter..=scatter) as f32,
            target.center.1 - muzzle.1 + rng.random_range(-scatter..=scatter) as f32,
        );
        let length = (aim.0 * aim.0 + aim.1 * aim.1).sqrt();
        let scale = spec.speed / length;
        aim = (aim.0 * scale, aim.1 * scale);

        // A fast shooter throws from ahead of itself, and the red devil throws from further ahead
        // still, along the line of the shot.
        let from = (
            muzzle.0 + npc.velocity.0 * spec.lead + aim.0 * spec.standoff,
            muzzle.1 + npc.velocity.1 * spec.lead + aim.1 * spec.standoff,
        );
        npc.dirty = true;
        return Some(Shot {
            projectile: spec.projectile,
            damage: spec.damage,
            position: from,
            velocity: aim,
            time_left: SHOT_LIFETIME,
            ai: [0.0; 3],
        });
    }

    let reload =
        f32::from(spec.reload_base) + rng.random_range(0..i32::from(spec.reload_spread)) as f32;
    if in_range && tick >= reload {
        npc.ai[0] = 0.0;
    }
    None
}

/// Drive one flier for a tick, returning the projectile it threw if it threw one.
pub fn update<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) -> Outcome {
    npc.no_gravity = true;
    bounce(npc);

    // Pick and face a target. The flying snake is the exception: rather than turning to chase
    // through a wall it holds whatever course it is already on until it can see someone again.
    if let Some(t) = world.target {
        if bat_holds_course_when_blind(npc.npc_type) {
            let held = (course(npc.velocity.0), course(npc.velocity.1));
            face(npc, t);
            if !can_see(world.tiles, npc, t) {
                npc.direction = held.0;
                npc.direction_y = held.1;
            }
        } else {
            face(npc, t);
        }
    }

    // Daylight above the surface drives the vampire bat off.
    if bat_flees_daylight(npc.npc_type)
        && npc.position.1 < world.conditions.surface_y
        && world.conditions.day
        && !world.conditions.eclipse
    {
        npc.direction_y = -1;
        npc.direction = -npc.direction;
    }

    steer(npc, bat_steering(npc.npc_type));

    if bat_rises_in_water(npc.npc_type) && world.wet {
        rise_out_of_water(npc);
        if let Some(t) = world.target {
            face(npc, t);
        }
    }
    // The true bats steer a second time, which is why a cave bat closes on you twice as fast as a
    // demon does while sharing its top speed.
    if let Some(extra) = bat_extra_steering(npc.npc_type) {
        steer(npc, extra);
    }

    // `NPC.cs:23452-23462`. Close enough, above your feet and in plain sight, the bat stops being
    // a bat. Without this a Vampire Bat was only ever a bat, and the Vampire never appeared.
    let mut became = None;
    if npc.npc_type == VAMPIRE_BAT
        && let Some(t) = world.target
    {
        let (cx, cy) = npc.center();
        let gap = ((t.center.0 - cx).powi(2) + (t.center.1 - cy).powi(2)).sqrt();
        let feet = t.center.1 + super::PLAYER_HEIGHT as f32 / 2.0;
        if gap < UNMASK_RANGE
            && npc.position.1 + npc.height() < feet
            && can_see(world.tiles, npc, t)
        {
            became = Some(VAMPIRE);
        }
    }

    npc.ai[1] += 1.0;
    if bat_flees_daylight(npc.npc_type) {
        // The vampire bat runs out of patience twice as fast as everything else.
        npc.ai[1] += 1.0;
    }
    if npc.ai[1] > PATIENCE {
        let visible = world
            .target
            .is_some_and(|t| !world.target_wet && can_see(world.tiles, npc, t));
        if visible {
            npc.ai[1] = 0.0;
        }
        if npc.ai[1] > PATIENCE_LIMIT {
            npc.ai[1] = 0.0;
        }
        drift(npc);
    }

    npc.sprite_direction = npc.direction;
    npc.dirty = true;

    let in_range = world.target.is_some_and(|t| {
        super::sight::within_firing_range(
            (
                npc.position.0 + npc.width() * 0.5,
                npc.position.1 + npc.height() * 0.5,
            ),
            t.center,
        )
    });
    Outcome {
        shot: fire(npc, world, rng, in_range),
        became,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc::TILE;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::BAT_STEERING_DEFAULT;
    use terrustia_proto::tile::Tile;

    #[derive(Default)]
    struct Cave(HashMap<(i32, i32), Tile>);

    impl TileView for Cave {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn open() -> Cave {
        Cave::default()
    }

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(7)
    }

    fn bat(npc_type: u16) -> Npc {
        Npc::new(npc_type, (10_000.0, 10_000.0), 1).expect("a style 14 type")
    }

    fn player_at(x: f32, y: f32) -> Target {
        Target {
            slot: 0,
            center: (x, y),
            velocity: (0.0, 0.0),
            alive: true,
        }
    }

    use crate::game::npc_ai::Target;

    fn world<'a>(tiles: &'a Cave, target: Option<Target>) -> World<'a, Cave> {
        crate::game::ai::calm(tiles, target)
    }

    #[test]
    fn a_bat_is_weightless() {
        let tiles = open();
        let mut b = bat(49);
        b.no_gravity = false;
        update(&mut b, &world(&tiles, None), &mut rng());
        assert!(b.no_gravity, "style 14 turns gravity off every tick");
    }

    #[test]
    fn a_bat_turns_to_face_its_target_on_both_axes() {
        let tiles = open();
        let mut b = bat(49);
        let (cx, cy) = b.center();
        update(
            &mut b,
            &world(&tiles, Some(player_at(cx - 400.0, cy - 300.0))),
            &mut rng(),
        );
        assert_eq!((b.direction, b.direction_y), (-1, -1));
    }

    #[test]
    fn a_bat_reaches_exactly_its_top_speed() {
        let tiles = open();
        let mut b = bat(49);
        let (cx, cy) = b.center();
        let t = Some(player_at(cx + 5000.0, cy + 5000.0));
        for _ in 0..400 {
            update(&mut b, &world(&tiles, t), &mut rng());
        }
        assert_eq!(b.velocity.0, BAT_STEERING_DEFAULT.x.max);
        assert_eq!(b.velocity.1, BAT_STEERING_DEFAULT.y.max);
    }

    /// The bats — demons included — steer twice per tick, and a harpy only once.
    #[test]
    fn the_true_bats_accelerate_twice_as_hard_as_a_harpy() {
        let tiles = open();
        let (mut cave, mut harpy) = (bat(49), bat(48));
        let far = |n: &Npc| {
            let (cx, cy) = n.center();
            Some(player_at(cx + 5000.0, cy))
        };
        let (ct, ht) = (far(&cave), far(&harpy));
        update(&mut cave, &world(&tiles, ct), &mut rng());
        update(&mut harpy, &world(&tiles, ht), &mut rng());
        assert!(
            (cave.velocity.0 - harpy.velocity.0 * 2.0).abs() < 1e-6,
            "cave bat {} vs harpy {}",
            cave.velocity.0,
            harpy.velocity.0
        );
        assert!(bat_extra_steering(62).is_some(), "a demon is a bat too");
    }

    #[test]
    fn a_bat_bounces_off_a_wall_it_flies_into() {
        let tiles = open();
        let mut b = bat(49);
        b.direction = 1;
        b.velocity = (4.0, 0.0);
        b.old_velocity = (4.0, 0.0);
        b.collide_x = true;
        update(&mut b, &world(&tiles, None), &mut rng());
        assert!(b.velocity.0 < 0.0, "should rebound, got {}", b.velocity.0);
    }

    /// Half a pixel per tick against a sinking bat: it takes a moment, but it always wins.
    #[test]
    fn a_bat_in_water_swims_up() {
        let tiles = open();
        let mut b = bat(49);
        b.velocity = (0.0, 2.0);
        let mut w = world(&tiles, None);
        w.wet = true;
        for _ in 0..10 {
            update(&mut b, &w, &mut rng());
        }
        assert!(b.velocity.1 < 0.0, "should rise, got {}", b.velocity.1);
        for _ in 0..200 {
            update(&mut b, &w, &mut rng());
        }
        assert!(
            b.velocity.1 >= -4.0,
            "and cap out at four, got {}",
            b.velocity.1
        );
    }

    /// `NPC.cs:23452-23462`. Nothing in this crate had ever mentioned type 158 or 159, so a
    /// Vampire Bat stayed a bat and the Vampire never appeared at all.
    #[test]
    fn a_vampire_bat_close_enough_drops_the_disguise() {
        let tiles = open();
        let mut b = bat(VAMPIRE_BAT);
        let (cx, cy) = b.center();
        // A hundred pixels away and level, so the bat's feet are above the player's.
        let near = Some(player_at(cx + 100.0, cy + 60.0));
        assert_eq!(
            update(&mut b, &world(&tiles, near), &mut rng()).became,
            Some(VAMPIRE)
        );

        // Too far.
        let mut far_bat = bat(VAMPIRE_BAT);
        let far = Some(player_at(cx + 400.0, cy + 60.0));
        assert_eq!(
            update(&mut far_bat, &world(&tiles, far), &mut rng()).became,
            None
        );

        // Close, but the player is above it, so it keeps flying.
        let mut below = bat(VAMPIRE_BAT);
        let over = Some(player_at(cx + 40.0, cy - 60.0));
        assert_eq!(
            update(&mut below, &world(&tiles, over), &mut rng()).became,
            None
        );

        // And nothing else in the style ever transforms.
        let mut cave = bat(49);
        assert_eq!(
            update(&mut cave, &world(&tiles, near), &mut rng()).became,
            None
        );
    }

    #[test]
    fn a_harpy_flies_through_water_but_still_swims_up() {
        assert!(bat_rises_in_water(48));
        assert!(bat_extra_steering(48).is_none(), "a harpy is not a bat");
    }

    #[test]
    fn a_flying_snake_holds_its_course_when_it_cannot_see_you() {
        let mut tiles = open();
        for y in 600..700 {
            tiles.0.insert((630, y), Tile::block(1));
        }
        let mut snake = bat(226);
        snake.position = (620.0 * TILE, 620.0 * TILE);
        snake.velocity = (-3.0, -1.0);
        let (cx, cy) = snake.center();
        // The player is to the right, behind the wall.
        update(
            &mut snake,
            &world(&tiles, Some(player_at(cx + 400.0, cy))),
            &mut rng(),
        );
        assert_eq!(
            snake.direction, -1,
            "should keep flying the way it already was rather than turn into the wall"
        );
    }

    #[test]
    fn a_bat_that_loses_you_starts_drifting() {
        let tiles = open();
        let mut b = bat(49);
        b.ai[1] = PATIENCE;
        update(&mut b, &world(&tiles, None), &mut rng());
        assert_eq!(b.ai[2], 1.0, "the drift sawtooth should have started");
    }

    #[test]
    fn seeing_you_again_resets_the_patience_timer() {
        let tiles = open();
        let mut b = bat(49);
        b.ai[1] = PATIENCE + 50.0;
        let (cx, cy) = b.center();
        update(
            &mut b,
            &world(&tiles, Some(player_at(cx + 100.0, cy))),
            &mut rng(),
        );
        assert_eq!(b.ai[1], 0.0);
    }

    #[test]
    fn a_target_standing_in_water_does_not_reset_the_timer() {
        let tiles = open();
        let mut b = bat(49);
        b.ai[1] = PATIENCE + 50.0;
        let (cx, cy) = b.center();
        let mut w = world(&tiles, Some(player_at(cx + 100.0, cy)));
        w.target_wet = true;
        update(&mut b, &w, &mut rng());
        assert!(b.ai[1] > PATIENCE, "got {}", b.ai[1]);
    }

    #[test]
    fn the_drift_sawtooth_wraps_rather_than_running_away() {
        let tiles = open();
        let mut b = bat(49);
        b.ai[1] = PATIENCE + 1.0;
        for _ in 0..2000 {
            b.ai[1] = PATIENCE + 1.0;
            update(&mut b, &world(&tiles, None), &mut rng());
            assert!(b.ai[2] >= -DRIFT_PERIOD && b.ai[2] <= DRIFT_PERIOD + 1.0);
        }
    }

    #[test]
    fn a_harpy_looses_a_feather_on_its_cadence() {
        let tiles = open();
        let mut h = bat(48);
        let (cx, cy) = h.center();
        let t = Some(player_at(cx + 200.0, cy));
        let mut shots = Vec::new();
        for _ in 0..100 {
            h.ai[1] = 0.0;
            if let Some(shot) = update(&mut h, &world(&tiles, t), &mut rng()).shot {
                shots.push((h.ai[0], shot));
            }
        }
        let ticks: Vec<f32> = shots.iter().map(|(t, _)| *t).collect();
        assert_eq!(ticks, vec![30.0, 60.0, 90.0]);
        assert_eq!(shots[0].1.projectile, 38);
        assert_eq!(shots[0].1.damage, 15);
        let v = shots[0].1.velocity;
        assert!(
            ((v.0 * v.0 + v.1 * v.1).sqrt() - 6.0).abs() < 1e-3,
            "feathers leave at 6 px/tick, got {v:?}"
        );
    }

    #[test]
    fn a_demon_throws_a_scythe_and_a_bat_throws_nothing() {
        assert_eq!(bat_shot(62).map(|s| s.projectile), Some(44));
        assert_eq!(bat_shot(66).map(|s| s.projectile), Some(44));
        assert!(bat_shot(49).is_none());
    }

    #[test]
    fn a_shot_needs_a_clear_line() {
        let mut tiles = open();
        for y in 0..2000 {
            tiles.0.insert((630, y), Tile::block(1));
        }
        let mut h = bat(48);
        h.position = (620.0 * TILE, 620.0 * TILE);
        let (cx, cy) = h.center();
        h.ai[0] = 29.0;
        let shot = update(
            &mut h,
            &world(&tiles, Some(player_at(cx + 300.0, cy))),
            &mut rng(),
        );
        assert!(shot.shot.is_none(), "a wall in the way stops the shot");
        assert_eq!(h.ai[0], 30.0, "but the timer still runs");
    }

    #[test]
    fn a_far_away_target_never_triggers_a_reload() {
        let tiles = open();
        let mut h = bat(48);
        h.ai[0] = 5000.0;
        let (cx, cy) = h.center();
        update(
            &mut h,
            &world(&tiles, Some(player_at(cx + 40_000.0, cy))),
            &mut rng(),
        );
        assert_eq!(h.ai[0], 5001.0, "out of firing range, so no reload");
    }

    #[test]
    fn the_reload_lands_somewhere_between_the_base_and_twice_it() {
        let tiles = open();
        let mut rng = rng();
        let mut reloads = Vec::new();
        for _ in 0..40 {
            let mut h = bat(48);
            let (cx, cy) = h.center();
            let t = Some(player_at(cx + 200.0, cy));
            h.ai[0] = 100.0;
            for tick in 0..1200 {
                h.ai[1] = 0.0;
                update(&mut h, &world(&tiles, t), &mut rng);
                if h.ai[0] == 0.0 {
                    reloads.push(100 + tick);
                    break;
                }
            }
        }
        assert_eq!(reloads.len(), 40, "every run should reload");
        let lo = *reloads.iter().min().unwrap();
        let hi = *reloads.iter().max().unwrap();
        assert!(lo > 400, "no reload before the base, got {lo}");
        assert!(hi < 800, "and none past twice it, got {hi}");
        // Re-rolling every tick means the chance of reloading climbs from 1-in-400 upward, so the
        // draws bunch up just past the base rather than spreading evenly to twice it.
        assert!(
            hi - lo > 10,
            "should be a spread, not a constant: {lo}..{hi}"
        );
        assert!(lo < 450, "and should bunch near the base, got {lo}");
    }
}
