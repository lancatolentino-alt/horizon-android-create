//! TerraFirmaCraft-style terrain, climate and rock layers, built into the generator.
//!
//! This is the Overworld's world generation, written as the same noise, density-function and
//! material-rule entries a data pack would hold, then laid over the prepared bundles through
//! [`WorldgenOverrides`]. Nothing is read from disk, so it works where data packs do not
//! (Android). Rock layers place the `tfc:raw_*` blocks of the TFC content mod, which must be
//! installed.
//!
//! * Terrain: ocean and land from a continent noise, rolling hills, inland ridged mountains.
//! * Climate: temperature follows latitude (hot at z = 0, coldest at |z| = 6000, mirrored every
//!   6000 blocks) with some wobble; rainfall is its own noise. Vanilla biomes follow these.
//! * Rock layers: sedimentary rock near the surface, mixed rock in the middle, intrusive and
//!   metamorphic rock below y = 20. Which rock shows is chosen by a regional noise.
//!
//! `HORIZON_TFC_WORLDGEN=0` turns it off. It also stays off until the game reports that the `tfc`
//! content mod is on (`set_mod_enabled`), because the rock layers place that mod's blocks.
use super::overrides::WorldgenOverrides;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

static MOD_ON: AtomicBool = AtomicBool::new(false);

/// Tells the generator whether the `tfc` content mod is on. Read when a world's worldgen loads.
pub fn set_mod_enabled(on: bool) {
    MOD_ON.store(on, Ordering::Relaxed);
}

const TOP: [&str; 7] = [
    "shale",
    "claystone",
    "limestone",
    "conglomerate",
    "dolomite",
    "chert",
    "chalk",
];
const MID: [&str; 7] = [
    "rhyolite",
    "andesite",
    "dacite",
    "basalt",
    "quartzite",
    "slate",
    "phyllite",
];
const DEEP: [&str; 7] = [
    "granite", "diorite", "gabbro", "schist", "gneiss", "marble", "tuff",
];
/// Cut points that split each rock noise into seven equally common bands (the noise's septiles,
/// measured over a 16 km square).
const TOP_CUTS: [f64; 6] = [-0.3596, -0.1893, -0.0586, 0.063, 0.1949, 0.3631];
const MID_CUTS: [f64; 6] = [-0.3463, -0.1849, -0.0618, 0.0555, 0.1776, 0.3445];
const DEEP_CUTS: [f64; 6] = [-0.3368, -0.1775, -0.0558, 0.0585, 0.1806, 0.3377];

/// Whether the native TFC overworld replaces the vanilla one (mod on and not disabled).
pub fn active() -> bool {
    enabled() && MOD_ON.load(Ordering::Relaxed)
}

/// Whether the TFC generator is on (it is unless `HORIZON_TFC_WORLDGEN=0`).
pub fn enabled() -> bool {
    std::env::var("HORIZON_TFC_WORLDGEN").map_or(true, |v| v != "0")
}

/// Lays the TFC entries over `out` (call before any data pack's, so packs still win).
pub fn apply(out: &mut WorldgenOverrides) {
    if !enabled() || !MOD_ON.load(Ordering::Relaxed) {
        return;
    }
    for (bundle, id, json) in entries() {
        out.set(bundle, &id, json.to_string());
    }
}

fn noise(octave: i32, count: i32) -> Value {
    json!({"base_octave": octave, "octave_count": count, "base_amplitude": 1.0})
}
/// A 2D (y-independent) sample of the named noise.
fn n2d(name: &str) -> Value {
    json!({"type": "minecraft:noise", "noise": format!("tfc:{name}"), "xz_scale": 1.0, "y_scale": 0.0})
}
fn mul(a: Value, b: Value) -> Value {
    json!({"type": "minecraft:mul", "left": a, "right": b})
}
fn add(a: Value, b: Value) -> Value {
    json!({"type": "minecraft:add", "left": a, "right": b})
}
fn sub(a: Value, b: Value) -> Value {
    json!({"type": "minecraft:sub", "left": a, "right": b})
}
fn clamp(x: Value, lo: f64, hi: f64) -> Value {
    json!({"type": "minecraft:clamp", "input": x, "min": lo, "max": hi})
}
fn grad(axis: &str, from: i32, to: i32, a: f64, b: f64, tiling: &str) -> Value {
    json!({"type": "minecraft:gradient", "axis": axis, "from_coordinate": from, "to_coordinate": to,
        "from_value": a, "to_value": b, "tiling": tiling})
}
fn block(name: &str) -> Value {
    json!({"type": "minecraft:block", "result_state": name})
}
fn cond(if_true: Value, then_run: Value) -> Value {
    json!({"type": "minecraft:condition", "if_true": if_true, "then_run": then_run})
}
fn depth_floor(offset: i32, secondary: i32, add_surface_depth: bool) -> Value {
    json!({"type": "minecraft:stone_depth", "offset": offset, "add_surface_depth": add_surface_depth,
        "secondary_depth_range": secondary, "surface_type": "floor"})
}
fn no_water() -> Value {
    json!({"type": "minecraft:water", "offset": 0, "surface_depth_multiplier": 0, "add_stone_depth": false})
}

/// One rock layer: the first band whose range holds the noise value places that raw rock.
fn band_rule(noise_name: &str, rocks: &[&str; 7], cuts: &[f64; 6]) -> Value {
    let mut edges = vec![-1000.0];
    edges.extend_from_slice(cuts);
    edges.push(1000.0);
    let mut sequence: Vec<Value> = (0..6)
        .map(|i| {
            cond(
                json!({"type": "minecraft:noise_threshold", "noise": format!("tfc:{noise_name}"),
                    "min_threshold": edges[i], "max_threshold": edges[i + 1]}),
                block(&format!("tfc:raw_{}", rocks[i])),
            )
        })
        .collect();
    sequence.push(block(&format!("tfc:raw_{}", rocks[6])));
    json!({"type": "minecraft:sequence", "sequence": sequence})
}

/// Every (bundle, id, JSON) entry of the generator.
pub fn entries() -> Vec<(&'static str, String, Value)> {
    let mut out: Vec<(&'static str, String, Value)> = Vec::new();
    let mut noises = |name: &str, octave, count| {
        out.push(("noises", format!("tfc:{name}"), noise(octave, count)))
    };
    noises("continent", -11, 4);
    noises("hills", -7, 4);
    noises("mountains", -9, 4);
    noises("detail", -4, 3);
    noises("rainfall", -10, 3);
    noises("temp_wobble", -8, 2);
    noises("erosion", -9, 3);
    noises("rock_top", -8, 3);
    noises("rock_mid", -8, 3);
    noises("rock_deep", -8, 3);

    let mut d = |name: &str, v: Value| out.push(("densities", format!("tfc:{name}"), v));
    // Terrain. land: -1 (deep ocean) to 1 (far inland); sea level is 63.
    d(
        "land",
        clamp(
            add(mul(n2d("continent"), json!(1.7)), json!(0.25)),
            -1.0,
            1.0,
        ),
    );
    d("hills", mul(n2d("hills"), json!(40.0)));
    d(
        "mountain_mask",
        clamp(
            mul(sub(json!("tfc:land"), json!(0.1)), json!(2.0)),
            0.0,
            1.0,
        ),
    );
    d(
        "mountain_ridge",
        clamp(
            sub(
                json!(1.0),
                json!({"type": "minecraft:abs", "input": mul(n2d("mountains"), json!(3.0))}),
            ),
            0.0,
            1.0,
        ),
    );
    d(
        "mountain_amount",
        mul(
            json!("tfc:mountain_mask"),
            json!({"type": "minecraft:square", "input": "tfc:mountain_ridge"}),
        ),
    );
    d(
        "height",
        add(
            add(
                add(json!(66.0), mul(json!("tfc:land"), json!(22.0))),
                json!("tfc:hills"),
            ),
            mul(json!("tfc:mountain_amount"), json!(90.0)),
        ),
    );
    d("y", grad("y", -64, 320, -64.0, 320.0, "clamp_to_edge"));
    d(
        "final_density",
        json!({"type": "minecraft:interpolated", "cell_size_xz": 4, "cell_size_y": 8,
            "input": add(
                mul(sub(json!("tfc:height"), json!("tfc:y")), json!(0.1)),
                mul(json!({"type": "minecraft:noise", "noise": "tfc:detail", "xz_scale": 1.0, "y_scale": 1.0}), json!(0.25)))}),
    );
    d(
        "surface_level",
        json!({"type": "minecraft:interpolated", "cell_size_xz": 16, "cell_size_y": 1, "input": "tfc:height"}),
    );
    // Climate (the router feeds these to the multi-noise biome source).
    d(
        "temperature",
        clamp(
            add(
                grad("z", 0, 6000, 0.85, -0.85, "mirrored_repeat"),
                mul(n2d("temp_wobble"), json!(0.5)),
            ),
            -1.0,
            1.0,
        ),
    );
    d(
        "rainfall",
        clamp(mul(n2d("rainfall"), json!(1.8)), -1.0, 1.0),
    );
    d(
        "depth",
        mul(sub(json!("tfc:height"), json!("tfc:y")), json!(0.00625)),
    );
    d(
        "erosion",
        clamp(
            add(
                sub(json!(0.6), mul(json!("tfc:mountain_amount"), json!(1.8))),
                mul(n2d("erosion"), json!(0.6)),
            ),
            -1.0,
            1.0,
        ),
    );
    d(
        "ridges",
        clamp(mul(n2d("mountains"), json!(2.0)), -1.0, 1.0),
    );

    out.push((
        "settings",
        "minecraft:overworld".to_owned(),
        json!({
            "noise": {"min_y": -64, "height": 384},
            "default_block": "minecraft:stone",
            "default_fluid": "minecraft:water",
            "noise_router": {
                "chunk_surface_level": "tfc:surface_level",
                "continents": "tfc:land",
                "depth": "tfc:depth",
                "erosion": "tfc:erosion",
                "final_density": "tfc:final_density",
                "ridges": "tfc:ridges",
                "temperature": "tfc:temperature",
                "vegetation": "tfc:rainfall"
            },
            "material_rule": "tfc:surface",
            "spawn_target": [],
            "sea_level": 63,
            "disable_mob_generation": false,
            "legacy_random_source": false
        }),
    ));

    // Rock layers and soil.
    out.push((
        "rules",
        "tfc:top_rock".to_owned(),
        band_rule("rock_top", &TOP, &TOP_CUTS),
    ));
    out.push((
        "rules",
        "tfc:mid_rock".to_owned(),
        band_rule("rock_mid", &MID, &MID_CUTS),
    ));
    out.push((
        "rules",
        "tfc:deep_rock".to_owned(),
        band_rule("rock_deep", &DEEP, &DEEP_CUTS),
    ));
    out.push((
        "rules",
        "tfc:surface".to_owned(),
        json!({"type": "minecraft:sequence", "sequence": [
            cond(
                json!({"type": "minecraft:vertical_gradient", "random_name": "tfc:bedrock_floor",
                    "true_at_and_below": {"above_bottom": 0}, "false_at_and_above": {"above_bottom": 5}}),
                block("minecraft:bedrock")),
            cond(depth_floor(0, 0, false), json!({"type": "minecraft:sequence", "sequence": [
                cond(no_water(), block("minecraft:grass_block")), block("minecraft:gravel")]})),
            cond(depth_floor(2, 0, false), json!({"type": "minecraft:sequence", "sequence": [
                cond(no_water(), block("minecraft:dirt")), block("minecraft:gravel")]})),
            cond(depth_floor(22, 8, true), json!("tfc:top_rock")),
            cond(
                json!({"type": "minecraft:vertical_gradient", "random_name": "tfc:basement",
                    "true_at_and_below": {"absolute": 0}, "false_at_and_above": {"absolute": 20}}),
                json!("tfc:deep_rock")),
            json!("tfc:mid_rock")
        ]}),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::density::DensityRegistry;
    use horizon_core::density::{SamplerContext, graph::DensityCompiler};
    use std::collections::BTreeMap;

    fn bundle(name: &str) -> BTreeMap<String, String> {
        entries()
            .into_iter()
            .filter(|(b, _, _)| *b == name)
            .map(|(_, id, v)| (id, v.to_string()))
            .collect()
    }

    #[test]
    fn terrain_is_solid_below_the_surface_and_open_above_it() {
        let registry = DensityRegistry::from_json(&bundle("noises"), &bundle("densities")).unwrap();
        let (height, density) = (
            registry.value("tfc:height").unwrap(),
            registry.value("tfc:final_density").unwrap(),
        );
        let compiler = DensityCompiler::new_seeded(registry.into_graph(), 1234, false);
        let (height, density) = (
            compiler.compile_raw(height).unwrap(),
            compiler.compile_raw(density).unwrap(),
        );
        let mut ctx = SamplerContext::default();
        for (x, z) in [
            (0, 0),
            (1500, -900),
            (-3000, 2200),
            (6000, 6000),
            (-9000, -400),
        ] {
            let top = height.sample_value(&mut ctx, [x, 64, z]).unwrap() as i32;
            let below = density
                .sample_value(&mut ctx, [x, (top - 20).max(-60), z])
                .unwrap();
            let above = density
                .sample_value(&mut ctx, [x, (top + 20).min(310), z])
                .unwrap();
            assert!(
                below > 0.0 && above < 0.0,
                "({x},{z}) top {top}: {below} / {above}"
            );
        }
    }

    #[test]
    fn temperature_is_hot_at_the_equator_and_cold_every_six_thousand_blocks() {
        let registry = DensityRegistry::from_json(&bundle("noises"), &bundle("densities")).unwrap();
        let id = registry.value("tfc:temperature").unwrap();
        let compiler = DensityCompiler::new_seeded(registry.into_graph(), 7, false);
        let temperature = compiler.compile_raw(id).unwrap();
        let mut ctx = SamplerContext::default();
        let mut at = |z| temperature.sample_value(&mut ctx, [0, 64, z]).unwrap();
        assert!(at(0) > 0.3 && at(12000) > 0.3);
        assert!(at(6000) < -0.3);
    }

    #[test]
    fn rock_bands_cover_every_rock_once() {
        for (rocks, cuts) in [(&TOP, &TOP_CUTS), (&MID, &MID_CUTS), (&DEEP, &DEEP_CUTS)] {
            assert!(cuts.windows(2).all(|w| w[0] < w[1]));
            let rule = band_rule("rock_top", rocks, cuts).to_string();
            for rock in rocks {
                assert_eq!(
                    rule.matches(&format!("tfc:raw_{rock}\"")).count(),
                    1,
                    "{rock}"
                );
            }
        }
    }

    #[test]
    fn overworld_settings_point_at_the_tfc_functions_and_surface_rule() {
        let settings = bundle("settings");
        let overworld: Value = serde_json::from_str(&settings["minecraft:overworld"]).unwrap();
        for (_, f) in overworld["noise_router"].as_object().unwrap() {
            let id = f.as_str().unwrap();
            assert!(bundle("densities").contains_key(id), "{id}");
        }
        assert!(bundle("rules").contains_key(overworld["material_rule"].as_str().unwrap()));
    }

    #[test]
    fn apply_fills_the_override_bundles_only_while_the_tfc_mod_is_on() {
        set_mod_enabled(false);
        let mut off = WorldgenOverrides::default();
        apply(&mut off);
        assert_eq!(off.entries("settings").count(), 0);
        assert_eq!(off.entries("noises").count(), 0);

        set_mod_enabled(true);
        let mut out = WorldgenOverrides::default();
        apply(&mut out);
        assert!(
            out.entries("settings")
                .any(|(id, _)| id == "minecraft:overworld")
        );
        assert!(out.entries("rules").any(|(id, _)| id == "tfc:surface"));
        assert_eq!(out.entries("noises").count(), 10);
    }
}
