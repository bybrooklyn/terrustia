//! Styles 23, 49, 62, 70, 100, 101 and 116 — things that hang in the air, and one that skates.
//!
//! * A **flying weapon** (23) — the enchanted sword, the cursed hammer — commits to a charge, holds
//!   it for a hundred ticks, then hangs spinning for two hundred more. Hitting one interrupts the
//!   charge and sends it straight to the spin, which is why they are easier to fight up close than
//!   at range.
//! * An **angry nimbus** (49) parks two hundred pixels above you and rains, but only while it is
//!   actually overhead and can see you.
//! * An **elf copter** (62) closes to within six hundred pixels, stops, and fires missiles while
//!   nearly stationary. Daylight reverses it, exactly as it does the flocko.
//! * A **detonating bubble** (70) drifts on the wind toward you and pops — on a fuse, or the moment
//!   anyone comes within forty pixels. Popping makes it briefly enormous and untouchable.
//! * An **ancient light** (100) flies until it hits something, sticks for five ticks, and is gone.
//! * An **ancient doom** (101) fades in over two seconds and fires four shots outward when its time
//!   is up — and it counts *faster* the more hurt its summoner is.
//! * A **water strider** (116) skates on the surface film: it is pushed up to the water line rather
//!   than floating in it, and shoves itself along every couple of seconds.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    BUBBLE_BLAST_SIZE, BUBBLE_BLAST_TICKS, BUBBLE_FUSE, BUBBLE_TRIGGER, COPTER_RANGE,
    COPTER_RELOAD, COPTER_SHOT_DAMAGE, COPTER_SHOT_SPEED, COPTER_SPEED, DOOM_FADE_IN,
    DOOM_LIFETIME, DOOM_SHOT_SPEED, FLYING_WEAPON_DRIVE, FLYING_WEAPON_REST, FLYING_WEAPON_SPEED,
    NIMBUS_ABOVE, NIMBUS_ACCEL, NIMBUS_EVERY, NIMBUS_SHOT_DAMAGE, NIMBUS_SPEED, STRIDER_RISE,
    STRIDER_RISE_CAP, STRIDER_SKIP, STRIDER_WAIT, STRIDER_WAIT_DRY,
};
use terrustia_proto::projectile::ids::{COPTER_SHOT, DOOM_SHOT, NIMBUS_SHOT};

use super::drifters::Outcome;
use crate::game::ai::{Shot, World, can_see, face};
use crate::game::npc::{Npc, TileView};

/// Ease one axis toward a wanted velocity, doubling the push while still going the wrong way.
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

/// Style 23 — a flying weapon.
pub fn flying_weapon<T: TileView>(npc: &mut Npc, world: &World<'_, T>) {
    npc.no_gravity = true;
    npc.no_tile_collide = true;
    let Some(target) = world.target else {
        return;
    };

    // Being hit while charging cancels it: it goes straight to the spin.
    if world.was_hurt && npc.ai[0] != 0.0 {
        npc.ai[0] = 2.0;
        npc.ai[1] = 0.0;
    }

    if npc.ai[0] == 0.0 {
        // Committing. Aimed once, and never corrected.
        let (cx, cy) = npc.center();
        let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
        let k = FLYING_WEAPON_SPEED / (dx * dx + dy * dy).sqrt().max(0.01);
        npc.velocity = (dx * k, dy * k);
        npc.rotation = npc.velocity.1.atan2(npc.velocity.0) + 0.785;
        npc.ai[0] = 1.0;
        npc.ai[1] = 0.0;
        npc.dirty = true;
    } else if npc.ai[0] == 1.0 {
        npc.velocity.0 *= 0.99;
        npc.velocity.1 *= 0.99;
        npc.ai[1] += 1.0;
        if npc.ai[1] >= FLYING_WEAPON_DRIVE {
            npc.ai[0] = 2.0;
            npc.ai[1] = 0.0;
            npc.velocity = (0.0, 0.0);
            npc.dirty = true;
        } else {
            npc.rotation = npc.velocity.1.atan2(npc.velocity.0) + 0.785;
        }
    } else {
        // Resting, and spinning faster the longer it rests.
        npc.velocity.0 *= 0.96;
        npc.velocity.1 *= 0.96;
        npc.ai[1] += 1.0;
        let spin = 0.1 + (npc.ai[1] / FLYING_WEAPON_REST) * 0.4;
        npc.rotation += spin * f32::from(npc.direction);
        if npc.ai[1] >= FLYING_WEAPON_REST {
            npc.ai[0] = 0.0;
            npc.ai[1] = 0.0;
            npc.dirty = true;
        }
    }
    npc.dirty = true;
}

/// Style 49 — an angry nimbus.
pub fn nimbus<T: TileView>(npc: &mut Npc, world: &World<'_, T>) -> Option<Shot> {
    npc.no_gravity = true;
    let target = world.target?;
    face(npc, target);

    let (cx, cy) = npc.center();
    let (mut dx, mut dy) = (target.center.0 - cx, target.center.1 - NIMBUS_ABOVE - cy);
    let gap = (dx * dx + dy * dy).sqrt();
    if gap < 20.0 {
        dx = npc.velocity.0;
        dy = npc.velocity.1;
    } else {
        let k = NIMBUS_SPEED / gap;
        dx *= k;
        dy *= k;
    }
    close_on(&mut npc.velocity.0, dx, NIMBUS_ACCEL);
    close_on(&mut npc.velocity.1, dy, NIMBUS_ACCEL);
    npc.dirty = true;

    // It only rains while it is actually overhead, which is what makes stepping aside work.
    let overhead = npc.position.0 + npc.width()
        > target.center.0 - super::super::PLAYER_WIDTH as f32 / 2.0
        && npc.position.0 < target.center.0 + super::super::PLAYER_WIDTH as f32 / 2.0
        && npc.position.1 + npc.height() < target.center.1
        && can_see(world.tiles, npc, target);
    if !overhead {
        return None;
    }
    npc.ai[0] += 1.0;
    if npc.ai[0] <= NIMBUS_EVERY {
        return None;
    }
    npc.ai[0] = 0.0;
    Some(Shot {
        projectile: NIMBUS_SHOT,
        damage: NIMBUS_SHOT_DAMAGE,
        position: (
            npc.position.0 + npc.width() / 2.0,
            npc.position.1 + npc.height() + 4.0,
        ),
        velocity: (0.0, 5.0),
        time_left: 300,
        ai: [0.0; 3],
    })
}

/// Style 62 — an elf copter.
pub fn copter<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    rng: &mut SmallRng,
) -> Option<Shot> {
    let target = world.target?;
    face(npc, target);
    npc.rotation = npc.velocity.0.abs() * f32::from(npc.direction) * 0.1;
    npc.sprite_direction = npc.direction;

    let muzzle = (
        npc.center().0 + f32::from(npc.direction) * 20.0,
        npc.center().1 + 6.0,
    );
    let (dx, dy) = (
        target.center.0 - muzzle.0,
        target.center.1 - super::super::PLAYER_HEIGHT as f32 / 2.0 - muzzle.1,
    );
    let gap = (dx * dx + dy * dy).sqrt().max(0.01);
    let k = COPTER_SPEED / gap;
    let (toward_x, toward_y) = (dx * k, dy * k);
    let visible = can_see(world.tiles, npc, target);

    // Daylight turns the approach into a retreat.
    if world.conditions.day {
        npc.velocity.0 = (npc.velocity.0 * 59.0 - toward_x) / 60.0;
        npc.velocity.1 = (npc.velocity.1 * 59.0 - toward_y) / 60.0;
        npc.time_left = npc.time_left.min(10);
        npc.dirty = true;
        return None;
    }
    if gap > COPTER_RANGE || !visible {
        npc.velocity.0 = (npc.velocity.0 * 59.0 + toward_x) / 60.0;
        npc.velocity.1 = (npc.velocity.1 * 59.0 + toward_y) / 60.0;
        npc.dirty = true;
        return None;
    }

    // In range: it stops, and only fires once it has nearly stopped.
    npc.velocity.0 *= 0.98;
    npc.velocity.1 *= 0.98;
    npc.dirty = true;
    if npc.velocity.0.abs() >= 1.0 || npc.velocity.1.abs() >= 1.0 {
        return None;
    }
    npc.local_ai[0] += 1.0;
    if npc.local_ai[0] < COPTER_RELOAD {
        return None;
    }
    npc.local_ai[0] = 0.0;
    let mut aim = (
        target.center.0 - muzzle.0 + rng.random_range(-35..36) as f32,
        target.center.1 - muzzle.1 + rng.random_range(-35..36) as f32,
    );
    aim.0 *= 1.0 + rng.random_range(-20..21) as f32 * 0.015;
    aim.1 *= 1.0 + rng.random_range(-20..21) as f32 * 0.015;
    let k = COPTER_SHOT_SPEED / (aim.0 * aim.0 + aim.1 * aim.1).sqrt().max(0.01);
    Some(Shot {
        projectile: COPTER_SHOT,
        damage: COPTER_SHOT_DAMAGE,
        position: muzzle,
        velocity: (aim.0 * k, aim.1 * k),
        time_left: 300,
        ai: [0.0; 3],
    })
}

/// Style 70 — a detonating bubble.
pub fn detonating_bubble<T: TileView>(
    npc: &mut Npc,
    world: &World<'_, T>,
    rng: &mut SmallRng,
) -> Outcome {
    let mut out = Outcome::default();
    let Some(target) = world.target else {
        return out;
    };

    // First tick: it is thrown off in roughly the player's direction, at a random size and speed.
    if npc.ai[2] == 0.0 {
        npc.ai[2] = 1.0;
        npc.ai[3] = rng.random_range(80..121) as f32 / 100.0;
        let speed = rng.random_range(165..265) as f32 / 15.0;
        let (cx, cy) = npc.center();
        let away = (
            target.center.0 - cx + rng.random_range(-100..101) as f32,
            target.center.1 - cy + rng.random_range(-100..101) as f32,
        );
        let k = speed / (away.0 * away.0 + away.1 * away.1).sqrt().max(0.01);
        npc.velocity = (away.0 * k, away.1 * k);
        npc.dirty = true;
    }

    // It drifts: mostly on the wind, and always slightly upward.
    let (cx, cy) = npc.center();
    let toward = {
        let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
        let length = (dx * dx + dy * dy).sqrt().max(0.01);
        (dx / length, dy / length)
    };
    npc.velocity.0 = (npc.velocity.0 * 40.0 + toward.0 * 20.0) / 41.0;
    npc.velocity.1 = (npc.velocity.1 * 40.0 + toward.1 * 20.0) / 41.0;
    npc.scale = npc.ai[3];
    npc.velocity.0 = (npc.velocity.0 * 50.0
        + world.conditions.wind * 2.0
        + rng.random_range(-10..11) as f32 * 0.1)
        / 51.0;
    npc.velocity.1 = (npc.velocity.1 * 50.0 - 0.25 + rng.random_range(-10..11) as f32 * 0.2) / 51.0;
    if npc.velocity.1 > 0.0 {
        npc.velocity.1 -= 0.04;
    }

    if npc.ai[0] == 0.0 {
        // Anyone within forty pixels sets it off early — and so does being shot, which is
        // supposed to kill one of these outright rather than just being ignored until the fuse.
        let close = (cx - target.center.0).abs() < BUBBLE_TRIGGER + npc.width()
            && (cy - target.center.1).abs() < BUBBLE_TRIGGER + npc.height();
        if close || world.was_hurt {
            npc.ai[0] = 1.0;
            npc.ai[1] = BUBBLE_BLAST_TICKS;
            npc.dirty = true;
        } else {
            npc.ai[1] += 1.0;
            if npc.ai[1] >= BUBBLE_FUSE {
                npc.ai[0] = 1.0;
                npc.ai[1] = BUBBLE_BLAST_TICKS;
            }
        }
    }
    if npc.ai[0] == 1.0 {
        npc.ai[1] -= 1.0;
        if npc.ai[1] <= 0.0 {
            out.spent = true;
            return out;
        }
        // Going off: briefly enormous, and nothing can stop it now.
        npc.invulnerable = true;
        let middle = npc.center();
        npc.stats.width = BUBBLE_BLAST_SIZE;
        npc.stats.height = BUBBLE_BLAST_SIZE;
        npc.position = (
            middle.0 - BUBBLE_BLAST_SIZE as f32 / 2.0,
            middle.1 - BUBBLE_BLAST_SIZE as f32 / 2.0,
        );
        npc.time_left = npc.time_left.min(3);
    }
    npc.dirty = true;
    out
}

/// Style 100 — an ancient light.
///
/// It is thrown with a velocity of its own, in `ai[2]`/`ai[3]`, rather than starting from rest.
/// After a second it starts to arc — `ai[1]` is a per-tick rotation, so its path curves rather
/// than bends once — and after two it sheds speed, which is what eventually brings it to rest:
/// once its velocity dies away to nothing, the landed check above catches it on the next tick and
/// it sticks for five ticks before it is gone, exactly as it does when it hits something.
pub fn ancient_light(npc: &mut Npc) -> Outcome {
    let mut out = Outcome::default();

    // First tick: launched with the velocity it was thrown with, not the zero it spawned at —
    // this has to happen before the landed check below, or a light that spawns at rest (as one
    // freshly placed in the world does) would read as already landed and die on the spot.
    if npc.local_ai[0] == 0.0 {
        npc.local_ai[0] = 1.0;
        npc.velocity = (npc.ai[2], npc.ai[3]);
    }

    if npc.velocity.1 == 0.0 && npc.ai[0] >= 0.0 {
        npc.ai[0] = -1.0;
        npc.ai[1] = 0.0;
        npc.dirty = true;
        return out;
    }
    if npc.ai[0] == -1.0 {
        npc.velocity = (0.0, 0.0);
        npc.position = npc.old_position;
        npc.ai[1] += 1.0;
        if npc.ai[1] >= 5.0 {
            out.spent = true;
        }
        return out;
    }

    if npc.ai[0] >= 0.0 {
        npc.ai[0] += 1.0;
        if npc.ai[0] > 60.0 {
            npc.velocity = rotated_by(npc.velocity, npc.ai[1]);
        }
        if npc.ai[0] > 120.0 {
            npc.velocity.0 *= 0.98;
            npc.velocity.1 *= 0.98;
        }
        if npc.velocity.0.hypot(npc.velocity.1) < 0.2 {
            npc.velocity = (0.0, 0.0);
        }
    }

    npc.rotation = npc.velocity.1.atan2(npc.velocity.0) - std::f32::consts::FRAC_PI_2;
    npc.dirty = true;
    out
}

/// Turn a vector by an angle in radians, the way `Vector2.RotatedBy` does.
fn rotated_by(v: (f32, f32), angle: f32) -> (f32, f32) {
    let (sin, cos) = angle.sin_cos();
    (v.0 * cos - v.1 * sin, v.0 * sin + v.1 * cos)
}

/// Style 101 — an ancient doom.
///
/// `summoner_health` is the fraction its parent has left; the more hurt that is, the faster this
/// counts down.
pub fn ancient_doom(npc: &mut Npc, summoner_health: Option<f32>) -> Outcome {
    let mut out = Outcome::default();
    let Some(health) = summoner_health else {
        out.spent = true;
        return out;
    };
    // A wounded cultist makes these arrive two and three times as fast.
    let rate = if health < 0.25 {
        3.0
    } else if health < 0.5 {
        2.0
    } else {
        1.0
    };
    npc.ai[1] += rate;

    let along = (npc.ai[1] / DOOM_FADE_IN).clamp(0.0, 1.0);
    npc.scale = along;
    if npc.ai[1] >= DOOM_LIFETIME {
        // Four shots, one to each quarter of the compass.
        for quarter in 0..4 {
            let angle = std::f32::consts::FRAC_PI_2 * quarter as f32;
            out.shots.push(Shot {
                projectile: DOOM_SHOT,
                damage: npc.stats.damage,
                position: npc.center(),
                velocity: (
                    angle.sin() * DOOM_SHOT_SPEED,
                    -angle.cos() * DOOM_SHOT_SPEED,
                ),
                time_left: 300,
                ai: [0.0; 3],
            });
        }
        out.spent = true;
    }
    npc.dirty = true;
    out
}

/// Style 116 — a water strider.
///
/// `water_line` is the surface height in its own column, if it is over water at all.
pub fn water_strider(npc: &mut Npc, water_line: Option<f32>, rng: &mut SmallRng, wet: bool) {
    let mut on_the_film = false;
    if let Some(line) = water_line {
        let feet = npc.position.1 + npc.height() - 1.0;
        if npc.center().1 > line {
            // Under the surface: pushed up toward it, and never past it.
            npc.velocity.1 = (npc.velocity.1 - STRIDER_RISE).max(-STRIDER_RISE_CAP);
            if feet + npc.velocity.1 < line {
                npc.velocity.1 = line - feet;
            }
        } else {
            npc.velocity.1 = npc.velocity.1.min(line - feet);
            on_the_film = true;
        }
    } else if wet {
        npc.velocity.1 -= 0.3;
    }

    if npc.ai[0] != 0.0 {
        return;
    }
    npc.ai[1] += 1.0;
    npc.velocity.0 *= 0.9;
    if npc.velocity.1 == 0.0 {
        npc.velocity.0 *= 0.6;
    }
    let floating = wet || on_the_film;
    let footed = floating || npc.velocity.1 == 0.0;
    let wait = if floating {
        rng.random_range(STRIDER_WAIT.0..STRIDER_WAIT.1)
    } else {
        rng.random_range(STRIDER_WAIT_DRY.0..STRIDER_WAIT_DRY.1)
    } as f32;
    if !footed || npc.ai[1] < wait {
        npc.dirty = true;
        return;
    }
    npc.ai[1] = 0.0;
    npc.velocity.0 = (rng.random::<f32>() * 2.0 - 1.0) * STRIDER_SKIP;
    if !floating {
        // On land it has to hop rather than skate, and then waits longer.
        if npc.velocity.1 == 0.0 {
            npc.velocity.1 = -2.0;
        }
        npc.ai[1] = 60.0;
    }
    npc.dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    #[derive(Default)]
    struct Air(HashMap<(i32, i32), Tile>);

    impl TileView for Air {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(70)
    }

    fn world<'a>(tiles: &'a Air, target: Option<Target>) -> World<'a, Air> {
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
    fn a_flying_weapon_charges_holds_and_rests() {
        let tiles = Air::default();
        let mut w = npc(83);
        let (cx, cy) = w.center();
        let t = Some(player_at(cx + 500.0, cy));
        flying_weapon(&mut w, &world(&tiles, t));
        assert_eq!(w.ai[0], 1.0, "committed");
        let speed = w.velocity.0.hypot(w.velocity.1);
        assert!((speed - FLYING_WEAPON_SPEED).abs() < 1e-3);

        for _ in 0..(FLYING_WEAPON_DRIVE as i32 + 1) {
            flying_weapon(&mut w, &world(&tiles, t));
        }
        assert_eq!(w.ai[0], 2.0, "then rests");
        // ...and then commits again, so the cycle is what to look for rather than a single tick.
        let mut recommitted = false;
        for _ in 0..(FLYING_WEAPON_REST as i32 + 2) {
            flying_weapon(&mut w, &world(&tiles, t));
            if w.ai[0] == 1.0 && w.ai[1] <= 1.0 {
                recommitted = true;
                break;
            }
        }
        assert!(recommitted, "then goes again");
    }

    /// Hitting one interrupts its charge, which is why they are easier to fight up close.
    #[test]
    fn hitting_a_flying_weapon_cancels_its_charge() {
        let tiles = Air::default();
        let mut w = npc(83);
        let (cx, cy) = w.center();
        let t = Some(player_at(cx + 500.0, cy));
        flying_weapon(&mut w, &world(&tiles, t));
        assert_eq!(w.ai[0], 1.0);
        let mut hit = world(&tiles, t);
        hit.was_hurt = true;
        flying_weapon(&mut w, &hit);
        assert_eq!(w.ai[0], 2.0, "straight to the rest");
    }

    #[test]
    fn a_nimbus_rains_only_while_it_is_overhead() {
        let tiles = Air::default();
        let mut n = npc(253);
        n.position = (10_000.0, 10_000.0);
        // Directly below it.
        let under = Some(player_at(n.center().0, n.position.1 + n.height() + 300.0));
        let mut rained = None;
        for _ in 0..40 {
            if let Some(shot) = nimbus(&mut n, &world(&tiles, under)) {
                rained = Some(shot);
                break;
            }
        }
        let shot = rained.expect("it should rain on someone underneath it");
        assert_eq!(shot.projectile, NIMBUS_SHOT);
        assert!(shot.velocity.1 > 0.0, "downward");

        // Step aside and it stops.
        let mut dry = npc(253);
        dry.position = (10_000.0, 10_000.0);
        let beside = Some(player_at(dry.center().0 + 900.0, dry.position.1 + 300.0));
        for _ in 0..40 {
            assert!(nimbus(&mut dry, &world(&tiles, beside)).is_none());
        }
    }

    #[test]
    fn a_copter_closes_stops_and_fires() {
        let tiles = Air::default();
        let mut c = npc(62);
        let (cx, cy) = c.center();
        let t = Some(player_at(cx + 300.0, cy));
        let mut r = rng();
        let mut fired = None;
        for _ in 0..300 {
            if let Some(shot) = copter(&mut c, &world(&tiles, t), &mut r) {
                fired = Some(shot);
                break;
            }
        }
        let shot = fired.expect("it should have fired");
        assert_eq!(shot.projectile, COPTER_SHOT);
    }

    #[test]
    fn a_copter_backs_off_in_daylight() {
        let tiles = Air::default();
        let mut c = npc(62);
        let (cx, cy) = c.center();
        let t = Some(player_at(cx + 300.0, cy));
        let mut day = world(&tiles, t);
        day.conditions.day = true;
        let mut r = rng();
        for _ in 0..200 {
            assert!(copter(&mut c, &day, &mut r).is_none());
        }
        assert!(c.velocity.0 < 0.0, "it should be retreating");
        assert!(c.time_left <= 10);
    }

    #[test]
    fn a_bubble_pops_when_you_come_near_it() {
        let tiles = Air::default();
        let mut b = npc(371);
        let (cx, cy) = b.center();
        let mut r = rng();
        // Far off: it drifts.
        detonating_bubble(
            &mut b,
            &world(&tiles, Some(player_at(cx + 4000.0, cy))),
            &mut r,
        );
        assert_eq!(b.ai[0], 0.0);
        // Close: it arms.
        detonating_bubble(
            &mut b,
            &world(&tiles, Some(player_at(cx + 10.0, cy))),
            &mut r,
        );
        assert_eq!(b.ai[0], 1.0, "it should have armed");
    }

    /// Being shot sets it off too, not just the proximity fuse or the timer.
    #[test]
    fn a_bubble_pops_when_shot() {
        let tiles = Air::default();
        let mut b = npc(371);
        let (cx, cy) = b.center();
        let mut r = rng();
        // Far off, so proximity would not trigger it — but it is shot.
        let mut w = world(&tiles, Some(player_at(cx + 4000.0, cy)));
        w.was_hurt = true;
        detonating_bubble(&mut b, &w, &mut r);
        assert_eq!(b.ai[0], 1.0, "being shot should have armed it");
    }

    #[test]
    fn a_bubble_going_off_is_briefly_enormous_and_untouchable() {
        let tiles = Air::default();
        let mut b = npc(371);
        let (cx, cy) = b.center();
        let mut r = rng();
        let small = b.stats.width;
        b.ai[0] = 1.0;
        b.ai[1] = BUBBLE_BLAST_TICKS;
        detonating_bubble(&mut b, &world(&tiles, Some(player_at(cx, cy))), &mut r);
        assert!(b.stats.width > small, "it should have swelled");
        assert!(b.invulnerable, "and be past stopping");
    }

    #[test]
    fn an_ancient_light_sticks_where_it_lands() {
        let mut l = npc(522);
        // Thrown with a real velocity, the way it actually spawns.
        l.ai[2] = 5.0;
        l.ai[3] = 5.0;
        assert!(!ancient_light(&mut l).spent);
        l.velocity.1 = 0.0;
        ancient_light(&mut l);
        assert_eq!(l.ai[0], -1.0);
        let mut spent = false;
        for _ in 0..8 {
            spent |= ancient_light(&mut l).spent;
        }
        assert!(spent);
    }

    /// It is launched with the velocity it was thrown with, arcs off that heading after a
    /// second, sheds speed after two, and — rather than flying dead straight forever — eventually
    /// decelerates to a stop and dies on its own, with nothing to hit.
    #[test]
    fn an_ancient_light_curves_then_slows_then_dies_on_its_own() {
        let mut l = npc(522);
        l.ai[2] = 6.0;
        l.ai[3] = -3.0;
        // A gentle per-tick turn, so it arcs rather than snapping round all at once.
        l.ai[1] = 0.05;

        ancient_light(&mut l);
        assert_eq!(
            l.velocity,
            (6.0, -3.0),
            "launched with ai[2]/ai[3], not at rest"
        );

        // Dead straight for the first minute.
        for _ in 0..59 {
            ancient_light(&mut l);
        }
        assert_eq!(
            l.velocity,
            (6.0, -3.0),
            "still on its original heading at 60 ticks"
        );

        // Past sixty it curves off that heading.
        for _ in 0..30 {
            ancient_light(&mut l);
        }
        assert!(
            (l.velocity.1 - -3.0).abs() > 0.5,
            "it should have arced off its original heading: {:?}",
            l.velocity
        );

        // It never hits anything here, yet it still eventually comes to rest and is gone.
        let mut spent = false;
        for _ in 0..1000 {
            spent |= ancient_light(&mut l).spent;
            if spent {
                break;
            }
        }
        assert!(spent, "it should decelerate to a stop and die on its own");
    }

    /// A wounded cultist makes its dooms arrive three times as fast.
    #[test]
    fn an_ancient_doom_counts_faster_for_a_wounded_summoner() {
        let ticks_to_fire = |health: f32| {
            let mut d = npc(524);
            for tick in 0..(DOOM_LIFETIME as i32 * 2) {
                if ancient_doom(&mut d, Some(health)).spent {
                    return tick;
                }
            }
            i32::MAX
        };
        assert!(ticks_to_fire(0.1) < ticks_to_fire(1.0) / 2);
    }

    #[test]
    fn an_ancient_doom_lets_go_of_four_shots() {
        let mut d = npc(524);
        let mut shots = Vec::new();
        for _ in 0..(DOOM_LIFETIME as i32 + 2) {
            let out = ancient_doom(&mut d, Some(1.0));
            shots.extend(out.shots);
            if out.spent {
                break;
            }
        }
        assert_eq!(shots.len(), 4);
        assert!(shots.iter().all(|s| s.projectile == DOOM_SHOT));
        // One to each quarter: the four headings should all differ.
        let mut headings: Vec<String> = shots
            .iter()
            .map(|s| format!("{:.1},{:.1}", s.velocity.0, s.velocity.1))
            .collect();
        headings.sort();
        headings.dedup();
        assert_eq!(headings.len(), 4);
    }

    #[test]
    fn a_doom_without_a_summoner_simply_goes() {
        let mut d = npc(524);
        assert!(ancient_doom(&mut d, None).spent);
    }

    #[test]
    fn a_water_strider_is_pushed_up_to_the_surface_film() {
        let mut s = npc(626);
        // Its middle is below the line, so it should rise.
        let line = s.center().1 - 20.0;
        water_strider(&mut s, Some(line), &mut rng(), true);
        assert!(s.velocity.1 < 0.0, "should rise, got {}", s.velocity.1);
        assert!(s.velocity.1 >= -STRIDER_RISE_CAP);
    }

    #[test]
    fn a_water_strider_shoves_itself_along() {
        let mut s = npc(626);
        let line = s.position.1 + s.height() + 4.0;
        let mut r = rng();
        let mut skated = false;
        for _ in 0..600 {
            water_strider(&mut s, Some(line), &mut r, true);
            if s.velocity.0.abs() > 1.0 {
                skated = true;
                break;
            }
        }
        assert!(skated, "it should push off eventually");
    }
}
