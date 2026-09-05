# Generated tables

## The rule

**Per-type variation lives in generated tables. Hand-written modules hold algorithms only.**

There are 697 NPC types, 754 tiles, 401 buffs and several thousand items. Any rule that differs
per type is *data*. A hand-written match over 697 cases is wrong the moment the game changes, and
wrong invisibly — nothing fails, a few types just quietly behave like the wrong thing.

So the shape everywhere is: a table generated from the game's own, and a small hand-written module
that reads it.

## What is generated

| File | Lines | From | Generator |
|---|---:|---|---|
| `npc_data.rs` | 13,323 | `NPC.SetDefaults` | — (`just check-npc-data`) |
| `tile_object.rs` | 6,099 | `TileObjectData.Initialize` | — |
| `npc_params.rs` | 4,721 | `NPCID.Sets`, `NPC.SetDefaults` | — |
| `npc_drops.rs` | ~6,800 | `ItemDropDatabase` | `gen_drops.py` |
| `projectile_data.rs` | ~10,000 | `Projectile.SetDefaults` | `gen_projectiles.py` |
| `banners.rs` | ~520 | `BannerSystem` / `ItemID.Sets.KillsToBanner` | `gen_banners.py` |
| `placed_items.rs` | 2,550 | `Item.SetDefaults` | — (`just check-placed-items`) |
| `town_names.rs` | 517 | localisation + `NPC.getNewNPCNameInner` | `gen_town_names.py` |
| `buffs.rs` | ~450 | `Main.debuff`, `BuffID.Sets`, `NPCID.Sets.DebuffImmunitySets` | `gen_buffs.py` |
| `tile_drops.rs` | 395 | `WorldGen.KillTile_GetItemDrops` | — |
| `conditional_drops.rs` | 490 | drop rules with conditions | — |
| `statues.rs` | 313 | `Wiring.HitSwitch` statue cases | — |
| `recipes.rs` | ~3,600 | `Recipe.SetupRecipes` | `gen_recipes.py` |
| `shimmer.rs` | ~200 | `ItemID.Sets`, `NPCID.Sets` | `gen_shimmer.py` |
| `hurt_tiles.rs` | ~120 | `TileID.Sets` + `Collision.CanTileHurt` | `gen_hurt_tiles.py` |
| `angler.rs` | ~120 | `Main.AnglerQuestSwap` | `gen_angler.py` |
| `travel_shop.rs` | ~90 | `Chest.SetupTravelShop_GetItem` | `gen_travel_shop.py` |
| `tile_death.rs` | 179 | `Main.tileLavaDeath`, `Main.tileWaterDeath` | `terrustia-codegen tile_death` |
| `net_variants.rs` | ~660 | `NPC.SetDefaultsFromNetId` | `terrustia-codegen net_variants` |

The ones with a generator in [`tools/`](../tools) can be rebuilt:

```sh
D=<path-to-decompiled-1.4.5.7-tree>
python3 tools/gen_buffs.py      "$D" crates/terrustia-proto/src/buffs.rs
python3 tools/gen_town_names.py "$D" crates/terrustia-proto/src/town_names.rs
python3 tools/gen_hurt_tiles.py "$D" crates/terrustia-proto/src/hurt_tiles.rs
python3 tools/gen_angler.py     "$D" crates/terrustia-proto/src/angler.rs
python3 tools/gen_shimmer.py    "$D" crates/terrustia-proto/src/shimmer.rs
python3 tools/gen_recipes.py    "$D" crates/terrustia-proto/src/recipes.rs
python3 tools/gen_travel_shop.py "$D" crates/terrustia-proto/src/travel_shop.rs
python3 tools/gen_drops.py      "$D" crates/terrustia-proto/src/npc_drops.rs
python3 tools/gen_projectiles.py "$D" crates/terrustia-proto/src/projectile_data.rs
python3 tools/gen_banners.py    "$D" crates/terrustia-proto/src/banners.rs
```

And two checkers, which report rather than emit:

```sh
python3 tools/check_recipes.py "$D"   # a sample of recipes, re-parsed independently
python3 tools/check_drops.py   "$D"   # loot the game gives that we do not
```

Each script fails loudly if the source's shape has changed — a parse that finds too few entries
raises rather than emitting a table that is quietly short. That matters more than it sounds: a
generator that silently produces an empty set turns "immune to nothing" into the default for
every NPC in the game.

## Writing a generator

Things learned the hard way:

**Parse the conditions, do not read them off.** `AnglerQuestSwap`'s fifteen availability rules are
a run of guard clauses. Transcribing them by hand is one typo away from asking a fresh world for a
hardmode fish, which costs the player a whole day. Parsing them means the table is checkable
against the source by re-running the script.

**Watch for conditions that are not about the type.** An early extractor for pre-289 header fields
attributed conditional overrides (`remixWorld`, `!hardMode`) to the *type* rather than to the
condition, and reported 41 disagreements that did not exist. Only the third attempt was right.

**Intern repeated data.** 697 NPC types share only 34 distinct debuff-immunity masks. Emitting 697
bitmaps would be 40× the bytes for the same table.

**A checker is the other way to hold a hand-written table to source.** `npc_data.rs` has no
generator and is unlikely to get one worth the risk: it is the largest table here, and the chain it
comes from is nested four ways that a naive parse gets wrong (see `tools/check_npc_data.py`'s own
header for all four). `just check-npc-data` re-reads that chain and compares all 691 entries on all
16 fields instead, which is the protection rule 7 is actually after - a table cannot go stale
without somebody seeing it. Written 2026-09-05; it found the table already correct, with seven
deliberate differences on the record.

**Validate a new generator against the table it replaces.** `npc_drops.rs` and
`projectile_data.rs` were both hand-written and both hand-verified, which made them the ideal test
for the generators that replaced them: anything the generator *loses* is a parsing bug. That check
caught four in the drop generator — multi-line id arrays, chained `RegisterToMultipleNPCs` calls,
calls assigned to a local first, and `NormalvsExpert` — each of which would otherwise have silently
deleted working loot. It also found a bug in the *old* table: `npcNetIds12` is `{-6, -7, -8, -9}`,
negative variant ids, and the transcription had read them as NPC types 6 and 7, giving the Slime
Staff to the Eater of Souls and the Devourer.

**Generate only what is genuinely flat.** The drop database is a tree of condition chains and
option pools. `gen_drops.py` takes the unconditional subset and refuses the rest, which stays
hand-written in `conditional_drops.rs` under `check_drops.py`'s eye. A generator that flattened a
condition would hand out the wrong loot forever while looking authoritative — worse than the gap it
closed.

**Check a big one against the source with a *second* script.** `recipes.rs` holds 2,551 recipes;
a bug in the generator would be invisible in review and would quietly give back the wrong
ingredients forever. So a separate checker, written from the format rather than from the
generator, re-parses a random sample and compares — 300 recipes, all matching. A bug shared by
both would have to be made twice.

**Use `static`, not `const`, for the large ones.** A `const` array is copied at every use site.
Clippy catches this; it is worth knowing why rather than just applying the fix.

**Say what is deliberately absent.** `hurt_tiles.rs` omits two tiles the game can make dangerous,
because it gates them behind world seeds this server does not offer. That is recorded in the
file's own doc comment, so the next person to compare against the game finds the answer rather
than the discrepancy.

## What is *not* generated, and why

`tile_sets.rs`, `tile_solid.rs` and the frame-importance table were transcribed rather than
generated. They are stable across versions and were verified mechanically against the source —
754×2 solidity entries and 754 frame-importance flags, all compared. Regenerating them is worth
doing if they ever drift, but they have not.

`is_dungeon_wall` in `game/teleport.rs` is a nine-entry `matches!` rather than a table. That is a
deliberate exception: the set is tiny and has not changed in four major versions, and a
nine-element generated file would be more machinery than the thing it holds.
