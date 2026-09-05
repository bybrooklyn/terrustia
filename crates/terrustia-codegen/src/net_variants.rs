//! `crates/terrustia-proto/src/net_variants.rs` — what each negative net id changes about the
//! type it rides on.
//!
//! A negative net id is not an NPC type. It names a *variant* of a positive one, which is how the
//! coloured slimes, the small and big zombies and skeletons, and the hornet families all ride on
//! one base type each. `NPC.SetDefaults` routes a negative straight to `SetDefaultsFromNetId`
//! (`NPC.cs:8460-8463`), which resolves the base through `NetIdMap`, calls `SetDefaults` on it,
//! and then applies a scale and a handful of stat overrides from one switch.
//!
//! That switch is this table. Sixty-five entries, and fifteen of them override nothing but the
//! scale (or nothing at all): a Big Stinger really is a Stinger at 1.15x with two npc slots, and
//! nothing else.
//!
//! `color` and `alpha` are read but not emitted: both are client-side rendering, and this server
//! sends the net id itself, so a real client applies its own.

use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;

use crate::csharp::read_lossy;

/// One variant's overrides, all optional: a case that sets nothing is a base type at 1x.
#[derive(Default, Clone, Copy)]
struct Variant {
    scale: Option<f64>,
    damage: Option<i64>,
    defense: Option<i64>,
    life: Option<i64>,
    knockback_mul: Option<f64>,
    height: Option<i64>,
    npc_slots: Option<f64>,
    value: Option<f64>,
    rarity: Option<i64>,
}

fn parse(root: &Path) -> BTreeMap<i64, Variant> {
    let text = read_lossy(&root.join("Terraria/NPC.cs"));
    let start = text
        .find("private void SetDefaultsFromNetId(")
        .expect("no SetDefaultsFromNetId");
    let body = &text[start..];

    let case = Regex::new(r"^\s*case (-\d+):").expect("case");
    let scale =
        Regex::new(r"^\s*SetDefaults_ForNetId\(num, spawnparams, ([0-9.]+)f?\)").expect("scale");
    let int_field =
        Regex::new(r"^\s*(damage|defense|life|rarity|height) = (-?[0-9]+);").expect("int");
    let float_field = Regex::new(r"^\s*(value|npcSlots) = ([0-9.]+)f?;").expect("float");
    let knockback = Regex::new(r"^\s*knockBackResist \*= ([0-9.]+)f?;").expect("knockback");

    let mut out: BTreeMap<i64, Variant> = BTreeMap::new();
    let mut current: Option<i64> = None;
    for line in body.lines() {
        if let Some(caps) = case.captures(line) {
            current = caps[1].parse().ok();
            if let Some(id) = current {
                out.entry(id).or_default();
            }
            continue;
        }
        // The switch ends at its own `default:`; everything after belongs to another routine.
        if line.trim_start().starts_with("default:") && !out.is_empty() {
            break;
        }
        let Some(id) = current else { continue };
        let entry = out.entry(id).or_default();
        if let Some(caps) = scale.captures(line) {
            entry.scale = caps[1].parse().ok();
        } else if let Some(caps) = int_field.captures(line) {
            let value = caps[2].parse().ok();
            match &caps[1] {
                "damage" => entry.damage = value,
                "defense" => entry.defense = value,
                "life" => entry.life = value,
                "rarity" => entry.rarity = value,
                _ => entry.height = value,
            }
        } else if let Some(caps) = float_field.captures(line) {
            let value = caps[2].parse().ok();
            if &caps[1] == "value" {
                entry.value = value;
            } else {
                entry.npc_slots = value;
            }
        } else if let Some(caps) = knockback.captures(line) {
            entry.knockback_mul = caps[1].parse().ok();
        }
    }
    out
}

fn option<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "None".into(), |v| format!("Some({v})"))
}

/// The same for a float field. `{}` on an `f64` drops a trailing `.0`, which emits an integer
/// literal into an `Option<f32>` field and does not compile - so the decimal point goes in here
/// rather than depending on whether the game happened to write a round number.
fn option_f32(value: Option<f64>) -> String {
    value.map_or_else(
        || "None".into(),
        |v| {
            let text = format!("{v}");
            if text.contains('.') {
                format!("Some({text})")
            } else {
                format!("Some({text}.0)")
            }
        },
    )
}

pub fn generate(root: &Path) -> String {
    let variants = parse(root);
    assert!(
        variants.len() >= 60,
        "only {} variants parsed; the switch shape must have changed",
        variants.len()
    );

    let mut lines: Vec<String> = vec![
        "//! What each negative net id changes about the type it rides on, generated from".into(),
        "//! `NPC.SetDefaultsFromNetId`.".into(),
        "//!".into(),
        "//! A negative net id names a *variant* of a positive type rather than a type of its own:"
            .into(),
        "//! the coloured slimes, the small and big zombies and skeletons, and the hornet families"
            .into(),
        "//! all ride on one base type each. [`crate::npc_data::from_net_id`] resolves which, and"
            .into(),
        "//! this says what changes: a scale, and a handful of stat overrides.".into(),
        "//!".into(),
        "//! Fifteen entries override nothing but the scale, or nothing at all. That is not a"
            .into(),
        "//! parsing miss - a Big Stinger really is a Stinger at 1.15x with two npc slots.".into(),
        "//!".into(),
        "//! `color` and `alpha` are deliberately absent. Both are client-side rendering, and a"
            .into(),
        "//! server that sends the net id itself has a real client applying its own.".into(),
        "//!".into(),
        "//! Generated by `terrustia-codegen` from Terraria 1.4.5.7. Do not edit by hand.".into(),
        "".into(),
        "/// What one negative net id overrides. `None` everywhere means the base type unchanged."
            .into(),
        "#[derive(Debug, Clone, Copy, PartialEq, Default)]".into(),
        "pub struct NetVariant {".into(),
        "    /// `SetDefaults_ForNetId`'s own third argument, applied to the sprite and hitbox."
            .into(),
        "    pub scale: Option<f32>,".into(),
        "    pub damage: Option<i32>,".into(),
        "    pub defense: Option<i32>,".into(),
        "    pub life: Option<i32>,".into(),
        "    /// A *multiplier* on the base type's own knockback resistance, not a replacement."
            .into(),
        "    pub knockback_mul: Option<f32>,".into(),
        "    pub height: Option<i32>,".into(),
        "    pub npc_slots: Option<f32>,".into(),
        "    pub value: Option<f32>,".into(),
        "    pub rarity: Option<i32>,".into(),
        "}".into(),
        "".into(),
        format!(
            "/// How many negative net ids the game defines ({}, `NPCID.NegativeIDCount` being -66).",
            variants.len()
        ),
        format!("pub const NET_VARIANTS: usize = {};", variants.len()),
        "".into(),
        "/// The overrides for a negative net id, or `None` for a positive one (which is a type,"
            .into(),
        "/// not a variant) or an id past the end of the game's own switch.".into(),
        "pub fn net_variant(net_id: i16) -> Option<NetVariant> {".into(),
        "    Some(match net_id {".into(),
    ];
    for (id, v) in &variants {
        lines.push(format!("        {id} => NetVariant {{"));
        lines.push(format!("            scale: {},", option_f32(v.scale)));
        lines.push(format!("            damage: {},", option(v.damage)));
        lines.push(format!("            defense: {},", option(v.defense)));
        lines.push(format!("            life: {},", option(v.life)));
        lines.push(format!(
            "            knockback_mul: {},",
            option_f32(v.knockback_mul)
        ));
        lines.push(format!("            height: {},", option(v.height)));
        lines.push(format!(
            "            npc_slots: {},",
            option_f32(v.npc_slots)
        ));
        lines.push(format!("            value: {},", option_f32(v.value)));
        lines.push(format!("            rarity: {},", option(v.rarity)));
        lines.push("        },".into());
    }
    lines.extend(
        [
            "        _ => return None,",
            "    })",
            "}",
            "",
            "#[cfg(test)]",
            "mod tests {",
            "    use super::*;",
            "",
            "    /// The table is populated, and did not silently regenerate empty.",
            "    #[test]",
            "    fn every_negative_net_id_the_game_defines_is_here() {",
            "        for id in 1..=NET_VARIANTS {",
            "            let id = -(id as i16);",
            "            assert!(net_variant(id).is_some(), \"net id {id}\");",
            "        }",
            "        assert!(net_variant(-(NET_VARIANTS as i16) - 1).is_none());",
            "        assert!(net_variant(0).is_none(), \"a positive id is a type, not a variant\");",
            "        assert!(net_variant(1).is_none());",
            "    }",
            "",
            "    /// Spot-checks against source, one per shape: a full override, a scale-only one,",
            "    /// and one that changes nothing at all.",
            "    #[test]",
            "    fn the_spot_checks_match_the_games_own_switch() {",
            "        // `case -4:` the Pinky (`NPC.cs`), the rare Slime Rain variant: a 0.6x slime",
            "        // with 150 life and a hundred silver on it.",
            "        let pinky = net_variant(-4).expect(\"-4\");",
            "        assert_eq!(pinky.scale, Some(0.6));",
            "        assert_eq!(pinky.life, Some(150));",
            "        assert_eq!(pinky.damage, Some(5));",
            "        assert_eq!(pinky.value, Some(10000.0));",
            "        assert_eq!(pinky.rarity, Some(2));",
            "",
            "        // `case -13:` a scale and nothing else.",
            "        let scaled = net_variant(-13).expect(\"-13\");",
            "        assert_eq!(scaled.scale, Some(0.9));",
            "        assert_eq!(scaled.life, None);",
            "",
            "        // `case -11:` the base type at its own size, which is a real case and not a",
            "        // parsing miss.",
            "        assert_eq!(net_variant(-11), Some(NetVariant::default()));",
            "    }",
            "}",
        ]
        .map(String::from),
    );
    lines.join("\n") + "\n"
}
