//! A point set for describing an irregular shape, independent of any particular tile.
//!
//! Transcribed from `Terraria.WorldBuilding.ShapeData` (`.scratch/decompiled/Terraria.WorldBuilding/ShapeData.cs`,
//! 114 lines — confirmed by reading it directly, not assumed from the earlier sizing pass's cited
//! count). Used by `CaveWallVariety` (Tier 3) and lightly by underground cabins (Tier 2) to build
//! up an arbitrary set of tile positions — a blob, a room outline, whatever a pass wants — that
//! can be stamped down, offset, unioned with another shape, or subtracted from one. It has nothing
//! to do with the `WorldUtils`/`Actions`/`Modifiers` DSL the original Tier 2 sizing guess assumed
//! was required; see `plan.md`'s correction and `structure_map.rs`'s doc comment for the same
//! point made about `StructureMap`.
//!
//! **One deliberate deviation.** Vanilla's static `GetBounds` calls `.First()` on the first
//! shape's point set with no empty check — a real latent panic in vanilla if ever called with an
//! empty shape, safe only because every real call site happens to pass a non-empty one. `bounds`
//! below returns `None` in that case instead of panicking, which is strictly safer and preserves
//! vanilla's actual behaviour for every case that matters.
//!
//! Not wired into anything yet — see `structure_map.rs`'s doc comment for why.

use std::collections::HashSet;

use super::structure_map::Rect;

/// Transcribed from `ShapeData`. Backed by a plain `(i32, i32)` pair rather than vanilla's
/// `Point16` — this engine's tile coordinates are `i32` everywhere else (`World::tile`, `Layout`),
/// and matching that beats reintroducing a 16-bit coordinate type for one struct.
#[derive(Debug, Clone, Default)]
pub struct ShapeData {
    points: HashSet<(i32, i32)>,
    /// Insertion order, kept alongside the set so iteration is deterministic.
    ///
    /// This matters and is not tidiness. `ModShapes::All`/`InnerOutline` walk the point set and
    /// hand each point to an action chain, and some of those chains consume RNG per point
    /// (`ActionVines` draws a vine length, `Modifiers.Blotches` draws four offsets). Rust seeds
    /// each `HashSet` from a per-process random state, so iterating one directly would make world
    /// generation differ run to run from the same seed - the one thing a world generator may not
    /// do. Vanilla is not affected because .NET's `HashSet` enumerates its backing slots in order,
    /// which for a set built by insertion with no intervening removals *is* insertion order, so
    /// keeping that order is also the faithful choice rather than merely a deterministic one.
    ///
    /// May hold points that have since been removed; [`Self::iter`] filters against the set, and
    /// [`Self::add`] compacts first if anything was removed, so a removed-then-readded point
    /// cannot appear twice.
    order: Vec<(i32, i32)>,
    removed: usize,
}

impl ShapeData {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn count(&self) -> usize {
        self.points.len()
    }

    pub fn add(&mut self, x: i32, y: i32) {
        if self.removed > 0 {
            self.compact();
        }
        if self.points.insert((x, y)) {
            self.order.push((x, y));
        }
    }

    /// Drops the removed points from the order list, so it matches the set again.
    fn compact(&mut self) {
        let points = &self.points;
        self.order.retain(|p| points.contains(p));
        self.removed = 0;
    }

    /// The points in insertion order. Use this for anything that writes to the world, and
    /// especially for anything that consumes RNG; see the field doc on `order`.
    pub fn iter(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.order
            .iter()
            .copied()
            .filter(|p| self.points.contains(p))
    }

    /// Fills the inclusive rectangle `[min_x, max_x] x [min_y, max_y]`.
    pub fn add_bounds(&mut self, min_x: i32, min_y: i32, max_x: i32, max_y: i32) {
        for x in min_x..=max_x {
            for y in min_y..=max_y {
                self.add(x, y);
            }
        }
    }

    pub fn remove(&mut self, x: i32, y: i32) {
        if self.points.remove(&(x, y)) {
            self.removed += 1;
        }
    }

    /// Clears the inclusive rectangle `[min_x, max_x] x [min_y, max_y]`.
    pub fn remove_bounds(&mut self, min_x: i32, min_y: i32, max_x: i32, max_y: i32) {
        for x in min_x..=max_x {
            for y in min_y..=max_y {
                self.remove(x, y);
            }
        }
    }

    pub fn clear(&mut self) {
        self.points.clear();
        self.order.clear();
        self.removed = 0;
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.points.contains(&(x, y))
    }

    pub fn data(&self) -> &HashSet<(i32, i32)> {
        &self.points
    }

    /// Unions `other`'s points into `self`, translated so `other`'s `remote_origin` lands on
    /// `self`'s `local_origin` — vanilla's `Add(ShapeData, Point, Point)`.
    pub fn merge_from(
        &mut self,
        other: &ShapeData,
        local_origin: (i32, i32),
        remote_origin: (i32, i32),
    ) {
        let (dx, dy) = (
            remote_origin.0 - local_origin.0,
            remote_origin.1 - local_origin.1,
        );
        // `iter`, not `data`: the order points arrive in becomes this shape's own iteration order.
        for (x, y) in other.iter() {
            self.add(dx + x, dy + y);
        }
    }

    /// Removes `other`'s points from `self`, under the same translation as [`Self::merge_from`] —
    /// vanilla's `Subtract`.
    pub fn subtract_from(
        &mut self,
        other: &ShapeData,
        local_origin: (i32, i32),
        remote_origin: (i32, i32),
    ) {
        let (dx, dy) = (
            remote_origin.0 - local_origin.0,
            remote_origin.1 - local_origin.1,
        );
        for (x, y) in other.iter() {
            self.remove(dx + x, dy + y);
        }
    }

    /// The union bounding box of one or more shapes, offset by `origin`. `None` where vanilla
    /// would panic — see the module doc.
    pub fn bounds(origin: (i32, i32), shapes: &[&ShapeData]) -> Option<Rect> {
        let mut points = shapes.iter().flat_map(|s| s.data().iter());
        let &(first_x, first_y) = points.next()?;
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (first_x, first_x, first_y, first_y);
        for &(x, y) in points {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        Some(Rect::new(
            min_x + origin.0,
            min_y + origin.1,
            max_x - min_x,
            max_y - min_y,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_is_present_after_adding_and_gone_after_removing() {
        let mut shape = ShapeData::new();
        assert!(!shape.contains(3, 4));
        shape.add(3, 4);
        assert!(shape.contains(3, 4));
        shape.remove(3, 4);
        assert!(!shape.contains(3, 4));
    }

    #[test]
    fn adding_the_same_point_twice_does_not_grow_the_count() {
        let mut shape = ShapeData::new();
        shape.add(1, 1);
        shape.add(1, 1);
        assert_eq!(shape.count(), 1);
    }

    #[test]
    fn add_bounds_fills_the_whole_inclusive_rectangle() {
        let mut shape = ShapeData::new();
        shape.add_bounds(0, 0, 2, 1);
        assert_eq!(shape.count(), 6); // 3 wide, 2 tall, inclusive
        assert!(shape.contains(2, 1));
        assert!(!shape.contains(3, 1));
    }

    #[test]
    fn remove_bounds_clears_only_the_named_rectangle() {
        let mut shape = ShapeData::new();
        shape.add_bounds(0, 0, 4, 4);
        shape.remove_bounds(1, 1, 2, 2);
        assert!(shape.contains(0, 0));
        assert!(!shape.contains(1, 1));
        assert!(!shape.contains(2, 2));
        assert!(shape.contains(4, 4));
    }

    #[test]
    fn merge_from_translates_by_the_origin_difference() {
        let mut stamp = ShapeData::new();
        stamp.add(0, 0);
        stamp.add(1, 0);

        let mut canvas = ShapeData::new();
        // Stamp the shape's local (0, 0) onto the canvas at (10, 10).
        canvas.merge_from(&stamp, (0, 0), (10, 10));
        assert!(canvas.contains(10, 10));
        assert!(canvas.contains(11, 10));
        assert!(!canvas.contains(0, 0));
    }

    #[test]
    fn subtract_from_removes_the_same_points_merge_from_would_add() {
        let mut stamp = ShapeData::new();
        stamp.add(0, 0);
        stamp.add(1, 0);

        let mut canvas = ShapeData::new();
        canvas.add_bounds(9, 9, 12, 11);
        canvas.subtract_from(&stamp, (0, 0), (10, 10));
        assert!(!canvas.contains(10, 10));
        assert!(!canvas.contains(11, 10));
        assert!(canvas.contains(9, 9), "outside the stamp, untouched");
    }

    #[test]
    fn bounds_of_an_empty_shape_list_is_none_rather_than_a_panic() {
        let empty = ShapeData::new();
        assert_eq!(ShapeData::bounds((0, 0), &[&empty]), None);
    }

    #[test]
    fn bounds_is_the_offset_union_of_every_shape_given() {
        let mut a = ShapeData::new();
        a.add_bounds(0, 0, 2, 2);
        let mut b = ShapeData::new();
        b.add(10, -1);

        let bounds = ShapeData::bounds((100, 200), &[&a, &b]).unwrap();
        // Union of a (0..=2, 0..=2) and b (10, -1): x in [0, 10], y in [-1, 2].
        assert_eq!(bounds, Rect::new(100, 199, 10, 3));
    }

    #[test]
    fn a_future_pass_builds_an_irregular_blob_from_two_stamps() {
        // Illustrative: how a real Tier 2/3 pass would actually use this — build a shape out of
        // pieces, then hand its bounds to StructureMap::can_place before ever touching a tile.
        let mut core = ShapeData::new();
        core.add_bounds(-2, -2, 2, 2);
        let mut fringe = ShapeData::new();
        fringe.add(3, 0);
        fringe.add(-3, 0);

        let mut blob = ShapeData::new();
        blob.merge_from(&core, (0, 0), (0, 0));
        blob.merge_from(&fringe, (0, 0), (0, 0));

        assert_eq!(blob.count(), 25 + 2);
        let bounds = ShapeData::bounds((500, 300), &[&blob]).unwrap();
        assert_eq!(bounds, Rect::new(497, 298, 6, 4));
    }

    /// Iteration is insertion-ordered, not hash-ordered.
    ///
    /// Worth a test rather than trust: the reason it matters is invisible at this level. The
    /// pipeline in `genpipe` walks a shape's points handing each to an action chain, and some of
    /// those chains draw from the world RNG per point, so an iteration order that varies run to
    /// run makes a seeded world generate differently every time. A plain `HashSet` does exactly
    /// that in Rust, and the failure would show up nowhere near here.
    #[test]
    fn iteration_follows_insertion_and_not_the_hash() {
        let inserted = [(5, 1), (-3, 9), (0, 0), (12, 12), (-7, 2), (4, -4)];
        let mut shape = ShapeData::new();
        for &(x, y) in &inserted {
            shape.add(x, y);
        }
        assert_eq!(shape.iter().collect::<Vec<_>>(), inserted);

        // Re-adding an existing point must not duplicate it or move it.
        shape.add(0, 0);
        assert_eq!(shape.iter().collect::<Vec<_>>(), inserted);

        // A removed point leaves the order of the rest alone.
        shape.remove(0, 0);
        assert_eq!(
            shape.iter().collect::<Vec<_>>(),
            [(5, 1), (-3, 9), (12, 12), (-7, 2), (4, -4)]
        );

        // Removed and re-added: it goes to the back, and appears exactly once. This is the case
        // the compaction in `add` exists for; without it the stale entry would still be in the
        // order list and the point would be yielded twice.
        shape.add(0, 0);
        assert_eq!(
            shape.iter().collect::<Vec<_>>(),
            [(5, 1), (-3, 9), (12, 12), (-7, 2), (4, -4), (0, 0)]
        );
        assert_eq!(shape.count(), 6);
    }

    /// The same shape built the same way twice iterates identically. This is the property a seeded
    /// world generator actually depends on; the test above pins the exact order, this one pins
    /// that there *is* one.
    #[test]
    fn two_identically_built_shapes_iterate_identically() {
        let build = || {
            let mut s = ShapeData::new();
            s.add_bounds(-6, -4, 6, 4);
            let mut hole = ShapeData::new();
            hole.add_bounds(-2, -2, 2, 2);
            s.subtract_from(&hole, (0, 0), (0, 0));
            s
        };
        assert_eq!(
            build().iter().collect::<Vec<_>>(),
            build().iter().collect::<Vec<_>>()
        );
    }
}
