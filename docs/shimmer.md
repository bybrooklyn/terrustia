# Shimmer

The 1.4.4 transmutation pool. An item dropped into it becomes another item, a creature, or — for
coins — luck.

**Code:** [`terrustia-proto/src/shimmer.rs`](../crates/terrustia-proto/src/shimmer.rs) (generated),
`shimmer()` in [`world/items.rs`](../crates/terrustia/src/world/items.rs), `tick_shimmer` in
[`game/server.rs`](../crates/terrustia/src/game/server.rs).

## It is not instant, and that matters

An item does not transmute on contact. It **sinks** — 0.01 per tick, so about a second and a half
— and changes at nine tenths of the way in. Pull it out before then and the counter runs back
down. That delay is the whole feel of the mechanic: shimmer is reversible until it isn't.

The threshold is held at 0.9 rather than 1.0, as the game holds it, so the transformation is
visible rather than happening to something that has already disappeared.

One detail worth recording: it takes **91** ticks, not 90. Adding `0.01f32` ninety times lands a
hair under 0.9. The game accumulates the same way and arrives at the same place, so the off-by-one
is faithful rather than sloppy — and the test asserts a range with that written next to it.

## What the test is

The game checks the tile **above** the item's position, not the one it occupies: an item is
sinking through a surface, not standing in a pool. Getting that wrong makes shimmer either
untriggerable or triggered by being near it.

## What it turns things into

Generated from `ItemID.Sets` and `NPCID.Sets`:

| Table | Entries | What |
|---|---:|---|
| `TRANSFORMS` | 316 | item → item |
| `COUNTS_AS` | 6 | items that shimmer as though they were another |
| `NPC_TRANSFORMS` | 114 | a caught creature → another creature |

Stored as sorted key/value pairs and binary-searched, not as a full array — a few hundred pairs
out of several thousand items would otherwise be mostly zeroes.

**Transform pairs are usually symmetric.** Wood becomes stone and stone becomes wood. So an item
carries a `shimmered` flag once transmuted, and without it a dropped item would flicker between
its two forms forever.

## Coins

Not transmuted but spent: a stack becomes coin luck and vanishes.

| Coin | Worth |
|---|---:|
| Copper | 1 each |
| Silver | 100 each |
| Gold | 10,000 each |
| Platinum | **1,000,000 flat** |

Platinum is capped at one coin's worth however many go in — the game's own rule, and what stops a
stack being worth a billion. The arithmetic saturates rather than wrapping.

## Decrafting

An item with no transform and no creature comes apart into what it was made of.

This needed a recipe table, which sounded like it needed a crafting system — it does not. Nothing
here knows about crafting stations, conditions or whether a player can reach a bench. It answers
one question: *if this were broken apart, what would come out?*

[`recipes.rs`](../crates/terrustia-proto/src/recipes.rs) is generated from `Recipe.SetupRecipes`:
**3,292 recipes parsed, 3,098 decraftable, 3,083 craftable items, 5,372 ingredient entries.** Two
of the game's rules are baked in rather than applied at runtime — where several recipes make the
same item the *last* wins (`UpdateWhichItemsAreCrafted` overwrites as it goes), and recipes marked
`notDecraftable` are excluded outright.

Around 570 of those recipes never appear as a literal `createItem.SetDefaults(N)` in `Recipe.cs`:
`SetupRecipes` builds them inside `for` loops and small per-set helper functions instead (fake
chests, weapon racks, alphabet and critter statues, jars, gemspark walls, the counterweight, and
every `AddXxxFurniture` set), which a plain literal parse cannot see. This was D1: the generator's
`loop_built_recipes` unrolls each family by hand against the cited `Recipe.cs` lines, and turned up
one real bug in the pre-D1 table along the way: the counterweight loop reused a literal
`createItem` argument across six iterations that only differed by a loop-variable ingredient, so the
old naive parse kept a real but truncated recipe (one ingredient instead of two) rather than missing
it outright.

Three rules govern what comes back:

- **Whole batches only.** A recipe that makes three at a time needs three to decraft. Two torches
  stay two torches; the remainder of a larger stack stays as it was.
- **The world's evil can change the answer.** Some recipes have a crimson and a corruption
  variant, so the same item decrafts differently depending on the world.
- **Alchemy gives back less.** Each unit of an alchemy recipe's ingredients has a one-in-three
  chance of being lost, which is what stops potions being a free material duplicator.

The table is checked against the source by a script written from the format rather than from the
generator, and as of 2026-09-05 it reads **all 3,090 recipes, all matching**. It used to read 2,545
of them: the loop-built and helper-built families named their results through parameters, counters
and locals rather than literals, so `tools/check_recipes.py` saw none of them and the 548 unique
new items had to be checked by hand instead. It inlines the helpers, unrolls the counted loops and
resolves the recipe groups now, so nothing in the table is unchecked and `just check-mutants` kills
100% of the mutants it makes against it.

## A note on chains

Decrafting can cascade, and that is correct. A gold bar breaks into four gold ore, and gold ore
has a transform of its own — so it shimmers again into platinum ore. Watching one go in and
several things come back out over a few seconds is the mechanic working, not a loop.

What *cannot* happen is an item going round forever: anything that has been through carries a
flag, and the things it became are new items with their own one turn each.

## Packets

| Id | Direction | What |
|---|---|---|
| 146 action 0 | server → clients | the sparkle where something transmuted |
| 146 action 1 | server → clients | coins became this much luck, here |
