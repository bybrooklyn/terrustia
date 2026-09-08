# Checking against a real Terraria client

Everything else in this repository is checked by tests. This one thing cannot be, and it is worth
being blunt about why.

## The blind spot

`terrustia-client` depends on `terrustia-proto` — the same crate the server encodes with:

```toml
# crates/terrustia-client/Cargo.toml
terrustia-proto = { path = "../terrustia-proto" }
```

So when a live test reports that all 691 NPC types arrived and synced, what it has proved is that
**our client and our server agree with each other**. If `terrustia-proto` reads a field at the
wrong width, or in the wrong order, both sides do it identically, the bytes match, and the test
passes. A shared misreading is invisible to every test we can write.

The audits found four defects of exactly this shape sitting behind a green suite — a tile id off
by six, ore tiers shifted one slot along, a dungeon coordinate that was silently the surface. None
of them could have been caught by testing against ourselves.

Terraria's own bytes owe nothing to this code. They are the only independent opinion available.

## Recording a session

```sh
cargo run --release -- --record capture.trcap
```

Then in Terraria: **Multiplayer → Join via IP → `127.0.0.1`**, port `7777`.

Play for two or three minutes and try to touch each thing the server has to get right. In rough
order of how much each one proves:

1. **Join, and wait for the world to finish loading.** This alone exercises the handshake, packet
   7 field by field, and the compressed tile sections. If any of those is wrong the client will
   not reach the world at all.
2. **Walk somewhere you have not been**, far enough that new sections stream in.
3. **Dig a few blocks, and place a few.**
4. **Open a chest**, take something out, put something back.
5. **Talk to a town NPC**, and open their shop.
6. **Hit something**, and let something hit you.
7. **Disconnect and rejoin**, which replays the whole join against a world that now has state.

Stop the server with Ctrl-C. The capture is closed on the way out.

## Reading the result

```sh
cargo run --release -p terrustia --example replay -- capture.trcap
```

It re-frames both directions of every connection and reports what it found:

```
  9 chunks, 55972 bytes, 2 stream(s), 0.6s

  slot 0 server: 25 frames, nothing left over
  slot 0 client: 8 frames, nothing left over

what the client sent:
        1    1  Hello
        1    4  SyncPlayer
        ...
```

Three things matter in that output.

- **"nothing left over"** on the client stream. Terraria's framing is
  `[u16 length][u8 id][payload]`, and a length misread by even one byte desynchronises everything
  after it. A stream that divides cleanly into whole frames is the strongest single signal in the
  file.
- **No message id outside the table.** Note that `Unknown42` and `Unknown68` are *named* entries —
  Terraria's own `MessageID.cs` calls them that — and both are handled. A bare `Unknown` is the
  real miss.
- **The census.** A capture records one session, so a message that does not appear proves nothing
  either way; the census is there to say what the session actually covered.

The tool exits non-zero if the stream desynchronised, if an unknown id arrived, or if the server
never sent one of the four packets the client blocks on (`PlayerInfo`, `WorldData`, the tile
sections, `FinishedConnectingToServer`) — in which case the client cannot have entered the world,
whatever it looked like on screen.

## Keeping it

A capture is worth checking in. It is a few tens of kilobytes, it holds bytes this project did not
produce, and replaying it in CI turns "somebody once connected the real game and it seemed fine"
into something that stays checked.

## What has actually been run

The recorder and the replay tool have been exercised end to end against `terrustia-client`, which
proves the plumbing: 9 chunks, 55,972 bytes, both streams re-framing with nothing left over.

**A real Terraria client has been connected, and it found a bug no test of ours could.** The server
was broadcasting message id 18 (`TimeSet`) every sixty seconds, which real vanilla's server never
sends at all: grepping the whole decompiled tree finds zero `SendData(18)` calls by anyone. The
client's own handler assigns the time with no smoothing, so a player watched the sky snap visibly
to a different time of day. Every test this project had asserted only that our own encoder agreed
with our own claim, and one of them had codified the bug outright. Fixed in `fc2f190`; the account
is in `AUDIT.md`.

**A real `TerrariaServer` has also been recorded and re-encoded**, which is the other direction:
66,542 bytes of Re-Logic's own output, re-framed with nothing left over, and every id with an
encoder coming back byte-identical, all 15 `TileSection` frames included.

**What is still open is checking a capture in.** Both runs were one-off and their bytes live under
`.scratch/`, out of the tree on purpose (rule 2 keeps game-derived data out). Nothing replays them
on a schedule, so "somebody once connected the real game and it was fine" is still what the record
rests on between sessions.
