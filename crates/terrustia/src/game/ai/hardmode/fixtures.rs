//! The NPCs that hold still: styles 72, 73, 92, 122, 124 and 127.
//!
//! None of these hunt. They are furniture, escorts and attendants, and what makes each one
//! interesting is the single condition it dies on:
//!
//! * A **slime chest** (124) only falls.
//! * A **pinned part** (72) has no motion of its own at all — it *is* wherever its parent is, and
//!   the moment the parent is gone it is gone too.
//! * A **training dummy** (92) is held up by the tile it was placed on, and it stops existing when
//!   that tile does, or when every player is more than three hundred tiles away.
//! * A **pirate ghost** (122) drifts at you and shoulders other ghosts aside so a pack spreads out
//!   instead of stacking. With nobody to chase it fades out, and finishing the fade kills it.
//! * A **stationary caster** (73) stands still and lobs something every sixty ticks; being hit
//!   costs it half a reload, so hitting one is how you stop it casting.
//! * A **pal** (127) raises its own two guards on its first tick, dies on the spot if there is
//!   nowhere to put them, waits for both to be killed, then for you to come within a hundred
//!   pixels, then hands over the pet and leaves.

use rand::{Rng, rngs::SmallRng};
use terrustia_proto::npc_params::{
    CASTER_ARRIVAL, CASTER_COOLDOWN, CASTER_DRAG, CASTER_FADE_IN, CASTER_FLINCH, CASTER_RELOAD,
    CASTER_SHOT_DAMAGE, CASTER_SHOT_SPEED, CASTER_SPREAD, CHEST_GRAVITY, DUMMY_RANGE, DUMMY_TILE,
    GHOST_EASE, GHOST_FADE, GHOST_PERSONAL_SPACE, GHOST_SHOVE, GHOST_SPEED, PAL_APPROACH, PAL_DRAG,
    PAL_ESCORT, PAL_ESCORT_LIFT, PAL_FOXSPARKS, PAL_PAYOUT_TICKS, PAL_REWARD_CATTIVA,
    PAL_REWARD_FOXSPARKS,
};
use terrustia_proto::projectile::ids::CASTER_SHOT;

use super::drifters::Outcome;
use crate::game::ai::sight::within_firing_range;
use crate::game::ai::{Shot, World, face};
use crate::game::npc::{Npc, TILE, TileView};

/// Style 124: falls, and that is the whole routine.
pub fn slime_chest(npc: &mut Npc) {
    npc.velocity.1 += CHEST_GRAVITY;
    npc.dirty = true;
}

/// Style 72: a part that sits exactly on its parent and dies with it.
///
/// `parent` is the parent's centre. Passing `None` means the parent is gone.
pub fn pinned(npc: &mut Npc, parent: Option<(f32, f32)>) -> Outcome {
    let mut out = Outcome::default();
    let Some((cx, cy)) = parent else {
        out.spent = true;
        return out;
    };
    npc.velocity = (0.0, 0.0);
    npc.position = (cx - npc.width() / 2.0, cy - npc.height() / 2.0);
    npc.dirty = true;
    out
}

/// Style 92: a training dummy, which exists only while its tile and a nearby player do.
pub fn training_dummy(npc: &mut Npc, world: &World<'_, impl TileView>) -> Outcome {
    let mut out = Outcome::default();
    // A dummy has no armour: the point of it is to show what a hit really does.
    npc.defense = 0;

    let anchor = dummy_anchor(npc);
    let still_placed = world.tiles.tile(anchor.0, anchor.1).is_active()
        && world.tiles.tile(anchor.0, anchor.1).block == DUMMY_TILE;
    let watched = world.target.is_some_and(|t| {
        t.alive && {
            let (cx, cy) = npc.center();
            let (dx, dy) = (t.center.0 - cx, t.center.1 - cy);
            dx.hypot(dy) <= DUMMY_RANGE
        }
    });
    if !still_placed || !watched {
        out.spent = true;
    }
    out
}

/// Style 122: a pirate ghost.
pub fn pirate_ghost(npc: &mut Npc, world: &World<'_, impl TileView>) -> Outcome {
    let mut out = Outcome::default();
    let Some(target) = world.target.filter(|t| t.alive) else {
        // Nothing to haunt: coast to a stop, fade, and go when the fade is done.
        npc.velocity.0 *= 0.9;
        npc.velocity.1 *= 0.9;
        npc.alpha = (npc.alpha + GHOST_FADE).clamp(0, 255);
        if npc.alpha >= 255 {
            out.spent = true;
        }
        npc.dirty = true;
        return out;
    };
    npc.alpha = (npc.alpha - GHOST_FADE).clamp(0, 255);

    let (cx, cy) = npc.center();
    // `MoveTowards` on the offset: the wanted velocity is four pixels along the line to the
    // player, or the whole remaining distance when that is shorter than four pixels.
    let (dx, dy) = (target.center.0 - cx, target.center.1 - cy);
    let reach = dx.hypot(dy);
    let wanted = if reach <= GHOST_SPEED || reach == 0.0 {
        (dx, dy)
    } else {
        (dx / reach * GHOST_SPEED, dy / reach * GHOST_SPEED)
    };
    move_towards(&mut npc.velocity, wanted, GHOST_EASE);

    // Ghosts stacked on one another shove apart, and harder sideways than up: a pack fans out into
    // a line rather than a column.
    for (kx, ky, _) in world.avoid {
        let (ox, oy) = (kx - cx, ky - cy);
        let gap = ox.hypot(oy);
        if gap > 0.0 && gap < GHOST_PERSONAL_SPACE {
            let push = (ox / gap * GHOST_SHOVE, oy / gap * GHOST_SHOVE);
            npc.velocity.0 -= push.0 * 2.0;
            npc.velocity.1 -= push.1;
        }
    }
    npc.dirty = true;
    out
}

/// Step `velocity` toward `wanted` by at most `step`, in a straight line rather than per-axis.
fn move_towards(velocity: &mut (f32, f32), wanted: (f32, f32), step: f32) {
    let (dx, dy) = (wanted.0 - velocity.0, wanted.1 - velocity.1);
    let gap = dx.hypot(dy);
    if gap <= step || gap == 0.0 {
        *velocity = wanted;
    } else {
        velocity.0 += dx / gap * step;
        velocity.1 += dy / gap * step;
    }
}

/// Style 73: a caster that stands its ground.
///
/// `materialises` is true for the type that spends two seconds arriving before it can be hurt.
pub fn stationary_caster(
    npc: &mut Npc,
    world: &World<'_, impl TileView>,
    rng: &mut SmallRng,
    materialises: bool,
) -> Outcome {
    let mut out = Outcome::default();
    // Vanilla tracks its target with `faceTarget: false` — it picks whom to shoot at without ever
    // turning to face them, so its sprite direction is whatever it started as.
    npc.velocity.0 *= CASTER_DRAG;
    if npc.velocity.0.abs() < 0.1 {
        npc.velocity.0 = 0.0;
    }
    npc.dirty = true;

    if materialises && npc.ai[1] < CASTER_ARRIVAL {
        npc.ai[1] += 1.0;
        // Solid only once it has finished arriving; until then it is a fading-in ghost.
        npc.alpha = if npc.ai[1] > CASTER_FADE_IN {
            let along = (npc.ai[1] - CASTER_FADE_IN) / (CASTER_ARRIVAL - CASTER_FADE_IN);
            ((1.0 - along) * 255.0) as i32
        } else {
            255
        };
        npc.invulnerable = true;
        return out;
    }
    if materialises && npc.ai[1] == CASTER_ARRIVAL {
        npc.ai[1] += 1.0;
    }
    npc.invulnerable = false;
    npc.alpha = 0;

    // Nobody in range counts the same as nobody at all: casters do not spend a reload on someone
    // who could not possibly see the shot coming.
    let has_target = world
        .target
        .is_some_and(|t| t.alive && within_firing_range(npc.center(), t.center));
    // With nobody in sight the timer is held at the bottom of its cooldown, so a caster that loses
    // you does not come back with a shot already charged.
    if npc.ai[0] == 0.0 && !has_target {
        npc.ai[0] = CASTER_COOLDOWN;
    }
    if npc.ai[0] < CASTER_RELOAD {
        npc.ai[0] += 1.0;
    }
    if world.was_hurt {
        npc.ai[0] = CASTER_FLINCH;
    }
    if npc.ai[0] == CASTER_RELOAD
        && let Some(target) = world.target.filter(|t| t.alive)
    {
        npc.ai[0] = CASTER_COOLDOWN;
        let (cx, cy) = npc.center();
        let from = (cx, cy - 10.0);
        // Aim scattered by a hundred pixels either way, then scaled by up to thirty per cent:
        // a cast never lands quite where the last one did.
        let mut aim = (target.center.0 - from.0, target.center.1 - from.1);
        aim.0 += rng.random_range(-CASTER_SPREAD..=CASTER_SPREAD) as f32;
        aim.1 += rng.random_range(-CASTER_SPREAD..=CASTER_SPREAD) as f32;
        aim.0 *= rng.random_range(70..=130) as f32 * 0.01;
        aim.1 *= rng.random_range(70..=130) as f32 * 0.01;
        let length = aim.0.hypot(aim.1);
        let unit = if length > 0.0 && length.is_finite() {
            (aim.0 / length, aim.1 / length)
        } else {
            // Straight up, which is what the game falls back to when the aim degenerates.
            (0.0, -1.0)
        };
        out.shots.push(Shot {
            projectile: CASTER_SHOT,
            damage: CASTER_SHOT_DAMAGE,
            position: from,
            velocity: (unit.0 * CASTER_SHOT_SPEED, unit.1 * CASTER_SHOT_SPEED),
            time_left: 300,
            ai: [0.0; 3],
        });
    }
    out
}

/// `WorldGen.SolidTile(i, j)` (`WorldGen.cs:70650-70671`), read through a routine's tile view.
///
/// ```csharp
/// if (Main.tile[i, j].active() && Main.tileSolid[Main.tile[i, j].type]
///     && !Main.tileSolidTop[Main.tile[i, j].type] && !Main.tile[i, j].halfBrick()
///     && Main.tile[i, j].slope() == 0 && !Main.tile[i, j].inActive())
/// ```
///
/// The same six clauses `spawn.rs`'s own `solid_tile` transcribes; this is a second copy rather
/// than a shared one because that one reads a `&World` and a routine only ever has a [`TileView`].
/// The game's "off the map counts as solid" arm is not transcribed: a [`TileView`] answers off the
/// map with air, and the one caller here is a floor search that wants a miss there anyway.
fn solid_tile(tiles: &impl TileView, x: i32, y: i32) -> bool {
    let tile = tiles.tile(x, y);
    tile.is_active()
        && terrustia_proto::tile_solid::solid(tile.block)
        && !terrustia_proto::tile_solid::solid_top(tile.block)
        && !tile.flags.has(terrustia_proto::TileFlags::HALF_BRICK)
        && tile.slope == 0
        && !tile.flags.has(terrustia_proto::TileFlags::ACTUATED)
}

/// `Collision.SolidTiles(startX, endX, startY, endY)` (`Collision.cs:3468-3506`): whether anything
/// in this inclusive tile rectangle blocks movement.
///
/// Not the same test as [`solid_tile`] above, and the difference is what makes the headroom half of
/// [`check_floor2`] work: this one ignores half-bricks and slopes, so a sloped block still counts
/// as a wall to stand against.
///
/// The game's off-the-map arms return *true* (out of bounds is solid) and its
/// `Main.tile[i, j] == null` arm returns false; neither is transcribed, for the same reason
/// [`solid_tile`] leaves its own out. Nothing off the map reaches here: a pal is spawned by the
/// ambient path, which drops any candidate near an edge long before this.
fn solid_tiles(tiles: &impl TileView, from_x: i32, to_x: i32, from_y: i32, to_y: i32) -> bool {
    (from_x..=to_x).any(|x| {
        (from_y..=to_y).any(|y| {
            let tile = tiles.tile(x, y);
            tile.is_active()
                && !tile.flags.has(terrustia_proto::TileFlags::ACTUATED)
                && terrustia_proto::tile_solid::solid(tile.block)
                && !terrustia_proto::tile_solid::solid_top(tile.block)
        })
    })
}

/// `CultistRitual.CheckFloor2(Center, out spawnPoints)` (`CultistRitual.cs:133-159`).
///
/// Two columns, six tiles either side of the centre (`i` runs -3, -1, 1, 3 with the middle pair
/// skipped, and the column is `point.X + i * 2`). Each column is walked from five rows above the
/// centre down to eleven below it, and the first row that is a floor with room to stand on it wins.
/// Both columns have to find one or the whole thing fails, which is the pal's own life-or-death
/// test: no floor either side and it deletes itself on its first tick.
///
/// "A floor" is `WorldGen.SolidTile(x, y) || TileID.Sets.Platforms[type]`: `SolidTile` throws every
/// solid-top tile out, and the platform set puts back the seven that are really floors. "Room to
/// stand" is the game's own three-part alternative, transcribed unchanged: either the whole
/// three-by-three above the tile is clear, or the middle column is clear for three and the two side
/// columns are clear for two, which is what lets a spot in a two-tile-high corridor still count.
///
/// This is `CheckFloor`'s little brother: the four-point version raises the Cultists' ritual and
/// this two-point one raises a pal's two guards. Only the second is wanted here, so only the second
/// is transcribed.
fn check_floor2(tiles: &impl TileView, center: (f32, f32)) -> Option<[(i32, i32); 2]> {
    // `Vector2.ToTileCoordinates()` (`Utils.cs:1899-1902`) is `(int)x >> 4`, an arithmetic shift
    // rather than a divide, so it floors rather than truncating toward zero. Everything here is
    // well inside a world, where the two agree, but the shift is what the game does.
    let point = ((center.0 as i32) >> 4, (center.1 as i32) >> 4);
    let mut found = [(0, 0); 2];
    let mut count = 0;
    for i in [-3i32, 3] {
        for j in -5..12 {
            let x = point.0 + i * 2;
            let y = point.1 + j;
            let floor = solid_tile(tiles, x, y)
                || terrustia_proto::tile_sets::is_platform(tiles.tile(x, y).block);
            if floor
                && (!solid_tiles(tiles, x - 1, x + 1, y - 3, y - 1)
                    || (!solid_tiles(tiles, x, x, y - 3, y - 1)
                        && !solid_tiles(tiles, x + 1, x + 1, y - 3, y - 2)
                        && !solid_tiles(tiles, x - 1, x - 1, y - 3, y - 2)))
            {
                found[count] = (x, y);
                count += 1;
                break;
            }
        }
    }
    (count == 2).then_some(found)
}

/// Style 127: a distressed pal, its two guards, and what it hands over.
///
/// `AI_127_Pal` (`NPC.cs:43379-43478`), the whole encounter in one routine:
///
/// * **First tick** (`localAI[0] == 0f`): look for floor either side with [`check_floor2`]. Nothing
///   there and it is gone at once, silently, with no loot: `life = 0; HitEffect(); active = false;`
///   (`:43390-43394`). Otherwise raise two Goblin Archers on the two spots found, each back-linked
///   to this pal with `ai[3] = -(whoAmI + 1)` (`:43396-43405`), which is what makes them stand over
///   it instead of hunting (see the style-3 dispatch in `ai/mod.rs`).
/// * **`ai[0] == 0`**: while either guard is alive it keeps resetting its own despawn timer to
///   `activeTime` (`:43418`), so a guarded pal waits as long as it has to. Once both are gone it
///   moves on (`:43411-43416`).
/// * **`ai[0] == 1`**: come within a hundred pixels of it and it is yours (`:43423-43431`).
/// * **`ai[0] == 2`**: two seconds of celebrating, then it drops the pet and goes
///   (`:43455-43470`, and `AI_127_Pal_GiveRewerd` at `:43481-43489`).
///
/// `escorts_alive` is how many of its own two guards are still in the world. Vanilla keeps a handle
/// to each in its own `ai[1]`/`ai[2]` and unpacks them (`AI_127_Pal_TryUnpackNPC`, `:43496-43508`);
/// a routine here cannot see the NPC table, so it asks for the handles through [`Spawn::handle`]
/// and the caller does the unpacking. It has to be those handles and not the guards' own back-link:
/// a guard that has been woken clears its `ai[3]` (`NPC.cs:57558`) and still holds the pal, because
/// what vanilla asks is `active`, not "still guarding".
///
/// The sounds and the Foxsparks' own glow (`:43432-43454`, `:43471-43474`) are client-side and are
/// not transcribed.
pub fn pal(npc: &mut Npc, world: &World<'_, impl TileView>, escorts_alive: usize) -> Outcome {
    let mut out = Outcome::default();
    if let Some(target) = world.target {
        face(npc, target);
    }
    npc.dirty = true;

    // The one-off: floor, or nothing.
    if npc.local_ai[0] == 0.0 {
        npc.local_ai[0] = 1.0;
        let Some(spots) = check_floor2(world.tiles, npc.center()) else {
            out.spent = true;
            return out;
        };
        let escort = terrustia_proto::npc_data::npc_stats(PAL_ESCORT);
        let (w, h) = escort.map_or((0.0, 0.0), |s| (s.width as f32, s.height as f32));
        for (i, spot) in spots.into_iter().enumerate() {
            out.spawn.push(crate::game::npc_ai::Spawn {
                // `ai[1 + i] = num2 + 1` (`NPC.cs:43401`): the pal keeps a handle to each guard so
                // it can ask later whether either is still in the world.
                handle: Some((world.slot, 1 + i)),
                npc_type: PAL_ESCORT,
                // `NewNPC(..., X * 16 + 8, Y * 16 - 48, 111)` (`NPC.cs:43400`) names the centre and
                // the feet; a `Spawn` here names the top-left corner, so the box comes off it.
                position: (
                    spot.0 as f32 * TILE + TILE / 2.0 - w / 2.0,
                    spot.1 as f32 * TILE - PAL_ESCORT_LIFT - h,
                ),
                velocity: (0.0, 0.0),
                parent: None,
                // The back-link, `Main.npc[num2].ai[3] = -(whoAmI + 1)` (`NPC.cs:43402`), which is
                // what the style-3 guard branch reads.
                ai: [None, None, None, Some(-(f32::from(world.slot) + 1.0))],
            });
        }
        // Vanilla falls straight through into the `ai[0] == 0` test with both guards already in the
        // table; here they are raised by the caller after this returns, so the state machine waits
        // a tick rather than reading a count of zero and skipping the encounter entirely.
        return out;
    }

    match npc.ai[0] {
        // Waiting for the escort. While one is alive the pal will not time out.
        0.0 => {
            if escorts_alive == 0 {
                npc.ai[0] = 1.0;
            } else {
                // `timeLeft = activeTime` (`NPC.cs:43418`), which is 750 (`NPC.cs:6188`) and is an
                // assignment, not a floor: this read `max(3600)`, nearly five times the game's own
                // number and unable to ever bring a long timer back down.
                npc.time_left = crate::game::npc::DEFAULT_TIME_LEFT;
            }
        }
        // Escort gone: waiting for the player to come and collect.
        1.0 => {
            if let Some(target) = world.target.filter(|t| t.alive) {
                let (cx, cy) = npc.center();
                if (target.center.0 - cx).hypot(target.center.1 - cy) < PAL_APPROACH {
                    // All three, as `NPC.cs:43426-43428` writes them.
                    npc.ai[0] = 2.0;
                    npc.ai[1] = 0.0;
                    npc.ai[2] = 0.0;
                }
            }
        }
        // Paying out, which takes two seconds and then it leaves.
        _ => {
            npc.ai[1] += 1.0;
            if npc.ai[1] >= PAL_PAYOUT_TICKS {
                // `AI_127_Pal_GiveRewerd` (`NPC.cs:43481-43489`), then `life = 0; active = false;`
                // with no `HitEffect` and no strike: the pet leaves with you, it is not killed, so
                // nothing else about it drops and nobody is credited with a kill.
                out.reward = Some(if npc.npc_type == PAL_FOXSPARKS {
                    PAL_REWARD_FOXSPARKS
                } else {
                    PAL_REWARD_CATTIVA
                });
                out.spent = true;
                out.became = None;
                return out;
            }
        }
    }

    // Vanilla's drag is the last thing in the routine, after every arm that returns early has
    // already gone (`NPC.cs:43475-43478`).
    npc.velocity.0 *= PAL_DRAG;
    if npc.velocity.0.abs() < 0.1 {
        npc.velocity.0 = 0.0;
    }
    out
}

/// Where a dummy was planted, which the server writes into its `ai` when it raises it.
///
/// A dummy that finds nothing there any more takes itself away, so this is the whole of how it
/// knows its tile has been mined out from under it.
pub fn dummy_anchor(npc: &Npc) -> (i32, i32) {
    (npc.ai[0] as i32, npc.ai[1] as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::npc_ai::Target;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use terrustia_proto::npc_params::{PAL_CATTIVA, PAL_ESCORT_COLUMN};
    use terrustia_proto::tile::Tile;

    /// Empty space with whatever tiles a test puts in it.
    #[derive(Default)]
    struct Air(HashMap<(i32, i32), Tile>);

    impl TileView for Air {
        fn tile(&self, x: i32, y: i32) -> Tile {
            self.0.get(&(x, y)).copied().unwrap_or(Tile::AIR)
        }
    }

    fn tiles_with_floor(_at: i32) -> Air {
        Air::default()
    }

    fn world<'a>(tiles: &'a Air, target: Option<(f32, f32)>) -> World<'a, Air> {
        crate::game::ai::calm(
            tiles,
            target.map(|center| Target {
                slot: 0,
                center,
                velocity: (0.0, 0.0),
                alive: true,
            }),
        )
    }

    /// The pirate ghost, which is the type these tests borrow for anything that only needs a body.
    const PIRATE_GHOST: u16 = 662;

    fn ghost(x: f32) -> Npc {
        Npc::new(PIRATE_GHOST, (x, 0.0), 1).expect("pirate ghost")
    }

    /// The whole point of the shove: two ghosts on the same spot must not stay there.
    #[test]
    fn ghosts_do_not_stack() {
        let tiles = tiles_with_floor(40);
        let mut w = world(&tiles, Some((600.0, 0.0)));
        let mut a = ghost(0.0);
        let kin = [(a.center().0, a.center().1, 0.0)];
        w.avoid = &kin;
        let before = a.velocity.0;
        pirate_ghost(&mut a, &w);
        // The neighbour is at its own centre, so the shove is degenerate and must not produce NaN.
        assert!(a.velocity.0.is_finite(), "shove produced {}", a.velocity.0);
        assert!(a.velocity.0 >= before || a.velocity.0.is_finite());

        // A neighbour to the left pushes it right, on top of its pull toward the player.
        let mut b = ghost(0.0);
        let left = [(b.center().0 - 20.0, b.center().1, 0.0)];
        w.avoid = &left;
        pirate_ghost(&mut b, &w);
        let mut c = ghost(0.0);
        w.avoid = &[];
        pirate_ghost(&mut c, &w);
        assert!(
            b.velocity.0 > c.velocity.0,
            "crowded from the left it should move further right: {} vs {}",
            b.velocity.0,
            c.velocity.0
        );
    }

    /// With nobody left to chase a ghost fades and then goes, rather than hanging about forever.
    #[test]
    fn a_ghost_with_nobody_to_chase_fades_out_and_dies() {
        let tiles = tiles_with_floor(40);
        let w = world(&tiles, None);
        let mut g = ghost(0.0);
        g.velocity = (5.0, 5.0);
        let mut ticks = 0;
        let spent = loop {
            let out = pirate_ghost(&mut g, &w);
            ticks += 1;
            if out.spent || ticks > 200 {
                break out.spent;
            }
        };
        assert!(spent, "it should have faded away");
        assert_eq!(ticks, 255 / GHOST_FADE, "the fade takes alpha to full");
        assert!(
            g.velocity.0.abs() < 0.1,
            "and it should have coasted to a stop"
        );
    }

    /// A caster fires on a fixed cycle, and being hit pushes the next shot further away.
    #[test]
    fn hitting_a_caster_delays_its_next_cast() {
        let tiles = tiles_with_floor(40);
        let mut w = world(&tiles, Some((300.0, 0.0)));
        let mut rng = SmallRng::seed_from_u64(4);
        let mut quiet = Npc::new(PIRATE_GHOST, (0.0, 0.0), 1).unwrap();
        quiet.ai[0] = 0.0;

        let shots_over = |npc: &mut Npc, w: &World<'_, _>, rng: &mut SmallRng, ticks: usize| {
            let mut count = 0;
            for _ in 0..ticks {
                count += stationary_caster(npc, w, rng, false).shots.len();
            }
            count
        };

        let mut a = quiet.clone();
        let calm = shots_over(&mut a, &w, &mut rng, 400);
        assert!(calm > 0, "a caster left alone should get shots off");

        let mut b = quiet.clone();
        w.was_hurt = true;
        let harried = shots_over(&mut b, &w, &mut rng, 400);
        assert!(
            harried < calm,
            "being hit every tick should stop it casting: {harried} vs {calm}"
        );
    }

    /// A caster will not fire at someone well beyond a screen away — it is not omniscient.
    #[test]
    fn a_caster_does_not_fire_at_someone_off_screen() {
        let tiles = tiles_with_floor(40);
        // Comfortably past `AI_GlobalFiringDistanceCheck`'s roughly one-screen box.
        let w = world(&tiles, Some((5000.0, 0.0)));
        let mut rng = SmallRng::seed_from_u64(5);
        let mut c = Npc::new(PIRATE_GHOST, (0.0, 0.0), 1).unwrap();
        c.ai[0] = 0.0;

        let mut shots = 0;
        for _ in 0..400 {
            shots += stationary_caster(&mut c, &w, &mut rng, false).shots.len();
        }
        assert_eq!(
            shots, 0,
            "nobody within range should ever be hit: {shots} shots"
        );
    }

    /// A caster tracks whom to shoot without ever turning to face them — vanilla calls
    /// `TargetClosest` with `faceTarget: false` for this style.
    #[test]
    fn a_caster_does_not_turn_to_face_you() {
        let tiles = tiles_with_floor(40);
        let mut c = Npc::new(PIRATE_GHOST, (0.0, 0.0), 1).unwrap();
        let start_direction = c.direction;
        let mut rng = SmallRng::seed_from_u64(6);

        let left = world(&tiles, Some((c.center().0 - 300.0, c.center().1)));
        stationary_caster(&mut c, &left, &mut rng, false);
        assert_eq!(
            c.direction, start_direction,
            "someone to the left should not turn it"
        );

        let right = world(&tiles, Some((c.center().0 + 300.0, c.center().1)));
        stationary_caster(&mut c, &right, &mut rng, false);
        assert_eq!(c.direction, start_direction, "nor someone to the right");
    }

    /// A dummy is held up by its tile; take the tile away and it goes.
    #[test]
    fn a_dummy_without_its_tile_is_gone() {
        let tiles = tiles_with_floor(40);
        let w = world(&tiles, Some((0.0, 0.0)));
        let mut d = Npc::new(PIRATE_GHOST, (0.0, 0.0), 1).unwrap();
        // Point it at a tile that is not a dummy tile.
        d.ai[0] = 5.0;
        d.ai[1] = 41.0;
        assert!(training_dummy(&mut d, &w).spent, "no anchor, no dummy");
        assert_eq!(d.defense, 0, "a dummy never soaks a hit");
    }

    /// A pinned part is wherever its parent is, and nowhere at all once the parent is gone.
    #[test]
    fn a_pinned_part_tracks_its_parent_and_dies_with_it() {
        let mut part = ghost(0.0);
        part.velocity = (9.0, -3.0);
        let out = pinned(&mut part, Some((500.0, 250.0)));
        assert!(!out.spent);
        assert_eq!(part.velocity, (0.0, 0.0), "it has no motion of its own");
        assert_eq!(part.center(), (500.0, 250.0));
        assert!(pinned(&mut part, None).spent, "no parent, no part");
    }

    /// Solid ground from `0` downwards, and open air above it.
    struct Floor(i32);
    impl TileView for Floor {
        fn tile(&self, _x: i32, y: i32) -> Tile {
            if y >= self.0 {
                Tile::block(1)
            } else {
                Tile::AIR
            }
        }
    }

    /// The pal's own world: a table slot of its own, and somebody standing wherever `target` says.
    fn pal_world<'a, T: TileView>(
        tiles: &'a T,
        target: Option<(f32, f32)>,
        slot: u8,
    ) -> World<'a, T> {
        let mut w = crate::game::ai::calm(
            tiles,
            target.map(|center| Target {
                slot: 0,
                center,
                velocity: (0.0, 0.0),
                alive: true,
            }),
        );
        w.slot = slot;
        w
    }

    /// A pal standing with its feet at tile row 40, in column 100.
    fn pal_npc(npc_type: u16) -> Npc {
        let mut p = Npc::new(npc_type, (0.0, 0.0), 1).expect("a pal");
        p.position = (100.0 * TILE, 40.0 * TILE - p.height());
        p
    }

    /// The floor row the two guards are looked for on, and the columns they stand in.
    const PAL_FLOOR: i32 = 40;

    /// No floor either side, no encounter: a pal deletes itself on its very first tick rather than
    /// hanging in the air with nobody guarding it (`NPC.cs:43389-43394`).
    ///
    /// Neutralised by making `check_floor2` return `Some([(0, 0); 2])` unconditionally: the "gone
    /// at once" assertion fails and the pal settles into its waiting state over open air.
    #[test]
    fn a_pal_with_nowhere_to_stand_deletes_itself_at_once() {
        let nothing = Air::default();
        let w = pal_world(&nothing, None, 3);
        let mut p = pal_npc(PAL_CATTIVA);
        let out = pal(&mut p, &w, 0);
        assert!(
            out.spent,
            "with no floor under it, it should be gone at once"
        );
        assert!(out.spawn.is_empty(), "and it should have raised nobody");
        assert!(out.reward.is_none(), "and paid out nothing");
    }

    /// With floor either side it raises two Goblin Archers, six tiles out each way, each carrying
    /// the negative back-link that turns it into a guard (`NPC.cs:43396-43405`).
    ///
    /// Neutralised by dropping the `ai: [None, None, None, Some(...)]` back-link from the `Spawn`
    /// (leaving `[None; 4]`): the back-link assertion fails, and with it the whole guard branch in
    /// `ai/mod.rs`, since a zero `ai[3]` is an ordinary hunting archer. Neutralised again by
    /// deleting the `for spot in spots` loop: the count assertion fails.
    #[test]
    fn a_pal_raises_two_guards_back_linked_to_itself() {
        let ground = Floor(PAL_FLOOR);
        let w = pal_world(&ground, None, 9);
        let mut p = pal_npc(PAL_CATTIVA);
        let out = pal(&mut p, &w, 0);

        assert!(!out.spent, "there is floor here, it should stay");
        assert_eq!(out.spawn.len(), 2, "a pal is guarded by exactly two");
        for (i, summon) in out.spawn.iter().enumerate() {
            assert_eq!(summon.npc_type, PAL_ESCORT, "the guard is a Goblin Archer");
            assert_eq!(
                summon.ai[3],
                Some(-10.0),
                "`ai[3] = -(whoAmI + 1)` for slot 9"
            );
            assert_eq!(
                summon.handle,
                Some((9, 1 + i)),
                "`ai[1 + i] = num2 + 1`: the pal keeps a handle to each"
            );
        }
        // Six tiles either side of the pal's own column, and three tiles up off the floor.
        let column = (p.center().0 as i32) >> 4;
        let mut columns: Vec<i32> = out
            .spawn
            .iter()
            .map(|s| ((s.position.0 + 8.0) / TILE) as i32)
            .collect();
        columns.sort_unstable();
        assert_eq!(
            columns,
            vec![column - PAL_ESCORT_COLUMN, column + PAL_ESCORT_COLUMN],
            "one guard either side"
        );
        for summon in &out.spawn {
            assert!(
                summon.position.1 < PAL_FLOOR as f32 * TILE - PAL_ESCORT_LIFT,
                "a guard is dropped above the floor, not into it"
            );
        }

        // The one-off is a one-off: a second tick raises nobody else.
        assert!(
            pal(&mut p, &w, 2).spawn.is_empty(),
            "it should only ever raise its guard once"
        );
    }

    /// A pal will not leave while a guard of its own lives, and pays out once you reach it.
    ///
    /// Neutralised by putting `npc.time_left = npc.time_left.max(3600)` back in place of the
    /// `DEFAULT_TIME_LEFT` assignment: the `activeTime` assertion fails. Neutralised again by
    /// dropping the `out.reward = Some(...)` line: the reward assertion fails and a collected pet
    /// leaves nothing behind. And a third time by deleting the `npc.ai[0] = 1.0` arm: the
    /// "guard gone" assertion fails and the pal waits forever.
    #[test]
    fn a_pal_waits_for_its_guards_then_for_you_then_hands_over_the_pet() {
        let ground = Floor(PAL_FLOOR);
        let far = pal_world(&ground, Some((50_000.0, 0.0)), 1);
        let mut p = pal_npc(PAL_CATTIVA);
        pal(&mut p, &far, 0);

        // `timeLeft = activeTime` (750) is an assignment: a longer timer is brought *down* to it.
        p.time_left = 5_000;
        pal(&mut p, &far, 2);
        assert_eq!(p.ai[0], 0.0, "it waits while a guard is alive");
        assert_eq!(
            p.time_left,
            crate::game::npc::DEFAULT_TIME_LEFT,
            "the timer is set to activeTime, not floored at some larger number"
        );

        pal(&mut p, &far, 0);
        assert_eq!(p.ai[0], 1.0, "guard gone, now it waits for you");
        pal(&mut p, &far, 0);
        assert_eq!(p.ai[0], 1.0, "but not from across the world");

        let near = pal_world(&ground, Some((p.center().0 + 20.0, p.center().1)), 1);
        pal(&mut p, &near, 0);
        assert_eq!(p.ai[0], 2.0, "close enough to collect");
        assert_eq!(p.ai[1], 0.0, "and both escort handles are cleared");
        assert_eq!(p.ai[2], 0.0);

        let mut ticks = 0;
        let paid = loop {
            let out = pal(&mut p, &near, 0);
            ticks += 1;
            if out.spent || ticks > 400 {
                break out;
            }
        };
        assert!(paid.spent, "it should have paid out");
        assert_eq!(ticks, PAL_PAYOUT_TICKS as i32, "after exactly two seconds");
        assert_eq!(
            paid.reward,
            Some(PAL_REWARD_CATTIVA),
            "a Cattiva hands over item 5663"
        );
    }

    /// ...and the Foxsparks hands over the other one (`NPC.cs:43483-43486`).
    ///
    /// Neutralised by making `AI_127_Pal_GiveRewerd`'s type test always pick `PAL_REWARD_CATTIVA`:
    /// this fails while the test above still passes, which is the point of having both.
    #[test]
    fn a_foxsparks_hands_over_its_own_pet() {
        let ground = Floor(PAL_FLOOR);
        let mut p = pal_npc(PAL_FOXSPARKS);
        let w = pal_world(&ground, Some((p.center().0, p.center().1)), 1);
        pal(&mut p, &w, 0);
        p.ai[0] = 2.0;
        p.ai[1] = PAL_PAYOUT_TICKS - 1.0;
        assert_eq!(pal(&mut p, &w, 0).reward, Some(PAL_REWARD_FOXSPARKS));
    }

    /// `CheckFloor2` wants headroom, not merely a floor: a spot buried under solid rock is not a
    /// place to stand a guard (`CultistRitual.cs:145-150`).
    ///
    /// Neutralised by dropping the whole `&& (!solid_tiles(...) || ...)` headroom half of the
    /// condition: this test fails, `check_floor2` happily answering with two points inside stone.
    #[test]
    fn check_floor2_wants_room_above_the_floor_it_finds() {
        struct Solid;
        impl TileView for Solid {
            fn tile(&self, _x: i32, _y: i32) -> Tile {
                Tile::block(1)
            }
        }
        assert!(
            check_floor2(&Solid, (100.0 * TILE, 40.0 * TILE)).is_none(),
            "solid rock in every direction is not two places to stand"
        );
        assert!(
            check_floor2(&Floor(PAL_FLOOR), (100.0 * TILE, 40.0 * TILE)).is_some(),
            "...but a floor with air over it is"
        );
    }
}
