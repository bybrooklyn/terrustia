//! Skeletron Prime and its four arms: styles 32 and 33–36.
//!
//! The head alternates between two states on a fixed clock and never varies it: ten seconds
//! hovering a few hundred pixels above you, then six and two thirds spinning — during which it
//! hits twice as hard and takes half as much, so the spin is a window where the *arms* are the
//! target rather than the head.
//!
//! Each arm hangs at a fixed station off the head and does nothing while the head hovers. When the
//! head spins, the arms come off their stations and go for you: the saw and the vice by touch, the
//! cannon by lobbing bombs *backwards* along its aim so they arc, and the laser by firing straight.
//! An arm dragged more than eight hundred pixels from its station gives up on you entirely and
//! flies back, which is why kiting the arms away does not work for long.
//!
//! Daylight does not end this fight the way it ends the Twins'. Prime becomes unkillable and runs
//! you down instead.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    PRIME_ABOVE_MAX, PRIME_ABOVE_MIN, PRIME_ARM_HEAD_START, PRIME_CANNON, PRIME_DRIFT,
    PRIME_DRIFT_CAP, PRIME_DRIFT_CAP_EXPERT, PRIME_DRIFT_EXPERT, PRIME_ENRAGED_GAIN,
    PRIME_ENRAGED_MAX, PRIME_ENRAGED_MIN, PRIME_ENRAGED_SPEED, PRIME_ENRAGED_STAT,
    PRIME_HOVER_TICKS, PRIME_LASER, PRIME_LEAVE_SINK, PRIME_LIFT, PRIME_LIFT_CAP,
    PRIME_LIFT_CAP_EXPERT, PRIME_LIFT_EXPERT, PRIME_LIMB_FOUND, PRIME_LIMB_LOST,
    PRIME_LIMB_RETURN_X, PRIME_LIMB_RETURN_X_CAP, PRIME_LIMB_RETURN_Y, PRIME_LIMB_RETURN_Y_CAP,
    PRIME_LOSE_RANGE, PRIME_SAW, PRIME_SHOT_SPREAD_STEPS, PRIME_SLACK, PRIME_SPIN_DAMAGE,
    PRIME_SPIN_DEFENSE, PRIME_SPIN_RANGE_FROM, PRIME_SPIN_RANGE_GAIN, PRIME_SPIN_RANGE_GAIN_FIRST,
    PRIME_SPIN_RANGE_STEP, PRIME_SPIN_SPEED, PRIME_SPIN_SPEED_EXPERT, PRIME_SPIN_TICKS, PRIME_VICE,
    PrimeLimb, prime_limb,
};

use super::skeletron::Parent;
use crate::game::ai::{Shot, World};
use crate::game::npc::{Npc, TileView};
use crate::game::npc_ai::Spawn;

/// The head's states, from `ai[1]`.
pub mod head_state {
    pub const HOVERING: f32 = 0.0;
    pub const SPINNING: f32 = 1.0;
    pub const ENRAGED: f32 = 2.0;
    pub const LEAVING: f32 = 3.0;
}

/// What a piece of Prime did this tick.
#[derive(Debug, Default)]
pub struct PrimeOutcome {
    pub shots: Vec<Shot>,
    /// Set when the head has given up and the whole assembly should go.
    pub leaving: bool,
    /// Set when this arm has lost its head and is coming apart.
    pub spent: bool,
    /// The arms a fresh head raises on its own first tick — see [`prime_head`].
    pub spawn: Vec<Spawn>,
}

/// Style 32: the head.
pub fn prime_head(npc: &mut Npc, world: &World<'_, impl TileView>) -> PrimeOutcome {
    let mut out = PrimeOutcome::default();
    npc.dirty = true;
    npc.damage_bonus = 1.0;
    npc.defense = npc.stats.defense;
    npc.invulnerable = false;

    // First tick: a head raises its four arms (`NPC.cs:27806-27832`, `aiStyle==32`'s own
    // `ai[0]==0` gate) — the same structural situation `skeletron::head` already handles for
    // Skeletron's two hands, and the same `Spawn`/`OWN_PARENT` mechanism reused rather than
    // reinvented. This was missing entirely until now: nothing anywhere else in this project ever
    // created Prime's arms, so the admin `/spawn` command — the only way to encounter this boss at
    // all today, since no in-game summon trigger exists yet either — produced a bare head with no
    // weapons, unable to ever come apart or be fought as the real four-part boss it is. `ai[0]` is
    // otherwise unused by the head (unlike an arm's own `ai[0]`, its station side), so it is safe
    // to repurpose as this one-shot flag, matching real vanilla's own use of the field here.
    if npc.ai[0] == 0.0 {
        npc.ai[0] = 1.0;
        let at = (
            npc.position.0 + npc.width() / 2.0,
            npc.position.1 + npc.height() / 2.0,
        );
        // Side matches vanilla exactly (`NewNPC(..., 128/129/130/131, ...)`, `ai[0]` per call);
        // the consumer that fulfils `Spawn` requests reads a parented spawn's side from the sign
        // of `velocity.0`, the same encoding `skeletron::head`'s own hands already use.
        for (limb, side, head_start) in [
            (PRIME_CANNON, -1.0, None),
            (PRIME_SAW, 1.0, None),
            // The Vice and the Laser start their attack timer a hundred and fifty ticks in
            // (`NPC.cs:27824`, `:27831`, `Main.npc[num508].ai[3] = 150f`). Without it all four arms
            // wind up and switch together, which turns a boss of four independent limbs into one
            // limb played four times.
            (PRIME_VICE, -1.0, Some(PRIME_ARM_HEAD_START)),
            (PRIME_LASER, 1.0, Some(PRIME_ARM_HEAD_START)),
        ] {
            out.spawn.push(Spawn {
                handle: None,
                npc_type: limb,
                position: at,
                velocity: (side, 0.0),
                parent: Some(Spawn::OWN_PARENT),
                ai: [None, None, None, head_start],
            });
        }
    }

    let target = world.target.filter(|t| {
        t.alive
            && (npc.position.0 - t.center.0).abs() < PRIME_LOSE_RANGE
            && (npc.position.1 - t.center.1).abs() < PRIME_LOSE_RANGE
    });
    if target.is_none() {
        npc.ai[1] = head_state::LEAVING;
    } else if world.conditions.day
        && npc.ai[1] != head_state::LEAVING
        && npc.ai[1] != head_state::ENRAGED
    {
        // Daylight does not send Prime home. It makes it worse.
        npc.ai[1] = head_state::ENRAGED;
    }

    if npc.ai[1] == head_state::LEAVING {
        out.leaving = true;
        npc.velocity.1 += PRIME_LEAVE_SINK;
        if npc.velocity.1 < 0.0 {
            npc.velocity.1 *= 0.95;
        }
        npc.velocity.0 *= 0.95;
        // No terminal speed on the way down. Vanilla's `velocity.Y > 13f` clamp
        // (`NPC.cs:28100-28103`) lives inside the `IsMechQueenUp` half of this branch, which is the
        // Mechdusa fight this server does not model; the ordinary one (`NPC.cs:28105-28113`) simply
        // accelerates away. Capping it here held a departing Prime on screen far longer than it
        // should have been.
        return out;
    }
    let Some(target) = target else { return out };
    let (cx, cy) = npc.center();
    let expert = world.conditions.expert;

    match npc.ai[1] {
        s if s == head_state::HOVERING => {
            npc.ai[2] += 1.0;
            if npc.ai[2] >= PRIME_HOVER_TICKS {
                npc.ai[2] = 0.0;
                npc.ai[1] = head_state::SPINNING;
                return out;
            }
            npc.rotation = npc.velocity.0 / 15.0;

            let (lift, lift_cap, drift, drift_cap) = if expert {
                (
                    PRIME_LIFT_EXPERT,
                    PRIME_LIFT_CAP_EXPERT,
                    PRIME_DRIFT_EXPERT,
                    PRIME_DRIFT_CAP_EXPERT,
                )
            } else {
                (PRIME_LIFT, PRIME_LIFT_CAP, PRIME_DRIFT, PRIME_DRIFT_CAP)
            };

            // Hold a band above the player rather than a single height.
            if npc.position.1 > target.center.1 - PRIME_ABOVE_MIN {
                if npc.velocity.1 > 0.0 {
                    npc.velocity.1 *= 0.98;
                }
                npc.velocity.1 = (npc.velocity.1 - lift).min(lift_cap);
            } else if npc.position.1 < target.center.1 - PRIME_ABOVE_MAX {
                if npc.velocity.1 < 0.0 {
                    npc.velocity.1 *= 0.98;
                }
                npc.velocity.1 = (npc.velocity.1 + lift).max(-lift_cap);
            }
            // ...and a hundred pixels of sideways slack, so it does not jitter overhead.
            if cx > target.center.0 + PRIME_SLACK {
                if npc.velocity.0 > 0.0 {
                    npc.velocity.0 *= 0.98;
                }
                npc.velocity.0 = (npc.velocity.0 - drift).min(drift_cap);
            }
            if cx < target.center.0 - PRIME_SLACK {
                if npc.velocity.0 < 0.0 {
                    npc.velocity.0 *= 0.98;
                }
                npc.velocity.0 = (npc.velocity.0 + drift).max(-drift_cap);
            }
        }

        s if s == head_state::SPINNING => {
            // The spin. Twice the damage, half the incoming, and it comes straight at you.
            npc.defense = npc.stats.defense * PRIME_SPIN_DEFENSE;
            npc.damage_bonus = PRIME_SPIN_DAMAGE as f32;
            npc.ai[2] += 1.0;
            if npc.ai[2] >= PRIME_SPIN_TICKS {
                npc.ai[2] = 0.0;
                npc.ai[1] = head_state::HOVERING;
            }
            npc.rotation += f32::from(npc.direction) * 0.3;

            let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
            let reach = dx.hypot(dy).max(1.0);
            let mut speed = if expert {
                let mut s = PRIME_SPIN_SPEED_EXPERT;
                // In expert the spin closes faster the further off you are, in fifty-pixel steps,
                // so backing away from a spinning Prime does not help. The *first* step is a
                // gentler one: `NPC.cs:27970` multiplies by 1.05 past 150 pixels and only then by
                // 1.1 at each of 200 through 600. Using 1.1 for the first step too made every
                // expert spin past 150 pixels 4.76% faster than it should be, compounding into
                // every step above it.
                if reach > PRIME_SPIN_RANGE_FROM {
                    s *= PRIME_SPIN_RANGE_GAIN_FIRST;
                }
                let mut step = PRIME_SPIN_RANGE_FROM + PRIME_SPIN_RANGE_STEP;
                while reach > step && step <= 600.0 {
                    s *= PRIME_SPIN_RANGE_GAIN;
                    step += PRIME_SPIN_RANGE_STEP;
                }
                s
            } else {
                PRIME_SPIN_SPEED
            };
            // Never overshoot: right on top of you it moves exactly the remaining distance.
            if reach < speed {
                speed = reach;
            }
            npc.velocity = (dx / reach * speed, dy / reach * speed);
        }

        _ => {
            // Enraged, and it simply runs you down. `NPC.cs:28034-28035`: `damage = 9999; defense =
            // 9999;`, both live numbers, and neither is `dontTakeDamage`. The 9999 armour is what
            // makes daylight a fail-state (`damage_taken` floors at one, so it is about four and a
            // half thousand hits), and the 9999 damage is what makes touching it fatal. This set
            // `invulnerable` instead and left the damage at the table's, so the boss could not be
            // hurt at all but was also no more dangerous to stand next to than a hovering one.
            npc.defense = PRIME_ENRAGED_STAT;
            npc.damage_bonus = PRIME_ENRAGED_STAT as f32 / npc.stats.damage.max(1) as f32;
            npc.rotation += f32::from(npc.direction) * 0.3;
            let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
            let reach = dx.hypot(dy).max(1.0);
            let speed = (PRIME_ENRAGED_SPEED + reach / PRIME_ENRAGED_GAIN)
                .clamp(PRIME_ENRAGED_MIN, PRIME_ENRAGED_MAX);
            npc.velocity = (dx / reach * speed, dy / reach * speed);
        }
    }
    out
}

/// Styles 33–36: one arm.
///
/// `head` is what it hangs off. `ai[0]` is which side it is on, and `ai[2]` its own state: 99 means
/// it has been dragged too far and is flying home.
pub fn prime_arm(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    head: Option<Parent>,
    rng: &mut SmallRng,
) -> PrimeOutcome {
    let mut out = PrimeOutcome::default();
    npc.dirty = true;
    let limb = prime_limb(npc.npc_type);

    // No head means no arm: it comes apart over half a second.
    let Some(head) = head else {
        npc.ai[2] += 10.0;
        if npc.ai[2] > 50.0 {
            out.spent = true;
        }
        return out;
    };
    let (hx, hy) = head.center();
    let (cx, cy) = npc.center();
    let station = (
        hx - limb.station.0 * npc.ai[0] - cx,
        hy + limb.station.1 - cy,
    );
    let from_station = station.0.hypot(station.1);
    npc.sprite_direction = -(npc.ai[0] as i8);

    // Dragged too far: give up on the player and fly home. It only re-engages well inside that.
    if npc.ai[2] != 99.0 {
        if from_station > PRIME_LIMB_LOST {
            npc.ai[2] = 99.0;
        }
    } else if from_station < PRIME_LIMB_FOUND {
        npc.ai[2] = 0.0;
    }

    if npc.ai[2] == 99.0 {
        fly_home(npc, (hx, hy));
        return out;
    }

    // While the head hovers, the arms hold station; while it spins, they come for you.
    if head.state == head_state::HOVERING {
        hold_station(npc, (hx, hy), &limb);
        return out;
    }
    if head.state == head_state::LEAVING {
        out.spent = true;
        return out;
    }

    let Some(target) = world.target.filter(|t| t.alive) else {
        // Nobody to chase: it sinks.
        npc.velocity.1 = (npc.velocity.1 + 0.1).min(16.0);
        return out;
    };

    // Chasing. The approach is gentle — an arm drifts onto you rather than diving.
    let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
    let reach = dx.hypot(dy).max(f32::MIN_POSITIVE);
    let wanted = (dx / reach * limb.chase_speed, dy / reach * limb.chase_speed);
    npc.rotation = wanted.1.atan2(wanted.0) - std::f32::consts::FRAC_PI_2;
    for (v, w) in [
        (&mut npc.velocity.0, wanted.0),
        (&mut npc.velocity.1, wanted.1),
    ] {
        if *v > w {
            if *v > 0.0 {
                *v *= 0.97;
            }
            *v -= limb.chase_accel;
        }
        if *v < w {
            if *v < 0.0 {
                *v *= 0.97;
            }
            *v += limb.chase_accel;
        }
    }

    // The armed limbs fire on their own charge while they chase.
    if let Some(projectile) = limb.shot {
        npc.local_ai[0] += 1.0;
        if npc.local_ai[0] > limb.shot_charge {
            npc.local_ai[0] = 0.0;
            let mut aim = (dx / reach * limb.shot_speed, dy / reach * limb.shot_speed);
            if limb.shot_reversed {
                // The cannon throws its bomb the other way, which is what makes it arc back down.
                aim = (-aim.0, -aim.1);
            }
            let spread = |rng: &mut SmallRng| {
                rng.random_range(-PRIME_SHOT_SPREAD_STEPS..=PRIME_SHOT_SPREAD_STEPS) as f32
                    * limb.shot_spread
            };
            aim.0 += spread(rng);
            aim.1 += spread(rng);
            out.shots.push(Shot {
                projectile,
                damage: limb.shot_damage,
                position: (cx + aim.0 * limb.shot_lead, cy + aim.1 * limb.shot_lead),
                velocity: aim,
                time_left: 600,
                ai: [0.0; 3],
            });
        }
    }

    npc.ai[3] += 1.0;
    if npc.ai[3] >= limb.attack_ticks {
        npc.ai[2] = 0.0;
        npc.ai[3] = 0.0;
    }
    out
}

/// Fly back toward the head, fast, ignoring everything else.
fn fly_home(npc: &mut Npc, head: (f32, f32)) {
    let (cx, cy) = npc.center();
    if cy > head.1 {
        if npc.velocity.1 > 0.0 {
            npc.velocity.1 *= 0.96;
        }
        npc.velocity.1 = (npc.velocity.1 - PRIME_LIMB_RETURN_Y).min(PRIME_LIMB_RETURN_Y_CAP);
    } else {
        if npc.velocity.1 < 0.0 {
            npc.velocity.1 *= 0.96;
        }
        npc.velocity.1 = (npc.velocity.1 + PRIME_LIMB_RETURN_Y).max(-PRIME_LIMB_RETURN_Y_CAP);
    }
    if cx > head.0 {
        if npc.velocity.0 > 0.0 {
            npc.velocity.0 *= 0.96;
        }
        npc.velocity.0 = (npc.velocity.0 - PRIME_LIMB_RETURN_X).min(PRIME_LIMB_RETURN_X_CAP);
    } else {
        if npc.velocity.0 < 0.0 {
            npc.velocity.0 *= 0.96;
        }
        npc.velocity.0 = (npc.velocity.0 + PRIME_LIMB_RETURN_X).max(-PRIME_LIMB_RETURN_X_CAP);
    }
}

/// Drift onto the station off the head and wait there.
fn hold_station(npc: &mut Npc, head: (f32, f32), limb: &PrimeLimb) {
    npc.ai[3] += 1.0;
    if npc.ai[3] >= limb.hold_ticks {
        npc.ai[2] += 1.0;
        npc.ai[3] = 0.0;
    }
    let (_, cy) = npc.center();
    // A band rather than a point, so it hangs rather than hunting for an exact spot.
    if cy > head.1 + limb.station.1 + 90.0 {
        if npc.velocity.1 > 0.0 {
            npc.velocity.1 *= 0.96;
        }
        npc.velocity.1 = (npc.velocity.1 - 0.04).min(3.0);
    } else if cy < head.1 + limb.station.1 + 30.0 {
        if npc.velocity.1 < 0.0 {
            npc.velocity.1 *= 0.96;
        }
        npc.velocity.1 = (npc.velocity.1 + 0.04).max(-3.0);
    }
    let wanted_x = head.0 - limb.station.0 * npc.ai[0];
    let (cx, _) = npc.center();
    if cx > wanted_x {
        if npc.velocity.0 > 0.0 {
            npc.velocity.0 *= 0.96;
        }
        npc.velocity.0 = (npc.velocity.0 - 0.1).min(6.0);
    } else {
        if npc.velocity.0 < 0.0 {
            npc.velocity.0 *= 0.96;
        }
        npc.velocity.0 = (npc.velocity.0 + 0.1).max(-6.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::ai::Conditions;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::PRIME_HEAD;
    use terrustia_proto::tile::Tile;

    struct Sky(HashMap<(i32, i32), Tile>);

    impl TileView for Sky {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn night(tiles: &Sky, target: Option<(f32, f32)>) -> World<'_, Sky> {
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
            ..Conditions::default()
        };
        w
    }

    fn head_at(position: (f32, f32), state: f32) -> Parent {
        Parent {
            position,
            size: (100.0, 100.0),
            rotation: 0.0,
            scale: 1.0,
            velocity: (0.0, 0.0),
            direction: 1,
            sprite_direction: 1,
            time_left: 3600,
            state,
            phase: 0.0,
            health: 1.0,
        }
    }

    fn piece(npc_type: u16, x: f32, y: f32) -> Npc {
        Npc::new(npc_type, (x, y), 1).expect("a piece of Prime")
    }

    /// The head alternates on a fixed clock: hover, spin, hover.
    #[test]
    fn the_head_alternates_hovering_and_spinning() {
        let tiles = Sky(HashMap::new());
        let mut h = piece(PRIME_HEAD, 0.0, 0.0);
        let w = night(&tiles, Some((0.0, 600.0)));

        let mut states = vec![h.ai[1]];
        for _ in 0..(PRIME_HOVER_TICKS + PRIME_SPIN_TICKS) as i32 * 2 {
            prime_head(&mut h, &w);
            if states.last() != Some(&h.ai[1]) {
                states.push(h.ai[1]);
            }
        }
        // Two full cycles' worth of ticks catches the start of a third, so what to check is the
        // alternation rather than an exact length.
        assert!(
            states.len() >= 4,
            "it should have changed several times: {states:?}"
        );
        for pair in states.windows(2) {
            assert_ne!(pair[0], pair[1], "it never repeats a state: {states:?}");
        }
        assert!(
            states
                .iter()
                .all(|s| *s == head_state::HOVERING || *s == head_state::SPINNING),
            "and only uses those two: {states:?}"
        );
    }

    /// While spinning it hits twice as hard and takes half as much.
    #[test]
    fn the_spin_is_when_it_is_dangerous_and_armoured() {
        let tiles = Sky(HashMap::new());
        let mut h = piece(PRIME_HEAD, 0.0, 0.0);
        let w = night(&tiles, Some((0.0, 600.0)));

        prime_head(&mut h, &w);
        assert_eq!(h.damage_bonus, 1.0, "hovering, it is ordinary");
        assert_eq!(h.defense, h.stats.defense);

        h.ai[1] = head_state::SPINNING;
        prime_head(&mut h, &w);
        assert_eq!(h.damage_bonus, PRIME_SPIN_DAMAGE as f32);
        assert_eq!(h.defense, h.stats.defense * PRIME_SPIN_DEFENSE);
    }

    /// A fresh head raises its four arms on its first tick, and never again — the gap this fix
    /// closes: nothing anywhere else in this project ever created Prime's arms at all, so the
    /// admin `/spawn` command (the only way to encounter this boss today) produced a bare head.
    #[test]
    fn a_fresh_head_raises_all_four_arms_once() {
        let tiles = Sky(HashMap::new());
        let w = night(&tiles, Some((0.0, 600.0)));
        let mut h = piece(PRIME_HEAD, 0.0, 0.0);

        let out = prime_head(&mut h, &w);
        let mut kinds: Vec<u16> = out.spawn.iter().map(|s| s.npc_type).collect();
        kinds.sort_unstable();
        let mut expected = vec![
            terrustia_proto::npc_params::PRIME_SAW,
            terrustia_proto::npc_params::PRIME_VICE,
            terrustia_proto::npc_params::PRIME_CANNON,
            terrustia_proto::npc_params::PRIME_LASER,
        ];
        expected.sort_unstable();
        assert_eq!(kinds, expected, "all four arms, exactly once");
        assert!(
            out.spawn
                .iter()
                .all(|s| s.parent == Some(Spawn::OWN_PARENT)),
            "every arm belongs to the head that raised it"
        );

        // Never again: a second tick with the flag now set must not raise a second set.
        let again = prime_head(&mut h, &w);
        assert!(
            again.spawn.is_empty(),
            "a head must only raise its arms once, got {:?}",
            again.spawn
        );
    }

    /// Daylight enrages it rather than sending it home, which is the opposite of the Twins.
    #[test]
    fn daylight_enrages_prime() {
        let tiles = Sky(HashMap::new());
        let mut h = piece(PRIME_HEAD, 0.0, 0.0);
        let mut w = night(&tiles, Some((600.0, 0.0)));
        w.conditions.day = true;

        prime_head(&mut h, &w);
        assert_eq!(h.ai[1], head_state::ENRAGED);
        // `NPC.cs:28034-28035`: nine thousand armour and nine thousand damage, both live numbers,
        // and neither of them `dontTakeDamage`. This asserted `invulnerable`, which the routine set
        // instead, and left the damage at the table's: an enraged Prime could not be hurt at all
        // but was no more dangerous to stand next to than a hovering one.
        assert_eq!(h.defense, PRIME_ENRAGED_STAT, "nothing is getting through");
        assert_eq!(
            h.contact_damage(),
            PRIME_ENRAGED_STAT,
            "and touching it is fatal"
        );
        let speed = h.velocity.0.hypot(h.velocity.1);
        assert!(speed >= PRIME_ENRAGED_MIN, "and it comes at you: {speed}");
    }

    /// The expert spin's first range step is the gentle one.
    ///
    /// `NPC.cs:27968-28008` multiplies by 1.05 past 150 pixels and only then by 1.1 at each of 200
    /// through 600. Using 1.1 for the first step too made every expert spin past 150 pixels 4.76%
    /// fast, compounding through every step above it. Setting `PRIME_SPIN_RANGE_GAIN_FIRST` back to
    /// 1.1 turns this red.
    #[test]
    fn the_expert_spins_first_range_step_is_the_gentle_one() {
        let tiles = Sky(HashMap::new());
        // Measured from the head's own centre, so the range bands are the ones under test rather
        // than an accident of where its corner happens to sit.
        let speed_at = |reach: f32| {
            let mut h = piece(PRIME_HEAD, 0.0, 0.0);
            h.ai[0] = 1.0;
            h.ai[1] = head_state::SPINNING;
            let (cx, cy) = h.center();
            let mut w = night(&tiles, Some((cx + reach, cy)));
            w.conditions.expert = true;
            prime_head(&mut h, &w);
            h.velocity.0.hypot(h.velocity.1)
        };
        let base = PRIME_SPIN_SPEED_EXPERT;
        let near = speed_at(100.0);
        assert!((near - base).abs() < 0.01, "inside 150 it is flat: {near}");
        let first = speed_at(175.0);
        assert!(
            (first - base * PRIME_SPIN_RANGE_GAIN_FIRST).abs() < 0.01,
            "the first step is 1.05, got {first}"
        );
        let second = speed_at(225.0);
        assert!(
            (second - base * PRIME_SPIN_RANGE_GAIN_FIRST * PRIME_SPIN_RANGE_GAIN).abs() < 0.01,
            "and the second 1.1 on top of it, got {second}"
        );
    }

    /// The Vice and the Laser start their attack timer a hundred and fifty ticks in
    /// (`NPC.cs:27824`, `:27831`), so the four arms do not wind up and switch in lockstep. Without
    /// it a boss of four independent limbs is one limb played four times.
    #[test]
    fn the_vice_and_the_laser_get_a_head_start() {
        let tiles = Sky(HashMap::new());
        let mut h = piece(PRIME_HEAD, 0.0, 0.0);
        let w = night(&tiles, Some((600.0, 0.0)));
        let arms = prime_head(&mut h, &w).spawn;

        let start_of = |npc_type: u16| {
            arms.iter()
                .find(|s| s.npc_type == npc_type)
                .and_then(|s| s.ai[3])
        };
        assert_eq!(start_of(PRIME_VICE), Some(PRIME_ARM_HEAD_START));
        assert_eq!(start_of(PRIME_LASER), Some(PRIME_ARM_HEAD_START));
        assert_eq!(start_of(PRIME_CANNON), None, "and the other two do not");
        assert_eq!(start_of(PRIME_SAW), None);
    }

    /// An arm holds station while the head hovers and comes for you while it spins.
    #[test]
    fn the_arms_wait_for_the_spin() {
        let tiles = Sky(HashMap::new());
        let player = (2000.0, 0.0);
        let w = night(&tiles, Some(player));

        // How far from the head it gets at its furthest. Measuring where it *ends up* would be
        // wrong: an arm that reaches its leash turns round and comes back, so the endpoint is
        // near the head either way.
        let reach_of = |head_state: f32| {
            let mut rng = SmallRng::seed_from_u64(32);
            let mut arm = piece(PRIME_SAW, 0.0, 0.0);
            arm.ai[0] = -1.0;
            let mut furthest: f32 = 0.0;
            for _ in 0..400 {
                prime_arm(
                    &mut arm,
                    &w,
                    Some(head_at((0.0, 0.0), head_state)),
                    &mut rng,
                );
                arm.position.0 += arm.velocity.0;
                arm.position.1 += arm.velocity.1;
                furthest = furthest.max(arm.center().0);
            }
            furthest
        };

        let waiting = reach_of(head_state::HOVERING);
        let chasing = reach_of(head_state::SPINNING);
        assert!(
            waiting < 600.0,
            "hovering, it should have stayed by the head, got {waiting}"
        );
        assert!(
            chasing > waiting + 200.0,
            "spinning, it should have gone for the player: {chasing} vs {waiting}"
        );
    }

    /// An arm dragged too far gives up on you and flies home.
    #[test]
    fn a_dragged_arm_goes_back_to_the_head() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(2);
        // Player a very long way off, so the arm is dragged past its leash.
        let w = night(&tiles, Some((10_000.0, 0.0)));
        let mut arm = piece(PRIME_SAW, 5000.0, 0.0);
        arm.ai[0] = -1.0;
        let head = head_at((0.0, 0.0), head_state::SPINNING);

        prime_arm(&mut arm, &w, Some(head), &mut rng);
        assert_eq!(arm.ai[2], 99.0, "it should have given up");
        for _ in 0..120 {
            prime_arm(&mut arm, &w, Some(head), &mut rng);
            arm.position.0 += arm.velocity.0;
        }
        assert!(arm.velocity.0 < 0.0, "and be heading home");
    }

    /// An arm with no head comes apart.
    #[test]
    fn an_arm_without_a_head_falls_apart() {
        let tiles = Sky(HashMap::new());
        let mut rng = SmallRng::seed_from_u64(3);
        let w = night(&tiles, Some((100.0, 0.0)));
        let mut arm = piece(PRIME_SAW, 0.0, 0.0);
        let mut ticks = 0;
        let gone = loop {
            let out = prime_arm(&mut arm, &w, None, &mut rng);
            ticks += 1;
            if out.spent || ticks > 100 {
                break out.spent;
            }
        };
        assert!(gone, "it should not survive its head");
    }

    /// The cannon lobs backwards; the laser fires straight; the saw fires nothing.
    #[test]
    fn each_arm_attacks_in_its_own_way() {
        let tiles = Sky(HashMap::new());
        let player = (600.0, 0.0);
        let w = night(&tiles, Some(player));
        let head = head_at((0.0, 0.0), head_state::SPINNING);

        let fire = |ty: u16| {
            let mut rng = SmallRng::seed_from_u64(5);
            let mut arm = piece(ty, 0.0, 0.0);
            arm.ai[0] = -1.0;
            let mut shots = Vec::new();
            for _ in 0..400 {
                shots.extend(prime_arm(&mut arm, &w, Some(head), &mut rng).shots);
            }
            shots
        };

        assert!(fire(PRIME_SAW).is_empty(), "the saw is a melee arm");

        let laser = fire(PRIME_LASER);
        assert!(!laser.is_empty(), "the laser should fire");
        assert!(
            laser[0].velocity.0 > 0.0,
            "and toward the player, got {:?}",
            laser[0].velocity
        );

        let cannon = fire(PRIME_CANNON);
        assert!(!cannon.is_empty(), "the cannon should fire");
        assert!(
            cannon[0].velocity.0 < 0.0,
            "and away from them, so the bomb arcs: {:?}",
            cannon[0].velocity
        );
    }
}
