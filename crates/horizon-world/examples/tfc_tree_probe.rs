//! Decodes the TFC tree and forest features of a `.hmod` data folder through Horizon's catalog.
//!
//! `cargo run -p horizon-world --example tfc_tree_probe -- <hmod dir>`
use horizon_world::{
    registry::BlockRegistry,
    worldgen::{
        block_feature::FeatureBlockBindings,
        feature_catalog::FeatureCatalog,
        structure::{StateTransforms, StructureResources},
        tags::TagDefinitions,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::Arc,
};

fn walk(dir: &Path, base: &Path, ns: &str, out: &mut BTreeMap<String, String>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, base, ns, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            let rel = path.strip_prefix(base).unwrap().with_extension("");
            out.insert(
                format!("{ns}:{}", rel.to_string_lossy()),
                fs::read_to_string(&path).unwrap(),
            );
        }
    }
}
fn names(text: &str, out: &mut BTreeSet<String>) {
    for part in text.split('"') {
        if let Some((ns, path)) = part.split_once(':') {
            if !ns.is_empty()
                && ns.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                && !path.is_empty()
                && path
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_/.".contains(c))
            {
                out.insert(part.to_owned());
            }
        }
    }
}

fn main() {
    let root = std::env::args().nth(1).expect("hmod dir");
    let data = Path::new(&root).join("data");
    let (mut features, mut placed) = (BTreeMap::new(), BTreeMap::new());
    for ns in fs::read_dir(&data).unwrap().flatten() {
        let n = ns.file_name().to_string_lossy().into_owned();
        let w = ns.path().join("worldgen");
        walk(
            &w.join("configured_feature"),
            &w.join("configured_feature"),
            &n,
            &mut features,
        );
        walk(
            &w.join("placed_feature"),
            &w.join("placed_feature"),
            &n,
            &mut placed,
        );
    }
    let mut block_names: BTreeSet<String> =
        ["minecraft:air", "minecraft:cave_air", "minecraft:void_air"]
            .map(String::from)
            .into();
    for j in features.values().chain(placed.values()) {
        names(j, &mut block_names);
    }
    let mut rows = Vec::new();
    let mut defaults = BTreeMap::new();
    for n in &block_names {
        defaults.insert(n.clone(), rows.len());
        if n == "minecraft:water" || n == "minecraft:lava" {
            for level in 0..16 {
                rows.push(serde_json::json!([n, false, true, "", {"id": n, "properties": {"level": level.to_string()}}]));
            }
        } else {
            rows.push(
                serde_json::json!([n, n.ends_with("air"), false, "", {"id": n, "properties": {}}]),
            );
        }
    }
    let facts = serde_json::json!({"states": rows, "defaults": defaults});
    let registry: &'static BlockRegistry = Box::leak(Box::new(
        BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
    ));
    let resources = Arc::new(StructureResources::new(
        registry,
        StateTransforms::default(),
        &data,
        BTreeMap::new(),
        &BTreeMap::new(),
    ));
    let bindings = FeatureBlockBindings::new(registry, BTreeSet::new())
        .unwrap()
        .with_structures(resources);
    let catalog =
        FeatureCatalog::from_resources(&features, &placed, &TagDefinitions::default()).unwrap();
    let (mut ok, mut bad) = (0, 0);
    for name in features.keys().filter(|n| {
        n.starts_with("tfc:tree/") && !n.ends_with("_vanilla") && !n.ends_with("_entry")
            || n.ends_with("_disc")
            || n.ends_with("forest")
    }) {
        match catalog.feature_with_blocks(&format!("\"{name}\""), &bindings) {
            Ok(_) => ok += 1,
            Err(e) => {
                bad += 1;
                println!("{name}: {e}");
            }
        }
    }
    println!("{ok} decoded, {bad} failed");
}
