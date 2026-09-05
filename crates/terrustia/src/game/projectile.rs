//! Projectiles: the things NPCs throw, and what happens to them afterwards.
//!
//! A projectile is a much simpler entity than an NPC — no targeting, no state machine, usually no
//! decisions at all — but it is the half of combat the server was missing. Every routine that
//! decided to shoot has been emitting its aim and cadence for a while; this is what makes those
//! decisions land.
//!
//! Nine behaviours are transcribed. That is **not** everything the roster and the world's traps
//! fire, and this file said it was until the count was actually taken: of the 79 projectile types
//! something here can put in the air, 43 reach no arm of their own and fly straight because that is
//! what the fallthrough does. Most are harmless as straight lines (a caster's bolt is one), but the
//! gravity styles are not, and the worst of them is written down at the bottom of this list.
//!
//! * **Style 1**, the arc: it flies straight for a quarter of a second and then starts falling, a
//!   tenth of a pixel a tick, capped at sixteen. Feathers, stingers, snowballs, skulls and darts.
//! * **Style 10**, the lob: it falls from the first tick and sticks where it lands.
//! * **Style 14**, the rolling ball: it falls, loses speed along the ground, and *bounces* off
//!   what it hits at nine tenths rather than dying on it. A spiky ball trap fills a corridor
//!   because of this one rule.
//! * **Style 18**, the scythe: it spins, and between its thirtieth and hundredth tick it
//!   *accelerates* by six per cent a tick — which is why a demon's scythe is harmless when it
//!   leaves and lethal by the time it reaches you.
//! * **Style 23**, the flame: straight, and never alight for more than a second.
//! * **Style 25**, the boulder: it falls, hops off a hard landing, and then rolls away from
//!   whichever side of it has a wall, working up to seven pixels a tick until it meets one head on
//!   and breaks. It is also harmless for its first seven ticks, which is the only reason standing
//!   next to a Boulder Statue you have just wired is survivable.
//! * **Style 37**, the spear: it grows out of its trap to three hundred pixels or until it hits
//!   something, then pulls back and dies when it is home.
//! * **Style 38**, the flamethrower: it does no damage itself. Every sixth tick it emits a flame,
//!   which is the thing that burns.
//! * **Style 126**, the geyser: it rises out of its vent until the way ahead is clear, then hangs
//!   there for a second. If it never gets clear it dies inside the wall.
//!
//! Style 25 is the one that was doing real harm by its absence. A Boulder Statue launches its
//! boulder *at rest*, so with no arm to give it gravity it never moved at all: a wired statue put a
//! stationary thirty-one-pixel hostile box under itself for a full minute and then took it away
//! again. The trap that is meant to chase you down a corridor was a damage aura around its own
//! pedestal.

use terrustia_proto::projectile::{MAX_PROJECTILES, ProjectileKey, SERVER_OWNER};
use terrustia_proto::projectile_data::{ProjectileStats, projectile_stats};
use terrustia_proto::tile_solid::{solid, solid_top};

use super::npc::{TILE, TileView};

/// Terminal speed for anything that falls.
const TERMINAL: f32 = 16.0;
/// How long a style-1 projectile flies flat before gravity takes it.
const ARC_DELAY: f32 = 15.0;
/// ...and how hard it then falls.
const ARC_GRAVITY: f32 = 0.1;
/// The scythe's acceleration window and rate.
const SCYTHE_FROM: f32 = 30.0;
const SCYTHE_UNTIL: f32 = 100.0;
const SCYTHE_ACCEL: f32 = 1.06;
/// ...and how fast it spins.
const SCYTHE_SPIN: f32 = 0.8;

/// One projectile in flight.
#[derive(Debug, Clone, Copy)]
pub struct Projectile {
    pub key: ProjectileKey,
    pub projectile_type: u16,
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    pub damage: i32,
    pub knockback: f32,
    pub ai: [f32; 3],
    /// Working state a routine keeps to itself.
    ///
    /// The game splits these from `ai` because they are never synced: they are what a projectile
    /// needs to remember, not what a client needs to be told. A spear's anchor lives here, and so
    /// does a boulder's age. Four wide because vanilla's is (`Projectile.localAI`), and slot 2 is
    /// the one the boulder's own damage gate reads.
    pub local_ai: [f32; 4],
    pub rotation: f32,
    pub time_left: i32,
    /// How many more things it can hit. -1 means no limit.
    pub penetrate: i32,
    pub stats: ProjectileStats,
    /// Set whenever clients need telling about it.
    pub dirty: bool,
}

impl Projectile {
    pub fn width(&self) -> f32 {
        self.stats.width as f32
    }

    pub fn height(&self) -> f32 {
        self.stats.height as f32
    }

    pub fn center(&self) -> (f32, f32) {
        (
            self.position.0 + self.width() / 2.0,
            self.position.1 + self.height() / 2.0,
        )
    }

    /// Whether this box overlaps another.
    pub fn overlaps(&self, position: (f32, f32), size: (f32, f32)) -> bool {
        self.position.0 < position.0 + size.0
            && self.position.0 + self.width() > position.0
            && self.position.1 < position.1 + size.1
            && self.position.1 + self.height() > position.1
    }

    /// Whether it can hurt anything *this* tick, as against whether it is hostile at all.
    ///
    /// `Projectile.CanDamage` (`Projectile.cs:12491-12494`) is a chain of per-type exemptions and
    /// only one of them reaches a projectile this server launches: a boulder is harmless for its
    /// first seven ticks. That grace is the whole reason a Boulder Statue is survivable, because
    /// the boulder appears twenty-eight pixels below the statue's own base, which is inside the
    /// hitbox of anybody standing next to it triggering the thing.
    pub fn can_damage(&self) -> bool {
        let excused = self.stats.ai_style == BOULDER_STYLE
            && !matches!(self.projectile_type, 1005 | 1014 | 1021 | 1047)
            && self.local_ai[2] <= BOULDER_GRACE;
        !excused
    }
}

/// `ProjectileID.PurificationPowder`. The only *client*-owned projectile this server has to know
/// the flight of, because it is the only one whose whole effect is a server-side decision.
pub const PURIFICATION_POWDER: u16 = 10;

/// A Purification Powder cloud in flight, tracked apart from [`ProjectileStore`].
///
/// It is deliberately not a [`Projectile`] in the store. The store's contents are the server's own
/// shots: they are stepped, they go dirty, they are broadcast, and they send a kill packet when
/// they expire. A powder is somebody else's projectile, already on every client's screen from the
/// thrower's own packet 27, so putting one in the store would draw a second cloud beside it and
/// then kill a projectile the server does not own. All the server wants is where the cloud is.
///
/// Vanilla decides the powder's one real effect on the server (`Projectile.Damage_TryUsingPowders`
/// runs under `Main.netMode != 1`, `Projectile.cs:14787-14808`), unlike ordinary weapon damage,
/// which the owning client decides and reports. That is why this is tracked at all rather than
/// trusting a client to say "I purified that": the game does not trust it either.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Powder {
    /// Top-left, the same corner packet 27 carries and vanilla's `Damage_GetHitbox` reads
    /// (`Projectile.cs:15040`: `new Rectangle((int)position.X, (int)position.Y, width, height)`).
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    /// `ai[0]`, the tick counter `aiStyle == 6` kills the cloud on.
    pub age: f32,
}

/// How wide and tall a powder cloud is (`projectile_data.rs`'s own entry for type 10, four tiles
/// square). Read once here rather than through `projectile_stats` so the hit test cannot silently
/// become a point when a lookup fails.
pub const POWDER_SIZE: (f32, f32) = (64.0, 64.0);
/// `aiStyle == 6`: how much speed the cloud keeps each tick, and the tick it dies on.
const POWDER_DRAG: f32 = 0.95;
const POWDER_LIFE: f32 = 180.0;

impl Powder {
    /// One tick of `aiStyle == 6` (`Projectile.cs:24366-24372`):
    ///
    /// ```csharp
    /// velocity *= 0.95f;
    /// this.ai[0]++;
    /// if (this.ai[0] == 180f) { Kill(); }
    /// ```
    ///
    /// Returns `false` when the cloud is spent. Nothing here reads tiles: type 10 carries
    /// `tileCollide = false`, so a cloud drifts through a wall rather than dying on one.
    pub fn step(&mut self) -> bool {
        self.velocity.0 *= POWDER_DRAG;
        self.velocity.1 *= POWDER_DRAG;
        self.age += 1.0;
        self.position.0 += self.velocity.0;
        self.position.1 += self.velocity.1;
        self.age < POWDER_LIFE
    }

    /// Whether the cloud covers a box, the same overlap [`Projectile::overlaps`] uses.
    pub fn overlaps(&self, position: (f32, f32), size: (f32, f32)) -> bool {
        self.position.0 < position.0 + size.0
            && self.position.0 + POWDER_SIZE.0 > position.0
            && self.position.1 < position.1 + size.1
            && self.position.1 + POWDER_SIZE.1 > position.1
    }
}

/// Whether a tile stops a projectile.
fn blocking(tiles: &impl TileView, x: i32, y: i32) -> bool {
    let tile = tiles.tile(x, y);
    tile.is_active() && solid(tile.block) && !solid_top(tile.block)
}

/// Whether a box overlaps anything solid.
fn hits_terrain(tiles: &impl TileView, position: (f32, f32), size: (f32, f32)) -> bool {
    let left = (position.0 / TILE).floor() as i32;
    let right = ((position.0 + size.0 - 1.0) / TILE).floor() as i32;
    let top = (position.1 / TILE).floor() as i32;
    let bottom = ((position.1 + size.1 - 1.0) / TILE).floor() as i32;
    (left..=right).any(|x| (top..=bottom).any(|y| blocking(tiles, x, y)))
}

/// What a projectile's tick concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Flying,
    /// It hit something, ran out of time, or left the world.
    Spent,
}

/// A projectile a routine wants in the air, which it cannot put there from inside its own tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emission {
    pub projectile_type: u16,
    /// The centre it should appear at.
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    pub damage: i32,
}

/// How far a spear reaches before it starts pulling back.
const SPEAR_REACH: f32 = 300.0;
/// How often a flamethrower emits, and what it emits.
const FLAME_EVERY: f32 = 6.0;
const FLAME: u16 = 188;
/// How much of its speed a rolling ball keeps when it bounces.
const BOUNCE: f32 = -0.9;
/// A boulder: `Projectile.cs:26596-26746` for the flight, `:19056-19099` for what it does to the
/// wall it meets, and `:12491-12494` for the seven ticks before it can hurt anybody.
const BOULDER_STYLE: i32 = 25;
const BOULDER_GRACE: f32 = 7.0;
/// How fast it rolls off the mark, and the ceiling it works up to.
const BOULDER_NUDGE: f32 = 0.5;
const BOULDER_TOP_SPEED: f32 = 7.0;
const BOULDER_ACCEL: f32 = 0.05;
/// Rolling only picks up speed while it is not really falling.
const BOULDER_ROLLING_FALL: f32 = 6.0;
const BOULDER_GRAVITY: f32 = 0.3;
/// A landing harder than this hops rather than settling, at a fifth of the speed it arrived with.
const BOULDER_HOP_FROM: f32 = 5.0;
const BOULDER_HOP: f32 = -0.2;
const BOULDER_SPIN: f32 = 0.06;

/// Walk a box from `from` towards `to` in sub-tile increments, stopping at the first solid tile.
///
/// Returns the furthest position reached and whether terrain was struck. This is the swept form of
/// the game's `Collision.TileCollision`: a projectile crosses at most half a tile between checks,
/// so nothing tunnels however fast it flies, while still advancing the *full* velocity vanilla
/// gives it in a single pass. The old code confused the two, dividing velocity by the pass count,
/// which is what flew the fast types at a fraction of their speed.
fn advance(
    from: (f32, f32),
    to: (f32, f32),
    size: (f32, f32),
    tiles: &impl TileView,
) -> ((f32, f32), bool) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let distance = (dx * dx + dy * dy).sqrt();
    let steps = (distance / (TILE * 0.5)).ceil().max(1.0) as i32;
    let mut last = from;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let probe = (from.0 + dx * t, from.1 + dy * t);
        if hits_terrain(tiles, probe, size) {
            return (last, true);
        }
        last = probe;
    }
    (to, false)
}

/// Drive one projectile for a tick.
///
/// Anything it decides to put in the air is pushed onto `emits` rather than spawned, because a
/// projectile cannot reach into the store that owns it.
///
/// The game runs the *entire* update body once per `extraUpdates + 1`: the AI, the movement and the
/// `timeLeft` decrement all repeat, at full velocity each pass. `Projectile.Update`
/// (`Projectile.cs:16781`) wraps `numUpdates = extraUpdates; while (numUpdates >= 0)` around the
/// whole thing, with `AI()` at 16881 and `timeLeft--` at 17308 inside the loop. Sub-stepping
/// velocity by the pass count and decrementing `timeLeft` once per frame flew the 33 hostile types
/// that carry extra updates at 1/(N+1) speed and kept them alive (N+1) times too long.
pub fn step(
    projectile: &mut Projectile,
    tiles: &impl TileView,
    emits: &mut Vec<Emission>,
) -> Outcome {
    let passes = projectile.stats.extra_updates + 1;
    for _ in 0..passes {
        match projectile.stats.ai_style {
            1 => {
                projectile.ai[0] += 1.0;
                if projectile.ai[0] >= ARC_DELAY {
                    projectile.velocity.1 = (projectile.velocity.1 + ARC_GRAVITY).min(TERMINAL);
                }
                projectile.rotation = projectile.velocity.1.atan2(projectile.velocity.0) + 1.57;
            }
            10 => {
                // A lob: it falls from the moment it leaves, and slows as it goes.
                projectile.velocity.1 = (projectile.velocity.1 + 0.41).min(TERMINAL);
                projectile.velocity.0 *= 0.98;
                projectile.rotation += 0.1;
            }
            14 => {
                // A rolling ball. It is given five ticks of free flight, then falls; once it is
                // running along the ground it scrubs off speed until it stops.
                projectile.ai[0] += 1.0;
                if projectile.ai[0] > 5.0 {
                    projectile.ai[0] = 5.0;
                    if projectile.velocity.1 == 0.0 && projectile.velocity.0 != 0.0 {
                        projectile.velocity.0 *= 0.97;
                        if projectile.velocity.0.abs() < 0.01 {
                            projectile.velocity.0 = 0.0;
                        }
                    }
                    projectile.velocity.1 += 0.2;
                }
                projectile.rotation += projectile.velocity.0 * 0.1;
            }
            18 => {
                projectile.rotation += SCYTHE_SPIN;
                projectile.ai[0] += 1.0;
                // The window that makes a demon scythe frightening.
                if (SCYTHE_FROM..SCYTHE_UNTIL).contains(&projectile.ai[0]) {
                    projectile.velocity.0 *= SCYTHE_ACCEL;
                    projectile.velocity.1 *= SCYTHE_ACCEL;
                }
            }
            23 => {
                // A flame is never alight for more than a second, however it was launched.
                projectile.time_left = projectile.time_left.min(60);
            }
            37 => {
                if spear(projectile, tiles) == Outcome::Spent {
                    return Outcome::Spent;
                }
            }
            38 => {
                // A flamethrower does no damage of its own: it is the thing holding the flame.
                projectile.ai[0] += 1.0;
                if projectile.ai[0] >= FLAME_EVERY {
                    projectile.ai[0] = 0.0;
                    emits.push(Emission {
                        projectile_type: FLAME,
                        position: projectile.center(),
                        velocity: projectile.velocity,
                        damage: projectile.damage,
                    });
                }
            }
            BOULDER_STYLE => boulder(projectile, tiles),
            126 => {
                // A geyser: it rises out of its vent until the way ahead is clear, then stops
                // and hangs there. If it never gets clear it dies inside the wall.
                if projectile.ai[0] == 0.0 {
                    let up = projectile.velocity.1 < 0.0;
                    let probe = (
                        projectile.position.0,
                        projectile.position.1 + if up { projectile.height() - 48.0 } else { 0.0 },
                    );
                    if !hits_terrain(tiles, probe, (projectile.width(), 48.0)) {
                        projectile.velocity = (0.0, if up { -0.001 } else { 0.001 });
                        projectile.ai[0] = 1.0;
                        projectile.ai[1] = 0.0;
                        projectile.time_left = 60;
                    } else {
                        projectile.ai[1] += 1.0;
                        if projectile.ai[1] >= 60.0 {
                            return Outcome::Spent;
                        }
                    }
                }
            }
            _ => {
                // Everything else flies straight and simply faces the way it is going.
                projectile.rotation = projectile.velocity.1.atan2(projectile.velocity.0) + 1.57;
            }
        }

        // Movement at full velocity, one pass at a time.
        let size = (projectile.width(), projectile.height());
        let next = (
            projectile.position.0 + projectile.velocity.0,
            projectile.position.1 + projectile.velocity.1,
        );
        if projectile.stats.tile_collide {
            if projectile.stats.ai_style == 14 {
                // A rolling ball bounces off what it hits instead of dying on it, and each axis is
                // settled on its own: a ball that lands on a floor keeps travelling along it, which
                // is why a spiky ball trap fills a corridor rather than a doorway. Rolling balls
                // carry no extra updates and move well under a tile a step, so a plain destination
                // check settles them without any sweep.
                if hits_terrain(tiles, next, size) {
                    let sideways = (next.0, projectile.position.1);
                    let downward = (projectile.position.0, next.1);
                    let across = !hits_terrain(tiles, sideways, size);
                    let down = !hits_terrain(tiles, downward, size);
                    let (moved, bounce_x, bounce_y) = match (across, down) {
                        (true, false) => (sideways, false, true),
                        (false, true) => (downward, true, false),
                        // Neither axis is free, or both are free alone but not together, a corner.
                        // It comes off both and stays where it is for this step.
                        _ => (projectile.position, true, true),
                    };
                    projectile.position = moved;
                    if bounce_x {
                        projectile.velocity.0 *= BOUNCE;
                    }
                    if bounce_y {
                        projectile.velocity.1 *= BOUNCE;
                    }
                } else {
                    projectile.position = next;
                }
            } else if projectile.stats.ai_style == BOULDER_STYLE {
                // A boulder slides rather than dying: `Collision.TileCollision` zeroes whichever
                // axis is blocked, and only then does the reaction at `Projectile.cs:19056-19099`
                // read which one moved. A hard landing hops at a fifth of the speed it arrived
                // with, and a boulder that was going sideways and is not any more has hit a wall,
                // which kills it. Falling straight onto a floor leaves `velocity.X` unchanged at
                // zero, so it is not a wall and the boulder lives to roll.
                let last = projectile.velocity;
                let (moved, blocked_x, blocked_y) = if !hits_terrain(tiles, next, size) {
                    (next, false, false)
                } else {
                    let across = !hits_terrain(tiles, (next.0, projectile.position.1), size);
                    let down = !hits_terrain(tiles, (projectile.position.0, next.1), size);
                    match (across, down) {
                        (true, false) => ((next.0, projectile.position.1), false, true),
                        (false, true) => ((projectile.position.0, next.1), true, false),
                        // Neither axis is free, or both are free alone but not together: a corner.
                        // Disclosed narrowing, shared with the rolling ball above: vanilla's
                        // `Collision.TileCollision` sweeps the box and clips whichever axis really
                        // intersects, so a diagonal clip of an inside corner takes one axis there
                        // and both here. For a boulder that means it can break on a corner vanilla
                        // would have let it round. Closing it wants the real swept collision rather
                        // than a per-axis probe, which is a change to every style, not this one.
                        _ => (projectile.position, true, true),
                    }
                };
                projectile.position = moved;
                if blocked_y {
                    projectile.velocity.1 = if last.1 > BOULDER_HOP_FROM {
                        last.1 * BOULDER_HOP
                    } else {
                        0.0
                    };
                }
                if blocked_x {
                    projectile.velocity.0 = 0.0;
                    // `if (velocity.X != lastVelocity.X)` and nothing more: a boulder that was not
                    // travelling sideways cannot have had its sideways travel stopped, so vanilla
                    // never reaches the kill for one. That matters at the moment of release, when a
                    // statue drops its boulder overlapping the floor it is standing on and both
                    // axes read as blocked.
                    if last.0 != 0.0 {
                        return Outcome::Spent;
                    }
                }
            } else {
                // Everything else dies on what it hits. Swept so a fast pass cannot skip a wall.
                let (moved, hit) = advance(projectile.position, next, size, tiles);
                projectile.position = moved;
                if hit {
                    return Outcome::Spent;
                }
            }
        } else {
            projectile.position = next;
        }

        // The time budget is spent once per pass, exactly where the game spends it
        // (`Projectile.cs:17308`), so a projectile with extra updates lives the right length of
        // time rather than (N+1) times too long.
        projectile.time_left -= 1;
        if projectile.time_left <= 0 {
            return Outcome::Spent;
        }
    }

    projectile.dirty = true;
    Outcome::Flying
}

/// A boulder: it falls, and once it has landed it rolls away from whichever side has a wall.
///
/// `Projectile.cs:26596-26746`. The arm serves seven boulders in the game and this transcribes the
/// shared body: the per-type branches belong to the Shimmer boulder (1055), the Moon boulder (1021)
/// and four cosmetic ones, none of which anything here launches.
///
/// The ledge probe is the whole character of the thing. A boulder that has come to rest with no
/// sideways speed looks eight pixels left and eight right, at its own top row and the one below;
/// whichever side has a wall is the side it rolls *away* from, so a boulder dropped into a corridor
/// runs down it rather than sitting there. Failing that it looks again a tile further out, then two,
/// and if nothing is walled at any reach the boulder's own column decides, which stops two boulders
/// released side by side from always going the same way.
fn boulder(p: &mut Projectile, tiles: &impl TileView) {
    // `localAI[2]++` (`Projectile.cs:26361`), which is only bookkeeping in vanilla until
    // `CanDamage` reads it. See [`Projectile::can_damage`].
    p.local_ai[2] += 1.0;

    if p.ai[0] != 0.0 && p.velocity.1 <= 0.0 && p.velocity.0 == 0.0 {
        let row = (p.position.1 / TILE) as i32;
        let walled = |x: i32| blocking(tiles, x, row) || blocking(tiles, x, row + 1);
        let mut rolled = false;
        for reach in [0.0, TILE, TILE * 2.0] {
            let left = ((p.position.0 - 8.0 - reach) / TILE) as i32;
            let right = ((p.position.0 + p.width() + 8.0 + reach) / TILE) as i32;
            if walled(left) {
                p.velocity.0 = BOULDER_NUDGE;
            } else if walled(right) {
                p.velocity.0 = -BOULDER_NUDGE;
            } else {
                continue;
            }
            rolled = true;
            break;
        }
        if !rolled {
            // `if ((int)(base.Center.X / 16f) % 2 == 0)` at the widest reach only.
            p.velocity.0 = if (p.center().0 / TILE) as i32 % 2 == 0 {
                BOULDER_NUDGE
            } else {
                -BOULDER_NUDGE
            };
        }
    }

    p.rotation += p.velocity.0 * BOULDER_SPIN;
    // Set unconditionally at the end of every tick, so the probe above never runs on the first one:
    // a boulder is falling out of a statue then, not resting on anything.
    p.ai[0] = 1.0;
    p.velocity.1 = p.velocity.1.min(TERMINAL);
    if p.velocity.1 <= BOULDER_ROLLING_FALL {
        if p.velocity.0 > 0.0 && p.velocity.0 < BOULDER_TOP_SPEED {
            p.velocity.0 += BOULDER_ACCEL;
        }
        if p.velocity.0 < 0.0 && p.velocity.0 > -BOULDER_TOP_SPEED {
            p.velocity.0 -= BOULDER_ACCEL;
        }
    }
    p.velocity.1 += BOULDER_GRAVITY;
}

/// A spear growing out of its trap: it reaches, then pulls back, then dies at home.
///
/// The anchor is set a step and a half behind where it started, because the spear is drawn from
/// there and the retraction has to know where "home" is.
fn spear(projectile: &mut Projectile, tiles: &impl TileView) -> Outcome {
    if projectile.ai[1] == 0.0 {
        projectile.ai[1] = 1.0;
        let centre = projectile.center();
        projectile.local_ai[0] = centre.0 - projectile.velocity.0 * 1.5;
        projectile.local_ai[1] = centre.1 - projectile.velocity.1 * 1.5;
    }
    let anchor = (projectile.local_ai[0], projectile.local_ai[1]);
    let centre = projectile.center();
    let (dx, dy) = (centre.0 - anchor.0, centre.1 - anchor.1);
    let reach = (dx * dx + dy * dy).sqrt();
    projectile.rotation = dy.atan2(dx) - std::f32::consts::FRAC_PI_2;

    let size = (projectile.width(), projectile.height());
    let solid = hits_terrain(tiles, projectile.position, size);
    if projectile.ai[0] == 0.0 {
        if solid || reach > SPEAR_REACH {
            projectile.velocity.0 = -projectile.velocity.0;
            projectile.velocity.1 = -projectile.velocity.1;
            projectile.ai[0] += 1.0;
        }
        return Outcome::Flying;
    }
    let speed = (projectile.velocity.0 * projectile.velocity.0
        + projectile.velocity.1 * projectile.velocity.1)
        .sqrt();
    if solid || reach < speed {
        return Outcome::Spent;
    }
    Outcome::Flying
}

/// The fixed table of projectile slots.
#[derive(Debug)]
pub struct ProjectileStore {
    slots: Vec<Option<Projectile>>,
    /// The generation the projectile most recently in each slot carried.
    ///
    /// Per slot, as the game keeps it: `Projectile.NewProjectile` builds its key from
    /// `++slotGenerations[num]`, indexed by the slot being taken. One counter shared by every slot
    /// — which this used to be — repeats a value after 16384 launches *anywhere*, where per slot it
    /// takes 16384 reuses *of that slot*. On a busy server with a thousand slots in play that is
    /// three orders of magnitude sooner, and the whole point of the generation is that a kill
    /// packet arriving a moment late fails to match rather than destroying a bystander.
    ///
    /// No guard against zero here, unlike the NPC store: the game does not have one either, and
    /// nothing asserts on a projectile generation of nought.
    slot_generations: Vec<u16>,
}

impl Default for ProjectileStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectileStore {
    pub fn new() -> Self {
        Self {
            slots: (0..MAX_PROJECTILES).map(|_| None).collect(),
            slot_generations: vec![0; MAX_PROJECTILES],
        }
    }

    /// Launch one, returning its slot.
    pub fn launch(
        &mut self,
        projectile_type: u16,
        position: (f32, f32),
        velocity: (f32, f32),
        damage: i32,
        time_left: i32,
    ) -> Option<u16> {
        // Loud, because the symptom of a silent miss here is a boss that fights with no attacks
        // and reports nothing wrong. Thirty-two types were absent from the stats table and every
        // shot of theirs was dropped on this line without a word.
        let Some(stats) = projectile_stats(projectile_type) else {
            tracing::warn!(
                projectile_type,
                "no stats for this projectile; the shot was not fired"
            );
            return None;
        };
        let index = self.slots.iter().position(Option::is_none)?;
        // Fourteen bits on the wire, so the counter is kept inside them rather than truncated at
        // the point of packing.
        let generation = match self.slot_generations.get_mut(index) {
            Some(slot) => {
                *slot = slot.wrapping_add(1) & 0x3FFF;
                *slot
            }
            None => 1,
        };
        let projectile = Projectile {
            key: ProjectileKey {
                owner: SERVER_OWNER,
                index: index as u16,
                generation,
            },
            projectile_type,
            // The aim point is the centre; the entity is placed by its corner.
            position: (
                position.0 - stats.width as f32 / 2.0,
                position.1 - stats.height as f32 / 2.0,
            ),
            velocity,
            // The base damage, unscaled. The game keeps a hostile shot's wire damage at its base
            // and applies `hostileDamageScaling(difficulty) * 2` on the client at the moment it hits
            // a player (`Projectile.cs:14916-14919`), so the server must not bake the difficulty
            // multiplier in here: a client that received a pre-scaled number would scale it a second
            // time. The server applies the same scaling itself when it originates the hit (see
            // `tick_contact_damage`).
            damage,
            knockback: stats.knockback,
            ai: [0.0; 3],
            local_ai: [0.0; 4],
            rotation: 0.0,
            time_left: if time_left > 0 {
                time_left
            } else {
                stats.time_left
            },
            penetrate: stats.penetrate,
            stats,
            dirty: true,
        };
        self.slots[index] = Some(projectile);
        Some(index as u16)
    }

    pub fn get(&self, index: u16) -> Option<&Projectile> {
        self.slots.get(index as usize)?.as_ref()
    }

    pub fn get_mut(&mut self, index: u16) -> Option<&mut Projectile> {
        self.slots.get_mut(index as usize)?.as_mut()
    }

    pub fn remove(&mut self, index: u16) -> Option<Projectile> {
        self.slots.get_mut(index as usize)?.take()
    }

    pub fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = (u16, &Projectile)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.as_ref().map(|p| (i as u16, p)))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u16, &mut Projectile)> {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(i, p)| p.as_mut().map(|p| (i as u16, p)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    #[derive(Default)]
    struct Air(HashMap<(i32, i32), Tile>);

    impl TileView for Air {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn launched(projectile_type: u16, velocity: (f32, f32)) -> Projectile {
        let mut store = ProjectileStore::new();
        let index = store
            .launch(projectile_type, (1000.0, 1000.0), velocity, 15, 0)
            .expect("a known type");
        *store.get(index).unwrap()
    }

    #[test]
    fn a_launch_gets_a_slot_and_a_fresh_generation() {
        let mut store = ProjectileStore::new();
        let first = store.launch(38, (0.0, 0.0), (6.0, 0.0), 15, 0).unwrap();
        let a = store.get(first).unwrap().key;
        store.remove(first);
        let second = store.launch(38, (0.0, 0.0), (6.0, 0.0), 15, 0).unwrap();
        let b = store.get(second).unwrap().key;
        assert_eq!(a.index, b.index, "the slot is reused");
        assert_ne!(
            a.generation, b.generation,
            "but a stale kill packet must not match it"
        );
    }

    /// The generation counts reuses of a slot, not launches anywhere.
    ///
    /// The game builds its key from `++slotGenerations[num]`, indexed by the slot. One counter for
    /// the whole store repeats a value after 16384 launches *anywhere*, where per slot it takes
    /// 16384 reuses *of that slot* — and the generation exists precisely so that a late kill packet
    /// fails to match rather than destroying whatever took the slot since.
    #[test]
    fn projectile_generation_counts_reuses_of_a_slot() {
        let mut store = ProjectileStore::new();
        let first = store.launch(38, (0.0, 0.0), (6.0, 0.0), 15, 0).unwrap();
        let second = store.launch(38, (0.0, 0.0), (6.0, 0.0), 15, 0).unwrap();
        assert_ne!(first, second, "two slots");
        assert_eq!(store.get(first).unwrap().key.generation, 1);
        assert_eq!(
            store.get(second).unwrap().key.generation,
            1,
            "a second slot's first occupant is still its first"
        );

        store.remove(first);
        let again = store.launch(38, (0.0, 0.0), (6.0, 0.0), 15, 0).unwrap();
        assert_eq!(again, first, "the freed slot is taken back");
        assert_eq!(store.get(again).unwrap().key.generation, 2);
        assert_eq!(
            store.get(second).unwrap().key.generation,
            1,
            "the untouched slot should not have moved"
        );
    }

    /// The generation stays inside the fourteen bits the wire gives it.
    ///
    /// Packing masks with `0x3FFF`, so a counter allowed past that silently starts repeating early
    /// — the value 16385 packs identically to 1, and a stale kill packet matches a live projectile.
    #[test]
    fn projectile_generation_stays_inside_its_fourteen_bits() {
        let mut store = ProjectileStore::new();
        for _ in 0..40_000 {
            let index = store.launch(38, (0.0, 0.0), (6.0, 0.0), 15, 0).unwrap();
            let key = store.get(index).unwrap().key;
            assert!(
                key.generation <= 0x3FFF,
                "generation {} does not fit the wire",
                key.generation
            );
            // What is packed is what comes back.
            assert_eq!(ProjectileKey::unpack(key.pack()), key);
            store.remove(index);
        }
    }

    #[test]
    fn a_launch_is_centred_on_its_aim_point() {
        let p = launched(38, (6.0, 0.0));
        let centre = p.center();
        assert!((centre.0 - 1000.0).abs() < 0.01);
        assert!((centre.1 - 1000.0).abs() < 0.01);
    }

    /// The arc is the whole character of a thrown feather: flat, then falling.
    #[test]
    fn a_feather_flies_flat_and_then_falls() {
        let tiles = Air::default();
        let mut p = launched(38, (6.0, 0.0));
        for _ in 0..(ARC_DELAY as i32 - 1) {
            step(&mut p, &tiles, &mut Vec::new());
        }
        assert_eq!(p.velocity.1, 0.0, "still flat");
        for _ in 0..30 {
            step(&mut p, &tiles, &mut Vec::new());
        }
        assert!(p.velocity.1 > 0.0, "now falling, got {}", p.velocity.1);
    }

    #[test]
    fn nothing_falls_faster_than_terminal() {
        let tiles = Air::default();
        let mut p = launched(38, (0.0, 0.0));
        for _ in 0..2000 {
            if step(&mut p, &tiles, &mut Vec::new()) == Outcome::Spent {
                break;
            }
        }
        assert!(p.velocity.1 <= TERMINAL);
    }

    /// A demon's scythe is slow when it leaves and lethal when it arrives.
    #[test]
    fn a_demon_scythe_speeds_up_on_the_way_to_you() {
        let tiles = Air::default();
        let mut p = launched(44, (0.2, 0.0));
        let leaving = p.velocity.0.abs();
        for _ in 0..(SCYTHE_UNTIL as i32) {
            step(&mut p, &tiles, &mut Vec::new());
        }
        let arriving = p.velocity.0.abs();
        assert!(
            arriving > leaving * 10.0,
            "should have picked up speed: {leaving} to {arriving}"
        );
    }

    #[test]
    fn a_scythe_stops_accelerating_eventually() {
        let tiles = Air::default();
        let mut p = launched(44, (0.2, 0.0));
        for _ in 0..(SCYTHE_UNTIL as i32) {
            step(&mut p, &tiles, &mut Vec::new());
        }
        let settled = p.velocity.0;
        for _ in 0..50 {
            step(&mut p, &tiles, &mut Vec::new());
        }
        assert_eq!(p.velocity.0, settled, "it does not accelerate forever");
    }

    #[test]
    fn terrain_stops_what_it_should_and_not_what_it_should_not() {
        let mut tiles = Air::default();
        for y in 0..200 {
            tiles.0.insert((64, y), Tile::block(1));
        }
        // A feather collides.
        let mut feather = launched(38, (16.0, 0.0));
        let mut stopped = false;
        for _ in 0..40 {
            if step(&mut feather, &tiles, &mut Vec::new()) == Outcome::Spent {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "a feather should hit the wall");

        // A skull passes straight through.
        let mut skull = launched(299, (16.0, 0.0));
        for _ in 0..30 {
            assert_eq!(step(&mut skull, &tiles, &mut Vec::new()), Outcome::Flying);
        }
    }

    #[test]
    fn a_projectile_runs_out_of_time() {
        let tiles = Air::default();
        let mut p = launched(38, (0.0, 0.0));
        p.time_left = 3;
        assert_eq!(step(&mut p, &tiles, &mut Vec::new()), Outcome::Flying);
        assert_eq!(step(&mut p, &tiles, &mut Vec::new()), Outcome::Flying);
        assert_eq!(step(&mut p, &tiles, &mut Vec::new()), Outcome::Spent);
    }

    /// Extra updates run the whole body again, at full velocity, so a fast type covers (N+1) times
    /// its per-pass velocity a tick and its life ticks down (N+1) times. The old code moved
    /// velocity/N and spent one tick of life a frame, so these types crawled and overstayed.
    #[test]
    fn extra_updates_move_full_velocity_and_spend_life_each_pass() {
        let tiles = Air::default();
        // The eye laser carries two extra updates: three passes a tick.
        let mut laser = launched(83, (0.0, -3.0));
        assert_eq!(laser.stats.extra_updates, 2);
        let start_y = laser.position.1;
        let start_life = laser.time_left;
        step(&mut laser, &tiles, &mut Vec::new());
        assert!(
            (laser.position.1 - (start_y - 9.0)).abs() < 0.001,
            "three passes of -3 should move -9, not {}",
            laser.position.1 - start_y
        );
        assert_eq!(
            laser.time_left,
            start_life - 3,
            "three passes should each spend a tick of life"
        );
    }

    /// Fast projectiles move in substeps, which is what stops them tunnelling.
    #[test]
    fn a_fast_projectile_cannot_tunnel_through_a_single_tile_wall() {
        let mut tiles = Air::default();
        for y in 0..200 {
            tiles.0.insert((70, y), Tile::block(1));
        }
        // The eye laser has two extra updates and moves nine pixels a step.
        let mut laser = launched(83, (30.0, 0.0));
        assert!(laser.stats.extra_updates > 0);
        let mut stopped = false;
        for _ in 0..40 {
            if step(&mut laser, &tiles, &mut Vec::new()) == Outcome::Spent {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "it should not have passed through");
    }

    #[test]
    fn overlap_is_measured_from_the_corners() {
        let p = launched(38, (0.0, 0.0));
        assert!(p.overlaps((1000.0, 1000.0), (4.0, 4.0)));
        assert!(!p.overlaps((2000.0, 2000.0), (4.0, 4.0)));
    }

    #[test]
    fn an_unknown_type_will_not_launch() {
        let mut store = ProjectileStore::new();
        assert!(store.launch(60_000, (0.0, 0.0), (1.0, 0.0), 1, 0).is_none());
    }

    /// A spiky ball bounces off a wall instead of dying on it, which is the whole point of the
    /// trap that throws them.
    #[test]
    fn a_spiky_ball_bounces_off_a_wall() {
        let mut tiles = Air::default();
        for y in 0..200 {
            tiles.0.insert((64, y), Tile::block(1));
        }
        // Thrown east, at a wall four tiles away.
        let mut ball = launched(185, (6.0, 0.0));
        let mut bounced = false;
        for _ in 0..40 {
            assert_eq!(
                step(&mut ball, &tiles, &mut Vec::new()),
                Outcome::Flying,
                "a spiky ball should never die on a wall"
            );
            if ball.velocity.0 < 0.0 {
                bounced = true;
                break;
            }
        }
        assert!(bounced, "it never turned round: {:?}", ball.velocity);
        assert!(
            ball.velocity.0 < -5.0 && ball.velocity.0 > -6.0,
            "it should keep nine tenths of its speed, not {}",
            ball.velocity.0
        );
    }

    /// A ball that lands on a floor keeps travelling along it: the axis that was blocked bounces,
    /// the one that was free carries on.
    ///
    /// This is what makes a spiky ball trap fill a corridor rather than pile its balls up under
    /// itself, and it is the only reason the things are worth wiring.
    #[test]
    fn a_spiky_ball_rolls_along_the_floor_it_lands_on() {
        let mut tiles = Air::default();
        for x in 0..400 {
            tiles.0.insert((x, 64), Tile::block(1));
        }
        let mut ball = launched(185, (3.0, 0.0));
        let start = ball.position.0;
        for _ in 0..400 {
            if step(&mut ball, &tiles, &mut Vec::new()) == Outcome::Spent {
                break;
            }
        }
        assert!(
            ball.position.0 - start > 200.0,
            "it should have rolled a good way east, not {} pixels",
            ball.position.0 - start
        );
        assert!(
            ball.position.1 < 64.0 * 16.0,
            "and stayed on top of the floor, not fallen through to {}",
            ball.position.1
        );
    }

    /// Put a boulder at rest at a tile position, the way a Boulder Statue and a broken Boulder
    /// tile both release one: no velocity at all, and gravity is the only thing that moves it.
    fn dropped_boulder(store: &mut ProjectileStore, tile_x: f32, tile_y: f32) -> u16 {
        let half = 31.0 / 2.0;
        store
            .launch(
                99,
                (tile_x * TILE + half, tile_y * TILE + half),
                (0.0, 0.0),
                70,
                0,
            )
            .expect("the boulder is a known type")
    }

    /// A Boulder Statue drops a boulder that *falls*, and once it lands it rolls.
    ///
    /// This is the whole trap and none of it happened: vanilla launches the boulder at rest and the
    /// arm gives it gravity, so with no arm at all a wired statue used to put a stationary hostile
    /// box under itself for a full minute. `Projectile.cs:26596-26746`.
    #[test]
    fn a_boulder_falls_and_then_rolls_away_from_the_wall_beside_it() {
        let mut tiles = Air::default();
        for x in 55..90 {
            tiles.0.insert((x, 64), Tile::block(1));
        }
        // A wall on its left, so it must roll east.
        for y in 50..64 {
            tiles.0.insert((61, y), Tile::block(1));
        }
        let mut store = ProjectileStore::new();
        let index = dropped_boulder(&mut store, 62.0, 56.0);
        let start = *store.get(index).unwrap();
        assert_eq!(start.velocity, (0.0, 0.0), "it is released at rest");

        let mut boulder = start;
        for _ in 0..200 {
            if step(&mut boulder, &tiles, &mut Vec::new()) == Outcome::Spent {
                break;
            }
        }
        assert!(
            boulder.position.1 > start.position.1 + 100.0,
            "it should have fallen to the floor, not hung at {}",
            boulder.position.1
        );
        assert!(
            boulder.velocity.0 > 1.0,
            "and rolled east away from the wall on its left, not sat at {}",
            boulder.velocity.0
        );
        assert!(
            boulder.position.0 > start.position.0 + 50.0,
            "which means actually travelling: {} pixels",
            boulder.position.0 - start.position.0
        );
    }

    /// It works up to seven pixels a tick and no further (`Projectile.cs:26715-26724`).
    ///
    /// The gate is `velocity.X < 7f` and the step is `+= 0.05f`, so the last step it is allowed to
    /// take can carry it a hair past seven and leave it there. That overshoot is the game's own and
    /// is transcribed rather than clamped; the tolerance below is what makes room for it.
    #[test]
    fn a_rolling_boulder_stops_gaining_speed_at_seven() {
        let mut tiles = Air::default();
        for x in 0..4000 {
            tiles.0.insert((x, 64), Tile::block(1));
        }
        for y in 50..64 {
            tiles.0.insert((61, y), Tile::block(1));
        }
        let mut store = ProjectileStore::new();
        let index = dropped_boulder(&mut store, 62.0, 62.0);
        let mut boulder = *store.get(index).unwrap();
        for _ in 0..1000 {
            if step(&mut boulder, &tiles, &mut Vec::new()) == Outcome::Spent {
                break;
            }
            assert!(
                boulder.velocity.0 <= BOULDER_TOP_SPEED + BOULDER_ACCEL,
                "a boulder should never pass seven by more than one step: {}",
                boulder.velocity.0
            );
        }
        assert!(
            boulder.velocity.0 > 6.0,
            "but it should get there: {}",
            boulder.velocity.0
        );
    }

    /// A boulder that meets a wall head on breaks; one that merely lands on a floor does not.
    ///
    /// `Projectile.cs:19084-19098`: the kill is on the *horizontal* axis changing, which is why
    /// falling straight down onto the ground leaves it alive to roll. Getting that the wrong way
    /// round would end the trap on the first tile it touched.
    #[test]
    fn a_boulder_breaks_on_a_wall_but_not_on_the_floor() {
        let mut tiles = Air::default();
        for x in 55..70 {
            tiles.0.insert((x, 64), Tile::block(1));
        }
        let mut store = ProjectileStore::new();

        let index = dropped_boulder(&mut store, 60.0, 56.0);
        let mut lands = *store.get(index).unwrap();
        for _ in 0..40 {
            assert_eq!(
                step(&mut lands, &tiles, &mut Vec::new()),
                Outcome::Flying,
                "landing on a floor must not break it"
            );
        }

        // And released *overlapping* the ground, which a statue flush against a floor will do. Both
        // axes read as blocked on that first tick, and the only thing telling it apart from a
        // boulder that has run into a wall is that this one was never moving sideways. Exactly one
        // tick is asserted because that is the whole contract: once it picks a direction, a boulder
        // embedded in rock has genuinely hit a wall and is supposed to break.
        let index = dropped_boulder(&mut store, 60.0, 63.0);
        let mut embedded = *store.get(index).unwrap();
        embedded.position.1 = 64.0 * TILE - 8.0;
        assert_eq!(
            step(&mut embedded, &tiles, &mut Vec::new()),
            Outcome::Flying,
            "a boulder released touching the ground broke before it had moved at all"
        );

        for y in 50..64 {
            tiles.0.insert((66, y), Tile::block(1));
        }
        let index = dropped_boulder(&mut store, 60.0, 63.0);
        let mut hits = *store.get(index).unwrap();
        hits.velocity = (6.0, 0.0);
        let mut broke = false;
        for _ in 0..40 {
            if step(&mut hits, &tiles, &mut Vec::new()) == Outcome::Spent {
                broke = true;
                break;
            }
        }
        assert!(
            broke,
            "it rolled into a wall and lived: {:?}",
            hits.position
        );
    }

    /// Seven ticks of grace, because the boulder appears inside whoever pulled the lever.
    ///
    /// `Projectile.CanDamage` (`Projectile.cs:12491-12494`). A Boulder Statue puts its boulder
    /// twenty-eight pixels below its own base, which is well inside the hitbox of anybody standing
    /// beside it, so without this the trap hits its own operator before it has moved at all.
    #[test]
    fn a_boulder_cannot_hurt_anybody_for_its_first_seven_ticks() {
        let tiles = Air::default();
        let mut store = ProjectileStore::new();
        let index = dropped_boulder(&mut store, 60.0, 20.0);
        let mut boulder = *store.get(index).unwrap();
        assert!(!boulder.can_damage(), "not on the tick it is released");
        for tick in 1..=7 {
            step(&mut boulder, &tiles, &mut Vec::new());
            assert!(
                !boulder.can_damage(),
                "still harmless on tick {tick}, local_ai[2] = {}",
                boulder.local_ai[2]
            );
        }
        step(&mut boulder, &tiles, &mut Vec::new());
        assert!(
            boulder.can_damage(),
            "and dangerous from the eighth: local_ai[2] = {}",
            boulder.local_ai[2]
        );
    }

    /// A flamethrower trap is not the thing that burns: it emits a flame every sixth tick, and
    /// that is what carries the damage.
    #[test]
    fn a_flamethrower_emits_flames_rather_than_burning() {
        let tiles = Air::default();
        let mut trap = launched(187, (5.0, 0.0));
        let mut emitted = Vec::new();
        for _ in 0..30 {
            step(&mut trap, &tiles, &mut emitted);
        }
        assert_eq!(emitted.len(), 5, "one every six ticks for thirty ticks");
        assert_eq!(emitted[0].projectile_type, 188);
        assert_eq!(
            emitted[0].velocity,
            (5.0, 0.0),
            "the flame goes where the trap points"
        );
        assert_eq!(emitted[0].damage, trap.damage);
    }

    /// A spear reaches out of its trap and then pulls back in, dying when it is home rather than
    /// hanging in the air.
    #[test]
    fn a_spear_reaches_out_and_comes_back() {
        let tiles = Air::default();
        let mut spear = launched(186, (8.0, 0.0));
        let start = spear.position.0;
        let mut furthest = start;
        let mut ticks = 0;
        loop {
            ticks += 1;
            let out = step(&mut spear, &tiles, &mut Vec::new());
            furthest = furthest.max(spear.position.0);
            if out == Outcome::Spent {
                break;
            }
            assert!(ticks < 300, "the spear never came home");
        }
        assert!(
            (furthest - start) > 290.0 && (furthest - start) < 320.0,
            "it should reach about three hundred pixels, not {}",
            furthest - start
        );
        assert!(
            (spear.position.0 - start).abs() < 20.0,
            "it should die where it started, not at {}",
            spear.position.0 - start
        );
    }

    /// A spear set into a wall stops at the wall rather than reaching its full length through it.
    #[test]
    fn a_spear_stops_at_what_it_hits() {
        let mut tiles = Air::default();
        for y in 0..200 {
            tiles.0.insert((66, y), Tile::block(1));
        }
        let mut spear = launched(186, (8.0, 0.0));
        let start = spear.position.0;
        let mut furthest = start;
        for _ in 0..300 {
            if step(&mut spear, &tiles, &mut Vec::new()) == Outcome::Spent {
                break;
            }
            furthest = furthest.max(spear.position.0);
        }
        assert!(
            (furthest - start) < 100.0,
            "the wall is 56 pixels away; it reached {}",
            furthest - start
        );
    }

    /// A flame burns out in a second however long its type says it lives.
    #[test]
    fn a_flame_burns_out_in_a_second() {
        let tiles = Air::default();
        let mut flame = launched(188, (4.0, 0.0));
        assert!(
            flame.time_left > 60,
            "it starts with the type's own lifetime"
        );
        step(&mut flame, &tiles, &mut Vec::new());
        assert!(
            flame.time_left <= 60,
            "and is cut to a second on its first tick"
        );
    }
}
