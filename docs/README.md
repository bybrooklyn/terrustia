# Documentation

Notes on how this server works and why it works that way. Each file covers one area and is
written to be read before touching the code it describes.

| File | What it covers |
|---|---|
| [protocol-notes.md](protocol-notes.md) | The wire format: frame layout, the handshake, and the packets whose shape is easy to get wrong |
| [real-client.md](real-client.md) | Why no test here can prove protocol correctness, and the capture-and-replay that can |
| [packet-coverage.md](packet-coverage.md) | Which of Terraria's 163 message ids (143 live) this server handles, which it does not, and why |
| [buffs.md](buffs.md) | Debuffs on NPCs: the twenty slots, damage-over-time, and why armour penetration is the client's job |
| [tile-entities.md](tile-entities.md) | The furniture that remembers something — pylons, item frames, mannequins — and its two serialised forms |
| [teleports.md](teleports.md) | The five items that ask the server to move a player, and how a safe landing spot is found |
| [wiring.md](wiring.md) | Circuits, the Grand Design's L-shaped path, and the limits this server adds that the game does not |
| [performance.md](performance.md) | The tick budget, where the time goes, and the autosave stall that hid behind a wrong guess |
| [shimmer.md](shimmer.md) | Transmutation and decrafting: what turns into what, why it takes a second and a half, and why a cascade is not a loop |
| [world-file.md](world-file.md) | The `.wld` format as this server reads and writes it, including what is preserved verbatim |
| [worldgen.md](worldgen.md) | How a playable world is built, and four things that were got wrong first |
| [worldgen-parity.md](worldgen-parity.md) | The separate, much longer job of making a seed match Terraria's |
| [generated-tables.md](generated-tables.md) | Which source files are generated, from what, and how to regenerate them |
| [release-blockers.md](release-blockers.md) | What is not done for v0.0.1: the release bar clause by clause, the known bugs left in on purpose, and the documents that were found lying |

## The rule the whole codebase follows

**Per-type variation lives in generated tables. Hand-written modules hold algorithms only.**

There are 697 NPC types, 754 tiles, 401 buffs and several thousand items. Any rule that differs
per type is data, and data belongs in a file generated from the game's own tables — because a
hand-written match over 697 cases is wrong the moment the game changes, and wrong invisibly.

The generators live in [`tools/`](../tools) and each one names its source. See
[generated-tables.md](generated-tables.md).

## How to check something is right

In rough order of how much it proves:

```sh
./tools/test_summary.sh     # the whole suite, honestly counted — see below
cargo test                  # unit and integration tests
cargo clippy --all-targets  # kept at zero warnings
cargo run --release -p terrustia --example bestiary -- 127.0.0.1:7777    # every NPC type, live
cargo run --release -p terrustia --example fuzz -- 127.0.0.1:7777        # malformed packets
cargo run --release -p terrustia --example crowd -- 127.0.0.1:7777       # many players at once
cargo run --release -p terrustia --example stress -- 127.0.0.1:7777      # the tick budget
cargo run --release -p terrustia --example roundtrip_wld -- in.wld out.wld
cargo run --release -p terrustia --example genparity -- reference.wld
cargo run --release -p terrustia --example playable  -- world.wld    # can it be finished?
python3 tools/packet_audit.py               # every message id's table row checked against the code
python3 tools/packet_audit.py --write-doc   # and regenerate packet-coverage.md from the table

cargo run --release -- --record capture.trcap                        # then connect real Terraria
cargo run --release -p terrustia --example replay -- capture.trcap   # and check what it sent
```

The last two are the only ones on this list that check this server against something it did not
write. Everything above them compares `terrustia-proto` with `terrustia-client`, which is built on
`terrustia-proto` — so a field read at the wrong width passes both. See
[real-client.md](real-client.md).

A tick has 16,666 µs to spend. Anything approaching that is a bug, not a tuning problem.

**Use `tools/test_summary.sh` rather than eyeballing `cargo test`.** `cargo test` stops at the
first failing target, so counting "N passed" lines both misses the failure and silently omits
every target after it. That is not hypothetical: a summary here once read 1,052 passing while a
handshake test was red and the entire proto crate had never run. The script runs every target,
reports how many failed, and exits non-zero when any did.
