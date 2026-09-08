# Buffs and debuffs on NPCs

Almost every weapon past the first hour of the game inflicts something. On Fire!, Ichor, Venom,
Betsy's Curse, Daybreak — these are not decoration. A good fraction of a late-game player's damage
arrives this way, and three of them lower armour, which changes what every *other* hit is worth.

A server that drops them makes the whole second half of the game feel wrong without ever looking
broken. This one did, until recently: the packet a client sends when its weapon lands an effect
was not handled at all, so no enemy in the world had ever been on fire.

**Code:** [`game/buffs.rs`](../crates/terrustia/src/game/buffs.rs),
[`terrustia-proto/src/buffs.rs`](../crates/terrustia-proto/src/buffs.rs) (generated).

## The three separate things

The game keeps these apart and so does this port.

### The slots

Twenty per NPC. `Buffs::add` fills them, and the eviction rule is worth stating because it is not
obvious:

> When all twenty are taken, the *first* slot holding something that is **not** a debuff is
> dropped to make room. If every slot holds a debuff, the new one is refused.

So a boss cannot be talked out of its poison, and a player cannot displace one by piling on
blessings. `Main.debuff[]` is what decides which is which; it is generated.

### The flags

Each tick the slots are read into a set of booleans and every timer counts down. The game resets
and re-derives on every update rather than maintaining them incrementally, so a buff that runs out
cannot leave its effect behind. This port does the same, with the three dozen booleans gathered
into one struct so the reset is one assignment rather than thirty-eight that can be forgotten one
at a time.

### The toll

`Buffs::dots`, a port of `NPC.DOTTally`. The shape is not "N damage per second":

1. Each active debuff adds to a **life-regeneration figure**.
2. That figure is added to a **running total** each tick.
3. A hit lands each time the total crosses **120**.

A debuff worth 12 therefore deals one point every ten ticks, and two of them deal one every five.
That is why poison ticks unevenly rather than once a second, and why stacking debuffs feels smooth
rather than stepped.

`expected_dps` is the game's own smoothing: once a debuff declares one, damage is dealt in larger,
rarer lumps, so a Daybreak spear does not send a hundred packets a second.

## Armour penetration is the client's job, and that is correct

This is the part most likely to look like a bug.

Ichor, Broken Armour and Betsy's Curse all lower the target's effective armour. **This server does
not subtract anything for them**, and it should not. The client adds its armour penetration into
the damage number it sends (`Player.cs:44765`), and the server applies plain defence to that
number:

```rust
max(1, damage - defense * 0.5) * (if crit { 2 } else { 1 })
```

which is exactly `Main.CalculateDamageNPCsTake`.

What the server owes the client is the **buff list** — and that is what was missing. Every client
was computing its penetration against a target it believed was clean. Sending packet 54 is what
makes ichor work.

## Immunity is the server's decision

Never the client's. `npc_is_immune` is generated from `NPCID.Sets.DebuffImmunitySets`: 697 types
over 34 distinct masks, interned because far fewer than 697 distinct sets exist.

Three corrections `NPC.SetDefaults` applies on top of the table are baked into the generated
masks rather than applied at runtime:

- immunity to poison (20) implies bleeding (30) and hemorrhage (375)
- immunity to ichor (69) implies broken armour (36)
- shimmer immunity (353) is set from its own list either way

King Slime does not take poison however politely a client asks.

## The stacking debuffs

Five are not a rate at all but a count of what is stuck in the target:

| Buff | Projectile | Per second | Divider |
|---|---|---|---|
| Javelin | 598 | 3 | 1 |
| Tentacle Spike | 971 | 3 | 1 |
| Blood Butchered | 975 | 4 | 1 |
| Daybreak | 636 | 100 | 4 |
| Celled (Stardust) | 614 | 20 | 1 |

The game's test is `ai[0] == 1 && ai[1] == whoAmI` — the first says the projectile has stuck
rather than still flying, the second says what it stuck in. The projectile table is counted once
per tick rather than once per NPC per debuff.

## Packets

| Id | Direction | What |
|---|---|---|
| 53 | client → server | "my weapon inflicted this". The server decides immunity |
| 54 | server → clients | the whole buff list of one NPC, zero-terminated |
| 137 | client → server | "take this off". **Refused for every buff** — see below |
| 153 | server → clients | damage a debuff did, in its own colour, credited to nobody |

Packet 137 is refused for every buff, and that is the game's behaviour rather than a gap: it
validates against `BuffID.Sets.CanBeRemovedByNetMessage`, which is **empty** in 1.4.5.8 too
(`Terraria.ID/BuffID.cs:24` is a bare `Factory.CreateBoolSet()`, so every entry is false). Reading
the packet still matters — several arrive in one batch and skipping its bytes would misparse
whatever follows.

## A naming trap

`NPC` type 1 is the **Blue Slime**, not King Slime (which is 50). Its `ai[1]` is not a phase
number but the **id of the item it swallowed** — the 1.4 "slime with something inside" mechanic.
It reaches into the debuff arithmetic in three places:

- swallowed item 8 (a torch) stops it burning in a Ravaged world
- swallowed item 9 makes it re-light itself every second
- swallowed items 29, 364, 365, 366, 1104, 1105, 1106 make it regenerate

Reading `ai[1]` as a phase here would be wrong in a way that looks right. The constants are named
`SWALLOWED_*` for that reason.
