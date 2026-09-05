#!/usr/bin/env python3
"""Compare `tile_object.rs` against `TileObjectData.Initialize`, entry by entry.

`TileObjectData.Initialize` is not a table. It is a 2,900-line *program*: it mutates a single
shared "current object" field by field and stamps it into a slot with `addTile(N)`, then keeps
mutating it for the next one. There is nothing to read off; the end state only exists once the
program has run.

So this runs it. `terrustia-codegen`'s `tile_object` generator runs it too, and this is the second
opinion on that reading - the same relationship `check_drops.py` has to the generated `npc_drops
.rs`. It was written first, and the two agree on all 389 entries and all 13 fields; a table whose
source is a program is exactly the kind that should not rest on one interpreter. It also catches
what any checker over a generated table catches: a hand-edit, or a `just regen` nobody ran.

The interpreter below is small because the statement vocabulary is small, but it has to be exact in
four places that a plausible-looking shortcut gets wrong:

* **`addTile` resets the current object.** Every `addTile`/`addBaseTile` ends with
  `newTile = new TileObjectData(_baseObject)`, so a tile inherits the *base* defaults, never the
  previous tile's. Reading `Initialize` as one running accumulator is the single most tempting
  misreading of this file, and it silently gives an entry whatever its predecessor last set.
* **`CopyFrom` shares module references; it does not copy them.** `_tileObjectCoords = copy._tile
  ObjectCoords` is an alias. Writes go through a copy-on-write guarded by `_hasOwn*`, and `CopyFrom`
  leaves those flags alone, so an object that already owns a module and is then `CopyFrom`'d keeps
  the flag and writes *through* into the object it copied from. Value semantics get this wrong.
* **A setter given the value already there returns early**, before the copy-on-write and before
  `calculated = false`. So `CoordinatePadding = 2` on something already at 2 is not a no-op with
  the same result: it is a no-op that leaves a stale `calculated` in place.
* **`Calculate()` is memoised on the shared coordinate module**, and `Width` is read at calculate
  time while living in a *different* module. Setting `Width` clears `calculated` only when the
  object did not already own its base module (`TileObjectData.cs:1186-1201`). That asymmetry is
  observable: it is how a tile ends up with a `CoordinateFullWidth` computed from another tile's
  width.

Only the per-type entry is compared. Vanilla also carries sub-tiles (per style) and alternates (per
placement direction), which `tile_object.rs` does not model at all and does not claim to; that gap
is disclosed in the table's own header rather than checked here.

Exit 0 when every entry matches or is on the record in EXPECTED; 1 otherwise.
"""

import re
import sys
from pathlib import Path

# Differences that are correct and deliberate. Keyed by (tile, field), value says what it stands in
# for. Anything not on this list is drift and fails the check.
EXPECTED = {}


class Base:
    """`TileObjectBaseModule` - the fields `Width`/`Height`/`Origin` live in."""

    __slots__ = ("width", "height", "origin")

    def __init__(self, copy=None):
        if copy is None:
            self.width, self.height, self.origin = 1, 1, (0, 0)
        else:
            self.width, self.height, self.origin = copy.width, copy.height, copy.origin


class Coords:
    """`TileObjectCoordinatesModule`, including `Calculate`'s memoised results."""

    __slots__ = (
        "width", "heights", "padding", "pad_fix", "style_width", "style_height", "calculated",
    )

    def __init__(self, copy=None, heights=None):
        if copy is None:
            self.width, self.padding, self.pad_fix = 0, 0, (0, 0)
            self.style_width = self.style_height = 0
            self.calculated = False
            self.heights = heights
            return
        self.width, self.padding, self.pad_fix = copy.width, copy.padding, copy.pad_fix
        self.style_width, self.style_height = copy.style_width, copy.style_height
        self.calculated = copy.calculated
        self.heights = heights if heights is not None else list(copy.heights or [])


class Style:
    """`TileObjectStyleModule`. Note `styleLineSkip` starts at 1, not 0."""

    __slots__ = ("style", "horizontal", "wrap_limit", "multiplier", "line_skip")

    def __init__(self, copy=None):
        if copy is None:
            self.style, self.horizontal = 0, False
            self.wrap_limit, self.multiplier, self.line_skip = 0, 1, 1
        else:
            self.style, self.horizontal = copy.style, copy.horizontal
            self.wrap_limit, self.multiplier = copy.wrap_limit, copy.multiplier
            self.line_skip = copy.line_skip


COORD_FIELDS = {
    "CoordinateWidth": "width",
    "CoordinatePadding": "padding",
    "CoordinatePaddingFix": "pad_fix",
    "CoordinateHeights": "heights",
}
STYLE_FIELDS = {
    "Style": "style",
    "StyleHorizontal": "horizontal",
    "StyleWrapLimit": "wrap_limit",
    "StyleMultiplier": "multiplier",
    "StyleLineSkip": "line_skip",
}


class Obj:
    """One `TileObjectData`. Modules are shared by reference exactly as the game shares them."""

    __slots__ = ("base", "coords", "style", "own_base", "own_coords", "own_style", "subtiles")

    def __init__(self, parent=None):
        self.own_base = self.own_coords = self.own_style = False
        self.subtiles = None
        if parent is None:
            self.base, self.coords, self.style = None, None, None
        else:
            self.copy_from(parent)

    def setup_base_object(self):
        """`SetupBaseObject` - the one object built from nothing."""
        self.base, self.own_base = Base(), True
        self.coords, self.own_coords = Coords(heights=[16]), True
        self.style, self.own_style = Style(), True

    def copy_from(self, other):
        # Reference assignment, and deliberately *not* touching own_*: that is what the game does.
        self.base, self.coords, self.style = other.base, other.coords, other.style

    def full_copy_from(self, other):
        self.copy_from(other)
        self.subtiles = list(other.subtiles) if other.subtiles else None

    def set_base(self, field, value):
        if not self.own_base:
            if getattr(self.base, field) == value:
                return
            self.own_base = True
            self.base = Base(self.base)
            # Width and Height alone drag the coordinate module with them, because `Calculate`
            # reads `Width` (`TileObjectData.cs:1194-1200`). `Origin` does not.
            if field in ("width", "height") and not self.own_coords:
                self.own_coords = True
                self.coords = Coords(self.coords)
                self.coords.calculated = False
        setattr(self.base, field, value)

    def set_coord(self, field, value):
        if not self.own_coords:
            if getattr(self.coords, field) == value:
                return
            self.own_coords = True
            self.coords = Coords(self.coords, heights=value if field == "heights" else None)
        setattr(self.coords, field, value)
        self.coords.calculated = False

    def set_style(self, field, value):
        if not self.own_style:
            if getattr(self.style, field) == value:
                return
            self.own_style = True
            self.style = Style(self.style)
        setattr(self.style, field, value)

    def calculate(self):
        if self.coords.calculated:
            return
        self.coords.calculated = True
        self.coords.style_width = (
            (self.coords.width + self.coords.padding) * self.base.width + self.coords.pad_fix[0]
        )
        total = sum(h + self.coords.padding for h in self.coords.heights)
        self.coords.style_height = total + self.coords.pad_fix[1]


def statements(lines):
    """Fold the method body into one logical statement per entry.

    Multi-line array and `Rectangle[,]` initialisers are the only continuations, and every one of
    them ends in `;`, so accumulating until a semicolon lands is enough. Braces inside such an
    initialiser get swallowed with it, which is the point: they must not reach the block walker.
    """
    out = []
    pending = ""
    for raw in lines:
        line = raw.strip()
        if not line:
            continue
        if not pending and (line in ("{", "}") or line.startswith("for (")):
            out.append(line)
            continue
        pending = (pending + " " + line).strip()
        if pending.endswith(";"):
            out.append(pending)
            pending = ""
    return out


ASSIGN = re.compile(r"^(newTile|newSubTile|newAlternate)\.(\w+) = (.*);$")
CALL = re.compile(r"^(newTile|newSubTile|newAlternate)\.(\w+)\((.*)\);$")
FOR = re.compile(r"^for \(int (\w+) = ([\w\d]+); \1 (<=|<) ([\w\d]+); \1\+\+\)$")
POINT = re.compile(r"^new Point16\((-?\w+), (-?\w+)\)$")
INTS = re.compile(r"^new int\[\d*\]\s*\{(.*)\}$")
LOCAL = re.compile(r"^int (\w+) = (\d+);$")


class Interpreter:
    def __init__(self):
        self.base_object = Obj()
        self.base_object.setup_base_object()
        self.data = {}
        self.named = {}
        self.locals = {}
        self.cur = {
            "newTile": Obj(self.base_object),
            "newSubTile": Obj(self.base_object),
            "newAlternate": Obj(self.base_object),
        }

    def value(self, token):
        """Evaluate the right-hand side of an assignment, or None for one we do not model."""
        token = token.strip()
        if token == "true":
            return True
        if token == "false":
            return False
        if re.fullmatch(r"-?\d+", token):
            return int(token)
        if token == "Point16.Zero":
            return (0, 0)
        m = POINT.match(token)
        if m:
            # `new Point16(0, k)` inside a loop: the components can be the loop variable.
            parts = []
            for raw in (m.group(1), m.group(2)):
                parts.append(int(raw) if re.fullmatch(r"-?\d+", raw) else self.locals[raw])
            return tuple(parts)
        m = INTS.match(token)
        if m:
            return [int(n) for n in re.findall(r"-?\d+", m.group(1))]
        if re.fullmatch(r"new int\[(\d+)\]", token):
            return [0] * int(token[token.index("[") + 1 : -1])
        if token in self.locals:
            return self.locals[token]
        return None

    def arg(self, token):
        """An `addTile`/`addSubTile` argument: a literal, a loop variable, or a small sum."""
        token = token.strip()
        if re.fullmatch(r"-?\d+", token):
            return int(token)
        if token in self.locals:
            return self.locals[token]
        m = re.fullmatch(r"(\d+) \+ newTile\.StyleWrapLimit", token)
        if m:
            return int(m.group(1)) + self.cur["newTile"].style.wrap_limit
        raise ValueError("unhandled argument %r" % token)

    def run(self, stmts):
        i = 0
        while i < len(stmts):
            i = self.step(stmts, i)

    def block(self, stmts, i):
        """The half-open range of the `{ ... }` starting at `i`."""
        assert stmts[i] == "{", stmts[i]
        depth, j = 0, i
        while j < len(stmts):
            if stmts[j] == "{":
                depth += 1
            elif stmts[j] == "}":
                depth -= 1
                if depth == 0:
                    return i + 1, j
            j += 1
        raise ValueError("unbalanced block")

    def step(self, stmts, i):
        line = stmts[i]

        m = FOR.match(line)
        if m:
            var, start, op, end = m.group(1), m.group(2), m.group(3), m.group(4)
            body_start, body_end = self.block(stmts, i + 1)
            lo = self.locals.get(start, None) if not start.isdigit() else int(start)
            hi = self.locals.get(end, None) if not end.isdigit() else int(end)
            if lo is None or hi is None:
                # `for (int i = 0; i < TileID.Count; i++) { _data.Add(null); }` and nothing else.
                return body_end + 1
            for n in range(lo, hi + 1 if op == "<=" else hi):
                self.locals[var] = n
                self.run(stmts[body_start:body_end])
            self.locals.pop(var, None)
            return body_end + 1

        if line in ("{", "}"):
            return i + 1

        m = LOCAL.match(line)
        if m:
            self.locals[m.group(1)] = int(m.group(2))
            return i + 1

        m = re.match(r"^(newTile|newSubTile|newAlternate) = new TileObjectData\(_baseObject\);$", line)
        if m:
            self.cur[m.group(1)] = Obj(self.base_object)
            return i + 1

        m = ASSIGN.match(line)
        if m:
            self.assign(self.cur[m.group(1)], m.group(2), m.group(3))
            return i + 1

        m = CALL.match(line)
        if m:
            self.method(self.cur[m.group(1)], m.group(2), m.group(3))
            return i + 1

        m = re.match(r"^add(Tile|BaseTile|SubTile|SubTileRange|Alternate)\((.*)\);$", line)
        if m:
            self.add(m.group(1), m.group(2))
            return i + 1

        # Everything left is a field this table does not carry: anchors, hooks, draw offsets, the
        # `_data` bookkeeping, and the manual alternate pushes.
        return i + 1

    def assign(self, obj, field, rhs):
        value = self.value(rhs)
        if field in ("Width", "Height", "Origin"):
            if value is None:
                raise ValueError("unparsed %s = %s" % (field, rhs))
            self.obj_set_base(obj, field, value)
        elif field in COORD_FIELDS:
            if value is None:
                raise ValueError("unparsed %s = %s" % (field, rhs))
            obj.set_coord(COORD_FIELDS[field], value)
        elif field in STYLE_FIELDS:
            if value is None:
                raise ValueError("unparsed %s = %s" % (field, rhs))
            obj.set_style(STYLE_FIELDS[field], value)

    def obj_set_base(self, obj, field, value):
        obj.set_base({"Width": "width", "Height": "height", "Origin": "origin"}[field], value)

    def method(self, obj, name, args):
        if name == "CopyFrom":
            obj.copy_from(self.resolve(args))
        elif name == "FullCopyFrom":
            source = self.arg(args) if re.fullmatch(r"\s*\d+\s*", args) else None
            obj.full_copy_from(self.tile_data(source) if source is not None else self.resolve(args))
        elif name == "Calculate":
            obj.calculate()
        # ApplyNaturalObjectRules touches only the placement flags.

    def tile_data(self, tile):
        """`GetTileData(type, 0)`: the style-0 sub-tile when there is one, else the tile itself."""
        entry = self.data[tile]
        if entry.subtiles and len(entry.subtiles) > 0 and entry.subtiles[0] is not None:
            return entry.subtiles[0]
        return entry

    def resolve(self, name):
        name = name.strip()
        if name in self.cur:
            return self.cur[name]
        return self.named[name]

    def add(self, kind, args):
        tile = self.cur["newTile"]
        if kind == "Tile":
            tile.calculate()
            self.data[self.arg(args)] = tile
            self.cur["newTile"] = Obj(self.base_object)
        elif kind == "BaseTile":
            tile.calculate()
            self.named[args.replace("out ", "").strip()] = tile
            self.cur["newTile"] = Obj(self.base_object)
        elif kind in ("SubTile", "SubTileRange"):
            sub = self.cur["newSubTile"]
            sub.calculate()
            if kind == "SubTile":
                styles = [self.arg(a) for a in args.split(",")]
            else:
                start, count = (self.arg(a) for a in args.split(","))
                styles = list(range(start, start + count))
            if tile.subtiles is None:
                tile.subtiles = []
            for style in styles:
                while len(tile.subtiles) <= style:
                    tile.subtiles.append(None)
                tile.subtiles[style] = sub
            self.cur["newSubTile"] = Obj(self.base_object)
        elif kind == "Alternate":
            alt = self.cur["newAlternate"]
            alt.calculate()
            alt.set_style("style", self.arg(args))
            self.cur["newAlternate"] = Obj(self.base_object)


def from_source(root):
    text = (root / "Terraria.ObjectData" / "TileObjectData.cs").read_text(errors="replace")
    lines = text.split("\n")
    try:
        start = next(
            i for i, l in enumerate(lines) if l.strip() == "public static void Initialize()"
        )
    except StopIteration:
        sys.exit("no Initialize in TileObjectData.cs")
    # Bounded by the method's own braces, the same discipline the other two checkers use.
    depth, seen, end = 0, False, None
    for i in range(start, len(lines)):
        depth += lines[i].count("{") - lines[i].count("}")
        if lines[i].count("{"):
            seen = True
        if seen and depth == 0:
            end = i
            break
    if end is None:
        sys.exit("Initialize never closes")

    interp = Interpreter()
    interp.run(statements(lines[start + 1 : end]))

    out = {}
    for tile, obj in interp.data.items():
        obj.calculate()
        out[tile] = {
            "width": obj.base.width,
            "height": obj.base.height,
            "origin": obj.base.origin,
            "coord_width": obj.coords.width,
            "coord_heights": list(obj.coords.heights),
            "padding": obj.coords.padding,
            "full_width": obj.coords.style_width,
            "full_height": obj.coords.style_height,
            "style_horizontal": obj.style.horizontal,
            "style_multiplier": obj.style.multiplier,
            "style_wrap": obj.style.wrap_limit,
            "style_line_skip": obj.style.line_skip,
            "style_base": obj.style.style,
        }
    return out


FIELD = re.compile(r"^\s*(\w+): (.*?),?$")


def from_table(path):
    text = path.read_text()
    body = text[text.index("pub fn tile_object("):]
    out = {}
    for m in re.finditer(r"^        (\d+) => TileObject \{\n(.*?)\n        \},$", body, re.S | re.M):
        entry = {}
        for line in m.group(2).split("\n"):
            fm = FIELD.match(line)
            if not fm:
                continue
            key, raw = fm.group(1), fm.group(2).strip().rstrip(",")
            if key in ("width", "height", "coord_width", "padding", "full_width", "full_height",
                       "style_multiplier", "style_wrap", "style_line_skip", "style_base"):
                entry[key] = int(raw)
            elif key == "origin":
                entry[key] = tuple(int(n) for n in re.findall(r"-?\d+", raw))
            elif key == "coord_heights":
                entry[key] = [int(n) for n in re.findall(r"-?\d+", raw)]
            elif key == "style_horizontal":
                entry[key] = raw == "true"
        out[int(m.group(1))] = entry
    return out


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: check_tile_object.py <decompiled tree>")
    source = from_source(Path(sys.argv[1]))
    # Relative to this file rather than the working directory, so a copy of this checker beside a
    # copy of the table reads *that* table. `mutate_tables.py` depends on it.
    repo = Path(__file__).resolve().parent.parent
    ours = from_table(repo / "crates/terrustia-proto/src/tile_object.rs")

    print("%d entries in TileObjectData.Initialize, %d in tile_object.rs" % (len(source), len(ours)))

    problems = []
    for tile in sorted(set(source) | set(ours)):
        if tile not in ours:
            problems.append("  tile %d is in Initialize and not in tile_object.rs" % tile)
            continue
        if tile not in source:
            problems.append("  tile %d is in tile_object.rs and Initialize never adds it" % tile)
            continue
        for field, want in source[tile].items():
            got = ours[tile].get(field)
            if got == want or (tile, field) in EXPECTED:
                continue
            problems.append(
                "  tile %d %s: Initialize says %r, tile_object.rs says %r" % (tile, field, want, got)
            )

    if EXPECTED:
        print("%d differences on the record:" % len(EXPECTED))
        for (tile, field), why in sorted(EXPECTED.items()):
            print("  tile %d %s: %s" % (tile, field, why))

    if problems:
        print("\n%d unexplained difference(s):" % len(problems))
        print("\n".join(problems[:80]))
        if len(problems) > 80:
            print("  ... and %d more" % (len(problems) - 80))
        print(
            "\nOne of the two is wrong. Walk `Initialize` to the tile's own `addTile` and read the\n"
            "state it stamps; if the difference is deliberate, put it in EXPECTED with why."
        )
        return 1
    print("\nevery entry in tile_object.rs matches TileObjectData.Initialize.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
