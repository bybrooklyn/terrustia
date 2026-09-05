#!/usr/bin/env python3
"""Compare `npc_data.rs` against `NPC.SetDefaults`, entry by entry and field by field.

`npc_data.rs` is the largest table in the project and one of three with no generator: 691 entries
transcribed by hand from a 9,400-line `if (type == N)` chain, with `AGENTS.md` rule 7 explicitly
not covering it. Rule 7 exists because a hand-written per-type table goes stale invisibly - nothing
errors, a few types just quietly behave like the wrong creature.

This closes that without a risky 13,000-line replacement: it re-reads the chain and says which
entries disagree. Run from `just check-data`, beside the drop and recipe checkers, for the same
reason and against the same kind of drift.

**What the chain actually looks like**, because a naive reading of it is wrong in four separate
ways and each one cost a debugging round:

* The walk must be bounded by `SetDefaults`' own braces. `NPC.cs` has many later methods that also
  branch on `type ==`, and an unbounded walk lets the last arm anywhere in the file win - which
  reads every stat as zero.
* Arms come in three shapes, not one: a bare `type == N`, an alternation of them, and a
  `type >= A && type <= B` range. Taking only bare arms misses 96 entries.
* A `type ==` head *inside* an arm narrows the set for its own block rather than starting a new
  arm: the scarecrow range sets the shared stats and five paired heads inside it refine each pair.
* Assignments inside a non-type conditional - `if (Main.remixWorld)`, `if (!Main.hardMode)` -
  describe a world this table does not speak for and must be skipped, or the Lava Slime reads as
  its remix-seed self.

Exit 0 when every entry matches or is on the record below; 1 otherwise.
"""

import re
import sys
from pathlib import Path

FIELDS = {
    "width": "width",
    "height": "height",
    "aiStyle": "ai_style",
    "damage": "damage",
    "defense": "defense",
    "lifeMax": "life_max",
    "knockBackResist": "knockback_resist",
    "value": "value",
    "npcSlots": "npc_slots",
    "noGravity": "no_gravity",
    "noTileCollide": "no_tile_collide",
    "friendly": "friendly",
    "townNPC": "town_npc",
    "boss": "boss",
    "lavaImmune": "lava_immune",
    "dontTakeDamage": "dont_take_damage",
}

# What `SetDefaults`' preamble leaves every field at before the chain runs. `width` and `height`
# are absent from the preamble on purpose: the game genuinely does not reset them, so a type whose
# arm sets neither inherits whatever the recycled `NPC` slot last held. See `EXPECTED` for the one
# type that reaches.
DEFAULTS = {
    "width": 0, "height": 0, "ai_style": 0, "damage": 0, "defense": 0, "life_max": 0,
    "knockback_resist": 1.0, "value": 0.0, "npc_slots": 1.0,
    "no_gravity": False, "no_tile_collide": False, "friendly": False,
    "town_npc": False, "boss": False, "lava_immune": False, "dont_take_damage": False,
}

# Differences that are correct and deliberate. Each names what it stands in for; anything not on
# this list is drift and fails the check.
EXPECTED = {
    (422, "boss"): "Lunar Tower (Vortex): `boss` stands in for `NPC.DoesntDespawnToInactivity()`",
    (493, "boss"): "Lunar Tower (Stardust): the same",
    (507, "boss"): "Lunar Tower (Nebula): the same",
    (517, "boss"): "Lunar Tower (Solar): the same",
    (453, "town_npc"): "Skeleton Merchant: `town_npc` stands in for `NPC.isLikeATownNPC`",
    (664, "width"): "Torch God: `SetDefaults` sets no size at all and the preamble does not reset "
                    "one, so vanilla inherits the recycled slot's; 16x16 is this project's own",
    (664, "height"): "Torch God: the same",
}

ARM = re.compile(r"^\s*(?:else )?if \((.*)\)\s*$")
ALTS = re.compile(r"^type == \d+(?: \|\| type == \d+)*$")
RANGE = re.compile(r"^type >= (\d+) && type <= (\d+)$")
INT = re.compile(r"^\s*(width|height|aiStyle|damage|defense|lifeMax) = (-?\d+);")
FLOAT = re.compile(r"^\s*(knockBackResist|value|npcSlots) = (-?[0-9.]+)f?;")
BOOL = re.compile(
    r"^\s*(noGravity|noTileCollide|friendly|townNPC|boss|lavaImmune|dontTakeDamage) = (true|false);"
)


def arm_types(condition):
    """Which types one conditional head covers, or None if it is not selecting a type at all."""
    if ALTS.match(condition):
        return [int(n) for n in re.findall(r"\d+", condition)]
    m = RANGE.match(condition)
    if m:
        return list(range(int(m.group(1)), int(m.group(2)) + 1))
    return None


def method_body(lines, start):
    """The lines of one method, bounded by its own braces."""
    depth = 0
    seen = False
    for i in range(start, len(lines)):
        depth += lines[i].count("{") - lines[i].count("}")
        if lines[i].count("{"):
            seen = True
        yield lines[i]
        if seen and depth == 0:
            return


def from_source(root):
    lines = (root / "Terraria" / "NPC.cs").read_text(errors="replace").split("\n")
    try:
        start = next(
            i for i, l in enumerate(lines) if "public void SetDefaults(int Type" in l
        )
    except StopIteration:
        sys.exit("no SetDefaults in NPC.cs")

    out = {}
    scopes = []  # (types, the depth this scope was opened at)
    depth = 0
    for line in method_body(lines, start):
        m = ARM.match(line)
        types = arm_types(m.group(1).strip()) if m else None
        if types is not None:
            for t in types:
                out.setdefault(t, dict(DEFAULTS))
            scopes.append((types, depth))
            depth += line.count("{") - line.count("}")
            continue
        opened, closed = line.count("{"), line.count("}")
        # Inside a conditional the arm carries, rather than the arm itself.
        suppressed = bool(scopes) and depth > scopes[-1][1] + 1
        if scopes and not suppressed and opened == 0 and closed == 0:
            for rx, cast in ((INT, int), (FLOAT, float), (BOOL, lambda v: v == "true")):
                mm = rx.match(line)
                if mm:
                    for t in scopes[-1][0]:
                        out[t][FIELDS[mm.group(1)]] = cast(mm.group(2))
                    break
        depth += opened - closed
        while scopes and depth <= scopes[-1][1]:
            scopes.pop()
    return out


def from_table(path):
    out = {}
    text = path.read_text()
    for m in re.finditer(r"\n        (\d+) => NpcStats \{(.*?)\n        \},", text, re.S):
        entry = dict(DEFAULTS)
        for fm in re.finditer(r"(\w+): ([^,\n]+),", m.group(2)):
            key, raw = fm.group(1), fm.group(2).strip()
            if key not in DEFAULTS:
                continue
            if raw in ("true", "false"):
                entry[key] = raw == "true"
            else:
                try:
                    # `1e+06` and `0.0` alike: every numeric field parses as a float, and the
                    # integer ones compare equal to their own value either way.
                    entry[key] = float(raw)
                except ValueError:
                    pass
        out[int(m.group(1))] = entry
    return out


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: check_npc_data.py <decompiled tree>")
    root = Path(sys.argv[1])
    # Relative to this file rather than the working directory, so a copy of this checker beside a
    # copy of the table reads *that* table. `mutate_tables.py` depends on it.
    repo = Path(__file__).resolve().parent.parent
    table = repo / "crates/terrustia-proto/src/npc_data.rs"
    source = from_source(root)
    ours = from_table(table)

    problems = []
    for npc in sorted(set(source) | set(ours)):
        if npc not in ours:
            problems.append(f"  npc {npc} is in SetDefaults and not in npc_data.rs")
            continue
        if npc not in source:
            problems.append(f"  npc {npc} is in npc_data.rs and not in SetDefaults")
            continue
        for field in DEFAULTS:
            a, b = source[npc][field], ours[npc][field]
            if isinstance(a, bool) or isinstance(b, bool):
                same = bool(a) == bool(b)
            else:
                same = abs(float(a) - float(b)) < 1e-6
            if same or (npc, field) in EXPECTED:
                continue
            problems.append(
                f"  npc {npc} {field}: SetDefaults says {a}, npc_data.rs says {b}"
            )

    print(f"{len(source)} types in SetDefaults, {len(ours)} in npc_data.rs")
    print(f"{len(EXPECTED)} differences on the record:")
    for (npc, field), why in sorted(EXPECTED.items()):
        print(f"  npc {npc} {field}: {why}")
    if problems:
        print(f"\n{len(problems)} unexplained difference(s):")
        print("\n".join(problems))
        print(
            "\nOne of the two is wrong. Read the arm in NPC.cs and fix whichever it is; if the\n"
            "difference is deliberate, put it in EXPECTED with what it stands in for."
        )
        return 1
    print("\nevery entry in npc_data.rs matches NPC.SetDefaults.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
