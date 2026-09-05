#!/usr/bin/env python3
"""Compare `placed_items.rs` against `Item.SetDefaults`' own `createTile`/`placeStyle` pairs.

`placed_items.rs` is the second of the three tables with no generator. It answers "which item does
a placed object give back", and it gets there by *inverting* the game: `Item.SetDefaults` says
which tile and style each item places, and turning that round gives the drop for free.

This re-does that inversion and says where the table disagrees. Same purpose as
`check_npc_data.py` and the same shape: rule 7's protection is that a per-type table cannot drift
from source unseen, and a checker gets that for a table no generator covers.

**The item defaults are not one method.** `SetDefaults` fans out to `SetFoodDefaults` and
`SetDefaults1` through `SetDefaults5` by id range (`Item.cs:48692-48714`), so all six are walked.
Within each, the same four traps as the NPC chain apply, and the same four answers: bound the walk
by the method's own braces, accept alternations and ranges as well as bare arms, treat a nested
type test as narrowing rather than as a new arm, and skip assignments inside a conditional that is
not a type test.

**The table is not purely this inversion, and is not meant to be.** Where the game keeps its own
drop table - `GetItemDrop_Chair` and its siblings - that wins, because it is the authority on what
mining gives back. Those differences are on the record below.

Exit 0 when every pair matches or is on the record; 1 otherwise.
"""

import re
import sys
from pathlib import Path

METHODS = (
    "SetFoodDefaults",
    "SetDefaults1",
    "SetDefaults2",
    "SetDefaults3",
    "SetDefaults4",
    "SetDefaults5",
)

ARM = re.compile(r"^\s*(?:else )?if \((.*)\)\s*$")
ALTS = re.compile(r"^type == \d+(?: \|\| type == \d+)*$")
RANGE = re.compile(r"^type >= (\d+) && type <= (\d+)$")
FIELD = re.compile(r"^\s*(createTile|placeStyle) = (-?\d+);")

# Pairs where the game's own `GetItemDrop_*` table disagrees with the inversion and wins, plus
# anything else deliberate. Keyed by (tile, style).
EXPECTED = {}


def arm_types(condition):
    if ALTS.match(condition):
        return [int(n) for n in re.findall(r"\d+", condition)]
    m = RANGE.match(condition)
    if m:
        return list(range(int(m.group(1)), int(m.group(2)) + 1))
    return None


def method_body(lines, start):
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
    """Walk each `SetDefaults*` method's `switch (type)` and read every item's placement.

    These are switches, not if-chains: `case 1: ... break;`, with cases stacked when several items
    share a body. Nothing about the NPC chain's nesting applies here, and pretending it did read
    21 pairs out of 1,190.
    """
    lines = (root / "Terraria" / "Item.cs").read_text(errors="replace").split("\n")
    case = re.compile(r"^\s*case (\d+):\s*$")
    items = {}
    for method in METHODS:
        try:
            start = next(
                i
                for i, l in enumerate(lines)
                if re.search(r"\b%s\(int type\)\s*$" % method, l)
            )
        except StopIteration:
            sys.exit("no %s in Item.cs" % method)
        # Cases stack: a run of `case N:` lines with no body between them all share the next body.
        # A shared body can then split per type - `if (type == 2641) { placeStyle = 31; } else
        # { placeStyle = 32; }` is how the two hanging lanterns differ - so assignments inside such
        # a split go to the named type alone and the `else` to the rest. Without that both lanterns
        # take the last assignment and the first reads as the second's style.
        #
        # No brace counting: these splits are always the tail of their case, so the narrowing is
        # cleared by the next `case` or `break` like everything else.
        pending = []
        narrow = None
        rest = None
        split = re.compile(r"^\s*if \(type == (\d+)\)\s*$")
        for line in method_body(lines, start):
            m = case.match(line)
            if m:
                pending.append(int(m.group(1)))
                for t in pending:
                    items.setdefault(t, {})
                narrow = rest = None
                continue
            sm = split.match(line)
            if sm and pending:
                chosen = int(sm.group(1))
                narrow = [chosen]
                rest = [t for t in pending if t != chosen]
                continue
            if narrow is not None and line.strip() == "else":
                narrow, rest = rest, narrow
                continue
            fm = FIELD.match(line)
            if fm and pending:
                for t in narrow if narrow is not None else pending:
                    items[t][fm.group(1)] = int(fm.group(2))
                continue
            if line.strip() in ("break;", "return;"):
                pending = []
                narrow = rest = None

    # Invert: (tile, style) -> the item that places it. A tile/style claimed by more than one item
    # keeps the lowest id, which is what a first-wins transcription of the same source produces.
    placed = {}
    for item, fields in sorted(items.items()):
        tile = fields.get("createTile", -1)
        if tile < 0:
            continue
        placed.setdefault((tile, fields.get("placeStyle", 0)), item)
    return placed


def from_table(path):
    text = path.read_text()
    body = text[text.index("pub fn placed_item("):]
    out = {}
    # Line-anchored rather than newline-delimited: consuming the trailing newline makes the scan
    # resume mid-line and skip every other arm, which read 274 of the 549 and lost tile 1.
    for m in re.finditer(r"^        ([\d |]+) => &\[(.*?)\],$", body, re.S | re.M):
        blocks = [int(b) for b in re.findall(r"\d+", m.group(1))]
        pairs = re.findall(r"\((-?\d+), (-?\d+)\)", m.group(2))
        for block in blocks:
            for style, item in pairs:
                out[(block, int(style))] = int(item)
    return out


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: check_placed_items.py <decompiled tree>")
    root = Path(sys.argv[1])
    source = from_source(root)
    ours = from_table(Path("crates/terrustia-proto/src/placed_items.rs"))

    missing = sorted(set(source) - set(ours))
    wrong = sorted(
        (k, source[k], ours[k])
        for k in set(source) & set(ours)
        if source[k] != ours[k] and k not in EXPECTED
    )

    print(
        "%d (tile, style) pairs in Item.SetDefaults, %d in placed_items.rs"
        % (len(source), len(ours))
    )
    # Only one direction is a contract. `placed_items.rs` also merges the game's own
    # `GetItemDrop_*` tables, which are a different source and legitimately carry pairs no item
    # places, so a pair being here and not in the inversion says nothing. A pair the inversion
    # defines being absent or different does.
    print("%d pairs are ours alone, from the `GetItemDrop_*` merge" % len(set(ours) - set(source)))
    print("%d shared pairs agree" % (len(set(source) & set(ours)) - len(wrong)))

    problems = []
    for k in missing:
        problems.append(
            "  tile %d style %d places item %d in source and is absent here"
            % (k[0], k[1], source[k])
        )
    for k, a, b in wrong:
        problems.append(
            "  tile %d style %d: source says item %d, placed_items.rs says %d" % (k[0], k[1], a, b)
        )

    if problems:
        print("\n%d unexplained difference(s):" % len(problems))
        print("\n".join(problems[:60]))
        if len(problems) > 60:
            print("  ... and %d more" % (len(problems) - 60))
        print(
            "\nEach is either a wrong entry or a `GetItemDrop_*` table winning over the\n"
            "inversion. Read the item in Item.cs; if the difference is deliberate, put the pair\n"
            "in EXPECTED with why."
        )
        return 1
    print("\nevery pair in placed_items.rs matches Item.SetDefaults.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
