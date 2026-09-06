//! Styles 11 and 12 — Skeletron and its hands.
//!
//! The head alternates between two states on a fixed clock, and the whole fight is built around
//! that clock. For thirteen seconds it **hovers**, holding two hundred and fifty pixels above you
//! and drifting sideways to stay overhead — this is the safe half, and it is when the hands come
//! for you. Then for six and a half seconds it **spins**, dropping ten points of defence and
//! grinding straight at you. That defence drop is the whole window: the fight is a matter of not
//! dying to the hands during the hover so that you can hit the head during the spin.
//!
//! Daylight does not end this fight. The head simply becomes unkillable and lethal — nine thousand
//! damage, nine thousand defence — and comes at you at eight pixels a tick. The Dungeon Guardian is
//! the same routine permanently in that state, which is why it is not a boss so much as a warning.
//!
//! A **hand** docks beside the head, and every five seconds winds up: it climbs above the head,
//! aims once, and throws itself down at eighteen pixels a tick. It gives up the moment it has
//! passed you, turned, or gone too far, and drifts back to its dock.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    HAND_DOCK_HIGH, HAND_DOCK_HIGH_DRIVE, HAND_DOCK_LOW, HAND_DOCK_LOW_DRIVE, HAND_LUNGE,
    HAND_LUNGE_LIMIT, HAND_RISE, HAND_RISE_ABOVE, HAND_RISE_CAP, HAND_SWEEP, HAND_SWEEP_CAP,
    HAND_WINDUP_AT, SKELETRON_BARRAGE_DAMAGE, SKELETRON_BARRAGE_HANDS_THRESHOLD,
    SKELETRON_BARRAGE_HEALTH_AT, SKELETRON_BARRAGE_INTERVAL, SKELETRON_BARRAGE_INTERVAL_NO_HANDS,
    SKELETRON_BARRAGE_JITTER, SKELETRON_BARRAGE_SPEED, SKELETRON_BARRAGE_SPEED_NO_HANDS,
    SKELETRON_ENRAGED_SPEED, SKELETRON_ENRAGED_STAT, SKELETRON_EXPERT_HAND_DEFENSE,
    SKELETRON_GIVE_UP, SKELETRON_HAND, SKELETRON_HOVER, SKELETRON_HOVER_ABOVE,
    SKELETRON_HOVER_TICKS, SKELETRON_SPIN_DEFENSE, SKELETRON_SPIN_RATE, SKELETRON_SPIN_SPEED,
    SKELETRON_SPIN_SPEED_EXPERT, SKELETRON_SPIN_SPEED_EXPERT_NO_HANDS,
    SKELETRON_SPIN_SPEED_EXPERT_ONE_HAND, SKELETRON_SPIN_SPEED_EXPERT_RANGE, SKELETRON_SPIN_TICKS,
};
use terrustia_proto::projectile::ids::SKELETRON_BARRAGE;

use crate::game::ai::{Shot, World, can_see};
use crate::game::npc::{Npc, TileView};
use crate::game::npc_ai::Spawn;

/// The head's states, as `ai[1]` records them.
pub const HOVERING: f32 = 0.0;
pub const SPINNING: f32 = 1.0;
pub const ENRAGED: f32 = 2.0;
pub const LEAVING: f32 = 3.0;

/// What a part can see of whatever it is attached to.
///
/// A part has no view of the NPC table, so everything it needs about its parent has to arrive
/// here. Riding parts need more than a position: a saucer's guns sit at offsets *rotated by the
/// hull*, so they need its rotation and scale too, and they inherit its despawn timer so a hull
/// that leaves does not strand its guns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parent {
    pub position: (f32, f32),
    pub size: (f32, f32),
    pub rotation: f32,
    pub scale: f32,
    pub velocity: (f32, f32),
    pub direction: i8,
    pub sprite_direction: i8,
    pub time_left: i32,
    /// Which state the parent is in, from its own `ai[1]`.
    pub state: f32,
    /// The parent's own `ai[0]`.
    ///
    /// Skeletron and the Golem keep their phase in `ai[1]`, which is what [`Self::state`] carries,
    /// but the Moon Lord keeps its in `ai[0]` and uses `ai[1]` for the timer inside that phase. A
    /// part that wanted to know its core had started dying was reading `ai[1] == 2`, which is true
    /// for exactly one tick of the death drama's own counter: right answer, wrong reason, and it
    /// would have gone wrong the moment either number moved.
    pub phase: f32,
    /// How much of its health it has left, from zero to one.
    pub health: f32,
}

impl Parent {
    /// The middle of the parent, which is what offsets are measured from.
    pub fn center(&self) -> (f32, f32) {
        (
            self.position.0 + self.size.0 / 2.0,
            self.position.1 + self.size.1 / 2.0,
        )
    }
}

/// The Dungeon Guardian, which is Skeletron's routine with no off switch.
const DUNGEON_GUARDIAN: u16 = 68;

/// How hard a drift bleeds off velocity that is already pointing the wrong way. The head's hover
/// uses 0.98 (`NPC.cs:22169`); a hand's docking uses 0.96 (`NPC.cs:22432`), which the shared
/// helper here used to flatten to the head's value for both.
const HOVER_DAMPING: f32 = 0.98;
const DOCK_DAMPING: f32 = 0.96;

/// Drive one axis toward a wanted position, easing off whatever it was doing the other way.
fn drift(velocity: &mut f32, here: f32, wanted: f32, accel: f32, cap: f32, damping: f32) {
    if here > wanted {
        if *velocity > 0.0 {
            *velocity *= damping;
        }
        *velocity -= accel;
        if *velocity > cap {
            *velocity = cap;
        }
    } else if here < wanted {
        if *velocity < 0.0 {
            *velocity *= damping;
        }
        *velocity += accel;
        if *velocity < -cap {
            *velocity = -cap;
        }
    }
}

/// What the head produced this tick: the hands it wants raised, and any skull barrage it threw.
#[derive(Debug, Default)]
pub struct HeadOutcome {
    pub spawn: Vec<Spawn>,
    pub shots: Vec<Shot>,
}

/// Drive Skeletron's head for a tick, returning the hands it wants raised and the expert-mode
/// skull barrage it may have thrown.
pub fn head<T: TileView>(npc: &mut Npc, world: &World<'_, T>, rng: &mut SmallRng) -> HeadOutcome {
    let mut out = HeadOutcome::default();
    let guardian = npc.npc_type == DUNGEON_GUARDIAN;

    // First tick: a head raises two hands, one to each side, out of step with each other.
    if npc.ai[0] == 0.0 {
        npc.ai[0] = 1.0;
        if !guardian {
            for side in [-1.0, 1.0] {
                out.spawn.push(Spawn {
                    handle: None,
                    npc_type: SKELETRON_HAND,
                    position: (
                        npc.position.0 + npc.width() / 2.0,
                        npc.position.1 + npc.height() / 2.0,
                    ),
                    velocity: (side, 0.0),
                    parent: Some(Spawn::OWN_PARENT),
                    ai: [None; 4],
                });
            }
        }
        npc.dirty = true;
    }

    // Back to the type's armour, every tick, before this tick's modifiers go on
    // (`NPC.cs:22019`, `defense = defDefense;` as the first statement of the whole style).
    //
    // This wrote `npc.stats.defense`, which nothing reads: combat takes `npc.defense`
    // (`dispatch.rs`, "Live armour, not the type's"), and every other boss writes that. Worse, it
    // was a clamp rather than a reset, so the modifiers below accumulated permanently instead of
    // being rebuilt each tick and settled to zero after the first spin. All three of this fight's
    // armour mechanics were inert: the expert per-hand bonus, the spin's ten-point window (the
    // whole point of the fight), and the dawn enrage's 9999, which is what makes daylight a
    // fail-state rather than a nuisance.
    npc.defense = npc.stats.defense;

    // Expert mode: every living hand toughens the head, and once few enough are left (or it is
    // hurt enough) it starts throwing a skull barrage of its own (`NPC.cs:22059-22114`).
    let living_hands = if guardian {
        0
    } else {
        world.count(SKELETRON_HAND)
    };
    if world.conditions.expert {
        npc.defense += living_hands as i32 * SKELETRON_EXPERT_HAND_DEFENSE;
    }

    // Nobody within two thousand pixels on either axis, or nobody left alive.
    let abandoned = match world.target {
        None => true,
        Some(t) => {
            !t.alive
                || (npc.position.0 - t.center.0).abs() > SKELETRON_GIVE_UP
                || (npc.position.1 - t.center.1).abs() > SKELETRON_GIVE_UP
        }
    };
    if abandoned {
        npc.ai[1] = LEAVING;
    } else if (guardian || world.conditions.day) && npc.ai[1] != LEAVING && npc.ai[1] != ENRAGED {
        // Dawn does not save you here.
        npc.ai[1] = ENRAGED;
    }

    let Some(target) = world.target else {
        npc.velocity.1 += 0.1;
        npc.time_left = npc.time_left.min(50);
        return out;
    };
    let (cx, cy) = npc.center();
    let health = npc.life as f32 / npc.life_max.max(1) as f32;

    if npc.ai[1] == HOVERING {
        npc.ai[2] += 1.0;
        if world.conditions.expert
            && (living_hands < SKELETRON_BARRAGE_HANDS_THRESHOLD
                || health < SKELETRON_BARRAGE_HEALTH_AT)
            && can_see(world.tiles, npc, target)
        {
            let interval = if living_hands == 0 {
                SKELETRON_BARRAGE_INTERVAL_NO_HANDS
            } else {
                SKELETRON_BARRAGE_INTERVAL
            };
            if npc.ai[2] % interval == 0.0 {
                let speed = if living_hands == 0 {
                    SKELETRON_BARRAGE_SPEED_NO_HANDS
                } else {
                    SKELETRON_BARRAGE_SPEED
                };
                let dx = target.center.0 - cx + rng.random_range(-20..=20) as f32;
                let dy = target.center.1 - cy + rng.random_range(-20..=20) as f32;
                let reach = (dx * dx + dy * dy).sqrt().max(1.0);
                let mut aim = (dx * speed / reach, dy * speed / reach);
                aim.0 += rng.random_range(-SKELETRON_BARRAGE_JITTER..=SKELETRON_BARRAGE_JITTER)
                    as f32
                    * 0.01;
                aim.1 += rng.random_range(-SKELETRON_BARRAGE_JITTER..=SKELETRON_BARRAGE_JITTER)
                    as f32
                    * 0.01;
                aim.0 += npc.velocity.0;
                aim.1 += npc.velocity.1;
                out.shots.push(Shot {
                    projectile: SKELETRON_BARRAGE,
                    damage: SKELETRON_BARRAGE_DAMAGE,
                    position: (cx + aim.0 * 5.0, cy + aim.1 * 5.0),
                    velocity: aim,
                    time_left: 300,
                    ai: [0.0; 3],
                });
            }
        }
        if npc.ai[2] >= SKELETRON_HOVER_TICKS {
            npc.ai[2] = 0.0;
            npc.ai[1] = SPINNING;
            npc.dirty = true;
        }
        npc.rotation = npc.velocity.0 / 15.0;
        let (up_accel, up_cap, across_accel, across_cap) = SKELETRON_HOVER;
        drift(
            &mut npc.velocity.1,
            npc.position.1,
            target.center.1 - crate::game::ai::PLAYER_HEIGHT as f32 / 2.0 - SKELETRON_HOVER_ABOVE,
            up_accel,
            up_cap,
            HOVER_DAMPING,
        );
        drift(
            &mut npc.velocity.0,
            cx,
            target.center.0,
            across_accel,
            across_cap,
            HOVER_DAMPING,
        );
    } else if npc.ai[1] == SPINNING {
        // The window: ten points off its defence while it grinds at you (`NPC.cs:22264`).
        npc.defense -= SKELETRON_SPIN_DEFENSE;
        npc.ai[2] += 1.0;
        if npc.ai[2] >= SKELETRON_SPIN_TICKS {
            npc.ai[2] = 0.0;
            npc.ai[1] = HOVERING;
            npc.dirty = true;
        }
        npc.rotation += f32::from(npc.direction) * SKELETRON_SPIN_RATE;
        let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
        let reach = (dx * dx + dy * dy).sqrt().max(0.01);
        let mut spin_speed = SKELETRON_SPIN_SPEED;
        if world.conditions.expert {
            spin_speed = SKELETRON_SPIN_SPEED_EXPERT;
            // SKEL-M5: the first threshold is 1.05 and the other nine are 1.1
            // (`NPC.cs:22292-22332`). Applying 1.1 to all ten made the charge about five percent
            // too fast at short range.
            for (threshold, factor) in SKELETRON_SPIN_SPEED_EXPERT_RANGE {
                if reach > threshold {
                    spin_speed *= factor;
                }
            }
            // And the term that was missing entirely: the fewer hands are left, the faster it
            // comes at you (`NPC.cs:22333-22342`). Without it the fight lost its escalation, and
            // a handless Skeletron charged at the same speed as a whole one.
            spin_speed *= match living_hands {
                0 => SKELETRON_SPIN_SPEED_EXPERT_NO_HANDS,
                1 => SKELETRON_SPIN_SPEED_EXPERT_ONE_HAND,
                _ => 1.0,
            };
        }
        let k = spin_speed / reach;
        npc.velocity = (dx * k, dy * k);
    } else if npc.ai[1] == ENRAGED {
        // Untouchable and fatal (`NPC.cs:22357-22358`, `damage = 9999; defense = 9999;`). Both are
        // the live numbers, so both are written where combat reads them: `defense` outright, and
        // the damage as the multiplier over the type's own that `Npc::contact_damage` applies.
        // Written to `stats` instead, the defence went nowhere at all and a ranged player killed
        // the enraged boss in seconds, which is to say daylight was not a fail-state.
        npc.damage_bonus = SKELETRON_ENRAGED_STAT as f32 / npc.stats.damage.max(1) as f32;
        npc.defense = SKELETRON_ENRAGED_STAT;
        npc.rotation += f32::from(npc.direction) * SKELETRON_SPIN_RATE;
        let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
        let reach = (dx * dx + dy * dy).sqrt().max(0.01);
        let k = SKELETRON_ENRAGED_SPEED / reach;
        npc.velocity = (dx * k, dy * k);
    } else {
        npc.velocity.1 += 0.1;
        if npc.velocity.1 < 0.0 {
            npc.velocity.1 *= 0.95;
        }
        npc.velocity.0 *= 0.95;
        npc.time_left = npc.time_left.min(50);
    }

    npc.dirty = true;
    out
}

/// What a hand's tick concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandOutcome {
    Attached,
    /// Its head is gone, so it is too.
    Orphaned,
}

/// Drive one of Skeletron's hands for a tick.
///
/// `head_at` is the head's position and size, and `head_hovering` says whether the head is in the
/// half of its cycle during which the hands come at you.
pub fn hand(
    npc: &mut Npc,
    head_at: Option<Parent>,
    head_hovering: bool,
    head_leaving: bool,
    target: Option<crate::game::npc_ai::Target>,
    expert: bool,
) -> HandOutcome {
    let Some(Parent {
        position: head_position,
        size: head_size,
        ..
    }) = head_at
    else {
        return HandOutcome::Orphaned;
    };
    // `ai[0]` is which side of the head it belongs to.
    let side = if npc.ai[0] < 0.0 { -1.0 } else { 1.0 };
    npc.sprite_direction = -(side as i8);
    if head_leaving {
        npc.time_left = npc.time_left.min(10);
    }

    let dock = |offset: (f32, f32)| {
        (
            head_position.0 + head_size.0 / 2.0 - offset.0 * side,
            head_position.1 + offset.1,
        )
    };
    let (cx, cy) = (
        npc.position.0 + npc.width() * 0.5,
        npc.position.1 + npc.height() * 0.5,
    );
    let half_width = npc.width() / 2.0;

    match npc.ai[2] as i32 {
        // Docked, either hanging low beneath the head or close in beside it.
        0 | 3 => {
            if head_hovering {
                // The head is in its safe half, which is exactly when the hands come for you:
                // the low dock is where a hand winds up before it lunges (`NPC.cs:22428-22478`).
                let at = dock(HAND_DOCK_LOW);
                let (uy, cyv, ux, cxv) = HAND_DOCK_LOW_DRIVE;
                // SKEL-M6: expert runs this whole steering block twice per tick, once inside
                // `if (Main.expertMode)` (`NPC.cs:22496-22546`) and again unconditionally
                // (`NPC.cs:22547-22592`) with identical numbers. Nothing moves between the two,
                // so a second pass over the same target is exactly what vanilla does: the hand
                // reaches its dock at twice the acceleration.
                for _ in 0..if expert { 2 } else { 1 } {
                    drift(
                        &mut npc.velocity.1,
                        npc.position.1,
                        at.1,
                        uy,
                        cyv,
                        DOCK_DAMPING,
                    );
                    let here = npc.position.0 + half_width;
                    drift(&mut npc.velocity.0, here, at.0, ux, cxv, DOCK_DAMPING);
                }
                // Only from the low dock does it wind itself up, and expert winds it up half
                // again as fast: 200 ticks to the lunge, not 300 (`NPC.cs:22479-22489`).
                npc.ai[3] += 1.0;
                if expert {
                    npc.ai[3] += 0.5;
                }
                if npc.ai[3] >= HAND_WINDUP_AT {
                    npc.ai[2] += 1.0;
                    npc.ai[3] = 0.0;
                    npc.dirty = true;
                }
            } else {
                // The head is spinning — the vulnerable half — and the hand retreats close in
                // beside it instead of attacking.
                let at = dock(HAND_DOCK_HIGH);
                let (uy, cyv, ux, cxv) = HAND_DOCK_HIGH_DRIVE;
                drift(
                    &mut npc.velocity.1,
                    npc.position.1,
                    at.1,
                    uy,
                    cyv,
                    DOCK_DAMPING,
                );
                let here = npc.position.0 + half_width;
                drift(&mut npc.velocity.0, here, at.0, ux, cxv, DOCK_DAMPING);
            }
            let at = dock(HAND_DOCK_LOW);
            npc.rotation = (at.1 - cy).atan2(at.0 - cx) + 1.57;
        }
        // Winding up: it climbs above the head and then commits.
        1 => {
            npc.velocity.0 *= 0.95;
            npc.velocity.1 -= HAND_RISE;
            if npc.velocity.1 < -HAND_RISE_CAP {
                npc.velocity.1 = -HAND_RISE_CAP;
            }
            if npc.position.1 < head_position.1 - HAND_RISE_ABOVE
                && let Some(t) = target
            {
                let (dx, dy) = (t.center.0 - cx, t.center.1 - cy);
                let reach = (dx * dx + dy * dy).sqrt().max(0.01);
                let k = HAND_LUNGE / reach;
                npc.velocity = (dx * k, dy * k);
                npc.ai[2] = 2.0;
                npc.dirty = true;
            }
        }
        // The lunge, which is over the moment it has passed you or turned.
        2 => {
            let done = match target {
                None => true,
                Some(t) => {
                    let toward = (t.center.0 - cx, t.center.1 - cy);
                    npc.position.1 > t.center.1
                        || npc.velocity.0 * toward.0 + npc.velocity.1 * toward.1 <= 0.0
                        || (toward.0 * toward.0 + toward.1 * toward.1).sqrt() > HAND_LUNGE_LIMIT
                        || npc.velocity.1 < 0.0
                }
            };
            if done {
                npc.ai[2] = 3.0;
                npc.dirty = true;
            }
        }
        // The sideways sweep, and its end.
        4 => {
            npc.velocity.1 *= 0.95;
            npc.velocity.0 += HAND_SWEEP * -side;
            npc.velocity.0 = npc.velocity.0.clamp(-HAND_SWEEP_CAP, HAND_SWEEP_CAP);
            if let Some(t) = target
                && ((npc.velocity.0 > 0.0 && cx > t.center.0)
                    || (npc.velocity.0 < 0.0 && cx < t.center.0))
            {
                npc.ai[2] = 5.0;
                npc.dirty = true;
            }
        }
        _ => {
            npc.ai[2] = 0.0;
            npc.dirty = true;
        }
    }

    npc.dirty = true;
    HandOutcome::Attached
}

#[cfg(test)]
mod tests {

    /// A parent standing still at a given place, which is all these tests need of one.
    fn parent_at(position: (f32, f32), size: (f32, f32)) -> Parent {
        Parent {
            position,
            size,
            rotation: 0.0,
            scale: 1.0,
            velocity: (0.0, 0.0),
            direction: 1,
            sprite_direction: 1,
            time_left: 3600,
            state: HOVERING,
            phase: 0.0,
            health: 1.0,
        }
    }
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use terrustia_proto::tile::Tile;

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(1)
    }

    struct Dungeon;

    impl TileView for Dungeon {
        fn tile(&self, _x: i32, _y: i32) -> Tile {
            Tile::AIR
        }
    }

    fn world<'a>(tiles: &'a Dungeon, target: Option<Target>) -> World<'a, Dungeon> {
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

    fn skeletron() -> Npc {
        Npc::new(35, (10_000.0, 9_600.0), 1).expect("skeletron")
    }

    #[test]
    fn it_raises_two_hands_on_its_first_tick_and_no_more() {
        let tiles = Dungeon;
        let mut s = skeletron();
        let mut r = rng();
        let t = Some(player_at(10_000.0, 10_000.0));
        let raised = head(&mut s, &world(&tiles, t), &mut r).spawn;
        assert_eq!(raised.len(), 2);
        assert!(raised.iter().all(|h| h.npc_type == SKELETRON_HAND));
        assert!(
            raised[0].velocity.0 != raised[1].velocity.0,
            "one to each side"
        );
        assert!(head(&mut s, &world(&tiles, t), &mut r).spawn.is_empty());
    }

    #[test]
    fn the_dungeon_guardian_has_no_hands_and_is_always_enraged() {
        let tiles = Dungeon;
        let mut g = Npc::new(68, (10_000.0, 9_600.0), 1).expect("dungeon guardian");
        let mut r = rng();
        let t = Some(player_at(10_000.0, 10_000.0));
        let raised = head(&mut g, &world(&tiles, t), &mut r).spawn;
        assert!(raised.is_empty());
        assert_eq!(g.ai[1], ENRAGED);
        assert_eq!(g.defense, SKELETRON_ENRAGED_STAT);
    }

    #[test]
    fn it_hovers_above_you_then_spins_at_you() {
        let tiles = Dungeon;
        let mut s = skeletron();
        let mut r = rng();
        let t = Some(player_at(10_000.0, 10_000.0));
        for _ in 0..(SKELETRON_HOVER_TICKS as i32 + 1) {
            head(&mut s, &world(&tiles, t), &mut r);
        }
        assert_eq!(s.ai[1], SPINNING, "should have started spinning");
        let before = s.rotation;
        head(&mut s, &world(&tiles, t), &mut r);
        assert!(s.rotation != before, "and be turning");

        for _ in 0..(SKELETRON_SPIN_TICKS as i32 + 1) {
            head(&mut s, &world(&tiles, t), &mut r);
        }
        assert_eq!(s.ai[1], HOVERING, "and settle again");
    }

    /// The whole fight in one number: the spin is when the head can actually be hurt.
    ///
    /// It has to be the *live* `defense`, which is what combat reads (`dispatch.rs`, "Live armour,
    /// not the type's"). This asserted `stats.defense`, which nothing reads, and the routine wrote
    /// only that, so the window this test is named after did not exist. It was also a permanent
    /// subtraction rather than one rebuilt each tick from the type's armour: after four hundred
    /// spin ticks the head sat at zero armour for the rest of the fight and never went back up.
    #[test]
    fn spinning_is_when_its_guard_drops() {
        let tiles = Dungeon;
        let mut s = skeletron();
        let mut r = rng();
        let t = Some(player_at(10_000.0, 10_000.0));
        head(&mut s, &world(&tiles, t), &mut r);
        let guarded = s.defense;
        assert_eq!(guarded, s.stats.defense, "hovering, it is the type's own");
        s.ai[1] = SPINNING;
        head(&mut s, &world(&tiles, t), &mut r);
        assert_eq!(s.defense, guarded - SKELETRON_SPIN_DEFENSE);

        // A second spin tick takes the same ten off the same base, not another ten off the last
        // answer: `NPC.cs:22019` resets to `defDefense` before every tick's modifiers.
        head(&mut s, &world(&tiles, t), &mut r);
        assert_eq!(s.defense, guarded - SKELETRON_SPIN_DEFENSE);

        // And back up again when it stops.
        s.ai[1] = HOVERING;
        head(&mut s, &world(&tiles, t), &mut r);
        assert_eq!(s.defense, guarded, "the guard comes back");
    }

    #[test]
    fn daylight_makes_it_lethal_rather_than_ending_it() {
        let tiles = Dungeon;
        let mut s = skeletron();
        let mut r = rng();
        let t = Some(player_at(10_000.0, 10_000.0));
        let mut day = world(&tiles, t);
        day.conditions.day = true;
        head(&mut s, &day, &mut r);
        head(&mut s, &day, &mut r);
        assert_eq!(s.ai[1], ENRAGED);
        assert_eq!(
            s.contact_damage(),
            SKELETRON_ENRAGED_STAT,
            "touching it at dawn is lethal (`NPC.cs:22357`)"
        );
        // And the other half of `NPC.cs:22357-22358`, which was written to a field nothing reads:
        // 9999 armour, so `damage_taken`'s floor of one means about four and a half thousand hits.
        // Without it the enraged boss kept its table armour of ten and a ranged player killed it in
        // seconds, so the dawn fail-state did not fail.
        assert_eq!(s.defense, SKELETRON_ENRAGED_STAT);
        assert!(s.time_left > 50, "it does not leave, it kills you");
    }

    #[test]
    fn a_player_who_runs_far_enough_ends_it() {
        let tiles = Dungeon;
        let mut s = skeletron();
        let mut r = rng();
        let t = Some(player_at(10_000.0 + SKELETRON_GIVE_UP + 100.0, 10_000.0));
        head(&mut s, &world(&tiles, t), &mut r);
        head(&mut s, &world(&tiles, t), &mut r);
        assert_eq!(s.ai[1], LEAVING);
        assert!(s.time_left <= 50);
    }

    /// SKEL-M5: two errors in one expert table.
    ///
    /// Vanilla opens the range ladder at 1.05 and only then repeats at 1.1
    /// (`NPC.cs:22292-22332`), and it then multiplies by a hand-count term that was missing here
    /// altogether (`switch (num173)`, `NPC.cs:22333-22342`). With 1.1 at all ten steps and no hand
    /// term, a two-handed Skeletron charged about five percent too fast at short range and a
    /// handless one about five percent too slow - and the fight lost the escalation it is built
    /// around, where breaking a hand makes the head come at you harder.
    #[test]
    fn breaking_its_hands_makes_it_charge_harder() {
        let tiles = Dungeon;
        let charge = |hands: usize, reach: f32| {
            let census = [(SKELETRON_HAND, hands)];
            let mut s = skeletron();
            let mut r = rng();
            let (cx, cy) = s.center();
            let mut w = world(&tiles, Some(player_at(cx + reach, cy)));
            w.conditions.expert = true;
            w.census = &census;
            // Already spinning, which is the charge this table paces.
            s.ai[0] = 1.0;
            s.ai[1] = SPINNING;
            head(&mut s, &w, &mut r);
            s.velocity.0.hypot(s.velocity.1)
        };
        // Inside the first range threshold, so this is the hand term on its own.
        let close = 140.0;
        let base = SKELETRON_SPIN_SPEED_EXPERT;
        for (hands, want) in [
            (2, base),
            (1, base * SKELETRON_SPIN_SPEED_EXPERT_ONE_HAND),
            (0, base * SKELETRON_SPIN_SPEED_EXPERT_NO_HANDS),
        ] {
            let got = charge(hands, close);
            assert!(
                (got - want).abs() < 1e-3,
                "with {hands} hands it should charge at {want}, got {got}"
            );
        }
        // And past a hundred and fifty pixels it picks up 1.05, not another 1.1.
        let got = charge(2, 180.0);
        assert!(
            (got - base * 1.05).abs() < 1e-3,
            "the first range step is 1.05, got {got}"
        );
    }

    fn a_hand() -> Npc {
        let mut n = Npc::new(36, (10_000.0, 9_800.0), 1).expect("skeletron hand");
        n.ai[0] = 1.0;
        n
    }

    /// SKEL-M6: expert winds a hand up half again as fast (`ai[3]++; ... if (Main.expertMode)
    /// ai[3] += 0.5f;`, `NPC.cs:22479-22489`), so it lets go after two hundred ticks rather than
    /// three hundred. The counter advanced flat, so expert hands attacked at classic pace.
    #[test]
    fn expert_hands_wind_up_half_again_as_fast() {
        let head_at = Some(parent_at((10_000.0, 9_600.0), (100.0, 100.0)));
        let t = Some(player_at(10_000.0, 10_400.0));
        let let_go_by = |expert: bool, ticks: i32| {
            let mut h = a_hand();
            for _ in 0..ticks {
                hand(&mut h, head_at, true, false, t, expert);
            }
            h.ai[2] != 0.0
        };
        assert!(!let_go_by(false, 200), "classic is still winding up at 200");
        assert!(let_go_by(true, 200), "expert has let go by 200");
        assert!(let_go_by(false, 300), "and classic lets go at 300");
    }

    /// SKEL-M6: expert runs the whole low-dock steering block twice a tick, once inside `if
    /// (Main.expertMode)` (`NPC.cs:22496-22546`) and again unconditionally
    /// (`NPC.cs:22547-22592`) with identical numbers, so a hand closes on its dock at twice the
    /// acceleration. We ran it once.
    #[test]
    fn expert_hands_dock_at_twice_the_rate() {
        let head_at = Some(parent_at((10_000.0, 9_600.0), (100.0, 100.0)));
        let after_a_tick = |expert: bool| {
            let mut h = a_hand();
            hand(&mut h, head_at, true, false, None, expert);
            h.velocity
        };
        let classic = after_a_tick(false);
        let expert = after_a_tick(true);
        assert!(
            classic.0 != 0.0 && classic.1 != 0.0,
            "it should be steering on both axes at all, got {classic:?}"
        );
        assert!(
            (expert.0 - classic.0 * 2.0).abs() < 1e-6 && (expert.1 - classic.1 * 2.0).abs() < 1e-6,
            "expert should be twice as far along: {expert:?} against {classic:?}"
        );
    }

    /// A hand bleeds off a wrong-way drift at 0.96 (`NPC.cs:22432`), not the head's own 0.98
    /// (`NPC.cs:22169`). One shared helper had flattened both to the head's figure.
    #[test]
    fn a_hand_damps_a_wrong_way_drift_at_its_own_rate() {
        let head_at = Some(parent_at((10_000.0, 9_600.0), (100.0, 100.0)));
        let mut h = a_hand();
        // Below its low dock and still falling, which is the case the damping is for.
        h.position.1 = 10_400.0;
        h.velocity.1 = 1.0;
        hand(&mut h, head_at, true, false, None, false);
        let (up_accel, ..) = HAND_DOCK_LOW_DRIVE;
        // Vanilla's own figure, spelled out rather than read back off the constant under test.
        let want = 1.0 * 0.96 - up_accel;
        assert!(
            (h.velocity.1 - want).abs() < 1e-6,
            "should have damped to {want}, got {}",
            h.velocity.1
        );
    }

    #[test]
    fn a_hand_without_a_head_is_finished() {
        let mut h = a_hand();
        assert_eq!(
            hand(&mut h, None, true, false, None, false),
            HandOutcome::Orphaned
        );
    }

    #[test]
    fn a_hand_winds_up_from_its_low_dock_and_lunges() {
        let mut h = a_hand();
        let head_at = Some(parent_at((10_000.0, 9_600.0), (100.0, 100.0)));
        let t = Some(player_at(10_000.0, 10_400.0));
        // Docked low, because the head is hovering — the safe half, and the one the hands
        // attack from (`NPC.cs:22422-22478`).
        for _ in 0..(HAND_WINDUP_AT as i32 + 1) {
            hand(&mut h, head_at, true, false, t, false);
        }
        assert_eq!(h.ai[2], 1.0, "should be winding up");

        // It climbs, and once above the head it commits.
        for _ in 0..400 {
            hand(&mut h, head_at, true, false, t, false);
            h.position.1 += h.velocity.1;
            if h.ai[2] == 2.0 {
                break;
            }
        }
        assert_eq!(h.ai[2], 2.0, "should have let go");
        let speed = h.velocity.0.hypot(h.velocity.1);
        assert!((speed - HAND_LUNGE).abs() < 1e-3, "at speed, got {speed}");
        assert!(h.velocity.1 > 0.0, "and downward at the player");
    }

    /// B5: hands only wind up and lunge while the head is hovering (the safe half); while the
    /// head is spinning (the vulnerable half) they dock close instead. The rhythm used to be
    /// inverted — the hands attacked exactly when you could hurt the head.
    #[test]
    fn a_hand_never_winds_up_while_the_head_spins() {
        let mut h = a_hand();
        let head_at = Some(parent_at((10_000.0, 9_600.0), (100.0, 100.0)));
        let t = Some(player_at(10_000.0, 10_400.0));
        for _ in 0..2000 {
            hand(&mut h, head_at, false, false, t, false);
        }
        assert_eq!(
            h.ai[2], 0.0,
            "it should stay docked close while the head spins, never winding up"
        );
    }

    #[test]
    fn a_hand_gives_up_its_lunge_once_it_has_passed_you() {
        let mut h = a_hand();
        let head_at = Some(parent_at((10_000.0, 9_600.0), (100.0, 100.0)));
        h.ai[2] = 2.0;
        h.velocity = (0.0, 18.0);
        // Already below the player.
        h.position.1 = 11_000.0;
        hand(
            &mut h,
            head_at,
            false,
            false,
            Some(player_at(10_000.0, 10_400.0)),
            false,
        );
        assert_eq!(h.ai[2], 3.0, "the lunge is over");
    }

    /// B5: the hand hangs at the low, far dock while the head hovers (winding up to attack), and
    /// docks close, near the high dock, while the head spins (retreating from the vulnerable
    /// half).
    #[test]
    fn a_hand_docks_low_while_the_head_hovers_and_close_while_it_spins() {
        assert!(
            HAND_DOCK_HIGH.1 < HAND_DOCK_LOW.1,
            "the close dock is the higher one"
        );
        let mut hovering = a_hand();
        let mut spinning = a_hand();
        let head_at = Some(parent_at((10_000.0, 9_600.0), (100.0, 100.0)));
        for _ in 0..200 {
            hand(&mut hovering, head_at, true, false, None, false);
            hovering.position.1 += hovering.velocity.1;
            hand(&mut spinning, head_at, false, false, None, false);
            spinning.position.1 += spinning.velocity.1;
        }
        assert!(
            hovering.position.1 > spinning.position.1,
            "the hand should hang low while the head hovers, and dock close while it spins: {} against {}",
            hovering.position.1,
            spinning.position.1
        );
    }

    /// B6: Expert mode toughens the head by 25 defence per living hand.
    #[test]
    fn expert_mode_toughens_the_head_per_living_hand() {
        let tiles = Dungeon;
        let mut r = rng();
        let t = Some(player_at(10_000.0, 10_000.0));

        let mut classic = skeletron();
        head(&mut classic, &world(&tiles, t), &mut r);
        let classic_defense = classic.defense;

        let mut expert = skeletron();
        let hands = [(SKELETRON_HAND, 2usize)];
        let mut w = world(&tiles, t);
        w.conditions.expert = true;
        w.census = &hands;
        head(&mut expert, &w, &mut r);

        assert_eq!(
            expert.defense,
            classic_defense + 2 * SKELETRON_EXPERT_HAND_DEFENSE,
            "two living hands should add fifty defence"
        );
    }

    /// B6: once expert mode has few hands left (or the head is hurt), it throws a skull barrage
    /// while it hovers — something the head never did at all before.
    #[test]
    fn expert_mode_throws_a_skull_barrage_while_hovering_with_few_hands() {
        let tiles = Dungeon;
        let mut r = rng();
        let mut s = skeletron();
        s.ai[1] = HOVERING;
        let t = Some(player_at(10_050.0, 9_600.0));
        let hands = [(SKELETRON_HAND, 1usize)];
        let mut w = world(&tiles, t);
        w.conditions.expert = true;
        w.census = &hands;

        let mut shots = 0;
        for _ in 0..(SKELETRON_HOVER_TICKS as i32) {
            shots += head(&mut s, &w, &mut r).shots.len();
        }
        assert!(shots > 0, "it should have thrown a skull barrage");
        assert!(
            shots <= (SKELETRON_HOVER_TICKS / SKELETRON_BARRAGE_INTERVAL) as usize + 1,
            "but not on every tick, got {shots}"
        );
    }

    /// B6: classic mode never throws the barrage, no matter how few hands are left.
    #[test]
    fn classic_mode_never_throws_a_skull_barrage() {
        let tiles = Dungeon;
        let mut r = rng();
        let mut s = skeletron();
        s.ai[1] = HOVERING;
        let t = Some(player_at(10_050.0, 9_600.0));
        let hands = [(SKELETRON_HAND, 0usize)];
        let mut w = world(&tiles, t);
        w.census = &hands;

        let mut shots = 0;
        for _ in 0..(SKELETRON_HOVER_TICKS as i32) {
            shots += head(&mut s, &w, &mut r).shots.len();
        }
        assert_eq!(shots, 0, "classic mode should never throw it");
    }

    /// B6: expert mode spins faster than classic, and faster still at range — not the flat speed
    /// the head used regardless of difficulty.
    #[test]
    fn expert_mode_spins_faster_and_faster_still_at_range() {
        let tiles = Dungeon;
        let speed_at = |expert: bool, player_x: f32| {
            let mut r = rng();
            let mut s = skeletron();
            s.ai[1] = SPINNING;
            let t = Some(player_at(player_x, 9_600.0));
            let mut w = world(&tiles, t);
            w.conditions.expert = expert;
            head(&mut s, &w, &mut r);
            s.velocity.0.hypot(s.velocity.1)
        };

        let classic_near = speed_at(false, 10_010.0);
        let classic_far = speed_at(false, 10_900.0);
        assert!(
            (classic_near - SKELETRON_SPIN_SPEED).abs() < 0.01,
            "classic should be flat at {SKELETRON_SPIN_SPEED}, got {classic_near}"
        );
        assert!(
            (classic_far - SKELETRON_SPIN_SPEED).abs() < 0.01,
            "classic should not ramp with range, got {classic_far}"
        );

        let expert_near = speed_at(true, 10_010.0);
        let expert_far = speed_at(true, 10_900.0);
        assert!(
            expert_near > classic_near,
            "expert should charge faster than classic even up close: {expert_near} vs {classic_near}"
        );
        assert!(
            expert_far > expert_near * 1.5,
            "expert should charge much faster at range: {expert_far} vs {expert_near}"
        );
    }
}
