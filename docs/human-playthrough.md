# The human playthrough

The one clause on the release bar that no automation closes, written down so it takes an evening
rather than a weekend, and so what it finds becomes tests rather than memories.

`TODO.md` calls it "strongly expected, waivable only if the automated and differential evidence is
otherwise complete". The waiver's condition is currently satisfied
(`docs/release-blockers.md`), which is not the same as the playthrough having happened. This file
is for the person doing it.

## Why a person, when the loot spine already passes

`cargo run --release -p terrustia-client --example playthrough` walks Eye of Cthulhu to the Moon
Lord and checks every drop, and it passes. `playbot` reaches thirteen goals across a save and a
reload. Neither of them can see the things this list is about: a wrong sprite, a sound that never
plays, an NPC that stands in a wall, a chest that opens into the wrong UI, a biome that reads as
the wrong place. Every automated check here compares the server against what this project believes
the game does. A person compares it against the game.

The sharpest reason, from this project's own record: eleven separate checkers, tools and documents
have been found reporting success while not testing what they claimed, and four production bugs
came out of chasing them - including a smoothing pass that silently deleted every multi-tile
object it touched, and a dungeon that had never once placed the door Skeletron guards. Each of
those was invisible to a green test suite. A person playing for two hours is a different kind of
instrument.

## Setup

```sh
just build                       # release binaries, panel embedded
./target/release/terrustia --new "Playthrough"
```

Then in Terraria: **Multiplayer -> Join via IP -> 127.0.0.1**, port 7777. Use a **fresh
character**, not one carrying gear: half of what this is looking for only shows up when
progression actually gates you.

Keep the server's console visible. A warning there while something looks wrong on screen is worth
more than either alone.

## What to look at, in the order you will meet it

Tick these off. Anything that fails becomes a test before it becomes a fix - that is the project's
own rule and it is why this list is worth keeping.

**The first five minutes**
- [ ] Spawn is somewhere you can stand, with ground under you and sky above
- [ ] The surface reads as a forest: grass, trees, the right backdrop for the biome
- [ ] Breaking and placing a block does what you expect, and the neighbours reframe
- [ ] A chest opens, holds what it should, and closes
- [ ] Day turns to night and the music and backdrop follow

**The world as a place**
- [ ] Each biome reads as itself from inside it - jungle, snow, desert, ocean, evil
- [ ] The Underground Desert is sandstone chambers, not a rectangle of sand
- [ ] A floating island is reachable and has its house and chest
- [ ] The dungeon entrance exists, and its door is **locked** until you use a Golden Key
- [ ] The temple is sealed and its traps are armed
- [ ] Water flows and settles; lava is where lava belongs

**Progression, boss by boss**
- [ ] Eye of Cthulhu can be summoned, fought, and drops
- [ ] The evil biome's orbs break and the third one summons
- [ ] Wall of Flesh is fightable in the underworld and turns the world hardmode
- [ ] Altars break and seed the hardmode ores
- [ ] The mechanical three, then Plantera, then Golem
- [ ] Lunatic Cultist, the pillars, the Moon Lord

**The parts a bot cannot judge**
- [ ] NPCs move like the game's NPCs, not like something following a rule
- [ ] Housing is accepted or rejected for the reasons the game gives
- [ ] Nothing renders as a corrupt sprite, anywhere
- [ ] Nothing is silent that should make a sound
- [ ] The server's log stays quiet the whole time

**Afterwards**
- [ ] `/save`, quit, reload, and the world is exactly as you left it
- [ ] The panel at `127.0.0.1:7778` shows the session that just happened

## What to do with what you find

Write it down before you fix it. A note in `docs/release-blockers.md` under "Known bugs, found and
deliberately not fixed" is worth more than a silent patch, and a failing test written first is
worth more than both. If nothing is found, say so in `release-blockers.md` and date it: "nothing
found" is evidence too, and it is the only line in this file that can retire the clause.
