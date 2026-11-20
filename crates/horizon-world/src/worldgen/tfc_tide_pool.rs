//! TerraFirmaCraft's `tfc:tide_pool` (`TidePoolFeature`, EUPL 1.2): a small rock-rimmed pool of
//! salt water on the shore, some cells holding shellfish.

use super::{
    DecodeError,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    placement::PlacementContext,
    tfc_tree::is_air,
};
use crate::registry::BlockStateId;
use horizon_core::random::RandomSource;
use std::{collections::HashMap, sync::Arc};

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

/// `#tfc:tide_pool_blocks`, in tag order.
const TIDE_POOL_BLOCKS: [&str; 4] = [
    "tfc:groundcover/clam",
    "tfc:groundcover/mollusk",
    "tfc:groundcover/mussel",
    "tfc:groundcover/sea_urchin",
];

#[derive(Clone, Debug)]
pub struct TfcTidePoolFeature {
    /// Rock name -> `(raw, cobble)`.
    rocks: Arc<HashMap<String, (BlockStateId, BlockStateId)>>,
    water: BlockStateId,
    guano: Option<BlockStateId>,
    /// Shellfish that can hold water, as the water-filled state (`FluidHelpers.fillWithFluid`).
    shellfish: Vec<Option<BlockStateId>>,
    names: Arc<Vec<String>>,
}

impl TfcTidePoolFeature {
    pub(crate) fn decode(bindings: &FeatureBlockBindings<'_>) -> Result<Self, DecodeError> {
        let registry = bindings.registry();
        let names: Vec<String> = (0..registry.len())
            .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
            .collect();
        let mut rocks = HashMap::new();
        for name in &names {
            if let Some(rock) = name.strip_prefix("tfc:raw_")
                && let (Some(raw), Some(cobble)) = (
                    registry.default_state(name),
                    registry.default_state(&format!("tfc:cobble_{rock}")),
                )
            {
                rocks.insert(rock.to_owned(), (raw, cobble));
            }
        }
        let shellfish = TIDE_POOL_BLOCKS
            .iter()
            .map(|name| {
                let base = registry.default_state(name)?;
                let mut state = registry.state(base)?.clone();
                // Only a block with a `waterlogged` property can be filled with water.
                state.properties.get("waterlogged")?;
                state.properties.insert("waterlogged".into(), "true".into());
                registry.id(&state)
            })
            .collect();
        Ok(Self {
            rocks: Arc::new(rocks),
            // `TFCBlocks.SALT_WATER`: a mod's salt water when one is loaded, else vanilla water.
            water: registry
                .default_state("tfc:salt_water")
                .or_else(|| registry.default_state("minecraft:water"))
                .ok_or(DecodeError::Field("water"))?,
            guano: registry.default_state("tfc:groundcover/guano"),
            shellfish,
            names: Arc::new(names),
        })
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        if origin[1] > 67 || origin[1] < 62 {
            return Ok(false);
        }
        let mut placed_any = false;
        let has_rim = random.next_double() < 0.9;
        let rim_x = if has_rim {
            mth_next_int(random, 0, 2)?
        } else {
            0
        };
        let rim_z = if has_rim {
            mth_next_int(random, 0, 2)?
        } else {
            0
        };
        let will_place_rim = has_rim && rim_x != 0 && rim_z != 0;
        let x_size = mth_next_int(random, 3, 7)?;
        let z_size = mth_next_int(random, 3, 7)?;
        let max_length = x_size.max(z_size);
        let seed = world.world_seed().map_err(FeatureExecutionError::Query)?;
        let Some(terrain) = super::tfc_terrain::active_for(seed) else {
            return Ok(false);
        };
        let mut cache = super::tfc_terrain::TfcTerrain::rock_cache(origin[0] >> 4, origin[2] >> 4);
        let surface = terrain.surface_height_at(origin[0], origin[2]);
        let rock = terrain.rock(origin[0], origin[1], origin[2], surface, &mut cache);
        let Some(&(raw, cobble)) = self.rocks.get(rock) else {
            return Ok(false);
        };
        for pos in within_manhattan(origin, x_size, 0, z_size) {
            if dist_manhattan(pos, origin) > max_length {
                break;
            }
            if !self.is_clear(world, pos, cobble)? {
                continue;
            }
            if will_place_rim {
                placed_any = true;
                world.set_block(pos, raw, 2).map_err(we)?;
                let above = [pos[0], pos[1] + 1, pos[2]];
                if random.next_float() < 0.01
                    && let Some(guano) = self.guano
                    && is_air(self.name(world, above)?)
                {
                    world.set_block(above, guano, 2).map_err(we)?;
                }
            }
            let offset = [pos[0] + rim_x, pos[1], pos[2] + rim_z];
            if self.is_clear(world, offset, cobble)? {
                placed_any = true;
                let mut to_place = cobble;
                if random.next_boolean() {
                    to_place = self.water;
                    if random.next_float() < 0.15 {
                        let pick = random.next_int_bound(TIDE_POOL_BLOCKS.len() as i32)? as usize;
                        if let Some(filled) = self.shellfish[pick] {
                            to_place = filled;
                        }
                    }
                }
                world.set_block(offset, to_place, 2).map_err(we)?;
            }
        }
        Ok(placed_any)
    }

    fn name<W: FeatureWorld>(&self, world: &mut W, p: [i32; 3]) -> R<&str, W> {
        let s = world.block(p).map_err(we)?;
        Ok(self.names[s.0 as usize].as_str())
    }

    /// `isClear`: not already cobble or fluid, air on every side except below-solid / above-air.
    fn is_clear<W: FeatureWorld>(
        &self,
        world: &mut W,
        pos: [i32; 3],
        cobble: BlockStateId,
    ) -> R<bool, W> {
        let here = world.block(pos).map_err(we)?;
        let name = self.names[here.0 as usize].as_str();
        if here == cobble || crate::fluid::is_water_block_name(name) || name == "minecraft:lava" {
            return Ok(false);
        }
        for (dx, dy, dz) in [
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
            (-1, 0, 0),
            (1, 0, 0),
        ] {
            let air = is_air(self.name(world, [pos[0] + dx, pos[1] + dy, pos[2] + dz])?);
            if (air && dy != 1) || (!air && dy == 1) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// `Mth.nextInt(random, min, max)`.
fn mth_next_int<E>(
    random: &mut impl RandomSource,
    min: i32,
    max: i32,
) -> Result<i32, FeatureExecutionError<E>> {
    if min >= max {
        Ok(min)
    } else {
        Ok(random.next_int_bound(max - min + 1)? + min)
    }
}

fn dist_manhattan(a: [i32; 3], b: [i32; 3]) -> i32 {
    (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()
}

/// `BlockPos.withinManhattan(origin, reachX, reachY, reachZ)`: nearest first, then x, y, and +z
/// before -z.
fn within_manhattan(o: [i32; 3], reach_x: i32, reach_y: i32, reach_z: i32) -> Vec<[i32; 3]> {
    let mut out = Vec::new();
    for depth in 0..=reach_x + reach_y + reach_z {
        let max_x = reach_x.min(depth);
        for x in -max_x..=max_x {
            let max_y = reach_y.min(depth - x.abs());
            for y in -max_y..=max_y {
                let z = depth - x.abs() - y.abs();
                if z <= reach_z {
                    out.push([o[0] + x, o[1] + y, o[2] + z]);
                    if z != 0 {
                        out.push([o[0] + x, o[1] + y, o[2] - z]);
                    }
                }
            }
        }
    }
    out
}

fn we<E>(e: E) -> FeatureExecutionError<E> {
    FeatureExecutionError::Basic(FeatureError::World(e))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::{
        heightmap::HeightmapType,
        registry::BlockRegistry,
        worldgen::{anchor::WorldGenerationContext, placement::PlacementQueryError},
    };
    use horizon_core::random::XoroshiroRandom;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    pub const ROCKS: [&str; 21] = [
        "andesite",
        "basalt",
        "chalk",
        "chert",
        "claystone",
        "conglomerate",
        "dacite",
        "diorite",
        "dolomite",
        "gabbro",
        "gneiss",
        "granite",
        "limestone",
        "marble",
        "phyllite",
        "quartzite",
        "rhyolite",
        "schist",
        "shale",
        "slate",
        "tuff",
    ];

    /// Flat world: solid `raw` rock below `surface`, air above.
    pub struct Flat {
        pub blocks: HashMap<[i32; 3], BlockStateId>,
        pub surface: i32,
        pub rock: BlockStateId,
        pub air: BlockStateId,
    }
    impl FeatureWorld for Flat {
        type Error = ();
        fn block(&mut self, p: [i32; 3]) -> Result<BlockStateId, ()> {
            Ok(*self.blocks.get(&p).unwrap_or(if p[1] < self.surface {
                &self.rock
            } else {
                &self.air
            }))
        }
        fn set_block(&mut self, p: [i32; 3], s: BlockStateId, _: i32) -> Result<bool, ()> {
            self.blocks.insert(p, s);
            Ok(true)
        }
    }
    impl PlacementContext for Flat {
        fn generation(&self) -> WorldGenerationContext {
            WorldGenerationContext::new(-64, 384, 63)
        }
        fn min_y(&mut self) -> Result<i32, PlacementQueryError> {
            Ok(-64)
        }
        fn max_y(&mut self) -> Result<i32, PlacementQueryError> {
            Ok(319)
        }
        fn height(&mut self, _: HeightmapType, _: i32, _: i32) -> Result<i32, PlacementQueryError> {
            Ok(self.surface)
        }
        fn world_seed(&mut self) -> Result<i64, PlacementQueryError> {
            Ok(99)
        }
    }

    pub fn registry() -> &'static BlockRegistry {
        let mut names: Vec<String> = [
            "minecraft:air",
            "minecraft:cave_air",
            "minecraft:void_air",
            "minecraft:stone",
            "minecraft:water",
            "minecraft:lava",
            "tfc:groundcover/guano",
            "tfc:groundcover/clam",
        ]
        .map(String::from)
        .into();
        for r in ROCKS {
            for kind in ["raw", "gravel", "cobble"] {
                names.push(format!("tfc:{kind}_{r}"));
            }
        }
        let mut rows = Vec::new();
        let mut defaults = BTreeMap::new();
        for n in &names {
            defaults.insert(n.clone(), rows.len());
            if n.ends_with("lava") || n.ends_with("water") {
                for level in 0..16 {
                    rows.push(json!([n, false, true, "", {"id": n, "properties": {"level": level.to_string()}}]));
                }
            } else if n == "tfc:groundcover/clam" {
                for w in ["false", "true"] {
                    rows.push(
                        json!([n, false, false, "", {"id": n, "properties": {"waterlogged": w}}]),
                    );
                }
            } else {
                rows.push(json!([n, n.ends_with("air"), false, "", {"id": n, "properties": {}}]));
            }
        }
        let facts = json!({"states": rows, "defaults": defaults});
        Box::leak(Box::new(
            BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
        ))
    }

    pub fn activate() {
        crate::worldgen::tfc_terrain::set_active(
            99,
            Arc::new(crate::worldgen::tfc_terrain::TfcTerrain::new(99)),
        );
    }

    #[test]
    fn manhattan_order_is_nearest_first_with_plus_z_before_minus_z() {
        let cells = within_manhattan([0, 0, 0], 2, 0, 2);
        assert_eq!(
            &cells[..5],
            &[[0, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1], [1, 0, 0]]
        );
        assert!(
            cells
                .windows(2)
                .all(|w| dist_manhattan(w[0], [0; 3]) <= dist_manhattan(w[1], [0; 3]))
        );
        assert_eq!(cells.len(), 25); // the whole 5 x 5 square
    }

    #[test]
    fn a_tide_pool_digs_water_and_cobble_into_a_flat_shore() {
        activate();
        let r = registry();
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        let feature = TfcTidePoolFeature::decode(&bindings).unwrap();
        let mut hits = 0;
        for seed in 0..30 {
            let mut world = Flat {
                blocks: HashMap::new(),
                surface: 64,
                rock: r.default_state("tfc:raw_granite").unwrap(),
                air: r.default_state("minecraft:air").unwrap(),
            };
            if feature
                .place(&mut world, &mut XoroshiroRandom::new(seed), [0, 63, 0])
                .unwrap()
            {
                hits += 1;
                let water = r.default_state("minecraft:water").unwrap();
                assert!(world.blocks.iter().all(|(p, _)| p[1] >= 63 && p[1] <= 64));
                assert!(world.blocks.values().any(|s| *s == water) || world.blocks.len() > 1);
            }
        }
        assert!(hits > 10, "pools placed in {hits}/30 tries");
        // Out of the shoreline band nothing is placed.
        let mut high = Flat {
            blocks: HashMap::new(),
            surface: 80,
            rock: r.default_state("tfc:raw_granite").unwrap(),
            air: r.default_state("minecraft:air").unwrap(),
        };
        assert!(
            !feature
                .place(&mut high, &mut XoroshiroRandom::new(1), [0, 79, 0])
                .unwrap()
        );
    }
}
