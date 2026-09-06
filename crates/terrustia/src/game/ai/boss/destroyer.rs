//! The Destroyer: style 37.
//!
//! Its movement is the ordinary worm burrow — the same routine an eater of worlds runs, at sixteen
//! pixels a tick and turning no more sharply for it, which is why it carves such enormous circles
//! and why standing still is the wrong answer. That part is [`super::super::worm`]; what is here
//! is what makes it the Destroyer rather than a fast worm.
//!
//! Every one of its eighty body segments carries its own probe, on its own fuse. The fuse advances
//! by nought to three a tick and fires somewhere between fourteen hundred and twenty-six thousand,
//! so any one segment fires rarely and the worm as a whole fires constantly — and the more of it
//! you can see, the more lasers you are under. That is the fight: the Destroyer's damage is a
//! function of how much of its length has a line to you.
//!
//! Daybreak ends it. The head dives, and once it is past the rock layer the whole worm is gone.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    DESTROYER_AIM_SPREAD, DESTROYER_BODY, DESTROYER_FLEE_SPEED, DESTROYER_FUSE,
    DESTROYER_FUSE_STEP, DESTROYER_HEAD, DESTROYER_LASER_DAMAGE, DESTROYER_LASER_DAMAGE_EXPERT,
    DESTROYER_LASER_LEAD, DESTROYER_LASER_LIFE, DESTROYER_LASER_SPEED, DESTROYER_SPEED_SPREAD,
};
use terrustia_proto::projectile::ids::DESTROYER_LASER;

use crate::game::ai::{Shot, World, can_see};
use crate::game::npc::{Npc, TileView};

/// What a piece of the Destroyer did this tick.
#[derive(Debug, Default)]
pub struct DestroyerOutcome {
    pub shots: Vec<Shot>,
    /// Set when daylight has driven it off and the whole worm should go.
    pub fleeing: bool,
}

/// Style 37.
///
/// The burrowing is delegated to the shared worm routine; this adds the probes and the retreat.
pub fn destroyer(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    rng: &mut SmallRng,
) -> DestroyerOutcome {
    let mut out = DestroyerOutcome::default();

    // Daylight, or nobody left alive, drives it underground for good.
    let fleeing = world.conditions.day || world.target.is_none_or(|t| !t.alive);
    if fleeing {
        if npc.npc_type == DESTROYER_HEAD {
            // It dives, and faster once it is below the surface, so the retreat accelerates.
            npc.velocity.1 += 1.0;
            if npc.position.1 > world.conditions.surface_y {
                npc.velocity.1 += 1.0;
                npc.velocity.1 = npc.velocity.1.min(DESTROYER_FLEE_SPEED);
                // Past the surface and burrowing away for good, the worm leaves. Vanilla removes
                // every segment at once once the head is below the rock layer
                // (`NPC.cs:50498-50507`); here the head reports the despawn and, once it is reaped,
                // each segment behind it is orphaned, becomes a head of its own, flees the same way
                // and follows it out one tick later. MECH-1: this flag was produced but consumed
                // nowhere, so the Destroyer simply dived and then hung underground for ever.
                out.fleeing = true;
            }
            npc.dirty = true;
        } else {
            // A body or tail segment reaches its own routine only after its leader has already
            // left and it has become the head of what remains: at dawn it leaves the same way.
            out.fleeing = true;
        }
        return out;
    }

    // The burrow, shared with every other worm in the game.
    super::super::worm::update(npc, world, world.conditions.expert);

    // Only the body segments carry probes.
    if npc.npc_type != DESTROYER_BODY {
        return out;
    }
    npc.local_ai[0] += rng.random_range(0..DESTROYER_FUSE_STEP) as f32;
    if npc.local_ai[0] < rng.random_range(DESTROYER_FUSE.0..DESTROYER_FUSE.1) as f32 {
        return out;
    }
    npc.local_ai[0] = 0.0;

    // A segment with no line to you does not fire, which is what makes cover work against it.
    let Some(target) = world.target.filter(|t| t.alive) else {
        return out;
    };
    if !can_see(world.tiles, npc, target) {
        return out;
    }

    let (cx, cy) = npc.center();
    // Scattered in pixels before the aim is normalised...
    let mut aim = (
        target.center.0 - cx
            + rng.random_range(-DESTROYER_AIM_SPREAD..=DESTROYER_AIM_SPREAD) as f32,
        target.center.1 - cy
            + rng.random_range(-DESTROYER_AIM_SPREAD..=DESTROYER_AIM_SPREAD) as f32,
    );
    let length = aim.0.hypot(aim.1).max(f32::MIN_POSITIVE);
    aim = (
        aim.0 / length * DESTROYER_LASER_SPEED,
        aim.1 / length * DESTROYER_LASER_SPEED,
    );
    // ...and again in speed afterwards, which is why two lasers from the same segment diverge.
    aim.0 += rng.random_range(-DESTROYER_AIM_SPREAD..=DESTROYER_AIM_SPREAD) as f32
        * DESTROYER_SPEED_SPREAD;
    aim.1 += rng.random_range(-DESTROYER_AIM_SPREAD..=DESTROYER_AIM_SPREAD) as f32
        * DESTROYER_SPEED_SPREAD;

    out.shots.push(Shot {
        projectile: DESTROYER_LASER,
        // The launch-time figure, which vanilla lerps between a classic and a separate expert one
        // (`GetAttackDamage_ForProjectiles(22f, 18f)`, `NPC.cs:50399`); the impact-time
        // `hostileDamageScaling` doubling is applied where this server already applies it, in
        // `tick_contact_damage`. `Remap` clamps outside classic..expert, so master reads the expert
        // figure too, which is why this is a two-way branch rather than a curve.
        damage: if world.conditions.expert {
            DESTROYER_LASER_DAMAGE_EXPERT
        } else {
            DESTROYER_LASER_DAMAGE
        },
        position: (
            cx + aim.0 * DESTROYER_LASER_LEAD,
            cy + aim.1 * DESTROYER_LASER_LEAD,
        ),
        velocity: aim,
        time_left: DESTROYER_LASER_LIFE,
        ai: [0.0; 3],
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::ai::Conditions;
    use crate::game::npc::TILE;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    struct Rock(HashMap<(i32, i32), Tile>);

    impl TileView for Rock {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn open() -> Rock {
        Rock(HashMap::new())
    }

    /// Solid everywhere, so a laser has nothing to shoot through.
    fn buried() -> Rock {
        let mut tiles = HashMap::new();
        for x in -200..200 {
            for y in -200..200 {
                tiles.insert((x, y), Tile::block(1));
            }
        }
        Rock(tiles)
    }

    fn night(tiles: &Rock, target: Option<(f32, f32)>) -> World<'_, Rock> {
        let mut w = crate::game::ai::calm(
            tiles,
            target.map(|center| Target {
                slot: 0,
                center,
                velocity: (0.0, 0.0),
                alive: true,
            }),
        );
        w.conditions = Conditions {
            day: false,
            surface_y: 100.0 * TILE,
            ..Conditions::default()
        };
        w
    }

    fn segment(npc_type: u16, x: f32, y: f32) -> Npc {
        Npc::new(npc_type, (x, y), 1).expect("a piece of the Destroyer")
    }

    /// Only the body segments carry probes; the head and tail do not.
    #[test]
    fn the_lasers_come_from_the_body() {
        let tiles = open();
        let w = night(&tiles, Some((400.0, 0.0)));
        let fired = |ty: u16| {
            let mut rng = SmallRng::seed_from_u64(37);
            let mut s = segment(ty, 0.0, 0.0);
            (0..40_000)
                .map(|_| destroyer(&mut s, &w, &mut rng).shots.len())
                .sum::<usize>()
        };
        assert!(fired(DESTROYER_BODY) > 0, "a body segment should fire");
        assert_eq!(fired(DESTROYER_HEAD), 0, "the head does not");
        assert_eq!(
            fired(terrustia_proto::npc_params::DESTROYER_TAIL),
            0,
            "nor does the tail"
        );
    }

    /// Its laser carries a separate expert figure, and it is the *lower* one.
    ///
    /// `GetAttackDamage_ForProjectiles(22f, 18f)` (`NPC.cs:50399`) lerps between a classic 22 and an
    /// expert 18 at launch, and the impact-time `hostileDamageScaling` doubles whichever landed.
    /// Carrying the classic 22 into every mode made an expert Destroyer's lasers 22% heavier than
    /// the game's. `Remap` clamps above expert, so master reads 18 too.
    #[test]
    fn the_laser_carries_its_own_expert_figure() {
        let tiles = open();
        let damage_in = |expert: bool| {
            let mut rng = SmallRng::seed_from_u64(37);
            let mut s = segment(DESTROYER_BODY, 0.0, 0.0);
            let mut w = night(&tiles, Some((400.0, 0.0)));
            w.conditions.expert = expert;
            (0..40_000)
                .flat_map(|_| destroyer(&mut s, &w, &mut rng).shots)
                .map(|shot| shot.damage)
                .next()
                .expect("a body segment should fire")
        };
        assert_eq!(damage_in(false), DESTROYER_LASER_DAMAGE);
        assert_eq!(
            damage_in(true),
            DESTROYER_LASER_DAMAGE_EXPERT,
            "and the expert figure is the lower of the two, which is the point"
        );
        assert!(damage_in(true) < damage_in(false));
    }

    /// A segment you cannot see cannot see you, so cover works.
    #[test]
    fn a_buried_segment_holds_its_fire() {
        let tiles = buried();
        let w = night(&tiles, Some((400.0, 0.0)));
        let mut rng = SmallRng::seed_from_u64(2);
        let mut s = segment(DESTROYER_BODY, 0.0, 0.0);
        let fired: usize = (0..40_000)
            .map(|_| destroyer(&mut s, &w, &mut rng).shots.len())
            .sum();
        assert_eq!(fired, 0, "it should not shoot through solid rock");
    }

    /// Two lasers from one segment do not travel the same line.
    #[test]
    fn its_lasers_scatter() {
        let tiles = open();
        let w = night(&tiles, Some((400.0, 0.0)));
        let mut rng = SmallRng::seed_from_u64(5);
        let mut s = segment(DESTROYER_BODY, 0.0, 0.0);
        let mut shots = Vec::new();
        for _ in 0..200_000 {
            shots.extend(destroyer(&mut s, &w, &mut rng).shots);
            if shots.len() >= 8 {
                break;
            }
        }
        assert!(shots.len() >= 4, "expected several lasers");
        let first = shots[0].velocity;
        assert!(
            shots.iter().any(|s| (s.velocity.0 - first.0).abs() > 0.01),
            "they should not all be identical"
        );
        // ...but still roughly toward the player.
        assert!(
            shots.iter().all(|s| s.velocity.0 > 0.0),
            "and all of them go the right way"
        );
    }

    /// Daybreak sends it down, and faster once it is under the surface; and only once the head is
    /// under does the worm signal that it is leaving (MECH-1: it dives to the rock before it goes,
    /// `NPC.cs:50489-50507`).
    #[test]
    fn daylight_drives_it_underground() {
        let tiles = open();
        let mut rng = SmallRng::seed_from_u64(9);
        let mut w = night(&tiles, Some((400.0, 0.0)));
        w.conditions.day = true;

        // Above the surface (surface_y is 100 tiles down here): it dives, but is not yet gone.
        let mut head = segment(DESTROYER_HEAD, 0.0, 0.0);
        let out = destroyer(&mut head, &w, &mut rng);
        assert!(
            !out.fleeing,
            "still above the surface: diving, not yet leaving"
        );
        let above = head.velocity.1;

        // Below the surface: it dives harder, and now the worm leaves.
        let mut deep = segment(DESTROYER_HEAD, 0.0, 200.0 * TILE);
        let deep_out = destroyer(&mut deep, &w, &mut rng);
        assert!(
            deep.velocity.1 > above,
            "below the surface it should dive harder: {} vs {above}",
            deep.velocity.1
        );
        assert!(
            deep_out.fleeing,
            "and once under, the worm signals it is gone"
        );
    }

    /// MECH-1: the flee has to reach the server as a despawn. The dispatch routes it to `expired`,
    /// which zeroes the boss's `time_left` (bosses never count down through `tick_life`). On the
    /// pre-fix code the `fleeing` flag was dropped in the dispatch and the worm never left.
    #[test]
    fn daybreak_actually_despawns_the_worm_through_the_dispatch() {
        let tiles = open();
        let mut rng = SmallRng::seed_from_u64(9);
        let mut w = night(&tiles, Some((400.0, 0.0)));
        w.conditions.day = true;
        // A head already below the surface, so it is past the dive and ready to go.
        let mut head = segment(DESTROYER_HEAD, 0.0, 200.0 * TILE);
        let effects = crate::game::ai::run(&mut head, &w, &mut rng);
        assert!(
            effects.expired,
            "daybreak must reach the server as a despawn, not just a dive"
        );
    }

    /// MECH-4: `DESTROYER_TURN_HARD` was a producer with no consumer.
    ///
    /// Style 37 trims the velocity twice a tick (`NPC.cs:50633-50651` then `:50652-50692`), and the
    /// first pass only lands while both axes already agree with the wanted heading. So a Destroyer
    /// already on course closes at `0.15 + 0.1` a tick, not at the shared style-6 rate of `0.1`.
    #[test]
    fn a_destroyer_already_on_course_trims_twice() {
        let tiles = buried();
        let w = night(&tiles, Some((4000.0, 4000.0)));
        let mut rng = SmallRng::seed_from_u64(3);
        let mut head = segment(DESTROYER_HEAD, 0.0, 0.0);
        // Already going down and right, which is where it wants to go, and well under top speed.
        head.velocity = (1.0, 1.0);
        destroyer(&mut head, &w, &mut rng);
        let across = head.velocity.0 - 1.0;
        let down = head.velocity.1 - 1.0;
        // Vanilla's own figures, written out rather than read back from the constants under test:
        // `num20 = 0.15f` then `num19 = 0.1f`, both applied.
        assert!(
            (across - 0.25).abs() < 1e-5,
            "expected 0.25 a tick across, got {across}"
        );
        assert!(
            (down - 0.25).abs() < 1e-5,
            "expected 0.25 a tick down, got {down}"
        );
    }

    /// ...and no other worm gets that pass: style 6 has a single rate (`NPC.cs:52662-52679`).
    #[test]
    fn an_ordinary_worm_trims_once() {
        let tiles = buried();
        let w = night(&tiles, Some((4000.0, 4000.0)));
        // Type 7, a Devourer head: the same burrow, one turn rate.
        let motion = terrustia_proto::npc_params::worm_motion(7, false, false);
        let mut worm = segment(7, 0.0, 0.0);
        worm.velocity = (1.0, 1.0);
        crate::game::ai::worm::update(&mut worm, &w, false);
        assert!(
            (worm.velocity.0 - 1.0 - motion.turn).abs() < 1e-5,
            "expected {} a tick, got {}",
            motion.turn,
            worm.velocity.0 - 1.0
        );
    }
}
