# World generation

This server generates worlds that can be **played through**: every biome, a dungeon, an
underworld, an evil with orbs in it, altars, a temple, chests, life crystals.

**Code:** [`world/worldgen/`](../crates/terrustia/src/world/worldgen).

## Two different targets, and which this is

| | Vanilla parity | This |
|---|---|---|
| Same seed as Terraria | identical world | different world |
| Cost | 219–372 engineer-days | done |
| Status | not started; harness built | working |

They are not the same job and confusing them is easy. **Seed-identical** generation means
transcribing 106 passes in exact order with exact random-number consumption, verified against the
oracle in [`manifest.rs`](../crates/terrustia/src/world/worldgen/manifest.rs). That is tracked
separately in [worldgen-parity.md](worldgen-parity.md) and is measured in months.

What is here is the fallback that plan names: *every structure present, built with our own
algorithms, beatable but not identical*. It exists because a server that generates unplayable
worlds is not a working server, and this is a much nearer target.

The two share a random generator — `UnifiedRandom`, the game's own — so a seed means the same kind
of thing in both, and the parity work can replace this a pass at a time without changing the
interface.

## Checking a world is playable

```sh
cargo run --release -p terrustia --example playable -- world.wld
```

It walks the world and reports each link in the progression chain, naming the boss that becomes
unreachable when one is missing. On a generated 4200×1200 world:

```
Terrustia (4200x1200)
  crimson · spawn 2100,312 · surface 336 · rock 504

        20  shadow orbs / crimson hearts
       210  demon altars
       184  life crystals
     14645  hellstone
       776  lihzahrd brick
         4  larva
      3641  dungeon brick
       310  chests with contents

this world can be played through: every link in the chain is present.
```

The same tool passes on a real Terraria world, which is the control that makes the result mean
something.

## The chain

Every structure gates something. This is the list the generator exists to satisfy:

| Structure | Without it |
|---|---|
| Evil biome with orbs or hearts | no Eater of Worlds or Brain of Cthulhu, so no demonite, so no meteor |
| Dungeon | no Skeletron, and nothing behind him |
| Underworld with hellstone | no Wall of Flesh, so no hardmode |
| Demon altars | no hardmode ores, so nothing to fight the mechanical bosses with |
| Jungle temple | no Golem |
| Bee hive with a larva | no Queen Bee without a summon item |
| Life crystals | a hundred hit points for the whole game |
| Chests | no starter weapons, no hooks, no boots |

## How it is built

Passes run in a fixed order and each carves into what the last left. The order is load-bearing,
not tidy: ore seeded before the caves would be hollowed back out, and chests placed before the
caves would have nowhere to stand.

1. **[`layout`]** decides where everything goes *before any tile is written*. The dungeon and the
   jungle take opposite sides — a player must cross the world to go from Skeletron to the temple,
   and putting them together removes most of a playthrough's middle. The evil keeps well clear of
   spawn, because landing in corruption on the first morning is a dead character rather than a
   difficulty curve.
2. **[`terrain`]** walks a surface line and fills the layers under it, with each biome's own
   material over both. Only the surface crust gets a background wall; the underground and cavern
   layers come out of this pass at wall 0, as vanilla's do (`DirtWallBackgrounds`,
   `WorldGen.cs:11895`, stops at `worldSurface + 0..10`), and stay that way until the two Tier 3
   passes paint wall back into selected open pockets.
3. Caves are **thousands of independent tapering runners**, which is vanilla's own mechanism:
   `SmallHoles`, `DirtLayerCaves`, `RockLayerCaves` and the `Caverer` tail of `SurfaceCaves`, all
   driven by `TileRunner` (`WorldGen.cs:77596`). Each runner digs a blob whose radius tapers from
   full to nothing over its own step count, walked along a velocity that is itself a bounded random
   walk, so it is self-terminating and nothing steers it towards anything else. That is what makes
   an underground of isolated pockets and occasional large caverns rather than one network, and
   several later passes (`GemCaves`, `SpiderCaves`, `CaveWallsInEnclosedSpaces`) can only site
   themselves at all because pockets come in measurable sizes.
4. Ore is seeded in depth bands: copper and iron near the surface so a new character can find a
   pickaxe's worth, gold and silver deeper so they are worth going down for.
5. Structures: evil chasms, dungeon, temple, hive, underworld.
6. Altars, life crystals and chests fill what is left.
7. Grass, plants and cobwebs finish it.

## Four things that were got wrong first

Recorded because each was reasonable and each was wrong.

**Oceans cannot be drifted into.** The surface is a walk that drifts a tile at a time towards
what each biome wants. An ocean wants to be forty-five tiles lower, and drifting there takes
hundreds of columns — so the seaward end sat at land height with no basin, and both oceans came
out dry. They are carved in a **second pass** now, after the land's height at the shore is known,
which also removes the seam: the shore matches the land exactly and the floor falls away squared,
so the beach shelves and the far end is properly deep.

**A random point almost never lands on a ledge.** Most of a world is either solid or open air, and
a ledge is the thin boundary between. Picking a random point and checking it produced *one* altar
where twelve were wanted. Falling from a random point until there is a floor — which is what a
player would do — finds them reliably.

**Hollowing a tile has to clear its frame.** Clearing the block and the active bit leaves the
frame behind, which is inconsistent state that nothing notices until the world is saved: the
format writes no frame for an inactive tile, so it reads back different from what was written. A
round-trip check that should have been exact came back five tiles short, and that is how this was
found.

**A small world is not a big world scaled down.** Fixed constants — a 250-tile ocean, a 200-tile
clearance — make the two oceans of an 800-tile world overlap in the middle, leaving nowhere for
anything else and sending every later band's arithmetic backwards. The generator throws on a
backwards range rather than returning nonsense, which is the right behaviour and is what surfaced
this. Every band now scales, and when a small world genuinely has nowhere left, the *best*
remaining spot is taken rather than the first — taking the first put the evil biome six tiles from
spawn.

## A server bug this found

Adding the generator broke eight unrelated tests, and the cause was not the generator.

`sections_for` sends a joining client a block of sections around spawn, and it *clipped* that
block against the world's edges rather than sliding it inside them. A player who spawned in the
topmost section therefore received one fewer section beneath them than intended — a hundred and
fifty tiles of world simply absent below their feet. It had never shown up because the old
generator always put the surface low enough to be in section one.

## Three more, found by emulating Terraria's loader

A later audit re-implemented `LoadWorld_Version2` and ran it against a world this generator had
made. The framing was right — all eleven section pointers validated exactly, with nothing left
over — but three things inside it were not.

**The hardmode ore tiers must be `-1`, not `0`.** This is the one that ended a playthrough.
Terraria's sentinel for "this tier has not been chosen yet" is `-1`, set by `WorldGen.clearWorld`.
`SmashAltar` only rolls an ore *when it reads that sentinel*; given `0` it skips the roll and
hands `OreRunner` tile type 0, which is dirt. And `CheckSavedOreTiers` repairs the four
pre-hardmode tiers on load but never these three, so the wrong value sticks for the world's life.
The result: no cobalt, no mythril, no adamantite, and therefore no mechanical boss, in any world
this server generated.

**Chest records outlive their chests.** Chests are placed part-way through generation, and
`greenery`, `cobwebs` and the spawn pocket all write tiles afterwards — sometimes straight through
a chest's footprint. The record survives, pointing at cleared ground. Terraria loads it happily
and then deletes it, and its contents, on its own first save, so the loot vanishes some time after
the world changed hands rather than when the damage was done. Seventeen of twenty-one worlds
checked had between one and four of these. There is now a footprint sweep at the end of `build`,
which is the same check the game runs when it saves.

**Two header fields were simply never written.** `dungeon_y` went out as the surface height, and
the seed text as an empty string — both of them recorded correctly by the generator, read back
correctly by the parser, and dropped only by the writer. The consequence of the first is that
`CultistRitual` puts the Lunatic Cultist in the wrong place after Golem.

### Why the tests did not see any of it

The round-trip test compared width, height, chest count and tiles. Header fields were never
looked at, so a writer that dropped one had nothing to fail. The fix is not the three patches but
`every_header_field_survives_a_save`, which compares the whole header and reports every difference
in one run.

Two things that test had to learn the hard way, both worth keeping in mind for any check here:

- **One world is not a sample.** The first version of the chest sweep's test used a single seed,
  which happened to be one of the four clean ones in twenty-one, and passed against the untouched
  bug. Both tests now run several sizes and seeds.
- **Size changes the header.** The header holds variable-length runs, so a writer and reader that
  disagree about one of them can agree at 1200x600 and not at 4200x1200. Both are checked.

[`layout`]: ../crates/terrustia/src/world/worldgen/layout.rs
[`terrain`]: ../crates/terrustia/src/world/worldgen/terrain.rs
