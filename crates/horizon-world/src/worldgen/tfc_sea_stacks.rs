//! TerraFirmaCraft's `tfc:sea_stacks` (`SeaStacksFeature`, EUPL 1.2): a column of metaball-shaped
//! rock rising out of the sea from a hanging cone beneath it, only on basalt, limestone, marble
//! and rhyolite shores.

use super::{
    DecodeError,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    placement::PlacementContext,
    tfc_hot_spring::Metaballs,
    tfc_tree::replaceable,
};
use crate::registry::BlockStateId;
use horizon_core::random::{RandomSource, XoroshiroRandom};
use std::{collections::HashMap, sync::Arc};

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

const SEA_LEVEL: i32 = 63;
/// `#tfc:sea_stack_rocks`.
const SEA_STACK_ROCKS: [&str; 4] = ["basalt", "limestone", "marble", "rhyolite"];

#[derive(Clone, Debug)]
pub struct TfcSeaStacksFeature {
    rocks: HashMap<&'static str, BlockStateId>,
    names: Arc<Vec<String>>,
}

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

impl TfcSeaStacksFeature {
    pub(crate) fn decode(bindings: &FeatureBlockBindings<'_>) -> Result<Self, DecodeError> {
        let registry = bindings.registry();
        let names: Vec<String> = (0..registry.len())
            .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
            .collect();
        let mut rocks = HashMap::new();
        for rock in SEA_STACK_ROCKS {
            if let Some(state) = registry.default_state(&format!("tfc:raw_{rock}")) {
                rocks.insert(rock, state);
            }
        }
        Ok(Self {
            rocks,
            names: Arc::new(names),
        })
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let here = world.block(origin).map_err(we)?;
        let name = self.names[here.0 as usize].as_str();
        // Only in water.
        if !(name == "minecraft:water" || name == "minecraft:lava") || origin[1] > SEA_LEVEL {
            return Ok(false);
        }
        let seed = world.world_seed().map_err(FeatureExecutionError::Query)?;
        let Some(terrain) = super::tfc_terrain::active_for(seed) else {
            return Ok(false);
        };
        let mut cache = super::tfc_terrain::TfcTerrain::rock_cache(origin[0] >> 4, origin[2] >> 4);
        let surface = terrain.surface_height_at(origin[0], origin[2]);
        let rock = terrain.rock(origin[0], 0, origin[2], surface, &mut cache);
        let Some(&state) = self.rocks.get(rock) else {
            return Ok(false);
        };
        let up = mth_next_int(random, 2, 6)?;
        let radius = mth_next_int(random, 3, 6)?;
        let top = [origin[0], origin[1] + up, origin[2]];
        let height = mth_next_int(random, 5, 15)?;
        self.stack(world, top, state, radius, height, random, false)?;
        let base = [origin[0], origin[1] + up - 1, origin[2]];
        self.stack(world, base, state, radius / 2, up, random, true)?;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    fn stack<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        origin: [i32; 3],
        state: BlockStateId,
        radius: i32,
        height: i32,
        random: &mut impl RandomSource,
        inverted: bool,
    ) -> R<(), W> {
        let mut accepted: Vec<(i32, i32)> = Vec::new();
        if inverted {
            for y in -height..=0 {
                self.layer(
                    world,
                    origin,
                    state,
                    radius,
                    height,
                    random,
                    y,
                    &mut accepted,
                )?;
            }
        } else {
            for y in (0..height).rev() {
                self.layer(
                    world,
                    origin,
                    state,
                    radius,
                    height,
                    random,
                    y,
                    &mut accepted,
                )?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn layer<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        origin: [i32; 3],
        state: BlockStateId,
        radius: i32,
        height: i32,
        random: &mut impl RandomSource,
        y: i32,
        accepted: &mut Vec<(i32, i32)>,
    ) -> R<(), W> {
        // `(int) (radius * Mth.abs(height - y) / (float) height)`
        let actual = ((radius * (height - y).abs()) as f32 / height as f32) as i32;
        let mut fork =
            XoroshiroRandom::from_state(random.next_long() as u64, random.next_long() as u64);
        let noise = Metaballs::simple(&mut fork, actual)?;
        for x in origin[0] - radius..=origin[0] + radius {
            for z in origin[2] - radius..=origin[2] + radius {
                let cursor = [x, origin[1] + y, z];
                let listed = accepted.contains(&(x, z));
                if noise.inside(x - origin[0], z - origin[2]) || listed {
                    let current = world.block(cursor).map_err(we)?;
                    if replaceable(&self.names[current.0 as usize]) {
                        world.set_block(cursor, state, 3).map_err(we)?;
                        if !listed {
                            accepted.push((x, z));
                        }
                        if actual == 1 && random.next_float() < 0.4 {
                            return Ok(());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn we<E>(e: E) -> FeatureExecutionError<E> {
    FeatureExecutionError::Basic(FeatureError::World(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::tfc_tide_pool::tests::{Flat, activate, registry};
    use std::collections::BTreeSet;

    fn rock_at(x: i32) -> String {
        let seed = 99;
        let terrain = super::super::tfc_terrain::active_for(seed).unwrap();
        let mut cache = super::super::tfc_terrain::TfcTerrain::rock_cache(x >> 4, 0);
        let surface = terrain.surface_height_at(x, 0);
        terrain.rock(x, 0, 0, surface, &mut cache).to_owned()
    }

    fn sea(r: &crate::registry::BlockRegistry, x: i32) -> Flat {
        let water = r.default_state("minecraft:water").unwrap();
        let mut blocks = std::collections::HashMap::new();
        for dx in -8..=8 {
            for dz in -8..=8 {
                for y in 40..=62 {
                    blocks.insert([x + dx, y, dz], water);
                }
            }
        }
        Flat {
            blocks,
            surface: 40,
            rock: r.default_state("tfc:raw_granite").unwrap(),
            air: r.default_state("minecraft:air").unwrap(),
        }
    }

    #[test]
    fn stacks_rise_from_the_sea_on_the_four_rocks_only() {
        activate();
        let r = registry();
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        let feature = TfcSeaStacksFeature::decode(&bindings).unwrap();
        let xs: Vec<i32> = (0..4000).step_by(8).collect();
        let good = xs
            .iter()
            .copied()
            .find(|x| SEA_STACK_ROCKS.contains(&rock_at(*x).as_str()));
        let bad = xs
            .iter()
            .copied()
            .find(|x| !SEA_STACK_ROCKS.contains(&rock_at(*x).as_str()));
        if let Some(x) = good {
            let rock = rock_at(x);
            let raw = r.default_state(&format!("tfc:raw_{rock}")).unwrap();
            let mut placed = false;
            for seed in 0..10 {
                let mut world = sea(r, x);
                let ok = feature
                    .place(&mut world, &mut XoroshiroRandom::new(seed), [x, 45, 0])
                    .unwrap();
                assert!(ok);
                let cells: Vec<_> = world.blocks.iter().filter(|(_, s)| **s == raw).collect();
                // 2..6 up, then at most 15 more: all above the sea floor start, none past 6 + 15.
                assert!(
                    cells
                        .iter()
                        .all(|(p, _)| p[1] >= 45 + 1 - 6 && p[1] <= 45 + 6 + 14)
                );
                placed |= !cells.is_empty();
            }
            assert!(placed, "a stack must leave rock behind");
            // Out of the water, or above sea level: nothing.
            let mut dry = sea(r, x);
            assert!(
                !feature
                    .place(&mut dry, &mut XoroshiroRandom::new(1), [x, 70, 0])
                    .unwrap()
            );
            let mut air = Flat {
                blocks: Default::default(),
                surface: 40,
                rock: raw,
                air: r.default_state("minecraft:air").unwrap(),
            };
            assert!(
                !feature
                    .place(&mut air, &mut XoroshiroRandom::new(1), [x, 50, 0])
                    .unwrap()
            );
        }
        if let Some(x) = bad {
            let mut world = sea(r, x);
            assert!(
                !feature
                    .place(&mut world, &mut XoroshiroRandom::new(1), [x, 45, 0])
                    .unwrap()
            );
        }
        assert!(good.is_some() && bad.is_some(), "{good:?} {bad:?}");
    }
}
