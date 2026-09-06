# The `.wld` file

How this server reads and writes Terraria's world format.

**Code:** [`world/wld.rs`](../crates/terrustia/src/world/wld.rs) (read),
[`world/wld_save.rs`](../crates/terrustia/src/world/wld_save.rs) (write).

## Layout

```
i32     format version          326 for 1.4.5.8; this reader accepts 279..=326
[7]u8   "relogic"
u8      file type               2 = world
u32     revision                incremented on every save
u64     favourite
i16     section count           11 in current formats
[n]i32  section pointers        absolute byte offsets
u16     frame-importance count
[..]    frame-importance bitset, least significant bit first
```

Then the sections, at the offsets named:

| # | Contents | This server |
|---:|---|---|
| 0 | World header — name, size, seed, clock, progress flags | **preserved verbatim**, patched by offset |
| 1 | Tiles, column-major, run-length encoded | re-encoded |
| 2 | Chests | re-encoded |
| 3 | Signs | re-encoded |
| 4 | Townsfolk | carried through |
| 5 | **Tile entities** | **re-encoded from state** |
| 6 | Pressure plates held down | carried through |
| 7 | Room assignments | carried through |
| 8 | Bestiary | carried through |
| 9 | Creative powers | carried through |
| 10 | Footer — `true`, world name, world id | carried through |

## Why the header is preserved verbatim

It has grown a field at a time across dozens of format versions, and which fields are present
depends on the version. Re-serialising it means knowing every one of them; preserving it and
patching the handful the server can change means knowing only those.

Each patchable field's byte offset is recorded when the world is read. A `None` offset means that
world's header never reached the field, and the value lives only for the session.

Patched on save: the clock, the moon phase, the progress flags, hardmode, the altar count, the orb
count, rain, wind, the sandstorm, the Old One's Army tiers, and both combat books.

## Why section 5 is not carried through

Everything from section 4 onward used to be one opaque blob copied byte for byte. That is wrong
for tile entities: a pylon placed while the server was running is not in the bytes that were
loaded, and one that was mined still is. Copying it back means the world remembers the pylons it
had when it was **opened** and nothing since.

So the trailing region is now sliced into one blob per section, and section 5 alone is written
from the server's own state. Because that section can change length, the pointers can no longer be
a single shift — each is taken from where its section actually lands.

## Verifying a round trip

```sh
cargo run --release -p terrustia --example roundtrip_wld -- in.wld out.wld
```

This compares *our parse* of the original against *our parse* of the re-save. That is a real
check, but it has a blind spot: **a field our parser ignores is ignored identically on both sides,
so the check passes while the field is lost.**

When a round trip looks suspicious, decode both files with something written from the format
rather than from this code. A played world came back 168 KB smaller than it went in; an
independent decoder found **zero differences across all 20,160,000 tiles**, and every other
section came back byte for byte the same size. Our run-length encoder simply merges runs the
original did not, which is legal and lossless.

Worth remembering: a size change is not evidence of a problem, and an equal size is not evidence
of correctness.

## Tile encoding traps

- Tiles are **column-major** in the file and **row-major** in network sections.
- The wall's high byte is on `flags3 & 0x40`, not on flags2.
- Run length is `(flags1 & 0xC0) >> 6`: 0 none, 1 a byte, 2 *and 3* a short.
- Shimmer rides the water slot with `flags3 & 0x80` to distinguish it.
- The file carries **its own** frame-importance table, and a save must be written with the table
  it declares or the file will not read back.

## Chests before and after format 294

Before 294 every chest had the same capacity, written once for the whole section. Since 294 each
carries its own. Which it was has to be remembered, because saving writes the section back in the
file's own shape.
