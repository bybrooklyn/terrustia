//! Per-type parameters pulled out of the game's AI routines.
//!
//! Terraria writes these as branches inside the AI itself — `AI_003_Fighters` is 85% such
//! branches by line count. They are numbers, not logic, so they live here as data and the
//! behaviour modules stay algorithms.
//!
//! The type ids here are NPC types. Projectile types live in [`crate::projectile::ids`], because
//! both spaces are `u16` and a dozen numbers mean one thing in each: the import path is what keeps
//! them apart.

use crate::projectile::ids::{
    DRAKIN_FIREBALL, GOBLIN_BOMB, GOBLIN_SHARK_SHOT, JAVELIN, JAVELIN_T3, OGRE_POUND, OGRE_SPIT,
};

/// How a fighter accelerates and how fast it walks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FighterMovement {
    pub max_speed: f32,
    pub accel: f32,
    /// Multiplier applied while over top speed and on the ground.
    pub friction: f32,
}

/// The values almost every fighter uses.
pub const FIGHTER_DEFAULT: FighterMovement = FighterMovement {
    max_speed: 1.0,
    accel: 0.07,
    friction: 0.8,
};

/// Movement for a fighter type.
pub fn fighter_movement(npc_type: u16) -> FighterMovement {
    match npc_type {
        214 => FighterMovement {
            max_speed: 2.0,
            accel: 0.09,
            friction: 0.8,
        },
        215 => FighterMovement {
            max_speed: 1.5,
            accel: 0.08,
            friction: 0.8,
        },
        381 => FighterMovement {
            max_speed: 2.0,
            accel: 0.5,
            friction: 0.8,
        },
        382 => FighterMovement {
            max_speed: 2.0,
            accel: 0.5,
            friction: 0.8,
        },
        409 => FighterMovement {
            max_speed: 2.0,
            accel: 0.5,
            friction: 0.8,
        },
        411 => FighterMovement {
            max_speed: 2.0,
            accel: 0.5,
            friction: 0.8,
        },
        426 => FighterMovement {
            max_speed: 4.0,
            accel: 0.6,
            friction: 0.95,
        },
        520 => FighterMovement {
            max_speed: 4.0,
            accel: 1.0,
            friction: 0.7,
        },
        _ => FIGHTER_DEFAULT,
    }
}

/// Fighters that open a door rather than smashing it.
///
/// The distinction only holds outside a blood moon: on one, even these break through, which is
/// why zombies come through the door on a blood moon and merely knock on it otherwise.
pub fn fighter_opens_doors(npc_type: u16) -> bool {
    matches!(
        npc_type,
        3 | 21
            | 44
            | 77
            | 132
            | 161
            | 167
            | 186
            | 187
            | 188
            | 189
            | 196
            | 197
            | 200
            | 201
            | 202
            | 203
            | 223
            | 319
            | 320
            | 321
            | 322
            | 323
            | 324
            | 331
            | 332
            | 430
            | 449
            | 450
            | 451
            | 452
            | 481
            | 590
            | 635
            | 691
    )
}

/// Fighters that reach further ahead when looking for an obstacle, because they are wide.
pub fn fighter_wide_probe(npc_type: u16) -> bool {
    matches!(
        npc_type,
        109 | 163
            | 164
            | 199
            | 236
            | 239
            | 257
            | 258
            | 290
            | 391
            | 415
            | 425
            | 426
            | 427
            | 508
            | 530
            | 532
            | 580
            | 582
    )
}

/// Fighters that can step up a taller ledge than the usual 16.1 pixels.
pub fn fighter_tall_step(npc_type: u16) -> bool {
    matches!(npc_type, 163 | 164 | 236 | 239 | 530)
}

/// How fast a slime's hop timer fills, beyond the one tick every slime gets.
///
/// From the `ai[0]` accumulation at the top of the hop block in `AI_001_Slimes`.
///
/// `hurt` is whether the NPC has taken any damage at all, which only Hoppin' Jack reads. Vanilla
/// writes its bonus as `(1 - life / lifeMax) * 10` over two `int`s (`NPC.cs:62200-62204`), so the
/// division is integer: exactly 1 while untouched and 0 from the first point of damage, making the
/// term a step from nothing to ten rather than the ramp the arithmetic looks like.
pub fn slime_timer_bonus(npc_type: u16, hurt: bool) -> f32 {
    match npc_type {
        304 if hurt => 10.0,
        304 => 0.0,
        59 => 2.0, // LavaSlime
        71 => 3.0, // DungeonSlime
        667 => 3.0,
        138 => 2.0, // RedSlime
        183 => 1.0,
        658 => 5.0,
        659 => 3.0,
        377 | 446 => 3.0,
        81 => 4.0, // CorruptSlime, at its normal scale
        _ => 0.0,
    }
}

/// The base of the three hop windows. Negative, and scaled to reach the other two.
pub fn slime_hop_window(npc_type: u16) -> f32 {
    match npc_type {
        659 => -500.0,
        667 => -400.0,
        _ => -1000.0,
    }
}

/// The arapaima, the one swimmer with its own set of numbers (`NPC.cs:23892-23919`).
pub const ARAPAIMA: u16 = 157;

/// How a swimmer accelerates and how fast it may go, from the `aiStyle == 16` block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwimSpeed {
    /// Per-tick acceleration, x then y. Only the arapaima steers differently on the two axes.
    pub accel: (f32, f32),
    /// The speed a hunting swimmer is tested against on each axis, and what one found over it is
    /// set back to.
    ///
    /// For every type but the arapaima the two numbers are equal, which is a plain clamp. The
    /// arapaima is tested against 8 and 5 but knocked back to 7 and 4 (`NPC.cs:23897-23919`), so
    /// it surges and falls back rather than riding a flat ceiling.
    pub max_x: (f32, f32),
    pub max_y: (f32, f32),
}

/// Swim speeds by type. Sharks and their kin move noticeably faster than a goldfish.
pub fn swim_speed(npc_type: u16) -> SwimSpeed {
    match npc_type {
        // `NPC.cs:23892-23919`. The fastest swimmer in the game by a wide margin, and the only one
        // that accelerates harder sideways than vertically.
        ARAPAIMA => SwimSpeed {
            accel: (0.25, 0.2),
            max_x: (8.0, 7.0),
            max_y: (5.0, 4.0),
        },
        65 | 102 | 692 => SwimSpeed {
            accel: (0.15, 0.15),
            max_x: (5.0, 5.0),
            max_y: (3.0, 3.0),
        },
        _ => SwimSpeed {
            accel: (0.1, 0.1),
            max_x: (3.0, 3.0),
            max_y: (2.0, 2.0),
        },
    }
}

/// How hard a swimmer already moving against its facing is slowed before it accelerates.
///
/// Only the arapaima does this (`NPC.cs:23892-23901`); it is what stops the fastest fish in the
/// game from overshooting every time its prey doubles back.
pub const ARAPAIMA_REVERSE_DAMPING: f32 = 0.95;

/// Swimmers that never hunt: they drift regardless of who is in the water with them.
pub fn swimmer_is_passive(npc_type: u16) -> bool {
    matches!(npc_type, 55 | 592 | 607 | 615 | 688)
}

/// Ordinary step-up limit, in pixels.
pub const STEP_HEIGHT: f32 = 16.1;

/// Step-up limit for the taller fighters.
pub const STEP_HEIGHT_TALL: f32 = 24.1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_what_almost_every_fighter_uses() {
        // Zombie and Skeleton take the generic values.
        assert_eq!(fighter_movement(3), FIGHTER_DEFAULT);
        assert_eq!(fighter_movement(21), FIGHTER_DEFAULT);
        assert_eq!(FIGHTER_DEFAULT.max_speed, 1.0);
        assert_eq!(FIGHTER_DEFAULT.accel, 0.07);
        assert_eq!(FIGHTER_DEFAULT.friction, 0.8);
    }

    #[test]
    fn the_fast_types_keep_their_overrides() {
        assert_eq!(fighter_movement(520).max_speed, 4.0);
        assert_eq!(fighter_movement(520).accel, 1.0);
        assert_eq!(fighter_movement(426).friction, 0.95);
        assert_eq!(fighter_movement(214).max_speed, 2.0);
    }

    #[test]
    fn slime_timers_match_the_types_the_game_singles_out() {
        assert_eq!(
            slime_timer_bonus(1, false),
            0.0,
            "a blue slime just gets its one tick"
        );
        assert_eq!(slime_timer_bonus(59, false), 2.0, "LavaSlime is twitchier");
        assert_eq!(slime_timer_bonus(658, false), 5.0);
        assert_eq!(slime_hop_window(1), -1000.0);
        assert_eq!(slime_hop_window(659), -500.0);
        assert_eq!(slime_hop_window(667), -400.0);
    }

    /// M2: `NPC.cs:62200-62204` writes Hoppin' Jack's bonus as `(1 - life / lifeMax) * 10` over
    /// two `int`s, so the division is integer and the term is a step, not a ramp: nothing at all
    /// while untouched, and the full ten from the first point of damage onward.
    #[test]
    fn a_hoppin_jack_only_speeds_up_once_it_has_been_hit() {
        assert_eq!(
            slime_timer_bonus(304, false),
            0.0,
            "untouched, it is normal"
        );
        assert_eq!(slime_timer_bonus(304, true), 10.0, "hurt, it is frantic");
        assert_eq!(
            slime_timer_bonus(1, true),
            0.0,
            "no other slime reads its own health"
        );
    }

    #[test]
    fn sharks_swim_faster_than_goldfish() {
        assert_eq!(swim_speed(65).max_x.0, 5.0, "shark");
        assert_eq!(swim_speed(55).max_x.0, 3.0, "goldfish takes the default");
        assert!(swim_speed(65).accel.0 > swim_speed(55).accel.0);
    }

    /// B2: `NPC.cs:23892-23919` gives type 157 a branch of its own ahead of the shark's, and it is
    /// the only swimmer whose speed test and speed assignment are different numbers: over eight it
    /// is knocked back to seven, so it surges instead of riding a ceiling. Before this the
    /// arapaima fell to the default and swam at three pixels a tick with a tenth of acceleration.
    #[test]
    fn the_arapaima_outswims_every_other_fish() {
        let it = swim_speed(ARAPAIMA);
        assert_eq!(it.accel, (0.25, 0.2), "harder sideways than vertically");
        assert_eq!(it.max_x, (8.0, 7.0));
        assert_eq!(it.max_y, (5.0, 4.0));
        assert!(
            it.accel.0 > swim_speed(65).accel.0,
            "it out-accelerates a shark"
        );
        assert!(it.max_x.1 > swim_speed(65).max_x.1, "and outruns one");
        // Every other type tests and assigns the same number, which is a plain clamp.
        for other in [55u16, 65, 102, 692] {
            let s = swim_speed(other);
            assert_eq!(s.max_x.0, s.max_x.1, "type {other} clamps on x");
            assert_eq!(s.max_y.0, s.max_y.1, "type {other} clamps on y");
            assert_eq!(s.accel.0, s.accel.1, "type {other} steers evenly");
        }
    }

    #[test]
    fn the_harmless_swimmers_are_listed() {
        assert!(swimmer_is_passive(55), "goldfish");
        assert!(!swimmer_is_passive(65), "a shark hunts");
    }

    #[test]
    fn the_classic_door_bashers_are_listed() {
        assert!(fighter_opens_doors(3), "Zombie");
        assert!(fighter_opens_doors(21), "Skeleton");
        assert!(!fighter_opens_doors(1), "a slime is not even a fighter");
    }

    #[test]
    fn wide_and_tall_lists_are_distinct_from_the_default() {
        assert!(fighter_wide_probe(109) && !fighter_wide_probe(3));
        assert!(fighter_tall_step(163) && !fighter_tall_step(3));
        // The two step limits come from the game as 16.1 and 24.1.
        assert_eq!(STEP_HEIGHT, 16.1);
        assert_eq!(STEP_HEIGHT_TALL, 24.1);
    }
}

/// How one axis of a flying enemy's steering behaves.
///
/// The game writes this same shape a dozen times over — inside the bat routine, the floating-eye
/// routine and several others — with different numbers in it, and in one place even lifts the
/// numbers into named locals first. It is one algorithm with a handful of parameters, so it is one
/// struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Steering {
    /// Added toward the facing direction every tick.
    pub accel: f32,
    /// Extra push applied once already moving faster than [`Steering::overshoot_at`] the other way.
    pub overshoot: f32,
    /// Applied while the velocity still has the wrong sign. Positive values push *against* the
    /// turn, which is what gives a bat its lazy arc; negative ones hurry it along instead, which is
    /// what makes a wandering eye feel darting.
    pub brake: f32,
    /// Top speed on this axis, which is also the speed the routine will still accelerate below.
    pub max: f32,
    /// Speed at which the overshoot term kicks in. The same as `max` for everything except the
    /// vampire bat, which is clamped at 7 but starts overshooting at 4.
    pub overshoot_at: f32,
    /// Speed below which the *positive* arm will engage. The same as `max` for everything except
    /// the demon eye's hardmode cousin, which climbs faster than it will choose to dive.
    pub engage_positive: f32,
}

impl Steering {
    /// The usual shape: one speed serves as the engage threshold, the overshoot threshold and the
    /// clamp.
    pub const fn new(accel: f32, overshoot: f32, brake: f32, max: f32) -> Self {
        Self {
            accel,
            overshoot,
            brake,
            max,
            overshoot_at: max,
            engage_positive: max,
        }
    }

    pub const fn overshooting_at(mut self, speed: f32) -> Self {
        self.overshoot_at = speed;
        self
    }

    pub const fn engaging_positive_below(mut self, speed: f32) -> Self {
        self.engage_positive = speed;
        self
    }
}

/// Steering on both axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlierSteering {
    pub x: Steering,
    pub y: Steering,
}

/// What almost every type in the bat style uses.
pub const BAT_STEERING_DEFAULT: FlierSteering = FlierSteering {
    x: Steering::new(0.1, 0.1, 0.05, 4.0),
    y: Steering::new(0.04, 0.05, 0.03, 1.5),
};

/// Steering for a type in the bat style.
pub fn bat_steering(npc_type: u16) -> FlierSteering {
    match npc_type {
        // VampireBat: fast on both axes, and the only type whose overshoot threshold is not its
        // top speed.
        158 => FlierSteering {
            x: Steering::new(0.2, 0.1, 0.05, 7.0).overshooting_at(4.0),
            y: Steering::new(0.2, 0.1, 0.05, 7.0).overshooting_at(4.0),
        },
        // FlyingSnake: twice the acceleration and a much freer climb.
        226 => FlierSteering {
            x: Steering::new(0.2, 0.1, 0.05, 4.0),
            y: Steering::new(0.1, 0.05, 0.03, 2.5),
        },
        // QueenSlimeMinionPurple, the one place the game names these numbers itself.
        660 => FlierSteering {
            x: Steering::new(0.35, 0.35, 0.175, 6.0),
            y: Steering::new(0.3, 0.3, 0.225, 5.0),
        },
        _ => BAT_STEERING_DEFAULT,
    }
}

/// The second steering pass the true bats run, on top of the shared one.
///
/// This is not a refinement of the first pass but a repeat of it: a cave bat accelerates twice per
/// tick and so closes on a player at double the rate a demon does, while still clamped to the same
/// top speed. Types outside this set run the shared pass only.
pub fn bat_extra_steering(npc_type: u16) -> Option<FlierSteering> {
    match npc_type {
        // Hellbat, whose second pass brakes more gently than the first.
        60 => Some(FlierSteering {
            x: Steering::new(0.1, 0.07, 0.03, 4.0),
            y: Steering::new(0.04, 0.03, 0.02, 1.5),
        }),
        49 | 51 | 62 | 66 | 93 | 137 | 150 | 151 | 152 | 634 => Some(BAT_STEERING_DEFAULT),
        _ => None,
    }
}

/// Whether a type in the bat style swims up out of water rather than flying through it.
pub fn bat_rises_in_water(npc_type: u16) -> bool {
    matches!(
        npc_type,
        48 | 49 | 51 | 60 | 62 | 66 | 93 | 137 | 150 | 151 | 152 | 634
    )
}

/// Whether a type keeps flying the way it already is when it loses sight of its target, instead of
/// turning to chase through walls. Only the flying snake does.
pub fn bat_holds_course_when_blind(npc_type: u16) -> bool {
    npc_type == 226
}

/// Whether daylight above the surface drives a type off. Only the vampire bat.
pub fn bat_flees_daylight(npc_type: u16) -> bool {
    npc_type == 158
}

/// How a flier drifts once it has lost its target for long enough to give up chasing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatDrift {
    pub accel_x: f32,
    pub accel_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

/// Drift parameters for a type in the bat style.
pub fn bat_drift(npc_type: u16) -> BatDrift {
    match npc_type {
        // Harpy, Demon and Voodoo Demon wander more slowly than the bats do.
        48 | 62 | 66 => BatDrift {
            accel_x: 0.12,
            accel_y: 0.07,
            max_x: 3.0,
            max_y: 1.25,
        },
        _ => BatDrift {
            accel_x: 0.2,
            accel_y: 0.1,
            max_x: 4.0,
            max_y: 1.5,
        },
    }
}

/// What a type in the bat style throws, and how often.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatShot {
    /// Values of the fire timer at which a shot leaves, counted from the last reload.
    pub cadence: &'static [u16],
    /// The timer is reloaded somewhere in `base..base + spread` ticks after the last volley.
    pub reload_base: u16,
    pub reload_spread: u16,
    pub projectile: u16,
    pub damage: i32,
    /// Muzzle speed. The demon's scythe is slow because it accelerates once it is out.
    pub speed: f32,
    /// Half-width of the square of inaccuracy added to the aim point, in pixels.
    pub scatter: i32,
    /// Multiple of the shooter's own velocity added to the muzzle position, so a fast mover leads
    /// its shot.
    pub lead: f32,
    /// Distance along the shot direction the projectile starts ahead of the muzzle.
    pub standoff: f32,
}

/// What a type in the bat style shoots, if anything.
pub fn bat_shot(npc_type: u16) -> Option<BatShot> {
    match npc_type {
        // Harpy feathers.
        48 => Some(BatShot {
            cadence: &[30, 60, 90],
            reload_base: 400,
            reload_spread: 400,
            projectile: 38,
            damage: 15,
            speed: 6.0,
            scatter: 100,
            lead: 0.0,
            standoff: 0.0,
        }),
        // Demon and Voodoo Demon: the demon scythe, which leaves slowly and picks up speed.
        62 | 66 => Some(BatShot {
            cadence: &[20, 40, 60, 80],
            reload_base: 300,
            reload_spread: 300,
            projectile: 44,
            damage: 21,
            speed: 0.2,
            scatter: 100,
            lead: 0.0,
            standoff: 0.0,
        }),
        // RedDevil, which throws its scythe from well ahead of itself.
        156 => Some(BatShot {
            cadence: &[20, 40, 60, 80, 100],
            reload_base: 250,
            reload_spread: 250,
            projectile: 115,
            damage: 80,
            speed: 0.2,
            scatter: 50,
            lead: 5.0,
            standoff: 100.0,
        }),
        _ => None,
    }
}

/// Steering for a type in the floating-eye style.
///
/// The eyes proper share the bat's numbers; the two hardmode variants that use this style are
/// quicker and, unusually, brake *into* their turns rather than against them.
pub fn eye_steering(npc_type: u16) -> FlierSteering {
    match npc_type {
        // The wandering eye, which phases through walls.
        170 | 171 | 180 => FlierSteering {
            x: Steering::new(0.08, 0.04, -0.2, 4.0),
            y: Steering::new(0.1, 0.05, -0.15, 2.5),
        },
        // A hardmode eye that dives faster than it climbs.
        116 => FlierSteering {
            x: Steering::new(0.1, 0.1, -0.2, 6.0),
            y: Steering::new(0.04, 0.05, -0.15, 2.5).engaging_positive_below(1.5),
        },
        _ => FlierSteering {
            x: Steering::new(0.1, 0.1, 0.05, 4.0),
            y: Steering::new(0.04, 0.05, 0.03, 1.5),
        },
    }
}

/// Faster steering a type switches to below half health, if it has one.
pub fn eye_enraged_steering(npc_type: u16) -> Option<FlierSteering> {
    (npc_type == 133).then(|| FlierSteering {
        x: Steering::new(0.1, 0.1, 0.05, 6.0),
        y: Steering::new(0.1, 0.1, 0.05, 4.0),
    })
}

/// Whether a type in the eye style periodically sinks through terrain to reach its target.
pub fn eye_phases_through_walls(npc_type: u16) -> bool {
    matches!(npc_type, 170 | 171 | 180)
}

/// Whether daylight above the surface sends a type home.
///
/// This is what empties the sky of demon eyes at dawn: they are not killed, they turn away and
/// their despawn timer is cut to ten ticks.
pub fn eye_flees_daylight(npc_type: u16) -> bool {
    matches!(npc_type, 2 | 133 | 190 | 191 | 192 | 193 | 194 | 317 | 318)
}

/// Whether a type in the eye style swims up out of water.
pub fn eye_rises_in_water(npc_type: u16) -> bool {
    !eye_phases_through_walls(npc_type)
}

/// Ticks of being unable to see its target before a phasing eye starts sinking through walls.
pub const EYE_PHASE_DELAY: f32 = 300.0;

/// How long a discouraged NPC has left once it turns away.
pub const DESPAWN_ENCOURAGED_TICKS: i32 = 10;

/// How large a type is drawn and, for a few routines, how fast it moves and how hard it hits.
///
/// `SetDefaults` sets this for sixty-odd types and leaves the rest at one. The hornet reads it as
/// `2 - scale` to turn a bigger body into a slower one, and its stinger's damage scales with it
/// directly.
///
/// Two groups set their scale inside a nested per-variant branch rather than once at the top of
/// their own block: the scarecrows (305-314, `NPC.cs:12966-13003`) and the mourning wood /
/// pumpking family (338-340). Both are transcribed below; the halves of each pair that vanilla
/// leaves alone (305, 310, 338) keep the default of one.
pub fn npc_scale(npc_type: u16) -> f32 {
    match npc_type {
        16 => 1.25,
        26 => 0.90,
        27 => 0.95,
        28 => 1.10,
        50 => 1.25,
        59 => 1.10,
        60 => 1.10,
        70 => 1.50,
        71 => 1.25,
        72 => 1.20,
        73 => 0.95,
        81 => 1.10,
        95 => 0.90,
        96 => 0.90,
        97 => 0.90,
        111 => 0.95,
        112 => 0.90,
        113 => 1.20,
        114 => 1.20,
        121 => 1.10,
        123 => 0.90,
        134 => 1.25,
        135 => 1.25,
        136 => 1.25,
        138 => 1.05,
        141 => 1.10,
        151 => 1.15,
        183 => 1.10,
        184 => 1.10,
        204 => 1.15,
        // Hoppin' Jack (`NPC.cs:12953`) and the scarecrows, whose scale is set per-variant inside
        // the 305-314 block (`NPC.cs:12979`, `:12987`, `:12995`, `:13003`).
        304 => 1.10,
        306 | 311 => 1.05,
        307 | 312 => 0.90,
        308 | 313 => 0.95,
        309 | 314 => 1.10,
        319 => 0.90,
        320 => 1.05,
        321 => 1.10,
        324 => 1.05,
        334 => 0.90,
        335 => 1.05,
        336 => 0.85,
        339 => 1.05,
        340 => 0.90,
        354 => 0.90,
        376 => 0.90,
        535 => 1.10,
        631 => 1.10,
        666 => 0.90,
        _ => 1.0,
    }
}

/// How fast a type in the eater-of-souls style flies, and how hard it accelerates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChaseSpeed {
    pub max: f32,
    pub accel: f32,
}

/// Speed and acceleration for a type in the eater-of-souls style.
///
/// `expert` picks the eater of souls' harder acceleration; everything else ignores it.
pub fn eater_speed(npc_type: u16, expert: bool) -> ChaseSpeed {
    match npc_type {
        6 if expert => ChaseSpeed {
            max: 4.0,
            accel: 0.035,
        },
        6 | 173 => ChaseSpeed {
            max: 4.0,
            accel: 0.02,
        },
        94 => ChaseSpeed {
            max: 4.2,
            accel: 0.022,
        },
        619 => ChaseSpeed {
            max: 6.0,
            accel: 0.1,
        },
        231 => ChaseSpeed {
            max: 3.0,
            accel: 0.017,
        },
        42 | 232..=235 => ChaseSpeed {
            max: 3.5,
            accel: 0.021,
        },
        205 => ChaseSpeed {
            max: 3.25,
            accel: 0.018,
        },
        176 => ChaseSpeed {
            max: 4.0,
            accel: 0.017,
        },
        23 => ChaseSpeed {
            max: 1.0,
            accel: 0.03,
        },
        5 => ChaseSpeed {
            max: 5.0,
            accel: 0.03,
        },
        _ => ChaseSpeed {
            max: 6.0,
            accel: 0.05,
        },
    }
}

/// When a type in the eater-of-souls style adds its wandering jitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jitter {
    /// Only once further from the target than a hundred pixels.
    WhenFar,
    /// Every tick, however close it gets.
    Always,
}

/// The jitter, if this type has one.
///
/// It is what stops these enemies converging into a single line: a slow sawtooth on `ai[0]` nudges
/// the velocity around so a swarm spreads out instead of stacking.
pub fn eater_jitter(npc_type: u16) -> Option<Jitter> {
    match npc_type {
        6 | 139 | 173 | 205 => Some(Jitter::WhenFar),
        42 | 94 | 176 | 210 | 211 | 231..=235 | 619 => Some(Jitter::Always),
        _ => None,
    }
}

/// Whether a type puts on a burst of homing inside 150 pixels.
pub fn eater_homes_in_close(npc_type: u16) -> bool {
    matches!(npc_type, 6 | 94 | 173 | 619)
}

/// Whether a type accelerates twice as hard while still moving the wrong way.
///
/// The types without it turn lazily, which is what makes an eater of souls drift past you and come
/// back round rather than snapping onto you.
pub fn eater_turns_hard(npc_type: u16) -> bool {
    !matches!(npc_type, 6 | 42 | 94 | 139 | 173 | 231..=235 | 619)
}

/// How much speed a type keeps when it bounces off terrain, if it bounces at all.
pub fn eater_bounce(npc_type: u16) -> Option<f32> {
    match npc_type {
        6 | 173 => Some(0.4),
        23 | 42 | 94 | 139 | 176 | 205 | 210 | 211 | 231..=235 | 619 => Some(0.7),
        _ => None,
    }
}

/// How a type climbs out of water.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaterRise {
    pub accel: f32,
    pub cap: f32,
    /// Whether surfacing also re-picks a target.
    pub retarget: bool,
}

/// Water behaviour for a type in the eater-of-souls style.
pub fn eater_water_rise(npc_type: u16) -> Option<WaterRise> {
    match npc_type {
        6 | 94 | 173 | 619 => Some(WaterRise {
            accel: 0.3,
            cap: 2.0,
            retarget: false,
        }),
        42 | 176 | 205 | 231..=235 => Some(WaterRise {
            accel: 0.5,
            cap: 4.0,
            retarget: true,
        }),
        _ => None,
    }
}

/// Whether daylight sends a type in the eater-of-souls style home.
///
/// Written in the game as a long list of exceptions, which inverts to a short list: only the
/// servants of Cthulhu and their kin leave at dawn.
pub fn eater_flees_daylight(npc_type: u16) -> bool {
    !matches!(
        npc_type,
        6 | 23 | 42 | 94 | 173 | 176 | 205 | 210 | 211 | 231..=235 | 252 | 619
    )
}

/// Whether a type slows its climb and its dive near the surface, so it stays at the treeline.
pub fn eater_hugs_the_surface(npc_type: u16) -> bool {
    matches!(npc_type, 42 | 231..=235)
}

/// A stinger a type in the eater-of-souls style spits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stinger {
    pub projectile: u16,
    /// Damage before the shooter's own scale is applied.
    pub damage: f32,
    pub speed: f32,
    pub scatter: i32,
    /// Charge needed before a shot is attempted.
    pub charge_needed: f32,
    /// Charge gained per tick is `rand(5..20) * 0.1 * scale`, applied this many times.
    pub charge_rolls: u32,
}

/// What a type in the eater-of-souls style spits, if anything.
pub fn eater_stinger(npc_type: u16) -> Option<Stinger> {
    match npc_type {
        42 | 231..=235 => Some(Stinger {
            projectile: 55,
            damage: 10.0,
            speed: 8.0,
            scatter: 20,
            charge_needed: 130.0,
            charge_rolls: 1,
        }),
        // The moss hornet charges twice as fast and stings three times as hard.
        176 => Some(Stinger {
            projectile: 55,
            damage: 30.0,
            speed: 8.0,
            scatter: 20,
            charge_needed: 130.0,
            charge_rolls: 2,
        }),
        _ => None,
    }
}

/// How fast a worm swims through rock, and how sharply it can turn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WormMotion {
    pub speed: f32,
    /// Change in velocity per tick. A worm has no brakes: this is all it has to steer with, which
    /// is why a fast one carves such wide circles.
    pub turn: f32,
}

/// Speed and turn rate for a worm.
///
/// `target_in_sandstorm` is the target's `ZoneSandstorm` (`SceneMetrics.cs:706`), which only the
/// tomb crawler reads: a sandstorm is the one thing that makes it worth outrunning.
pub fn worm_motion(npc_type: u16, expert: bool, target_in_sandstorm: bool) -> WormMotion {
    match npc_type {
        // The Solar Crawltipede (`NPC.cs:52325-52328`), which vanilla sets after the rest of the
        // chain rather than alongside it; no other branch claims 412, so the order does not matter
        // here.
        SOLAR_CRAWLTIPEDE_HEAD => WormMotion {
            speed: 10.0,
            turn: 0.3,
        },
        // The Destroyer, which is the fastest burrower in the game and turns no better for it.
        DESTROYER_HEAD | DESTROYER_BODY | DESTROYER_TAIL => WormMotion {
            speed: DESTROYER_SPEED,
            turn: DESTROYER_TURN,
        },
        95 => WormMotion {
            speed: 5.5,
            turn: 0.045,
        },
        10 => WormMotion {
            speed: 6.0,
            turn: 0.05,
        },
        513 => WormMotion {
            speed: 7.0,
            turn: 0.1,
        },
        7 => WormMotion {
            speed: 9.0,
            turn: 0.1,
        },
        13 if expert => WormMotion {
            speed: 12.0,
            turn: 0.15,
        },
        13 => WormMotion {
            speed: 10.0,
            turn: 0.07,
        },
        // The tomb crawler, which more than doubles its turn rate and half again its speed while
        // its target is out in a sandstorm (`NPC.cs:52255-52267`).
        510 if target_in_sandstorm => WormMotion {
            speed: 16.0,
            turn: 0.35,
        },
        510 => WormMotion {
            speed: 10.0,
            turn: 0.25,
        },
        87 => WormMotion {
            speed: 11.0,
            turn: 0.25,
        },
        621 => WormMotion {
            speed: 15.0,
            turn: 0.45,
        },
        375 => WormMotion {
            speed: 6.0,
            turn: 0.15,
        },
        454 => WormMotion {
            speed: 20.0,
            turn: 0.55,
        },
        402 => WormMotion {
            speed: 9.0,
            turn: 0.3,
        },
        39 => WormMotion {
            speed: 9.0,
            turn: 0.1,
        },
        _ => WormMotion {
            speed: 8.0,
            turn: 0.07,
        },
    }
}

/// How far behind its leader a worm segment sits, in pixels.
///
/// Usually its own width, with a handful of types nudged apart or squeezed together so the sprites
/// meet cleanly.
pub fn worm_segment_gap(npc_type: u16, width: i32) -> f32 {
    match npc_type {
        87..=92 => 42.0,
        454..=459 => 36.0,
        513..=515 => width as f32 - 6.0,
        412..=414 => width as f32 + 6.0,
        621..=623 => 24.0,
        _ => width as f32,
    }
}

/// Whether a type burrows even in open air, rather than falling when it leaves the ground.
pub fn worm_always_digs(npc_type: u16) -> bool {
    matches!(npc_type, 87..=92 | 402 | 412..=414 | 454..=459 | 621..=623)
}

/// Whether a type leads a worm, as opposed to being dragged along as a segment.
///
/// Only a head burrows away when there is nobody near; a segment simply follows.
pub fn worm_is_head(npc_type: u16) -> bool {
    matches!(
        npc_type,
        7 | 10 | 13 | 39 | 95 | 98 | 117 | 134 | 375 | 454 | 510 | 513 | 621
    )
}

/// Whether a type gives up on a target who climbs above the surface.
///
/// The tomb crawler's version depends on the player standing in an underground desert; with no
/// biome tracking that reads as false, which is the same answer for anyone outside one.
pub fn worm_flees_surface_target(npc_type: u16) -> bool {
    matches!(npc_type, 10 | 39 | 95 | 117 | 510 | 513)
}

/// How hard a fleeing worm dives back into the rock.
pub fn worm_sink_accel(npc_type: u16) -> f32 {
    if npc_type == 513 { 0.1 } else { 0.2 }
}

/// Gravity on a worm in open air, which the bone serpent halves while it is still rising so its
/// leaps arc higher.
pub fn worm_air_gravity(npc_type: u16, rising: bool) -> f32 {
    if npc_type == 39 && rising { 0.08 } else { 0.11 }
}

/// How far a worm looks for a player before deciding nobody is around and burrowing off.
pub const WORM_ATTENTION_RANGE: f32 = 1000.0;

/// How long a worm has left once it gives up.
pub const WORM_DESPAWN_TICKS: i32 = 300;

/// How fast something in the town style walks, and how quickly it gets there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Walk {
    pub max: f32,
    pub accel: f32,
}

/// Whether a type is one of the wandering critters rather than a resident.
///
/// The distinction runs through the whole routine: a critter has no home to return to, faces
/// whoever is nearest instead of whoever is talking, walks off ledges a resident would stop at,
/// and takes its rests in shorter bursts.
pub fn town_is_critter(npc_type: u16) -> bool {
    matches!(
        npc_type,
        46 | 148
            | 149
            | 230
            | 299
            | 300
            | 303
            | 337
            | 361
            | 362
            | 364
            | 366
            | 367
            | 443
            | 445
            | 447
            | 538
            | 539
            | 540
            | 583..=585
            | 592
            | 593
            | 602
            | 607
            | 608
            | 610
            | 616
            | 617
            | 625..=627
            | 639..=652
            | 687
            | 688
    )
}

/// The town slimes, which are pets rather than critters and swim rather than sink.
pub fn town_is_slime(npc_type: u16) -> bool {
    matches!(npc_type, 670 | 678..=684)
}

/// Turtles and frogs, which are at home in water and never drown.
pub fn town_breathes_underwater(npc_type: u16) -> bool {
    matches!(npc_type, 361 | 445 | 616 | 617 | 625 | 687)
}

/// Frogs, which shove themselves forward in a single kick rather than swimming steadily.
pub fn town_hops_in_water(npc_type: u16) -> bool {
    matches!(npc_type, 361 | 445 | 687)
}

/// Mice and rats, which scurry: quick, and quick to stop.
pub fn town_scurries(npc_type: u16) -> bool {
    matches!(npc_type, 300 | 447 | 610)
}

/// How close something hostile has to be before a resident counts itself in danger.
///
/// `NPCID.Sets.DangerDetectRange` (`NPCID.cs:4841`), whose default is -1 and read as 200
/// (`NPC.cs:54010-54014`). The spread is the point: the Guide notices trouble at 300 pixels and the
/// Tax Collector at 1200, while the Stylist and the Golfer barely look up.
pub fn town_danger_range(npc_type: u16) -> f32 {
    match npc_type {
        441 => 50.0,
        207 | 353 => 60.0,
        633 => 100.0,
        550 | 588 => 120.0,
        637 | 638 | 656 | 670 | 678..=684 => 250.0,
        17 => 320.0,
        18 | 38 | 107 | 369 | 453 => 300.0,
        208 => 400.0,
        142 => 500.0,
        22 | 54 | 108 | 160 | 663 => 700.0,
        124 | 227 | 228 => 800.0,
        19 | 178 | 368 => 900.0,
        209 | 229 => 1000.0,
        20 => 1200.0,
        _ => 200.0,
    }
}

/// Walking speed for a type in the town style.
///
/// `alarmed` is vanilla's `friendly && (flag16 | flag21)` (`NPC.cs:54467`): a resident with a
/// hostile inside [`town_danger_range`], or one that is drowning. `hurt` is `1 - life / lifeMax`,
/// which is what turns a wounded resident's retreat into a run.
pub fn town_walk(npc_type: u16, wet: bool, alarmed: bool, hurt: f32) -> Walk {
    // A town slime in water is the one case vanilla writes *after* the danger override
    // (`NPC.cs:54473-54477`), so it wins outright.
    if town_is_slime(npc_type) && wet {
        return Walk {
            max: 2.0,
            accel: 0.2,
        };
    }
    // `NPC.cs:54467-54473`, which overrides the whole per-type table below rather than joining it.
    if alarmed {
        return Walk {
            max: 1.5 + hurt * 0.9,
            accel: 0.1,
        };
    }
    match npc_type {
        // Mice outrun everything else on land, and stop just as sharply.
        300 | 447 | 610 => Walk {
            max: 2.0,
            accel: 1.0,
        },
        625 if wet => Walk {
            max: 2.5,
            accel: 1.0,
        },
        625 => Walk {
            max: 0.2,
            accel: 0.07,
        },
        616 | 617 if wet => Walk {
            max: 2.0,
            accel: 1.0,
        },
        616 | 617 => Walk {
            max: 0.5,
            accel: 0.07,
        },
        299 | 538 | 539 | 639..=645 => Walk {
            max: 1.5,
            accel: 0.07,
        },
        _ => Walk {
            max: 1.0,
            accel: 0.07,
        },
    }
}

/// How far from its home tile a resident will drift before turning back.
pub const TOWN_LEASH: i32 = 25;
/// ...and the distance at which it stops choosing and simply turns.
pub const TOWN_LEASH_HARD: i32 = 50;
/// Beyond this, walking away from home burns the walk timer six times as fast.
pub const TOWN_FAR_FROM_HOME: i32 = 35;

/// Upward impulse for clearing a three-, two- and one-tile obstacle respectively.
pub const TOWN_JUMP_TALL: f32 = 6.0;
pub const TOWN_JUMP: f32 = 5.0;
pub const TOWN_JUMP_LOW: f32 = 4.4;

/// How high a step a town NPC will walk up rather than jump.
pub const TOWN_STEP_HEIGHT: f32 = 20.0;

/// How fast a grub inches along the ground.
///
/// The whole style is one speed and a pair of timers; only the glowing bait worms hurry.
pub fn grub_speed(npc_type: u16) -> f32 {
    match npc_type {
        485 => 0.25,
        486 => 0.325,
        487 => 0.4,
        // The truffle worm covers ground three times as fast as anything else here.
        374 => 0.2 * 3.0,
        _ => 0.2,
    }
}

/// How long a grub spends resting and moving, as inclusive-exclusive tick ranges.
pub const GRUB_REST_TICKS: (u32, u32) = (300, 900);
pub const GRUB_CRAWL_TICKS: (u32, u32) = (600, 1800);

/// Speed of the things that run along tracks and walls.
pub const WHEEL_SPEED: f32 = 6.0;
/// How fast a blazing wheel spins, in radians per tick.
pub const WHEEL_SPIN: f32 = 0.13;

/// A spike ball's launch speed and how hard it accelerates between bounces.
pub const SPIKE_BALL_SPEED: f32 = 6.0;
pub const SPIKE_BALL_ACCEL: f32 = 0.2;

/// The antlion's sand ball: speed, damage and reload. The type is
/// `projectile::ids::ANTLION_SHOT_TYPE`.
pub const ANTLION_SHOT_SPEED: f32 = 12.0;
pub const ANTLION_SHOT_DAMAGE: i32 = 10;
pub const ANTLION_RELOAD: f32 = 200.0;

/// How close a player has to come, and for how long, before a lost girl drops her disguise.
pub const LOST_GIRL_RANGE: f32 = 200.0;
pub const LOST_GIRL_WINDUP: f32 = 21.0;

/// What a lost girl turns into.
pub const NYMPH: u16 = 196;

/// How close a player has to come, and for how long, before a truffle worm flees.
pub const TRUFFLE_WORM_RANGE: f32 = 160.0;
pub const TRUFFLE_WORM_WINDUP: f32 = 90.0;
/// ...and what it becomes when it does.
pub const TRUFFLE_WORM_DIGGER: u16 = 375;

/// How far a rooted plant can lunge from its anchor, and how hard it pulls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rooted {
    /// Reach in pixels.
    pub reach: f32,
    /// Acceleration toward the aim point.
    pub pull: f32,
    /// Top speed on each axis.
    pub cap: f32,
}

/// Reach and speed for a rooted plant.
pub fn rooted(npc_type: u16) -> Rooted {
    match npc_type {
        // Man Eater: the long one.
        43 => Rooted {
            reach: 250.0,
            pull: 0.035,
            cap: 3.0,
        },
        101 => Rooted {
            reach: 175.0,
            pull: 0.035,
            cap: 2.0,
        },
        // Fungi Bulb: barely reaches past its own stalk.
        259 => Rooted {
            reach: 100.0,
            pull: 0.035,
            cap: 2.0,
        },
        175 => Rooted {
            reach: 500.0,
            pull: 0.05,
            cap: 4.0,
        },
        260 => Rooted {
            reach: 350.0,
            pull: 0.15,
            cap: 2.0,
        },
        _ => Rooted {
            reach: 150.0,
            pull: 0.035,
            cap: 2.0,
        },
    }
}

/// A plant's stretch cycle: for the last third of it, its reach grows by 30%.
pub const ROOTED_CYCLE: f32 = 450.0;
pub const ROOTED_STRETCH_AT: f32 = 300.0;
pub const ROOTED_STRETCH: f32 = 1.3;

/// How close a player has to come before a perched vulture takes off.
pub const PERCH_STARTLE: f32 = 100.0;
/// The kick it gives itself on take-off.
pub const PERCH_LAUNCH: f32 = 6.0;
/// How high above its target a vulture prefers to circle when it is not directly overhead.
pub const VULTURE_CEILING: f32 = 100.0;
/// ...and the horizontal distance beyond which it starts climbing to that height.
pub const VULTURE_CLIMB_AT: f32 = 50.0;

/// How a jellyfish gathers itself before a lunge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jelly {
    /// Speed kept each tick while winding up. The closer to one, the longer the wind-up.
    pub drag: f32,
    /// Speed it has to drop below before it lets go.
    pub trigger: f32,
    /// Speed of the lunge itself.
    pub lunge: f32,
}

/// Lunge parameters for a type in the jellyfish style.
pub fn jelly(npc_type: u16) -> Jelly {
    match npc_type {
        103 => Jelly {
            drag: 0.98 * 0.98,
            trigger: 0.6,
            lunge: 9.0,
        },
        // A squid goes off at the slightest excuse. `NPC.cs:24410-24438` applies `velocity *= 0.98f`
        // to every type first and only then the per-type extra, so the drag here is the product of
        // the two, not the extra on its own.
        221 => Jelly {
            drag: 0.98 * 0.99,
            trigger: 1.0,
            lunge: 7.0,
        },
        242 => Jelly {
            drag: 0.98 * 0.995,
            trigger: 3.0,
            lunge: 7.0,
        },
        _ => Jelly {
            drag: 0.98,
            trigger: 0.2,
            lunge: 7.0,
        },
    }
}

/// How something in the flying-fish style hunts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hover {
    pub accel_x: f32,
    pub accel_y: f32,
    pub max_x: f32,
    pub max_y: f32,
    /// Horizontal distance inside which it stops pushing sideways.
    pub deadband: f32,
    /// ...and beyond which it climbs to half that height above its target.
    pub climb_at: f32,
    /// Whether it shoulders its own kind aside rather than stacking with them.
    pub avoids_its_own_kind: bool,
}

/// Hover parameters for a type in the flying-fish style.
pub fn hover(npc_type: u16) -> Hover {
    match npc_type {
        509 => Hover {
            accel_x: 0.08,
            accel_y: 0.03,
            max_x: 4.5,
            max_y: 2.0,
            deadband: 40.0,
            climb_at: 150.0,
            avoids_its_own_kind: true,
        },
        581 => Hover {
            accel_x: 0.06,
            accel_y: 0.02,
            max_x: 4.0,
            max_y: 2.0,
            deadband: 40.0,
            climb_at: 150.0,
            avoids_its_own_kind: true,
        },
        587 => Hover {
            accel_x: 0.13,
            accel_y: 0.09,
            max_x: 6.5,
            max_y: 3.5,
            deadband: 0.0,
            climb_at: 250.0,
            avoids_its_own_kind: false,
        },
        _ => Hover {
            accel_x: 0.05,
            accel_y: 0.01,
            max_x: 3.0,
            max_y: 1.0,
            deadband: 30.0,
            climb_at: 100.0,
            avoids_its_own_kind: false,
        },
    }
}

/// How long a flying fish keeps hunting after it loses sight of its target.
pub const HOVER_ATTENTION: f32 = 90.0;

/// A cursed skull's speed and acceleration at a given range.
///
/// The tiers are what make one feel like it is stalking you: far away it closes fast, and the
/// closer it gets the more it slows, until inside 250 pixels it stops steering altogether and just
/// jitters around you waiting for its charge.
pub fn skull_approach(distance: f32) -> (f32, f32) {
    if distance > 350.0 {
        (5.0, 0.3)
    } else if distance > 300.0 {
        (3.0, 0.2)
    } else if distance > 250.0 {
        (1.5, 0.1)
    } else {
        (1.0, 0.011)
    }
}

/// How long a cursed skull circles before it charges, and how long the charge lasts.
pub const SKULL_CHARGE_AT: f32 = 600.0;
pub const SKULL_CHARGE_OVER: f32 = 650.0;
/// Speed and acceleration during that charge.
pub const SKULL_CHARGE_SPEED: f32 = 4.0;
pub const SKULL_CHARGE_ACCEL: f32 = 0.011 * 8.0;
/// Range inside which it starts jittering rather than steering.
pub const SKULL_JITTER_RANGE: f32 = 250.0;
/// The jitter itself: how fast the sawtooth runs and how hard it pushes.
pub const SKULL_JITTER_RATE: f32 = 0.9;
pub const SKULL_JITTER_PUSH: f32 = 0.019;
pub const SKULL_JITTER_PERIOD: f32 = 200.0;
pub const SKULL_JITTER_TURN: f32 = 100.0;

/// The giant cursed skull's shot: range, cycle, speed and damage. The type is
/// `projectile::ids::GIANT_SKULL_SHOT_TYPE`.
pub const GIANT_SKULL_RANGE: f32 = 500.0;
pub const GIANT_SKULL_WINDUP: f32 = 120.0;
pub const GIANT_SKULL_RECOVER: f32 = 40.0;
pub const GIANT_SKULL_RELEASE: f32 = 20.0;
pub const GIANT_SKULL_SHOT_SPEED: f32 = 6.0;
pub const GIANT_SKULL_SHOT_DAMAGE: i32 = 25;

/// The Prismatic Lacewing (`NPCID.EmpressButterfly`) and what killing one wakes
/// (`NPCID.HallowBoss`). Killing a Lacewing is the only summon the Empress of Light has
/// (`NPC.cs:80309-80319`).
pub const PRISMATIC_LACEWING: u16 = 661;
pub const EMPRESS_OF_LIGHT: u16 = 636;

/// The Lacewing's own half of style 65 (`NPC.cs:45387-45445`), which is where it stops behaving
/// like an ordinary butterfly.
///
/// `ai[2]` is a fade counter rather than the variety an ordinary butterfly keeps there (vanilla's
/// own variety roll is guarded `type != 661`). It climbs by one every tick the nearest player is
/// invalid or further off than [`LACEWING_KEEP_RANGE`], falls by one otherwise, and is clamped to
/// [`LACEWING_FADE_CAP`] while that player is standing in the hallow. At [`LACEWING_FADE_GONE`] the
/// Lacewing simply goes, which only the raised cap (leaving the hallow lifts it to
/// [`LACEWING_FADE_GONE`]) can ever reach: wandering out of range alone makes it fade and turn
/// untouchable, but it stays.
pub const LACEWING_KEEP_RANGE: f32 = 300.0;
pub const LACEWING_FADE_GONE: f32 = 60.0;
pub const LACEWING_FADE_CAP: f32 = 50.0;
/// It also looks around for danger half again as often as an ordinary butterfly
/// (`NPC.cs:45549-45556`, `localAI[1] = 10f` rather than `15f`).
pub const LACEWING_FEAR_INTERVAL: f32 = 10.0;

/// How long a butterfly holds a heading before picking another, as an inclusive-exclusive range.
pub const BUTTERFLY_REPLAN: (u32, u32) = (90, 240);
/// How gently it eases onto that heading: one sixtieth of the difference per tick.
pub const BUTTERFLY_EASE: f32 = 60.0;
/// Beyond this it flies back toward the nearest player rather than wandering freely.
pub const BUTTERFLY_HOMING_RANGE: f32 = 700.0;
/// How close something dangerous has to be before a butterfly bolts, and how often it checks.
pub const BUTTERFLY_FEAR_RANGE: f32 = 100.0;
pub const BUTTERFLY_FEAR_INTERVAL: f32 = 15.0;
/// The hardest a fleeing butterfly can be flying.
pub const BUTTERFLY_PANIC_SPEED: f32 = 16.0;

/// Walls that mark the dungeon, which is where the dungeon's casters will teleport and nowhere
/// else.
pub const fn dungeon_wall(wall: u16) -> bool {
    matches!(wall, 7 | 8 | 9 | 94..=99)
}

/// A projectile a caster throws, from the `ai[1] == num92` block at `NPC.cs:21249-21354`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thrown {
    pub projectile: u16,
    /// Damage in classic. `GetAttackDamage_ForProjectiles(n, n * 0.8f)` (`NPC.cs:21290`) means
    /// expert and above take four fifths of it, because the difficulty scales it back up.
    pub damage: i32,
    /// How fast it leaves, in pixels a tick. Zero means the shot is placed rather than aimed.
    pub speed: f32,
    /// Random jitter added to each axis of the aim, in pixels, before it is normalised.
    pub scatter: i32,
    /// Ticks of the target's own velocity to lead by. Only the Necromancer bothers.
    pub lead: f32,
    /// Whether it leaves from the caster's middle rather than the top of its head.
    pub from_center: bool,
}

/// What a caster conjures, and where.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conjuring {
    /// The NPC it summons. Every *pre-hardmode* caster summons an NPC rather than firing a
    /// projectile - the fire imp's burning sphere and the dark caster's water sphere are NPCs with
    /// one hit point and no gravity. The hardmode dungeon casters throw ordinary projectiles.
    pub summons: Option<u16>,
    /// A projectile instead.
    pub throws: Option<Thrown>,
    /// Offset from the caster's own position, in pixels. The x component is applied along its
    /// facing for the types that conjure to the side.
    pub offset: (f32, f32),
    /// Whether the offset's x follows the caster's facing.
    pub offset_follows_facing: bool,
    /// Tick of the wind-up at which it lets go.
    pub release_at: f32,
    /// How far from its target it will teleport, in tiles.
    /// How long it is held in place after choosing somewhere to blink to: vanilla's `num91`
    /// (`NPC.cs:21160-21164`), which is 20 for everything but the Fire Imp's 5. It is *not* a
    /// search range: `AI_AttemptToFindTeleportSpot` is always called with its default 20-tile
    /// `rangeFromTargetTile` (`NPC.cs:18973`, `:21173`).
    pub blink: f32,
    /// Whether it will only teleport within the dungeon.
    pub dungeon_bound: bool,
    /// Ticks of the cycle at which it starts a wind-up (`NPC.cs:21082-21155`). The dungeon casters
    /// each get their own, faster, cadence.
    pub cadence: &'static [f32],
    /// A cycle that ends early, as (at or past this, jump the timer to that). Four types shorten
    /// their 650-tick cycle this way and so teleport more often than they cast.
    pub cycle_ends_at: Option<(f32, f32)>,
}

/// What a type in the caster style conjures.
///
/// Style 8 is thirteen types, not five: the four pre-hardmode summoners and the librarian, plus the
/// Rune Wizard and the four two-variant hardmode dungeon casters, each with its own cadence and its
/// own projectile.
pub fn conjuring(npc_type: u16) -> Option<Conjuring> {
    let base = Conjuring {
        summons: None,
        throws: None,
        offset: (0.0, -8.0),
        offset_follows_facing: false,
        release_at: 25.0,
        blink: CASTER_BLINK,
        dungeon_bound: false,
        cadence: &CASTER_CADENCE,
        cycle_ends_at: None,
    };
    // The dungeon casters all fire from the top of their head, unscattered and unled, until they
    // say otherwise.
    let bolt = Thrown {
        projectile: 0,
        damage: 0,
        speed: 0.0,
        scatter: 0,
        lead: 0.0,
        from_center: false,
    };
    match npc_type {
        // Fire Imp: a burning sphere, thrown out to the side, and it barely moves to do it.
        24 => Some(Conjuring {
            summons: Some(25),
            offset: (8.0, 20.0),
            offset_follows_facing: true,
            release_at: 10.0,
            blink: CASTER_BLINK_SHORT,
            ..base
        }),
        29 => Some(Conjuring {
            summons: Some(30),
            ..base
        }),
        32 => Some(Conjuring {
            summons: Some(33),
            dungeon_bound: true,
            ..base
        }),
        45 => Some(Conjuring {
            summons: Some(665),
            ..base
        }),
        693 => Some(Conjuring {
            throws: Some(Thrown {
                projectile: 1092,
                damage: 13,
                from_center: true,
                ..bolt
            }),
            dungeon_bound: true,
            ..base
        }),
        // Rune Wizard: six casts a cycle, from the middle, with a small aim wobble
        // (`NPC.cs:21095-21101`, `:21339-21352`).
        172 => Some(Conjuring {
            throws: Some(Thrown {
                projectile: 129,
                damage: 40,
                speed: 10.0,
                scatter: 10,
                from_center: true,
                ..bolt
            }),
            cadence: &[75.0, 150.0, 225.0, 300.0, 375.0, 450.0],
            ..base
        }),
        // Ragged Caster: three bursts of three, and a cycle that ends at 540 (`:21112-21122`).
        281 | 282 => Some(Conjuring {
            throws: Some(Thrown {
                projectile: 293,
                damage: 40,
                speed: 4.0,
                ..bolt
            }),
            offset: (0.0, 0.0),
            cadence: &[
                100.0, 120.0, 140.0, 200.0, 220.0, 240.0, 300.0, 320.0, 340.0,
            ],
            cycle_ends_at: Some((540.0, 700.0)),
            dungeon_bound: true,
            ..base
        }),
        // Necromancer: five casts, scattered and led, and the shortest cycle of the four
        // (`:21083-21093`, `:21275-21279`).
        283 | 284 => Some(Conjuring {
            throws: Some(Thrown {
                projectile: 290,
                damage: 30,
                speed: 6.0,
                scatter: 30,
                lead: 10.0,
                ..bolt
            }),
            offset: (0.0, 0.0),
            cadence: &[100.0, 150.0, 200.0, 250.0, 300.0],
            cycle_ends_at: Some((450.0, 700.0)),
            dungeon_bound: true,
            ..base
        }),
        // Diabolist: the ordinary cadence, but it leaves early (`:21146-21149`).
        285 | 286 => Some(Conjuring {
            throws: Some(Thrown {
                projectile: 291,
                damage: 40,
                speed: 8.0,
                ..bolt
            }),
            offset: (0.0, 0.0),
            // `ai[0] > 400f`, and the timer only ever holds whole numbers.
            cycle_ends_at: Some((401.0, 650.0)),
            dungeon_bound: true,
            ..base
        }),
        // Desert Djinn: one wind-up a cycle, six times as long as anyone else's, which drops five
        // ghost lanterns around its target rather than throwing anything at it
        // (`:21105-21109`, `:21150-21153`, `:21190-21237`).
        533 => Some(Conjuring {
            throws: Some(Thrown {
                projectile: 596,
                damage: 0,
                ..bolt
            }),
            cadence: &[180.0],
            cycle_ends_at: Some((360.0, 650.0)),
            ..base
        }),
        _ => None,
    }
}

/// The Desert Djinn's wind-up is 181 ticks rather than everyone else's 30
/// (`NPC.cs:21107-21109`), and it drops a lantern on every thirtieth tick of it while the count is
/// still under five, so five in all (`NPC.cs:21192`). Its `release_at` is therefore not a single tick and the routine handles it
/// by type.
pub const DJINN_WINDUP: f32 = 181.0;
pub const DJINN_LANTERNS: f32 = 5.0;
/// How far from the target, in tiles, a djinn's lanterns may land, and how far from itself they
/// must (`NPC.cs:21200-21203`).
pub const DJINN_SPREAD: i32 = 6;

/// A caster's cycle: it casts at these points and teleports when the timer runs out.
pub const CASTER_CADENCE: [f32; 3] = [100.0, 200.0, 300.0];
/// How far from its target a caster looks for somewhere to land, in tiles. The same for all
/// thirteen: `AI_AttemptToFindTeleportSpot`'s `rangeFromTargetTile` default (`NPC.cs:18973`).
pub const CASTER_TELEPORT_RANGE: i32 = 20;
pub const CASTER_CYCLE: f32 = 650.0;
/// The wind-up a cast sets going.
pub const CASTER_WINDUP: f32 = 30.0;
/// How long the caster is held in place after choosing somewhere to teleport to.
pub const CASTER_BLINK: f32 = 20.0;
pub const CASTER_BLINK_SHORT: f32 = 5.0;
/// How near a player a caster refuses to land, in tiles.
pub const CASTER_TELEFRAG_GUARD: i32 = 5;
/// Beyond this many pixels of Manhattan distance it will not attempt a teleport at all.
pub const CASTER_TELEPORT_LIMIT: f32 = 2000.0;

/// How fast a member of the frost legion hops, and how hard it pushes off.
pub fn frost_hop(npc_type: u16) -> Walk {
    match npc_type {
        143 => Walk {
            max: 3.0,
            accel: 0.7,
        },
        145 => Walk {
            max: 3.5,
            accel: 0.8,
        },
        _ => Walk {
            max: 4.0,
            accel: 1.0,
        },
    }
}

/// The two hop impulses: every third hop is the big one.
pub const FROST_HOP: f32 = -6.0;
pub const FROST_LEAP: f32 = -8.2;
/// How long a frost legionnaire keeps its own counsel after walking into a wall.
pub const FROST_STUBBORN: f32 = 60.0;

/// What a member of the frost legion throws, if anything.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrostShot {
    pub projectile: u16,
    pub damage: i32,
    pub speed: f32,
    /// Whether it fires flat along its facing rather than aiming.
    pub flat: bool,
    /// Ticks between volleys for the flat shooter, or the length of the pause for the aimer.
    pub cycle: f32,
    /// Tick of that pause at which the aimer lets go.
    pub release_at: f32,
}

/// What a type in the frost legion throws.
pub fn frost_shot(npc_type: u16) -> Option<FrostShot> {
    match npc_type {
        // Snowman Gangsta: a flat burst along its facing, every two seconds, without stopping.
        143 => Some(FrostShot {
            projectile: 110,
            damage: 25,
            speed: 12.0,
            flat: true,
            cycle: 120.0,
            release_at: 0.0,
        }),
        // Snow Balla: stops, winds up for eight ticks, throws, and sets off again.
        145 => Some(FrostShot {
            projectile: 109,
            damage: 35,
            speed: 10.0,
            flat: false,
            cycle: 16.0,
            release_at: 8.0,
        }),
        _ => None,
    }
}

/// How long Mister Stabby stands still between charges.
pub const FROST_STABBY_PAUSE: f32 = 200.0;
/// How many hops any of them take before stopping to do whatever they do.
pub const FROST_HOPS_BEFORE_PAUSE: f32 = 3.0;

/// How fast a snail crawls.
pub fn snail_speed(npc_type: u16) -> f32 {
    if matches!(npc_type, 360 | 655) {
        0.6
    } else {
        0.3
    }
}

/// One chance in this many, per tick, that a snail simply lets go of its wall.
pub const SNAIL_SLIP_CHANCE: u32 = 7200;
/// ...and how many ticks of touching nothing before it accepts it has fallen off.
pub const SNAIL_LOST_GRIP: f32 = 5.0;

/// How a balloon-borne NPC drifts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Balloon {
    /// Base horizontal speed, before the wind is added.
    pub speed: f32,
    /// How hard it pulls itself along.
    pub push: f32,
    /// The extra shove it gives itself while still going the wrong way, over and under speed.
    pub reverse_fast: f32,
    pub reverse_slow: f32,
}

/// Drift parameters for a balloon type.
pub fn balloon(npc_type: u16) -> Balloon {
    match npc_type {
        // The clumsy slime balloon is the brisker of the two.
        125 | 686 => Balloon {
            speed: 3.0,
            push: 0.04,
            reverse_fast: 0.15,
            reverse_slow: 0.1,
        },
        _ => Balloon {
            speed: 2.0,
            push: 0.01,
            reverse_fast: 0.1,
            reverse_slow: 0.05,
        },
    }
}

/// How far a balloon looks down for ground before it decides it is over open air.
pub const BALLOON_LOOKDOWN: i32 = 8;
/// ...and how close that ground has to be before it climbs harder.
pub const BALLOON_TOO_LOW: i32 = 5;
/// Range within which a balloon stops drifting and starts matching its target's height.
pub const BALLOON_CHASE_RANGE: f32 = 400.0;
/// Vertical speed and acceleration while it is doing that.
pub const BALLOON_CHASE_SPEED: f32 = 2.0;
pub const BALLOON_CHASE_ACCEL: f32 = 0.035;

/// How long a dragonfly rests, and how long it flies, in ticks.
pub const DRAGONFLY_REST: (u32, u32) = (60, 120);
pub const DRAGONFLY_FLIGHT: f32 = 4.0;
/// ...unless it has strayed this far from its perch, in which case it flies until it is back.
pub const DRAGONFLY_TETHER: f32 = 112.0;
pub const DRAGONFLY_LONG_FLIGHT: f32 = 200.0;
/// Distances at which it heads home briskly, gently, or not at all.
pub const DRAGONFLY_FAR: f32 = 96.0;
pub const DRAGONFLY_NEAR: f32 = 16.0;
/// How close something has to come before a dragonfly bolts.
pub const DRAGONFLY_FEAR_NPC: f32 = 100.0;
pub const DRAGONFLY_FEAR_PLAYER: f32 = 150.0;

/// How something in the haunting style drifts and how far ahead it feels for ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Haunt {
    /// Steering on both axes: the same shape the bats use, with its own numbers.
    pub steering: FlierSteering,
    /// How many tiles ahead and down it feels for something to float above.
    pub feel: i32,
    /// How hard it sinks with nothing beneath it, and how fast that sink gets.
    pub sink: f32,
    pub sink_cap: f32,
    /// How hard it pushes back up off whatever it finds.
    pub lift: f32,
    /// A cap on that climb, where the type has one.
    pub lift_cap: Option<f32>,
}

/// Drift parameters for a type in the haunting style.
pub fn haunt(npc_type: u16) -> Haunt {
    match npc_type {
        // The drippler hangs much lower and much more slowly than a ghost, and feels further
        // ahead the further away its target is.
        490 => Haunt {
            steering: FlierSteering {
                x: Steering::new(0.1, 0.1, 0.05, 1.5),
                y: Steering::new(0.04, 0.05, 0.03, 1.0),
            },
            feel: 4,
            sink: 0.03,
            sink_cap: 0.75,
            lift: 0.075,
            lift_cap: Some(0.75),
        },
        75 | 169 => Haunt {
            steering: FlierSteering {
                x: Steering::new(0.1, 0.1, 0.05, 3.0),
                y: Steering::new(0.04, 0.05, 0.03, 1.5),
            },
            // `num312`: 4 for the wraith-like 75, 10 for the ice elemental (`NPC.cs:25044`).
            feel: if npc_type == 75 { 4 } else { 10 },
            sink: 0.2,
            sink_cap: 2.0,
            lift: 0.2,
            lift_cap: None,
        },
        _ => Haunt {
            steering: FlierSteering {
                x: Steering::new(0.1, 0.1, 0.05, 2.0),
                y: Steering::new(0.04, 0.05, 0.03, 1.5),
            },
            // A Gastropod feels eight tiles ahead (`NPC.cs:24953`), not three, which is why it
            // floats over floors and doorways an ordinary ghost would sink into. The Ichor
            // Sticker's depth changes with where its target is, so its routine sets it.
            feel: if npc_type == 122 { 8 } else { 3 },
            sink: 0.1,
            sink_cap: 3.0,
            lift: 0.1,
            lift_cap: None,
        },
    }
}

/// How deep an Ichor Sticker feels ahead of itself (`NPC.cs:25066`).
///
/// The one type whose probe depth is not constant: it reaches twice as far when its target is above
/// it, which is what lets it climb to you rather than losing you over a ledge.
pub const ICHOR_STICKER_FEEL: (i32, i32) = (6, 12);

/// Whether a type in the haunting style feels further ahead the further off its target is.
pub fn haunt_feels_by_distance(npc_type: u16) -> bool {
    npc_type == 490
}

/// Whether daylight sends a type in the haunting style away.
pub fn haunt_flees_daylight(npc_type: u16) -> bool {
    npc_type == 490
}

/// What a projectile-throwing haunting throws, as (projectile, damage, speed).
///
/// Three of the style's types shoot, and none of them did before: the Gastropod's laser is its
/// entire threat (`NPC.cs:24937-24953`), the Ice Elemental's shard (`:25037-25043`) and the Ichor
/// Sticker's glob (`:25084-25096`).
pub fn haunt_shot(npc_type: u16) -> Option<(u16, i32, f32)> {
    match npc_type {
        122 => Some((84, 25, 7.0)),
        169 => Some((128, 45, 5.0)),
        268 => Some((288, 40, 10.0)),
        _ => None,
    }
}

/// The Gastropod's and the Ice Elemental's shared attack cycle (`NPC.cs:24954-24975`,
/// `:25045-25060`): `localAI[1]` counts to 120, then `ai[3]` runs from 1 to 64 and the shot leaves
/// partway through.
pub const HAUNT_RELOAD: f32 = 120.0;
pub const HAUNT_WINDUP: f32 = 64.0;
/// Tick of that wind-up at which each of the two lets go.
pub fn haunt_release_at(npc_type: u16) -> f32 {
    if npc_type == 122 { 32.0 } else { 16.0 }
}

/// The Ichor Sticker fires on its own timer instead: 60 ticks plus a roll of up to 60 more, and a
/// hit knocks it back to minus forty-five (`NPC.cs:25068-25081`).
pub const STICKER_RELOAD: i32 = 60;
pub const STICKER_HIT_PENALTY: f32 = -45.0;

/// Whether a type gives up on a target who is dead or has got a long way away.
pub fn haunt_gives_up_at_range(npc_type: u16) -> Option<f32> {
    (npc_type == 316).then_some(3000.0)
}

/// How long something in the haunting style will hover in one spot before it decides it is stuck.
pub const HAUNT_STUCK_AT: f32 = 30.0;
pub const HAUNT_STUCK_OVER: f32 = 60.0;
/// ...and how long it then spends backing away.
pub const HAUNT_BACK_OFF: f32 = 200.0;

/// How a granite flyer moves in each of its states.
pub const GRANITE_CHASE_BASE: f32 = 2.0;
/// Its chase speed grows with distance, at this many pixels per pixel of range.
pub const GRANITE_CHASE_RAMP: f32 = 200.0;
/// How heavily each state smooths its steering: the higher, the lazier the turn.
pub const GRANITE_CHASE_SMOOTH: f32 = 50.0;
pub const GRANITE_PHASE_SMOOTH: f32 = 4.0;
pub const GRANITE_ROUTE_SMOOTH: f32 = 3.0;
pub const GRANITE_WANDER_SMOOTH: f32 = 20.0;
pub const GRANITE_ROUTE_SPEED: f32 = 1.0;
pub const GRANITE_WANDER_SPEED: f32 = 1.5;
/// How long it will keep wandering before giving up and re-planning.
pub const GRANITE_WANDER_LIMIT: f32 = 180.0;
/// ...and how often, while wandering, it looks for a route again.
pub const GRANITE_REPLAN_EVERY: f32 = 5.0;
/// How far a waypoint has to be to be worth flying to.
pub const GRANITE_WAYPOINT_MIN: f32 = 8.0;
/// ...and how far is too far.
pub const GRANITE_WAYPOINT_MAX: f32 = 800.0;

/// How close a statue mimic lets you get before it stops pretending.
pub const MIMIC_TRIGGER: f32 = 96.0;
/// How long it holds still between hops once it is moving.
pub const MIMIC_HOP_DELAY: f32 = 20.0;
/// The hop itself: a fixed impulse plus a share of the height it has to make up.
pub const MIMIC_HOP: f32 = -9.01;
pub const MIMIC_HOP_PER_DROP: f32 = 40.0;
pub const MIMIC_HOP_EXTRA_CAP: f32 = 10.0;
/// Horizontal speed: a base plus a share of the gap, capped.
pub const MIMIC_RUN: f32 = 4.0;
pub const MIMIC_RUN_PER_GAP: f32 = 50.0;
pub const MIMIC_RUN_EXTRA_CAP: f32 = 12.0;
/// How often a dormant mimic considers moving somewhere better.
pub const MIMIC_RELOCATE_EVERY: f32 = 10.0;

/// A tumbleweed's rolling speed in still air, and how quickly it gets there.
pub const TUMBLEWEED_SPEED: f32 = 4.0;
pub const TUMBLEWEED_ACCEL: f32 = 0.05;
/// How much a sandstorm adds to that, at full strength.
pub const TUMBLEWEED_WIND: f32 = 3.0;
/// How long it will keep failing to get anywhere before it stops trying to chase.
pub const TUMBLEWEED_PATIENCE: f32 = 30.0;
/// ...and the cap on that counter, so it eventually recovers.
pub const TUMBLEWEED_PATIENCE_CAP: f32 = 120.0;
/// The four upward impulses, by how tall the obstacle ahead turns out to be.
pub const TUMBLEWEED_JUMPS: [f32; 4] = [-8.5, -7.5, -7.0, -6.0];
/// ...and the one it uses to clear a gap when it is already rolling fast.
pub const TUMBLEWEED_LEAP: f32 = -8.0;
/// How high a step it rolls over rather than jumping.
pub const TUMBLEWEED_STEP: f32 = 16.1;

/// King Slime's hop cycle: three ordinary hops and then a leap.
///
/// The impulses are (rise, horizontal push, recovery ticks), and the fourth entry is the leap —
/// higher, flatter and followed by nearly twice the pause.
pub const KING_SLIME_HOPS: [(f32, f32, f32); 4] = [
    (-8.0, 4.0, -120.0),
    (-8.0, 4.0, -120.0),
    (-6.0, 4.5, -120.0),
    (-13.0, 3.5, -200.0),
];

/// How fast the hop timer fills, and the extra it gains at each health threshold.
pub const KING_SLIME_WIND: f32 = 2.0;
pub const KING_SLIME_RAGE: [(f32, f32); 5] =
    [(0.8, 1.0), (0.6, 1.0), (0.4, 2.0), (0.2, 3.0), (0.1, 4.0)];

/// Airborne steering speed, and how hard it pushes to reach it.
pub const KING_SLIME_DRIFT: f32 = 3.0;
pub const KING_SLIME_DRIFT_PUSH: f32 = 0.2;

/// How long out of sight before King Slime teleports, and the two halves of that teleport.
pub const KING_SLIME_PATIENCE: f32 = 300.0;
pub const KING_SLIME_FADE_OUT: f32 = 60.0;
pub const KING_SLIME_FADE_IN: f32 = 30.0;
/// How long it will tolerate being unreachable before it stops being fussy about where it lands.
pub const KING_SLIME_ANTI_CHEESE: f32 = 360.0;
/// Or, however visible you are, this far away (`vector.Length() > 2000f`, `NPC.cs:43643`): held at
/// arm's length it lands on top of you outright rather than hunting for a floor near you.
pub const KING_SLIME_ANTI_CHEESE_RANGE: f32 = 2000.0;
/// Vertical slack within which it counts as being on your level.
pub const KING_SLIME_LEVEL: f32 = 160.0;
/// Beyond this it gives up entirely.
pub const KING_SLIME_GIVE_UP: f32 = 3000.0;

/// Its unscaled size, which the routine multiplies by the current scale every tick.
pub const KING_SLIME_SIZE: (f32, f32) = (98.0, 92.0);
/// Scale runs from this fraction of health, plus this floor.
pub const KING_SLIME_SCALE_SPAN: f32 = 0.5;
pub const KING_SLIME_SCALE_FLOOR: f32 = 0.75;

/// Every this fraction of its health lost, it sheds a slime or three.
pub const KING_SLIME_SHED_STEP: f32 = 0.05;
/// What it sheds.
pub const KING_SLIME_SPAWN: u16 = 1;
/// In Expert Mode, each shed slime independently has a 1-in-4 chance of being this — a Spiked
/// Slime (`NPC.cs`'s `AI_015_KingSlime`: `if (Main.expertMode && Main.rand.Next(4) == 0)
/// num12 = 535;`, rolled once per slime inside the shed loop, not once for the whole batch).
pub const KING_SLIME_SPAWN_SPECIAL: u16 = 535;

/// How many creepers the Brain of Cthulhu surrounds itself with.
pub const BRAIN_CREEPERS: usize = 20;
/// What they are.
pub const CREEPER: u16 = 267;

/// Drift speed while the creepers are alive, and charge speed once they are not.
pub const BRAIN_DRIFT: f32 = 1.0;
pub const BRAIN_CHARGE: f32 = 8.0;
/// How heavily the charge is smoothed. Fifty to one, so it turns like a barge.
pub const BRAIN_CHARGE_SMOOTH: f32 = 50.0;

/// How long between teleports in each phase, as (base, extra).
pub const BRAIN_BLINK_SHIELDED: (u32, u32) = (120, 300);
pub const BRAIN_BLINK_EXPOSED: (u32, u32) = (60, 120);
/// On a dedicated server (netMode != 0) the exposed blink wait gains a further 30-89 ticks
/// (`num859 += Main.rand.Next(30, 90)`, `NPC.cs:32690-32693`); single-player omits it.
pub const BRAIN_BLINK_EXPOSED_MP_EXTRA: (u32, u32) = (30, 90);
/// How far from the target it appears, in tiles, in each phase.
pub const BRAIN_RANGE_SHIELDED: (i32, i32) = (12, 40);
pub const BRAIN_RANGE_EXPOSED: (i32, i32) = (10, 12);
/// How much of the target's own speed is added to that offset, so it leads a running player.
pub const BRAIN_LEAD: f32 = 16.0;

/// Fade rate per tick while blinking, in each phase. The exposed rate is the dedicated-server
/// (netMode != 0) value of 15 (`NPC.cs:32744`); single-player fades faster, at 25 (`NPC.cs:32748`).
pub const BRAIN_FADE_SHIELDED: f32 = 5.0;
pub const BRAIN_FADE_EXPOSED: f32 = 15.0;

/// Beyond this Manhattan distance the Brain simply leaves.
pub const BRAIN_GIVE_UP: f32 = 6000.0;
/// How long out of the crimson before it sinks away, and how fast it then falls.
pub const BRAIN_HOMESICK: f32 = 120.0;
pub const BRAIN_SINK_AFTER: f32 = 60.0;
pub const BRAIN_SINK_RATE: f32 = 0.25;

/// The Eye of Cthulhu's servant.
pub const SERVANT_OF_CTHULHU: u16 = 5;

/// How the Eye hovers in each of its two forms: (offset above the target, speed, acceleration).
pub const EYE_HOVER_FIRST: (f32, f32, f32) = (200.0, 5.0, 0.04);
pub const EYE_HOVER_SECOND: (f32, f32, f32) = (120.0, 6.0, 0.07);
/// Expert Mode's own speed and acceleration for the first form's hover — the offset above the
/// target is unchanged.
pub const EYE_HOVER_FIRST_EXPERT: (f32, f32) = (7.0, 0.15);
/// How long it hovers before the first dash of a set, and Expert Mode's much shorter version.
pub const EYE_HOVER_TICKS_FIRST: f32 = 600.0;
pub const EYE_HOVER_TICKS_FIRST_EXPERT: f32 = 210.0;
pub const EYE_HOVER_TICKS_SECOND: f32 = 200.0;

/// Expert Mode's own hover for the second form, which gains speed and acceleration the further off
/// you are (`NPC.cs:20476-20490`): one more pixel a tick and 0.05 more acceleration at each of four
/// hundred, six hundred and eight hundred pixels, cumulative. Classic has no such term, which is why
/// running away from a classic second form works and running from an Expert one does not.
pub const EYE_HOVER_SECOND_EXPERT_STEPS: [f32; 3] = [400.0, 600.0, 800.0];
pub const EYE_HOVER_SECOND_EXPERT_SPEED_STEP: f32 = 1.0;
pub const EYE_HOVER_SECOND_EXPERT_ACCEL_STEP: f32 = 0.05;

/// Dash speed in each form, and Expert Mode's faster first-form one (`NPC.cs:20252-20256`).
pub const EYE_DASH_FIRST: f32 = 6.0;
pub const EYE_DASH_FIRST_EXPERT: f32 = 7.0;
pub const EYE_DASH_SECOND: f32 = 6.8;
/// The second form's dash gets faster within a set in Expert: the second of the three is 15% up and
/// the third 30% (`NPC.cs:20558-20565`, keyed on `ai[3]`). Classic runs all three flat.
pub const EYE_DASH_SECOND_EXPERT_STEPS: [f32; 2] = [1.15, 1.3];
/// How long a dash runs before it starts bleeding off, and how long the whole dash lasts. The
/// second form drives 25% longer before braking in Expert (`NPC.cs:20582-20587`) and then ends the
/// dash far sooner (`:20608-20612`), so an Expert dash is shorter *and* commits harder.
pub const EYE_DASH_DRIVE: f32 = 40.0;
pub const EYE_DASH_DRIVE_SECOND_EXPERT: f32 = 50.0;
pub const EYE_DASH_TICKS_FIRST: f32 = 150.0;
pub const EYE_DASH_TICKS_FIRST_EXPERT: f32 = 100.0;
pub const EYE_DASH_TICKS_SECOND: f32 = 130.0;
pub const EYE_DASH_TICKS_SECOND_EXPERT: f32 = 90.0;
/// Friction once a dash is spent, in each form. Expert applies a second multiplier on top of the
/// first rather than replacing it (`NPC.cs:20276-20280`, `:20590-20594`).
pub const EYE_DASH_DRAG_FIRST: f32 = 0.98;
pub const EYE_DASH_DRAG_FIRST_EXPERT: f32 = 0.985;
pub const EYE_DASH_DRAG_SECOND: f32 = 0.97;
pub const EYE_DASH_DRAG_SECOND_EXPERT: f32 = 0.98;
/// Dashes per set.
pub const EYE_DASHES: f32 = 3.0;

/// How often the first form throws out a servant, and how far it can be to bother. Expert Mode
/// calls one much more often, and throws it a little faster.
pub const EYE_SERVANT_EVERY: f32 = 110.0;
pub const EYE_SERVANT_EVERY_EXPERT: f32 = 44.0;
pub const EYE_SERVANT_RANGE: f32 = 500.0;
pub const EYE_SERVANT_SPEED: f32 = 5.0;
pub const EYE_SERVANT_SPEED_EXPERT: f32 = 6.0;

/// The health fraction at which it splits open, and Expert Mode's own, higher one.
pub const EYE_SPLIT_AT: f32 = 0.5;
pub const EYE_SPLIT_AT_EXPERT: f32 = 0.65;
/// How long the transformation takes, in each of its two halves.
pub const EYE_SPLIT_TICKS: f32 = 100.0;
/// How fast the spin builds during it, and where it tops out.
pub const EYE_SPIN_RAMP: f32 = 0.005;
pub const EYE_SPIN_MAX: f32 = 0.5;
/// EYE-4: an Expert split is not a free two hundred ticks. Every twentieth tick of the spin throws
/// out a servant on a random bearing (`NPC.cs:20363-20400`), ten across the whole transformation.
/// The bearing is `rand(-200, 200)` on each axis normalised to five, thrown ten bearings ahead of
/// its own centre exactly as the first form's aimed servant is.
pub const EYE_SPLIT_SERVANT_EVERY: f32 = 20.0;
pub const EYE_SPLIT_SERVANT_SPEED: f32 = 5.0;
pub const EYE_SPLIT_SERVANT_SPREAD: i32 = 200;
pub const EYE_SERVANT_THROW: f32 = 10.0;

/// Its damage and defence once it has split.
///
/// The damage is a pre-scaling figure that Expert Mode lerps *down* before the difficulty
/// multiplier is applied (`NPC.cs:20447-20461`): 23 in classic, 18 in Expert and Master, and 20 in
/// the `flag3` band where it is nearly dead. The multiplier is what makes an Expert one hit harder
/// (18 x 2 = 36), not the base figure.
pub const EYE_SECOND_FORM_DAMAGE: i32 = 23;
pub const EYE_SECOND_FORM_DAMAGE_EXPERT: i32 = 18;
pub const EYE_SECOND_FORM_DAMAGE_EXPERT_LOW: i32 = 20;
pub const EYE_SECOND_FORM_DEFENSE: i32 = 0;
/// Expert Mode strips even more armour once it is nearly dead (`flag2`/`flag3` in source, each
/// overwriting the last since the lower threshold always implies the higher one).
pub const EYE_SECOND_FORM_DEFENSE_LOW: i32 = -15;
pub const EYE_SECOND_FORM_DEFENSE_LOW_AT: f32 = 0.12;
pub const EYE_SECOND_FORM_DEFENSE_VERY_LOW: i32 = -30;
pub const EYE_SECOND_FORM_DEFENSE_VERY_LOW_AT: f32 = 0.04;

/// EYE-7: the second form's Expert-only endgame, `ai[1]` states 3, 4 and 5
/// (`NPC.cs:20636-20857`), which was absent in full.
///
/// Below half health an Expert second form stops running dash *sets* and starts lunging: it aims
/// once at twenty pixels a tick, leading your own velocity, holds the line for twenty ticks and
/// then brakes, five times over before it goes back to hovering. Below 12% it opens each cycle by
/// backing six hundred pixels *underneath* you first, and below 4% the aim is scattered twice more
/// and the hold halves, which is the fight's last, wildest phase. None of it happened here: our
/// second form ran the classic hover-and-three-dashes at any health.
///
/// The three health gates. [`EYE_LUNGE_AT`] is checked at the end of a dash set, [`EYE_LUNGE_AT_HOVER`]
/// at the end of a hover, and the two lower ones are vanilla's `flag2`/`flag3`, which are also what
/// [`EYE_SECOND_FORM_DEFENSE_LOW_AT`] and its `VERY_LOW` neighbour read.
pub const EYE_LUNGE_AT: f32 = 0.5;
pub const EYE_LUNGE_AT_HOVER: f32 = 0.35;
/// How many lunges before it returns to hovering (`NPC.cs:20780`), and the `rand(1, 4)` head start
/// on that count a lunge entered from a dash set carries (`:20626`), which shortens the run.
pub const EYE_LUNGES: f32 = 5.0;
pub const EYE_LUNGE_HEAD_START: std::ops::Range<i32> = 1..4;
/// The lunge itself: twenty pixels a tick, held for twenty ticks and then braked over thirteen more
/// (`NPC.cs:20649`, `:20752-20775`). Nearly dead it halves the hold, so the lunges come twice as
/// fast (`:20022-20026`).
pub const EYE_LUNGE_SPEED: f32 = 20.0;
pub const EYE_LUNGE_HOLD: f32 = 20.0;
pub const EYE_LUNGE_HOLD_DESPERATE: f32 = 10.0;
pub const EYE_LUNGE_RECOVER: f32 = 13.0;
pub const EYE_LUNGE_DRAG: f32 = 0.95;
/// Inside this it will not start braking, so a close lunge runs on rather than stalling on top of
/// you (`NPC.cs:20754-20757`).
pub const EYE_LUNGE_STALL_RANGE: f32 = 200.0;
/// How far ahead of you it aims.
///
/// Vanilla computes a lead from your own speed and then throws it away on the very next line
/// (`num53 += 10f - num53`, `NPC.cs:20653-20654`), so the figure is always ten and the 5..15 clamp
/// below it can never bite. Transcribed as the constant it actually is, with the dead arithmetic
/// left out and named here instead. A lunge entered straight off the back-off leads four times as
/// far and travels 30% faster (`:20663-20667`); nearly dead it leads twice as far instead
/// (`:20668-20671`).
pub const EYE_LUNGE_LEAD: f32 = 10.0;
pub const EYE_LUNGE_LEAD_FROM_BELOW: f32 = 4.0;
pub const EYE_LUNGE_SPEED_FROM_BELOW: f32 = 1.3;
pub const EYE_LUNGE_LEAD_DESPERATE: f32 = 2.0;
/// The aim is scattered in three passes: a per-axis stretch before it is normalised, a flat nudge
/// after, and, nearly dead, both again at a wider figure (`NPC.cs:20674-20707`).
pub const EYE_LUNGE_STRETCH: i32 = 10;
pub const EYE_LUNGE_NUDGE: i32 = 20;
pub const EYE_LUNGE_NUDGE_DESPERATE: i32 = 50;
/// Under this, an aimed lunge is turned ninety degrees rather than fired straight at you, so a
/// point-blank lunge sweeps past instead of landing (`NPC.cs:20709-20726`); above it, the same swap
/// runs on the averaged component instead (`:20727-20741`).
pub const EYE_LUNGE_SWAP_RANGE: f32 = 100.0;
/// The back-off: nearly dead it drops six hundred pixels *below* you at nine pixels a tick for
/// seventy ticks, then lunges up (`NPC.cs:20800-20852`). `ai[2] = -1` on the way out is what marks
/// the next lunge as the fast one.
pub const EYE_BACKOFF_BELOW: f32 = 600.0;
pub const EYE_BACKOFF_SPEED: f32 = 9.0;
pub const EYE_BACKOFF_ACCEL: f32 = 0.3;
pub const EYE_BACKOFF_TICKS: f32 = 70.0;
/// How many lunges the cycle after a back-off gets: `rand(-3, 1)` in `ai[3]` (`NPC.cs:20850`), so
/// between five and eight rather than five.
pub const EYE_BACKOFF_LUNGE_BONUS: std::ops::RangeInclusive<i32> = -3..=0;
/// The one escape from the lunge cycle: on the fifth of them, in the `flag2` band, with the Eye
/// below you, it breaks off and hovers again (`NPC.cs:20638-20645`).
pub const EYE_LUNGE_BREAK_AT: f32 = 4.0;

/// Skeletron's hand.
pub const SKELETRON_HAND: u16 = 36;

/// How long the head spends hovering, and how long it then spins.
pub const SKELETRON_HOVER_TICKS: f32 = 800.0;
pub const SKELETRON_SPIN_TICKS: f32 = 400.0;
/// How far above its target the head holds station.
pub const SKELETRON_HOVER_ABOVE: f32 = 250.0;
/// Hover steering: (vertical accel, vertical cap, horizontal accel, horizontal cap).
pub const SKELETRON_HOVER: (f32, f32, f32, f32) = (0.02, 2.0, 0.05, 8.0);
/// How fast it charges while spinning, and how fast it spins. Expert mode raises the charge speed
/// to 3.5 and then compounds two separate multipliers onto it: one per range threshold crossed
/// (`NPC.cs:22292-22332`) and one for how many hands are still alive (`NPC.cs:22333-22342`).
pub const SKELETRON_SPIN_SPEED: f32 = 1.5;
pub const SKELETRON_SPIN_SPEED_EXPERT: f32 = 3.5;
/// Range thresholds (in pixels) and the factor each one compounds on. The first step is 1.05, not
/// 1.1: vanilla opens with `if (num199 > 150f) num200 *= 1.05f;` and only then repeats at 1.1 for
/// every fifty pixels up to six hundred (`NPC.cs:22292-22332`).
pub const SKELETRON_SPIN_SPEED_EXPERT_RANGE: [(f32, f32); 10] = [
    (150.0, 1.05),
    (200.0, 1.1),
    (250.0, 1.1),
    (300.0, 1.1),
    (350.0, 1.1),
    (400.0, 1.1),
    (450.0, 1.1),
    (500.0, 1.1),
    (550.0, 1.1),
    (600.0, 1.1),
];
/// And then the escalation the fight is actually built around: breaking a hand makes the head
/// charge faster (`switch (num173)`, `NPC.cs:22333-22342`). Two hands alive is the plain rate.
pub const SKELETRON_SPIN_SPEED_EXPERT_NO_HANDS: f32 = 1.1;
pub const SKELETRON_SPIN_SPEED_EXPERT_ONE_HAND: f32 = 1.05;
pub const SKELETRON_SPIN_RATE: f32 = 0.3;
/// How much of its defence it drops while spinning — the window the fight gives you.
pub const SKELETRON_SPIN_DEFENSE: i32 = 10;
/// Daylight makes it unkillable and lethal instead of ending the fight.
pub const SKELETRON_ENRAGED_SPEED: f32 = 8.0;
pub const SKELETRON_ENRAGED_STAT: i32 = 9999;
/// Beyond this on either axis it gives up.
pub const SKELETRON_GIVE_UP: f32 = 2000.0;

/// Expert mode: each living hand adds this much defence to the head, and once fewer than two
/// hands are left (or the head has dropped under three quarters health) it starts throwing a
/// skull barrage while it hovers (`NPC.cs:22059-22114`).
pub const SKELETRON_EXPERT_HAND_DEFENSE: i32 = 25;
pub const SKELETRON_BARRAGE_HANDS_THRESHOLD: usize = 2;
pub const SKELETRON_BARRAGE_HEALTH_AT: f32 = 0.75;
/// The barrage fires every eighty ticks, or forty once every hand is dead.
pub const SKELETRON_BARRAGE_INTERVAL: f32 = 80.0;
pub const SKELETRON_BARRAGE_INTERVAL_NO_HANDS: f32 = 40.0;
pub const SKELETRON_BARRAGE_DAMAGE: i32 = 17;
/// Its speed, faster still with no hands left to soak hits for it.
pub const SKELETRON_BARRAGE_SPEED: f32 = 3.0;
pub const SKELETRON_BARRAGE_SPEED_NO_HANDS: f32 = 5.0;
pub const SKELETRON_BARRAGE_JITTER: i32 = 50;

/// Where a hand docks relative to the head while the head is hovering, and while it is not.
pub const HAND_DOCK_HIGH: (f32, f32) = (120.0, -100.0);
pub const HAND_DOCK_LOW: (f32, f32) = (200.0, 230.0);
/// Docking steering: (accel, cap) for the near dock and the far one.
pub const HAND_DOCK_HIGH_DRIVE: (f32, f32, f32, f32) = (0.07, 6.0, 0.1, 8.0);
pub const HAND_DOCK_LOW_DRIVE: (f32, f32, f32, f32) = (0.04, 3.0, 0.07, 8.0);
/// How long a hand waits at the low dock before winding up.
pub const HAND_WINDUP_AT: f32 = 300.0;
/// The wind-up climb, and how far above the head it goes before it lets go.
pub const HAND_RISE: f32 = 0.1;
pub const HAND_RISE_CAP: f32 = 8.0;
pub const HAND_RISE_ABOVE: f32 = 200.0;
/// How fast it lunges.
pub const HAND_LUNGE: f32 = 18.0;
/// How far it will chase before giving the lunge up.
pub const HAND_LUNGE_LIMIT: f32 = 2000.0;
/// The sideways sweep: acceleration and cap.
pub const HAND_SWEEP: f32 = 0.1;
pub const HAND_SWEEP_CAP: f32 = 8.0;

/// The bees the Queen calls up, and what the stinger she spits
/// (`projectile::ids::STINGER`) does.
pub const BEE: u16 = 210;
pub const BEE_STRONG: u16 = 211;
pub const STINGER_DAMAGE: i32 = 11;
pub const STINGER_SPEED: f32 = 8.0;

/// Queen Bee's attacks, as `ai[0]` records them.
pub const QUEEN_CHOOSING: f32 = -1.0;
pub const QUEEN_CHARGING: f32 = 0.0;
pub const QUEEN_SUMMONING: f32 = 1.0;
pub const QUEEN_CLIMBING: f32 = 2.0;
pub const QUEEN_STINGING: f32 = 3.0;
/// The player has run past 3000: she gives chase rather than leaving, and drops back into the
/// chooser once they are within 2000 again (`NPC.cs:31053-31076`). Not a despawn.
pub const QUEEN_CHASING: f32 = 4.0;
pub const QUEEN_LEAVING: f32 = 5.0;

/// Charge speed at full health, and the extra it gains at each quarter lost — Expert Mode only;
/// Normal mode never gains any of it.
pub const QUEEN_CHARGE: f32 = 12.0;
pub const QUEEN_CHARGE_RAGE: [(f32, f32); 4] = [(0.75, 1.0), (0.5, 1.0), (0.25, 2.0), (0.1, 2.0)];
/// Expert Mode's own, higher base speed, and its own larger bonus at the same four thresholds.
pub const QUEEN_CHARGE_EXPERT: f32 = 16.0;
pub const QUEEN_CHARGE_SPEED_RAGE_EXPERT: [(f32, f32); 4] =
    [(0.75, 2.0), (0.5, 2.0), (0.25, 2.0), (0.1, 2.0)];
/// How level with you she has to be before she commits, in pixels.
pub const QUEEN_CHARGE_ALIGN: f32 = 20.0;
/// How many charges she strings together.
pub const QUEEN_CHARGES: i32 = 2;
/// The three health fractions (1/2, 1/3, 1/5) at which Expert Mode alone adds another charge to
/// the string — and, at the same three, brakes harder out of one.
pub const QUEEN_EXPERT_STEPS: [f32; 3] = [0.5, 1.0 / 3.0, 0.2];
/// The band she holds while lining a charge up.
pub const QUEEN_STANDOFF: (f32, f32) = (300.0, 600.0);
/// How fast she climbs into position while lining up a charge, and her acceleration — Expert
/// Mode's own bonus to each, at the same four thresholds as the charge speed above, is added on
/// top of these base values (unlike the charge speed, whose base itself changes in Expert Mode).
pub const QUEEN_HOVER: f32 = 12.0;
pub const QUEEN_HOVER_ACCEL: f32 = 0.07;
pub const QUEEN_CLIMB_ACCEL_RAGE_EXPERT: [(f32, f32); 4] =
    [(0.75, 0.05), (0.5, 0.05), (0.25, 0.05), (0.1, 0.1)];
/// How far a charge overshoots before it brakes, and Expert Mode's tighter, health-tiered leash
/// on it — Normal mode always uses the flat value below.
pub const QUEEN_CHARGE_LIMIT: f32 = 600.0;
/// Her acceleration while hovering into position to call bees — Expert Mode replaces it outright.
pub const QUEEN_CLIMBING_HOVER_ACCEL_EXPERT: f32 = 0.1;
/// How far above you she hovers to summon, and to sting.
pub const QUEEN_SUMMON_ABOVE: f32 = 200.0;
pub const QUEEN_STING_ABOVE: f32 = 300.0;
/// How often she calls a bee, how fast it leaves, and how many she calls before moving on.
/// Expert Mode's own bonus to the cadence, at the same four thresholds again.
pub const QUEEN_SUMMON_EVERY: f32 = 40.0;
pub const QUEEN_SUMMON_CADENCE_RAGE_EXPERT: [(f32, f32); 4] =
    [(0.75, 0.25), (0.5, 0.25), (0.25, 0.25), (0.1, 0.25)];
pub const QUEEN_BEE_SPEED: f32 = 5.0;
pub const QUEEN_SUMMONS: f32 = 5.0;
/// How often she spits at full health (Normal mode, always) and at a tenth (either mode); Expert
/// Mode's own baseline above half health sits between the two.
pub const QUEEN_STING_EVERY: f32 = 40.0;
pub const QUEEN_STING_EVERY_EXPERT: f32 = 35.0;
pub const QUEEN_STING_EVERY_ENRAGED: f32 = 15.0;
/// Expert Mode's own bonus to stinger speed, and the extra it adds again below a tenth health.
pub const QUEEN_STING_SPEED_EXPERT: f32 = 2.0;
pub const QUEEN_STING_SPEED_EXPERT_ENRAGED: f32 = 3.0;
/// Beyond this she leaves.
pub const QUEEN_GIVE_UP: f32 = 3000.0;
/// Her defence climbs by this much as her health falls away — Expert Mode only; Normal mode's
/// defence never moves from the type table.
pub const QUEEN_DEFENSE_RAMP: f32 = 20.0;

/// What Deerclops' three projectiles hit for; their types are `projectile::ids::DEER_*`.
pub const DEER_SPIKE_DAMAGE: i32 = 13;
pub const DEER_RUBBLE_DAMAGE: i32 = 18;
pub const DEER_SHADOW_DAMAGE: i32 = 15;
/// DEER-1: Expert Mode's passive shadow hands hit softer than the dedicated attack's do
/// (`SpawnPassiveShadowHands`'s `shadowHandDamage = 10`, `NPC.cs:44490`).
pub const DEER_SHADOW_DAMAGE_PASSIVE: i32 = 10;
/// DEER-1: how often the passive hands come, in ticks, at full health and at none. Vanilla
/// `Utils.Remap(lifePercent, 1, 0, 80, 40)` (`NPC.cs:44892`), i.e. `40 + 40 * lifePercent`, so
/// they quicken from every 80 ticks to every 40 as it is worn down; three waves, then a pause.
pub const DEER_PASSIVE_SHADOW_SLOW: f32 = 80.0;
pub const DEER_PASSIVE_SHADOW_FAST: f32 = 40.0;
pub const DEER_PASSIVE_SHADOW_WAVES: f32 = 3.0;
/// BS3-M4: a wave does not hit everybody. `Boss_CanShootExtraAt` (`NPC.cs:47474-47494`) takes each
/// wave's index modulo three and only raises a hand for the players whose own slot matches, so any
/// one player is picked by one wave in three: three waves are one hand each, not three. And it
/// refuses outright past 1200 pixels from the boss, so running away from the fight really does stop
/// the passive rain rather than merely spreading it out.
pub const DEER_PASSIVE_SHADOW_ROTATION: u32 = 3;
pub const DEER_PASSIVE_SHADOW_RANGE: f32 = 1200.0;
/// How far from the player a hostile shadow hand comes up. `RandomizeInsanityShadowFor`'s own
/// `num3 = isHostile ? 200f : 100f` (`Projectile.cs:43187`), used as the radius by all four of its
/// placements. A wider ring gives the player room the game does not.
pub const DEER_PASSIVE_SHADOW_RING: f32 = 200.0;

/// Deerclops' states, as `ai[0]` records them.
pub const DEER_STALKING: f32 = 0.0;
pub const DEER_SPIKES_FORWARD: f32 = 1.0;
pub const DEER_RUBBLE_SLAM: f32 = 2.0;
pub const DEER_ROAR: f32 = 3.0;
pub const DEER_SPIKES_BOTH: f32 = 4.0;
pub const DEER_SHADOW_HANDS: f32 = 5.0;
pub const DEER_GOING_HOME: f32 = 6.0;
pub const DEER_TELEPORTING: f32 = 7.0;
pub const DEER_LEAVING: f32 = 8.0;

/// How long each of its attacks lasts, and the wind-up before the damage lands.
pub const DEER_SPIKES_FORWARD_TICKS: f32 = 80.0;
pub const DEER_SPIKES_FORWARD_WINDUP: f32 = 36.0;
pub const DEER_SPIKES_BOTH_TICKS: f32 = 90.0;
pub const DEER_SPIKES_BOTH_WINDUP: f32 = 56.0;
pub const DEER_RUBBLE_TICKS: f32 = 60.0;
pub const DEER_RUBBLE_WINDUP: f32 = 32.0;
pub const DEER_ROAR_TICKS: f32 = 60.0;
pub const DEER_SHADOW_TICKS: f32 = 60.0;
pub const DEER_SHADOW_AT: f32 = 30.0;
pub const DEER_SHADOW_HANDS_COUNT: usize = 6;
/// How many spikes go up in a line, and how far apart.
pub const DEER_SPIKE_COUNT: i32 = 20;

/// How long it stalks before each attack becomes available.
pub const DEER_UNTIL_RUBBLE: f32 = 240.0;
pub const DEER_UNTIL_SHADOW: f32 = 90.0;
pub const DEER_UNTIL_ROAR: f32 = 120.0;
/// How close it has to be for the spike attacks.
pub const DEER_SPIKE_RANGE: f32 = 120.0;
/// How far it has to be for the roar.
pub const DEER_ROAR_RANGE: f32 = 100.0;
/// How long the roar's Slow lasts, and so how long it holds off the next roar. Vanilla gates the
/// roar on the target not already carrying the Slow buff (`flag13`, `NPC.cs:44653-44654`); the
/// server keeps no queryable player-buff state, so this reproduces that with a cooldown equal to the
/// twelve-second Slow (`ROAR_SLOW_TICKS`) the roar itself applies.
pub const DEER_ROAR_SLOW: f32 = 720.0;

/// Walking speed at full health, and the extra it gains as it is worn down.
pub const DEER_WALK: f32 = 3.5;
pub const DEER_WALK_RAGE: f32 = 1.0;
/// How sharply it changes speed: one quarter of the difference a tick.
pub const DEER_WALK_EASE: f32 = 4.0;
/// It stops pushing once this close.
pub const DEER_STOP_WITHIN: f32 = 80.0;

/// How far a player can get before it goes home, and how far from home counts as its den.
pub const DEER_GIVE_UP: f32 = 2400.0;
pub const DEER_DEN: f32 = 480.0;
/// How long it will spend walking home before simply teleporting there.
pub const DEER_PATIENCE_DEEP: f32 = 300.0;
pub const DEER_PATIENCE: f32 = 1500.0;
pub const DEER_TELEPORT_AT: f32 = 40.0;
/// How long it goes untouchable once its target is well out of reach.
pub const DEER_SHIELD_RANGE: f32 = 450.0;
pub const DEER_SHIELD_AFTER: f32 = 30.0;

/// The Wall of Flesh's parts, and what it spits.
pub const WALL_EYE: u16 = 114;
pub const WALL_HUNGRY: u16 = 115;
pub const WALL_LEECH: u16 = 117;
pub const WALL_IMP: u16 = 24;
/// What the eye's laser (`projectile::ids::WALL_LASER`) does.
pub const WALL_LASER_DAMAGE: i32 = 11;
pub const WALL_LASER_SPEED: f32 = 9.0;

/// How many Hungry hang off it at the start.
pub const WALL_HUNGRY_COUNT: usize = 11;
/// The most leeches it keeps alive at once, and how often it spits one.
pub const WALL_LEECH_CAP: usize = 10;
pub const WALL_LEECH_EVERY: f32 = 60.0;
/// How long before it starts spitting at all, and the extra its wounds buy.
pub const WALL_LEECH_AFTER: f32 = 2700.0;

/// Its walking speed at full health, and what each threshold adds.
pub const WALL_SPEED: f32 = 1.5;
pub const WALL_SPEED_RAGE: [(f32, f32); 4] = [(0.75, 0.25), (0.5, 0.4), (0.25, 0.5), (0.1, 0.6)];
/// Five more thresholds Expert Mode alone crosses, on top of the four above (`aiStyle==27`'s
/// `Main.expertMode` block in `NPC.cs`).
pub const WALL_SPEED_RAGE_EXPERT: [(f32, f32); 5] = [
    (0.66, 0.3),
    (0.33, 0.3),
    (0.05, 0.6),
    (0.035, 0.6),
    (0.025, 0.6),
];
/// Expert Mode's own multiplier and bonus, applied once on top of every threshold above —
/// separate from, and stacking with, get-fixed-boi's pair below.
pub const WALL_EXPERT_SPEED_SCALE: f32 = 1.35;
pub const WALL_EXPERT_SPEED_BONUS: f32 = 0.35;
/// The flat multiplier and bonus get-fixed-boi (`Main.getGoodWorld`) applies on top of that.
///
/// WOF-3: applied only in a For-the-Worthy world now, gated on `Conditions::get_good_world`
/// (`secret_seeds.get_good`), matching vanilla's `Main.getGoodWorld` guard. It used to be
/// unconditional because no gameplay flag tracked the secret seed; the world now carries one.
pub const WALL_SPEED_SCALE: f32 = 1.1;
pub const WALL_SPEED_BONUS: f32 = 0.2;
/// How tall the wall is kept, at least.
pub const WALL_MIN_HEIGHT: f32 = 160.0;
/// How long it takes to fade out once everyone is dead.
pub const WALL_FADE_TICKS: f32 = 180.0;

/// How long an eye charges before its first shot, and between shots in a volley.
pub const WALL_EYE_CHARGE: f32 = 600.0;
pub const WALL_EYE_CADENCE: f32 = 45.0;
/// How many shots are in a volley at full health, and what each threshold adds.
pub const WALL_EYE_VOLLEY: i32 = 4;

/// How far a Hungry will stray from the wall, and how that leash grows as the wall dies.
///
/// Normal Mode only (`NPC.cs:26372-26400`): Expert Mode never applies either of these two
/// overrides, using its own formula below instead.
pub const HUNGRY_LEASH: f32 = 300.0;
pub const HUNGRY_LEASH_WOUNDED: f32 = 500.0;
pub const HUNGRY_LEASH_DYING: f32 = 700.0;
/// Its acceleration and top speed.
pub const HUNGRY_ACCEL: f32 = 0.1;
pub const HUNGRY_SPEED: f32 = 4.0;
/// How long a hit knocks it out of its chase.
pub const HUNGRY_RECOIL: f32 = 10.0;
/// The 30/20 defence it takes on below 50%/75% wall health — Normal Mode only. Expert Mode's own
/// `if (Main.expertMode) { defense = defDefense; ... }` (`NPC.cs:26406-26408`) runs unconditionally
/// afterward and discards it, reverting to the type's own baseline regardless of health.
pub const HUNGRY_DEFENSE_DYING: i32 = 30;
pub const HUNGRY_DEFENSE_WOUNDED: i32 = 20;
/// Expert Mode's own acceleration bonus at the same two thresholds where Normal Mode instead
/// lengthens the leash above — Expert Mode leaves the leash at its own formula below and speeds
/// the pull up instead (`NPC.cs:26380-26400`, `num414 += ...`).
pub const HUNGRY_EXPERT_ACCEL_DYING: f32 = 0.066;
pub const HUNGRY_EXPERT_ACCEL_WOUNDED: f32 = 0.033;
/// Expert Mode's own leash: not the health-tiered range above at all, but `HUNGRY_LEASH` times a
/// multiplier keyed to which of the world's live NPC slots this particular Hungry occupies
/// (`NPC.cs:26406-26430`, `whoAmI % 4` then `whoAmI % 3`, applied in turn) and then this flat
/// scale — twelve distinct multipliers in all, since 4 and 3 are coprime.
pub const HUNGRY_EXPERT_LEASH_SCALE: f32 = 0.75;
/// Expert Mode's own top-speed bonus (`NPC.cs:26488-26520`): unconditional whenever Expert Mode is
/// on, and larger again at each of the wall's own four health thresholds — a separate table from
/// every other health-tiered scaling in this file, keyed to the *wall's* health, not this NPC's
/// own.
pub const HUNGRY_EXPERT_SPEED_RAGE: [(f32, f32); 4] =
    [(0.75, 0.7), (0.5, 0.7), (0.25, 0.9), (0.1, 0.9)];
pub const HUNGRY_EXPERT_SPEED_BASE: f32 = 1.5;
pub const HUNGRY_EXPERT_SPEED_SCALE: f32 = 1.25;
pub const HUNGRY_EXPERT_SPEED_BONUS: f32 = 0.3;
pub const HUNGRY_EXPERT_SPEED_FACTOR: f32 = 0.35;
/// A further flat bonus while this Hungry sits behind the wall relative to the way the wall
/// itself is moving, so it can catch back up rather than being left behind as the wall advances.
pub const HUNGRY_EXPERT_SPEED_CATCHUP: f32 = 6.0;

/// The Eater of Worlds' three parts, in order: head, body, tail.
pub const EATER_OF_WORLDS: (u16, u16, u16) = (13, 14, 15);

/// The head, body and tail of a worm type, if it splits when cut.
///
/// Only the Eater of Worlds does. Every other worm in the game dies as one animal: sever a giant
/// worm and the pieces simply vanish.
pub fn splitting_worm(npc_type: u16) -> Option<(u16, u16, u16)> {
    matches!(npc_type, 13..=15).then_some(EATER_OF_WORLDS)
}

/// A worm-headed type's own body, tail and real segment count, if `head_type` is one — for the
/// types created all at once by a spawn-time path (the admin `/spawn` command,
/// `summon_on_player`). The Solar Crawltipede needs the same idea but is deliberately not here —
/// see [`SOLAR_CRAWLTIPEDE_HEAD`]'s own doc comment for why.
///
/// A worm head spawned alone is a floating face: the body has to be created alongside it, every
/// segment linked to the one ahead. Covers the four ordinary worm monsters `NPC.cs`'s own spawn
/// code gives 20/8/6/12 segments respectively, and the Destroyer (`DESTROYER_SEGMENTS`).
pub fn worm_body(head_type: u16) -> Option<(u16, u16, usize)> {
    match head_type {
        13 => Some((14, 15, 20)), // Eater of Worlds
        7 => Some((8, 9, 8)),     // Devourer
        10 => Some((11, 12, 6)),  // Giant Worm
        39 => Some((40, 41, 12)), // Bone Serpent
        DESTROYER_HEAD => Some((DESTROYER_BODY, DESTROYER_TAIL, DESTROYER_SEGMENTS)),
        _ => None,
    }
}

/// The Solar Crawltipede: a Solar Pillar escort with the same "head grows its own body on its own
/// first AI tick" mechanism as the Destroyer (`NPC.cs:51913-51936`, `ai[0]==0 && type==412`, 30
/// trailing segments — the loop there is exclusive, `num36 < 30`, so 30 really is the total). Kept
/// out of [`worm_body`] deliberately: that table is only consulted by spawn-time paths
/// (`summon_on_player`, the admin `/spawn` command), but a Crawltipede head can also appear from
/// this project's own ambient hostile spawning during the Lunar Apocalypse, which neither of those
/// paths sees — the tick-loop check this constant feeds (`game/server.rs`) catches every path
/// uniformly, the same way real vanilla's own AI-driven self-growth does.
pub const SOLAR_CRAWLTIPEDE_HEAD: u16 = 412;

/// The Wyvern's head, and the fourteen parts that trail it.
///
/// Same mechanism as the Crawltipede and kept out of [`worm_body`] for the same reason: a Wyvern
/// grows its own body on its own first AI tick (`NPC.cs:51700-51730`, `type == 87 && ai[0] == 0f`),
/// so it has to work for a head that arrives from ambient sky spawning as well as from `/spawn`.
///
/// The order is the game's own loop: `num14 = 89` by default, `88` at `m == 1` and `m == 8`, then
/// `90`, `91`, `92` at 11, 12 and 13. It is not a run of one body type and a tail, which is why it
/// is a table rather than a count.
pub const WYVERN_HEAD: u16 = 87;
pub const WYVERN_SEGMENTS: [u16; 14] = [89, 88, 89, 89, 89, 89, 89, 89, 88, 89, 89, 90, 91, 92];

/// The two desert worms, and the body, tail and inclusive segment range each grows for itself on its
/// own first AI tick (`NPC.cs:51772-51794` for the Tomb Crawler, `:51796-51819` for the Dune
/// Splicer, both `type == T && ai[0] == 0f`).
///
/// Same mechanism as [`WYVERN_HEAD`] and [`SOLAR_CRAWLTIPEDE_HEAD`], and out of [`worm_body`] for
/// the same reason: both arrive from ambient underground-desert and sandstorm spawning, which no
/// spawn-time path sees. Unlike those two the count is rolled rather than fixed, so the range comes
/// back and the caller draws inside it: `Main.rand.Next(12, 21)` and `Main.rand.Next(6, 10)`, whose
/// upper bounds are exclusive in the game and inclusive here.
pub fn self_growing_sand_worm(head_type: u16) -> Option<(u16, u16, usize, usize)> {
    match head_type {
        510 => Some((511, 512, 12, 20)), // DuneSplicer
        513 => Some((514, 515, 6, 9)),   // TombCrawler
        _ => None,
    }
}
pub const SOLAR_CRAWLTIPEDE_BODY: u16 = 413;
pub const SOLAR_CRAWLTIPEDE_TAIL: u16 = 414;
pub const SOLAR_CRAWLTIPEDE_SEGMENTS: usize = 30;

/// How long a Mothron egg takes to hatch, and what it hatches into. Expert Mode halves the wait
/// (but also only sets it back once when hit, rather than twice — the two together are not as
/// lopsided a change as the raw tick counts alone suggest).
pub const MOTHRON_EGG_TICKS: f32 = 900.0;
pub const MOTHRON_EGG_TICKS_EXPERT: f32 = 600.0;
pub const MOTHRON_SPAWN: u16 = 479;

/// How long a stardust cell takes to grow up, and what into.
pub const STARDUST_CELL_TICKS: f32 = 300.0;
pub const STARDUST_CELL_GROWN: u16 = 405;

/// A dungeon spirit's chase speed and how heavily it is smoothed.
pub const SPIRIT_SPEED: f32 = 12.0;
pub const SPIRIT_SMOOTH: f32 = 100.0;

/// A flocko's charge speed, the range at which it commits, and how long it spins afterwards.
pub const FLOCKO_SPEED: f32 = 11.0;
pub const FLOCKO_RANGE: f32 = 200.0;
pub const FLOCKO_SPIN_TICKS: f32 = 20.0;

/// The big stardust jellyfish: how far above its target it hangs, its speed, and its cadence.
pub const JELLYFISH_ABOVE: f32 = 250.0;
pub const JELLYFISH_SPEED: f32 = 5.0;
pub const JELLYFISH_EASE: f32 = 0.15;
pub const JELLYFISH_EVERY: f32 = 70.0;
pub const JELLYFISH_SHOT_DAMAGE: i32 = 60;

/// Solar goop: how long it sits before it dries up once it has landed.
pub const GOOP_SETTLE_TICKS: f32 = 5.0;

/// An elf copter's hover, and the missile it drops.
pub const COPTER_SPEED: f32 = 7.0;
pub const COPTER_RANGE: f32 = 600.0;
pub const COPTER_RELOAD: f32 = 15.0;
pub const COPTER_SHOT_DAMAGE: i32 = 32;
pub const COPTER_SHOT_SPEED: f32 = 10.0;

/// An angry nimbus: how far above you it sits and how often it rains.
pub const NIMBUS_ABOVE: f32 = 200.0;
pub const NIMBUS_SPEED: f32 = 4.0;
pub const NIMBUS_ACCEL: f32 = 0.25;
pub const NIMBUS_EVERY: f32 = 8.0;
pub const NIMBUS_SHOT_DAMAGE: i32 = 20;

/// A detonating bubble: how long it drifts before it goes off, and how near you have to be to
/// trigger it early.
pub const BUBBLE_FUSE: f32 = 150.0;
pub const BUBBLE_TRIGGER: f32 = 40.0;
pub const BUBBLE_BLAST_TICKS: f32 = 4.0;
pub const BUBBLE_BLAST_SIZE: i32 = 100;

/// A flying weapon's charge speed, and how long it rests between charges.
pub const FLYING_WEAPON_SPEED: f32 = 9.0;
pub const FLYING_WEAPON_DRIVE: f32 = 100.0;
pub const FLYING_WEAPON_REST: f32 = 120.0;

/// An ancient doom: how long it grows before it fires, how fast it grows once its parent is hurt,
/// and the four shots it lets go.
pub const DOOM_LIFETIME: f32 = 420.0;
pub const DOOM_FADE_IN: f32 = 120.0;
pub const DOOM_SHOT_SPEED: f32 = 4.0;

/// A water strider: how hard it pushes off the surface, and how long it waits between skips.
pub const STRIDER_RISE: f32 = 0.8;
pub const STRIDER_RISE_CAP: f32 = 4.0;
pub const STRIDER_SKIP: f32 = 5.0;
pub const STRIDER_WAIT: (u32, u32) = (120, 241);
pub const STRIDER_WAIT_DRY: (u32, u32) = (60, 241);

// --- Fixtures: the tethered, stationary and short-lived hardmode NPCs -------------------------

/// Style 124: an elder slime chest is a falling prop and nothing else.
pub const CHEST_GRAVITY: f32 = 0.2;

/// Style 122: a pirate ghost closes at four pixels a tick, easing by two fifteenths.
pub const GHOST_SPEED: f32 = 4.0;
pub const GHOST_EASE: f32 = 2.0 / 15.0;
/// It fades in and out five steps of alpha at a time, and dies once fully faded.
pub const GHOST_FADE: i32 = 5;
/// Ghosts closer than this shove each other apart.
pub const GHOST_PERSONAL_SPACE: f32 = 50.0;
pub const GHOST_SHOVE: f32 = 0.1;

/// Style 92: a training dummy gives up on a player further away than this.
pub const DUMMY_RANGE: f32 = 4800.0;
/// The tile a dummy is anchored to; without it there is nothing holding it up.
pub const DUMMY_TILE: u16 = 378;

/// The Martian turret, the one style-73 type that spends two seconds deploying before it fires.
pub const MARTIAN_TURRET: u16 = 387;
/// The force bubble a Martian turret is pinned to; without it the turret has nothing to sit on.
pub const FORCE_BUBBLE: u16 = 386;

/// Style 73: how long a turret waits between casts, and how far the shot is thrown off.
pub const CASTER_RELOAD: f32 = 60.0;
pub const CASTER_COOLDOWN: f32 = -120.0;
/// Being hit interrupts the cast and costs half a reload.
pub const CASTER_FLINCH: f32 = -30.0;
pub const CASTER_SPREAD: i32 = 100;
pub const CASTER_SHOT_SPEED: f32 = 14.0;
pub const CASTER_SHOT_DAMAGE: i32 = 35;
/// The friction a stationary caster settles under.
pub const CASTER_DRAG: f32 = 0.93;
/// Type 387 spends this long materialising, untouchable, before it will fight.
pub const CASTER_ARRIVAL: f32 = 120.0;
pub const CASTER_FADE_IN: f32 = 60.0;

/// Style 127: the two "pals" wait for their escort to die, then two seconds more before paying out.
///
/// `AI_127_Pal` (`NPC.cs:43379-43478`) is the whole encounter: on its first tick it looks for two
/// bits of floor either side of itself, dies outright if there are none, and otherwise raises the
/// two Goblin Archers that stand over it. Only when both are gone will it let anybody near, and
/// two seconds after that it hands over the pet and goes.
pub const PAL_CATTIVA: u16 = 695;
pub const PAL_FOXSPARKS: u16 = 696;
pub const PAL_ESCORT: u16 = 111;
pub const PAL_ESCORTS: usize = 2;
pub const PAL_APPROACH: f32 = 100.0;
pub const PAL_PAYOUT_TICKS: f32 = 120.0;
pub const PAL_DRAG: f32 = 0.93;
/// What it hands over: `AI_127_Pal_GiveRewerd` (`NPC.cs:43481-43489`), one Palworld Minion item
/// matched to which of the pair it was (`ItemID.cs:12872`, `:12874`).
pub const PAL_REWARD_CATTIVA: i16 = 5663;
pub const PAL_REWARD_FOXSPARKS: i16 = 5664;
/// Where the two escorts stand: `CheckFloor2` walks the columns six tiles either side of the pal
/// (`CultistRitual.cs:135-140`, `i` of -3 and 3 with `num2 = point.X + i * 2`) and the archer is
/// dropped three tiles above the floor it found (`NPC.cs:43400`, `spawnPoints[i].Y * 16 - 48`).
pub const PAL_ESCORT_COLUMN: i32 = 6;
pub const PAL_ESCORT_LIFT: f32 = 48.0;

/// The other half of the encounter, in `AI_003_Fighters` rather than here: a Goblin Archer with a
/// negative `ai[3]` is one of a pal's two guards and stands over it instead of hunting
/// (`NPC.cs:57553-57592`). It wakes on being hit or on anybody coming within this many pixels, and
/// while it waits `ai[0]` cycles through an idle band.
pub const PAL_ESCORT_WAKE: f32 = 200.0;
pub const PAL_ESCORT_IDLE_FROM: f32 = 1000.0;
pub const PAL_ESCORT_IDLE_TO: f32 = 1300.0;

// --- The Martian invasion and its scout -------------------------------------------------------

/// Style 80: how far below itself a probe looks for ground, and the two bands it steers between.
pub const PROBE_SCAN: i32 = 30;
pub const PROBE_TOO_LOW: i32 = 15;
pub const PROBE_COMFORTABLE: i32 = 20;
pub const PROBE_CRUISE: f32 = 3.0;
pub const PROBE_CLIMB: f32 = 0.05;
pub const PROBE_CLIMB_CAP: f32 = 3.5;
pub const PROBE_SINK_CAP: f32 = 1.5;
/// It only counts a player it can see below it, within this far.
pub const PROBE_SIGHT: f32 = 352.0;
/// Then it hangs still for a second before bolting.
pub const PROBE_ALERT_TICKS: f32 = 60.0;
pub const PROBE_ESCAPE_TICKS: f32 = 180.0;
pub const PROBE_ESCAPE_CLIMB: f32 = 0.1;
pub const PROBE_ESCAPE_CLIMB_CAP: f32 = 10.0;
pub const PROBE_ESCAPE_DRIFT: f32 = 0.05;
pub const PROBE_ESCAPE_DRIFT_CAP: f32 = 4.0;

/// Style 93: the Flying Dutchman's four cannon, and how it holds station above the ground.
pub const DUTCHMAN_GUN: u16 = 492;
pub const DUTCHMAN_CANNON: usize = 4;
pub const DUTCHMAN_CANNON_SPACING: f32 = 40.0;
pub const DUTCHMAN_CANNON_OFFSET: f32 = 150.0;
/// It aims to fly this high, and corrects while it is outside the band.
pub const DUTCHMAN_HOVER: f32 = 350.0;
pub const DUTCHMAN_HOVER_SLACK: f32 = 100.0;
pub const DUTCHMAN_HOVER_EASE: f32 = 0.05;
pub const DUTCHMAN_HOVER_RATE: f32 = 4.0;
pub const DUTCHMAN_GROUND_SCAN: i32 = 150;
/// It closes only when the player is further away than this, and never past this speed.
pub const DUTCHMAN_STANDOFF: f32 = 300.0;
pub const DUTCHMAN_SPEED: f32 = 6.0;
pub const DUTCHMAN_ACCEL: f32 = 0.06;
/// Every so often it drops one of the invasion's foot soldiers.
pub const DUTCHMAN_DROP_CHANCE: u32 = 300;
pub const DUTCHMAN_DROPS: [u16; 4] = [213, 215, 214, 212];
pub const DUTCHMAN_DROP_RISE: f32 = -8.01;

/// The Snowman Gangsta, which only wakes up during the Frost Moon.
pub const SNOWMAN_GANGSTA: u16 = 341;

/// Style 25: a hopper's two-beat jump — a long low one, then a high one.
pub const HOP_REST: f32 = 20.0;
pub const HOP_FIRST_REST: f32 = 12.0;
pub const HOP_LONG: (f32, f32) = (3.5, -4.0);
pub const HOP_HIGH: (f32, f32) = (2.5, -8.0);
/// It wakes when a player comes this close to its box, or when anything hurts it.
pub const HOP_WAKE_MARGIN: f32 = 100.0;
/// Airborne it leans into its direction, gently, up to this.
pub const HOP_LEAN: f32 = 0.1;
pub const HOP_LEAN_CAP: f32 = 1.0;
/// Grounded and waiting, it sheds speed at this rate.
pub const HOP_DRAG: f32 = 0.9;

// --- Wall crawlers and leapers -----------------------------------------------------------------

/// Style 40: how fast each wall-crawling form homes, and how hard it accelerates.
///
/// The jungle creeper and desert scorpion are quicker than the rest; everything else shares the
/// base pair.
pub const CRAWLER_SPEED: f32 = 2.0;
pub const CRAWLER_ACCEL: f32 = 0.08;
pub const JUNGLE_CRAWLER_SPEED: f32 = 3.0;
pub const JUNGLE_CRAWLER_ACCEL: f32 = 0.12;
pub const SCORPION_CRAWLER_SPEED: f32 = 4.0;
pub const SCORPION_CRAWLER_ACCEL: f32 = 0.16;
/// Blind, it wanders on this ramp, turning over at these bounds.
pub const CRAWLER_WANDER: f32 = 0.023;
/// The sideways ramp switches over at a hundred either way, so a full cycle pushes left as long as
/// it pushes right and the wander itself goes nowhere; the lean toward the player is what moves it.
pub const CRAWLER_WANDER_BAND: f32 = 100.0;
/// The cycle runs to two hundred and then starts again from the other end.
pub const CRAWLER_WANDER_TURN: f32 = 200.0;
pub const CRAWLER_PULL: f32 = 0.007;
pub const CRAWLER_DRIFT_BRAKE: f32 = 1.5;
pub const CRAWLER_DRIFT_CAP: f32 = 3.0;
/// It rebounds off terrain at half the speed it hit at, but never slower than this.
pub const CRAWLER_BOUNCE: f32 = 0.5;
pub const CRAWLER_BOUNCE_FLOOR: f32 = 2.0;
/// In expert it spits web (`projectile::ids::CRAWLER_SPIT`), on a random fuse, and being hit sets
/// the fuse back.
pub const CRAWLER_SPIT_DAMAGE: i32 = 18;
pub const CRAWLER_SPIT_SPEED: f32 = 8.0;
/// The wall forms and the ground forms they turn into once they have room.
pub const CRAWLER_FORMS: [(u16, u16); 5] =
    [(165, 164), (237, 236), (238, 163), (240, 239), (531, 530)];

/// Style 41: a leaper's charge-up, which counts *faster* the closer you are.
pub const LEAPER_CHARGE: f32 = 5.0;
pub const DERPLING_CHARGE: f32 = 2.0;
pub const LEAPER_URGENCY: f32 = 400.0;
pub const LEAPER_URGENCY_SCALE: f32 = 10.0;
pub const DERPLING_URGENCY_SCALE: f32 = 5.0;
pub const LEAPER_URGENCY_CAP: f32 = 30.0;
/// The small hop, then the big one that ends the set.
pub const LEAPER_HOP: (f32, f32) = (5.0, -5.0);
pub const LEAPER_LEAP: (f32, f32) = (3.0, -9.0);
pub const LEAPER_HOPS_BEFORE_LEAP: f32 = 3.0;
pub const DERPLING_HOP: (f32, f32) = (4.0, -7.5);
pub const DERPLING_LEAP: (f32, f32) = (2.0, -11.5);
pub const DERPLING_HOPS_BEFORE_LEAP: f32 = 2.0;
/// The rest after a hop, and the longer one after a leap.
pub const LEAPER_REST: f32 = -120.0;
pub const LEAPER_LONG_REST: f32 = -200.0;
/// Stuck against a wall, it turns round and waits this long.
pub const LEAPER_STUCK_WAIT: f32 = 300.0;
/// Airborne it leans into its direction up to this, gently.
pub const LEAPER_AIR_LEAN: f32 = 0.2;
pub const LEAPER_AIR_CAP: f32 = 3.0;
pub const DERPLING_AIR_CAP: f32 = 4.0;
/// The chattering teeth bomb, which is a leaper that detonates instead of biting.
pub const TEETH_BOMB: u16 = 378;
pub const TEETH_FUSE: f32 = 10.0;
pub const TEETH_TRIGGER: f32 = 64.0;
pub const TEETH_BLAST_SIZE: f32 = 160.0;
/// The herpling, whose numbers differ from the derpling's throughout.
pub const DERPLING: u16 = 177;

// --- Rollers: tortoises, shellies and the solar sroller ----------------------------------------

/// Style 39: the charge meter, which fills faster the further away and the clearer the shot.
pub const ROLLER_READY: f32 = 400.0;
pub const ROLLER_SEEN_BONUS: f32 = 4.0;
pub const ROLLER_FAR_BONUS: f32 = 10.0;
pub const SHELLY_SEEN_BONUS: f32 = 2.0;
pub const SHELLY_FAR_BONUS: f32 = 4.0;
pub const ROLLER_SEEN_RANGE: f32 = 200.0;
pub const ROLLER_FAR_RANGE: f32 = 600.0;
/// Getting wet makes one charge immediately: it will not swim.
pub const ROLLER_WET_READY: f32 = 1000.0;
/// Patrol speeds — slower while you are close, quicker once you are not.
pub const ROLLER_WALK_NEAR: f32 = 1.0;
pub const ROLLER_WALK_FAR: f32 = 1.5;
pub const ROLLER_WALK_ACCEL: f32 = 0.07;
pub const ROLLER_NEAR_RANGE: f32 = 400.0;
pub const SHELLY_WALK: f32 = 0.5;
/// Winding up, then rolling, then slowing, then standing back up.
pub const ROLLER_WINDUP: f32 = 30.0;
pub const ROLLER_ROLL_TICKS: f32 = 90.0;
pub const ROLLER_STANDUP: f32 = 30.0;
/// A rolling tortoise hits twice as hard and takes half as much.
pub const ROLLER_ROLL_DAMAGE: f32 = 2.0;
pub const SHELLY_ROLL_DAMAGE: f32 = 1.5;
pub const ROLLER_ROLL_DEFENSE: i32 = 2;
/// How fast it rolls, and how much slower when it cannot see where you are.
pub const ROLLER_ROLL_SPEED: f32 = 10.0;
pub const ROLLER_BLIND_SPEED: f32 = 6.0;
pub const SHELLY_ROLL_SCALE: f32 = 0.75;
/// A roll aims slightly above you, by a fifth of the horizontal gap.
pub const ROLLER_LEAD: f32 = 0.2;
/// And climbs as it goes, which is what makes one come up a slope at you.
pub const ROLLER_CLIMB: f32 = 0.22;
pub const ROLLER_SPIN: f32 = 0.3;
pub const ROLLER_SPIN_DECAY: f32 = 0.01;
/// The solar sroller, which bounces several times instead of rolling once.
pub const SOLAR_SROLLER: u16 = 417;
pub const SROLLER_SPEED: f32 = 16.0;
pub const SROLLER_BLIND_SPEED: f32 = 10.0;
pub const SROLLER_DAMAGE: f32 = 1.8;
pub const SROLLER_BOUNCE_LIMIT: f32 = 1200.0;
/// Curled up it is shorter, and it stands back up to its full height afterwards.
pub const SROLLER_CURLED_HEIGHT: f32 = 32.0;
pub const SROLLER_STANDING_HEIGHT: f32 = 52.0;
/// The two shellies, which are slower and gentler than the tortoises throughout.
pub const SHELLY_FIRST: u16 = 496;
pub const SHELLY_LAST: u16 = 497;

// --- The sand: sharks that swim through it, and the elemental that raises it -------------------

/// Style 103: a sand shark only swims where there is sand — or water — to swim in.
pub const SHARK_SWIM_SPEED: f32 = 6.0;
pub const SHARK_SWIM_ACCEL: f32 = 0.1;
pub const SHARK_BOB: f32 = 0.06;
pub const SHARK_BOB_ACCEL: f32 = 0.01;
/// Out of the sand it walks, badly, and falls.
pub const SHARK_BEACHED_SPEED: f32 = 1.0;
pub const SHARK_GRAVITY: f32 = 0.3;
pub const SHARK_FALL_CAP: f32 = 10.0;
/// The lunge: it breaks the surface at this speed when it has a clear run at you.
pub const SHARK_LUNGE_SPEED: f32 = 12.0;
pub const SHARK_LUNGE_RANGE: f32 = 400.0;
pub const SHARK_LUNGE_ARC: f32 = -80.0;
pub const SHARK_LUNGE_COOLDOWN: f32 = -30.0;
pub const SHARK_LUNGE_READY: f32 = 30.0;
/// Homing while submerged is slower vertically than horizontally.
pub const SHARK_HOME_ACCEL: f32 = 0.15;
pub const SHARK_HOME_X: f32 = 5.0;
pub const SHARK_HOME_Y: f32 = 3.0;
/// It will only commit to a lunge at a player who is not already falling onto it.
pub const SHARK_MIN_RANGE: f32 = 150.0;

/// Style 102: how a sand elemental drifts, and how it raises sandnadoes.
pub const ELEMENTAL: u16 = 541;
pub const ELEMENTAL_GRAVITY: f32 = 0.1;
pub const ELEMENTAL_FALL_CAP: f32 = 2.0;
pub const ELEMENTAL_RISE: f32 = -0.1;
pub const ELEMENTAL_RISE_CAP: f32 = -4.0;
pub const ELEMENTAL_SPEED: f32 = 2.0;
pub const ELEMENTAL_ACCEL: f32 = 0.1;
pub const ELEMENTAL_CLIMB_SPEED: f32 = 1.0;
pub const ELEMENTAL_CLIMB_ACCEL: f32 = 0.04;
/// A wounded one moves faster: full health adds nothing, an almost-dead one adds all of this.
pub const ELEMENTAL_WOUNDED_SPEED: f32 = 2.0;
pub const ELEMENTAL_WOUNDED_ACCEL: f32 = 0.02;
/// Below half it cannot be knocked back at all.
pub const ELEMENTAL_STUBBORN_AT: f32 = 0.5;
/// The cast: it holds still from the moment it starts until a hundred and thirty-five ticks later,
/// raising the sandnadoes on tick fifty-four.
pub const ELEMENTAL_CAST_TICKS: f32 = 135.0;
pub const ELEMENTAL_CAST_AT: f32 = 54.0;
pub const ELEMENTAL_CAST_REST: f32 = -300.0;
pub const ELEMENTAL_MISS_REST: f32 = -200.0;
pub const ELEMENTAL_CAST_RANGE: f32 = 900.0;
pub const ELEMENTAL_DRAG: f32 = 0.96;
pub const SANDNADOES: usize = 3;
/// They are raised within this many tiles of where the player is heading, and no two within ten.
pub const SANDNADO_SPREAD: i32 = 30;
pub const SANDNADO_APART: i32 = 10;
/// The cast leads the player by a second of their own movement.
pub const SANDNADO_LEAD: f32 = 30.0;

/// How long an NPC has to make no progress before it gives up and turns round. Used by the
/// drifting hunters, which have no ground contact to tell them they are against a wall.
pub const STUCK_TURN_TICKS: f32 = 60.0;
pub const STUCK_TURN_REST: f32 = -180.0;
pub const STUCK_TOLERANCE: f32 = 16.0;

/// The sand sharks and their variants, which pass through sand rather than colliding with it.
pub const SANDSHARK_FIRST: u16 = 542;
pub const SANDSHARK_LAST: u16 = 545;

/// Whether an NPC of this type moves through a tile as though it were not there.
///
/// Almost every NPC either collides with all solid tiles or with none, and the two flags on
/// `NpcStats` cover that. The sand sharks are the exception the game itself carves out: they use a
/// collision routine that ignores sand, hardened sand, sandstone and desert fossil, which is what
/// lets one swim through a dune and still be stopped by the stone underneath it.
pub fn phases_through(npc_type: u16, block: u16) -> bool {
    if !(SANDSHARK_FIRST..=SANDSHARK_LAST).contains(&npc_type) {
        return false;
    }
    // `TileID.Sets.ForAdvancedCollision.ForSandshark`: the three sand families plus desert fossil
    // and its hardened form.
    crate::tile_sets::sandy(block) || matches!(block, 404 | 407)
}

// --- Fireflies and their kin --------------------------------------------------------------------

/// Style 64: how long a firefly holds a heading before choosing another.
pub const FIREFLY_HOLD: (u32, u32) = (60, 180);
/// Each one is a slightly different size, fixed when it hatches.
pub const FIREFLY_SCALE: (u32, u32) = (75, 111);
/// Headings are eased in over eighty ticks, which is what makes the drift look weightless.
pub const FIREFLY_SMOOTH: f32 = 80.0;
/// Far from anybody it drifts *toward* them, faster the further off it is.
pub const FIREFLY_SEEK_AT: f32 = 700.0;
pub const FIREFLY_SEEK_SPEED: (u32, u32) = (50, 151);
pub const FIREFLY_SEEK_FAR: f32 = 850.0;
pub const FIREFLY_SEEK_FAR_SPEED: (u32, u32) = (100, 151);
pub const FIREFLY_SEEK_FARTHER: f32 = 1000.0;
pub const FIREFLY_SEEK_FARTHER_SPEED: (u32, u32) = (150, 201);
/// Once it has arrived it wanders instead, and never goes back to seeking.
pub const FIREFLY_WANDER_SPEED: (u32, u32) = (5, 151);
/// It will not fly into the ground, nor away from it: four tiles below is too close and thirty
/// tiles of nothing below is too high.
pub const FIREFLY_FLOOR_LOOK: i32 = 4;
pub const FIREFLY_SKY_LOOK: i32 = 30;
/// The glow, which only happens at night or underground.
pub const FIREFLY_DARK_BELOW: f32 = 10.0;
pub const FIREFLY_GLOW_GAP: (u32, u32) = (30, 180);
pub const FIREFLY_GLOW_FOR: (u32, u32) = (10, 30);
/// The shimmerfly, which keeps clear of the world edges and of anything alive.
pub const SHIMMERFLY: u16 = 677;
pub const SHIMMERFLY_MARGIN: i32 = 40;
pub const SHIMMERFLY_EDGE_PUSH: f32 = 0.5;
pub const SHIMMERFLY_EDGE_CAP: f32 = 3.0;
pub const SHIMMERFLY_CHECK_EVERY: f32 = 15.0;
pub const SHIMMERFLY_SHY_OF_NPCS: f32 = 100.0;
pub const SHIMMERFLY_SHY_OF_PLAYERS: f32 = 150.0;
pub const SHIMMERFLY_BOLT: f32 = 2.0;
pub const SHIMMERFLY_BOLT_CAP: f32 = 8.0;

// --- Waterfowl ----------------------------------------------------------------------------------

/// Style 68: how fast a duck paddles, and how far above the waterline it floats.
pub const DUCK_PADDLE: f32 = 2.0;
pub const DUCK_FLOAT_ABOVE: f32 = 6.0;
pub const DUCK_SURFACE_CLIMB: f32 = 0.1;
pub const DUCK_SURFACE_CLIMB_CAP: f32 = -8.0;
/// A player this close to its box, or any injury, puts it up.
pub const DUCK_STARTLE: f32 = 100.0;
pub const DUCK_TAKEOFF: f32 = -6.0;
/// It flies for five seconds and then looks for somewhere to come down.
pub const DUCK_FLIGHT_TICKS: f32 = 300.0;
pub const DUCK_FLY_SPEED: f32 = 3.0;
pub const DUCK_FLY_ACCEL: f32 = 0.1;
pub const DUCK_CLIMB: f32 = 0.1;
pub const DUCK_CLIMB_CAP: f32 = -4.0;
pub const DUCK_SINK_CAP: f32 = 3.0;
/// It looks fifteen tiles down for somewhere to be, and panics if the ground is within five.
pub const DUCK_LOOK_DOWN: i32 = 15;
pub const DUCK_TOO_CLOSE: i32 = 5;
/// Landing on dry ground turns a flying duck back into a walking one.
pub const DUCK_LANDS_AS: [u16; 4] = [363, 365, 603, 609];
pub const DUCK_LANDED_REST: (u32, u32) = (200, 400);

// --- Chargers: the things that line you up and then throw themselves at you --------------------

/// Style 74's numbers, which differ enough between the two types to be a table rather than a pair
/// of branches.
#[derive(Debug, Clone, Copy)]
pub struct Charge {
    /// How fast it drifts while it is choosing a moment.
    pub approach: f32,
    /// How far above you it prefers to sit, and how far to the side.
    pub above: f32,
    pub beside: f32,
    /// It will not commit closer than this, nor further than this.
    pub too_close: f32,
    pub too_far: f32,
    /// How heavily the drift is smoothed, and how long the wind-up lasts.
    pub drift_smooth: f32,
    pub windup: f32,
    /// How much speed it keeps each tick of the wind-up.
    pub windup_drag: f32,
    /// Aim scatter, in hundredths of a pixel per tick.
    pub scatter: i32,
    /// Launch speed, and how long the dash may last.
    pub dash: f32,
    pub dash_ticks: f32,
    /// It only breaks off once it is this far past you and below you.
    pub break_off: f32,
    /// How hard it keeps steering during the dash, and how much it accelerates.
    pub steer: f32,
    pub steer_gain: f32,
    /// Below this speed the dash is spent.
    pub spent_below: f32,
    /// Knockback it takes while drifting; during a dash it takes none.
    pub knockback: f32,
    /// Whether hitting something makes it go off.
    pub explodes: bool,
}

/// The Martian drone: it explodes on contact and on terrain.
pub const MARTIAN_DRONE: u16 = 388;
/// The solar corite: it bounces off and recovers rather than detonating.
pub const SOLAR_CORITE: u16 = 418;

pub const DRONE_CHARGE: Charge = Charge {
    approach: 10.0,
    above: 200.0,
    beside: 0.0,
    too_close: 0.0,
    too_far: 750.0,
    drift_smooth: 30.0,
    windup: 30.0,
    windup_drag: 0.95,
    scatter: 50,
    dash: 14.0,
    dash_ticks: 30.0,
    break_off: 100.0,
    steer: 20.0,
    steer_gain: 0.0,
    spent_below: 7.0,
    knockback: 0.4,
    explodes: true,
};

pub const CORITE_CHARGE: Charge = Charge {
    approach: 8.0,
    above: 175.0,
    beside: 175.0,
    too_close: 80.0,
    too_far: 600.0,
    drift_smooth: 60.0,
    windup: 20.0,
    windup_drag: 0.75,
    scatter: 0,
    dash: 9.0,
    dash_ticks: 30.0,
    break_off: 150.0,
    steer: 60.0,
    steer_gain: 4.0 / 15.0,
    spent_below: 7.0,
    knockback: 0.3,
    explodes: false,
};

/// A drone's blast: it swells to this across, hits this hard, and is gone three ticks later.
pub const DRONE_BLAST_SIZE: f32 = 192.0;
pub const DRONE_BLAST_DAMAGE: i32 = 80;
pub const DRONE_BLAST_TICKS: f32 = 3.0;
/// It goes off on contact at this range as well as on terrain.
pub const DRONE_TOUCH: f32 = 64.0;
/// A corite that has finished a dash rests for this long, counting down three at a time.
pub const CORITE_REST: f32 = 45.0;
/// Blocked for two seconds, a charger commits anyway rather than circling forever.
pub const CHARGE_PATIENCE: f32 = 120.0;
/// It will not dash at a target more than this far off the horizontal, in eighths of a half-turn.
pub const CHARGE_ARC: f32 = 8.0;

// --- Riders: the parts that sit on something else ---------------------------------------------

/// Style 75: where each riding part sits on its mount, before the mount's rotation and scale are
/// applied.
///
/// The offsets are in pixels from the mount's centre. `mirrored` marks the parts that come in
/// left-and-right pairs, whose `ai[1]` says which side this one is.
#[derive(Debug, Clone, Copy)]
pub struct Seat {
    /// The type it rides on.
    pub mount: u16,
    /// Offset from the mount's centre.
    pub offset: (f32, f32),
    /// Extra sideways offset applied `+` on one side and `-` on the other.
    pub side_offset: f32,
    /// Whether the part faces outward rather than the way the mount faces.
    pub faces_outward: bool,
}

/// A scutlix rider sits above its mount, which it also summons.
pub const SCUTLIX_RIDER: u16 = 390;
pub const SCUTLIX: u16 = 391;
/// A drakomire rider likewise.
pub const DRAKOMIRE_RIDER: u16 = 416;
pub const DRAKOMIRE: u16 = 415;
/// The Martian saucer's hull, its top, its two turrets and its two cannon.
pub const SAUCER_CORE: u16 = 395;
pub const SAUCER_TOP: u16 = 392;
pub const SAUCER_TURRET: u16 = 393;
pub const SAUCER_CANNON: u16 = 394;

/// Where each of them sits.
pub fn seat(npc_type: u16) -> Option<Seat> {
    Some(match npc_type {
        SCUTLIX_RIDER => Seat {
            mount: SCUTLIX,
            offset: (0.0, -14.0),
            side_offset: 0.0,
            faces_outward: false,
        },
        DRAKOMIRE_RIDER => Seat {
            mount: DRAKOMIRE,
            offset: (-10.0, -30.0),
            side_offset: 0.0,
            faces_outward: false,
        },
        SAUCER_TOP => Seat {
            mount: SAUCER_CORE,
            offset: (0.0, 2.0),
            side_offset: 0.0,
            faces_outward: false,
        },
        SAUCER_TURRET => Seat {
            mount: SAUCER_CORE,
            offset: (0.0, 29.0),
            side_offset: 60.0,
            faces_outward: false,
        },
        SAUCER_CANNON => Seat {
            mount: SAUCER_CORE,
            offset: (0.0, -13.0),
            side_offset: 49.0,
            faces_outward: true,
        },
        DUTCHMAN_GUN => Seat {
            mount: 491,
            // The Dutchman's four guns are spaced along the hull rather than mirrored.
            offset: (-122.0, -6.0),
            side_offset: 0.0,
            faces_outward: false,
        },
        _ => return None,
    })
}

/// The Dutchman's guns are spaced this far apart along the hull.
pub const DUTCHMAN_GUN_SPACING: f32 = 68.0;

/// What a scutlix rider's shot (`projectile::ids::RIDER_SHOT`) does.
pub const RIDER_SHOT_DAMAGE: i32 = 30;
pub const RIDER_SHOT_SPEED: f32 = 7.0;
pub const RIDER_RELOAD: f32 = 60.0;
pub const RIDER_FLINCH: f32 = -30.0;
pub const RIDER_RANGE: f32 = 700.0;
pub const RIDER_SPREAD: i32 = 50;

/// A Dutchman cannon's shot (`projectile::ids::CANNON_SHOT`): a slow lobbed ball, fired every four
/// seconds.
pub const CANNON_SHOT_DAMAGE: i32 = 30;
pub const CANNON_SHOT_SPEED: f32 = 14.0;
pub const CANNON_SHOT_RISE: f32 = -5.0;
pub const CANNON_RELOAD: f32 = 240.0;

// --- Pathfinders and pouncers -------------------------------------------------------------------

/// Style 85: how fast each of the three closes when it has a clear line.
pub const SPHERE_CHASE: f32 = 5.5;
pub const CELL_CHASE: f32 = 8.0;
/// The chase speeds up with distance, a pixel per tick for every hundred pixels away.
pub const CHASE_DISTANCE_GAIN: f32 = 100.0;
pub const CHASE_SMOOTH: f32 = 50.0;
/// Beyond this it gives up on terrain entirely and flies through it.
pub const PATH_GIVE_UP: f32 = 800.0;
pub const PATH_PHASE_SPEED: f32 = 3.0;
pub const CELL_PHASE_SPEED: f32 = 6.0;
pub const PATH_PHASE_SMOOTH: f32 = 3.0;
/// ...and comes back through once it is this close again.
pub const PATH_RESURFACE: f32 = 600.0;
/// Rounding a corner: it aims at a waypoint level with or above the player.
pub const PATH_CORNER_SPEED: f32 = 2.0;
pub const CELL_CORNER_SPEED: f32 = 3.0;
pub const PATH_CORNER_SMOOTH: f32 = 3.0;
/// The waypoint has to be at least this far off to be worth going to.
pub const PATH_WAYPOINT_MIN: f32 = 8.0;
/// Lost: it drifts, bouncing, and looks for a corner every five ticks.
pub const PATH_DRIFT_SPEED: f32 = 2.0;
pub const CELL_DRIFT_SPEED: f32 = 3.0;
pub const PATH_DRIFT_SMOOTH: f32 = 20.0;
pub const PATH_DRIFT_BOUNCE: f32 = -0.8;
pub const PATH_LOOK_EVERY: f32 = 5.0;
pub const PATH_LOST_TICKS: f32 = 180.0;
/// The nebula headcrab, which latches onto your head instead of hitting you.
pub const HEADCRAB: u16 = 421;
pub const HEADCRAB_LATCH: f32 = 40.0;
/// The stardust cell, which pushes its own kind apart rather than stacking.
pub const STARDUST_CELL_BIG: u16 = 405;
pub const PATH_SHOVE: f32 = 0.05;

/// Style 90: a Mothron spawn's three speeds, and how long it holds each.
pub const SPAWN_CIRCLE_TICKS: f32 = 90.0;
pub const SPAWN_CHASE: f32 = 5.5;
pub const SPAWN_CHASE_GAIN: f32 = 100.0;
pub const SPAWN_CHASE_SMOOTH: f32 = 40.0;
pub const SPAWN_CIRCLE_RANGE: f32 = 200.0;
pub const SPAWN_FAR: f32 = 800.0;
/// Out of sight it phases through terrain, gaining speed the longer it takes.
pub const SPAWN_PHASE_GAIN: f32 = 150.0;
pub const SPAWN_PHASE_SMOOTH: f32 = 35.0;
pub const SPAWN_PHASE_ACCEL: f32 = 1.0 / 60.0;
pub const SPAWN_REACQUIRE: f32 = 300.0;
pub const SPAWN_LOSE: f32 = 1000.0;
/// The pounce: it lines up for ten ticks and then commits.
pub const SPAWN_AIM_TICKS: f32 = 10.0;
pub const SPAWN_POUNCE: f32 = 9.0;
pub const SPAWN_POUNCE_SMOOTH: f32 = 8.0;
pub const SPAWN_POUNCE_TICKS: f32 = 45.0;
pub const SPAWN_POUNCE_GAIN: f32 = 1.01;
/// Without an eclipse to be out in, it climbs away and leaves.
pub const SPAWN_LEAVE_CLIMB: f32 = -0.2;
pub const SPAWN_LEAVE_CAP: f32 = -8.0;
pub const SPAWN_DESPAWN_RANGE: f32 = 3000.0;

// --- Swoopers: the things that run past you and come back ---------------------------------------

/// Style 86's numbers. The apparition is slower and ranges wider; the squidhead is faster and
/// turns tighter.
#[derive(Debug, Clone, Copy)]
pub struct Swoop {
    /// Horizontal acceleration and cap during the run.
    pub run_accel: f32,
    pub run_cap: f32,
    /// How closely it tracks your height while running, and the band inside which it tracks
    /// gently rather than hard.
    pub track_band: f32,
    pub track_smooth: f32,
    /// How far past you it goes before turning.
    pub overshoot: f32,
    /// The vertical leg: acceleration, speed cap and the drag past it.
    pub climb_accel: f32,
    pub climb_cap: f32,
    pub climb_drag: f32,
    /// The return leg.
    pub return_accel: f32,
    pub return_pull: f32,
    pub return_cap: f32,
    pub return_drag: f32,
}

/// The shadowflame apparition.
pub const APPARITION: u16 = 472;
/// The ancient cultist's squidhead.
pub const SQUIDHEAD: u16 = 521;

pub const APPARITION_SWOOP: Swoop = Swoop {
    run_accel: 0.3,
    run_cap: 7.0,
    track_band: 4.0,
    track_smooth: 4.0,
    overshoot: 660.0,
    climb_accel: 0.4,
    climb_cap: 5.0,
    climb_drag: 0.95,
    return_accel: 0.4,
    return_pull: 0.2,
    return_cap: 5.0,
    return_drag: 0.95,
};

pub const SQUIDHEAD_SWOOP: Swoop = Swoop {
    run_accel: 0.7,
    run_cap: 14.0,
    track_band: 6.0,
    track_smooth: 3.0,
    overshoot: 500.0,
    climb_accel: 0.3,
    climb_cap: 7.0,
    climb_drag: 0.9,
    return_accel: 0.6,
    return_pull: 0.3,
    return_cap: 7.0,
    return_drag: 0.9,
};

/// Outside the tracking band it corrects much harder, which is what keeps a run level with you.
pub const SWOOP_HARD_TRACK: f32 = 15.0;
/// They fade in over this many ticks, with a shove to get them moving.
pub const SWOOP_ENTRANCE: f32 = 120.0;
pub const SWOOP_ENTRANCE_SHOVE: f32 = 2.0;
pub const SWOOP_FADE: i32 = 30;
/// Two closer than this shoulder each other apart.
pub const SWOOP_PERSONAL_SPACE: f32 = 50.0;
pub const SWOOP_SHOVE: f32 = 0.4;

// --- The nebula brain, which teleports rather than travels --------------------------------------

/// How fast it closes, and how heavily that is smoothed.
pub const BRAIN_APPROACH: f32 = 7.0;
pub const BRAIN_APPROACH_SMOOTH: f32 = 30.0;
/// It stops closing inside this, and drifts instead.
pub const BRAIN_STANDOFF: f32 = 400.0;
pub const BRAIN_DRIFT_DRAG: f32 = 0.98;
/// It relocates every eight seconds, to somewhere within twenty tiles of you.
pub const BRAIN_TELEPORT_EVERY: f32 = 480.0;
pub const BRAIN_TELEPORT_RANGE: i32 = 20;
/// ...but never within twelve tiles of a player, so it cannot land on top of you.
pub const BRAIN_TELEPORT_CLEARANCE: i32 = 12;
/// The floaters it puts out (`projectile::ids::NEBULA_FLOATER`): three of them, one a second, at
/// the start of its life.
pub const BRAIN_FLOATER_WINDOW: f32 = 180.0;
pub const BRAIN_FLOATER_EVERY: f32 = 60.0;
pub const BRAIN_FLOATER_SPEED: (f32, f32) = (4.0, 2.5);
/// A teleport hurries its floaters along by half a second each.
pub const BRAIN_FLOATER_HURRY: f32 = 30.0;

// --- The Dreadnautilus, which only exists during a blood moon -----------------------------------

/// How it drifts to its station, and where that station is relative to you.
pub const NAUTILUS_SPEED: f32 = 7.5;
pub const NAUTILUS_ACCEL: f32 = 0.15;
pub const NAUTILUS_STANDOFF: f32 = 300.0;
pub const NAUTILUS_ABOVE: f32 = 200.0;
pub const NAUTILUS_ARRIVED: f32 = 50.0;
/// It holds station this long before choosing an attack.
pub const NAUTILUS_HOLD: f32 = 60.0;
/// Rising out of the ground when it arrives.
pub const NAUTILUS_EMERGE_TICKS: f32 = 50.0;
pub const NAUTILUS_EMERGE_AT: f32 = 5.0;
pub const NAUTILUS_EMERGE_RISE: f32 = -2.5;
pub const NAUTILUS_FADE_IN: i32 = 10;
/// The charge: it winds up, reflecting shots, then rams backwards along its own mouth line.
pub const NAUTILUS_CHARGE_WINDUP: f32 = 90.0;
pub const NAUTILUS_CHARGE_TICKS: f32 = 180.0;
pub const NAUTILUS_CHARGE_SPEED: f32 = -16.0;
pub const NAUTILUS_CHARGE_HOMING: f32 = 1.5;
pub const NAUTILUS_CHARGE_MIN: f32 = 150.0;
/// The spray: three bursts of five to ten bolts, each shoving it backwards.
pub const NAUTILUS_SPRAY_WINDUP: f32 = 90.0;
pub const NAUTILUS_SPRAY_TICKS: f32 = 90.0;
pub const NAUTILUS_SPRAY_BURSTS: i32 = 3;
pub const NAUTILUS_SPRAY_RECOIL: f32 = -8.0;
pub const NAUTILUS_SPRAY_DAMAGE: i32 = 30;
pub const NAUTILUS_SPRAY_SPEED: f32 = 10.0;
pub const NAUTILUS_SPRAY_SPREAD: f32 = 6.0;
pub const NAUTILUS_SPRAY_COUNT: (u32, u32) = (5, 11);
/// The summon: it holds still and calls three helpers out of the blood moon, through
/// `projectile::ids::NAUTILUS_HELPER_PORTAL`.
pub const NAUTILUS_SUMMON_TICKS: f32 = 180.0;
pub const NAUTILUS_SUMMON_AT: [f32; 3] = [10.0, 20.0, 30.0];
pub const NAUTILUS_HELPER: u16 = 619;
pub const NAUTILUS_HELPERS_MAX: usize = 3;
/// Where its mouth is, and how far off the body's own rotation it points.
pub const NAUTILUS_MOUTH_ANGLE: f32 = 0.471_238_94;
pub const NAUTILUS_MOUTH_REACH: f32 = 50.0;

// --- Big mimics: the chest that fights back ------------------------------------------------------

/// How long it sits there rattling before it stands up.
pub const MIMIC_WAKE_TICKS: f32 = 36.0;
/// A player this close wakes one even if nothing has touched it.
pub const MIMIC_WAKE_RANGE: f32 = 80.0;
/// The hop: it waits between fifteen and forty-five ticks depending on how hurt it is, and jumps
/// harder and further the more it has taken.
pub const MIMIC_HOP_REST_MIN: f32 = 15.0;
pub const MIMIC_HOP_REST_HEALTHY: f32 = 30.0;
pub const MIMIC_HOP_ACROSS_MIN: f32 = 3.0;
pub const MIMIC_HOP_ACROSS_HURT: f32 = 4.0;
pub const MIMIC_HOP_UP: f32 = 4.0;
/// It jumps higher when it cannot see you, to get over whatever is in the way.
pub const MIMIC_HOP_BLIND_BONUS: f32 = 2.0;
/// Every third hop is a big one: twice the height and half the distance.
pub const MIMIC_BIG_HOP_EVERY: f32 = 3.0;
/// After this long hopping it picks one of its three specials.
pub const MIMIC_HOP_PATIENCE: f32 = 210.0;
/// Curling up: it takes nothing at all for three seconds, and in expert it bats shots back.
pub const MIMIC_CURL_TICKS: f32 = 180.0;
/// The dive: it climbs to this far above you before dropping.
pub const MIMIC_DIVE_HEIGHT: f32 = 350.0;
pub const MIMIC_DIVE_CLIMB: f32 = 12.0;
pub const MIMIC_DIVE_LINEUP: f32 = 40.0;
pub const MIMIC_DIVE_AIM_TICKS: f32 = 6.0;
pub const MIMIC_DIVE_AIM_SPEED: f32 = 8.0;
pub const MIMIC_DIVE_GRAVITY: f32 = 0.2;
pub const MIMIC_DIVE_CAP: f32 = 16.0;
pub const MIMIC_DIVE_LAND_TICKS: f32 = 10.0;
/// The charge: three long low bounds at twelve pixels a tick.
pub const MIMIC_CHARGE_BOUNDS: f32 = 3.0;
pub const MIMIC_CHARGE_REST: f32 = 5.0;
pub const MIMIC_CHARGE_ACROSS: f32 = 12.0;
pub const MIMIC_CHARGE_UP: f32 = 4.0;
pub const MIMIC_CHARGE_AIR_SPEED: f32 = 8.0;
/// Coming back through terrain to reach you.
pub const MIMIC_RETURN_SPEED: f32 = 10.0;
pub const MIMIC_RETURN_RANGE: f32 = 200.0;
/// Beyond this it stops fighting and comes back to you through the walls.
pub const MIMIC_LOSE_RANGE: f32 = 600.0;

// --- Lunar pillars --------------------------------------------------------------------------------

/// The four towers, in the order the game numbers their fragments.
pub const TOWER_SOLAR: u16 = 517;
pub const TOWER_VORTEX: u16 = 422;
pub const TOWER_NEBULA: u16 = 507;
pub const TOWER_STARDUST: u16 = 493;

/// How many of a tower's minions must die before its shield drops.
///
/// Halved once the Moon Lord has been beaten, which is the game's way of making a second lunar
/// event shorter than the first.
pub const TOWER_SHIELD: i32 = 100;

/// A tower bobs on this cycle, half a pixel either way.
pub const TOWER_BOB_TICKS: f32 = 300.0;
pub const TOWER_BOB: f32 = 0.5;
/// It holds itself between ten and thirty tiles above whatever is beneath it.
pub const TOWER_TOO_LOW: i32 = 10;
pub const TOWER_COMFORTABLE: i32 = 20;
pub const TOWER_TOO_HIGH: i32 = 30;
pub const TOWER_LIFT: f32 = 1.5;
/// It stays this many tiles clear of the world's edges.
pub const TOWER_MARGIN: i32 = 60;
pub const TOWER_MARGIN_NUDGE: f32 = 80.0;
/// Left alone for a second it starts healing, two hundred a time.
pub const TOWER_ABANDONED_RANGE: f32 = 2000.0;
pub const TOWER_ABANDONED_TICKS: f32 = 60.0;
pub const TOWER_REGEN: i32 = 200;
/// Its collapse: three seconds, fading out over the last one.
pub const TOWER_COLLAPSE_TICKS: f32 = 180.0;
pub const TOWER_COLLAPSE_FADE_AT: f32 = 120.0;
pub const TOWER_COLLAPSE_DRIFT: f32 = 0.25;
pub const TOWER_COLLAPSE_EASE: f32 = 0.02;

// --- Mothron ------------------------------------------------------------------------------------

/// It will not lay more than this many eggs and spawn at once.
pub const MOTHRON_BROOD: usize = 7;
/// The egg it lays, and the spawn that hatches from it. NPCID: MothronEgg=478, MothronSpawn=479.
/// These were previously 470/471 — CrimsonPenguin and GoblinSummoner — so during an eclipse Mothron
/// laid Crimson Penguins and the brood census counted the wrong types.
pub const MOTHRON_EGG: u16 = 478;
pub const MOTHRON_SPAWN_TYPE: u16 = 479;
/// Hovering: it holds two hundred pixels above you and picks an attack every three seconds.
pub const MOTHRON_ABOVE: f32 = 200.0;
pub const MOTHRON_HOVER_SPEED: f32 = 6.0;
pub const MOTHRON_HOVER_SMOOTH: f32 = 30.0;
pub const MOTHRON_HOVER_HOLD: f32 = 80.0;
pub const MOTHRON_DECIDE_TICKS: f32 = 180.0;
/// Being hit hurries the decision along, by ten to thirty ticks.
pub const MOTHRON_HIT_HURRY: (u32, u32) = (10, 30);
/// It gives up on terrain past this and comes straight through it.
pub const MOTHRON_FAR: f32 = 800.0;
pub const MOTHRON_LOSE: f32 = 1000.0;
pub const MOTHRON_REACQUIRE: f32 = 300.0;
pub const MOTHRON_CROSS_SPEED: f32 = 7.0;
pub const MOTHRON_CROSS_GAIN: f32 = 100.0;
pub const MOTHRON_CROSS_SMOOTH: f32 = 25.0;
/// The chase: it accelerates for as long as it holds you in sight, and hits at half strength.
pub const MOTHRON_CHASE_DAMAGE: f32 = 0.5;
pub const MOTHRON_CHASE_BASE: f32 = 4.0;
pub const MOTHRON_CHASE_ACCEL: f32 = 1.0 / 45.0;
pub const MOTHRON_CHASE_ACCEL_EXPERT: f32 = 1.0 / 60.0;
pub const MOTHRON_CHASE_GAIN: f32 = 120.0;
pub const MOTHRON_CHASE_SMOOTH: f32 = 20.0;
pub const MOTHRON_CHASE_TICKS: f32 = 240.0;
/// The sweep: it draws off to one side, lines up, and comes across at speed, hitting harder.
pub const MOTHRON_SWEEP_OFFSET: f32 = 400.0;
pub const MOTHRON_SWEEP_DRAW_SPEED: f32 = 8.0;
pub const MOTHRON_SWEEP_DRAW_ACCEL: f32 = 1.0 / 30.0;
pub const MOTHRON_SWEEP_DRAW_SMOOTH: f32 = 4.0;
pub const MOTHRON_SWEEP_READY_X: f32 = 350.0;
pub const MOTHRON_SWEEP_READY_Y: f32 = 20.0;
pub const MOTHRON_SWEEP_AIM_SPEED: f32 = 16.0;
pub const MOTHRON_SWEEP_AIM_SMOOTH: f32 = 8.0;
pub const MOTHRON_SWEEP_AIM_TICKS: f32 = 10.0;
pub const MOTHRON_SWEEP_DAMAGE: f32 = 1.3;
pub const MOTHRON_SWEEP_ACCEL: f32 = 1.0 / 30.0;
pub const MOTHRON_SWEEP_PAST: f32 = 260.0;
/// Where it will lay: within this many tiles of you, on a floor, and not in lava.
pub const MOTHRON_LAY_RANGE_X: i32 = 30;
pub const MOTHRON_LAY_RANGE_Y: i32 = 20;
/// Laying is not instant: it has to fly down to the spot it picked first, easing toward it a
/// tenth of the way each tick and never faster than the cap.
pub const MOTHRON_LAY_SPEED_BASE: f32 = 6.0;
pub const MOTHRON_LAY_SPEED_GAIN: f32 = 150.0;
pub const MOTHRON_LAY_SPEED_CAP: f32 = 10.0;
/// How close counts as arrived, for the flight down and then the hover once there.
pub const MOTHRON_LAY_ARRIVE: f32 = 10.0;
pub const MOTHRON_SETTLE_ARRIVE: f32 = 4.0;
pub const MOTHRON_SETTLE_SPEED_CAP: f32 = 4.0;
/// Once settled it hovers over the spot for this long before the egg actually appears, and the
/// same again after that before it goes back to hovering — halved in Expert Mode.
pub const MOTHRON_SETTLE_WAIT: f32 = 70.0;
pub const MOTHRON_SETTLE_WAIT_EXPERT: f32 = 52.0;
/// The odds, out of three, that a Mothron with room left in its brood goes straight back down to
/// lay another egg rather than returning to its hover.
pub const MOTHRON_RELAY_ODDS: u32 = 3;

// --- The Twins ------------------------------------------------------------------------------------

/// One eye's numbers. The two share a skeleton and differ in every constant in it.
#[derive(Debug, Clone, Copy)]
pub struct Twin {
    /// Where it holds station in its first form, relative to the player.
    pub station: (f32, f32),
    pub speed: f32,
    pub accel: f32,
    pub speed_expert: f32,
    pub accel_expert: f32,
    /// How long it holds station before running its dashes.
    pub hover_ticks: f32,
    /// The dash: how fast, how long each one lasts, and how many before it settles again.
    pub dash_speed: f32,
    pub dash_speed_expert: f32,
    pub dash_ticks: f32,
    pub dash_brake_at: f32,
    pub dashes: f32,
    /// The multiplier applied to velocity once braking starts. Retinazer's dash barely bleeds
    /// speed (0.96); Spazmatism's sheds it much faster (0.9).
    pub dash_decay: f32,
    /// Cumulative expert-only speed bumps added to the dash once life drops under each listed
    /// fraction. Empty for Retinazer, whose dash speed is a flat expert value; Spazmatism's
    /// climbs from 13 toward ~15.8 as it is worn down (`NPC.cs:27410-27433`).
    pub dash_speed_ramp: &'static [(f32, f32)],
    /// Its ranged attack in the first form.
    pub shot: u16,
    pub shot_damage: i32,
    pub shot_speed: f32,
    pub shot_speed_expert: f32,
    pub shot_charge: f32,
    /// How far ahead of the eye the shot is spawned, and how much its aim is scattered.
    pub shot_lead: f32,
    pub shot_spread_scale: f32,
    /// Whether it only shoots from above and close, as Retinazer does.
    pub shoots_only_from_above: bool,
    /// The second form: where it holds, how fast, and what it throws (Retinazer) or breathes
    /// (Spazmatism's cursed-inferno flame, gated the same way through `second_shot_charge` but
    /// with a much lower threshold).
    pub second_station: (f32, f32),
    pub second_speed: f32,
    pub second_accel: f32,
    pub second_speed_expert: f32,
    pub second_accel_expert: f32,
    pub second_hover_ticks: f32,
    pub second_shot: u16,
    pub second_shot_damage: i32,
    pub second_shot_speed: f32,
    pub second_shot_speed_expert: f32,
    pub second_shot_charge: f32,
    /// The strafing sub-state of Retinazer's second form. Unused by Spazmatism, whose second
    /// form dashes instead (`second_dash_*` below).
    pub strafe_offset: f32,
    pub strafe_speed: f32,
    pub strafe_accel: f32,
    pub strafe_speed_expert: f32,
    pub strafe_accel_expert: f32,
    /// Spazmatism's second-form dash: structurally the same loop as the first form's, but with
    /// its own speed, timing and decay (`NPC.cs:27733-27795`). Zero and unused for Retinazer.
    pub second_dash_speed: f32,
    pub second_dash_speed_expert: f32,
    pub second_dash_ticks: f32,
    pub second_dash_brake_at: f32,
    pub second_dash_decay: f32,
    pub second_dashes: f32,
}

pub const RETINAZER: u16 = 125;
pub const SPAZMATISM: u16 = 126;

pub const RETINAZER_TWIN: Twin = Twin {
    station: (300.0, -300.0),
    speed: 7.0,
    accel: 0.1,
    speed_expert: 8.25,
    accel_expert: 0.115,
    hover_ticks: 600.0,
    // 12/15, not 14/17: the +2 only applies under getGoodWorld (`NPC.cs:26806-26814`), which is
    // out of scope here.
    dash_speed: 12.0,
    dash_speed_expert: 15.0,
    dash_ticks: 70.0,
    dash_brake_at: 25.0,
    dashes: 4.0,
    dash_decay: 0.96,
    dash_speed_ramp: &[],
    shot: 83,
    shot_damage: 20,
    shot_speed: 9.0,
    shot_speed_expert: 10.5,
    shot_charge: 60.0,
    shot_lead: 15.0,
    shot_spread_scale: 0.08,
    shoots_only_from_above: true,
    second_station: (0.0, -300.0),
    second_speed: 8.0,
    second_accel: 0.15,
    second_speed_expert: 9.5,
    second_accel_expert: 0.175,
    second_hover_ticks: 300.0,
    second_shot: 100,
    second_shot_damage: 25,
    second_shot_speed: 8.5,
    second_shot_speed_expert: 10.0,
    second_shot_charge: 180.0,
    strafe_offset: 340.0,
    strafe_speed: 8.0,
    strafe_accel: 0.2,
    strafe_speed_expert: 9.5,
    strafe_accel_expert: 0.25,
    second_dash_speed: 0.0,
    second_dash_speed_expert: 0.0,
    second_dash_ticks: 0.0,
    second_dash_brake_at: 0.0,
    second_dash_decay: 0.0,
    second_dashes: 0.0,
};

pub const SPAZMATISM_TWIN: Twin = Twin {
    // Holds level with the player at 400px, not above (`NPC.cs:27289-27291`).
    station: (400.0, 0.0),
    speed: 12.0,
    accel: 0.4,
    speed_expert: 12.0,
    accel_expert: 0.4,
    hover_ticks: 600.0,
    // Its own dash, not Retinazer's: 10 short 42-tick dashes braking hard at tick 8, decaying
    // 0.9/tick, base speed 13 climbing toward ~15.8 in expert as it takes damage
    // (`NPC.cs:27407-27483`).
    dash_speed: 13.0,
    dash_speed_expert: 13.0,
    dash_ticks: 42.0,
    dash_brake_at: 8.0,
    dashes: 10.0,
    dash_decay: 0.9,
    dash_speed_ramp: &[(0.9, 0.5), (0.8, 0.5), (0.7, 0.55), (0.6, 0.6), (0.5, 0.65)],
    shot: 96,
    shot_damage: 25,
    shot_speed: 12.0,
    shot_speed_expert: 14.0,
    shot_charge: 60.0,
    shot_lead: 4.0,
    shot_spread_scale: 0.05,
    shoots_only_from_above: false,
    // Its second form is a close-range flamethrower, not Retinazer's hover-and-throw: station at
    // player.X +-180 / player.Y, speed 4, cursed-inferno flame (proj 101) gated by
    // `second_shot_charge` at localAI[1] > 8 instead of 180 — roughly twenty times faster
    // (`NPC.cs:27558-27732`).
    second_station: (180.0, 0.0),
    second_speed: 4.0,
    second_accel: 0.1,
    second_speed_expert: 4.0,
    second_accel_expert: 0.1,
    second_hover_ticks: 400.0,
    second_shot: 101,
    second_shot_damage: 30,
    second_shot_speed: 6.0,
    second_shot_speed_expert: 6.0,
    second_shot_charge: 8.0,
    // Unused: Spazmatism's second form dashes instead of strafing.
    strafe_offset: 0.0,
    strafe_speed: 0.0,
    strafe_accel: 0.0,
    strafe_speed_expert: 0.0,
    strafe_accel_expert: 0.0,
    // Six dashes of 80 ticks, braking at 50, decaying 0.93/tick, at 14 (16.5 in expert)
    // (`NPC.cs:27733-27795`).
    second_dash_speed: 14.0,
    second_dash_speed_expert: 16.5,
    second_dash_ticks: 80.0,
    second_dash_brake_at: 50.0,
    second_dash_decay: 0.93,
    second_dashes: 6.0,
};

/// Below this fraction of its health an eye transforms.
pub const TWIN_TRANSFORM_AT: f32 = 0.4;
/// The transformation: two spins of a hundred ticks, up and then down.
pub const TWIN_SPIN_TICKS: f32 = 100.0;
pub const TWIN_SPIN_RATE: f32 = 0.005;
pub const TWIN_SPIN_CAP: f32 = 0.5;
/// The second form hits half again as hard and soaks ten more.
pub const TWIN_SECOND_DAMAGE: f32 = 1.5;
/// For the worthy takes the second form's hover speed and acceleration up by a seventh
/// (`NPC.cs:26944-26948`).
pub const TWIN_GET_GOOD_GAIN: f32 = 1.15;
pub const TWIN_SECOND_DEFENSE: i32 = 10;
/// Its first-form shot only comes when it is above you and within this far.
pub const TWIN_SHOT_RANGE: f32 = 400.0;
/// The lead on the second form's heavy throw (both eyes use the same fifteen-tick lead there;
/// the first-form shot's lead and scatter are per-eye, in the `Twin` table).
pub const TWIN_SHOT_LEAD: f32 = 15.0;
pub const TWIN_SHOT_SPREAD: i32 = 40;
/// Daylight sends both of them home.
pub const TWIN_FLEE_CLIMB: f32 = -0.04;

// --- The Destroyer ------------------------------------------------------------------------------

/// Its head, body and tail.
pub const DESTROYER_HEAD: u16 = 134;
pub const DESTROYER_BODY: u16 = 135;
pub const DESTROYER_TAIL: u16 = 136;
/// How many trailing segments (body + tail) it is built from — `GetDestroyerSegmentsCount()`
/// returns 80, but the loop that actually spawns them is `for (j = 0; j <= destroyerSegmentsCount;
/// j++)` (`NPC.cs:50358-50365`), inclusive, so it runs 81 times, not 80 — corrected here from an
/// earlier, unused, off-by-one guess this constant held before anything actually consumed it.
pub const DESTROYER_SEGMENTS: usize = 81;
/// WOF-3: a For-the-Worthy world lengthens it. `GetDestroyerSegmentsCount()` returns 100 rather
/// than 80 when `Main.getGoodWorld` (`NPC.cs:51488-51495`), so the same inclusive loop runs 101
/// times: 100 body segments and the tail. The spawn path picks this over [`DESTROYER_SEGMENTS`]
/// when the world carries the secret seed.
pub const DESTROYER_SEGMENTS_GOOD: usize = 101;

/// It burrows faster than anything else in the game, and turns no more sharply for it.
pub const DESTROYER_SPEED: f32 = 16.0;
pub const DESTROYER_TURN: f32 = 0.1;
/// The second, sharper trim it alone gets, on top of [`DESTROYER_TURN`] rather than instead of it
/// (`NPC.cs:50509-50510`, `num19`/`num20`). See [`worm_hard_turn`].
pub const DESTROYER_TURN_HARD: f32 = 0.15;
/// A get-fixed-boi world sharpens both of its turn rates by a fifth (`NPC.cs:50511-50515`).
pub const DESTROYER_TURN_GET_GOOD: f32 = 1.2;
/// Fleeing at daybreak it dives at twice that.
pub const DESTROYER_FLEE_SPEED: f32 = 32.0;

/// The extra turn rate a worm applies *before* [`WormMotion::turn`], and only while it is already
/// heading the right way on **both** axes.
///
/// Style 6's shared burrow (`NPC.cs:52660-52747`) has a single rate, `num47`. The Destroyer's own
/// style-37 copy (`NPC.cs:50633-50651`) opens with a second pass at `num20 = 0.15` and then runs
/// the ordinary `num19 = 0.1` one over the result, so a Destroyer already on course closes on its
/// wanted velocity at 0.25 a tick rather than 0.1. Zero for every other worm, which is what makes
/// the extra pass a no-op for them.
pub fn worm_hard_turn(npc_type: u16) -> f32 {
    match npc_type {
        DESTROYER_HEAD | DESTROYER_BODY | DESTROYER_TAIL => DESTROYER_TURN_HARD,
        _ => 0.0,
    }
}

/// Every body segment carries a probe that fires on a long random fuse.
///
/// The fuse advances by nought to three a tick and fires somewhere between fourteen hundred and
/// twenty-six thousand, so across eighty segments the swarm of lasers is steady while any one
/// segment fires rarely.
pub const DESTROYER_FUSE_STEP: u32 = 4;
pub const DESTROYER_FUSE: (u32, u32) = (1400, 26000);
/// `GetAttackDamage_ForProjectiles(22f, 18f)` (`NPC.cs:50399`): a launch-time lerp between a
/// classic figure and a separate, lower expert one, which the impact-time
/// `hostileDamageProjectileMultiplier` then doubles on top. `Remap` clamps outside classic..expert,
/// so master reads the same 18 as expert. Using the classic 22 in every mode made an expert
/// Destroyer's lasers 22% heavier than the game's.
pub const DESTROYER_LASER_DAMAGE: i32 = 22;
pub const DESTROYER_LASER_DAMAGE_EXPERT: i32 = 18;
pub const DESTROYER_LASER_SPEED: f32 = 8.0;
/// The aim is scattered twice: once in pixels before it is normalised, once in speed after.
pub const DESTROYER_AIM_SPREAD: i32 = 20;
pub const DESTROYER_SPEED_SPREAD: f32 = 0.05;
pub const DESTROYER_LASER_LEAD: f32 = 5.0;
pub const DESTROYER_LASER_LIFE: u16 = 300;

// --- Skeletron Prime ------------------------------------------------------------------------------

pub const PRIME_HEAD: u16 = 127;
// NPCID order (`NPCID.cs`): 128 PrimeCannon, 129 PrimeSaw, 130 PrimeVice, 131 PrimeLaser. These
// were previously shifted (SAW=128/VICE=129/CANNON=130), which put the bomb-lobbing behavior on
// the Vice arm and left the real Cannon as a plain melee arm. The head-spawn side list below is
// written explicitly by arm so it still reproduces vanilla's per-type `ai[0]`.
pub const PRIME_CANNON: u16 = 128;
pub const PRIME_SAW: u16 = 129;
pub const PRIME_VICE: u16 = 130;
pub const PRIME_LASER: u16 = 131;

/// The head hovers for ten seconds, then spins for six and two thirds, and repeats.
pub const PRIME_HOVER_TICKS: f32 = 600.0;
pub const PRIME_SPIN_TICKS: f32 = 400.0;
/// While spinning it hits twice as hard and takes half as much.
pub const PRIME_SPIN_DAMAGE: i32 = 2;
pub const PRIME_SPIN_DEFENSE: i32 = 2;
/// It holds between two and five hundred pixels above you, gently.
pub const PRIME_ABOVE_MIN: f32 = 200.0;
pub const PRIME_ABOVE_MAX: f32 = 500.0;
pub const PRIME_LIFT: f32 = 0.1;
pub const PRIME_LIFT_CAP: f32 = 2.0;
pub const PRIME_DRIFT: f32 = 0.1;
pub const PRIME_DRIFT_CAP: f32 = 8.0;
pub const PRIME_LIFT_EXPERT: f32 = 0.03;
pub const PRIME_LIFT_CAP_EXPERT: f32 = 4.0;
pub const PRIME_DRIFT_EXPERT: f32 = 0.07;
pub const PRIME_DRIFT_CAP_EXPERT: f32 = 9.5;
/// It keeps a hundred pixels of sideways slack, so it does not jitter directly overhead.
pub const PRIME_SLACK: f32 = 100.0;
/// The spin: it comes at you at a fixed speed, faster in expert and faster still at range.
pub const PRIME_SPIN_SPEED: f32 = 2.0;
pub const PRIME_SPIN_SPEED_EXPERT: f32 = 6.0;
pub const PRIME_SPIN_RANGE_STEP: f32 = 50.0;
pub const PRIME_SPIN_RANGE_FROM: f32 = 150.0;
/// The first step is gentler than the rest: 1.05 past 150 pixels, then 1.1 at each of 200 through
/// 600 (`NPC.cs:27968-28008`). Reading the first as 1.1 too made every expert spin past 150 pixels
/// 4.76% fast, and the error compounded through every step above it.
pub const PRIME_SPIN_RANGE_GAIN_FIRST: f32 = 1.05;
pub const PRIME_SPIN_RANGE_GAIN: f32 = 1.1;
/// Daylight enrages it: `damage = 9999; defense = 9999;` (`NPC.cs:28034-28035`), and it runs you
/// down. Both are live numbers, and neither is `dontTakeDamage`: the armour is what makes daylight
/// a fail-state rather than a nuisance, and the damage is what makes touching it fatal.
pub const PRIME_ENRAGED_STAT: i32 = 9999;
pub const PRIME_ENRAGED_SPEED: f32 = 10.0;
pub const PRIME_ENRAGED_GAIN: f32 = 100.0;
pub const PRIME_ENRAGED_MIN: f32 = 8.0;
pub const PRIME_ENRAGED_MAX: f32 = 32.0;
/// Losing you entirely sends it down and away, with no terminal speed: vanilla's own 13-pixel clamp
/// (`NPC.cs:28100-28103`) is inside the `IsMechQueenUp` half of that branch, and the ordinary fight
/// simply accelerates (`NPC.cs:28105-28113`).
pub const PRIME_LEAVE_SINK: f32 = 0.1;
/// How far into their attack timer the Vice and the Laser start, so the four arms do not switch in
/// lockstep (`NPC.cs:27824`, `:27831`, `ai[3] = 150f`).
pub const PRIME_ARM_HEAD_START: f32 = 150.0;
pub const PRIME_LOSE_RANGE: f32 = 6000.0;

/// One limb's numbers.
#[derive(Debug, Clone, Copy)]
pub struct PrimeLimb {
    /// Which side of the head it hangs on: -1 left, 1 right. Read from `ai[0]`.
    pub station: (f32, f32),
    /// How fast it chases while the head is spinning.
    pub chase_speed: f32,
    pub chase_accel: f32,
    /// Its cycle: how long it holds station, and how long it attacks for.
    pub hold_ticks: f32,
    pub attack_ticks: f32,
    /// What it throws, if anything.
    pub shot: Option<u16>,
    pub shot_damage: i32,
    pub shot_speed: f32,
    pub shot_charge: f32,
    pub shot_spread: f32,
    pub shot_lead: f32,
    /// Whether the shot is fired *away* from the aim, as the cannon's arc is.
    pub shot_reversed: bool,
}

/// Where every limb hangs, before its side is applied.
pub const PRIME_LIMB_STATION: (f32, f32) = (200.0, 230.0);
/// A limb further than this from its station gives up and flies back; it resumes inside this.
pub const PRIME_LIMB_LOST: f32 = 800.0;
pub const PRIME_LIMB_FOUND: f32 = 400.0;
/// How fast it flies back.
pub const PRIME_LIMB_RETURN_X: f32 = 0.5;
pub const PRIME_LIMB_RETURN_X_CAP: f32 = 12.0;
pub const PRIME_LIMB_RETURN_Y: f32 = 0.1;
pub const PRIME_LIMB_RETURN_Y_CAP: f32 = 8.0;

pub fn prime_limb(npc_type: u16) -> PrimeLimb {
    let base = PrimeLimb {
        station: PRIME_LIMB_STATION,
        chase_speed: 7.0,
        chase_accel: 0.05,
        hold_ticks: 300.0,
        attack_ticks: 600.0,
        shot: None,
        shot_damage: 0,
        shot_speed: 0.0,
        shot_charge: 0.0,
        shot_spread: 0.0,
        shot_lead: 0.0,
        shot_reversed: false,
    };
    match npc_type {
        // The cannon lobs its bomb *backwards* along the aim, which is what gives it its arc.
        PRIME_CANNON => PrimeLimb {
            attack_ticks: 1100.0,
            shot: Some(102),
            shot_damage: 0,
            shot_speed: 12.0,
            shot_charge: 140.0,
            shot_spread: 0.01,
            shot_lead: 4.0,
            shot_reversed: true,
            ..base
        },
        PRIME_LASER => PrimeLimb {
            attack_ticks: 800.0,
            shot: Some(100),
            shot_damage: 25,
            shot_speed: 8.0,
            shot_charge: 200.0,
            shot_spread: 0.05,
            shot_lead: 8.0,
            ..base
        },
        // The saw and the vice have no shot at all: they are the melee arms.
        _ => base,
    }
}

/// Shots are scattered by up to forty steps of the limb's spread.
pub const PRIME_SHOT_SPREAD_STEPS: i32 = 40;

// --- The Golem ------------------------------------------------------------------------------------

pub const GOLEM_BODY: u16 = 245;
pub const GOLEM_HEAD: u16 = 246;
pub const GOLEM_FIST_LEFT: u16 = 247;
pub const GOLEM_FIST_RIGHT: u16 = 248;
pub const GOLEM_HEAD_FREE: u16 = 249;

/// Where each part hangs off the body, before scale.
pub const GOLEM_HEAD_OFFSET: (f32, f32) = (-3.0, -57.0);
pub const GOLEM_FIST_OFFSET: (f32, f32) = (84.0, -9.0);

/// Fighting the Golem outside its temple doubles everything it does. It is meant to be fought
/// where it lives.
pub const GOLEM_OUTSIDE_PENALTY: f32 = 2.0;

/// The body's hop: a charge that fills faster for every part already destroyed and every health
/// threshold crossed.
pub const GOLEM_HOP_READY: f32 = 300.0;
pub const GOLEM_HOP_PAUSE: f32 = -20.0;
pub const GOLEM_HOP_BONUS_PART: f32 = 2.0;
pub const GOLEM_HOP_BONUS_HURT: f32 = 1.0;
pub const GOLEM_HOP_BONUS_HALF: f32 = 4.0;
pub const GOLEM_HOP_BONUS_THIRD: f32 = 8.0;
pub const GOLEM_HOP_ACROSS: f32 = 4.0;
pub const GOLEM_HOP_UP: f32 = -12.1;
pub const GOLEM_HOP_UP_CAP: f32 = -19.1;
/// In the air it steers, and slams down when it is directly over you.
pub const GOLEM_AIR_ACCEL: f32 = 0.2;
pub const GOLEM_SLAM: f32 = 0.2;
pub const GOLEM_AIR_SPEED: f32 = 3.0;
/// For the worthy more than doubles it (`NPC.cs:46006-46010`, `num12 = 3f` becoming `7f`).
pub const GOLEM_AIR_SPEED_GET_GOOD: f32 = 7.0;
/// Past this it gives up entirely.
pub const GOLEM_LEASH: f32 = 3000.0;

/// The head, while attached: it hovers on the body and spits fireballs.
pub const GOLEM_HEAD_TETHER_SPEED: f32 = 100.0;
pub const GOLEM_HEAD_CHARGE: f32 = 300.0;
pub const GOLEM_FIREBALL_DAMAGE: i32 = 18;
pub const GOLEM_FIREBALL_SPEED: f32 = 8.0;
/// Past half health the attached head's fireball hits harder (`NPC.cs:31480`).
pub const GOLEM_FIREBALL_DAMAGE_UPGRADED: i32 = 24;
/// The free head's own fireball hits harder still than the attached head's base one
/// (`NPC.cs:31684`).
pub const GOLEM_FREE_FIREBALL_DAMAGE: i32 = 20;
/// Fractions of the **body's** health past which the free head's fireball charge picks up another
/// `(pace + 4) / 5` per tick (`NPC.cs:31645-31661`). Four steps on top of the base take a 300-tick
/// cycle down to sixty.
pub const GOLEM_FREE_FIREBALL_STEPS: [f32; 4] = [0.8, 0.6, 0.2, 0.1];

/// Eye-lasers. The attached head only grows these past half health, alongside its upgraded
/// fireball (`NPC.cs:31504-31564`); the free head always has them (`NPC.cs:31736-31801`).
///
/// Vanilla's interval is `60 + rand(0..600)` (attached) or `100 + rand(0..4800)` (free), rerolled
/// every tick until crossed. This module has no source of randomness available to it without
/// threading one in from outside its lane, so both use the roll's fixed average instead — the
/// same cadence, without the jitter.
pub const GOLEM_LASER_DAMAGE: i32 = 28;
/// Centred on the player it fires two; off to one side of the body, one.
pub const GOLEM_LASER_SPEED: f32 = 11.0;
pub const GOLEM_LASER_SPEED_OFFSIDE: f32 = 12.0;
/// `60 + 600/2`, plus four more per tick spent unable to see you.
pub const GOLEM_LASER_INTERVAL: f32 = 360.0;
pub const GOLEM_LASER_NO_LOS_BONUS: f32 = 4.0;

/// The free head's own laser: a slower cadence that quickens both as it is hurt and while it
/// cannot see you, and hits harder and faster once badly hurt.
pub const GOLEM_FREE_LASER_DAMAGE: i32 = 24;
pub const GOLEM_FREE_LASER_SPEED: f32 = 11.0;
/// Fractions of the **body's** health past which the interval speeds up by one more `pace`
/// (`NPC.cs:31694-31725`; vanilla writes these as `lifeMax / 1.25` through `lifeMax / 6`).
pub const GOLEM_FREE_LASER_INTERVAL_STEPS: [f32; 7] = [
    1.0 / 1.25,
    1.0 / 1.5,
    1.0 / 2.0,
    1.0 / 3.0,
    1.0 / 4.0,
    1.0 / 5.0,
    1.0 / 6.0,
];
/// Multiplied by the pace, not added flat: vanilla's `ai[2] += num770 * 10f` (`NPC.cs:31732`).
pub const GOLEM_FREE_LASER_NO_LOS_BONUS: f32 = 10.0;
/// `100 + 4800/2`.
pub const GOLEM_FREE_LASER_INTERVAL: f32 = 2500.0;
/// Fractions of the **body's** health past which each laser hits one harder and a quarter faster
/// (`NPC.cs:31755-31778`).
pub const GOLEM_FREE_LASER_DAMAGE_STEPS: [f32; 5] = [0.5, 0.4, 0.3, 0.2, 0.1];
/// Without line of sight, the volley is fired blind but hits much harder and faster.
pub const GOLEM_FREE_LASER_NO_LOS_DAMAGE_MULT: f32 = 1.5;
pub const GOLEM_FREE_LASER_NO_LOS_SPEED_MULT: f32 = 2.5;

/// A fist: it holds its station, winds up, and punches.
pub const GOLEM_FIST_RETURN: f32 = 14.0;
pub const GOLEM_FIST_RETURN_HALF: f32 = 3.0;
pub const GOLEM_FIST_RETURN_QUARTER: f32 = 3.0;
pub const GOLEM_FIST_RETURN_BODY_HURT: f32 = 8.0;
pub const GOLEM_FIST_RETURN_CAP: f32 = 32.0;
pub const GOLEM_FIST_READY: f32 = 60.0;
pub const GOLEM_FIST_WINDUP: f32 = 30.0;
pub const GOLEM_FIST_REACH: f32 = 100.0;
pub const GOLEM_PUNCH_SPEED: f32 = 12.0;
pub const GOLEM_PUNCH_HALF: f32 = 4.0;
pub const GOLEM_PUNCH_QUARTER: f32 = 4.0;
pub const GOLEM_PUNCH_BODY_HURT: f32 = 10.0;
pub const GOLEM_PUNCH_CAP: f32 = 48.0;
/// GOL-1: a punch retracts by distance, not a timer. It goes home once the fist is more than this
/// far from its station or has struck terrain (`NPC.cs:19483`, `num2 > 700f || collideX ||
/// collideY`). `num2` is the fist's distance from its home station, so this is its reach.
pub const GOLEM_PUNCH_REACH: f32 = 700.0;

/// The free head, once the body is dead: it hovers three hundred pixels above you.
pub const GOLEM_FREE_ABOVE: f32 = 300.0;
pub const GOLEM_FREE_SPEED: f32 = 7.0;
pub const GOLEM_FREE_ACCEL: f32 = 0.05;

// --- Plantera -------------------------------------------------------------------------------------

pub const PLANTERA: u16 = 262;
pub const PLANTERA_HOOK: u16 = 263;
pub const PLANTERA_TENTACLE: u16 = 264;
/// It starts with three hooks (`NPC.cs:31974-31976`, three literal `NewNPC` calls rather than a
/// loop) and, in its second form, eight tentacles on the body (`NPC.cs:32226-32234`), six more of
/// those in a for-the-worthy world.
pub const PLANTERA_HOOKS: usize = 3;
pub const PLANTERA_TENTACLES: usize = 8;
pub const PLANTERA_TENTACLES_GET_GOOD: usize = 6;
/// In expert each hook grows its own set as well, `body / 2 - 1` apiece, and those orbit the hook
/// rather than the body (`NPC.cs:32235-32248`, `:32505-32508`). Three hooks makes nine more, so
/// an expert Plantera fights with seventeen tentacles rather than eight.
pub const fn plantera_tentacles_per_hook(body: usize) -> usize {
    body / 2 - 1
}
/// A killed body tentacle grows back, in expert only: a one-in-sixty roll each tick, and then a
/// second roll that gets longer the more of the eight are already standing (`NPC.cs:32250-32264`).
/// Only tentacles with no hook of their own are counted or replaced.
pub const PLANTERA_TENTACLE_REGROW_ODDS: u32 = 60;
pub const PLANTERA_TENTACLE_REGROW_PER_ALIVE: u32 = 10;

/// How fast it swings, and how hard it accelerates, at each health threshold.
pub const PLANTERA_SPEED: f32 = 2.5;
pub const PLANTERA_SPEED_HALF: f32 = 5.0;
pub const PLANTERA_SPEED_QUARTER: f32 = 7.0;
pub const PLANTERA_ACCEL: f32 = 0.025;
pub const PLANTERA_ACCEL_HALF: f32 = 0.05;
/// Dragged out of the jungle it becomes far faster and hits twice as hard — the same refusal the
/// Golem makes about its temple.
pub const PLANTERA_ENRAGED_SPEED: f32 = 8.0;
pub const PLANTERA_ENRAGED_ACCEL: f32 = 0.15;
pub const PLANTERA_ENRAGED_LEASH: f32 = 350.0;
/// How far from its hooks it will swing.
pub const PLANTERA_LEASH: f32 = 500.0;
pub const PLANTERA_LEASH_EXPERT: f32 = 150.0;

/// The first form: armoured, and it shoots.
pub const PLANTERA_DEFENSE: i32 = 36;
pub const PLANTERA_DAMAGE: i32 = 50;
pub const PLANTERA_CHARGE: f32 = 80.0;
pub const PLANTERA_SEED_DAMAGE: i32 = 22;
pub const PLANTERA_SEED_SPEED: f32 = 15.0;
pub const PLANTERA_SEED_SPEED_EXPERT: f32 = 17.0;
/// Below eighty per cent it mixes in thorn balls and spiky seeds (`projectile::ids::PLANTERA_*`),
/// which cost it a pause.
pub const PLANTERA_THORN_BALL_DAMAGE: i32 = 27;
pub const PLANTERA_THORN_BALL_REST: f32 = -30.0;
pub const PLANTERA_SPIKY_DAMAGE: i32 = 31;
pub const PLANTERA_SPIKY_REST: f32 = -120.0;
pub const PLANTERA_MIX_AT: f32 = 0.8;

/// The second form: it drops most of its armour, hits far harder, and grows tentacles.
pub const PLANTERA_SECOND_DEFENSE: i32 = 10;
pub const PLANTERA_SECOND_DAMAGE: i32 = 70;

/// The second form also spits a Spore at the player every 350 ticks — faster the more it is
/// hurt, one more tick shaved per threshold crossed at 40/30/20/10% health (`NPC.cs:32277-32315`).
pub const PLANTERA_SPORE: u16 = 265;
pub const PLANTERA_SPORE_AT: f32 = 350.0;
pub const PLANTERA_SPORE_HEALTH_STEPS: [f32; 4] = [0.4, 0.3, 0.2, 0.1];
pub const PLANTERA_SPORE_SPEED: f32 = 8.0;
pub const PLANTERA_SPORE_JITTER: i32 = 10;

/// A hook re-anchors somewhere new every five to ten seconds, sooner as Plantera weakens.
pub const HOOK_REST: (u32, u32) = (300, 600);
pub const HOOK_STAGGER: (u32, u32) = (60, 300);
pub const HOOK_HURRY_HALF: f32 = 2.0;
pub const HOOK_HURRY_QUARTER: f32 = 2.0;
/// Out of the jungle the timer is drained twice over, once before the "never bitten" reset
/// (`NPC.cs:32333`) and once after it (`:32364`), for eleven a tick against the ordinary one.
pub const HOOK_HURRY_ENRAGED_EARLY: f32 = 4.0;
pub const HOOK_HURRY_ENRAGED: f32 = 6.0;
/// How far from its anchor point it looks for somewhere to bite.
pub const HOOK_SEARCH: i32 = 20;
pub const HOOK_SEARCH_WIDEN: f32 = 100.0;
/// Past half health one attempt in six is spent on the player's own tile instead, if it is walled
/// (`NPC.cs:32400-32410`), which is what lets a hook bite the room you are standing in.
pub const HOOK_WALL_SNAP_ODDS: u32 = 6;
pub const HOOK_SPEED: f32 = 6.0;
pub const HOOK_SPEED_HALF: f32 = 8.0;
pub const HOOK_SPEED_QUARTER: f32 = 10.0;
/// Expert adds one, and another below half health, before any doubling (`NPC.cs:32440-32447`).
pub const HOOK_SPEED_EXPERT: f32 = 1.0;
/// Then an enraged hook, or one with nobody left to chase, travels at twice whatever that came to
/// (`NPC.cs:32448-32455`). This is most of why a Plantera dragged out of the jungle crosses ground
/// the way it does.
pub const HOOK_SPEED_HURRIED: f32 = 2.0;

/// A tentacle orbits Plantera at a radius that grows as Plantera weakens.
pub const TENTACLE_RADIUS: f32 = 200.0;
pub const TENTACLE_RADIUS_QUARTER: f32 = 100.0;
pub const TENTACLE_RADIUS_TENTH: f32 = 100.0;
pub const TENTACLE_ACCEL: f32 = 0.2;
pub const TENTACLE_ACCEL_EXPERT: f32 = 0.3;
pub const TENTACLE_EXPERT_RADIUS: f32 = 300.0;
pub const TENTACLE_CAP: f32 = 8.0;
/// It picks a new offset within its orbit every two to eight seconds.
pub const TENTACLE_DRIFT: (u32, u32) = (120, 480);
pub const TENTACLE_SPREAD: i32 = 100;

// --- Summoning ------------------------------------------------------------------------------------

/// The bosses a player may summon by using an item.
///
/// From `NPCID.Sets.MPAllowedEnemies`. Anything not on this list cannot be summoned however the
/// packet is crafted, which is what stops a client asking for an army of Moon Lords.
pub const SUMMONABLE: [u16; 17] = [
    4,   // Eye of Cthulhu
    13,  // Eater of Worlds
    50,  // King Slime
    125, // Retinazer
    126, // Spazmatism
    127, // Skeletron Prime
    128, // Prime Saw
    129, // Prime Vice
    130, // Prime Cannon
    131, // Prime Laser
    134, // The Destroyer
    222, // Queen Bee
    245, // Golem
    266, // Brain of Cthulhu
    370, // Duke Fishron
    657, // Queen Slime
    668, // Deerclops
];

/// Whether a type may be summoned by a player.
pub fn summonable(npc_type: u16) -> bool {
    SUMMONABLE.contains(&npc_type)
}

/// How far above the player a boss with no ground to stand on appears.
pub const SUMMON_ABOVE: f32 = 150.0;
/// How far around a player the summoner looks for ground, and how many tries it makes.
pub const SUMMON_RANGE_X: i32 = 60;
pub const SUMMON_RANGE_Y: i32 = 40;
pub const SUMMON_ATTEMPTS: usize = 500;
/// It will not put one inside this box around the player.
pub const SUMMON_SAFE_X: i32 = 20;
pub const SUMMON_SAFE_Y: i32 = 12;

// --- Duke Fishron ---------------------------------------------------------------------------------

pub const FISHRON: u16 = 370;
/// What Duke Fishron actually throws (`NPC.cs:49768`, `:49999`).
///
/// This was named `SHARKRON` on the 1.4.3 numbering, where 371 was the sharkron. On 1.4.5.8 the
/// ids shifted: 371 is the detonating bubble, 372 the sharkron and 373 its second form
/// (`NPCID.cs:11813-11817`). The number was right and only the name was a lie, so nothing about
/// the fight changes here.
pub const DETONATING_BUBBLE: u16 = 371;
/// The real sharkron, style 71 (`NPCID.cs:11815`).
///
/// Nothing in the game's own tree spawns it or its second form (373), so nothing here does either;
/// the constant exists because the style is implemented and its test has to name a type that
/// really runs it.
pub const SHARKRON: u16 = 372;

/// How Fishron moves. It runs the same skeleton three times over with different numbers, which is
/// why they are a table rather than a stack of branches.
///
/// Movement only: the damage and armour multipliers are [`FISHRON_DAMAGE`]/[`FISHRON_DEFENSE`],
/// because vanilla picks those off the phase alone while every number here also turns on the half
/// of the attack cycle it is in and on expert (`NPC.cs:49320-49353`).
#[derive(Debug, Clone, Copy)]
pub struct FishronPhase {
    /// How long it holds station before choosing an attack.
    pub hover_ticks: f32,
    pub hover_accel: f32,
    pub hover_speed: f32,
    /// The charge: how long it lasts and how fast it goes.
    pub charge_ticks: f32,
    pub charge_speed: f32,
}

/// The first phase: measured, and it still has its armour. Also the fallback row the second phase
/// drops back to for the half of its cycle that is winding up to a burst or a bubble
/// (`NPC.cs:49320-49322`, `:49339-49340`, which is where `num3`..`num7` are seeded before any
/// branch touches them).
pub const FISHRON_FIRST: FishronPhase = FishronPhase {
    hover_ticks: 60.0,
    hover_accel: 0.45,
    hover_speed: 7.5,
    charge_ticks: 30.0,
    charge_speed: 16.0,
};

/// The same row in expert, which vanilla writes inline on every one of those five numbers.
pub const FISHRON_FIRST_EXPERT: FishronPhase = FishronPhase {
    hover_ticks: 40.0,
    hover_accel: 0.55,
    hover_speed: 8.5,
    charge_ticks: 28.0,
    charge_speed: 17.0,
};

/// The second, below half health: faster, harder, and thinner-skinned. Only for the charging half
/// of its cycle (`flag3 & flag5`, `NPC.cs:49329-49334` and `:49346-49353`).
pub const FISHRON_SECOND: FishronPhase = FishronPhase {
    hover_ticks: 20.0,
    hover_accel: 0.5,
    hover_speed: 8.0,
    charge_ticks: 30.0,
    charge_speed: 16.0,
};

/// The same, in expert. Note the hover goes *up* to forty rather than down to twenty: expert
/// Fishron holds station longer here and charges harder for it.
pub const FISHRON_SECOND_EXPERT: FishronPhase = FishronPhase {
    hover_ticks: 40.0,
    hover_accel: 0.6,
    hover_speed: 10.0,
    charge_ticks: 27.0,
    charge_speed: 21.0,
};

/// The third, in expert below fifteen per cent: no armour at all, and very fast. The one row with
/// no expert variant, because it only exists in expert (`NPC.cs:49323-49328`, `:49341-49345`).
pub const FISHRON_THIRD: FishronPhase = FishronPhase {
    hover_ticks: 30.0,
    hover_accel: 0.7,
    hover_speed: 12.0,
    charge_ticks: 25.0,
    charge_speed: 27.0,
};

/// The hover in the first phase's charging half, which is the one number that half changes
/// (`NPC.cs:49335-49338`) and the same in expert as out of it.
pub const FISHRON_FIRST_HOVER_CHARGING: f32 = 30.0;

/// How far into the attack cycle counts as its charging half: `flag5 = ai[3] < num2 * 2`, with
/// `num2` five in the first phase and three after (`NPC.cs:49303-49304`).
///
/// `ai[3]` only reaches `num2 * 2` on the two hovers that decide the burst and the bubble, so the
/// effect is a longer wind-up before each of those and a short one before every charge.
pub const FISHRON_HALF_CYCLE: i32 = 10;
pub const FISHRON_HALF_CYCLE_LATER: i32 = 6;

/// How hard it hits and how much it soaks, per phase (`NPC.cs:49307-49318`). The first phase is
/// flat `defDamage` with no expert multiplier at all; only the later two take
/// [`FISHRON_EXPERT_PACE`].
pub const FISHRON_DAMAGE: [f32; 3] = [1.0, 1.2, 1.1];
pub const FISHRON_DEFENSE: [f32; 3] = [1.0, 0.8, 0.0];

/// Expert makes everything a fifth faster again.
pub const FISHRON_EXPERT_PACE: f32 = 1.2;

/// Fought anywhere but over the ocean it enrages: hovering at a tenth the interval, hitting and
/// soaking double, charging six pixels a tick faster, and trading its sharkron burst for a bubble
/// (`NPC.cs:49390-49397`, and the two swaps at `:49647` and `:49684`).
///
/// The three ways to be out of bounds are: above the sky line, below the surface, or further than
/// [`FISHRON_ENRAGE_FROM_EDGE`] from both edges of the world, which is to say inland.
pub const FISHRON_ENRAGE_ABOVE: f32 = 800.0;
pub const FISHRON_ENRAGE_FROM_EDGE: f32 = 6400.0;
pub const FISHRON_ENRAGED_HOVER_TICKS: f32 = 10.0;
pub const FISHRON_ENRAGED_CHARGE_BONUS: f32 = 6.0;
pub const FISHRON_ENRAGED_DAMAGE: f32 = 2.0;
pub const FISHRON_ENRAGED_DEFENSE: f32 = 2.0;

/// It arrives faded out and holds still for seventy-five ticks (`ai[0] = -1`,
/// `NPC.cs:49399-49409` and `:49517-49566`, `num21` at `:49367`), rising after the first twenty
/// (`NPC.cs:49518-49535`). It cannot be hurt for any of it.
pub const FISHRON_ARRIVAL_TICKS: f32 = 75.0;
pub const FISHRON_ARRIVAL_RISE_AT: f32 = 20.0;
pub const FISHRON_ARRIVAL_RISE: f32 = -2.0;
pub const FISHRON_ARRIVAL_FADE: i32 = 5;
/// Where it holds station: three hundred pixels to one side, two hundred above.
pub const FISHRON_BESIDE: f32 = 300.0;
pub const FISHRON_ABOVE: f32 = 200.0;
/// The attack cycle: five charges, then a sharkron burst, then bubbles, repeating. Only the
/// first phase runs this cycle; the second and third use the shorter one below
/// (`NPC.cs:49624-49646` vs `49889-49907`).
pub const FISHRON_CYCLE_SHARKRONS: i32 = 10;
pub const FISHRON_CYCLE_BUBBLES: i32 = 11;
/// The first phase's burst: it holds station and throws a sharkron every four ticks, for eighty
/// (`NPC.cs:49354-49357`, `num8`/`num9`/`num10`/`num11` — previously 120/20/10/0.4, roughly a
/// third the sharkrons vanilla throws).
pub const FISHRON_BURST_TICKS: f32 = 80.0;
pub const FISHRON_BURST_EVERY: f32 = 4.0;
pub const FISHRON_BURST_SPEED: f32 = 5.0;
pub const FISHRON_BURST_ACCEL: f32 = 0.3;

/// The second and third phases' cycle is three charges to a burst, not five
/// (`NPC.cs:49889-49907`).
pub const FISHRON_CYCLE_SHARKRONS_LATER: i32 = 6;
pub const FISHRON_CYCLE_BUBBLES_LATER: i32 = 7;
/// Their burst is a different attack entirely: a dash toward the player that curves through the
/// air for its whole duration, spraying a sharkron out to each side of its own heading every four
/// ticks instead of holding station (`NPC.cs:49916-50015`).
pub const FISHRON_BURST_LATER_TICKS: f32 = 120.0;
pub const FISHRON_BURST_LATER_DASH_SPEED: f32 = 20.0;
pub const FISHRON_BURST_LATER_SPRAY_EVERY: f32 = 4.0;
pub const FISHRON_BURST_LATER_SPRAY_SPEED: f32 = 6.0;
/// How far it turns each tick: a full half-circle spread over the whole burst.
pub const FISHRON_BURST_LATER_CURVE: f32 = std::f32::consts::TAU / 60.0;
/// The bubbles: two, thrown from its mouth partway through the wind-up (`NPC.cs:49801`).
pub const FISHRON_BUBBLE_TICKS: f32 = 90.0;
pub const FISHRON_BUBBLE_AT: f32 = 30.0;
pub const FISHRON_BUBBLE_SPEED: (f32, f32) = (2.0, 8.0);
/// Enraged, it starts the wind-up forty ticks from the end instead of ninety, so the bubble is out
/// almost at once (`NPC.cs:49684`).
pub const FISHRON_BUBBLE_ENRAGED_AT: f32 = 40.0;
/// The pause between phases, during which it does nothing at all and cannot be hurt (`num13` and
/// `num14`, `NPC.cs:49359-49360`, spent at `:49823` and `:50075`).
pub const FISHRON_SHIFT_TICKS: f32 = 180.0;
/// Half health starts the second phase; in expert, fifteen per cent starts the third.
pub const FISHRON_SECOND_AT: f32 = 0.5;
pub const FISHRON_THIRD_AT: f32 = 0.15;

// --- The moon events' walking bosses ---------------------------------------------------------------

pub const MOURNING_WOOD: u16 = 325;
pub const EVERSCREAM: u16 = 344;

/// How fast one of them walks, at each health threshold. Daylight makes it flee at eight.
pub const TREE_WALK: f32 = 2.0;
pub const TREE_WALK_HURT: f32 = 3.0;
pub const TREE_WALK_HALF: f32 = 4.0;
pub const TREE_FLEE: f32 = 8.0;
/// It waits five seconds between attacks, less as it is worn down.
pub const TREE_WAIT: f32 = 300.0;
/// Below a quarter, Mourning Wood gains two heavier attacks the Everscream never gets.
pub const TREE_DESPERATE_AT: f32 = 0.25;
/// Inside fifty pixels it stops walking, which is what lets you stand under one.
pub const TREE_TOO_CLOSE: f32 = 50.0;
/// It steers toward the player at a twenty-first of the difference, so it drifts rather than turns.
pub const TREE_STEER: f32 = 20.0;

/// One attack: what it throws, how often, for how long, and how fast.
#[derive(Debug, Clone, Copy)]
pub struct TreeAttack {
    pub projectile: u16,
    pub damage: i32,
    /// A range means the projectile id is chosen from it at random.
    pub projectile_span: u16,
    pub every: f32,
    pub ticks: f32,
    pub from: f32,
    pub speed: f32,
    /// How much the aim is lifted, as a fraction of the horizontal gap: a lob rather than a shot.
    pub arc: f32,
    /// How much the speed grows with distance.
    pub reach_gain: f32,
    pub speed_cap: f32,
    /// How wide the scatter is, in hundredths.
    pub spread: i32,
    pub spread_scale: f32,
    /// Whether it starts partway through, as the two lobbing attacks do.
    pub warmup: f32,
    /// How far the aim is jittered on each axis, in pixels: the Everscream's ornaments are thrown
    /// off by up to this either way in both x and y (`NPC.cs:33088-33089`, `rand(-50, 51)`). Zero
    /// for the attacks that do not scatter their aim point.
    pub scatter: i32,
    /// The most the aim is lofted upward, as a per-shot random percentage of the horizontal gap
    /// (`NPC.cs:33090`, `-= abs(dx) * rand(0, 21) * 0.01`). Zero for the attacks with no loft. This
    /// is distinct from `arc`, which is a fixed lob for the heavy attacks.
    pub loft: i32,
}

const NO_SPREAD: TreeAttack = TreeAttack {
    projectile: 0,
    damage: 0,
    projectile_span: 1,
    every: 15.0,
    ticks: 120.0,
    from: 30.0,
    speed: 10.0,
    arc: 0.0,
    reach_gain: 0.0,
    speed_cap: f32::MAX,
    spread: 20,
    spread_scale: 0.01,
    warmup: 0.0,
    scatter: 0,
    loft: 0,
};

/// Mourning Wood's flaming spears, straight at you.
pub const WOOD_SPEARS: TreeAttack = TreeAttack {
    projectile: 325,
    damage: 50,
    ..NO_SPREAD
};
/// ...and its wave of flaming spheres, lobbed.
pub const WOOD_SPHERES: TreeAttack = TreeAttack {
    projectile: 326,
    projectile_span: 3,
    damage: 40,
    every: 8.0,
    ticks: 300.0,
    speed: 10.0,
    arc: 0.3,
    reach_gain: 0.004,
    speed_cap: 14.0,
    spread: 30,
    warmup: 60.0,
    ..NO_SPREAD
};
/// Below a quarter it fires spears twice as hard, and spheres faster.
pub const WOOD_DESPERATE_SPEARS: TreeAttack = TreeAttack {
    projectile: 325,
    damage: 75,
    every: 30.0,
    ticks: 120.0,
    speed: 16.0,
    spread: 20,
    spread_scale: 0.001,
    ..NO_SPREAD
};
pub const WOOD_DESPERATE_SPHERES: TreeAttack = TreeAttack {
    projectile: 326,
    projectile_span: 3,
    damage: 50,
    every: 10.0,
    ticks: 240.0,
    speed: 12.0,
    arc: 0.2,
    reach_gain: 0.002,
    speed_cap: 16.0,
    spread: 30,
    spread_scale: 0.005,
    ..NO_SPREAD
};
/// The Everscream's ornaments, thrown fast and wide: aimed at you, then jittered up to fifty pixels
/// either way on both axes and lofted upward by up to a fifth of the horizontal gap
/// (`NPC.cs:33087-33093`), so a volley arrives spread out rather than flat and stacked.
pub const SCREAM_ORNAMENTS: TreeAttack = TreeAttack {
    projectile: 345,
    damage: 43,
    every: 5.0,
    ticks: 180.0,
    speed: 12.5,
    spread: 20,
    spread_scale: 0.02,
    scatter: 50,
    loft: 20,
    ..NO_SPREAD
};
/// ...and its pine needles, lobbed slowly.
pub const SCREAM_NEEDLES: TreeAttack = TreeAttack {
    projectile: 346,
    damage: 57,
    every: 15.0,
    ticks: 300.0,
    speed: 4.5,
    arc: 0.3,
    reach_gain: 0.004,
    spread: 30,
    warmup: 60.0,
    ..NO_SPREAD
};

// --- The moon events' flying bosses ----------------------------------------------------------------

pub const PUMPKING: u16 = 327;
pub const PUMPKING_BLADE: u16 = 328;
pub const ICE_QUEEN: u16 = 345;
pub const SANTA_NK1: u16 = 346;

/// Pumpking cycles a mood every five seconds: nought throws spheres, one charges, two sets its
/// blades scything.
pub const PUMPKING_MOOD_TICKS: f32 = 300.0;
pub const PUMPKING_MOODS: u32 = 3;
/// Its hover, two hundred pixels above you, and how fast it closes when it means to.
pub const PUMPKING_ABOVE: f32 = 200.0;
pub const PUMPKING_HOVER: f32 = 6.0;
pub const PUMPKING_HOVER_SMOOTH: f32 = 14.0;
pub const PUMPKING_CHARGE: f32 = 16.0;
pub const PUMPKING_CHARGE_SMOOTH: f32 = 49.0;
pub const PUMPKING_CHARGE_TICKS: f32 = 600.0;
/// It closes faster the further off you are while it is in its charging mood.
pub const PUMPKING_RUSH_STEPS: [(f32, f32); 3] = [(900.0, 12.0), (600.0, 10.0), (300.0, 8.0)];
/// The spheres it throws while hovering (`projectile::ids::PUMPKING_SPHERE`): one of three,
/// picked at random over this span.
pub const PUMPKING_SPHERE_SPAN: u16 = 3;
pub const PUMPKING_SPHERE_DAMAGE: i32 = 40;
pub const PUMPKING_SPHERE_SPEED: f32 = 5.0;
pub const PUMPKING_SPHERE_EVERY: f32 = 30.0;
/// Its two scythe blades, which orbit it.
pub const PUMPKING_BLADES: usize = 2;
pub const PUMPKING_LEASH: f32 = 2000.0;

/// The Ice Queen sweeps back and forth rather than hovering, turning at eight hundred pixels.
pub const QUEEN_SWEEP: f32 = 800.0;
pub const QUEEN_ABOVE_MIN: f32 = 150.0;
pub const QUEEN_ABOVE_MAX: f32 = 200.0;
pub const QUEEN_CLIMB: f32 = 0.2;
pub const QUEEN_CLIMB_CAP: f32 = 8.0;
/// It accelerates and tops out faster the more it is hurt, at every quarter.
pub const QUEEN_PACE: [(f32, f32, f32); 4] = [
    (1.0, 0.45, 7.0),
    (0.75, 0.55, 8.0),
    (0.5, 0.7, 10.0),
    (0.25, 0.8, 11.0),
];

/// Mode 0's forward mist: fired while above the player and either close or already mid-volley
/// (`NPC.cs:33751-33796`). The interval already carries vanilla's `+1`.
pub const ICE_QUEEN_MIST_DAMAGE: i32 = 42;
pub const ICE_QUEEN_MIST_INTERVAL: [(f32, f32); 4] =
    [(1.0, 14.0), (0.75, 13.0), (0.5, 12.0), (0.25, 11.0)];
pub const ICE_QUEEN_MIST_SPEED: [(f32, f32); 4] =
    [(1.0, 6.0), (0.75, 7.0), (0.5, 8.0), (0.25, 9.0)];
pub const ICE_QUEEN_MIST_RANGE: f32 = 500.0;

/// Mode 1: a gentler pursuit that drops ice shards straight down instead of sweeping and firing
/// forward (`NPC.cs:33811-33919`).
pub const ICE_QUEEN_SHARD_DAMAGE: i32 = 37;
/// (accel, cap) for mode 1's chase — already carrying vanilla's flat `-0.05`/`-1` adjustments
/// (`NPC.cs:33823-33843`). ICEQ-1: this was called `ICE_QUEEN_MODE2_PACE` while holding the
/// *second* mode's numbers, one short of the mode-2 that actually exists.
pub const ICE_QUEEN_CHASE_PACE: [(f32, f32, f32); 4] = [
    (1.0, 0.10, 6.0),
    (0.75, 0.12, 7.0),
    (0.5, 0.15, 8.0),
    (0.25, 0.20, 9.0),
];
/// The shard interval, already carrying vanilla's flat `+3`.
pub const ICE_QUEEN_SHARD_INTERVAL: [(f32, f32); 5] = [
    (1.0, 18.0),
    (0.75, 17.0),
    (0.5, 15.0),
    (0.25, 13.0),
    (0.1, 11.0),
];

/// Mode 2: it stops dead, spins, and sprays shards on random bearings (`NPC.cs:33903-33959`).
///
/// The bearing is `rand(-1000, 1001)` on each axis normalised to [`ICE_QUEEN_SPIN_SPEED`], and the
/// muzzle is that bearing times four out from a point twenty pixels above its centre, so the shards
/// appear away from the body rather than inside it.
pub const ICE_QUEEN_SPIN_DAMAGE: i32 = 35;
pub const ICE_QUEEN_SPIN_SPEED: f32 = 15.0;
pub const ICE_QUEEN_SPIN_MUZZLE: f32 = 4.0;
pub const ICE_QUEEN_SPIN_ABOVE: f32 = 20.0;
pub const ICE_QUEEN_SPIN_DRAG: f32 = 0.95;
pub const ICE_QUEEN_SPIN_ROTATION: f32 = 0.2;
/// Vanilla's decrements are cumulative (`num977--`, then `-= 2`, `-= 3`, `-= 4`), so the last two
/// bands are one shot every other tick and then one every tick. Resolved here rather than summed at
/// runtime; the counter fires on `ai[3] > interval`, so the period is one more than the value.
pub const ICE_QUEEN_SPIN_INTERVAL: [(f32, f32); 5] = [
    (1.0, 7.0),
    (0.75, 6.0),
    (0.5, 4.0),
    (0.25, 1.0),
    (0.1, -3.0),
];

/// How long each mode runs before handing back to the picker (`NPC.cs:33804-33805`, `:33913-33917`,
/// `:33955-33959`). Each mode's counter advances by `rand(1, 4)` a tick, not by one, so the mean
/// life of a mode is half the threshold.
pub const ICE_QUEEN_MODE0_AT: f32 = 800.0;
pub const ICE_QUEEN_MODE1_AT: f32 = 600.0;
pub const ICE_QUEEN_MODE2_AT: f32 = 500.0;
pub const ICE_QUEEN_MODE_STEP: std::ops::Range<i32> = 1..4;
/// Mode 0 only ends while the player is inside this (`NPC.cs:33803`); further off it keeps sweeping.
pub const ICE_QUEEN_MODE0_EXIT_RANGE: f32 = 600.0;
/// The picker (`NPC.cs:33961-33978`): one of the three at random, but forced back to the sweep when
/// the player is further off than this, which is what stops it spinning at nothing.
pub const ICE_QUEEN_MODES: u32 = 3;
pub const ICE_QUEEN_MODE_PICK_RANGE: f32 = 1000.0;

/// Santa-NK1 walks and shoots, faster at every quarter of its health.
pub const SANTA_WALK: [(f32, f32); 4] = [(1.0, 2.0), (0.75, 3.0), (0.5, 4.0), (0.25, 5.0)];
pub const SANTA_WAIT: f32 = 300.0;
/// How long a firing burst lasts before it plants and waits again (`NPC.cs:34067`, `ai[1] > 240`).
/// Its own value, not `SANTA_WAIT`: the two are unrelated in vanilla and only looked alike here.
pub const SANTA_FIRE_TICKS: f32 = 240.0;
/// Its gun fires faster as it is worn down: every sixteen ticks down to every eight.
pub const SANTA_FIRE_RATE: [(f32, f32); 4] = [(1.0, 16.0), (0.75, 14.0), (0.5, 11.0), (0.25, 8.0)];
pub const SANTA_BULLET_DAMAGE: i32 = 36;
pub const SANTA_BULLET_SPEED: f32 = 15.0;
pub const SANTA_MUZZLE: f32 = 50.0;
/// Its 2D range (`NPC.cs:33992`, `Distance(player.Center) <= 2000f`), which gates the gun and the
/// rockets only. SNK-2: this was measured horizontally here, so a player directly overhead was
/// treated as in range from any height.
pub const SANTA_LEASH: f32 = 2000.0;
/// It plants rather than driving on once you are this close (`NPC.cs:34165-34168`).
pub const SANTA_PLANT_RANGE: f32 = 50.0;

/// SNK-1: the three weapons besides the machine gun, each on its own random fuse
/// (`NPC.cs:34073-34163`). All three fire from a hardpoint behind and above the cab,
/// `Center - (direction * 24, 64)`; none of them is gated on the firing state, so they go off while
/// it is driving.
pub const SANTA_HARDPOINT: (f32, f32) = (24.0, 64.0);
/// The odds, as a one-in-N a tick, of each weapon starting. Shortened as it is worn down, at every
/// quarter (`NPC.cs:34075-34095`): nine tenths, three quarters, then a half.
pub const SANTA_PRESENT_ODDS: u32 = 600;
pub const SANTA_ROCKET_ODDS: u32 = 1200;
pub const SANTA_MISSILE_ODDS: u32 = 2700;
pub const SANTA_ODDS_STEPS: [(f32, f32); 4] = [(1.0, 1.0), (0.75, 0.9), (0.5, 0.75), (0.25, 0.5)];
/// The present bomb: one lobbed nearly flat in the direction it faces, at a single pixel a tick, so
/// it falls short and sits (`NPC.cs:34098-34106`).
pub const SANTA_PRESENT_DAMAGE: i32 = 80;
pub const SANTA_PRESENT_SPEED: f32 = 1.0;
/// The rocket volley: once started it runs a hundred ticks, firing every twelfth, aimed at you with
/// fifty pixels of scatter (`NPC.cs:34112-34135`).
pub const SANTA_ROCKET_DAMAGE: i32 = 42;
pub const SANTA_ROCKET_SPEED: f32 = 12.5;
pub const SANTA_ROCKET_EVERY: f32 = 12.0;
pub const SANTA_ROCKET_SPREAD: i32 = 50;
/// The missile volley: the same hundred ticks, every ninth, fired near-vertically and unaimed
/// (`NPC.cs:34141-34158`).
pub const SANTA_MISSILE_DAMAGE: i32 = 50;
pub const SANTA_MISSILE_SPEED: f32 = 11.0;
pub const SANTA_MISSILE_EVERY: f32 = 9.0;
pub const SANTA_MISSILE_LEAN: i32 = 100;
pub const SANTA_MISSILE_RISE: f32 = -300.0;
/// How long either volley runs once started.
pub const SANTA_VOLLEY_TICKS: f32 = 100.0;
/// The missile volley's counter starts at two rather than one, so its first shot lands on the
/// seventh tick rather than the ninth (`NPC.cs:34139`).
pub const SANTA_MISSILE_START: f32 = 2.0;

/// SNK-3: its vertical routine, which was absent entirely (`NPC.cs:34188-34237`).
///
/// It probes an 80x20 box under its treads: on solid ground it climbs, in open air it falls, and a
/// player standing directly underneath makes it drop hard.
pub const SANTA_PROBE: (i32, i32) = (80, 20);
pub const SANTA_CLIMB_CREEP: f32 = 0.025;
pub const SANTA_CLIMB: f32 = 0.2;
pub const SANTA_CLIMB_CAP: f32 = -4.0;
pub const SANTA_FALL: f32 = 0.5;
pub const SANTA_FALL_CAP: f32 = 10.0;
/// The band the falling creep applies in (`NPC.cs:34225`). The climbing one uses [`SANTA_CLIMB`]
/// itself as its threshold (`NPC.cs:34206`), so the two are not symmetric.
pub const SANTA_CREEP_BAND: f32 = 0.1;
/// How far above the player's feet its own have to be before the drop counts (`NPC.cs:34193`).
pub const SANTA_DROP_CLEARANCE: f32 = 16.0;

// --- Queen Slime ----------------------------------------------------------------------------------

pub const QUEEN_SLIME: u16 = 657;
/// Below half health it stops hopping and takes to the air.
pub const QUEEN_SLIME_FLIES_AT: f32 = 0.5;
/// It waits this long between attacks: a second on the ground, two in the air.
pub const QUEEN_SLIME_WAIT: f32 = 60.0;
pub const QUEEN_SLIME_WAIT_FLYING: f32 = 120.0;
/// The four-hop set: two identical low hops, a slightly higher third, then one high that ends it
/// (`NPC.cs:45946-46023`; the ground case's first hop repeats because `ai[2]==0` and `ai[2]==1`
/// both fall through to the same `else` branch).
pub const QUEEN_SLIME_HOPS: [(f32, f32, f32); 4] = [
    // rise, drift, rest afterwards
    (-8.0, 4.0, -40.0),
    (-8.0, 4.0, -40.0),
    (-6.0, 4.5, -40.0),
    (-13.0, 3.5, 0.0),
];
/// The hop charge fills faster at two thirds and a third of its health.
pub const QUEEN_SLIME_CHARGE: f32 = 4.0;
pub const QUEEN_SLIME_CHARGE_STEPS: [f32; 2] = [0.66, 0.33];
/// Losing sight of it, or being far above it, builds a teleport.
pub const QUEEN_SLIME_CHEESE_RATE: f32 = 1.5;
pub const QUEEN_SLIME_CHEESE_AT: f32 = 300.0;
pub const QUEEN_SLIME_CHEESE_MAX: f32 = 360.0;
pub const QUEEN_SLIME_REACH: f32 = 320.0;
/// The teleport: it fades out over a second and back in over half of one.
pub const QUEEN_SLIME_FADE_OUT: f32 = 60.0;
pub const QUEEN_SLIME_FADE_IN: f32 = 30.0;
/// Past five hundred tiles it gives up entirely.
pub const QUEEN_SLIME_LEASH_TILES: f32 = 500.0;
/// Flying: it holds above you and dives.
pub const QUEEN_SLIME_HOVER: f32 = 250.0;
pub const QUEEN_SLIME_DIVE_RANGE: f32 = 250.0;

/// The dive: a stationary burst dropped where it lands, on the ground or in the air alike
/// (`NPC.cs:46024-46118`).
pub const QUEEN_SLIME_DIVE_DAMAGE: i32 = 40;
/// It hangs above the aim point for up to this many ticks before committing to the fall.
pub const QUEEN_SLIME_DIVE_WINDUP: f32 = 60.0;
/// How far above the player it aims before dropping, and how fast it closes on that point.
pub const QUEEN_SLIME_DIVE_ABOVE: f32 = 384.0;
pub const QUEEN_SLIME_DIVE_APPROACH_SPEED: f32 = 20.0;
/// Once it commits, gravity is hand-rolled: this much added to its fall speed each tick, capped.
pub const QUEEN_SLIME_DIVE_FALL_ACCEL: f32 = 1.0;
pub const QUEEN_SLIME_DIVE_FALL_CAP: f32 = 14.0;

/// The swoop: a ring of these fired outward once it commits — six on the ground, ten in the air
/// (`NPC.cs:46159-46236`; vanilla's getGoodWorld bump to fifteen is disclosed and not counted).
pub const QUEEN_SLIME_RING_DAMAGE: i32 = 30;
pub const QUEEN_SLIME_RING_SPEED: f32 = 9.0;
pub const QUEEN_SLIME_RING_COUNT_GROUND: usize = 6;
pub const QUEEN_SLIME_RING_COUNT_FLYING: usize = 10;
/// How long it hangs before firing the ring: fifty ticks of windup, then ten more once committed.
pub const QUEEN_SLIME_SWOOP_WINDUP: f32 = 50.0;
pub const QUEEN_SLIME_SWOOP_COMMIT: f32 = 10.0;

/// The three minions she sheds as she is worn down (`NPC.cs:46286-46298`).
///
/// One or two of these, picked evenly, every time she loses a fixed slice of her maximum life. The
/// slice is measured against `localAI[0]`, a watermark she seeds with `lifeMax` on her first tick
/// (`NPC.cs:45703-45708`) and moves down to her current life each time she sheds, so the count is
/// damage-driven rather than timed: burst her and the adds arrive in a wave.
pub const QUEEN_SLIME_ADDS: [u16; 3] = [658, 659, 660];
/// Two percent of her maximum life on the ground, and a shorter one and a half in the air
/// (`NPC.cs:46271-46275`), so the flying half sheds them faster for the same damage.
pub const QUEEN_SLIME_ADD_STEP: f32 = 0.02;
pub const QUEEN_SLIME_ADD_STEP_FLYING: f32 = 0.015;
/// Each one arrives dazed: `ai[0] = -500 * rand(3)` (`NPC.cs:46303`), so roughly two in three sit
/// out several seconds of their slime hop timer before they start moving.
pub const QUEEN_SLIME_ADD_DELAY: f32 = -500.0;
pub const QUEEN_SLIME_ADD_DELAY_STEPS: u32 = 3;

// --- The Lunatic Cultist ---------------------------------------------------------------------------

pub const CULTIST: u16 = 439;
pub const CULTIST_CLONE: u16 = 440;
/// Spawned five at a time by the shadowflame attack below.
pub const CULTIST_ANCIENT_LIGHT: u16 = 522;
/// Unused here: the NPC an expert-only, chance-triggered variant of that same attack summons
/// instead (`NPC.cs:65471-65474`, gated on `CountNPCS(523) < 10`). Not implemented — out of the
/// audited scope, and its trigger is a random substitution into an otherwise-fixed script.
pub const CULTIST_ANCIENT_DOOM: u16 = 523;

/// It arrives over seven seconds before it will fight.
pub const CULTIST_ARRIVAL: f32 = 420.0;
/// Between attacks it pauses for two thirds of a second.
pub const CULTIST_PAUSE: f32 = 40.0;
/// Below half health it sheds a third of its armour.
pub const CULTIST_HALF_DEFENSE: f32 = 0.65;

/// The attack scripts, indexed by how many attacks have been made — one for above half health,
/// one for at or below it (`NPC.cs:65361-65461`). Nought is "move", which is why it spends so
/// much of the fight drifting into a new position: every other entry is a reposition. Both
/// sequences are fixed, so the fight is memorisable, and that is the point of it. The digits here
/// are this module's own state numbering (1 ice, 2 fireballs, 3 lightning, 4 ritual, 5
/// shadowflame — see `cultist::state`), translated from vanilla's `num13` codes (1 fire, 2 ice, 3
/// lightning, 4 ritual, 5 shadowflame) so they route through the existing match arms unchanged.
pub const CULTIST_SCRIPT_HEALTHY: [u8; 12] = [0, 2, 0, 1, 0, 3, 0, 2, 0, 1, 0, 4];
pub const CULTIST_SCRIPT_WOUNDED: [u8; 14] = [0, 2, 0, 5, 0, 3, 0, 5, 0, 1, 0, 3, 0, 4];

/// The ice mist: a slow, heavy shot. (`ProjectileID.CultistBossIceMist`, `NPC.cs:65569-65639`.)
pub const CULTIST_ICE_DAMAGE: i32 = 35;
pub const CULTIST_ICE_EVERY: f32 = 120.0;
pub const CULTIST_ICE_EVERY_EXPERT: f32 = 90.0;
/// The fireballs: a burst of three, or four in expert.
/// (`ProjectileID.CultistBossFireBall`, `NPC.cs:65640-65719`.)
pub const CULTIST_FIRE_DAMAGE: i32 = 30;
pub const CULTIST_FIRE_EVERY: f32 = 18.0;
pub const CULTIST_FIRE_EVERY_EXPERT: f32 = 12.0;
pub const CULTIST_FIRE_COUNT: i32 = 3;
pub const CULTIST_FIRE_COUNT_EXPERT: i32 = 4;
/// The lightning orb: rarer and harder.
/// (`ProjectileID.CultistBossLightningOrb`, `NPC.cs:65720-65779`.)
pub const CULTIST_LIGHTNING_DAMAGE: i32 = 45;
pub const CULTIST_LIGHTNING_EVERY: f32 = 80.0;
pub const CULTIST_LIGHTNING_EVERY_EXPERT: f32 = 40.0;
/// The shadowflame: five Ancient Lights fanned out toward you, twice — once wounded scripts ever
/// reach it (`NPC.cs:65949-66020`).
pub const CULTIST_SHADOWFLAME_EVERY: f32 = 20.0;
pub const CULTIST_SHADOWFLAME_EVERY_EXPERT: f32 = 30.0;
pub const CULTIST_SHADOWFLAME_COUNT: i32 = 2;
pub const CULTIST_SHADOWFLAME_SPAWNS: usize = 5;
pub const CULTIST_SHADOWFLAME_SPEED: f32 = 8.0;
/// The angular step between each of the five, and the arc they fan across.
pub const CULTIST_SHADOWFLAME_ANGLE_STEP: f32 = std::f32::consts::TAU / 25.0;
/// How it repositions: a two-hundred-by-three-hundred ellipse above you, shared out between it and
/// its clones so they fan rather than stack.
pub const CULTIST_ORBIT: (f32, f32) = (300.0, 200.0);
pub const CULTIST_ORBIT_SPREAD: f32 = 0.4;
pub const CULTIST_MOVE_STEP: f32 = 50.0;
/// The ritual: it makes clones and only the real one flinches.
///
/// Not a fixed four. Each ritual tops the group up by at most two, and never past six in all
/// (`NPC.cs:65808-65812`, `num28 = 6 - existing`, clamped to 2), so the first ritual is a choice
/// between three and the last between seven. They are laid out on a circle of this radius around
/// the boss, which then takes the slot furthest from the player (`NPC.cs:65798`, `:65826`).
pub const CULTIST_CLONES_PER_RITUAL: usize = 2;
pub const CULTIST_CLONES_MAX: usize = 6;
pub const CULTIST_CLONE_RING: f32 = 180.0;
pub const CULTIST_RITUAL_TICKS: f32 = 420.0;
pub const CULTIST_RITUAL_WINDOW: (f32, f32) = (120.0, 420.0);
/// How many clones a *correct* guess destroys (`NPC.cs:65229-65232`, `num9`).
///
/// The name is vanilla's own framing: `num9` counts the ones that are culled, and the ten is more
/// than can ever be out, so a classic guess clears the group outright. Expert's three is the real
/// number: against a group grown past three, some always survive, and that asymmetry is the expert
/// fight. This doc comment used to say the opposite - that these were the clones' *lights* that
/// *survive* - which would have had anyone wiring it up implement the mechanic backwards.
pub const CULTIST_RIGHT_GUESS_CULL: usize = 10;
pub const CULTIST_RIGHT_GUESS_CULL_EXPERT: usize = 3;
/// What a wrong guess costs: the decoy dies and the real one is stunned for two seconds
/// (`NPC.cs:65203-65206` sets its owner to state 6, `NPC.cs:65936-65948` counts that state out).
pub const CULTIST_STUN_TICKS: f32 = 120.0;

/// The tablet the cultists gather at, and the devotes that kneel around it.
pub const CULTIST_TABLET: u16 = 437;
pub const CULTIST_DEVOTE: u16 = 438;
pub const CULTIST_ARCHER: u16 = 379;
/// Four gather: two archers and two devotes.
pub const TABLET_CULTISTS: usize = 4;
/// Once they are all dead the tablet shatters over five seconds and the Cultist rises.
pub const TABLET_SHATTER_TICKS: f32 = 300.0;
pub const TABLET_SHARD_FROM: f32 = 120.0;
pub const TABLET_SHARD_EVERY: f32 = 10.0;
/// A devote paces, turning to face the tablet, and gives up after five seconds of nothing.
pub const DEVOTE_DRAG: f32 = 0.93;
pub const DEVOTE_PATIENCE: f32 = 300.0;

// --- The Moon Lord --------------------------------------------------------------------------------

pub const MOON_LORD_CORE: u16 = 398;
pub const MOON_LORD_HEAD: u16 = 396;
pub const MOON_LORD_HAND: u16 = 397;
pub const MOON_LORD_FREE_EYE: u16 = 400;
pub const MOON_LORD_LEECH: u16 = 401;

/// Each part runs one of three attack scripts. A script is five entries of
/// `(attack, how long it lasts)`, and which script a part gets is fixed when it opens — so the
/// three eyes are always doing different things at once, and the fight has a shape rather than a
/// rhythm.
///
/// Attack 0 is "wait". Attack 1 is a hand's rapid eye-stream, or, for the head, the charged
/// deathray (a hundred and eighty ticks of wind-up, then the beam). Attack 2 is a hand's six-sphere
/// barrage, or, for the head, the leech attack. Attack 3 is the spread of bolts, the same for both.
/// The row a part runs is not random: a hand takes the row for its side (left row 0, right row 1),
/// the head takes row 2 (`NPC.cs:42032` `num6 = (ai[2]==0)?0:1`, `NPC.cs:42530` `num5 = 2`).
pub const MOON_LORD_SCRIPTS: [[(u8, i32); 5]; 3] = [
    [(0, 50), (1, 70), (2, 330), (0, 60), (3, 90)],
    [(1, 70), (0, 50), (3, 90), (0, 60), (2, 330)],
    [(3, 180), (0, 30), (2, 435), (3, 180), (1, 375)],
];

/// The core hangs a hundred and thirty pixels below the player and cannot be hurt until every eye
/// is open.
pub const MOON_LORD_BELOW: f32 = 130.0;
pub const MOON_LORD_SPEED: f32 = 8.0;
pub const MOON_LORD_ACCEL: f32 = 0.5;
/// Its parts stand this far out from the core: `Center + (350 * side, -100)` for a hand
/// (`NPC.cs:42073`, `:42094`) and `Center + (0, -400)` for the head (`NPC.cs:42534`). The hands
/// were fifty pixels too far out, which widens the whole arena the fight is fought in.
pub const MOON_LORD_HAND_OUT: f32 = 350.0;
pub const MOON_LORD_HAND_UP: f32 = 100.0;
pub const MOON_LORD_HEAD_UP: f32 = 400.0;
/// The opening and the death both take a second.
pub const MOON_LORD_OPENING: f32 = 60.0;
/// The death drama runs for ten seconds.
pub const MOON_LORD_DEATH_TICKS: f32 = 600.0;
/// Past this it leaves.
pub const MOON_LORD_FIGHTING_DISTANCE: f32 = 4500.0;

/// What its parts throw for. The damage each `NewProjectile` call passes in `AI_078`/`AI_079`: the
/// eye stream (30, `NPC.cs:42155`), the sphere barrage (40, `NPC.cs:42199`), the head's deathray
/// (75, `NPC.cs:42667`) and the bolt spread (30, `NPC.cs:42502`). The ids themselves are
/// `projectile::ids::PHANTASMAL_*`.
pub const PHANTASMAL_EYE_DAMAGE: i32 = 30;
pub const PHANTASMAL_SPHERE_DAMAGE: i32 = 40;
pub const PHANTASMAL_DEATHRAY_DAMAGE: i32 = 75;
pub const PHANTASMAL_BOLT_DAMAGE: i32 = 30;
/// The bolts come in threes, seven ticks apart: the hand and the head both fire at `num2 - 14`,
/// `num2 - 7` and `num2` (`NPC.cs:42274-42278` and `:42780-42784`), and the shot leaves at eight
/// pixels a tick.
pub const MOON_LORD_BOLT_EVERY: f32 = 7.0;
pub const MOON_LORD_BOLT_SPEED: f32 = 8.0;
/// The head's deathray sweeps across nine seconds.
pub const MOON_LORD_RAY_SWEEP: f32 = 540.0;

/// How an eye socket's eyelid works: the whole "you cannot hurt it while the eye is shut" mechanic.
///
/// Each attack step names an openness from 0 (wide open) to 3 (shut). A counter eases one step a
/// tick toward `openness * STEP` and is clamped at `SHUT`; the part takes no damage while the
/// counter is at the cap (`dontTakeDamage = frameCounter >= 21.0`, `NPC.cs:42023` for a hand;
/// `dontTakeDamage = localAI[3] >= 15f`, `NPC.cs:42532` for the head). The ramp is what makes it a
/// window rather than a switch: a hand is shut for about a seventh of its cycle, the head for well
/// over a third of its own.
pub const EYE_SOCKET_LID_STEP_HAND: f32 = 7.0;
pub const EYE_SOCKET_LID_SHUT_HAND: f32 = 21.0;
pub const EYE_SOCKET_LID_STEP_HEAD: f32 = 5.0;
pub const EYE_SOCKET_LID_SHUT_HEAD: f32 = 15.0;
/// A free eye, once its socket is broken, hunts on its own.
///
/// Its own ten-step script, `MoonLordAttacksArray2` (`NPC.cs:7009-7033`): a rest of
/// `(1200 - 935) / 5 = 53` ticks between every attack, and five attacks that fill the other 935.
/// Attack 0 is the chase, 1 the three-bolt spread, 2 the six-sphere gather-and-launch, 3 the
/// spinning eye-spray, 4 the charged deathray. It is the whole second half of the fight: an eye
/// that only chases is an eye that can be ignored.
pub const TRUE_EYE_REST: i32 = 53;
pub const TRUE_EYE_SCRIPT: [(u8, i32); 10] = [
    (0, TRUE_EYE_REST),
    (1, 90),
    (0, TRUE_EYE_REST),
    (2, 135),
    (0, TRUE_EYE_REST),
    (3, 200),
    (0, TRUE_EYE_REST),
    (4, 375),
    (0, TRUE_EYE_REST),
    (2, 135),
];
/// The chase (`NPC.cs:42988-42996`): twenty-four pixels a tick toward a point two hundred pixels
/// above the player, eased over thirty ticks so it swings rather than tracks.
pub const FREE_EYE_SPEED: f32 = 24.0;
pub const FREE_EYE_ABOVE: f32 = 200.0;
pub const FREE_EYE_SMOOTH: f32 = 30.0;
/// What a True Eye throws. Its own damage figures, all higher than the parts': the bolt spread
/// (462, 35, `NPC.cs:43078`), the six spheres (454, 40, `NPC.cs:43132`), the eye-spray (452, 35,
/// `NPC.cs:43236`) and its deathray (455, 50, `NPC.cs:43343`).
pub const TRUE_EYE_BOLT_DAMAGE: i32 = 35;
pub const TRUE_EYE_SPHERE_DAMAGE: i32 = 40;
pub const TRUE_EYE_SPRAY_DAMAGE: i32 = 35;
pub const TRUE_EYE_DEATHRAY_DAMAGE: i32 = 50;
/// A leech ferries life back to whichever part is most hurt.
pub const LEECH_TICKS: f32 = 90.0;
pub const LEECH_HEAL: i32 = 1000;
/// The head puts leeches out at three fixed marks in its leech step, not on a metronome
/// (`NPC.cs:42741`, `num == 120f || num == 180f || num == 240f`), and they arrive on the *player*,
/// not on the boss (`NewNPC(..., Main.player[target].Center, 401)`).
pub const LEECH_MARKS: [f32; 3] = [120.0, 180.0, 240.0];

/// The Old One's Army: the crystal, its lane portals and everything that comes out of them.
///
/// Every one of these is a separate type per tier rather than one type with a level, which is why
/// the wave tables read as long lists of ids.
pub const MARTIAN_SAUCER_CORE: u16 = 395;
pub const DD2_ETERNIA_CRYSTAL: u16 = 548;
pub const DD2_LANE_PORTAL: u16 = 549;
pub const DD2_BARTENDER: u16 = 550;
pub const DD2_BETSY: u16 = 551;
pub const DD2_GOBLIN_T1: u16 = 552;
pub const DD2_GOBLIN_T2: u16 = 553;
pub const DD2_GOBLIN_T3: u16 = 554;
pub const DD2_GOBLIN_BOMBER_T1: u16 = 555;
pub const DD2_GOBLIN_BOMBER_T2: u16 = 556;
pub const DD2_GOBLIN_BOMBER_T3: u16 = 557;
pub const DD2_WYVERN_T1: u16 = 558;
pub const DD2_WYVERN_T2: u16 = 559;
pub const DD2_WYVERN_T3: u16 = 560;
pub const DD2_JAVELINST_T1: u16 = 561;
pub const DD2_JAVELINST_T2: u16 = 562;
pub const DD2_JAVELINST_T3: u16 = 563;
pub const DD2_DARK_MAGE_T1: u16 = 564;
pub const DD2_DARK_MAGE_T3: u16 = 565;
pub const DD2_SKELETON_T1: u16 = 566;
pub const DD2_SKELETON_T3: u16 = 567;
pub const DD2_WITHER_BEAST_T2: u16 = 568;
pub const DD2_WITHER_BEAST_T3: u16 = 569;
pub const DD2_DRAKIN_T2: u16 = 570;
pub const DD2_DRAKIN_T3: u16 = 571;
pub const DD2_KOBOLD_WALKER_T2: u16 = 572;
pub const DD2_KOBOLD_WALKER_T3: u16 = 573;
pub const DD2_KOBOLD_FLYER_T2: u16 = 574;
pub const DD2_KOBOLD_FLYER_T3: u16 = 575;
pub const DD2_OGRE_T2: u16 = 576;
pub const DD2_OGRE_T3: u16 = 577;
pub const DD2_LIGHTNING_BUG_T3: u16 = 578;
/// Not part of the event at all — it shares the troops' routine, and swims.
pub const GOBLIN_SHARK: u16 = 620;
pub const FAIRY_CRITTER_PINK: u16 = 583;
pub const FAIRY_CRITTER_GREEN: u16 = 584;
pub const FAIRY_CRITTER_BLUE: u16 = 585;
pub const HALLOW_BOSS: u16 = 636;

/// What style 108 — the diving flyers — needs to know about a type.
///
/// It is one routine with two very different creatures in it: the wyvern circles, dives, and pulls
/// out; the kobold flyer circles, dives, and does not pull out. The difference is entirely in these
/// numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DivingFlyer {
    /// How much knockback it takes while circling. It takes none at all once committed.
    pub knockback_resist: f32,
    /// Circling speed.
    pub speed: f32,
    /// How far above its target it wants to sit.
    pub hover_above: f32,
    /// How close it must be before it will commit.
    pub engage: f32,
    /// Smoothing while circling: larger is lazier.
    pub approach: f32,
    /// How long it hangs still before the dive.
    pub wind_up: f32,
    /// How much speed it sheds each tick of that wind-up.
    pub decay: f32,
    /// Random spread on the dive, in fortieths of a pixel per tick.
    pub spread: i32,
    /// Dive speed.
    pub dive_speed: f32,
    /// How long a dive runs before it is allowed to end.
    pub dive_ticks: f32,
    /// How far past its target it must get before a dive counts as finished.
    pub break_off: f32,
    /// Steering smoothing during the dive.
    pub turn: f32,
    /// How much speed the dive gains per tick, before smoothing.
    pub accel: f32,
    /// Below this speed the dive is spent.
    pub min_dive_speed: f32,
    /// Downward drift during the wind-up.
    pub sink: f32,
    /// Whether hitting the world — or the player — ends it in an explosion.
    pub splat: bool,
    /// Whether it ever pulls out of a dive of its own accord.
    pub commits: bool,
    /// How long it will chase without a line of sight before taking one on faith.
    pub patience: f32,
    /// How hard the flock pushes itself apart.
    pub separation: f32,
}

pub const DIVING_FLYER_EXPLOSION: i32 = 192;
pub const DIVING_FLYER_EXPLOSION_DAMAGE: i32 = 80;

/// The numbers for one of style 108's types.
pub fn diving_flyer(npc_type: u16) -> DivingFlyer {
    let base = DivingFlyer {
        knockback_resist: 0.4,
        speed: 10.0,
        hover_above: 200.0,
        engage: 750.0,
        approach: 30.0,
        wind_up: 30.0,
        decay: 0.95,
        spread: 50,
        dive_speed: 14.0,
        dive_ticks: 30.0,
        break_off: 100.0,
        turn: 20.0,
        accel: 0.0,
        min_dive_speed: 7.0,
        sink: 0.0,
        splat: true,
        commits: false,
        patience: 120.0,
        separation: 0.05,
    };
    match npc_type {
        DD2_WYVERN_T1 | DD2_WYVERN_T2 | DD2_WYVERN_T3 => DivingFlyer {
            knockback_resist: match npc_type {
                DD2_WYVERN_T2 => 0.5,
                DD2_WYVERN_T3 => 0.2,
                _ => 0.7,
            },
            speed: 3.0,
            hover_above: 400.0,
            engage: 500.0,
            approach: 90.0,
            wind_up: 20.0,
            dive_speed: 8.0,
            break_off: 150.0,
            turn: 60.0,
            accel: 0.05,
            min_dive_speed: 6.0,
            spread: 0,
            splat: false,
            ..base
        },
        DD2_KOBOLD_FLYER_T2 | DD2_KOBOLD_FLYER_T3 => DivingFlyer {
            knockback_resist: if npc_type == DD2_KOBOLD_FLYER_T3 {
                0.4
            } else {
                0.6
            },
            speed: 4.0,
            hover_above: 400.0,
            engage: 500.0,
            approach: 90.0,
            dive_speed: 8.0,
            break_off: 150.0,
            turn: 10.0,
            accel: 0.05,
            min_dive_speed: 0.0,
            spread: 3,
            sink: -0.1,
            commits: true,
            ..base
        },
        _ => base,
    }
}

/// How long a lane portal takes to open, and how long it takes to fade once the event ends.
pub const LANE_PORTAL_OPENING: f32 = 180.0;
pub const LANE_PORTAL_CLOSING: f32 = 550.0;
/// Where a gate stands relative to the arena edge, in tiles.
pub const LANE_PORTAL_INSET: i32 = 2;
/// How long the crystal's own countdown between checks is.
pub const CRYSTAL_TICK: f32 = 180.0;
/// The two death dramas, won and lost, both run ten seconds.
pub const CRYSTAL_DRAMA: f32 = 600.0;

/// The lightning bug: hover at range, gather, and throw a bolt.
pub const LIGHTNING_BUG_SPEED: f32 = 4.0;
pub const LIGHTNING_BUG_SMOOTHING: f32 = 20.0;
pub const LIGHTNING_BUG_RANGE: f32 = 200.0;
/// How close it will let its target get vertically before it climbs away.
pub const LIGHTNING_BUG_FLOOR: f32 = 50.0;
pub const LIGHTNING_BUG_SETTLE: f32 = 1.0;
pub const LIGHTNING_BUG_DECAY: f32 = 0.96;
pub const LIGHTNING_BUG_CHARGE: f32 = 5.0;
pub const LIGHTNING_BUG_COOLDOWN: f32 = 30.0;
pub const LIGHTNING_BUG_SEPARATION: f32 = 0.1;
pub const LIGHTNING_BUG_BOLT_DAMAGE: i32 = 50;
pub const LIGHTNING_BUG_BOLT_SPEED: f32 = 10.0;
/// How long a spawned army enemy spends fading in out of its gate.
pub const ARMY_FADE_IN: f32 = 60.0;

/// What a fairy will lead you to, and how badly it wants to.
///
/// This is the ore finder's own priority list: higher wins, and a tie is broken by distance. The
/// fairy is not looking for treasure in general — it is looking for the *best* thing in a hundred
/// and fifty tiles by this ranking, which is why one will fly past a copper vein to reach a chest.
pub fn ore_finder_priority(block: u16) -> i16 {
    match block {
        28 => 100,
        404 | 407 => 150,
        7 => 200,
        166 => 210,
        6 => 220,
        167 => 230,
        9 => 240,
        168 => 250,
        8 => 260,
        169 => 270,
        22 => 300,
        204 => 310,
        37 => 400,
        21 | 441 | 467 | 468 => 500,
        12 | 639 | 665 => 550,
        107 => 600,
        221 => 610,
        108 => 620,
        222 => 630,
        111 => 640,
        223 => 650,
        129 => 675,
        211 => 700,
        227 => 750,
        656 | 701 => 760,
        751 | 752 => 770,
        236 | 702 => 810,
        _ => 0,
    }
}

/// Whether a fairy will lead anyone to this block at all.
///
/// A much shorter list than the priorities: the ore finder ranks everything, but a fairy only
/// bothers with the good stuff — chests, life crystals, the hardmode ores and the rarer plants.
pub fn fairy_lures_to(block: u16) -> bool {
    matches!(
        block,
        8 | 12
            | 21
            | 107
            | 108
            | 111
            | 169
            | 211
            | 221
            | 222
            | 223
            | 227
            | 236
            | 467
            | 639
            | 665
            | 702
    )
}

/// Whether this block counts for the ore finder in the state it is in.
///
/// Two blocks are only sometimes worth pointing at: an enchanted sword shrine's sword is a
/// particular frame of tile 227, and a plantera bulb is a particular frame of 129.
pub fn valid_for_ore_finder(block: u16, frame_x: i16) -> bool {
    match block {
        227 => (272..=374).contains(&frame_x),
        129 => frame_x >= 324,
        _ => true,
    }
}

/// Which blocks are ore, for the fairy's "is this a real vein or one stray block" check.
pub fn is_ore(block: u16) -> bool {
    matches!(
        block,
        6 | 7
            | 8
            | 9
            | 22
            | 37
            | 58
            | 107
            | 108
            | 111
            | 166
            | 167
            | 168
            | 169
            | 204
            | 211
            | 221
            | 222
            | 223
    )
}

/// How far a fairy will look for something worth showing you, in tiles.
pub const FAIRY_SEARCH_X: i32 = 75;
pub const FAIRY_SEARCH_Y: i32 = 50;
/// How many blocks of the same ore must be within three tiles before a vein counts as a vein.
pub const FAIRY_VEIN: usize = 40;
/// How close you have to get before a fairy notices you.
pub const FAIRY_NOTICE: f32 = 250.0;
/// How far ahead of you it will get while leading.
pub const FAIRY_LEAD: f32 = 300.0;
/// How long it celebrates before setting off, and how long it dances at the end before it goes.
pub const FAIRY_CELEBRATE: f32 = 210.0;
pub const FAIRY_ARRIVAL: f32 = 200.0;
/// How long a fairy will stay with you at all: five minutes, and then it leaves.
pub const FAIRY_PATIENCE: f32 = 18000.0;

/// The Martian saucer's parts. The core is the thing that flies; everything else rides it.
pub const MARTIAN_SAUCER_BODY: u16 = 392;
pub const MARTIAN_SAUCER_TURRET: u16 = 393;
pub const MARTIAN_SAUCER_CANNON: u16 = 394;
/// Its deathray (`projectile::ids::SAUCER_DEATHRAY`), fired once at the start of each strafe of
/// the last phase.
pub const SAUCER_DEATHRAY_DAMAGE: i32 = 80;
/// Whole, it is not toothless: a single, weaker deathray as the strafe of its circuit opens.
pub const SAUCER_CIRCUIT_RAY_AT: f32 = 20.0;
pub const SAUCER_CIRCUIT_RAY_DAMAGE: i32 = 50;
/// Missiles, sprayed loosely outward through the whole overhead hover of an intact circuit.
pub const SAUCER_MISSILE_DAMAGE: i32 = 50;
pub const SAUCER_MISSILE_DAMAGE_EXPERT: i32 = 37;
pub const SAUCER_MISSILE_SPEED: f32 = 8.0;
pub const SAUCER_MISSILE_FROM: f32 = 440.0;
pub const SAUCER_MISSILE_PERIOD: f32 = 20.0;
/// Lasers, aimed at you, through the whole low hold of an intact circuit.
pub const SAUCER_LASER_DAMAGE: i32 = 35;
pub const SAUCER_LASER_DAMAGE_EXPERT: i32 = 30;
pub const SAUCER_LASER_SPEED: f32 = 16.0;
pub const SAUCER_LASER_FROM: f32 = 280.0;
pub const SAUCER_LASER_PERIOD: f32 = 6.0;
/// How far the parts sit from the core when it puts itself together.
pub const SAUCER_PART_OUT: f32 = 150.0;
/// Beyond this it gives up on you.
pub const SAUCER_GIVE_UP: f32 = 5600.0;
/// The attack cycle, and the boundaries of its six phases within it.
pub const SAUCER_CYCLE: f32 = 600.0;
pub const SAUCER_PHASES: [(f32, u8); 6] = [
    (580.0, 0),
    (440.0, 5),
    (420.0, 4),
    (280.0, 3),
    (260.0, 2),
    (20.0, 1),
];
/// Where it wants to sit relative to you in each of the approach phases.
pub const SAUCER_WIDE: f32 = 600.0;
pub const SAUCER_CLOSE: f32 = 300.0;
pub const SAUCER_HIGH: f32 = 250.0;
pub const SAUCER_LOW: f32 = 170.0;
/// How long the death spin runs, and how long the last phase lasts.
pub const SAUCER_SPIN: f32 = 150.0;
pub const SAUCER_LAST_STAND: f32 = 3600.0;
/// The last phase alternates on this rhythm: hover, then strafe.
pub const SAUCER_BEAT: f32 = 120.0;
pub const SAUCER_HALF_BEAT: f32 = 60.0;

/// The Dark Mage: a caster that raises the goblins you have already killed.
///
/// Its three spells cycle in order and each has its own wind-up. The bolt is the only one aimed at
/// you; the other two are why leaving its escort dead near it is a mistake.
///
/// The three are `projectile::ids::DARK_MAGE_BOLT`, `_HEAL` (the sigil it plants on the ground)
/// and `_PORTAL` (what its skeletons come out of).
pub const DARK_MAGE_BOLT_DAMAGE: i32 = 40;
pub const DARK_MAGE_BOLT_SPEED: f32 = 14.0;
/// The skeletons it raises, per tier.
pub const DD2_SKELETON_BY_TIER: [u16; 3] = [DD2_SKELETON_T1, DD2_SKELETON_T1, DD2_SKELETON_T3];
/// How long each of the three spells takes, and the cooldown after each.
pub const DARK_MAGE_CASTS: [f32; 3] = [97.0, 127.0, 183.0];
pub const DARK_MAGE_COOLDOWN: f32 = 120.0;
pub const DARK_MAGE_SHORT_COOLDOWN: f32 = 20.0;
/// Where in each cast the spell actually goes off.
pub const DARK_MAGE_BOLT_AT: f32 = 32.0;
pub const DARK_MAGE_HEAL_AT: [f32; 3] = [40.0, 48.0, 56.0];
pub const DARK_MAGE_RAISE_AT: f32 = 64.0;
/// How far it will look for hurt allies worth healing, and how far it throws its bolt.
pub const DARK_MAGE_HEAL_RANGE: (f32, f32) = (600.0, 200.0);
pub const DARK_MAGE_BOLT_RANGE: f32 = 1000.0;
/// How far away a dead goblin can be and still be worth raising.
pub const RAISE_CHECK_RANGE: f32 = 800.0;
pub const RAISE_RANGE: f32 = 850.0;
/// How many corpses it takes before raising is worth casting, and how many come back at once.
pub const RAISE_MINIMUM: usize = 3;
pub const RAISE_MOST: usize = 8;
/// Where it puts the healing sigil, relative to itself.
pub const DARK_MAGE_HEAL_OUT: f32 = 240.0;

/// Betsy, tier three's champion.
///
/// Her fight is a script rather than a reaction: eight slots, cycled, each naming one attack. Only
/// one slot is uncertain — the spin has a one-in-three chance of being skipped for the scream —
/// so learning Betsy is learning the order.
pub const BETSY_SCRIPT: [u8; 8] = [2, 2, 3, 2, 4, 5, 3, 6];
/// The slot that sometimes gets skipped, and what it turns into.
pub const BETSY_SKIPPABLE: usize = 5;
/// What either of her two projectiles (`projectile::ids::BETSY_*`) hits for.
pub const BETSY_ATTACK_DAMAGE: i32 = 35;
/// The wyverns she screams up, and how many she will have out at once.
pub const BETSY_WYVERN: u16 = DD2_WYVERN_T3;
pub const BETSY_WYVERN_CAP: usize = 4;
/// Hovering between attacks: where she sits, how fast she gets there, how long she waits.
pub const BETSY_HOVER_OUT: f32 = 300.0;
pub const BETSY_HOVER_UP: f32 = 200.0;
pub const BETSY_HOVER_SPEED: f32 = 7.5;
pub const BETSY_HOVER_ACCEL: f32 = 0.45;
pub const BETSY_HOVER_TICKS: f32 = 30.0;
pub const BETSY_ARRIVE: f32 = 10.0;
/// The plain dash.
pub const BETSY_DASH_SPEED: f32 = 23.0;
pub const BETSY_DASH_TICKS: f32 = 30.0;
/// The flame breath: line up this far out, fire, and hold the run this long.
pub const BETSY_BREATH_OUT: f32 = 600.0;
pub const BETSY_BREATH_UP: f32 = 250.0;
pub const BETSY_BREATH_APPROACH: f32 = 12.0;
pub const BETSY_BREATH_LINE_UP: f32 = 40.0;
pub const BETSY_BREATH_RUN: f32 = 80.0;
pub const BETSY_BREATH_DASH: f32 = 10.0;
/// The fireball run: much further out, much faster, six fireballs ten ticks apart.
pub const BETSY_RUN_OUT: f32 = 1500.0;
pub const BETSY_RUN_UP: f32 = 350.0;
pub const BETSY_RUN_APPROACH: f32 = 13.0;
pub const BETSY_RUN_LINE_UP: f32 = 60.0;
pub const BETSY_RUN_SPEED: f32 = 12.0;
pub const BETSY_FIREBALL_EVERY: i32 = 10;
pub const BETSY_FIREBALLS: i32 = 6;
pub const BETSY_RUN_CLIMB: f32 = 60.0;
/// The spin: one full turn in a second.
pub const BETSY_SPIN_TICKS: f32 = 60.0;
/// The scream: how long she will chase before doing it anyway, and when the wyverns come.
pub const BETSY_SCREAM_CHASE: f32 = 180.0;
pub const BETSY_SCREAM_CLOSE: f32 = 350.0;
pub const BETSY_SCREAM_TICKS: f32 = 90.0;
pub const BETSY_SCREAM_AT: [f32; 3] = [20.0, 45.0, 70.0];
pub const BETSY_LEAP_AT: f32 = 20.0;

/// What style 107 — the Old One's Army's ground troops — needs to know about a type.
///
/// One routine drives twenty creatures, from a goblin that runs at you to an ogre that has three
/// separate attacks. Everything that differs between them is here; the routine itself holds only
/// the machinery they share.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Walker {
    /// Whether it chases at all. A goblin shark by day does not.
    pub chases: bool,
    /// Whether it wanders off when there is nobody to chase.
    pub despawns: bool,
    /// Whether it fades in out of a lane portal, and can walk through walls to reach the arena.
    pub from_portal: bool,
    /// Whether it can shove itself past terrain it has been stuck against.
    pub teleports_when_stuck: bool,
    /// Whether it swims when it is in water.
    pub swims: bool,

    /// Whether it has a close attack on the `ai[0]` timer.
    pub melee: bool,
    /// How close it must be to start one, how long it takes, and how long it waits afterwards.
    pub melee_range: f32,
    pub melee_ticks: i32,
    pub melee_cooldown: i32,
    /// How hard it brakes while swinging.
    pub melee_brake: f32,
    /// Whether the close attack also throws something.
    pub melee_throws: bool,
    /// Whether the close attack leaps: when in the swing, how far down it must be, how hard.
    pub leaps: bool,
    pub leap_at: i32,
    pub leap_floor: i32,
    pub leap_speed: f32,

    /// Whether it has a ranged attack on the `ai[1]` timer.
    pub ranged: bool,
    /// How long the throw takes and at which tick of it the thing actually leaves.
    pub ranged_ticks: i32,
    pub ranged_at: i32,
    /// How close it must be to start one.
    pub ranged_range: f32,
    /// How long it stands still after being hit before it will throw again.
    pub hurt_delay: f32,
    /// How long it waits between throws, beyond the throw itself.
    pub shot_cooldown: f32,
    /// Whether it turns to face what it just threw.
    pub turn_to_shot: bool,
    /// Whether it keeps re-aiming during the throw, and from when in the throw.
    pub retarget_from: i32,

    /// What it throws, how much it hurts, how fast it goes, and how many at once. Every troop
    /// that throws something deals noticeably less in Expert Mode — the two are not the same
    /// number scaled by a fixed ratio, so both have to be stored rather than derived from one.
    pub shot: u16,
    pub shot_damage: i32,
    pub shot_damage_expert: i32,
    pub shot_speed: f32,
    pub shot_count: i32,
    /// How much the throw arcs upward with distance, and how much it scatters.
    pub shot_arc: f32,
    pub shot_spread: f32,
    /// How far along its own throw the projectile starts.
    pub shot_lead: f32,
    /// Where on the creature the throw comes from.
    pub muzzle: (f32, f32),

    /// Walking: top speed, how fast it gets there, and how hard it slows when it is over.
    pub max_speed: f32,
    pub accel: f32,
    pub brake: f32,
    /// How far in front of itself it looks for a step to climb, beyond half its own width.
    pub step_reach: f32,
    /// How long it must be stuck before it does something about it.
    pub stuck_ticks: i32,
    /// Whether it explodes rather than dying.
    pub explodes: bool,
    /// Whether it stands in an aura that hurts you and heals it.
    pub aura: bool,
    /// How long it takes to climb out of the ground before it can be touched.
    pub rises_for: f32,
}

impl Walker {
    /// The plain walker every type starts from.
    pub const PLAIN: Self = Self {
        chases: false,
        despawns: false,
        from_portal: true,
        teleports_when_stuck: true,
        swims: false,
        melee: false,
        melee_range: 40.0,
        melee_ticks: 30,
        melee_cooldown: 0,
        melee_brake: 0.9,
        melee_throws: false,
        leaps: false,
        leap_at: 32,
        leap_floor: 15,
        leap_speed: 9.0,
        ranged: false,
        ranged_ticks: 70,
        ranged_at: 35,
        ranged_range: 700.0,
        hurt_delay: 30.0,
        shot_cooldown: 0.0,
        turn_to_shot: false,
        retarget_from: i32::MAX,
        shot: 81,
        shot_damage: 1,
        shot_damage_expert: 1,
        shot_speed: 11.0,
        shot_count: 1,
        shot_arc: 0.1,
        shot_spread: 0.5,
        shot_lead: 1.0,
        muzzle: (0.0, 0.0),
        max_speed: 1.0,
        accel: 0.07,
        brake: 0.8,
        step_reach: 6.0,
        stuck_ticks: 30,
        explodes: false,
        aura: false,
        rises_for: 0.0,
    };
}

/// How large a kobold's blast is, and how long the fuse burns before it goes.
pub const KOBOLD_BLAST: f32 = 192.0;
pub const KOBOLD_BLAST_DAMAGE: i32 = 80;
/// The wither beast's aura: how far it reaches, how often it feeds, and how much it gives back.
pub const WITHER_AURA: f32 = 400.0;
pub const WITHER_FEEDS_EVERY: f32 = 60.0;
pub const WITHER_HEALS: i32 = 20;

/// The numbers for one of style 107's types.
pub fn walker(npc_type: u16) -> Walker {
    let base = Walker::PLAIN;
    match npc_type {
        DD2_GOBLIN_T1 | DD2_GOBLIN_T2 | DD2_GOBLIN_T3 => Walker {
            chases: true,
            melee: true,
            accel: base.accel
                + match npc_type {
                    DD2_GOBLIN_T2 => 0.01,
                    DD2_GOBLIN_T3 => 0.02,
                    _ => 0.0,
                },
            max_speed: base.max_speed
                + match npc_type {
                    DD2_GOBLIN_T2 => 0.2,
                    DD2_GOBLIN_T3 => 0.4,
                    _ => 0.0,
                },
            ..base
        },
        DD2_GOBLIN_BOMBER_T1 | DD2_GOBLIN_BOMBER_T2 | DD2_GOBLIN_BOMBER_T3 => Walker {
            chases: true,
            ranged: true,
            retarget_from: 18,
            ranged_ticks: 42,
            ranged_at: 18,
            ranged_range: 280.0,
            shot: GOBLIN_BOMB,
            shot_speed: 6.0,
            shot_arc: 0.4,
            muzzle: (0.0, -14.0),
            shot_damage: match npc_type {
                DD2_GOBLIN_BOMBER_T3 => 40,
                DD2_GOBLIN_BOMBER_T2 => 30,
                _ => 20,
            },
            shot_damage_expert: match npc_type {
                DD2_GOBLIN_BOMBER_T3 => 35,
                DD2_GOBLIN_BOMBER_T2 => 25,
                _ => 15,
            },
            shot_spread: if npc_type == DD2_GOBLIN_BOMBER_T3 {
                0.4
            } else {
                0.6
            },
            max_speed: if npc_type == DD2_GOBLIN_BOMBER_T3 {
                1.12
            } else {
                0.88
            },
            ..base
        },
        DD2_JAVELINST_T1 | DD2_JAVELINST_T2 | DD2_JAVELINST_T3 => Walker {
            chases: true,
            ranged: true,
            retarget_from: 82,
            ranged_ticks: 90,
            ranged_at: 82,
            shot: if npc_type == DD2_JAVELINST_T3 {
                JAVELIN_T3
            } else {
                JAVELIN
            },
            shot_arc: 0.0,
            muzzle: (0.0, -14.0),
            ranged_range: match npc_type {
                DD2_JAVELINST_T1 => 500.0,
                DD2_JAVELINST_T2 => 550.0,
                _ => 600.0,
            },
            shot_speed: match npc_type {
                DD2_JAVELINST_T1 => 11.5,
                DD2_JAVELINST_T2 => 12.2,
                _ => 13.0,
            },
            shot_damage: match npc_type {
                DD2_JAVELINST_T1 => 15,
                DD2_JAVELINST_T2 => 30,
                _ => 45,
            },
            shot_damage_expert: match npc_type {
                DD2_JAVELINST_T1 => 10,
                DD2_JAVELINST_T2 => 20,
                _ => 30,
            },
            shot_spread: match npc_type {
                DD2_JAVELINST_T1 => 0.6,
                DD2_JAVELINST_T2 => 0.5,
                _ => 0.4,
            },
            max_speed: match npc_type {
                DD2_JAVELINST_T1 => 0.88,
                DD2_JAVELINST_T2 => 0.94,
                _ => 1.0,
            },
            ..base
        },
        DD2_DRAKIN_T2 | DD2_DRAKIN_T3 => Walker {
            chases: true,
            ranged: true,
            retarget_from: 40,
            ranged_ticks: 60,
            ranged_at: 40,
            ranged_range: 600.0,
            shot: DRAKIN_FIREBALL,
            shot_speed: 13.0,
            shot_arc: 0.15,
            shot_lead: 0.0,
            muzzle: (22.0, 0.0),
            shot_spread: if npc_type == DD2_DRAKIN_T2 { 2.5 } else { 1.5 },
            shot_damage: if npc_type == DD2_DRAKIN_T3 { 60 } else { 35 },
            shot_damage_expert: if npc_type == DD2_DRAKIN_T3 { 45 } else { 25 },
            max_speed: 0.77,
            ..base
        },
        DD2_KOBOLD_WALKER_T2 | DD2_KOBOLD_WALKER_T3 => Walker {
            chases: true,
            melee: true,
            melee_ticks: 40,
            melee_range: 700.0,
            max_speed: 0.88,
            explodes: true,
            ..base
        },
        DD2_WITHER_BEAST_T2 | DD2_WITHER_BEAST_T3 => Walker {
            chases: true,
            melee: true,
            melee_ticks: 110,
            melee_range: 600.0,
            accel: 0.16,
            brake: 0.7,
            max_speed: 1.4,
            aura: true,
            ..base
        },
        DD2_SKELETON_T1 | DD2_SKELETON_T3 => Walker {
            chases: true,
            rises_for: 120.0,
            ..base
        },
        DD2_OGRE_T2 | DD2_OGRE_T3 => Walker {
            chases: true,
            melee: true,
            melee_range: 130.0,
            melee_ticks: 44,
            melee_cooldown: 60,
            melee_brake: 0.7,
            step_reach: base.step_reach - 32.0,
            ..base
        },
        GOBLIN_SHARK => Walker {
            chases: true,
            despawns: true,
            from_portal: false,
            swims: true,
            ranged: true,
            turn_to_shot: true,
            retarget_from: 40,
            ranged_ticks: 60,
            ranged_at: 40,
            ranged_range: 600.0,
            hurt_delay: 20.0,
            shot_cooldown: 150.0,
            shot: GOBLIN_SHARK_SHOT,
            shot_speed: 13.0,
            shot_damage: 40,
            shot_damage_expert: 30,
            shot_arc: 0.15,
            shot_spread: 2.5,
            shot_lead: 0.0,
            muzzle: (-4.0, -20.0),
            max_speed: 8.0,
            accel: base.accel * 3.0,
            brake: 0.9,
            ..base
        },
        _ => base,
    }
}

/// The ogre's three attacks, chosen by range rather than cycled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OgreAttack {
    /// Close: a swing of the club.
    Swipe = 0,
    /// Far: a lobbed spit.
    Spit = 1,
    /// Middle: a leap and a ground pound.
    Pound = 2,
}

/// How the ogre's numbers change for each of its three attacks.
pub fn ogre_attack(walker: Walker, attack: OgreAttack) -> Walker {
    match attack {
        OgreAttack::Swipe => Walker {
            melee_ticks: 44,
            ..walker
        },
        OgreAttack::Spit => Walker {
            melee_throws: true,
            melee_ticks: 90,
            melee_range: 1000.0,
            melee_cooldown: 240,
            shot: OGRE_SPIT,
            shot_damage: 40,
            shot_damage_expert: 30,
            muzzle: (30.0, -70.0),
            ..walker
        },
        OgreAttack::Pound => Walker {
            melee_throws: true,
            leaps: true,
            melee_ticks: 90,
            melee_range: 250.0,
            shot: OGRE_POUND,
            shot_damage: 60,
            shot_damage_expert: 40,
            ranged_at: 36,
            leap_at: 56,
            leap_floor: 41,
            leap_speed: 13.0,
            muzzle: (-20.0, 0.0),
            ..walker
        },
    }
}

/// How long the ogre waits before it will pound again.
pub const OGRE_POUND_COOLDOWN: f32 = 300.0;

/// The Empress of Light.
///
/// Her fight is a fixed rotation of set pieces rather than a reaction to what you do, and the
/// rotation changes wholesale at half health: a second script, faster, with two attacks the first
/// one never uses. Fighting her by day enrages her and every one of her attacks kills outright.
pub const EMPRESS_GIVE_UP: f32 = 6400.0;
pub const EMPRESS_FLY_SPEED: f32 = 12.0;
pub const EMPRESS_FLY_ACCEL: f32 = 0.5;
/// How close to its station she considers herself arrived.
pub const EMPRESS_SETTLED: f32 = 40.0;
/// How long she takes to arrive, and how long the idle between attacks runs in each phase.
pub const EMPRESS_ARRIVAL: f32 = 180.0;
pub const EMPRESS_IDLE: f32 = 45.0;
pub const EMPRESS_IDLE_PHASE_2: f32 = 20.0;
/// The first script, and the second. Numbers are attack ids, not slots.
pub const EMPRESS_SCRIPT: [u8; 10] = [2, 8, 6, 8, 5, 2, 8, 4, 8, 5];
pub const EMPRESS_SCRIPT_PHASE_2: [u8; 9] = [7, 2, 8, 5, 2, 6, 4, 8, 12];
pub const EMPRESS_SCRIPT_PHASE_2_EXPERT: [u8; 10] = [7, 2, 8, 11, 5, 2, 6, 4, 8, 12];
/// The sun dance's own damage: flat regardless of phase, difficulty, or the enrage override.
///
/// `AI_120_HallowBoss`, `NPC.cs:46462` declares it (`num5 = 40`) alongside the other five damage
/// locals, but unlike them it is never touched again: not by the phase-2 block that raises the
/// rest (`NPC.cs:46482-46494`), not by `GetAttackDamage_ForProjectiles` (`NPC.cs:46495-46499`,
/// which the other five all pass through to interpolate classic to expert), and not by the
/// enrage override that sets the other five to 9999 (`NPC.cs:46500-46508`). Case 3's own shot
/// (`NPC.cs:46833`) passes `num5` straight through. The very first sun dance, planted while she
/// arrives (`NPC.cs:46528`), is a separate, always-literal-zero shot, ported as `planted(..., 0,
/// ...)` at its call site rather than through this constant.
pub const EMPRESS_SUN_DANCE_DAMAGE: i32 = 40;
/// Damage per attack, classic then expert, phase 1: blast, rainbow, bolt, ethereal-lance ring,
/// lance wall. Ported from `AI_120_HallowBoss`'s five `num6`..`num10` locals (`NPC.cs:46463-
/// 46467`) and the `GetAttackDamage_ForProjectiles(classic, expert)` calls that finalise them
/// (`NPC.cs:46495-46499`), matched to their attacks by the projectile id and damage local each
/// case block's own `Projectile.NewProjectile` call passes: blast is `num8`/873 (case 2,
/// `NPC.cs:46765-46820`, reused by circling blasts, case 12, `NPC.cs:47304-47353`); rainbow is
/// `num9`/872 (case 5, `NPC.cs:46953-46994`); bolt is `num6`/919 (case 4, `NPC.cs:46843-46952`,
/// reused by chasing bolts, case 11, `NPC.cs:47213-47303`); the ethereal-lance ring is
/// `num10`/923 (case 6, `NPC.cs:46995-47034`); the lance wall is `num7`/919 (case 7, `NPC.cs:47035-
/// 47135`), a genuinely different local from the ring's `num10` despite the similar name, which a
/// prior pass had collapsed both into a single shared slot.
pub const EMPRESS_DAMAGE: [(i32, i32); 5] = [(45, 30), (45, 30), (50, 30), (50, 35), (70, 65)];
/// As [`EMPRESS_DAMAGE`], phase 2: the same five locals after the `if (flag)` block raises them
/// (`NPC.cs:46482-46494`), then through the same `GetAttackDamage_ForProjectiles` calls.
pub const EMPRESS_DAMAGE_PHASE_2: [(i32, i32); 5] =
    [(50, 35), (50, 35), (60, 35), (60, 40), (65, 30)];
/// Where she stations herself for each attack, relative to you.
pub const EMPRESS_STATION_LEFT: (f32, f32) = (-150.0, -250.0);
pub const EMPRESS_STATION_RIGHT: (f32, f32) = (150.0, -250.0);
pub const EMPRESS_STATION_HIGH: (f32, f32) = (0.0, -350.0);
pub const EMPRESS_STATION_RING: (f32, f32) = (-80.0, -500.0);
/// How far to one side she pulls back before a dash, and how fast she comes through.
pub const EMPRESS_DASH_OUT: f32 = 550.0;
pub const EMPRESS_DASH_SPEED: f32 = 50.0;
/// The prismatic bolts: how many, how fast, and how long the volley runs.
pub const EMPRESS_RAINBOW_COUNT: i32 = 13;
pub const EMPRESS_RAINBOW_SPEED: f32 = 8.0;
/// The lance walls: how many lances to a wall, and how far apart.
pub const EMPRESS_WALL_LANCES: f32 = 13.0;
pub const EMPRESS_WALL_SPACING: f32 = 150.0;

/// Debuffs an enemy can land on you by standing near you rather than by hitting you.
pub const BUFF_WITHERED_ARMOR: u16 = 195;

/// How much tougher a town NPC is for everything the world has beaten.
///
/// A townsperson is not a fixed creature: every boss down makes them hit harder, reload faster and
/// soak more, which is why the guide who was killed by a zombie on night one can hold a doorway by
/// hardmode. The steps are the game's own, and the wall falling is worth more than any single boss.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TownToughness {
    /// A multiplier on what they hit for; starts at one.
    pub damage: f32,
    /// A multiplier on how long they take between attacks; starts at two and only ever falls.
    pub reload: f32,
    /// Armour on top of the type's own.
    pub defense: i32,
}

/// What the world's history has done to its townsfolk.
///
/// The flags are taken as booleans in the game's own order, because each step is applied in turn
/// rather than being looked up: two worlds with the same *number* of bosses down are not equally
/// dangerous places to live.
#[allow(clippy::too_many_arguments)]
pub fn town_toughness(down: &[bool; 15], combat_books: (bool, bool)) -> TownToughness {
    // (damage added, defence added) for each step, in the order the game applies them.
    const STEPS: [(f32, i32); 15] = [
        (0.05, 2), // King Slime
        (0.05, 2), // Eye of Cthulhu
        (0.1, 3),  // Deerclops
        (0.1, 3),  // the evil boss
        (0.1, 3),  // Skeletron
        (0.1, 3),  // Queen Bee
        (0.4, 12), // the wall falling, which is worth more than any boss
        (0.15, 6), // Queen Slime
        (0.15, 6), // the Destroyer
        (0.15, 6), // the Twins
        (0.15, 6), // Skeletron Prime
        (0.15, 8), // Plantera
        (0.15, 8), // the Empress
        (0.15, 8), // Duke Fishron
        (0.15, 8), // Golem
    ];
    let mut out = TownToughness {
        damage: 1.0,
        reload: 2.0,
        defense: 0,
    };
    // The combat books come first and are worth far more than a boss.
    for used in [combat_books.0, combat_books.1] {
        if used {
            out.reload *= 0.8;
            out.damage += 0.25;
            out.defense += 8;
        }
    }
    for (step, beaten) in STEPS.iter().zip(down) {
        if *beaten {
            out.reload *= 0.985;
            out.damage += step.0;
            out.defense += step.1;
        }
    }
    out
}

#[cfg(test)]
mod id_pin_tests {
    use super::*;

    /// Pins the Skeletron Prime limb behavior to the real NPCIDs, not to the (self-consistent)
    /// constant names: 128 is `PrimeCannon` and must lob a bomb (proj 102); 129/130 (Saw/Vice) are
    /// melee. Every other Prime test builds its arm from the constants, so they passed whatever the
    /// constants held — this one catches the shift.
    #[test]
    fn the_prime_cannon_is_npcid_128_and_lobs_a_bomb() {
        assert_eq!(PRIME_CANNON, 128, "NPCID.PrimeCannon");
        assert_eq!(
            prime_limb(128).shot,
            Some(102),
            "NPC 128 (PrimeCannon) must lob its bomb"
        );
        assert_eq!(prime_limb(129).shot, None, "NPC 129 (PrimeSaw) is melee");
        assert_eq!(prime_limb(130).shot, None, "NPC 130 (PrimeVice) is melee");
        assert_eq!(
            prime_limb(131).shot,
            Some(100),
            "NPC 131 (PrimeLaser) fires its laser"
        );
    }

    /// Mothron must lay its actual egg (478), not CrimsonPenguin (470). Verified through the type
    /// table so the constant is tied to the real entity, not just a literal.
    #[test]
    fn mothron_lays_its_real_egg_not_a_penguin() {
        assert_eq!(
            crate::npc_data::npc_stats(MOTHRON_EGG).unwrap().name,
            "MothronEgg"
        );
        assert_eq!(
            crate::npc_data::npc_stats(MOTHRON_SPAWN_TYPE).unwrap().name,
            "MothronSpawn"
        );
    }
}

#[cfg(test)]
mod town_toughness_tests {
    use super::*;

    /// A fresh world's townsfolk are exactly the type's own.
    #[test]
    fn a_fresh_world_leaves_them_alone() {
        let plain = town_toughness(&[false; 15], (false, false));
        assert_eq!(plain.damage, 1.0);
        assert_eq!(plain.reload, 2.0);
        assert_eq!(plain.defense, 0);
    }

    /// Every step makes them tougher, and none makes them weaker.
    #[test]
    fn every_step_only_helps() {
        let mut last = town_toughness(&[false; 15], (false, false));
        for step in 0..15 {
            let mut down = [false; 15];
            for d in down.iter_mut().take(step + 1) {
                *d = true;
            }
            let now = town_toughness(&down, (false, false));
            assert!(now.damage >= last.damage, "damage fell at step {step}");
            assert!(now.defense >= last.defense, "defence fell at step {step}");
            assert!(now.reload <= last.reload, "reload rose at step {step}");
            last = now;
        }
    }

    /// The wall falling is worth more than any single boss.
    #[test]
    fn the_wall_is_worth_more_than_a_boss() {
        let mut boss = [false; 15];
        boss[5] = true; // Queen Bee
        let mut wall = [false; 15];
        wall[6] = true; // hardmode
        assert!(
            town_toughness(&wall, (false, false)).defense
                > town_toughness(&boss, (false, false)).defense
        );
    }

    /// A combat book is worth more than a boss, and two are worth more than one.
    #[test]
    fn the_books_are_worth_more_than_a_boss() {
        let none = town_toughness(&[false; 15], (false, false));
        let one = town_toughness(&[false; 15], (true, false));
        let both = town_toughness(&[false; 15], (true, true));
        assert!(one.defense > none.defense);
        assert!(both.defense > one.defense);
        assert!(both.reload < one.reload, "and they reload faster still");
    }

    /// A finished world's townsfolk are a different creature from a new one's.
    #[test]
    fn a_finished_world_has_hard_townsfolk() {
        let done = town_toughness(&[true; 15], (true, true));
        assert!(done.defense > 80, "only {} armour", done.defense);
        assert!(done.damage > 2.5, "only {}x damage", done.damage);
        assert!(done.reload < 1.2, "still reloading at {}", done.reload);
    }
}
