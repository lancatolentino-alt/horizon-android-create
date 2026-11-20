//! TerraFirmaCraft's `tfc:tall_wild_crop`, `tfc:emergent_plant` and `tfc:submerged_tall_plant`
//! (EUPL 1.2): two-block plants placed with the block's `placeTwoHalves`.
//!
//! The mod's plants are vanilla look-alikes without TFC's fluid property, so the water around
//! them is not kept: a half that stands in water replaces it.

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    optional,
    placement::PlacementContext,
    required,
    tfc_tree::{is_air, single_block_replaceable},
};
use crate::registry::BlockStateId;
use horizon_core::random::RandomSource;
use std::sync::Arc;

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    TallWildCrop,
    Emergent,
    Submerged,
}

#[derive(Clone, Debug)]
pub struct TfcTallPlantsFeature {
    kind: Kind,
    lower: BlockStateId,
    upper: BlockStateId,
    names: Arc<Vec<String>>,
}

fn is_fluid(n: &str) -> bool {
    matches!(n, "minecraft:water" | "minecraft:lava")
}

/// `isAirOrEmptyFluid || single block replaceable`.
fn replaceable(n: &str) -> bool {
    is_air(n) || is_fluid(n) || single_block_replaceable(n)
}

impl TfcTallPlantsFeature {
    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(json)?;
        let kind = match serde_json::from_str::<String>(required(&object, "type")?.get())?.as_str()
        {
            "tfc:tall_wild_crop" => Kind::TallWildCrop,
            "tfc:emergent_plant" => Kind::Emergent,
            "tfc:submerged_tall_plant" => Kind::Submerged,
            _ => return Err(DecodeError::Field("tall plant type")),
        };
        let config: Object = match optional(&object, "config") {
            Some(config) => serde_json::from_str(config.get())?,
            None => object.clone(),
        };
        let block: String = serde_json::from_str(required(&config, "block")?.get())?;
        let registry = bindings.registry();
        let default = registry
            .default_state(&block)
            .ok_or_else(|| DecodeError::Registry(format!("unknown block {block}")))?;
        let template = registry.state(default).unwrap().clone();
        // `PART` is lower/upper on water plants and bottom/top on double crops.
        let halves = [
            ("part", "lower", "upper"),
            ("part", "bottom", "top"),
            ("half", "lower", "upper"),
        ];
        let (mut lower, mut upper) = (default, default);
        for (property, low, high) in halves {
            if registry.property_values(&block, property).contains(&low) {
                let pick = |value: &str| {
                    let mut state = template.clone();
                    state.properties.insert(property.into(), value.into());
                    registry
                        .id(&state)
                        .ok_or(DecodeError::Field("tall plant state"))
                };
                lower = pick(low)?;
                upper = pick(high)?;
                break;
            }
        }
        Ok(Self {
            kind,
            lower,
            upper,
            names: Arc::new(
                (0..registry.len())
                    .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
                    .collect(),
            ),
        })
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        _random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let we = |e| FeatureExecutionError::Basic(FeatureError::World(e));
        let above = [origin[0], origin[1] + 1, origin[2]];
        if self.kind != Kind::TallWildCrop {
            let bottom = world.block(origin).map_err(we)?;
            let top = world.block(above).map_err(we)?;
            let (bottom, top) = (
                self.names[bottom.0 as usize].as_str(),
                self.names[top.0 as usize].as_str(),
            );
            let fluid_ok = match self.kind {
                // The top is out of the water.
                Kind::Emergent => !is_fluid(top),
                // Both halves are in the same fluid.
                _ => is_fluid(top) == is_fluid(bottom) && (!is_fluid(top) || top == bottom),
            };
            // `placeTwoHalves` only places when the bottom holds a fluid.
            if !(fluid_ok && replaceable(bottom) && replaceable(top) && bottom == "minecraft:water")
            {
                return Ok(true);
            }
        }
        world.set_block(origin, self.lower, 2).map_err(we)?;
        world.set_block(above, self.upper, 2).map_err(we)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{registry::BlockRegistry, worldgen::tfc_tide_pool::tests::Flat};
    use horizon_core::random::XoroshiroRandom;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    fn registry() -> &'static BlockRegistry {
        let mut rows = Vec::new();
        let mut defaults = BTreeMap::new();
        let mut add = |name: &str, air: bool, fluid: bool, key: &str, values: &[&str]| {
            defaults.insert(name.to_owned(), rows.len());
            if values.is_empty() {
                rows.push(json!([name, air, fluid, "", {"id": name, "properties": {}}]));
            }
            for v in values {
                rows.push(json!([name, air, fluid, "", {"id": name, "properties": {key: v}}]));
            }
        };
        for n in ["air", "cave_air", "void_air"] {
            add(&format!("minecraft:{n}"), true, false, "", &[]);
        }
        add("minecraft:stone", false, false, "", &[]);
        let levels: Vec<String> = (0..16).map(|l| l.to_string()).collect();
        let levels: Vec<&str> = levels.iter().map(String::as_str).collect();
        add("minecraft:water", false, true, "level", &levels);
        add(
            "tfc:plant/cordgrass",
            false,
            false,
            "part",
            &["lower", "upper"],
        );
        add(
            "tfc:wild_crop/jute",
            false,
            false,
            "part",
            &["bottom", "top"],
        );
        add("tfc:plant/single", false, false, "", &[]);
        let facts = json!({"states": rows, "defaults": defaults});
        Box::leak(Box::new(
            BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
        ))
    }

    fn feature(kind: &str, block: &str) -> TfcTallPlantsFeature {
        let r = registry();
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        TfcTallPlantsFeature::decode(
            &json!({"type": kind, "block": block}).to_string(),
            &bindings,
        )
        .unwrap()
    }

    /// Water from y 0 to `top` (inclusive), air above.
    fn world(top: i32) -> Flat {
        let r = registry();
        let water = r.default_state("minecraft:water").unwrap();
        let mut blocks = HashMap::new();
        for y in 0..=top {
            blocks.insert([0, y, 0], water);
        }
        Flat {
            blocks,
            surface: 0,
            rock: r.default_state("minecraft:stone").unwrap(),
            air: r.default_state("minecraft:air").unwrap(),
        }
    }

    fn name_at(w: &Flat, p: [i32; 3]) -> String {
        let r = registry();
        let s = w.blocks.get(&p).copied().unwrap_or(w.air);
        let st = r.state(s).unwrap();
        format!(
            "{}:{}",
            st.name,
            st.properties.get("part").map_or("", |v| v.as_str())
        )
    }

    #[test]
    fn emergent_plants_need_water_below_and_air_above() {
        let f = feature("tfc:emergent_plant", "tfc:plant/cordgrass");
        let mut w = world(5);
        f.place(&mut w, &mut XoroshiroRandom::new(1), [0, 5, 0])
            .unwrap();
        assert_eq!(name_at(&w, [0, 5, 0]), "tfc:plant/cordgrass:lower");
        assert_eq!(name_at(&w, [0, 6, 0]), "tfc:plant/cordgrass:upper");
        // Water above too: the plant is not emergent, so nothing grows.
        let mut w = world(8);
        f.place(&mut w, &mut XoroshiroRandom::new(1), [0, 5, 0])
            .unwrap();
        assert_eq!(name_at(&w, [0, 5, 0]), "minecraft:water:");
    }

    #[test]
    fn submerged_plants_need_water_on_both_halves() {
        let f = feature("tfc:submerged_tall_plant", "tfc:plant/cordgrass");
        let mut w = world(8);
        f.place(&mut w, &mut XoroshiroRandom::new(1), [0, 5, 0])
            .unwrap();
        assert_eq!(name_at(&w, [0, 5, 0]), "tfc:plant/cordgrass:lower");
        assert_eq!(name_at(&w, [0, 6, 0]), "tfc:plant/cordgrass:upper");
        let mut w = world(5);
        f.place(&mut w, &mut XoroshiroRandom::new(1), [0, 5, 0])
            .unwrap();
        assert_eq!(
            name_at(&w, [0, 5, 0]),
            "minecraft:water:",
            "the top is in air"
        );
    }

    #[test]
    fn wild_crops_always_place_both_halves_and_single_blocks_fall_back() {
        let f = feature("tfc:tall_wild_crop", "tfc:wild_crop/jute");
        let mut w = world(0);
        f.place(&mut w, &mut XoroshiroRandom::new(1), [0, 10, 0])
            .unwrap();
        assert_eq!(name_at(&w, [0, 10, 0]), "tfc:wild_crop/jute:bottom");
        assert_eq!(name_at(&w, [0, 11, 0]), "tfc:wild_crop/jute:top");
        let f = feature("tfc:tall_wild_crop", "tfc:plant/single");
        f.place(&mut w, &mut XoroshiroRandom::new(1), [0, 20, 0])
            .unwrap();
        assert_eq!(name_at(&w, [0, 21, 0]), "tfc:plant/single:");
    }
}
