//! TerraFirmaCraft's `tfc:branching_cactus` (`BranchingCactusFeature`, EUPL 1.2): a saguaro of
//! up to eight blocks with side arms, grown by `GrowingBranchingCactusBlock.growRecursively`.
//!
//! The geometry and the world-random draws are TFC's. The mod's saguaro has none of TFC's
//! connection properties (up, down, the four sides, `grows_branches`), so every cell is the one
//! block, and the flower is `tfc:plant/saguaro_flower` where the mod has it.

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    optional,
    placement::{PlacementContext, PlacementQueryError},
    required,
    tfc_tree::{is_air, single_block_replaceable},
    tree_blocks as tb,
};
use crate::registry::BlockStateId;
use horizon_core::{geometry::Direction, random::RandomSource};
use std::sync::Arc;

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

#[derive(Clone, Debug)]
pub struct TfcCactusFeature {
    cactus: BlockStateId,
    flower: Option<BlockStateId>,
    names: Arc<Vec<String>>,
    cactus_name: String,
}

fn query<E>(e: PlacementQueryError) -> FeatureExecutionError<E> {
    FeatureExecutionError::Query(e)
}

impl TfcCactusFeature {
    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(json)?;
        let config: Object = match optional(&object, "config") {
            Some(config) => serde_json::from_str(config.get())?,
            None => object.clone(),
        };
        let block: String = serde_json::from_str(required(&config, "block")?.get())?;
        let registry = bindings.registry();
        let cactus = registry
            .default_state(&block)
            .ok_or_else(|| DecodeError::Registry(format!("unknown block {block}")))?;
        Ok(Self {
            cactus,
            flower: registry.default_state(&format!("{block}_flower")),
            cactus_name: block,
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
        self.grow(world, origin, true, 8)
    }

    fn name_at<W: FeatureWorld>(&self, world: &mut W, p: [i32; 3]) -> R<&str, W> {
        let s = world
            .block(p)
            .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))?;
        Ok(self.names[s.0 as usize].as_str())
    }

    /// `canGrowInto`: air, with no cactus beside it except behind the way it grows.
    fn can_grow_into<W: FeatureWorld>(
        &self,
        world: &mut W,
        pos: [i32; 3],
        direction: Direction,
    ) -> R<bool, W> {
        if !is_air(self.name_at(world, pos)?) {
            return Ok(false);
        }
        for dir in [
            Direction::Down,
            Direction::Up,
            Direction::North,
            Direction::South,
            Direction::West,
            Direction::East,
        ] {
            if dir == direction.opposite() {
                continue;
            }
            let neighbour = tb::relative(pos, dir, 1);
            if self.name_at(world, neighbour)? == self.cactus_name {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn try_fruit<W: FeatureWorld>(&self, world: &mut W, pos: [i32; 3]) -> R<(), W> {
        let n = self.name_at(world, pos)?;
        if (is_air(n) || single_block_replaceable(n))
            && let Some(flower) = self.flower
        {
            world
                .set_block(pos, flower, 3)
                .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))?;
        }
        Ok(())
    }

    /// `growRecursively`: `grows_branches` is the state's property, `height` the stem.
    fn grow<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        pos: [i32; 3],
        grows_branches: bool,
        height: i32,
    ) -> R<bool, W> {
        let we = |e| FeatureExecutionError::Basic(FeatureError::World(e));
        if !self.can_grow_into(world, pos, Direction::Up)?
            || !self.can_grow_into(world, tb::offset(pos, 0, 1, 0), Direction::Up)?
        {
            return Ok(false);
        }
        let mut used: Vec<Direction> = Vec::new();
        for i in 0..height {
            let at = tb::offset(pos, 0, i, 0);
            if i == height - 1 {
                self.try_fruit(world, tb::offset(at, 0, 1, 0))?;
            }
            if !self.can_grow_into(world, at, Direction::Up)? {
                return Ok(true);
            }
            world.set_block(at, self.cactus, 3).map_err(we)?;
            // `state.getValue(GROWS_BRANCHES) && i > 2 && nextFloat() < 0.6f && i < height - 1`
            if grows_branches
                && i > 2
                && world.next_world_float().map_err(query)? < 0.6
                && i < height - 1
            {
                let dir = tb::HORIZONTAL[world.next_world_int(4).map_err(query)? as usize];
                if used.contains(&dir) {
                    continue;
                }
                let one = tb::relative(at, dir, 1);
                let two = tb::relative(one, dir, 1);
                if self.can_grow_into(world, one, dir)?
                    && self.can_grow_into(world, two, dir)?
                    && self.can_grow_into(world, tb::offset(two, 0, 1, 0), Direction::Up)?
                {
                    used.push(dir);
                    world.set_block(one, self.cactus, 3).map_err(we)?;
                    world.set_block(two, self.cactus, 3).map_err(we)?;
                    self.grow(world, tb::offset(two, 0, 1, 0), false, 3)?;
                    // Go back and fix the branch connection.
                    world.set_block(at, self.cactus, 3).map_err(we)?;
                }
            }
        }
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

    /// A `Flat` world with a seeded world random.
    struct Ctx {
        flat: Flat,
        random: XoroshiroRandom,
    }
    impl FeatureWorld for Ctx {
        type Error = ();
        fn block(&mut self, p: [i32; 3]) -> Result<BlockStateId, ()> {
            self.flat.block(p)
        }
        fn set_block(&mut self, p: [i32; 3], s: BlockStateId, f: i32) -> Result<bool, ()> {
            self.flat.set_block(p, s, f)
        }
    }
    impl PlacementContext for Ctx {
        fn generation(&self) -> crate::worldgen::anchor::WorldGenerationContext {
            self.flat.generation()
        }
        fn min_y(&mut self) -> Result<i32, PlacementQueryError> {
            self.flat.min_y()
        }
        fn max_y(&mut self) -> Result<i32, PlacementQueryError> {
            self.flat.max_y()
        }
        fn height(
            &mut self,
            kind: crate::heightmap::HeightmapType,
            x: i32,
            z: i32,
        ) -> Result<i32, PlacementQueryError> {
            self.flat.height(kind, x, z)
        }
        fn next_world_float(&mut self) -> Result<f32, PlacementQueryError> {
            Ok(self.random.next_float())
        }
        fn next_world_int(&mut self, bound: i32) -> Result<i32, PlacementQueryError> {
            Ok(self.random.next_int_bound(bound).unwrap())
        }
    }

    fn setup() -> (TfcCactusFeature, Ctx, BlockStateId) {
        let names = [
            "minecraft:air",
            "minecraft:cave_air",
            "minecraft:void_air",
            "minecraft:stone",
            "tfc:plant/saguaro",
            "tfc:plant/saguaro_flower",
        ];
        let rows: Vec<_> = names
            .iter()
            .map(|n| json!([n, n.ends_with("air"), false, "", {"id": n, "properties": {}}]))
            .collect();
        let defaults: BTreeMap<_, _> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let facts = json!({"states": rows, "defaults": defaults});
        let registry: &'static BlockRegistry = Box::leak(Box::new(
            BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
        ));
        let bindings = FeatureBlockBindings::new(registry, BTreeSet::new()).unwrap();
        let feature = TfcCactusFeature::decode(
            r#"{"type":"tfc:branching_cactus","block":"tfc:plant/saguaro"}"#,
            &bindings,
        )
        .unwrap();
        let cactus = registry.default_state("tfc:plant/saguaro").unwrap();
        let ctx = Ctx {
            flat: Flat {
                blocks: HashMap::new(),
                surface: 0,
                rock: registry.default_state("minecraft:stone").unwrap(),
                air: registry.default_state("minecraft:air").unwrap(),
            },
            random: XoroshiroRandom::new(1),
        };
        (feature, ctx, cactus)
    }

    #[test]
    fn a_clear_site_grows_a_stem_and_flower_and_an_occupied_one_does_not() {
        let (f, mut w, cactus) = setup();
        let flower = f.flower.unwrap();
        assert!(f.grow(&mut w, [0, 0, 0], false, 8).unwrap());
        for y in 0..8 {
            assert_eq!(w.flat.blocks[&[0, y, 0]], cactus, "stem at {y}");
        }
        assert_eq!(w.flat.blocks[&[0, 8, 0]], flower);
        // A cactus already beside the site blocks it.
        let (f, mut w, cactus) = setup();
        w.flat.blocks.insert([1, 0, 0], cactus);
        let before = w.flat.blocks.len();
        assert!(!f.grow(&mut w, [0, 0, 0], true, 8).unwrap());
        assert_eq!(w.flat.blocks.len(), before);
    }

    #[test]
    fn growing_stems_sometimes_sprout_a_three_high_arm_two_blocks_out() {
        let mut arms = 0;
        for seed in 0..60 {
            let (f, mut w, cactus) = setup();
            w.random = XoroshiroRandom::new(seed);
            f.grow(&mut w, [0, 0, 0], true, 8).unwrap();
            for (dx, dz) in [(2, 0), (-2, 0), (0, 2), (0, -2)] {
                for i in 3..7 {
                    if w.flat.blocks.get(&[dx, i, dz]) == Some(&cactus)
                        && w.flat.blocks.get(&[dx, i - 1, dz]) != Some(&cactus)
                    {
                        arms += 1;
                        // The arm stands on a connector and rises three blocks.
                        assert_eq!(w.flat.blocks.get(&[dx / 2, i, dz / 2]), Some(&cactus));
                        for up in 1..=3 {
                            assert_eq!(w.flat.blocks.get(&[dx, i + up, dz]), Some(&cactus));
                        }
                    }
                }
            }
        }
        assert!(arms > 10, "{arms} arms in 60 saguaros");
    }
}
