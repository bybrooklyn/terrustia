#!/usr/bin/env python3
"""Compare this server's drop tables against the game's, and report what is missing.

    python3 tools/check_drops.py <decompiled-tree>

Why a checker rather than a generator: `ItemDropDatabase` is not a flat table. It is built from
`LeadingConditionRule` chains with `OnSuccess`/`OnFailedRoll` branches, `OneFromOptions` pools and
mode-dependent rerolls, and a generator that got any of that subtly wrong would hand players the
wrong loot forever while looking authoritative. A checker cannot do that. It can only say "the game
gives NPC 262 item 1141 and you do not", which is exactly the sentence that was missing when the
Temple Key went absent and took the second half of the game with it.

It is deliberately generous about *how* a drop is registered and strict about *whether* it exists:
false positives here are cheap to dismiss, and a false negative is another Temple Key.

Exit code is 1 when a boss is missing loot, because those are the ones that gate progression.
"""

import re
import sys
from pathlib import Path

# Bosses and event bosses. A missing drop on any of these can stop a playthrough, so they are
# reported separately and are what the exit code turns on.
#
# 344/345/346 (Everscream, Santa-NK1, Ice Queen — the Frost Moon's own bosses) and 564/565/576/577
# (DD2's Dark Mage and Ogre, both tiers) used to be entirely absent from this set: a missing drop
# on any of the seven was invisible to the exit code, reported as an ordinary-enemy gap (if at
# all) rather than the boss-loot regression it actually is.
BOSSES = {
    4, 13, 14, 15, 35, 36, 50, 113, 125, 126, 127, 134, 222, 245, 262, 266, 267,
    325, 326, 344, 345, 346, 370, 395, 398, 439, 551, 564, 565, 576, 577, 636, 657, 668,
}

# How many ordinary-enemy gaps to print before summarising. The list, not the count, is the point.
MAX_LISTED = 40

# Drops this project knowingly does not give out, and why. Anything not in here is a bug, whoever
# it drops from: the exit code turns on the whole gap list now, not just the boss half of it.
#
# A key is either an item id (deferred wherever it drops) or an `(npc, item)` pair, for the items
# that are perfectly ordinary drops in general and only unreachable from one enemy. Gel really does
# drop from twenty-five slimes; it is only the Lava Slime's own Gel that is Remix-seed exclusive.
#
# The bar for adding an entry is a *mechanism* this server does not have, named here. "It is only
# an ordinary enemy" is not a reason, and neither is "it is a mode we do not care about": the
# previous shape of this file excused master-mode drops and treasure bags wholesale on exactly
# that reasoning, and 57 items plus four bosses' entire expert loot went missing behind it.
DEFERRED: dict[int | tuple[int, int], str] = {
    # `Conditions.WindyEnoughForKiteDrops` reads `Main.WindyEnoughForKiteDrops`. This server does
    # not simulate wind at all: it sends `wind_speed_target: 0.0` in every world-data packet, so
    # the condition can never be true and there is nothing to gate on.
    **{
        item: "needs wind simulation (Conditions.WindyEnoughForKiteDrops)"
        for item in (
            4379, 4610, 4611, 4613, 4648, 4649, 4650, 4651, 4669, 4670, 4671, 4675, 4683, 4684,
        )
    },
    # World seeds this project does not model: Remix (157 Aqua Scepter), Skyblock (1786 Sickle),
    # and `getGoodWorld`/Mechdusa (5382 Waffle Iron).
    157: "Remix seed only (Conditions.RemixSeed*)",
    1786: "Skyblock seed only (Conditions.SkyblockIsUpNoSickle)",
    5382: "getGoodWorld seed only (Conditions.MechdusaKill)",
    # `Conditions.NamedNPC` matches the NPC's own randomly-given name. Names are generated here
    # (`town_names.rs`) but no drop condition can reach one.
    867: "needs the NPC's given name (Conditions.NamedNPC)",
    4372: "needs the NPC's given name (Conditions.NamedNPC)",
    5290: "needs the NPC's given name (Conditions.NamedNPC)",
    # `Conditions.IsChristmas`/`XmasPresentDrop` need the real-world calendar date, which nothing
    # in this server reads.
    1869: "needs the calendar date (Conditions.IsChristmas)",
    # The Terraprisma. `Conditions.EmpressOfLightIsGenuinelyEnraged` reads
    # `NPC.AI_120_HallowBoss_IsGenuinelyEnraged()`, per-instance `ai[3]` state set when the fight
    # begins in daylight at full life. This server has no ai style 120 at all, so there is no
    # instance state to read. It used to drop from every Empress kill in every mode instead.
    # `Conditions.EyeOfCthulhuDefeatedAndNoAltarsInWorld` needs two facts `conditional_drops`'
    # `Conditions` does not carry: whether the Eye is down, and how many altars are unsmashed.
    # Item 43 has no other drop source in the game, so this is item-keyed rather than per-npc.
    43: "needs downed-Eye plus an unsmashed-altar count (Conditions.EyeOfCthulhu...)",
    # Remix seed, but only from these particular enemies: each of these items is an ordinary drop
    # elsewhere, so they are pairs rather than blanket exclusions.
    (49, 1314): "Remix seed only (Conditions.RemixSeed)",
    (59, 23): "Remix seed only (Conditions.RemixSeed)",
    (59, 1309): "Remix seed only (Conditions.RemixSeed)",
    (85, 3069): "Remix seed only (Conditions.RemixSeedHardmode)",
    (109, 1325): "Remix seed only (Conditions.RemixSeed)",
    (156, 112): "Remix seed only (Conditions.RemixSeed)",
    # `Conditions.SkyblockIsUp` on the four armed/unarmed Zombie Elves' Wood.
    **{(npc, 9): "Skyblock seed only (Conditions.SkyblockIsUp)" for npc in (188, 189, 434, 435)},
    # `RegisterToNPC(44, Common(118, 25)).OnFailedRoll(OneFromOptions(4, 410, 411))
    #  .OnFailedRoll(Common(166, 1, 1, 3))` (`ItemDropDatabase.cs:1157`): a *pool* in the middle of
    # a failed-roll chain. `npc_drops`' chains are runs of single rules and `conditional_drops`'
    # pools are standalone, so neither shape can hold "roll one item, else draw from a pool, else
    # roll another item" without inventing a third.
    **{
        (44, item): "a OneFromOptions pool mid-chain, which neither table's shape can express"
        for item in (166, 410, 411)
    },
    # The Ice Mimic's weapon pool. `RegisterIceMimic` (`ItemDropDatabase.cs:232-243`) hangs four
    # `OneFromOptions` pools off `Common(1312, 20).OnFailedRoll(...)`, one per remix/hardmode
    # branch, two of them behind a helper method that builds the pool at runtime. That is the same
    # mid-chain-pool shape as npc 44's, and `conditional_drops.rs` says so at its own `629` arm.
    # It only became visible here once `parse_game` learned to follow a rule variable that is
    # declared before it is registered, which is how the whole of this NPC's loot was missing from
    # the game side of this comparison rather than reported.
    **{
        (629, item): "a OneFromOptions pool mid-chain (RegisterIceMimic), same shape as npc 44's"
        for item in (676, 725, 1264, 6172)
    },
    (629, 1319): "Remix seed only (Conditions.RemixSeedHardmode)",
}


# A bare integer, but never one that is actually the tail of an identifier — `condition2` and
# `npcNetIds18` both end in digits that are not numbers in this text, and matching them as though
# they were is how Eye of Cthulhu ended up "owing" the player items 2, 3 and 282 that were really
# `condition2`, `condition3` and an unrelated `npcNetIds18` array's own name. Three separate bugs
# of this shape were found by hand-tracing every line that (mis)attributed something to npc 4 —
# see the git history of this fix for the trace.
NUM = re.compile(r"(?<![A-Za-z0-9_])-?\d+")


def _split_top_level(s: str) -> list[str]:
    """Split on commas at paren depth 0 only, so `Foo(1, 2), 3, 4` splits into two arguments —
    `Foo(1, 2)` and `3, 4` joined back — not four."""
    parts, depth, start = [], 0, 0
    for i, ch in enumerate(s):
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        elif ch == "," and depth == 0:
            parts.append(s[start:i])
            start = i + 1
    parts.append(s[start:])
    return parts


def _find_matching_paren(s: str, open_at: int) -> int:
    """Index of the `)` that closes the `(` at `open_at`."""
    depth = 0
    for i in range(open_at, len(s)):
        if s[i] == "(":
            depth += 1
        elif s[i] == ")":
            depth -= 1
            if depth == 0:
                return i
    return -1


# Which argument of a rule constructor is the item id, and which are chance denominators in front
# of a pool's item list. A constructor not named here takes the item first, which is the common
# case; `ByCondition`'s own condition object comes first instead, and every `*OneFromOptions*`
# variant leads with one or two denominators before its options.
#
# The `new X(...)` classes matter as much as the `ItemDropRule.X(...)` factories: source builds
# rules both ways, and reading only the factories is why the Bloody Tear, the Nail Gun, the lunar
# fragments and every Don't Starve crossover drop were invisible to *both* sides of this
# comparison, so nothing could ever have reported them.
ITEM_AT = {"ByCondition": 1, "BossBagByCondition": 1}
POOL_SKIP = {
    "OneFromOptions": 1,
    "OneFromOptionsNotScalingWithLuck": 1,
    "OneFromOptionsWithNumerator": 2,
    "OneFromOptionsNotScalingWithLuckWithX": 2,
    "NormalvsExpertOneFromOptions": 2,
    "NormalvsExpertOneFromOptionsNotScalingWithLuck": 2,
    "OneFromOptionsDropRule": 2,
    "OneFromOptionsNotScaledWithLuckDropRule": 2,
    "FromOptionsWithoutRepeatsDropRule": 1,
}
# `new X(...)` classes whose first argument is an item. Anything else built with `new` (the
# condition wrappers, the mode branches, the pool classes above) is walked into rather than read.
DIRECT_RULES = {
    "CommonDrop",
    "CommonDropNotScalingWithLuck",
    "CommonDropScalingWithOnlyBadLuck",
    "CommonDropWithRerolls",
    "ItemDropWithConditionRule",
    "DropOneByOne",
    "DropPerPlayerOnThePlayer",
    "DropLocalPerClientAndResetsNPCMoneyTo0",
}
CALL = re.compile(r"(ItemDropRule\.|new )(\w+)\(")


def _items_in(line: str) -> set[int]:
    """Every item id the rule constructors on this line name.

    Arguments are split with balanced parentheses rather than a `[^)]*` slice: an
    `ItemDropRule.ByCondition(new Conditions.WindyEnoughForKiteDrops(), 4611, 25)` ends its first
    argument with a `)`, so the cheap slice stopped there and the item never appeared at all. Every
    `ByCondition` drop in the file was invisible on the game side for that reason, which is a whole
    class of gap this checker could not have reported.
    """
    items: set[int] = set()

    def literal(arg: str) -> int | None:
        arg = arg.strip()
        return int(arg) if re.fullmatch(r"-?\d+", arg) else None

    for m in CALL.finditer(line):
        constructed, name = m.group(1) == "new ", m.group(2)
        # A `new X(...)` that is not a rule class names no item of its own: `new Conditions.X()`
        # is a condition and `new DropBasedOnExpertMode(a, b)` is a branch, and the rules nested
        # inside either are matched separately by this same loop.
        if constructed and name not in DIRECT_RULES and name not in POOL_SKIP:
            continue
        close_at = _find_matching_paren(line, m.end() - 1)
        if close_at == -1:
            continue
        args = _split_top_level(line[m.end() : close_at])
        # `Gel(chanceDenominator, min, max)` has no item argument at all: the item is always Gel
        # (23). Reading its first number as an item invented a phantom drop out of a chance value;
        # skipping the call entirely was the other half of the same mistake, and it is why nothing
        # noticed that item 23 dropped from nothing in the whole world.
        if name == "Gel":
            items.add(23)
        elif (skip := POOL_SKIP.get(name)) is not None:
            items.update(v for a in args[skip:] if (v := literal(a)) and v > 0)
        else:
            at = ITEM_AT.get(name, 0)
            if at < len(args) and (v := literal(args[at])) and v > 0:
                items.add(v)
    return items


def parse_game(root: Path) -> dict[int, set[int]]:
    """Every (npc type -> item ids) pair the drop database registers.

    Reads whole statements rather than lines, because a rule chain is written across several.

    Nothing is excused here any more. This function used to sort master-mode relics, master-mode
    pets and treasure bags into a second `mode_only` set that `main` then *subtracted* from every
    gap, on the grounds that this server ran classic. It supports master, it has bags, and the
    excuse hid 57 unreachable items and four bosses that dropped nothing at all in expert. An
    absence that is genuinely deliberate belongs in `DEFERRED` below, with its reason written down.
    """
    text = (root / "Terraria.GameContent.ItemDropRules" / "ItemDropDatabase.cs").read_text(
        errors="replace"
    )
    # `int[] npcNetIds = new int[N] { ... };` is declared fresh, under the *same* name, in nearly
    # every one of these register-methods — it is a local, not a file-scoped constant. Collapsing
    # its (possibly multi-line) literal onto the declaration line lets the sequential scan below
    # resolve each `RegisterToMultipleNPCs(..., npcNetIds)` against whichever declaration of that
    # name most recently preceded it, the same way `current_type` already tracks `short type = N`.
    # Treating the name as globally unique — this file's previous shape — resolved every one of
    # them to whatever the *last* `npcNetIds` in the whole file happened to be: one real Zombie
    # drop list ended up attributed to hundreds of unrelated lines across a dozen functions.
    def _collapse(m: re.Match[str]) -> str:
        return m.group(0).replace("\n", " ")

    text = re.sub(r"int\[\]\s+\w+\s*=\s*new int\[\d+\]\s*\{[^}]*\}", _collapse, text, flags=re.S)

    drops: dict[int, set[int]] = {}
    # Track every `short varName = N;` local, not just the one literally named `type` — several
    # register-methods (the Creeper's own `short type2 = 267;` inside `RegisterBoss_BOC`, found
    # independently while auditing this fix) use a second, differently-named local in the same
    # function, and `RegisterToNPC(type2, ...)` was previously unresolvable to any npc at all: the
    # old code checked only the exact literal variable name `type`, so every registration through
    # a second local silently vanished from `game` — not flagged wrong, just never counted.
    type_vars: dict[str, int] = {}
    # Track `IItemDropRule ruleName = RegisterToNPC(...)` so a *later* line's `ruleName.OnSuccess(
    # ...)` / `ruleName.OnFailedRoll(...)` — a separate statement, chained onto a rule object
    # rather than a fresh `RegisterToNPC(...)` call — still resolves to the npc that rule was
    # registered to. `RegisterBoss_FrostMoon`'s own `rule`/`rule2`/`rule3` locals are exactly this
    # shape: `IItemDropRule rule = RegisterToNPC(344, new LeadingConditionRule(condition));` on one
    # line, then three or more `rule.OnSuccess(ItemDropRule.ByCondition(...));`-style statements on
    # the lines after it, none of which mention `RegisterToNPC` at all. Every item those carried —
    # the whole of the Frost Moon bosses' loot — was invisible to `game` for exactly this reason.
    #
    # Bound to a *set* of npcs, not one, because the shape also occurs with
    # `RegisterToMultipleNPCs` (`RegisterBoss_EOW`'s own
    # `IItemDropRule rule = RegisterToMultipleNPCs(new LeadingConditionRule(...), npcNetIds);`
    # followed by two `rule.OnSuccess(MasterMode...)` lines), which the old single-npc binding
    # could not represent, so those two lines fell through to whatever `rule` had last meant.
    rule_vars: dict[str, set[int]] = {}
    arrays: dict[str, set[int]] = {}
    # Items chained onto a rule variable *before* anything registered it, and the child rules a
    # parent rule carries. Both are flushed by `bind` the moment the name gains an npc.
    pending: dict[str, set[int]] = {}
    aliases: dict[str, set[str]] = {}

    def bind(name: str, targets: set[int]) -> None:
        """Give a rule variable its npcs, and hand them on to whatever was waiting on it."""
        rule_vars[name] = set(targets)
        for item in pending.pop(name, set()):
            for npc in targets:
                drops.setdefault(npc, set()).add(item)
        for child in aliases.pop(name, set()):
            if child != name:
                bind(child, targets)

    for raw in text.splitlines():
        line = raw.strip()

        # These are all method locals. `rule`, `type` and `npcNetIds` are redeclared in method
        # after method, so a scan that never forgets them attributes one method's chained
        # `rule.OnSuccess(...)` lines to whatever npc a *previous* method's `rule` named. That is
        # how Everscream came to "owe" the player the Eater of Worlds' relic and pet, plus the
        # whole of `RegisterToGlobal`'s Christmas present pool.
        if re.match(r"(?:private|public|internal)\s+[\w\[\]<>]+\s+\w+\(", line):
            type_vars.clear()
            rule_vars.clear()
            arrays.clear()
            # A rule chained onto a name that this method never registers is dead in the source
            # too; dropping it at the boundary keeps one method's `leadingConditionRule` from
            # picking up the next method's items.
            pending.clear()
            aliases.clear()
            continue

        if m := re.match(r"short (\w+) = (\d+);", line):
            type_vars[m.group(1)] = int(m.group(2))
            continue
        if m := re.match(r"int\[\]\s+(\w+)\s*=\s*new int\[\d+\]\s*\{([^}]*)\}", line):
            arrays[m.group(1)] = {int(n) for n in NUM.findall(m.group(2)) if int(n) >= 0}
            continue

        # A rule built on its own line and registered on a later one:
        #
        #     IItemDropRule itemDropRule = ItemDropRule.Common(1312, 20);
        #     ... four `itemDropRule.OnFailedRoll(...)` lines ...
        #     RegisterToNPC(629, itemDropRule);
        #
        # The item is on the *declaration*, and nothing on the register line names it, so the Ice
        # Mimic's own Toy Sled read as ours-only rather than as a drop the game registers. Held
        # against the name and flushed by `bind`, exactly like a chained one. A declaration that
        # builds no rule (`Conditions.NotExpert condition = new Conditions.NotExpert();`) names no
        # item and contributes nothing.
        if (
            "RegisterToNPC(" not in line
            and "RegisterToMultipleNPCs(" not in line
            and (
                m := re.match(
                    r"([\w.<>\[\]]+)\s+(\w+)\s*=\s*(?:new\s|ItemDropRule\.)", line
                )
            )
        ):
            # A declaration whose *type* is a rule is registered as a name even when it carries no
            # item yet, so a later line can hang something off it. `RegisterBoss_Plantera` declares
            # `LeadingConditionRule leadingConditionRule2 = new LeadingConditionRule(condition);`
            # and only fills it in three lines later; without the name existing here, the
            # `leadingConditionRule.OnSuccess(leadingConditionRule2)` that attaches it to the
            # already-registered parent has nothing to attach. A `Conditions.NotExpert condition =`
            # is not a rule and is left out, so nothing can be chained onto a condition by mistake.
            if m.group(1).endswith("Rule") or m.group(1).endswith("Rule[]"):
                pending.setdefault(m.group(2), set())
            if items := _items_in(line):
                pending.setdefault(m.group(2), set()).update(items)
            continue

        # A rule-chain continuation line has neither call in it — it is `ruleVar.OnSuccess(...)`
        # or `ruleVar.OnFailedRoll(...)`, referring back to a rule a previous line registered, or
        # a *later* one will.
        chain_target: set[int] | None = None
        if "RegisterToNPC(" not in line and "RegisterToMultipleNPCs(" not in line:
            if m := re.match(r"(\w+)\.(?:OnSuccess|OnFailedRoll|OnFailedConditions)\(", line):
                name = m.group(1)
                chain_target = rule_vars.get(name)
                if chain_target is None:
                    # Not bound *yet*. `RegisterBoss_Twins` builds its whole tree before it
                    # registers anything:
                    #
                    #     LeadingConditionRule leadingConditionRule = new (Conditions.MissingTwin);
                    #     LeadingConditionRule leadingConditionRule2 = new (Conditions.NotExpert);
                    #     leadingConditionRule.OnSuccess(ItemDropRule.BossBag(3326));
                    #     leadingConditionRule.OnSuccess(leadingConditionRule2);
                    #     leadingConditionRule2.OnSuccess(ItemDropRule.Common(2106, 7));
                    #     ... three more
                    #     RegisterToMultipleNPCs(leadingConditionRule, 126, 125);
                    #
                    # A strictly forward scan drops every one of those lines on the floor, which
                    # is why the Twins' entire non-expert table (Souls of Sight, Hallowed Bars,
                    # the treasure bag, both master-mode drops) was invisible on the game side.
                    # Hold the items against the name instead, and flush them when the name is
                    # bound below. `parent.OnSuccess(child)` with a bare identifier is an alias,
                    # so binding the parent binds the child too.
                    # A rule variable hung off another can be the sole argument, one of several
                    # (`OnSuccess(itemDropRule, hideLootReport: true)`), or nested inside a rule
                    # built inline (`OnFailedConditions(new OneFromRulesRule(1, itemDropRule,
                    # ...))`). Plantera's Grenade Launcher and its 50-150 rockets are the third
                    # shape, and reading only the first left them looking like loot we invented.
                    #
                    # The sole-identifier form aliases unconditionally, because the name it points
                    # at may not exist yet: `RegisterBoss_Twins` writes
                    # `leadingConditionRule.OnSuccess(leadingConditionRule2);` before
                    # `leadingConditionRule2` has been given anything, and requiring it to be known
                    # already loses the Twins' whole non-expert table. The wider forms only alias a
                    # name already seen, since anything else is an ordinary argument.
                    if inner := re.fullmatch(
                        r"\w+\.(?:OnSuccess|OnFailedRoll|OnFailedConditions)"
                        r"\(\s*([A-Za-z_]\w*)\s*\);?",
                        line,
                    ):
                        aliases.setdefault(name, set()).add(inner.group(1))
                        continue
                    named = {
                        tok
                        for tok in re.findall(r"(?<![\w.])([A-Za-z_]\w*)", line[len(name) + 1 :])
                        if tok in pending or tok in aliases or tok in rule_vars
                    }
                    if named:
                        aliases.setdefault(name, set()).update(named)
                    if items := _items_in(line):
                        pending.setdefault(name, set()).update(items)
                    continue
            if chain_target is None:
                continue

        # Which NPCs this line registers to.
        targets: set[int] = set()
        if chain_target is not None:
            targets |= chain_target
            # A rule hung off one that is *already* bound. The pending/alias branch above only
            # runs while the parent is still unresolved, and `RegisterBoss_Plantera` registers its
            # outer rule on the line after declaring it, so by the time
            # `leadingConditionRule.OnSuccess(leadingConditionRule2)` arrives the parent is bound
            # and the child was dropped. That lost the Grenade Launcher and its 50-150 rockets,
            # which then read as loot this project had invented for Plantera.
            for tok in re.findall(r"(?<![\w.])([A-Za-z_]\w*)", line[line.index("(") :]):
                if tok in pending or tok in aliases:
                    bind(tok, targets)
        elif m := re.search(r"RegisterToNPC\((\w+)\s*,", line):
            tok = m.group(1)
            if tok.isdigit():
                targets.add(int(tok))
            elif tok in type_vars:
                targets.add(type_vars[tok])
            # Anything else (a method call, an unrecognised variable) is left unresolved rather
            # than guessed at, same as `RegisterToMultipleNPCs`'s own id-list handling below.
        elif "RegisterToMultipleNPCs(" in line:
            open_at = line.index("RegisterToMultipleNPCs(") + len("RegisterToMultipleNPCs")
            close_at = _find_matching_paren(line, open_at)
            if close_at != -1:
                inner = line[open_at + 1 : close_at]
                # The first top-level argument is the rule expression; everything after it is the
                # id list — a bare integer literal, or a named `int[]` declared earlier.
                args = _split_top_level(inner)
                for tok in args[1:]:
                    tok = tok.strip()
                    if re.fullmatch(r"-?\d+", tok):
                        targets.add(int(tok))
                    elif tok in arrays:
                        targets.update(arrays[tok])
                    # Anything else (a method call, an unrecognised variable) is left unresolved
                    # rather than guessed at — silence here is a missed check, not a wrong one.

        if not targets:
            continue

        # Remember a rule variable's npcs so a later `ruleVar.OnSuccess(...)` continuation line
        # (caught by `chain_target` above) can resolve back to them. Both register calls take this
        # shape: `RegisterBoss_EOW` assigns a `RegisterToMultipleNPCs` to a local and then chains
        # the Eater of Worlds' master-mode relic and pet onto it a line later.
        if m := re.match(
            r"(?:[\w.<>\[\]]+\s+)?(\w+)\s*=\s*RegisterTo(?:NPC|MultipleNPCs)\(", line
        ):
            bind(m.group(1), targets)
        # The other half of the same shape, and the one that was missing. `RegisterBoss_KingSlime`
        # declares the rule on its own line and then *passes* it in, rather than assigning the
        # register call's result:
        #
        #     LeadingConditionRule leadingConditionRule = new LeadingConditionRule(...);
        #     RegisterToNPC(type, leadingConditionRule);
        #     leadingConditionRule.OnSuccess(ItemDropRule.Common(2430, 4));
        #     ... five more OnSuccess lines
        #
        # The binding above needs an `=` on the register line and there is none here, so the name
        # was never bound and every one of those `OnSuccess` lines resolved to no npc at all. King
        # Slime, Queen Slime, the Empress, Plantera and the two mechanical bosses each register
        # their whole non-expert loot table this way: 69 items across 21 NPCs were invisible on the
        # game side of this comparison, so the checker could not have reported any of them missing.
        # Found by `tools/mutate_tables.py`, which noticed that corrupting those rows in our own
        # table changed nothing.
        if m := re.search(r"RegisterToNPC\(\s*\w+\s*,\s*([A-Za-z_]\w*)\s*\)", line):
            bind(m.group(1), targets)
        elif m := re.search(r"RegisterToMultipleNPCs\(\s*([A-Za-z_]\w*)\s*,", line):
            bind(m.group(1), targets)
        # ...and any rule variable this line merely *mentions*, wherever in the expression it sits.
        # The Ice Golem's is nested two deep:
        #
        #     IItemDropRule itemDropRule = ItemDropRule.Common(3107, 25);
        #     IItemDropRule itemDropRule2 = ItemDropRule.WithRerolls(3107, 1, 25);
        #     itemDropRule.OnSuccess(ItemDropRule.Common(3108, 1, 100, 200), ...);
        #     RegisterToNPC(463, new LeadingConditionRule(c)).OnSuccess(
        #         new DropBasedOnExpertMode(itemDropRule, itemDropRule2));
        #
        # Neither name is the register call's own argument, so neither pattern above reaches them
        # and npc 463 had *no* loot at all on the game side of this comparison. Only names already
        # holding pending items are considered, and `pending` is cleared at every method boundary,
        # so this cannot reach across functions.
        for name in list(pending):
            if re.search(rf"(?<![\w.]){re.escape(name)}(?![\w])", line):
                bind(name, targets)

        items = _items_in(line)

        for npc in targets:
            if items:
                drops.setdefault(npc, set()).update(items)

    return drops


def _expand_npcs(label: str) -> list[int]:
    """`13..=15 | 20` -> `[13, 14, 15, 20]`. A match-arm label's range is inclusive on both ends,
    and taking only the two digit tokens the old code did (`13`, `15`) silently dropped every id
    in between — 14 was never checked against anything for exactly that reason."""
    npcs: list[int] = []
    for part in re.split(r"\s*\|\s*", label.strip()):
        if m := re.fullmatch(r"(\d+)\.\.=(\d+)", part):
            npcs.extend(range(int(m.group(1)), int(m.group(2)) + 1))
        elif part.isdigit():
            npcs.append(int(part))
    return npcs


# Global drop rules: `RegisterToGlobal(...)`, which hangs a rule off *every* NPC rather than a type.
#
# The per-npc comparison below is structurally blind to these - there is no npc to key them by - so
# they went unchecked in both directions until 2026-09-05, and all sixteen of them turned out to be
# missing from this server entirely. What that cost, before it was measured: the five biome keys and
# the Desert Key never dropped, so none of the Dungeon's six biome chests could ever be opened; the
# Pirate Map never dropped, so a Pirate Invasion could not be summoned by ordinary play; and the four
# hardmode yoyos, the two Halloween weapons, the Goodie Bag, the Present and the Living Fire Block
# had no source at all.
#
# The check is set membership rather than per-npc: a global rule is satisfied if the item can be
# produced anywhere in `conditional_drops.rs`, since which NPC carries it is the rule's own
# condition's business.
GLOBAL_DEFERRED: dict[int, str] = {
    1533: "Jungle Key: needs `Conditions.JungleKeyCondition`'s jungle-biome flag, which "
          "`Conditions` does not carry yet",
    1534: "Corruption Key: the same, for the corruption",
    1535: "Crimson Key: the same, for the crimson",
    1536: "Hallowed Key: the same, for the hallow",
    1537: "Frozen Key: the same, for the snow biome",
    4714: "Desert Key: the same, for the desert",
    1315: "Pirate Map: needs the ocean-and-surface flag `Conditions.PirateMap` reads",
    3282: "Cascade: needs `Conditions.YoyoCascade`'s depth and biome test",
    3286: "Yelets: the same",
    3289: "Amarok: the same",
    3290: "Hel-Fire: the same",
    1825: "Halloween weapon: needs the Halloween season flag",
    1827: "Halloween weapon: the same",
    1774: "Goodie Bag: the same",
    1869: "Present: needs the Christmas season flag",
    2701: "Living Fire Block: needs `Conditions.LivingFlames`' biome test",
}


def check_globals(root: Path, repo: Path) -> list[str]:
    """Every `RegisterToGlobal` item, against what our table can produce anywhere."""
    text = (root / "Terraria.GameContent.ItemDropRules" / "ItemDropDatabase.cs").read_text(
        errors="replace"
    )
    wanted: set[int] = set()
    for line in text.splitlines():
        if "RegisterToGlobal(" in line and "public IItemDropRule" not in line:
            wanted |= _items_in(line)
    if not wanted:
        raise SystemExit("parsed 0 RegisterToGlobal items; ItemDropDatabase.cs's shape changed")
    cond = (repo / "crates" / "terrustia-proto" / "src" / "conditional_drops.rs").read_text()
    cond = re.sub(r"^[ \t]*//.*$", "", cond, flags=re.M)
    ours = {
        int(n)
        for n in re.findall(r"(?:always|sometimes|a_few|m_in_n)\((\d+)", cond)
    }
    missing = sorted(wanted - ours)
    deferred = [f"  item {item}: {GLOBAL_DEFERRED[item]}" for item in missing if item in GLOBAL_DEFERRED]
    gaps = [
        f"  item {item}: registered globally and produced nowhere here, and not on the deferred list"
        for item in missing
        if item not in GLOBAL_DEFERRED
    ]
    return deferred, gaps


def parse_ours(root: Path) -> dict[int, set[int]]:
    """What our two tables give, keyed the same way."""
    ours: dict[int, set[int]] = {}

    flat = (root / "crates" / "terrustia-proto" / "src" / "npc_drops.rs").read_text()
    # Split into arms by their `N =>` markers and take everything up to the next one. Matching an
    # arm's closing brace with a regex silently swallowed most of the file: the shapes differ
    # between a one-line arm and a multi-chain one, and a non-greedy match spanned several.
    marker = re.compile(r"^        (\d+(?:\s*\|\s*\d+)*) => ", re.M)
    starts = [(m.start(), m.end(), m.group(1)) for m in marker.finditer(flat)]
    for nth, (_, body_at, label) in enumerate(starts):
        end = starts[nth + 1][0] if nth + 1 < len(starts) else len(flat)
        body = flat[body_at:end]
        items = {int(i) for i in re.findall(r"item: (\d+)", body)}
        for npc in _expand_npcs(label):
            ours.setdefault(npc, set()).update(items)

    cond = (root / "crates" / "terrustia-proto" / "src" / "conditional_drops.rs").read_text()
    # Comments go first, before any scan below reads it. `classic_only`'s Queen Slime arm explains
    # in prose that a stray `a_few(4958, 20, 1, 1)` used to be there, and that line sits between
    # npc 636's arm and npc 657's, so the arm scan read the Empress as dropping Queen Slime's
    # trophy. Prose about a drop is not a drop.
    cond = re.sub(r"^[ \t]*//.*$", "", cond, flags=re.M)
    for m in re.finditer(
        r"^        (\d+(?:\.\.=\d+)?(?:\s*\|\s*\d+)*) => vec!\[(.*?)\],\n", cond, re.S | re.M
    ):
        npcs = _expand_npcs(m.group(1))
        items = {int(i) for i in re.findall(r"(?:always|sometimes|a_few|m_in_n)\((\d+)", m.group(2))}
        for npc in npcs:
            ours.setdefault(npc, set()).update(items)
    # A handful of drops in `conditional()` are gated by an `if npc_type == N { ... }` guard
    # instead of living in that npc's own match arm — the Eye of Cthulhu's world-evil ore is one,
    # correctly implemented and tested, but invisible to the scan above since it is not a `vec!`
    # arm at all. Brace-matched by hand because the block can nest (an `if/else` inside it, in
    # EoC's case) and a regex cannot balance that reliably.
    #
    # The Mimic (85) in `one_from()` is the same shape with a different body: its two pools differ
    # by `at.hard_mode`, so it is handled ahead of that function's own match as an early-return
    # `if`/`else`, each branch a bare `&[&[items]]` array rather than an `always/sometimes/a_few`
    # call — a second call shape this scan needs to recognise on top of the first. The Headless
    # Horseman (315) is a third: its rate is computed at runtime (the pumpkin moon's wave gate), so
    # it is a `Conditional { item: N, ... }` struct literal rather than any of the above.
    for m in re.finditer(r"if npc_type == (\d+)[^{]*\{", cond):
        npc = int(m.group(1))
        depth, i = 1, m.end()
        while depth > 0 and i < len(cond):
            if cond[i] == "{":
                depth += 1
            elif cond[i] == "}":
                depth -= 1
            i += 1
        body = cond[m.end() : i]
        items = {int(n) for n in re.findall(r"(?:always|sometimes|a_few|m_in_n)\((\d+)", body)}
        for arr in re.finditer(r"&\[([\d,\s]+)\]", body):
            items.update(int(n) for n in re.findall(r"\d+", arr.group(1)))
        items.update(int(n) for n in re.findall(r"item:\s*(\d+)", body))
        # `for item in [5624, 5625, ...] { out.push(always(item)); }` is a bare array literal
        # rather than a `&[...]` slice, and the only thing that carries Skeleton's five
        # RedHatSkeletron drops. They read as missing loot on a boss until this was added.
        for arr in re.finditer(r"in \[([\d,\s]+)\]", body):
            items.update(int(n) for n in re.findall(r"\d+", arr.group(1)))
        ours.setdefault(npc, set()).update(items)
    # `if matches!(npc_type, 6 | 7 | 173..=181) { out.push(...) }` is a third guard shape, and the
    # one the Don't Starve crossover drops use, where one item goes to a long list of NPCs that
    # would be noise as twenty separate match arms.
    for m in re.finditer(r"if matches!\(\s*npc_type,\s*([^)]*)\)\s*\{", cond):
        npcs = _expand_npcs(m.group(1).replace("\n", " "))
        depth, i = 1, m.end()
        while depth > 0 and i < len(cond):
            if cond[i] == "{":
                depth += 1
            elif cond[i] == "}":
                depth -= 1
            i += 1
        body = cond[m.end() : i]
        items = {int(n) for n in re.findall(r"(?:always|sometimes|a_few|m_in_n)\((\d+)", body)}
        for npc in npcs:
            ours.setdefault(npc, set()).update(items)
    # `conditional()`'s own dominant style is neither of the above: `match npc_type { N =>
    # out.push(...), N2 => { out.push(...); out.push(...); } ... }`, imperative pushes into `out`
    # rather than a returned `vec![]`. The two scans above only ever saw a match block's *first*
    # arm's contents (a bare `=> vec![...]` scan doesn't apply here at all, and there is no
    # single `if npc_type ==` guard around the whole thing) — every arm of every such block was
    # invisible to this checker, including the hardmode-materials block and any match added since.
    # Brace-matched the same way as the `if npc_type ==` scan above: find each `match npc_type {`,
    # balance to its closing brace, then split the block on arm markers (reusing the same
    # `NUM (| NUM)*` / `NUM..=NUM` marker shape gen_drops.py's own npc_drops.rs scan uses) so each
    # arm's items are attributed only to that arm's own NPCs, not smeared across the whole match.
    #
    # Two more real blind spots, found closing the 66-gap drop-table pass: a guard clause
    # (`156 if at.hard_mode =>`) fell entirely outside the old marker regex, which required `=>`
    # right after the pattern — the whole arm, guard included, was invisible, not just missed. And
    # `chance_pools()` (a chance-gated `OneFromOptions` pool: roll `1` in `one_in` first, then draw
    # one item) writes its items through `pool(one_in, &[items])`, a call shape this scanner never
    # looked for — it walked `chance_pools()`'s own `match npc_type {}` block same as any other
    # (nothing here is specific to `conditional()`) but found nothing worth keeping in it.
    arm_marker = re.compile(
        r"(?:^|\n)\s*(\d+(?:\.\.=\d+)?(?:\s*\|\s*\d+)*)(?:\s+if\s+[^=\n]*)?\s*=>", re.M
    )
    for m in re.finditer(r"match npc_type \{", cond):
        depth, i = 1, m.end()
        while depth > 0 and i < len(cond):
            if cond[i] == "{":
                depth += 1
            elif cond[i] == "}":
                depth -= 1
            i += 1
        block = cond[m.end() : i - 1]
        arm_starts = [(am.start(), am.end(), am.group(1)) for am in arm_marker.finditer(block)]
        for nth, (_, body_at, label) in enumerate(arm_starts):
            arm_end = arm_starts[nth + 1][0] if nth + 1 < len(arm_starts) else len(block)
            arm_body = block[body_at:arm_end]
            items = {int(n) for n in re.findall(r"(?:always|sometimes|a_few|m_in_n)\((\d+)", arm_body)}
            # `pool`'s own first argument is a chance denominator, not an item — sometimes a
            # literal, sometimes (the pumpkin moon's wave-scaled gate, the goblin summoner's
            # mode-scaled one) an expression computed at runtime. Either way this scan cannot and
            # need not evaluate it: only the `&[...]` item list after the comma is a drop.
            # `pool`'s gate can itself be a call with its own comma
            # (`pool(pumpkin_moon_gate_denominator(wave, at.expert), &[...])`), so the list is
            # found on its own rather than by walking past a `[^,]+` gate that cannot span one.
            # Nothing else in this file writes a bare `&[1, 2, 3]` of integers that is not a list
            # of items.
            for pool_call in re.finditer(r"&\[([\d,\s]+)\]", arm_body):
                items.update(int(n) for n in re.findall(r"\d+", pool_call.group(1)))
            # A `Conditional { item: N, ... }` struct literal — needed when the rate itself is
            # computed at runtime (the same pumpkin-moon gate above) and so cannot go through the
            # `sometimes`/`a_few` constructors, which take a compile-time rate.
            items.update(int(n) for n in re.findall(r"item:\s*(\d+)", arm_body))
            if not items:
                continue
            for npc in _expand_npcs(label):
                ours.setdefault(npc, set()).update(items)
    # The bag, trophy and mask maps, and the one-from pools.
    # The bag, trophy, mask and lunar-fragment maps, and the one-from pools.
    #
    # Scoped to the functions that actually hold a npc-to-item map, not to the whole file. A bare
    # `N => M,` at eight spaces of indent is a *shape*, not a meaning: this file also contains
    # `pumpkin_moon_trophy_gate_denominator`, whose `match wave { 15 | 16 => 4, 17 | 18 => 3, .. }`
    # maps a moon wave to a chance denominator. Read as a drop map it gave NPCs 15, 16, 17 and 18
    # items 4 and 3 that nothing anywhere gives them, and (worse than the noise) anything the
    # game really does register as item 3 or 4 on those four NPCs would have been masked by it.
    # The reverse-direction report added below is what surfaced it.
    map_bodies = "".join(
        m.group(0)
        for m in re.finditer(
            r"^pub fn (?:treasure_bag|trophy|mask|lunar_fragment|one_from|moon_lord_weapons)\("
            r".*?^\}$",
            cond,
            re.S | re.M,
        )
    )
    for pattern in (
        r"^        (\d+(?:\.\.=\d+)?(?:\s*\|\s*\d+)*) => (\d+),",
        # `one_from` pools, which may be one list or several on a line — or, once `rustfmt` wraps a
        # long enough one (the Flying Dutchman's nineteen-item pool does), several lines. `\s` in
        # place of a literal space covers that; `re.S` lets `.` span the newlines within.
        r"^        (\d+) => &\[((?:&\[[\d,\s]+\],?\s*)+)\],",
    ):
        for m in re.finditer(pattern, map_bodies, re.M):
            npcs = _expand_npcs(m.group(1))
            items = {int(n) for n in re.findall(r"\d+", m.group(2))}
            for npc in npcs:
                ours.setdefault(npc, set()).update(items)

    # `bundled_with(weapon) -> (ammunition, min, max)`: an item the caller drops automatically
    # whenever the weapon lands, so anything that has the weapon also has its ammunition. Golem's
    # Stynger Bolt, the two pumpkin-moon launchers' shells and the Nail Gun's Nails have no other
    # source at all, and read as missing loot on their bosses without this.
    bundled = {
        int(k): int(v)
        for k, v in re.findall(r"^\s*(\d+) => Some\(\((\d+),", cond, re.M)
    }
    for items in ours.values():
        items.update(bundled[item] for item in list(items) if item in bundled)

    return ours


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 1
    game_root = Path(sys.argv[1])
    repo = Path(__file__).resolve().parent.parent

    game = parse_game(game_root)
    ours = parse_ours(repo)

    boss_gaps: list[str] = []
    other_gaps: list[str] = []
    deferred_seen: set[int | tuple[int, int]] = set()
    for npc in sorted(game):
        missing = set()
        for item in game[npc] - ours.get(npc, set()):
            for key in (item, (npc, item)):
                if key in DEFERRED:
                    deferred_seen.add(key)
                    break
            else:
                missing.add(item)
        if not missing:
            continue
        line = f"  npc {npc}: missing {sorted(missing)}"
        (boss_gaps if npc in BOSSES else other_gaps).append(line)

    # The other direction: what we hand out that the game's own database does not register.
    #
    # This half did not exist. The comparison ran one way only, so a drop invented here, or copied
    # onto the wrong NPC, was not merely unreported - it was unreportable, and mutating one of
    # those rows in `conditional_drops.rs` changed nothing a checker could see (measured by
    # `tools/mutate_tables.py`; three of its surviving mutants were exactly this).
    #
    # Printed rather than gated. An entry here is not automatically a bug: some of it is loot
    # vanilla registers outside `ItemDropDatabase` (the legacy `NPC.NPCLoot` path), and each one
    # needs its own trace through the game before it can be called wrong or excused. The list is
    # the backlog; a count would be the thing this file already refuses to print elsewhere.
    extras = {
        npc: sorted(items - game.get(npc, set()))
        for npc, items in sorted(ours.items())
        if items - game.get(npc, set())
    }

    print(f"game registers drops for {len(game)} NPC types; we have {len(ours)}")
    print(f"deferred drops seen and excused: {len(deferred_seen)} of {len(DEFERRED)}")
    # An excuse nothing needs any more is an excuse that will one day cover a real gap. Naming
    # them keeps the list honest without failing the build over it.
    if stale := sorted(map(str, DEFERRED.keys() - deferred_seen)):
        print(f"deferred but no longer missing (drop from DEFERRED): {', '.join(stale)}")
    print()
    if boss_gaps:
        print(f"BOSSES MISSING LOOT ({len(boss_gaps)}):")
        print("\n".join(boss_gaps))
        print()
    if other_gaps:
        print(f"ORDINARY ENEMIES MISSING LOOT ({len(other_gaps)}):")
        print("\n".join(other_gaps[:MAX_LISTED]))
        if len(other_gaps) > MAX_LISTED:
            print(f"  ... and {len(other_gaps) - MAX_LISTED} more")
        print()
    if extras:
        total = sum(len(v) for v in extras.values())
        print(f"WE DROP WHAT THE DATABASE DOES NOT REGISTER ({total} items, {len(extras)} NPCs):")
        for npc, items in extras.items():
            print(f"  npc {npc}: extra {items}")
        print("  Each is loot on an NPC the game does not give it to: an over-drop, an item on")
        print("  the wrong type, or one invented here. Every one of the fourteen this used to")
        print("  list turned out to be one of those or a hole in this checker; none was an")
        print("  excusable difference, which is why the direction is gated now.")
        print()
    global_deferred, global_gaps = check_globals(Path(sys.argv[1]), repo)
    if global_deferred:
        print(f"GLOBAL RULES WITH NO SOURCE HERE, DEFERRED ({len(global_deferred)}):")
        print("\n".join(global_deferred))
        print("  A `RegisterToGlobal` rule hangs off every NPC, so nothing keys it by type and the")
        print("  per-npc comparison above cannot see it. Each line names what it still needs.")
        print()
    if global_gaps:
        print(f"GLOBAL RULES WITH NO SOURCE HERE ({len(global_gaps)}):")
        print("\n".join(global_gaps))
        print()

    if boss_gaps or other_gaps or extras or global_gaps:
        print("Every gap above is either a bug or a decision. If it is a decision, put the item")
        print("in DEFERRED at the top of this file with the reason, so it stays visible; do not")
        print("widen a rule to make it disappear. A count with no list is how 102 items went")
        print("unreachable for a year.")
        return 1
    print("every drop the game registers is reachable here, or deferred on the record.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
