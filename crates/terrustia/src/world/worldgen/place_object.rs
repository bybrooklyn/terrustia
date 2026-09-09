//! Placing a multi-tile object at generation time.
//!
//! Every one of the middle-tier passes — pots, statues, piles, fallen logs, and (later) buried
//! chests — is a siting loop around the same eight lines: find a spot, check it is clear, stamp
//! the frames. Terraria's own generator writes those eight lines out by hand every time it needs
//! them, because `TileObjectData` is a runtime table there, not something a generation pass reads
//! directly. Here it can be a function, because `terrustia-proto/src/tile_object.rs` already has
//! the complete `TileObjectData` transcription — 391 entries, with `frame_of` giving the top-left
//! cell's frame for a given style — so nothing here needs to know how any specific object's sheet
//! is laid out.
//!
//! This is the generation-time sibling of `GameServer::on_place_object`
//! (`crates/terrustia/src/game/server.rs`), which does the same per-tile frame arithmetic for a
//! player-placed object. The two are kept separate rather than shared, because the player path
//! has consequences this one does not — a broadcast to other clients, a chest record, a tile
//! entity — and a generation pass has none of those to get right or wrong.

use terrustia_proto::{Tile, tile_object, tile_solid};

use crate::world::World;

/// Place a multi-tile object anchored at `(x, y)`.
///
/// `(x, y)` is the *anchor* tile, in the same sense the client's placement packet uses one: the
/// tile the object's own `origin` maps back onto, not necessarily its top-left corner. A chest's
/// origin is `(0, 1)` — its bottom-left cell — so `place_object(world, x, y, 21, style, -1)`
/// anchors the chest with `(x, y)` as that corner, and the object's other three cells are worked
/// out from `width`/`height`/`origin` the same way `on_place_object` works them out from the
/// packet.
///
/// Refuses — writing nothing — when any cell of the footprint is already active, or when the row
/// immediately beneath the footprint is not entirely solid. Vanilla's individual `PlacePot`,
/// `Place2xX` and friends each run this same pair of checks inline before writing; this is that
/// check, done once.
///
/// `random` is the object's per-style variant, or `-1` for none — see [`tile_object::TileObject::frame_of`].
pub fn place_object(
    world: &mut World,
    x: i32,
    y: i32,
    block: u16,
    style: i32,
    random: i32,
) -> bool {
    let Some(object) = tile_object::tile_object(block) else {
        return false;
    };
    let (left, top) = (x - object.origin.0, y - object.origin.1);
    let (right, bottom) = (left + object.width - 1, top + object.height - 1);
    if !world.in_bounds(left, top) || !world.in_bounds(right, bottom) {
        return false;
    }

    // The whole footprint has to be empty — vanilla refuses the object entirely rather than
    // filling in whichever cells happen to be free.
    for dx in 0..object.width {
        for dy in 0..object.height {
            if world.tile(left + dx, top + dy).is_active() {
                return false;
            }
        }
    }
    // And the row directly beneath it has to be solid ground, all the way across, or the object
    // is left floating over whatever gap is under one corner of it. `is_active()` first: an
    // inactive tile's `block` still carries whatever type it last held (0 is Dirt's own id, and
    // Dirt is solid), so checking `solid(block)` alone reads empty air as solid ground.
    for dx in 0..object.width {
        let below = world.tile(left + dx, bottom + 1);
        if !below.is_active() || !tile_solid::solid(below.block) {
            return false;
        }
    }

    let (frame_x, frame_y) = object.frame_of(style, random);
    for dx in 0..object.width {
        let fx = frame_x + dx * (object.coord_width + object.padding);
        let mut fy = frame_y;
        for dy in 0..object.height {
            // Vanilla's own placers (`PlacePot`, `Place2xX`, and friends) only ever set
            // `active`/`type`/`frameX`/`frameY` on the tile that was already there — never
            // `wall` or `liquid`. Building from `Tile::framed` (which starts from `Tile::AIR`,
            // wall 0, no liquid) instead wiped whatever wall was lining the room and any liquid
            // sitting in it. Preserve both explicitly.
            let existing = world.tile(left + dx, top + dy);
            let mut tile = Tile::framed(block, fx as i16, fy as i16);
            tile.wall = existing.wall;
            tile.wall_color = existing.wall_color;
            tile.liquid = existing.liquid;
            tile.liquid_kind = existing.liquid_kind;
            world.set_tile(left + dx, top + dy, tile);
            fy += object.coord_heights.get(dy as usize).copied().unwrap_or(16) + object.padding;
        }
    }
    true
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_locked_dungeon_door_can_be_placed_on_brick() {
        let mut world = World::empty(200, 200, "door");
        for x in 90..110 {
            world.set_tile(x, 103, Tile::block(41));
        }
        let ok = place_object(&mut world, 100, 100, 10, 11, -1);
        let t = world.tile(100, 100);
        assert!(ok, "the door was refused; tile is {t:?}");
        assert_eq!(t.block, 10);
        assert_eq!(t.frame_y, 594, "style 11 should frame at 594");
        assert!(t.frame_x < 54, "and IsLockedDoor also wants frameX < 54");
    }
    use super::*;

    /// A flat, wide floor with nothing on it — every object in this module fits on it somewhere.
    fn floor(width: i32) -> World {
        let mut world = World::empty(width, 40, "place_object");
        for x in 0..width {
            world.set_tile(x, 30, Tile::block(1));
        }
        world
    }

    /// Placing a known object produces exactly the frames `tile_object`'s own arithmetic predicts
    /// — the whole point of building this on top of the existing table rather than a fresh one.
    #[test]
    fn a_placed_objects_frames_match_tile_object_directly() {
        // A statue: 2 wide, 3 tall, origin (1, 2) — anchored at the bottom-right cell.
        let object = tile_object::tile_object(105).expect("statues are in the table");
        let mut world = floor(40);
        assert!(place_object(&mut world, 20, 29, 105, 3, -1));

        let (base_x, base_y) = object.frame_of(3, -1);
        for dx in 0..object.width {
            for dy in 0..object.height {
                let tile = world.tile(20 - object.origin.0 + dx, 29 - object.origin.1 + dy);
                assert_eq!(tile.block, 105);
                let want_x = base_x + dx * (object.coord_width + object.padding);
                let mut want_y = base_y;
                for h in &object.coord_heights[..dy as usize] {
                    want_y += h + object.padding;
                }
                assert_eq!(tile.frame_x, want_x as i16, "column {dx}");
                assert_eq!(tile.frame_y, want_y as i16, "row {dy}");
            }
        }
    }

    /// Vanilla's own placers only ever touch `active`/`type`/`frameX`/`frameY` — never `wall` or
    /// `liquid`. Building the new tile from `Tile::framed` (which starts from `Tile::AIR`) instead
    /// wiped whatever wall lined the room and any liquid sitting in it, leaving a hole in the wall
    /// behind every pot/statue/pile/log/door/altar this module places. Fails on the pre-fix code
    /// (`after.wall == 0`, `after.liquid == 0`).
    #[test]
    fn placing_an_object_keeps_the_wall_and_liquid_already_behind_it() {
        let mut world = floor(40);
        // The top-left cell of a statue's footprint (anchored at (20, 29), origin (1, 2) ->
        // columns 19-20, rows 27-29) already has a wall and standing water behind it, matching a
        // real wall-lined room interior.
        let mut seeded = world.tile(19, 27);
        seeded.wall = 5;
        seeded.wall_color = 3;
        seeded.liquid = 120;
        seeded.liquid_kind = terrustia_proto::Liquid::Water;
        world.set_tile(19, 27, seeded);

        assert!(place_object(&mut world, 20, 29, 105, 3, -1));

        let after = world.tile(19, 27);
        assert_eq!(after.block, 105, "the object should still have been placed");
        assert_eq!(
            after.wall, 5,
            "placing an object must not erase the wall behind it"
        );
        assert_eq!(after.wall_color, 3, "wall_color must survive too");
        assert_eq!(
            after.liquid, 120,
            "placing an object must not erase liquid behind it"
        );
        assert_eq!(after.liquid_kind, terrustia_proto::Liquid::Water);
    }

    /// Every shape this module's callers actually use, placed once, checked for internal
    /// consistency: the anchor tile lands where `origin` says it should, and nothing is left
    /// with a frame of -1 (the "no frame" sentinel — see the doors/vines/cacti bugs found
    /// earlier this session, all the same root cause).
    #[test]
    fn every_object_this_module_places_is_fully_framed() {
        for &(block, style) in &[
            (21u16, 0i32), // chest
            (28, 5),       // pot
            (105, 10),     // statue
            (27, 2),       // sunflower
            (488, 0),      // fallen log
            (185, 1),      // small pile
            (186, 3),      // large pile
            (187, 0),      // large pile, alt
        ] {
            let mut world = floor(40);
            assert!(
                place_object(&mut world, 20, 29, block, style, -1),
                "block {block} should have placed"
            );
            let object = tile_object::tile_object(block).unwrap();
            let (left, top) = (20 - object.origin.0, 29 - object.origin.1);
            for dx in 0..object.width {
                for dy in 0..object.height {
                    let tile = world.tile(left + dx, top + dy);
                    assert_eq!(tile.block, block);
                    assert_ne!(tile.frame_x, -1, "block {block} cell {dx},{dy} unframed");
                    assert_ne!(tile.frame_y, -1, "block {block} cell {dx},{dy} unframed");
                }
            }
        }
    }

    #[test]
    fn refuses_to_place_over_something_already_there() {
        let mut world = floor(40);
        world.set_tile(19, 29, Tile::block(2));
        assert!(
            !place_object(&mut world, 20, 29, 105, 0, -1),
            "a statue overlapping an occupied tile must be refused entirely"
        );
        // And refusing must not have written the cells that *were* clear.
        assert!(!world.tile(20, 27).is_active());
    }

    #[test]
    fn refuses_to_float_over_a_gap() {
        // A statue anchored at (20, 29) with origin (1, 2) occupies columns 19-20, rows 27-29,
        // so the floor beneath it is row 30 across those same two columns.
        let mut world = floor(40);
        world.set_tile(19, 30, Tile::AIR); // a one-tile hole under the left column of the footprint
        assert!(
            !place_object(&mut world, 20, 29, 105, 0, -1),
            "one gap under the footprint should refuse the whole object"
        );
        assert!(
            !world.tile(20, 27).is_active(),
            "and must not have written anything"
        );
    }

    #[test]
    fn an_unknown_block_is_simply_refused() {
        let mut world = floor(40);
        assert!(!place_object(&mut world, 20, 29, 2, 0, -1));
    }
}
