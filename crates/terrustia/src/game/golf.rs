//! A golf ball's physics: `Terraria.Physics.BallCollision` and the golf contact listener.
//!
//! Every other projectile in this server is a routine - a handful of statements deciding a
//! velocity. A golf ball is the exception: `aiStyle 149` does nothing of its own but hand the
//! projectile to `GolfHelper.StepGolfBall`, which is a small rigid-body simulation with substeps,
//! circle-against-edge collision, per-material dampening and a resting state
//! (`Terraria.Physics/BallCollision.cs:24-90`, `Terraria.GameContent.Golf/GolfHelper.cs:32-160`).
//!
//! It is in the roster because the **Golfer** is one of the twenty-eight town NPCs that fight
//! back, and what he throws is a golf ball. Without this it flew in a straight line and never
//! landed.
//!
//! The shape of the thing, in order:
//!
//! * **Drag first, then substep.** Both the velocity and the spin lose one per cent, and the step
//!   is then divided into `ceil(speed / 2)` passes so a fast ball cannot tunnel through a wall.
//!   Gravity is divided by the *square* of the pass count, because it is applied once per pass to
//!   a velocity that has already been divided once.
//! * **Buried is stopped.** A ball whose centre is inside a solid tile has its velocity and spin
//!   zeroed outright rather than being pushed out - that is the game's own answer, and it is what
//!   makes a ball that lands in a closing door simply stop.
//! * **The collision is a circle against the nearest tile *edge*,** not a box against a box.
//!   Which edges are eligible is decided by the direction of travel, so a ball moving right never
//!   collides with a tile's right face, and the interior faces of a solid run are skipped by
//!   asking whether the neighbour on that side is solid.
//! * **Resting is a state, not a speed.** It needs a contact this step, a sideways speed inside a
//!   hundredth of a pixel, and a downward speed between zero and one gravity - so a ball rolling
//!   slowly along the ground is still `Moving`, and only one that has actually settled reads as
//!   `Resting`.
//!
//! **What is deliberately not here**, all of it the shot rather than the ball, and all of it on
//! the client that took the shot: the club impact that launches one, the accessory that resists
//! dampening, the cup at tile 476 (`PutBallInCup` scores a hole, which needs a golf state this
//! server does not keep), and the conveyor belts at 421/422. The two liquid dampenings are here.

use terrustia_proto::golf_physics::{self, Golf};
use terrustia_proto::tile_solid::{solid, solid_top};

use super::npc::{TILE, TileView};

/// `GolfHelper.PhysicsProperties = new PhysicsProperties(0.3f, 0.99f)`.
const GRAVITY: f32 = 0.3;
const DRAG: f32 = 0.99;
/// `if (num3 > 1000f)`: the speed a step is clamped to before it is divided into passes.
const SPEED_CAP: f32 = 1000.0;
/// `Math.Max(1, (int)Math.Ceiling(num3 / 2f))`.
const PASS_PIXELS: f32 = 2.0;
/// `position = collisionPoint + vector * (num2 + 0.0001f)`: the sliver that keeps a resolved
/// contact from immediately re-colliding.
const CLEARANCE: f32 = 0.0001;
/// What water and honey take off a ball passing through them (`GolfHelper.cs:141-148`).
const WATER_DRAG: f32 = 0.91;
const HONEY_DRAG: f32 = 0.8;

/// What one step concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BallState {
    Moving,
    /// Settled: it had a contact this step and is no longer going anywhere.
    Resting,
    /// It left the world, which ends it.
    OutOfBounds,
}

/// Which of a tile's four faces and four slope diagonals a ball may hit, given where it is going.
///
/// `BallCollision.TileEdges`. A ball moving right can only meet a *left* face, so the set is
/// chosen once per step from the velocity and then narrowed per tile.
#[derive(Clone, Copy)]
struct Edges(u32);

impl Edges {
    const TOP: u32 = 1;
    const BOTTOM: u32 = 2;
    const LEFT: u32 = 4;
    const RIGHT: u32 = 8;
    const TOP_LEFT_SLOPE: u32 = 0x10;
    const TOP_RIGHT_SLOPE: u32 = 0x20;
    const BOTTOM_LEFT_SLOPE: u32 = 0x40;
    const BOTTOM_RIGHT_SLOPE: u32 = 0x80;

    fn has(self, which: u32) -> bool {
        self.0 & which != 0
    }

    fn only(self, keep: u32) -> Self {
        Self(self.0 & keep)
    }
}

/// A tile edge, as the two points of a segment.
#[derive(Clone, Copy, Default)]
struct Segment {
    start: (f32, f32),
    end: (f32, f32),
}

fn dot(a: (f32, f32), b: (f32, f32)) -> f32 {
    a.0 * b.0 + a.1 * b.1
}

/// `BallCollision.ClosestPointOnLineSegment`.
fn closest_on(point: (f32, f32), segment: Segment) -> (f32, f32) {
    let to_point = (point.0 - segment.start.0, point.1 - segment.start.1);
    let along = (
        segment.end.0 - segment.start.0,
        segment.end.1 - segment.start.1,
    );
    let length_squared = along.0 * along.0 + along.1 * along.1;
    if length_squared == 0.0 {
        return segment.start;
    }
    let t = dot(to_point, along) / length_squared;
    if t < 0.0 {
        return segment.start;
    }
    if t > 1.0 {
        return segment.end;
    }
    (segment.start.0 + along.0 * t, segment.start.1 + along.1 * t)
}

/// `BallCollision.IsNeighborSolid`: a fully solid neighbour, which hides the face between them.
fn neighbour_solid(tiles: &impl TileView, x: i32, y: i32) -> bool {
    let tile = tiles.tile(x, y);
    tile.is_active() && solid(tile.block) && !solid_top(tile.block)
}

/// `BallCollision.GetSlopeEdge`, which also narrows the eligible faces for a sloped tile.
fn slope_edge(edges: &mut Edges, slope: u8, at: (f32, f32)) -> Option<Segment> {
    let (keep, needs, segment) = match slope {
        1 => (
            Edges::BOTTOM | Edges::LEFT | Edges::BOTTOM_LEFT_SLOPE,
            Edges::BOTTOM_LEFT_SLOPE,
            Segment {
                start: at,
                end: (at.0 + TILE, at.1 + TILE),
            },
        ),
        2 => (
            Edges::BOTTOM | Edges::RIGHT | Edges::BOTTOM_RIGHT_SLOPE,
            Edges::BOTTOM_RIGHT_SLOPE,
            Segment {
                start: (at.0, at.1 + TILE),
                end: (at.0 + TILE, at.1),
            },
        ),
        3 => (
            Edges::TOP | Edges::LEFT | Edges::TOP_LEFT_SLOPE,
            Edges::TOP_LEFT_SLOPE,
            Segment {
                start: (at.0, at.1 + TILE),
                end: (at.0 + TILE, at.1),
            },
        ),
        4 => (
            Edges::TOP | Edges::RIGHT | Edges::TOP_RIGHT_SLOPE,
            Edges::TOP_RIGHT_SLOPE,
            Segment {
                start: at,
                end: (at.0 + TILE, at.1 + TILE),
            },
        ),
        _ => return None,
    };
    *edges = edges.only(keep);
    edges.has(needs).then_some(segment)
}

/// `BallCollision.GetTopOrBottomEdge`.
fn horizontal_edge(
    tiles: &impl TileView,
    edges: Edges,
    x: i32,
    y: i32,
    at: (f32, f32),
    half_brick: bool,
) -> Option<Segment> {
    if edges.has(Edges::BOTTOM) {
        let below = tiles.tile(x, y + 1);
        if neighbour_solid(tiles, x, y + 1)
            && below.slope != 1
            && below.slope != 2
            && !below
                .flags
                .has(terrustia_proto::tile::TileFlags::HALF_BRICK)
        {
            return None;
        }
        return Some(Segment {
            start: (at.0, at.1 + TILE),
            end: (at.0 + TILE, at.1 + TILE),
        });
    }
    if edges.has(Edges::TOP) {
        let above = tiles.tile(x, y - 1);
        if !half_brick && neighbour_solid(tiles, x, y - 1) && above.slope != 3 && above.slope != 4 {
            return None;
        }
        // A half brick's top face is halfway down, which is what makes a ball roll along one.
        let top = if half_brick { at.1 + TILE / 2.0 } else { at.1 };
        return Some(Segment {
            start: (at.0, top),
            end: (at.0 + TILE, top),
        });
    }
    None
}

/// `BallCollision.GetLeftOrRightEdge`.
fn vertical_edge(
    tiles: &impl TileView,
    edges: Edges,
    x: i32,
    y: i32,
    at: (f32, f32),
    half_brick: bool,
) -> Option<Segment> {
    let is_half = |tile: terrustia_proto::tile::Tile| {
        tile.flags.has(terrustia_proto::tile::TileFlags::HALF_BRICK)
    };
    if edges.has(Edges::LEFT) {
        let left = tiles.tile(x - 1, y);
        if neighbour_solid(tiles, x - 1, y)
            && left.slope != 1
            && left.slope != 3
            && (!is_half(left) || half_brick)
        {
            return None;
        }
        let top = if half_brick { at.1 + TILE / 2.0 } else { at.1 };
        return Some(Segment {
            start: (at.0, top),
            end: (at.0, at.1 + TILE),
        });
    }
    if edges.has(Edges::RIGHT) {
        let right = tiles.tile(x + 1, y);
        if neighbour_solid(tiles, x + 1, y)
            && right.slope != 2
            && right.slope != 4
            && (!is_half(right) || half_brick)
        {
            return None;
        }
        let top = if half_brick { at.1 + TILE / 2.0 } else { at.1 };
        return Some(Segment {
            start: (at.0 + TILE, top),
            end: (at.0 + TILE, at.1 + TILE),
        });
    }
    None
}

/// `BallCollision.GetCollisionPointForTile`: the nearest point on any eligible edge of one tile.
fn contact_with(
    tiles: &impl TileView,
    edges: Edges,
    x: i32,
    y: i32,
    centre: (f32, f32),
) -> Option<((f32, f32), f32)> {
    let tile = tiles.tile(x, y);
    if !tile.is_active() || (!solid(tile.block) && !solid_top(tile.block)) {
        return None;
    }
    // A platform is only solid at its top, and only the un-framed variety at that.
    if !solid(tile.block) && solid_top(tile.block) && tile.frame_y != 0 {
        return None;
    }
    let mut edges = edges;
    if solid_top(tile.block) {
        edges = edges.only(Edges::TOP | Edges::BOTTOM_LEFT_SLOPE | Edges::BOTTOM_RIGHT_SLOPE);
    }
    let at = (x as f32 * TILE, y as f32 * TILE);
    let half_brick = tile.flags.has(terrustia_proto::tile::TileFlags::HALF_BRICK);
    let mut best: Option<((f32, f32), f32)> = None;
    let mut consider = |segment: Segment| {
        let point = closest_on(centre, segment);
        let away = (point.0 - centre.0).powi(2) + (point.1 - centre.1).powi(2);
        if best.is_none_or(|(_, was)| away < was) {
            best = Some((point, away));
        }
    };
    if let Some(segment) = slope_edge(&mut edges, tile.slope, at) {
        consider(segment);
    }
    if let Some(segment) = horizontal_edge(tiles, edges, x, y, at, half_brick) {
        consider(segment);
    }
    if let Some(segment) = vertical_edge(tiles, edges, x, y, at, half_brick) {
        consider(segment);
    }
    best
}

/// `BallCollision.GetClosestEdgeToCircle`: the nearest contact across every tile the ball overlaps.
fn closest_contact(
    tiles: &impl TileView,
    position: (f32, f32),
    size: (f32, f32),
    velocity: (f32, f32),
) -> Option<((f32, f32), u16)> {
    let mut set = 0u32;
    set |= if velocity.1 < 0.0 {
        Edges::BOTTOM
    } else {
        Edges::TOP
    };
    set |= if velocity.0 < 0.0 {
        Edges::RIGHT
    } else {
        Edges::LEFT
    };
    set |= if velocity.1 > velocity.0 {
        Edges::BOTTOM_LEFT_SLOPE
    } else {
        Edges::TOP_RIGHT_SLOPE
    };
    set |= if velocity.1 > -velocity.0 {
        Edges::BOTTOM_RIGHT_SLOPE
    } else {
        Edges::TOP_LEFT_SLOPE
    };
    let edges = Edges(set);

    let centre = (position.0 + size.0 * 0.5, position.1 + size.1 * 0.5);
    let left = (position.0 / TILE).floor() as i32;
    let top = (position.1 / TILE).floor() as i32;
    let right = ((position.0 + size.0) / TILE).floor() as i32;
    let bottom = ((position.1 + size.1) / TILE).floor() as i32;

    let mut nearest = f32::MAX;
    let mut found = None;
    for x in left..=right {
        for y in top..=bottom {
            let Some((point, away)) = contact_with(tiles, edges, x, y, centre) else {
                continue;
            };
            // Behind it does not count: a contact the ball is already moving away from is one it
            // has just bounced off, and resolving it again would trap the ball on the surface.
            //
            // **Carried from vanilla, and untested, because nothing distinguishes it here.** The
            // clearance sliver already excludes a just-resolved contact - it puts the ball at
            // `radius + 0.0001`, and the search below only returns a contact strictly inside
            // `radius` - so this test can only ever fire on a *different* tile. Flat ground, a
            // concave corner, a one-tile notch and a run of slopes were each rolled, dropped and
            // lobbed with the clause neutered, and every one produced byte-identical positions,
            // tick counts and end states. It stays because it is the game's line and removing a
            // line for want of a case is how a transcription drifts; it has no test because a
            // test that cannot fail is worse than none.
            let outward = (centre.0 - point.0, centre.1 - point.1);
            if away >= nearest || dot(velocity, outward) > 0.0 {
                continue;
            }
            nearest = away;
            found = Some((point, tiles.tile(x, y).block));
        }
    }
    let radius = size.0 / 2.0;
    (nearest < radius * radius).then_some(found).flatten()
}

/// What a ball is passing through, if anything (`BallCollision.CheckForPassThrough`).
enum Through {
    Tile(u16),
    Water,
    Honey,
    Lava,
}

fn passing_through(tiles: &impl TileView, centre: (f32, f32)) -> Option<Through> {
    let (x, y) = ((centre.0 / TILE) as i32, (centre.1 / TILE) as i32);
    let tile = tiles.tile(x, y);
    if tile.is_active() {
        // Inside the tile's own body, which for a slope or a half brick is only part of its square.
        let inside =
            if tile.slope == 0 && !tile.flags.has(terrustia_proto::tile::TileFlags::HALF_BRICK) {
                true
            } else {
                let within = (centre.0 / TILE - x as f32, centre.1 / TILE - y as f32);
                match tile.slope {
                    0 => within.1 > 0.5,
                    1 => within.1 > within.0,
                    2 => within.1 > 1.0 - within.0,
                    3 => within.1 < 1.0 - within.0,
                    4 => within.1 < within.0,
                    _ => false,
                }
            };
        return inside.then_some(Through::Tile(tile.block));
    }
    if tile.liquid > 0 {
        // The surface is measured off the liquid's own depth, so a ball skimming the top of a
        // shallow pool is not slowed by it.
        let surface = (y + 1) as f32 * TILE - f32::from(tile.liquid) / 255.0 * TILE;
        if surface >= centre.1 {
            return None;
        }
        // Shimmer has no golf listener case of its own, so it reads as water, which is vanilla's
        // `default:` arm rather than a choice made here.
        return Some(match tile.liquid_kind {
            terrustia_proto::tile::Liquid::Lava => Through::Lava,
            terrustia_proto::tile::Liquid::Honey => Through::Honey,
            _ => Through::Water,
        });
    }
    None
}

/// Drive one golf ball for a tick. `BallCollision.Step`.
///
/// `spin` is vanilla's `entityAngularVelocity`, which is not decoration: it is dampened with the
/// velocity, rebuilt from the surface normal on every contact, and is what a client rolls the
/// sprite by.
pub fn step_ball(
    position: &mut (f32, f32),
    velocity: &mut (f32, f32),
    spin: &mut f32,
    size: (f32, f32),
    tiles: &impl TileView,
) -> BallState {
    let radius = size.0 * 0.5;
    *spin *= DRAG;
    velocity.0 *= DRAG;
    velocity.1 *= DRAG;

    let mut speed = velocity.0.hypot(velocity.1);
    if speed > SPEED_CAP {
        let scale = SPEED_CAP / speed;
        velocity.0 *= scale;
        velocity.1 *= scale;
        speed = SPEED_CAP;
    }
    // One pass per two pixels of travel, so a fast ball cannot step over a wall.
    let passes = (speed / PASS_PIXELS).ceil().max(1.0);
    let share = 1.0 / passes;
    let mut step = (velocity.0 * share, velocity.1 * share);
    let mut turn = *spin * share;
    // Divided by the *square*: gravity is applied once per pass to a velocity already divided once.
    let pull = GRAVITY / (passes * passes);

    let mut touched = false;
    for _ in 0..(passes as i32) {
        step.1 += pull;
        let centre = (position.0 + size.0 * 0.5, position.1 + size.1 * 0.5);
        match passing_through(tiles, centre) {
            Some(Through::Tile(block)) if solid(block) && !solid_top(block) => {
                // Buried in something solid: it stops dead rather than being pushed out.
                step = (0.0, 0.0);
                turn = 0.0;
                touched = true;
            }
            Some(through) => {
                let keep = match through {
                    Through::Water => WATER_DRAG,
                    Through::Honey => HONEY_DRAG,
                    Through::Tile(block) => golf_physics::of(block).through,
                    // Lava does nothing to a ball's movement in vanilla's listener.
                    Through::Lava => 1.0,
                };
                step.0 *= keep;
                step.1 *= keep;
                turn *= keep;
            }
            None => {}
        }
        position.0 += step.0;
        position.1 += step.1;
        if !position.0.is_finite() || !position.1.is_finite() || position.1 < -TILE {
            return BallState::OutOfBounds;
        }
        if let Some((point, block)) = closest_contact(tiles, *position, size, step) {
            let centre = (position.0 + size.0 * 0.5, position.1 + size.1 * 0.5);
            let away = (centre.0 - point.0, centre.1 - point.1);
            let length = away.0.hypot(away.1);
            if length == 0.0 {
                continue;
            }
            let normal = (away.0 / length, away.1 / length);
            // Placed exactly one radius off the surface, plus a sliver so the next pass does not
            // find the same contact again.
            position.0 = point.0 + normal.0 * (radius + CLEARANCE) - size.0 * 0.5;
            position.1 = point.1 + normal.1 * (radius + CLEARANCE) - size.1 * 0.5;
            // Reflect, then split the result into its along-surface and into-surface parts so the
            // material can dampen each differently: `side` is what decides how far a ball rolls
            // and `direct` is what decides how high it bounces.
            let into = dot(step, normal);
            let reflected = (
                step.0 - 2.0 * into * normal.0,
                step.1 - 2.0 * into * normal.1,
            );
            let Golf { direct, side, .. } = golf_physics::of(block);
            let along = dot(reflected, normal) * (direct - side);
            step = (
                reflected.0 * side + normal.0 * along,
                reflected.1 * side + normal.1 * along,
            );
            touched = true;
            turn = (normal.0 * step.1 - normal.1 * step.0) / radius;
        }
    }
    velocity.0 = step.0 / share;
    velocity.1 = step.1 / share;
    *spin = turn / share;

    // Settled: it touched something this step, is not travelling sideways, and is not falling
    // faster than one tick of gravity. A ball rolling slowly is still moving.
    if touched
        && velocity.0 > -0.01
        && velocity.0 < 0.01
        && velocity.1 <= 0.0
        && velocity.1 > -GRAVITY
    {
        return BallState::Resting;
    }
    BallState::Moving
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use terrustia_proto::tile::Tile;

    struct Course(HashMap<(i32, i32), Tile>);

    impl TileView for Course {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    /// Flat ground of one material across a wide span, with the surface at tile row 100.
    fn ground(block: u16) -> Course {
        let mut c = Course(HashMap::new());
        for x in -200..400 {
            for y in 100..110 {
                c.0.insert((x, y), Tile::block(block));
            }
        }
        c
    }

    /// A golf ball's own size, from its table entry.
    const BALL: (f32, f32) = (14.0, 14.0);

    /// Roll one until it stops or gives up, and say where it ended and after how long.
    fn roll(
        course: &Course,
        from: (f32, f32),
        velocity: (f32, f32),
    ) -> ((f32, f32), i32, BallState) {
        let mut at = from;
        let mut v = velocity;
        let mut spin = 0.0;
        for tick in 1..3_000 {
            let state = step_ball(&mut at, &mut v, &mut spin, BALL, course);
            if state != BallState::Moving {
                return (at, tick, state);
            }
        }
        (at, 3_000, BallState::Moving)
    }

    /// A ball dropped on flat ground comes to rest on it rather than falling through or bouncing
    /// for ever.
    ///
    /// `BallCollision.Step`'s resting test (`:82-86`) needs a contact this step, a sideways speed
    /// inside a hundredth of a pixel and a downward speed under one gravity - so this is the whole
    /// simulation end to end, not one branch of it.
    #[test]
    fn a_dropped_ball_settles_on_the_ground() {
        let course = ground(1);
        let floor = 100.0 * TILE;
        let (at, ticks, state) = roll(&course, (1000.0, floor - 200.0), (0.0, 0.0));
        assert_eq!(
            state,
            BallState::Resting,
            "it should settle, not fall through"
        );
        assert!(
            (at.1 + BALL.1 - floor).abs() < 2.0,
            "and settle *on* the surface: bottom at {} against a floor at {floor}",
            at.1 + BALL.1
        );
        // Measured at 314 ticks from two hundred pixels up. Stone keeps 0.95 of a head-on bounce,
        // which is a very bouncy surface, so this is dozens of ever-smaller hops rather than one
        // landing - and the bound is set from what it actually does rather than from a guess,
        // because the first guess here was 300 and the behaviour was right.
        assert!(
            ticks < 500,
            "it should settle in seconds, not eventually: {ticks}"
        );
    }

    /// The material is half the simulation: a ball rolls much further on ice than on sand.
    ///
    /// Side dampening is 1.0 for ice and 0.2 for sand, and it is what a glancing contact keeps
    /// along the surface - so it is the number that decides how far a ball goes. Without the
    /// per-tile table one figure would serve both, and this is what that would cost.
    #[test]
    fn a_ball_rolls_far_on_ice_and_dies_in_sand() {
        /// `TileID.IceBlock` and `TileID.Sand`.
        const ICE: u16 = 161;
        const SAND: u16 = 53;

        let start = (1000.0, 100.0 * TILE - BALL.1 - 1.0);
        let (ice_at, _, _) = roll(&ground(ICE), start, (8.0, 0.0));
        let (sand_at, _, _) = roll(&ground(SAND), start, (8.0, 0.0));
        // A ball still travelling is `Moving`, however long it has been rolling: `Resting` needs a
        // sideways speed inside a hundredth of a pixel, and without that bound a ball would be
        // declared settled the moment it touched anything - which is a golf ball that stops dead
        // where it lands.
        {
            let course = ground(ICE);
            let mut at = start;
            let mut v = (8.0, 0.0);
            let mut spin = 0.0;
            for tick in 1..20 {
                assert_eq!(
                    step_ball(&mut at, &mut v, &mut spin, BALL, &course),
                    BallState::Moving,
                    "tick {tick}: still rolling at {}",
                    v.0
                );
            }
            assert!(v.0 > 1.0, "and genuinely still moving: {}", v.0);
        }

        let on_ice = ice_at.0 - start.0;
        let on_sand = sand_at.0 - start.0;
        assert!(on_sand >= 0.0 && on_ice > 0.0, "both should go forwards");
        assert!(
            on_ice > on_sand * 3.0,
            "ice keeps all of a glancing contact and sand keeps a fifth: {on_ice} against {on_sand}"
        );
    }

    /// The other half of the material: `direct` is what a head-on bounce keeps, so a ball dropped
    /// straight down bounces high off ice and barely at all off sand.
    ///
    /// `side` decides how far a ball *rolls* and `direct` decides how high it *bounces*, and
    /// vanilla splits a contact into the two by projecting the reflection onto the surface normal
    /// (`GolfHelper.cs:36-39`). Rolling distance alone cannot tell the split from a flat
    /// `velocity *= side`, which is why this is a second test rather than another assertion.
    #[test]
    fn a_ball_bounces_high_off_ice_and_dead_off_sand() {
        /// `TileID.IceBlock` and `TileID.Sand`.
        const ICE: u16 = 161;
        const SAND: u16 = 53;

        let highest_after_landing = |block: u16| {
            let course = ground(block);
            let floor = 100.0 * TILE;
            let mut at = (1000.0, floor - 200.0);
            let mut v = (0.0, 0.0);
            let mut spin = 0.0;
            let mut landed = false;
            let mut highest = f32::MAX;
            for _ in 0..600 {
                if step_ball(&mut at, &mut v, &mut spin, BALL, &course) != BallState::Moving {
                    break;
                }
                // The first upward tick after the drop is the bounce.
                if !landed && v.1 < 0.0 {
                    landed = true;
                }
                if landed {
                    highest = highest.min(at.1);
                }
            }
            floor - highest
        };
        let ice = highest_after_landing(ICE);
        let sand = highest_after_landing(SAND);
        assert!(
            ice > sand * 2.0,
            "ice keeps 0.95 of a head-on bounce and sand keeps 0.3: {ice} against {sand}"
        );
    }

    /// A full step adds exactly one gravity however fast the ball is going.
    ///
    /// This is what the square in `num6 = Gravity / (num4 * num4)` is *for*, and it is invisible
    /// at any single speed. The velocity is divided by the pass count, gravity is applied once per
    /// pass, and the result is multiplied back - so the pull has to be divided twice to come out
    /// as one gravity per tick rather than one per pass. Divide it once and a fast ball falls
    /// harder than a slow one, which is not a thing the game does.
    #[test]
    fn gravity_is_one_a_tick_at_any_speed() {
        let sky = Course(HashMap::new());
        let gained_at = |speed: f32| {
            let mut at = (1000.0, 0.0);
            let mut v = (speed, 0.0);
            let mut spin = 0.0;
            let before = v.1;
            step_ball(&mut at, &mut v, &mut spin, BALL, &sky);
            // The drag is applied to the *old* velocity before gravity, so the gain is measured
            // against what drag left rather than against the raw previous value.
            v.1 - before * DRAG
        };
        // One pass at rest, and hundreds at speed.
        let slow = gained_at(0.0);
        let fast = gained_at(600.0);
        assert!(
            (slow - GRAVITY).abs() < 0.001,
            "a still ball should gain one gravity: {slow}"
        );
        assert!(
            (fast - GRAVITY).abs() < 0.01,
            "and so should one crossing six hundred pixels: {fast}"
        );
    }

    /// A head-on bounce keeps the material's `direct`, not its `side`.
    ///
    /// The two are separate numbers and vanilla splits every contact between them by projecting
    /// the reflection onto the surface normal. Ice is the case that exposes a missing split: its
    /// `side` is 1.0 and its `direct` is 0.95, so a bounce that kept `side` on both axes would
    /// come off a floor *perfectly elastic* and never settle at all.
    #[test]
    fn a_head_on_bounce_keeps_the_direct_figure() {
        /// `TileID.IceBlock`, whose side is 1.0 and direct 0.95.
        const ICE: u16 = 161;

        let course = ground(ICE);
        let floor = 100.0 * TILE;
        let mut at = (1000.0, floor - 300.0);
        let mut v = (0.0, 0.0);
        let mut spin = 0.0;
        let mut impact = 0.0;
        for _ in 0..600 {
            let before = v.1;
            if step_ball(&mut at, &mut v, &mut spin, BALL, &course) != BallState::Moving {
                break;
            }
            // The tick the sign flips is the bounce: `before` is the speed going in.
            if before > 0.0 && v.1 < 0.0 {
                impact = before;
                break;
            }
        }
        assert!(
            impact > 5.0,
            "it should have been going somewhere: {impact}"
        );
        let rebound = -v.1;
        let kept = rebound / impact;
        assert!(
            (0.90..0.97).contains(&kept),
            "ice keeps 0.95 of a head-on bounce, not its 1.0 side figure: kept {kept}"
        );
    }

    /// A ball leaving the world ends rather than falling for ever.
    #[test]
    fn a_ball_that_leaves_the_world_is_spent() {
        let course = Course(HashMap::new());
        let (_, _, state) = roll(&course, (1000.0, -100.0), (0.0, -20.0));
        assert_eq!(state, BallState::OutOfBounds);
    }

    /// Substepping is what stops a fast ball from stepping over a wall.
    ///
    /// One pass per two pixels of travel (`num4 = Math.Max(1, ceil(num3 / 2f))`). A ball thrown at
    /// sixty pixels a tick covers nearly four tiles in one step, so a single-pass move would put
    /// it on the far side of a one-tile wall without ever touching it.
    #[test]
    fn a_fast_ball_cannot_step_over_a_wall() {
        let mut course = ground(1);
        // A wall three tiles high, one tile thick, forty tiles along.
        for y in 97..100 {
            course.0.insert((40, y), Tile::block(1));
        }
        let start = (30.0 * TILE, 97.0 * TILE);
        let (at, _, _) = roll(&course, start, (60.0, 0.0));
        assert!(
            at.0 < 40.0 * TILE,
            "it should have been stopped by the wall at {}, ended at {}",
            40.0 * TILE,
            at.0
        );
    }

    /// Water slows a ball passing through it, and honey slows it harder
    /// (`GolfHelper.cs:141-148`, 0.91 against 0.8).
    #[test]
    fn a_ball_is_slowed_by_water_and_slowed_more_by_honey() {
        let through = |kind: terrustia_proto::tile::Liquid| {
            let mut course = Course(HashMap::new());
            for x in -20..200 {
                for y in 90..100 {
                    let mut tile = Tile::AIR;
                    tile.liquid = 255;
                    tile.liquid_kind = kind;
                    course.0.insert((x, y), tile);
                }
            }
            let mut at = (1000.0, 95.0 * TILE);
            let mut v = (10.0, 0.0);
            let mut spin = 0.0;
            for _ in 0..10 {
                step_ball(&mut at, &mut v, &mut spin, BALL, &course);
            }
            v.0
        };
        let dry = {
            let course = Course(HashMap::new());
            let mut at = (1000.0, 95.0 * TILE);
            let mut v = (10.0, 0.0);
            let mut spin = 0.0;
            for _ in 0..10 {
                step_ball(&mut at, &mut v, &mut spin, BALL, &course);
            }
            v.0
        };
        let wet = through(terrustia_proto::tile::Liquid::Water);
        let sticky = through(terrustia_proto::tile::Liquid::Honey);
        assert!(
            wet < dry,
            "water should slow it: {wet} against {dry} in air"
        );
        assert!(sticky < wet, "and honey harder: {sticky} against {wet}");
    }
}
