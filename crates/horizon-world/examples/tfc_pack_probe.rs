//! Loads the TFC worldgen data pack's noises and density functions through Horizon's own codec,
//! samples them, and prints terrain / climate statistics and the rock-band cut points.
//!
//! `cargo run -p horizon-world --example tfc_pack_probe -- <pack dir> [cuts.json out]`
use horizon_core::density::{SamplerContext, graph::DensityCompiler};
use horizon_world::worldgen::density::DensityRegistry;
use std::{collections::BTreeMap, fs, path::Path};

fn collect(root: &Path, kind: &str, out: &mut BTreeMap<String, String>) {
    let data = root.join("data");
    for ns in fs::read_dir(&data).unwrap().flatten() {
        let base = ns.path().join(kind);
        if base.exists() {
            walk(&base, &base, &ns.file_name().to_string_lossy(), out);
        }
    }
}
fn walk(base: &Path, dir: &Path, ns: &str, out: &mut BTreeMap<String, String>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(base, &path, ns, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            let rel = path.strip_prefix(base).unwrap().with_extension("");
            out.insert(
                format!("{ns}:{}", rel.to_string_lossy().replace('\\', "/")),
                fs::read_to_string(&path).unwrap(),
            );
        }
    }
}

fn quantile(sorted: &[f32], q: f64) -> f32 {
    sorted[((sorted.len() - 1) as f64 * q) as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = Path::new(&args[1]);
    let mut noises = BTreeMap::new();
    let mut densities = BTreeMap::new();
    collect(root, "worldgen/noise", &mut noises);
    collect(root, "worldgen/density_function", &mut densities);
    // The material rule's noise_threshold reads a noise at (x, 0, z): probe the same thing.
    for rock in ["top", "mid", "deep"] {
        densities.insert(
            format!("probe:rock_{rock}"),
            format!(r#"{{"type":"minecraft:noise","noise":"tfc:rock_{rock}","xz_scale":1.0,"y_scale":0.0}}"#),
        );
    }
    let registry = DensityRegistry::from_json(&noises, &densities).expect("pack decodes");
    let ids: BTreeMap<String, _> = [
        "tfc:height",
        "tfc:temperature",
        "tfc:rainfall",
        "tfc:land",
        "tfc:erosion",
        "tfc:final_density",
        "probe:rock_top",
        "probe:rock_mid",
        "probe:rock_deep",
    ]
    .iter()
    .map(|n| {
        (
            n.to_string(),
            registry.value(n).unwrap_or_else(|| panic!("missing {n}")),
        )
    })
    .collect();
    let compiler = DensityCompiler::new_seeded(registry.into_graph(), 1234, false);
    let samplers: BTreeMap<_, _> = ids
        .iter()
        .map(|(n, id)| {
            (
                n.clone(),
                compiler
                    .compile_raw(*id)
                    .unwrap_or_else(|e| panic!("{n}: {e:?}")),
            )
        })
        .collect();
    let mut ctx = SamplerContext::default();
    let mut sample = |name: &str, x: i32, y: i32, z: i32| -> f32 {
        samplers[name].sample_value(&mut ctx, [x, y, z]).unwrap()
    };

    // 160 x 160 grid, 100 blocks apart: a 16 km square.
    let mut grids: BTreeMap<&str, Vec<f32>> = BTreeMap::new();
    for name in [
        "tfc:height",
        "tfc:temperature",
        "tfc:rainfall",
        "tfc:land",
        "probe:rock_top",
        "probe:rock_mid",
        "probe:rock_deep",
    ] {
        let mut v = Vec::new();
        for gx in 0..160 {
            for gz in 0..160 {
                v.push(sample(name, (gx - 80) * 100, 64, (gz - 80) * 100));
            }
        }
        grids.insert(name, v);
    }
    for (name, v) in &grids {
        let mut s = v.clone();
        s.sort_by(|a, b| a.total_cmp(b));
        println!(
            "{name:18} min {:7.2} p10 {:7.2} p50 {:7.2} p90 {:7.2} max {:7.2}",
            s[0],
            quantile(&s, 0.1),
            quantile(&s, 0.5),
            quantile(&s, 0.9),
            s[s.len() - 1]
        );
    }
    let h = &grids["tfc:height"];
    let ocean = h.iter().filter(|v| **v < 63.0).count() as f64 / h.len() as f64;
    let high = h.iter().filter(|v| **v > 140.0).count() as f64 / h.len() as f64;
    println!(
        "share below sea level (63): {:.1}%   above 140: {:.1}%",
        ocean * 100.0,
        high * 100.0
    );
    // Solid column check: density > 0 below the surface, < 0 above it, at a few spots.
    for (x, z) in [(0, 0), (1500, -900), (-3000, 2200), (6000, 6000)] {
        let top = sample("tfc:height", x, 64, z);
        let below = sample("tfc:final_density", x, (top as i32 - 20).max(-60), z);
        let above = sample("tfc:final_density", x, (top as i32 + 20).min(310), z);
        println!("({x},{z}) height {top:.1} density 20 below {below:.2} 20 above {above:.2}");
    }
    for z in [0, 1500, 3000, 4500, 6000, 7500, 9000, 12000] {
        println!(
            "temperature at z={z}: {:.2}",
            sample("tfc:temperature", 0, 64, z)
        );
    }
    if let Some(out) = args.get(2) {
        let mut json = serde_json::Map::new();
        for (layer, bands) in [("top", 7), ("mid", 7), ("deep", 7)] {
            let mut s = grids[format!("probe:rock_{layer}").as_str()].clone();
            s.sort_by(|a, b| a.total_cmp(b));
            let cuts: Vec<f64> = (1..bands)
                .map(|k| (quantile(&s, k as f64 / bands as f64) as f64 * 10000.0).round() / 10000.0)
                .collect();
            json.insert(layer.into(), serde_json::json!(cuts));
        }
        fs::write(out, serde_json::to_string_pretty(&json).unwrap()).unwrap();
        println!("rock cut points written to {out}");
    }
}
