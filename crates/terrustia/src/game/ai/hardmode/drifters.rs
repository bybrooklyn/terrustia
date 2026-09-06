//! The simple hardmode drifters: styles 56, 63, 89, 95, 96 and 99.
//!
//! Each of these is one idea carried out plainly, which is why they share a file rather than each
//! getting one.
//!
//! * A **dungeon spirit** (56) homes on you at twelve pixels a tick, smoothed a hundred to one, so
//!   it drifts through walls in a long unhurried curve you cannot outrun by dodging.
//! * A **flocko** (63) charges, and *reverses* in daylight — the same aim, negated — so by day it
//!   flees rather than closes.
//! * A **Mothron egg** (89) is a timer that hatches, and hitting it sets the timer *back*: the way
//!   to stop one hatching is to keep hitting it. Near the end it starts twitching, harder the
//!   closer it gets.
//! * A **stardust cell** (95) shrinks its own momentum, swells, and turns into the grown version.
//! * A **stardust jellyfish** (96) holds station two hundred and fifty pixels above you and drops
//!   something every seventy ticks.
//! * **Solar goop** (99) falls, sticks where it lands, and dries up five ticks later.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    FLOCKO_RANGE, FLOCKO_SPEED, FLOCKO_SPIN_TICKS, GOOP_SETTLE_TICKS, JELLYFISH_ABOVE,
    JELLYFISH_EASE, JELLYFISH_EVERY, JELLYFISH_SHOT_DAMAGE, JELLYFISH_SPEED, MOTHRON_EGG_TICKS,
    MOTHRON_EGG_TICKS_EXPERT, MOTHRON_SPAWN, SPIRIT_SMOOTH, SPIRIT_SPEED, STARDUST_CELL_GROWN,
    STARDUST_CELL_TICKS,
};
use terrustia_proto::projectile::ids::JELLYFISH_SHOT;

use crate::game::ai::{Shot, World, face};
use crate::game::npc::{Npc, TileView};

/// What one of these concluded.
#[derive(Debug, Default)]
pub struct Outcome {
    /// A type it turned into.
    pub became: Option<u16>,
    /// Something it threw.
    pub shot: Option<Shot>,
    /// Several things it threw.
    pub shots: Vec<Shot>,
    /// Set when it is simply finished.
    pub spent: bool,
    /// Set when it finished by going off, which hurts whatever is standing next to it.
    pub detonated: bool,
    /// Anything it put into the world.
    pub spawn: Vec<crate::game::npc_ai::Spawn>,
    /// Set when it should count as killed rather than merely removed — a pillar that finishes
    /// collapsing has been beaten, and the world needs to know.
    pub died: bool,
    /// An item this routine wants put into the world where its NPC stands, outside the kill path.
    ///
    /// A pal hands its pet over and *leaves* rather than dying (`AI_127_Pal_GiveRewerd`,
    /// `NPC.cs:43481-43489`, an `Item.NewItem` followed by `life = 0; active = false;` with no
    /// strike), so its reward cannot come from a drop table keyed on being killed.
    pub reward: Option<i16>,
}

/// Ease toward a wanted velocity on both axes, doubling the push while still going the wrong way.
///
/// This is `NPC.SimpleFlyMovement`, which a good many hardmode routines lean on.
pub fn simple_fly(npc: &mut Npc, wanted: (f32, f32), speed: f32) {
    for (v, w) in [
        (&mut npc.velocity.0, wanted.0),
        (&mut npc.velocity.1, wanted.1),
    ] {
        if *v < w {
            *v += speed;
            if *v < 0.0 && w > 0.0 {
                *v += speed;
            }
        } else if *v > w {
            *v -= speed;
            if *v > 0.0 && w < 0.0 {
                *v -= speed;
            }
        }
    }
}

/// Style 56 — the dungeon spirit.
pub fn dungeon_spirit<T: TileView>(npc: &mut Npc, world: &World<'_, T>) {
    let Some(target) = world.target else {
        return;
    };
    face(npc, target);
    let (cx, cy) = npc.center();
    let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
    let k = SPIRIT_SPEED / (dx * dx + dy * dy).sqrt().max(0.01);
    // A hundred to one: it barely turns at all, and simply keeps coming.
    npc.velocity.0 = (npc.velocity.0 * SPIRIT_SMOOTH + dx * k) / (SPIRIT_SMOOTH + 1.0);
    npc.velocity.1 = (npc.velocity.1 * SPIRIT_SMOOTH + dy * k) / (SPIRIT_SMOOTH + 1.0);
    npc.rotation = (dy * k).atan2(dx * k) - 1.57;
    npc.dirty = true;
}

/// Style 63 — the flocko.
pub fn flocko<T: TileView>(npc: &mut Npc, world: &World<'_, T>) {
    let Some(target) = world.target else {
        return;
    };
    face(npc, target);
    let from = (
        npc.center().0 + f32::from(npc.direction) * 20.0,
        npc.center().1 + 6.0,
    );
    let (mut dx, mut dy) = (target.center.0 - from.0, target.center.1 - from.1);
    let reach = (dx * dx + dy * dy).sqrt().max(0.01);
    let k = FLOCKO_SPEED / reach;
    dx *= k;
    dy *= k;
    // By day it runs from you rather than at you: the same aim, negated.
    if world.conditions.day {
        dx = -dx;
        dy = -dy;
    }

    npc.ai[0] -= 1.0;
    if reach < FLOCKO_RANGE || npc.ai[0] > 0.0 {
        if reach < FLOCKO_RANGE {
            npc.ai[0] = FLOCKO_SPIN_TICKS;
        }
        npc.direction = if npc.velocity.0 < 0.0 { -1 } else { 1 };
        npc.rotation += f32::from(npc.direction) * 0.3;
        npc.dirty = true;
        return;
    }

    // The closer it gets the harder it steers, so its approach tightens into a dive.
    let ease = |v: &mut f32, want: f32, weight: f32| *v = (*v * weight + want) / (weight + 1.0);
    ease(&mut npc.velocity.0, dx, 50.0);
    ease(&mut npc.velocity.1, dy, 50.0);
    if reach < 350.0 {
        ease(&mut npc.velocity.0, dx, 10.0);
        ease(&mut npc.velocity.1, dy, 10.0);
    }
    if reach < 300.0 {
        ease(&mut npc.velocity.0, dx, 7.0);
        ease(&mut npc.velocity.1, dy, 7.0);
    }
    npc.rotation = npc.velocity.0 * 0.15;
    npc.dirty = true;
}

/// Style 89 — a Mothron egg.
///
/// `expert` halves the hatch time — but also makes a hit set it back only once rather than
/// twice, which is why an expert egg is not simply twice as fragile as it looks.
pub fn mothron_egg(npc: &mut Npc, was_hurt: bool, expert: bool, rng: &mut SmallRng) -> Outcome {
    let mut out = Outcome::default();
    if npc.velocity.1 == 0.0 {
        npc.velocity.0 *= 0.9;
        npc.rotation += npc.velocity.0 * 0.02;
    } else {
        npc.velocity.0 *= 0.99;
        npc.rotation += npc.velocity.0 * 0.04;
    }

    let hatch_at = if expert {
        MOTHRON_EGG_TICKS_EXPERT
    } else {
        MOTHRON_EGG_TICKS
    };

    // Hitting it sets the clock back, which is the only way to stop one hatching — twice over in
    // classic and normal mode, but only once in expert, where the shorter clock already does
    // most of the work.
    if was_hurt {
        npc.ai[0] -= rng.random_range(10..21) as f32;
        if !expert {
            npc.ai[0] -= rng.random_range(10..21) as f32;
        }
    }
    npc.ai[0] += 1.0;
    if npc.ai[0] >= hatch_at {
        out.became = Some(MOTHRON_SPAWN);
        return out;
    }

    // The last quarter of the wait: it starts twitching, harder the closer it is.
    if npc.velocity.1 == 0.0 && npc.velocity.0.abs() < 0.2 && npc.ai[0] >= hatch_at * 0.75 {
        let along = (npc.ai[0] - hatch_at * 0.75) / (hatch_at * 0.25);
        if (rng.random_range(-10..120) as f32) < along * 100.0 {
            npc.velocity.1 -= rng.random_range(20..40) as f32 * 0.025;
            npc.velocity.0 += rng.random_range(-20..20) as f32 * 0.025;
            npc.velocity.0 *= 1.0 + along * 2.0;
            npc.velocity.1 *= 1.0 + along * 2.0;
            npc.dirty = true;
        }
    }
    npc.dirty = true;
    out
}

/// Style 95 — a small stardust cell growing into a big one.
pub fn stardust_cell(npc: &mut Npc) -> Outcome {
    let mut out = Outcome::default();
    if npc.velocity.0.hypot(npc.velocity.1) > 4.0 {
        npc.velocity.0 *= 0.95;
        npc.velocity.1 *= 0.95;
    }
    npc.velocity.0 *= 0.99;
    npc.velocity.1 *= 0.99;

    npc.ai[0] += 1.0;
    let along = (npc.ai[0] / STARDUST_CELL_TICKS).clamp(0.0, 1.0);
    npc.scale = 1.0 + 0.3 * along;
    if npc.ai[0] >= STARDUST_CELL_TICKS {
        out.became = Some(STARDUST_CELL_GROWN);
        return out;
    }
    npc.rotation += npc.velocity.0 * 0.1;
    npc.dirty = true;
    out
}

/// Style 96 — the big stardust jellyfish.
pub fn stardust_jellyfish<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    rng: &mut SmallRng,
) -> Outcome {
    let mut out = Outcome::default();
    let Some(target) = world.target else {
        return out;
    };
    face(npc, target);

    let (cx, cy) = npc.center();
    let mut wanted = (target.center.0 - cx, target.center.1 - JELLYFISH_ABOVE - cy);
    let gap = wanted.0.hypot(wanted.1);
    // Four bands rather than a smooth approach, which is what makes it settle rather than drift in.
    let scale = match gap {
        g if g < 20.0 => None,
        g if g < 40.0 => Some(JELLYFISH_SPEED * 0.35),
        g if g < 80.0 => Some(JELLYFISH_SPEED * 0.65),
        _ => Some(JELLYFISH_SPEED),
    };
    match scale {
        None => wanted = npc.velocity,
        Some(speed) => {
            let k = speed / gap.max(0.01);
            wanted = (wanted.0 * k, wanted.1 * k);
        }
    }
    simple_fly(npc, wanted, JELLYFISH_EASE);
    npc.rotation = npc.velocity.0 * 0.1;

    npc.ai[0] += 1.0;
    if npc.ai[0] >= JELLYFISH_EVERY {
        npc.ai[0] = 0.0;
        // It drops something roughly downward, never straight down.
        let mut throw = (0.0f32, 0.0f32);
        while throw.0.abs() < 1.5 {
            let angle = (rng.random::<f32>() - 0.5) * std::f32::consts::PI;
            throw = (angle.sin() * 5.0, angle.cos() * 3.0);
        }
        out.shot = Some(Shot {
            projectile: JELLYFISH_SHOT,
            damage: JELLYFISH_SHOT_DAMAGE,
            position: npc.center(),
            velocity: throw,
            time_left: 300,
            // `NewProjectile(..., 0f, whoAmI)` (`NPC.cs:39860`). The small one is an escort, not a
            // drop: `aiStyle 102` reads `ai[1]` to find the parent it hovers beside for three and
            // a half seconds before it picks anyone. Launched without it, it left in a straight
            // line at whatever it was thrown with.
            ai: [0.0, f32::from(world.slot), 0.0],
        });
        npc.dirty = true;
    }
    npc.dirty = true;
    out
}

/// Style 99 — solar goop.
pub fn solar_goop(npc: &mut Npc) -> Outcome {
    let mut out = Outcome::default();
    if npc.velocity.1 == 0.0 && npc.ai[0] == 0.0 {
        npc.ai[0] = 1.0;
        npc.ai[1] = 0.0;
        npc.dirty = true;
        return out;
    }
    if npc.ai[0] == 1.0 {
        // Landed: it holds exactly still and dries up.
        npc.velocity = (0.0, 0.0);
        npc.position = npc.old_position;
        npc.ai[1] += 1.0;
        if npc.ai[1] >= GOOP_SETTLE_TICKS {
            out.spent = true;
        }
        return out;
    }
    npc.velocity.1 = (npc.velocity.1 + 0.2).min(12.0);
    npc.dirty = true;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use terrustia_proto::tile::Tile;

    struct Void;

    impl TileView for Void {
        fn tile(&self, _x: i32, _y: i32) -> Tile {
            Tile::AIR
        }
    }

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(2)
    }

    fn world<'a>(tiles: &'a Void, target: Option<Target>) -> World<'a, Void> {
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

    fn npc(npc_type: u16) -> Npc {
        Npc::new(npc_type, (10_000.0, 10_000.0), 1).expect("a hardmode type")
    }

    #[test]
    fn a_dungeon_spirit_barely_turns_but_never_stops() {
        let tiles = Void;
        let mut s = npc(288);
        let (cx, cy) = s.center();
        let t = Some(player_at(cx + 2000.0, cy));
        for _ in 0..600 {
            dungeon_spirit(&mut s, &world(&tiles, t));
        }
        let speed = s.velocity.0.hypot(s.velocity.1);
        assert!(
            (speed - SPIRIT_SPEED).abs() < 1.0,
            "should settle at its speed, got {speed}"
        );
        assert!(s.velocity.0 > 0.0, "and be coming at you");
    }

    /// The same aim, negated: by day a flocko runs.
    #[test]
    fn a_flocko_charges_at_night_and_flees_by_day() {
        let tiles = Void;
        let heading = |day: bool| {
            let mut f = npc(63);
            f.ai[0] = -100.0;
            let (cx, cy) = f.center();
            let t = Some(player_at(cx + 800.0, cy));
            let mut w = world(&tiles, t);
            w.conditions.day = day;
            for _ in 0..200 {
                flocko(&mut f, &w);
            }
            f.velocity.0
        };
        assert!(heading(false) > 0.0, "at night it comes at you");
        assert!(heading(true) < 0.0, "by day it does not");
    }

    #[test]
    fn a_mothron_egg_hatches_on_a_timer() {
        let mut e = npc(89);
        let mut r = rng();
        let mut hatched = None;
        for _ in 0..(MOTHRON_EGG_TICKS as i32 + 5) {
            if let Some(into) = mothron_egg(&mut e, false, false, &mut r).became {
                hatched = Some(into);
                break;
            }
        }
        assert_eq!(hatched, Some(MOTHRON_SPAWN));
    }

    /// Hitting an egg sets its clock back, so keeping at it is what stops the hatch.
    #[test]
    fn hitting_a_mothron_egg_delays_it() {
        let time_to_hatch = |hit: bool| {
            let mut r = rng();
            let mut e = npc(89);
            for tick in 0..(MOTHRON_EGG_TICKS as i32 * 3) {
                if mothron_egg(&mut e, hit, false, &mut r).became.is_some() {
                    return tick;
                }
            }
            i32::MAX
        };
        assert!(
            time_to_hatch(true) > time_to_hatch(false),
            "a battered egg takes longer"
        );
    }

    /// Expert Mode halves the hatch clock, but also only sets it back once when hit rather than
    /// twice — so an expert egg does not hatch twice as fast under fire as the raw clock alone
    /// would suggest, and a normal-mode egg does not take the expert clock at all.
    #[test]
    fn expert_mode_shortens_the_clock_but_softens_the_setback() {
        let hatches_by = |expert: bool, tick_cap: i32| {
            let mut r = rng();
            let mut e = npc(89);
            (0..tick_cap).find(|_tick| mothron_egg(&mut e, false, expert, &mut r).became.is_some())
        };
        let normal = hatches_by(false, MOTHRON_EGG_TICKS as i32 + 5).expect("it should hatch");
        let expert = hatches_by(true, MOTHRON_EGG_TICKS as i32 + 5).expect("it should hatch");
        assert_eq!(normal, MOTHRON_EGG_TICKS as i32 - 1);
        assert_eq!(
            expert,
            MOTHRON_EGG_TICKS_EXPERT as i32 - 1,
            "half the clock"
        );

        // Being hit sets it back once in expert mode and twice in normal mode. Both branches draw
        // their first setback from the same fresh, identically-seeded rng, so they are bit-for-bit
        // comparable up to that draw; classic then takes a second draw expert does not, so one hit
        // should always leave the expert egg strictly further along than the classic one.
        let after_one_hit = |expert: bool| {
            let mut r = rng();
            let mut e = npc(89);
            // Comfortably clear of both the hatch and the twitch thresholds either way.
            e.ai[0] = 300.0;
            mothron_egg(&mut e, true, expert, &mut r);
            e.ai[0]
        };
        let normal_after = after_one_hit(false);
        let expert_after = after_one_hit(true);
        assert!(
            expert_after > normal_after,
            "one hit should set a classic egg back further than an expert one: \
             classic {normal_after} vs expert {expert_after}"
        );
    }

    #[test]
    fn a_stardust_cell_swells_and_grows_up() {
        let mut c = npc(95);
        let mut grown = None;
        let mut biggest: f32 = 0.0;
        for _ in 0..(STARDUST_CELL_TICKS as i32 + 5) {
            let out = stardust_cell(&mut c);
            biggest = biggest.max(c.scale);
            if let Some(into) = out.became {
                grown = Some(into);
                break;
            }
        }
        assert_eq!(grown, Some(STARDUST_CELL_GROWN));
        assert!(biggest > 1.2, "it should have swelled, got {biggest}");
    }

    #[test]
    fn a_stardust_jellyfish_hangs_above_you_and_drops_things() {
        let tiles = Void;
        let mut j = npc(96);
        let (cx, cy) = j.center();
        let t = Some(player_at(cx, cy + 600.0));
        let mut r = rng();
        let mut dropped = Vec::new();
        for _ in 0..600 {
            if let Some(shot) = stardust_jellyfish(&mut j, &world(&tiles, t), &mut r).shot {
                dropped.push(shot);
            }
            j.position.0 += j.velocity.0;
            j.position.1 += j.velocity.1;
        }
        assert!(!dropped.is_empty(), "it should have dropped something");
        assert_eq!(dropped[0].projectile, JELLYFISH_SHOT);
        assert!(
            dropped.iter().all(|s| s.velocity.0.abs() >= 1.5),
            "and never straight down"
        );
        assert!(
            j.center().1 < cy + 600.0,
            "and be above its target, at {}",
            j.center().1
        );
    }

    #[test]
    fn solar_goop_falls_sticks_and_dries_up() {
        let mut g = npc(99);
        g.velocity.1 = 1.0;
        solar_goop(&mut g);
        assert!(g.velocity.1 > 1.0, "it should be falling");

        g.velocity.1 = 0.0;
        assert!(!solar_goop(&mut g).spent, "landing is not the end of it");
        assert_eq!(g.ai[0], 1.0);
        let mut spent = false;
        for _ in 0..(GOOP_SETTLE_TICKS as i32 + 2) {
            spent |= solar_goop(&mut g).spent;
        }
        assert!(spent, "and then it dries up");
    }
    /// A dropped spawn is told which jellyfish made it.
    ///
    /// `NewProjectile(..., 0f, whoAmI)` (`NPC.cs:39860`), and `aiStyle 102` reads `ai[1]` to find
    /// the parent it hovers beside for three and a half seconds before it picks anyone. Launched
    /// without it, the small one left in a straight line at whatever it was thrown with.
    #[test]
    fn a_dropped_spawn_carries_its_parents_slot() {
        let tiles = Void;
        let mut j = npc(96);
        let (cx, cy) = j.center();
        let t = Some(player_at(cx, cy + 600.0));
        let mut w = world(&tiles, t);
        w.slot = 7;
        let mut r = rng();
        let mut dropped = None;
        for _ in 0..600 {
            if let Some(shot) = stardust_jellyfish(&mut j, &w, &mut r).shot {
                dropped = Some(shot);
                break;
            }
            j.position.0 += j.velocity.0;
            j.position.1 += j.velocity.1;
        }
        let shot = dropped.expect("it should have dropped one");
        assert_eq!(shot.ai[1], 7.0, "the slot the caller filled in, not zero");
    }
}
