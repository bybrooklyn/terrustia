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

**The table is not purely this inversion, and is not meant to be.** The game also keeps its own
answer for "what does breaking this give", in two places, and where they speak they win, because
they are what it actually runs:

* the two dozen `GetItemDrop_*` methods (`WorldGen.cs:40125-43497`), one per furniture family, and
* six arms written inline in the shape validators: banners (`WorldGen.cs:46572`) and the
  five painting sizes (`Check3x3Wall` through `Check6x4Wall`).

**Every one of those readings was added because this checker was mutation-tested, not because
anyone read it.** `just check-mutants` corrupts one entry of the table at a time and requires a
failure; each surviving mutant is a row the checker cannot see. It killed 43% of them when it read
only the literal `createTile = N;` assignments, and each widening since - the placement helpers,
the drop methods, the inline arms, and the ~120 assignments that compute the field from `type` -
closed a class of survivor and turned up real defects on the way. It kills 100% now.

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
# **These are not always literals.** A shared case body covering a run of item types computes the
# field from `type`: `placeStyle = 1 + type - 3046;` is the five campfires, and there are about 120
# more like it, including `(type - 2612) * 2` and one that sums comparisons. Reading only `= N;`
# missed every pair they define.
FIELD = re.compile(r"^\s*(createTile|placeStyle) = ([^;]+);")

# **Most items do not write those two fields at all.** They call a helper that writes them, and
# there are 889 `DefaultToPlaceableTile` calls against 1,103 literal `createTile =` assignments, so
# a reader that only sees the literals sees a little over half the source. That is how a first
# version of this checker decided 1,414 of the table's 2,401 pairs came from somewhere else.
#
# The helpers that place, both directly and through each other. `DefaultToBanner` and
# `DefaultToMonolith` reach `createTile` only by calling `DefaultToPlaceableTile`, so a scan that
# greps helper bodies for the field name misses them; the transitive closure in
# [`placing_helpers`] finds them, and anything it finds that is not handled here stops the run.
# That closure has to look at `private` helpers as well as `public` ones: `DefaultToTorch` and
# `DefaultToSeaShell` are private, and a version of this scan that read only the public ones missed
# the Water Torch and every sea shell - the same class of hole, one visibility keyword further in.
CALL = re.compile(r"^\s*(DefaultTo\w+)\((.*)\);$")
# `(tile expression, style expression or None)` for each, given the call's arguments and the item.
HELPERS = {
    "DefaultToPlaceableTile": lambda a, _t: (a[0], a[1] if len(a) > 1 else "0"),
    "DefaultToMonolith": lambda a, _t: (a[0], a[1] if len(a) > 1 else "0"),
    "DefaultToMusicBox": lambda a, _t: ("139", a[0]),
    "DefaultToBanner": lambda a, _t: ("91", a[0] if a and a[0] else "0"),
    # The second argument is `allowWaterPlacement`, not a style.
    "DefaultToTorch": lambda a, _t: ("4", a[0]),
    # This one takes no arguments at all: it sets the tile and then picks the style from its own
    # `switch (type)`, so the per-item table lives inside the helper. Read from source rather than
    # transcribed, in [`sea_shell_styles`].
    "DefaultToSeaShell": lambda _a, item: ("324", str(SEA_SHELL.get(item, 0))),
    # These two set `createTile` and leave `placeStyle` at the preamble's 0.
    "DefaultToKite": lambda _a, _t: ("723", None),
    "DefaultToCapturedCritter": lambda _a, _t: ("724", None),
}
# Filled by [`sea_shell_styles`] on the first read of `Item.cs`.
SEA_SHELL: dict[int, int] = {}
# A shared case body covering a run of item types computes its tile or style from `type`:
# `DefaultToPlaceableTile(179 + type - 4349)`, `(ushort)(type - 4327 + 521)`, `376, 18 + type -
# 4405`. Every one is linear in `type`, so the arithmetic is evaluated rather than pattern-matched,
# on a string first checked to hold nothing but digits, `type` and `+-()`.
# `type`, digits and arithmetic, and nothing else. `>` is here for the one arm that sums
# comparisons (`type - 3665 + (type > 3666).ToInt() + ...`), which Python evaluates the same way
# once `.ToInt()` is dropped, because its bools are ints too.
SAFE_EXPR = re.compile(r"^[\d\s+\-*()<>]*(?:type[\d\s+\-*()<>]*)*$")

# Pairs where the game's own `GetItemDrop_*` table disagrees with the inversion and wins, plus
# anything else deliberate. Keyed by (tile, style).
# Nothing is on it: the four pairs the sources disagree about are resolved in favour of the drop
# side, which is what the game actually runs, rather than recorded as exceptions.
#   (13, 1) and (13, 2): items 5320 and 5321 place the bottles that `GetItemDrop_Bottles` gives
#     back as 28 and 110 (`WorldGen.cs:41628-41633`).
#   (89, 23): items 2413 and 2539 both declare `createTile = 89; placeStyle = 23;`, and
#     `GetItemDrop_Benches(23)` names 2539.
#   (246, 0): the inversion finds item 5258, and `Check3x2Wall`'s own arm gives `1479 + style`
#     (`WorldGen.cs:45250`), so a plain 3x2 painting gives 1479.
EXPECTED = {}


def expression(text, item, strict=True):
    """A literal, or arithmetic on the item's own type. None when it is neither and `strict` is off."""
    text = text.strip().removeprefix("(ushort)").strip().replace(".ToInt()", "")
    if re.fullmatch(r"-?\d+", text):
        return int(text)
    if not SAFE_EXPR.match(text):
        if strict:
            sys.exit("unhandled placement expression %r" % text)
        return None
    return int(eval(text, {"__builtins__": {}}, {"type": item}))  # noqa: S307


def split_args(text):
    """A call's arguments, split on the commas that are not inside parentheses."""
    out, depth, current = [], 0, ""
    for ch in text:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        if ch == "," and depth == 0:
            out.append(current)
            current = ""
            continue
        current += ch
    if current.strip():
        out.append(current)
    return [a.strip() for a in out]


def sea_shell_styles(text):
    """`DefaultToSeaShell`'s own `switch (type)`: which item gets which style of tile 324.

    The helper takes no arguments, so unlike every other one here the style is not at the call
    site. Four named types and a default of 0.
    """
    start = text.index("private void DefaultToSeaShell()")
    lines = text[start:].split("\n")
    depth, seen, body = 0, False, []
    for line in lines:
        depth += line.count("{") - line.count("}")
        if line.count("{"):
            seen = True
        body.append(line)
        if seen and depth == 0:
            break
    out, pending = {}, []
    for line in body:
        m = re.match(r"^\s*case (\d+):\s*$", line)
        if m:
            pending.append(int(m.group(1)))
            continue
        m = re.match(r"^\s*placeStyle = (\d+);", line)
        if m and pending:
            for item in pending:
                out[item] = int(m.group(1))
            continue
        if line.strip() == "break;":
            pending = []
    if not out:
        sys.exit("DefaultToSeaShell's switch read as empty")
    return out


def placing_helpers(text):
    """Every `Item` method that reaches `createTile` or `placeStyle`, transitively.

    Written as a closure rather than a list because the shallow version of this check is what let
    `DefaultToBanner` through: its body never names either field, it just calls
    `DefaultToPlaceableTile`, and 130 banner styles went missing as a result. Anything this finds
    that `HELPERS` does not model stops the run rather than being silently skipped.
    """
    bodies = {}
    lines = text.split("\n")
    # `private` as well as `public`: `DefaultToTorch` is private, sets `createTile = 4`, and
    # was missed by a scan that only looked at the public ones - the same class of hole this
    # closure exists to close, one visibility keyword further in.
    signature = re.compile(r"^\t(?:public|private|internal) void (\w+)\(")
    for i, line in enumerate(lines):
        m = signature.match(line)
        if not m:
            continue
        depth, seen, body = 0, False, []
        for j in range(i, len(lines)):
            depth += lines[j].count("{") - lines[j].count("}")
            if lines[j].count("{"):
                seen = True
            body.append(lines[j])
            if seen and depth == 0:
                break
        bodies[m.group(1)] = body

    reaching = {
        name
        for name, body in bodies.items()
        if any("createTile" in l or "placeStyle" in l for l in body)
    }
    while True:
        grown = {
            name
            for name, body in bodies.items()
            if name not in reaching
            and any(re.search(r"\b(%s)\(" % "|".join(reaching), l) for l in body)
        }
        if not grown:
            break
        reaching |= grown
    # The machinery that reaches the fields by resetting or re-entering `SetDefaults`, rather than
    # by being one item's placement. None of these is ever called from inside a `case` arm.
    return {n for n in reaching if n.startswith("DefaultTo")}


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
    text = (root / "Terraria" / "Item.cs").read_text(errors="replace")
    lines = text.split("\n")
    SEA_SHELL.update(sea_shell_styles(text))
    unmodelled = placing_helpers(text) - set(HELPERS)
    if unmodelled:
        sys.exit(
            "these Item helpers set createTile or placeStyle and this checker does not model "
            "them, so every item that uses one would read as placing nothing: %s"
            % ", ".join(sorted(unmodelled))
        )
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
                    # Not strict: `createTile = tileIDToPlace;` inside a helper is a real
                    # assignment this walk can see but cannot evaluate, and it is not an item's.
                    value = expression(fm.group(2), t, strict=False)
                    if value is not None:
                        items[t][fm.group(1)] = value
                continue
            cm = CALL.match(line)
            if cm and pending and cm.group(1) in HELPERS:
                for t in narrow if narrow is not None else pending:
                    tile_expr, style_expr = HELPERS[cm.group(1)](split_args(cm.group(2)), t)
                    items[t]["createTile"] = expression(tile_expr, t)
                    # A one-argument `DefaultToPlaceableTile` is `tileStyleToPlace = 0`, and the
                    # helper assigns it either way: a style set before the call is overwritten,
                    # not kept. Only the two that never touch the field pass None.
                    if style_expr is not None:
                        items[t]["placeStyle"] = expression(style_expr, t)
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


# ---------------------------------------------------------------------------------------------
# The other source: the game's own `GetItemDrop_*` methods.
#
# The inversion above says which item *places* a tile and style. For 26 tiles the game also keeps a
# method saying what breaking one *gives*, and where the two differ that one wins, because it is
# the one `KillTile_GetItemDrops` and the `Check*` shape validators actually call. Reading only the
# inversion left 1,414 of the table's entries unchecked; reading both leaves 479.
#
# Each method is a small program over `style`, so it is interpreted rather than pattern-matched:
# a `switch` statement with stacked cases and a `default`, a `switch` *expression* with `N => M`
# arms, range tests, and offsets like `2591 + style - 8`.
# ---------------------------------------------------------------------------------------------

# Which tile each method answers for, read off the nearest enclosing `if (type == N)` (or `case N:`)
# at its call site in `WorldGen.cs`. Two of them take a `secondType` flag choosing between a pair of
# tiles: chests 21/467 (`WorldGen.cs:64499`) and fake chests 441/468 (`:52879`, `:52888`).
DROP_TILES = {
    ("Bottles", None): 13,
    ("Tables", False): 14,
    ("Tables", True): 469,
    ("Chair", None): 15,
    ("Workbenches", None): 18,
    ("Platforms", None): 19,
    ("Chests", False): 21,
    ("Chests", True): 467,
    ("Candles", None): 33,
    ("Chandeliers", None): 34,
    ("Lanterns", None): 42,
    ("Beds", None): 79,
    ("Pianos", None): 87,
    ("Dressers", None): 88,
    ("Benches", None): 89,
    ("Bathtubs", None): 90,
    ("Lamps", None): 93,
    ("Candelabras", None): 100,
    ("Bookcases", None): 101,
    ("Clocks", None): 104,
    ("MusicBoxes", None): 139,
    ("Sinks", None): 172,
    ("FakeChests", False): 441,
    ("FakeChests", True): 468,
    ("PicnicTables", None): 487,
    ("Toilet", None): 497,
}

DROP_SIG = r"^\t(?:public|private) static int GetItemDrop_%s\(int style"
CASE = re.compile(r"^\s*case (\d+):\s*$")
DEFAULT = re.compile(r"^\s*default:\s*$")
ARROW = re.compile(r"^\s*(\d+) => (\d+),?\s*$")
ARROW_DEFAULT = re.compile(r"^\s*_ => (\d+),?\s*$")
SWITCH_EXPR = re.compile(r"^\s*return style switch\s*$")
SWITCH_STMT = re.compile(r"^\s*switch \(style\)\s*$")
DROP_ASSIGN = re.compile(r"^\s*(?:int )?\w+ = ([-+ 0-9a-z]+);$")
DROP_RETURN = re.compile(r"^\s*return ([-+ 0-9a-z]+);$")
DROP_IF = re.compile(r"^\s*(?:else )?if \((.+)\)\s*$")
STYLE_EXPR = re.compile(r"^[\d\s+\-]*(?:style[\d\s+\-]*)*$")
DROP_RANGE = re.compile(r"^style >= (\d+) && style <= (\d+)$")
DROP_GE = re.compile(r"^style >= (\d+)$")
DROP_LE = re.compile(r"^style <= (\d+)$")
DROP_LT = re.compile(r"^style < (\d+)$")
DROP_EQ = re.compile(r"^style == (\d+)$")
# `GetItemDrop_PicnicTables`' whole body: `if (style == 0 || style != 1)`, i.e. everything but 1.
DROP_NOT = re.compile(r"^style == (\d+) \|\| style != (\d+)$")


class Returned(Exception):
    """A `return` reached while interpreting one of these methods."""

    def __init__(self, result):
        self.result = result


def style_value(text, style, state):
    """A literal, arithmetic on `style`, or the single local the method accumulates into."""
    text = text.strip()
    if re.fullmatch(r"\w+", text) and not text.isdigit():
        return state["value"]
    if not STYLE_EXPR.match(text):
        sys.exit("unhandled drop-method expression %r" % text)
    return int(eval(text, {"__builtins__": {}}, {"style": style}))  # noqa: S307


def style_condition(text, style, second):
    text = text.strip()
    if text == "secondType":
        return bool(second)
    for pattern, test in (
        (DROP_RANGE, lambda m: int(m.group(1)) <= style <= int(m.group(2))),
        (DROP_GE, lambda m: style >= int(m.group(1))),
        (DROP_LE, lambda m: style <= int(m.group(1))),
        (DROP_LT, lambda m: style < int(m.group(1))),
        (DROP_NOT, lambda m: style == int(m.group(1)) or style != int(m.group(2))),
        (DROP_EQ, lambda m: style == int(m.group(1))),
    ):
        m = pattern.match(text)
        if m:
            return test(m)
    sys.exit("unhandled drop-method condition %r" % text)


def take_block(lines, i):
    """The statements of the `{ ... }` opening at `i`, and the index just past its close."""
    depth, j = 0, i
    while j < len(lines):
        depth += lines[j].count("{") - lines[j].count("}")
        if depth == 0:
            return lines[i + 1 : j], j + 1
        j += 1
    sys.exit("unbalanced block in a GetItemDrop_ method")


def run_drop_block(block, style, second, state):
    """Interpret one block of a drop method for a single style."""
    i = 0
    while i < len(block):
        line = block[i]
        text = line.strip()

        if SWITCH_EXPR.match(line):
            body, i = take_block(block, i + 1)
            fallback = None
            for entry in body:
                m = ARROW.match(entry)
                if m and int(m.group(1)) == style:
                    raise Returned(int(m.group(2)))
                m = ARROW_DEFAULT.match(entry)
                if m:
                    fallback = int(m.group(1))
            raise Returned(fallback)

        if SWITCH_STMT.match(line):
            body, i = take_block(block, i + 1)
            run_drop_switch(body, style, second, state)
            continue

        m = DROP_IF.match(line)
        if m:
            body, i = take_block(block, i + 1)
            taken = style_condition(m.group(1), style, second)
            if taken:
                run_drop_block(body, style, second, state)
            # Trailing `else` / `else if` chains, tried in order until one is taken.
            while i < len(block):
                head = block[i].strip()
                if head == "else":
                    other, i = take_block(block, i + 1)
                    if not taken:
                        run_drop_block(other, style, second, state)
                    break
                if head.startswith("else if ("):
                    condition = DROP_IF.match(block[i]).group(1)
                    other, i = take_block(block, i + 1)
                    if not taken and style_condition(condition, style, second):
                        run_drop_block(other, style, second, state)
                        taken = True
                    continue
                break
            continue

        m = DROP_RETURN.match(line)
        if m:
            raise Returned(style_value(m.group(1), style, state))

        m = DROP_ASSIGN.match(line)
        if m:
            state["value"] = style_value(m.group(1), style, state)
            i += 1
            continue

        if text in ("break;", "{", "}", ""):
            i += 1
            continue
        sys.exit("unhandled drop-method statement %r" % text)


def run_drop_switch(body, style, second, state):
    """The `case`/`default` arms of a `switch (style)`, for one style."""
    arms, pending, current = [], [], None
    depth = 0
    for line in body:
        # Only labels at this switch's own level are its arms: `GetItemDrop_Clocks` nests another
        # `switch (style)` inside one, and reading its labels as this one's tears the arms apart.
        inner = depth > 0
        depth += line.count("{") - line.count("}")
        if inner:
            current = current or []
            current.append(line)
            continue
        if CASE.match(line) or DEFAULT.match(line):
            if current is not None:
                arms.append((pending, current))
                pending, current = [], None
            m = CASE.match(line)
            pending.append(int(m.group(1)) if m else None)
            continue
        current = current if current is not None else []
        current.append(line)
    if current is not None:
        arms.append((pending, current))

    fallback = None
    for labels, statements in arms:
        if style in labels:
            run_drop_block(statements, style, second, state)
            return
        if None in labels:
            fallback = statements
    if fallback is not None:
        run_drop_block(fallback, style, second, state)


def drop_method(lines, name):
    signature = re.compile(DROP_SIG % name)
    start = next((i for i, l in enumerate(lines) if signature.match(l)), None)
    if start is None:
        sys.exit("no GetItemDrop_%s in WorldGen.cs" % name)
    depth, seen = 0, False
    for j in range(start, len(lines)):
        depth += lines[j].count("{") - lines[j].count("}")
        if lines[j].count("{"):
            seen = True
        if seen and depth == 0:
            return lines[start + 1 : j]
    sys.exit("GetItemDrop_%s never closes" % name)


def highest_style(body):
    """The largest style a method actually names.

    Every one of them falls through to a default for any integer, so "what did it answer" cannot be
    the bound: it would claim hundreds of styles per tile. What a method *names* - in a `case`, an
    `N =>` arm, or a comparison against `style` - is the real extent of the family.
    """
    top = 0
    for line in body:
        m = CASE.match(line) or ARROW.match(line)
        if m:
            top = max(top, int(m.group(1)))
            continue
        m = DROP_IF.match(line)
        if m and "style" in m.group(1):
            top = max([top] + [int(n) for n in re.findall(r"\d+", m.group(1))])
    return top


def from_drop_methods(root):
    """`(tile, style) -> item` for every style the game's own drop methods answer for."""
    lines = (root / "Terraria" / "WorldGen.cs").read_text(errors="replace").split("\n")
    out = {}
    for (name, second), tile in DROP_TILES.items():
        body = drop_method(lines, name)
        for style in range(highest_style(body) + 1):
            state = {"value": None}
            try:
                run_drop_block(body, style, second, state)
                result = state["value"]
            except Returned as done:
                result = done.result if done.result is not None else state["value"]
            # `GetItemDrop_FakeChests` answers -1 for a style that gives nothing, and its call sites
            # guard on it (`WorldGen.cs:52880`, `:52889`). That is "no item", not an item.
            if result is not None and result >= 0:
                out[(tile, style)] = result
    return out


# Six tiles have their drop written inline in `WorldGen.cs` rather than in a `GetItemDrop_*`
# method: banners, and the five sizes of painting. Each is a switch or if-chain over the style, in
# exactly the vocabulary the drop methods use, so they are rewritten into it and handed to the same
# interpreter rather than getting a parser each. 340 pairs, 226 of which nothing else defines.
#
# `num`/`num4` in these arms is the style: `frameX / 18` plus `frameY / 54` (or `/ 36`) times the
# row width. The lines that compute it are the definition of "style" here, so they are dropped.
INLINE_ARMS = {
    91: ("if", None, "the banner chain"),
    240: ("case", "Check3x3Wall", "3x3 paintings, inside that method's `switch (type)`"),
    241: ("if", None, "4x3 paintings, one item for every style"),
    242: ("if", None, "6x4 paintings"),
    245: ("if", None, "2x3 paintings"),
    246: ("if", None, "3x2 paintings"),
}
STYLE_LOCAL = re.compile(r"^\s*(?:int )?num4? (?:=|\+=) ")
NEW_ITEM = re.compile(r"Item\.NewItem\(.*?, 32, 32, ([^)]+)\);")


def method_bounds(lines, name):
    """The half-open line range of one method, so a `case N:` can be found inside the right one."""
    head = re.compile(r"^\t(?:public|private|internal).*\b%s\(" % name)
    start = next((i for i, l in enumerate(lines) if head.match(l)), None)
    if start is None:
        sys.exit("no method %s in WorldGen.cs" % name)
    depth, seen = 0, False
    for j in range(start, len(lines)):
        depth += lines[j].count("{") - lines[j].count("}")
        if lines[j].count("{"):
            seen = True
        if seen and depth == 0:
            return start, j
    sys.exit("method %s never closes" % name)


def inline_arm(lines, tile, form, method):
    """The statements of one inline drop arm, rewritten into the drop-method vocabulary."""
    if form == "if":
        head = re.compile(r"^\t\tif \(type == %d\)$" % tile)
        starts = [i for i, l in enumerate(lines) if head.match(l)]
        if len(starts) != 1:
            sys.exit("expected exactly one `if (type == %d)` arm, found %d" % (tile, len(starts)))
        start = starts[0]
        depth, seen, end = 0, False, None
        for j in range(start, len(lines)):
            depth += lines[j].count("{") - lines[j].count("}")
            if lines[j].count("{"):
                seen = True
            if seen and depth == 0:
                end = j
                break
        if end is None:
            sys.exit("the tile-%d drop arm never closes" % tile)
        body = lines[start + 1 : end]
    else:
        # `case 240:` appears in four methods; only the one inside the shape validator is the
        # drop, so the search is bounded by that method.
        low, high = method_bounds(lines, method)
        head = re.compile(r"^\t\tcase %d:$" % tile)
        starts = [i for i, l in enumerate(lines[low:high], low) if head.match(l)]
        if len(starts) != 1:
            sys.exit("expected exactly one `case %d:` arm, found %d" % (tile, len(starts)))
        start = starts[0]
        # A switch arm runs to its own `break;`, which is the one at the arm's own brace depth.
        depth, end = 0, None
        for j in range(start + 1, len(lines)):
            if depth == 0 and lines[j].strip() == "break;":
                end = j
                break
            depth += lines[j].count("{") - lines[j].count("}")
        if end is None:
            sys.exit("the tile-%d case arm never breaks" % tile)
        body = lines[start + 1 : end]

    rewritten = []
    for line in body:
        if STYLE_LOCAL.match(line):
            continue
        text = line.replace("num4", "style").replace("num", "style")
        item = NEW_ITEM.search(text)
        rewritten.append("\t\t\treturn %s;" % item.group(1) if item else text)
    return rewritten


def from_inline_arms(root):
    """`(tile, style) -> item` for the six tiles whose drop is written inline."""
    lines = (root / "Terraria" / "WorldGen.cs").read_text(errors="replace").split("\n")
    out = {}
    for tile, (form, method, _why) in INLINE_ARMS.items():
        body = inline_arm(lines, tile, form, method)
        for style in range(highest_style(body) + 1):
            state = {"value": None}
            try:
                run_drop_block(body, style, None, state)
                result = state["value"]
            except Returned as done:
                result = done.result
            if result is not None and result >= 0:
                out[(tile, style)] = result
    return out


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
    inversion = from_source(root)
    drops = from_drop_methods(root)
    drops.update(from_inline_arms(root))
    # The drop side wins where both speak: it is what `KillTile_GetItemDrops`, the `Check*`
    # validators and the tile-91 arm actually run, and the inversion is a derivation of what places
    # rather than of what is given back. They disagree on three pairs, named above `EXPECTED`.
    source = dict(inversion)
    source.update(drops)
    # Relative to this file rather than the working directory, so a copy of this checker beside a
    # copy of the table reads *that* table. `mutate_tables.py` depends on it.
    repo = Path(__file__).resolve().parent.parent
    ours = from_table(repo / "crates/terrustia-proto/src/placed_items.rs")

    missing = sorted(set(source) - set(ours))
    wrong = sorted(
        (k, source[k], ours[k])
        for k in set(source) & set(ours)
        if source[k] != ours[k] and k not in EXPECTED
    )

    both = set(inversion) & set(drops)
    print(
        "%d pairs in Item.SetDefaults, %d in the drop methods and inline arms, %d in placed_items.rs"
        % (len(inversion), len(drops), len(ours))
    )
    print(
        "%d pairs both sources define, disagreeing on %d"
        % (len(both), sum(1 for k in both if inversion[k] != drops[k]))
    )
    # Only one direction is a contract. `placed_items.rs` carries pairs from neither source -
    # banners come from `BannerSystem.BannerToItem`, paintings from their own worldgen - so a pair
    # being here and in neither says nothing. A pair a source defines being absent or different does.
    print("%d pairs are ours alone, from neither source" % len(set(ours) - set(source)))
    print("%d shared pairs agree" % (len(set(source) & set(ours)) - len(wrong)))

    problems = []
    for k in missing:
        problems.append(
            "  tile %d style %d gives item %d in source and is absent here" % (k[0], k[1], source[k])
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
            "\nEach is a wrong or missing entry. Read the item's arm in `Item.SetDefaults` and the\n"
            "tile's own `GetItemDrop_*` method in `WorldGen.cs`; where the two disagree the drop\n"
            "method wins. If the difference is deliberate, put the pair in EXPECTED with why."
        )
        return 1
    print("\nevery pair in placed_items.rs matches the source that defines it.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
