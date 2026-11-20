//! TerraFirmaCraft's `tfc:epiphyte_plant`, `tfc:spreading_crop` and `tfc:loose_rock` (EUPL 1.2):
//! `EpiphytePlantFeature`, `SpreadingCropFeature` and `LooseRockFeature`.
//!
//! The mod's plants and loose rocks are vanilla look-alikes without TFC's `facing`, `count` or
//! fluid properties; where the Java sets them the draws still happen, and the state is the
//! block's default. A loose rock that lands in water replaces the water.

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    optional,
    placement::PlacementContext,
    required, tfc_terrain,
    tfc_tree::{is_bush_plantable_on, replaceable, sturdy},
};
use crate::registry::BlockStateId;
use horizon_core::random::RandomSource;
use std::{collections::BTreeMap, sync::Arc};

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

/// `Direction.Plane.HORIZONTAL`: north, east, south, west.
const HORIZONTAL: [[i32; 2]; 4] = [[0, -1], [1, 0], [0, 1], [-1, 0]];

#[derive(Clone, Debug)]
enum Kind {
    Epiphyte,
    SpreadingCrop { fruit: BlockStateId },
    LooseRock,
}

#[derive(Clone, Debug)]
pub struct TfcSmallPlantsFeature {
    kind: Kind,
    block: BlockStateId,
    names: Arc<Vec<String>>,
    /// `rock name -> (loose, mossy loose)`.
    rocks: Arc<BTreeMap<String, (BlockStateId, Option<BlockStateId>)>>,
}

/// `BlockTags.LOGS`, by name.
fn is_log(name: &str) -> bool {
    name.starts_with("tfc:wood/log/")
        || name.starts_with("tfc:wood/wood/")
        || name
            .strip_prefix("minecraft:")
            .is_some_and(|p| p.ends_with("_log") || p.ends_with("_wood"))
}

fn is_ice(name: &str) -> bool {
    matches!(
        name,
        "minecraft:ice"
            | "minecraft:packed_ice"
            | "minecraft:blue_ice"
            | "minecraft:frosted_ice"
            | "tfc:sea_ice"
    )
}

impl TfcSmallPlantsFeature {
    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(json)?;
        let kind_name: String = serde_json::from_str(required(&object, "type")?.get())?;
        let config: Object = match optional(&object, "config") {
            Some(config) => serde_json::from_str(config.get())?,
            None => object.clone(),
        };
        let registry = bindings.registry();
        let state = |name: &str| {
            registry
                .default_state(name)
                .ok_or_else(|| DecodeError::Registry(format!("unknown block {name}")))
        };
        let names = Arc::new(
            (0..registry.len())
                .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
                .collect::<Vec<_>>(),
        );
        let mut rocks = BTreeMap::new();
        let (kind, block) = match kind_name.as_str() {
            "tfc:loose_rock" => {
                for name in names.iter() {
                    if let Some(rock) = name.strip_prefix("tfc:rock/loose/") {
                        let mossy = registry.default_state(&format!("tfc:rock/mossy_loose/{rock}"));
                        rocks.insert(rock.to_owned(), (state(name)?, mossy));
                    }
                }
                // The block is chosen per column from the surface rock.
                (Kind::LooseRock, state("minecraft:air")?)
            }
            "tfc:epiphyte_plant" | "tfc:spreading_crop" => {
                let name: String = serde_json::from_str(required(&config, "block")?.get())?;
                let block = state(&name)?;
                if kind_name == "tfc:epiphyte_plant" {
                    (Kind::Epiphyte, block)
                } else {
                    // `getFruit`: the wild melon and pumpkin bear vanilla fruit here.
                    let crop = name
                        .strip_prefix("tfc:wild_crop/")
                        .ok_or(DecodeError::Field("spreading crop block"))?;
                    (
                        Kind::SpreadingCrop {
                            fruit: state(&format!("minecraft:{crop}"))?,
                        },
                        block,
                    )
                }
            }
            _ => return Err(DecodeError::Field("small plant type")),
        };
        Ok(Self {
            kind,
            block,
            names,
            rocks: Arc::new(rocks),
        })
    }

    fn name<W: FeatureWorld>(&self, world: &mut W, pos: [i32; 3]) -> R<&str, W> {
        let state = world
            .block(pos)
            .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))?;
        Ok(self.names[state.0 as usize].as_str())
    }

    fn set<W: FeatureWorld>(&self, world: &mut W, pos: [i32; 3], state: BlockStateId) -> R<(), W> {
        world
            .set_block(pos, state, 2)
            .map(|_| ())
            .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        match &self.kind {
            Kind::Epiphyte => self.epiphyte(world, random, origin),
            Kind::SpreadingCrop { fruit } => self.spreading_crop(world, random, origin, *fruit),
            Kind::LooseRock => self.loose_rock(world, random, origin),
        }
    }

    /// `EpiphytePlantFeature`: somewhere up to 11 blocks above, against the side of a log.
    fn epiphyte<W: FeatureWorld>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let height = random.next_int_bound(12).unwrap();
        let pos = [origin[0], origin[1] + height, origin[2]];
        if !replaceable(self.name(world, pos)?) {
            return Ok(false);
        }
        for [dx, dz] in HORIZONTAL {
            // The plant faces `direction` and hangs from the log behind it.
            let attached = [pos[0] - dx, pos[1], pos[2] - dz];
            if is_log(self.name(world, attached)?) {
                self.set(world, pos, self.block)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `SpreadingCropFeature`: a fruit beside the vine, on ground a bush can grow on. Always true.
    fn spreading_crop<W: FeatureWorld>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
        fruit: BlockStateId,
    ) -> R<bool, W> {
        let [dx, dz] = HORIZONTAL[random.next_int_bound(4).unwrap() as usize];
        let offset = [origin[0] + dx, origin[1], origin[2] + dz];
        let below = [offset[0], offset[1] - 1, offset[2]];
        if is_bush_plantable_on(self.name(world, below)?) && replaceable(self.name(world, offset)?)
        {
            self.set(world, offset, fruit)?;
            self.set(world, origin, self.block)?;
        }
        Ok(true)
    }

    /// `LooseRockFeature`: the column's surface rock, mossy where it is wet.
    fn loose_rock<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let seed = world.world_seed().map_err(FeatureExecutionError::Query)?;
        let Some(terrain) = tfc_terrain::active_for(seed) else {
            return Ok(false);
        };
        let rock = terrain.surface_rock(origin[0], origin[2]);
        let Some(&(loose, mossy)) = self.rocks.get(rock) else {
            return Ok(false);
        };
        let at = self.name(world, origin)?.to_owned();
        let wet = at == "minecraft:water";
        let rainfall = terrain.climate_at(origin[0], origin[2]).rainfall;
        let state = match mossy {
            Some(mossy) if rainfall > 250.0 && random.next_boolean() && !wet => mossy,
            _ => loose,
        };
        let below = [origin[0], origin[1] - 1, origin[2]];
        let below_name = self.name(world, below)?;
        // `canSurvive` needs a sturdy floor, and the rock never grows on ice.
        if replaceable(&at) && sturdy(below_name) && !is_ice(below_name) {
            // The 1-2 rocks of `COUNT_1_3`.
            random.next_int_bound(2).unwrap();
            self.set(world, origin, state)?;
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{registry::BlockRegistry, worldgen::tfc_tide_pool::tests::Flat};
    use horizon_core::random::XoroshiroRandom;
    use serde_json::json;
    use std::collections::{BTreeSet, HashMap};

    const NAMES: [&str; 16] = [
        "minecraft:air",
        "minecraft:cave_air",
        "minecraft:void_air",
        "minecraft:water",
        "minecraft:stone",
        "minecraft:dirt",
        "minecraft:ice",
        "tfc:wood/log/oak",
        "minecraft:melon",
        "minecraft:pumpkin",
        "tfc:plant/vriesea",
        "tfc:wild_crop/melon",
        "tfc:rock/loose/granite",
        "tfc:rock/mossy_loose/granite",
        "tfc:rock/loose/basalt",
        "tfc:rock/mossy_loose/basalt",
    ];

    fn registry() -> &'static BlockRegistry {
        let rows: Vec<_> = NAMES
            .iter()
            .map(|n| {
                let water = *n == "minecraft:water";
                let props = if water {
                    json!({"level": "0"})
                } else {
                    json!({})
                };
                json!([n, n.ends_with("air"), water, "", {"id": n, "properties": props}])
            })
            .collect();
        let defaults: BTreeMap<_, _> = NAMES.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let facts = json!({"states": rows, "defaults": defaults});
        Box::leak(Box::new(
            BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
        ))
    }

    fn feature(kind: &str, block: Option<&str>) -> TfcSmallPlantsFeature {
        let r = registry();
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        let config = block.map_or(json!({}), |b| json!({"block": b}));
        TfcSmallPlantsFeature::decode(
            &json!({"type": kind, "config": config}).to_string(),
            &bindings,
        )
        .unwrap()
    }

    fn world() -> Flat {
        let r = registry();
        Flat {
            blocks: HashMap::new(),
            surface: 0,
            rock: r.default_state("minecraft:stone").unwrap(),
            air: r.default_state("minecraft:air").unwrap(),
        }
    }

    fn id(n: &str) -> BlockStateId {
        registry().default_state(n).unwrap()
    }

    #[test]
    fn epiphytes_cling_to_logs_within_twelve_blocks() {
        let f = feature("tfc:epiphyte_plant", Some("tfc:plant/vriesea"));
        let mut grown = BTreeSet::new();
        for seed in 0..60 {
            let mut w = world();
            // A trunk one block east of the origin column, 12 blocks tall.
            for y in 0..12 {
                w.blocks.insert([1, y, 0], id("tfc:wood/log/oak"));
            }
            let ok = f
                .place(&mut w, &mut XoroshiroRandom::new(seed), [0, 0, 0])
                .unwrap();
            let placed: Vec<_> = w
                .blocks
                .iter()
                .filter(|(_, s)| **s == id("tfc:plant/vriesea"))
                .map(|(p, _)| *p)
                .collect();
            assert_eq!(ok, placed.len() == 1);
            if let Some(p) = placed.first() {
                assert_eq!(p[0], 0);
                grown.insert(p[1]);
            }
        }
        assert!(grown.len() > 5, "heights vary: {grown:?}");
        // No log, no plant.
        let mut w = world();
        assert!(
            !f.place(&mut w, &mut XoroshiroRandom::new(1), [0, 0, 0])
                .unwrap()
        );
    }

    #[test]
    fn spreading_crops_bear_fruit_beside_them_on_soil() {
        let f = feature("tfc:spreading_crop", Some("tfc:wild_crop/melon"));
        let mut sides = BTreeSet::new();
        for seed in 0..40 {
            let mut w = world();
            for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
                w.blocks.insert([dx, -1, dz], id("minecraft:dirt"));
            }
            // The call always reports success.
            assert!(
                f.place(&mut w, &mut XoroshiroRandom::new(seed), [0, 0, 0])
                    .unwrap()
            );
            let fruit: Vec<_> = w
                .blocks
                .iter()
                .filter(|(_, s)| **s == id("minecraft:melon"))
                .map(|(p, _)| *p)
                .collect();
            assert_eq!(fruit.len(), 1);
            sides.insert(fruit[0]);
            assert_eq!(w.blocks[&[0, 0, 0]], id("tfc:wild_crop/melon"));
        }
        assert_eq!(sides.len(), 4, "every side is chosen: {sides:?}");
        // On stone nothing is placed.
        let mut w = world();
        w.blocks.insert([0, -1, -1], id("minecraft:stone"));
        assert!(
            f.place(&mut w, &mut XoroshiroRandom::new(2), [0, 0, 0])
                .unwrap()
        );
        assert!(w.blocks.values().all(|s| *s != id("minecraft:melon")));
    }

    #[test]
    fn loose_rocks_use_the_surface_rock_and_need_dry_footing() {
        crate::worldgen::tfc_tide_pool::tests::activate();
        let f = feature("tfc:loose_rock", None);
        let terrain = tfc_terrain::active_for(99).unwrap();
        // A column whose surface rock is one of the two in the test registry.
        let (x, z, rock) = (0..400)
            .map(|i| (i * 53 - 5000, i * 29 - 3000))
            .map(|(x, z)| (x, z, terrain.surface_rock(x, z)))
            .find(|(_, _, rock)| ["granite", "basalt"].contains(rock))
            .expect("a granite or basalt column");
        let mut w = world();
        w.blocks.insert([x, 9, z], id("minecraft:stone"));
        let ok = f
            .place(&mut w, &mut XoroshiroRandom::new(5), [x, 10, z])
            .unwrap();
        assert!(ok, "rock {rock}");
        {
            let placed = w.blocks[&[x, 10, z]];
            assert!(
                [
                    id(&format!("tfc:rock/loose/{rock}")),
                    id(&format!("tfc:rock/mossy_loose/{rock}"))
                ]
                .contains(&placed)
            );
        }
        // Ice below, or no floor: no rock.
        let mut w = world();
        w.blocks.insert([x, 9, z], id("minecraft:ice"));
        assert!(
            !f.place(&mut w, &mut XoroshiroRandom::new(5), [x, 10, z])
                .unwrap()
        );
        let mut w = world();
        assert!(
            !f.place(&mut w, &mut XoroshiroRandom::new(5), [x, 10, z])
                .unwrap()
        );
    }
}
