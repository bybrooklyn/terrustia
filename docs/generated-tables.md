# Generated tables

## The rule

**Per-type variation lives in generated tables. Hand-written modules hold algorithms only.**

There are 697 NPC type slots (691 of them defined; `NPC_COUNT` is the array bound and six slots
carry nothing, which is why the README says 691 and this says 697), 754 tiles, 401 buffs and
several thousand items. Any rule that differs per type is *data*. A hand-written match over 697
cases is wrong the moment the game changes, and wrong invisibly — nothing fails, a few types just
quietly behave like the wrong thing.

So the shape everywhere is: a table generated from the game's own, and a small hand-written module
that reads it.

## What is generated

There was a `Lines` column here until 2026-09-06. It is gone rather than corrected: thirteen of its
twenty rows were wrong, several by three to eight times (`conditional_drops.rs` was listed at 490
against 3,850), because a generated file's length changes on every regen and nothing checked the
column. `wc -l crates/terrustia-proto/src/*.rs` answers the question it was trying to answer, and
cannot be out of date.

| File | From | Generator |
|---|---|---|
| `npc_data.rs` | `NPC.SetDefaults` | none (`just check-npc-data`) |
| `tile_object.rs` | `TileObjectData.Initialize` | `terrustia-codegen tile_object` (`just check-tile-object`) |
| `npc_params.rs` | `NPCID.Sets`, `NPC.SetDefaults` | none |
| `npc_drops.rs` | `ItemDropDatabase` | `terrustia-codegen drops` |
| `projectile_data.rs` | `Projectile.SetDefaults` | `terrustia-codegen projectiles` |
| `banners.rs` | `BannerSystem` / `ItemID.Sets.KillsToBanner` | `terrustia-codegen banners` |
| `golf_physics.rs` | `MaterialData/Materials.json` + `Tiles.json` | `terrustia-codegen golf` |
| `placed_items.rs` | `Item.SetDefaults`, `GetItemDrop_*`, six inline arms | none (`just check-placed-items`) |
| `town_names.rs` | localisation + `NPC.getNewNPCNameInner` | `terrustia-codegen town_names` |
| `buffs.rs` | `Main.debuff`, `BuffID.Sets`, `NPCID.Sets.DebuffImmunitySets` | `terrustia-codegen buffs` |
| `tile_drops.rs` | `WorldGen.KillTile_GetItemDrops` | none |
| `conditional_drops.rs` | drop rules with conditions | none |
| `statues.rs` | `Wiring.HitSwitch` statue cases | none |
| `recipes.rs` | `Recipe.SetupRecipes` | `terrustia-codegen recipes` (`just check-recipes`) |
| `shimmer.rs` | `ItemID.Sets`, `NPCID.Sets` | `terrustia-codegen shimmer` |
| `hurt_tiles.rs` | `TileID.Sets` + `Collision.CanTileHurt` | `terrustia-codegen hurt_tiles` |
| `angler.rs` | `Main.AnglerQuestSwap` | `terrustia-codegen angler` |
| `travel_shop.rs` | `Chest.SetupTravelShop_GetItem` | `terrustia-codegen travel_shop` |
| `tile_death.rs` | `Main.tileLavaDeath`, `Main.tileWaterDeath` | `terrustia-codegen tile_death` |
| `net_variants.rs` | `NPC.SetDefaultsFromNetId` | `terrustia-codegen net_variants` |

The `gen_*.py` scripts this table used to name are gone: every one of them is now a module of the
`terrustia-codegen` binary, which `just regen` runs over the whole set at once.

```sh
just regen            # every generated table, then `cargo fmt --all`
cargo run -p terrustia-codegen --bin codegen -- <table> "$D" <out.rs>   # just one
```

And the checkers, which report rather than emit:

```sh
just check-recipes      # every recipe, re-parsed independently
just check-drops        # loot the game gives that we do not
just check-npc-data     # every `NPC.SetDefaults` entry, all 16 fields
just check-placed-items # every `createTile`/`placeStyle` pair the game defines
just check-tile-object  # `TileObjectData.Initialize`, read by a second interpreter
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

**Check a big one against the source with a *second* script.** `recipes.rs` holds 3,090 recipes;
a bug in the generator would be invisible in review and would quietly give back the wrong
ingredients forever. So a separate checker, written from the format rather than from the
generator, re-parses every one of them and compares. A bug shared by both would have to be made
twice.

**A checker that reads part of its source is worth what it reads, and no more.** Every one of these
started out reading less than it appeared to, and in each case the gap was found by mutation
testing rather than by review: `check_recipes.py` read 2,545 of 3,090 recipes because the rest are
built by helpers, counted loops and reassigned locals rather than written literally;
`check_placed_items.py` read 1,027 of 3,129 placement pairs because 889 items place through a
helper and ~120 more compute the field from `type`. Both read all of theirs now, and both kill 100%
of their mutants, but neither would have without `just check-mutants` saying so. Write the checker,
then make it fail on purpose.

**Use `static`, not `const`, for the large ones.** A `const` array is copied at every use site.
Clippy catches this; it is worth knowing why rather than just applying the fix.

**Some sources are programs, and the generator has to be an interpreter.** `TileObjectData
.Initialize` declares nothing: it mutates one shared object field by field, stamps it into a slot,
resets it from a base, and does that 389 times over 2,900 lines. A regex sweep cannot read it, and
the hand transcription that stood in for one had eight fields right out of thirteen and the other
five filled in with a uniform guess (`style_multiplier` 2, `style_wrap` 2, `style_line_skip` 2)
that matched the game on 15 entries of 389. So `tile_object` is a small interpreter for the fifteen
statement forms that method uses, and getting it right meant modelling three things a value-semantic
reading skips: `addTile` resets the current object, `CopyFrom` shares modules by reference through a
copy-on-write, and a setter given the value already in place returns *before* invalidating the
memoised `Calculate`. Where a table's source is a program, write the second reading too
(`tools/check_tile_object.py` here) - a single interpreter of 2,900 lines of mutation is not a thing
to trust once.

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
