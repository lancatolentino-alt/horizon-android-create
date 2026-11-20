//! TerraFirmaCraft's `tfc:erosion` (`ErosionFeature`, EUPL 1.2): walks every column of the chunk
//! top-down and stops collapsible blocks (dirt, sand, gravel, cobble...) from hanging over air:
//! a lone block is removed, a taller overhang gets a rock block under it.
//!
//! Approximations: the landslide recipes and `#tfc:can_landslide` are matched by block name,
//! `isFaceSturdy(UP)` is `tfc_tree::sturdy`, the aquifer's air-or-water answer is "water at or
//! below sea level", and the raw-to-hardened swap is skipped because the mod's hardened rock is
//! the raw block.

use super::{
    DecodeError,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    placement::PlacementContext,
    tfc_tree::{is_air, sturdy},
};
use crate::{heightmap::HeightmapType, registry::BlockStateId};
use horizon_core::random::RandomSource;
use std::{collections::HashMap, sync::Arc};

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

const SEA_LEVEL: i32 = 63;

#[derive(Clone, Debug)]
pub struct TfcErosionFeature {
    rocks: HashMap<String, BlockStateId>,
    names: Arc<Vec<String>>,
    collapsible: Arc<Vec<bool>>,
    air: BlockStateId,
    water: BlockStateId,
}

/// A block with a landslide recipe, or in `#tfc:can_landslide`, by name.
fn can_landslide(n: &str) -> bool {
    matches!(
        n,
        "minecraft:dirt"
            | "minecraft:grass_block"
            | "minecraft:coarse_dirt"
            | "minecraft:podzol"
            | "minecraft:rooted_dirt"
            | "minecraft:dirt_path"
            | "minecraft:farmland"
            | "minecraft:mycelium"
            | "minecraft:clay"
            | "minecraft:sand"
            | "minecraft:red_sand"
            | "minecraft:gravel"
    ) || n.starts_with("tfc:cobble_")
        || n.starts_with("tfc:mossy_cobble_")
        || n.starts_with("tfc:gravel_")
        || n.starts_with("tfc:deposit/")
        || n.starts_with("tfc:dirt/")
        || n.starts_with("tfc:grass/")
        || n.starts_with("tfc:clay/")
        || n.starts_with("tfc:sand/")
}

impl TfcErosionFeature {
    pub(crate) fn decode(bindings: &FeatureBlockBindings<'_>) -> Result<Self, DecodeError> {
        let registry = bindings.registry();
        let names: Vec<String> = (0..registry.len())
            .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
            .collect();
        let mut rocks = HashMap::new();
        for name in &names {
            if let Some(rock) = name.strip_prefix("tfc:raw_")
                && let Some(state) = registry.default_state(name)
            {
                rocks.insert(rock.to_owned(), state);
            }
        }
        let collapsible = names.iter().map(|n| can_landslide(n)).collect();
        Ok(Self {
            rocks,
            collapsible: Arc::new(collapsible),
            air: registry
                .default_state("minecraft:air")
                .ok_or(DecodeError::Field("air"))?,
            water: registry
                .default_state("minecraft:water")
                .ok_or(DecodeError::Field("water"))?,
            names: Arc::new(names),
        })
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        _random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let seed = world.world_seed().map_err(FeatureExecutionError::Query)?;
        let terrain = super::tfc_terrain::active_for(seed);
        let min_y = world.min_y().map_err(FeatureExecutionError::Query)?;
        let (chunk_x, chunk_z) = (origin[0] & !15, origin[2] & !15);
        let mut cache = super::tfc_terrain::TfcTerrain::rock_cache(chunk_x >> 4, chunk_z >> 4);
        for x in 0..16 {
            for z in 0..16 {
                let (wx, wz) = (chunk_x + x, chunk_z + z);
                let base = world
                    .height(HeightmapType::WorldSurfaceWg, wx, wz)
                    .map_err(FeatureExecutionError::Query)?;
                let mut previous_can_landslide = false;
                let mut last_safe = base;
                for y in (min_y..=base).rev() {
                    let at = world.block([wx, y, wz]).map_err(we)?;
                    let name = self.names[at.0 as usize].as_str();
                    let recipe = !is_air(name) && self.collapsible[at.0 as usize];
                    let fragile = is_air(name) || !sturdy(name);
                    if previous_can_landslide {
                        if !recipe {
                            if fragile {
                                if last_safe > y + 2 {
                                    self.support(
                                        world,
                                        terrain.as_deref(),
                                        &mut cache,
                                        [wx, y + 1, wz],
                                        base,
                                    )?;
                                } else {
                                    let above = world.block([wx, y + 2, wz]).map_err(we)?;
                                    let above_air = is_air(&self.names[above.0 as usize]);
                                    if above_air {
                                        let fill =
                                            if y < SEA_LEVEL { self.water } else { self.air };
                                        world.set_block([wx, y + 1, wz], fill, 2).map_err(we)?;
                                    } else {
                                        self.support(
                                            world,
                                            terrain.as_deref(),
                                            &mut cache,
                                            [wx, y + 1, wz],
                                            base,
                                        )?;
                                    }
                                }
                            }
                            previous_can_landslide = false;
                            last_safe = y;
                        }
                    } else if !recipe {
                        last_safe = y;
                    } else {
                        previous_can_landslide = true;
                    }
                }
            }
        }
        Ok(true)
    }

    /// Replaces `pos` with the hardened rock there (the raw block in this mod).
    fn support<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        terrain: Option<&super::tfc_terrain::TfcTerrain>,
        cache: &mut super::tfc_chunk_data::ChunkRockDataCache,
        pos: [i32; 3],
        surface: i32,
    ) -> R<(), W> {
        let Some(terrain) = terrain else {
            return Ok(());
        };
        let rock = terrain.rock(pos[0], pos[1], pos[2], surface, cache);
        if let Some(&state) = self.rocks.get(rock) {
            world.set_block(pos, state, 2).map_err(we)?;
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
    use horizon_core::random::XoroshiroRandom;
    use std::collections::BTreeSet;

    #[test]
    fn overhangs_lose_a_lone_block_or_gain_a_rock_support() {
        activate();
        let r = registry();
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        let feature = TfcErosionFeature::decode(&bindings).unwrap();
        let air = r.default_state("minecraft:air").unwrap();
        let cobble = r.default_state("tfc:cobble_granite").unwrap();
        let mut world = Flat {
            blocks: Default::default(),
            surface: 100,
            rock: r.default_state("tfc:raw_granite").unwrap(),
            air,
        };
        // Column (1, 1): one cobble block hanging over a 3 block gap.
        for y in 94..=99 {
            world.blocks.insert([1, y, 1], air);
        }
        world.blocks.insert([1, 97, 1], cobble);
        // Column (3, 3): two cobble blocks hanging over a 2 block gap.
        for y in 94..=99 {
            world.blocks.insert([3, y, 3], air);
        }
        world.blocks.insert([3, 96, 3], cobble);
        world.blocks.insert([3, 97, 3], cobble);
        let before = world.blocks.len();
        feature
            .place(&mut world, &mut XoroshiroRandom::new(1), [0, 0, 0])
            .unwrap();
        assert_eq!(world.blocks[&[1, 97, 1]], air, "a lone overhang is removed");
        let support = world.blocks[&[3, 96, 3]];
        assert!(
            r.state(support).unwrap().name.starts_with("tfc:raw_"),
            "a taller overhang is propped with rock"
        );
        assert_eq!(world.blocks[&[3, 97, 3]], cobble);
        // Everything else was already solid or air: only those two cells changed.
        assert_eq!(world.blocks.len(), before);
    }
}
