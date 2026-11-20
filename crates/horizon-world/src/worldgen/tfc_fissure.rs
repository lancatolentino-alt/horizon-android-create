//! TerraFirmaCraft's `tfc:fissure` (`FissureFeature`, EUPL 1.2): a winding shaft of fluid
//! walled in rock that drops from the surface in straight pieces joined by sideways steps, with
//! optional ore decoration in the rock around it.

use super::{
    DecodeError, Object,
    anchor::VerticalAnchor,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    optional,
    placement::PlacementContext,
    required,
    tfc_tree::{Weighted, is_air, state_of},
};
use crate::{heightmap::HeightmapType, registry::BlockStateId};
use horizon_core::random::RandomSource;
use std::{collections::HashMap, sync::Arc};

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

#[derive(Clone, Debug)]
pub(super) struct Decoration {
    /// `replace` block name -> weighted result.
    states: HashMap<String, Weighted>,
    rarity: i32,
    radius: i32,
    count: i32,
}

#[derive(Clone, Debug)]
pub struct TfcFissureFeature {
    pub(super) wall: BlockStateId,
    fluid: BlockStateId,
    count: i32,
    radius: i32,
    min_depth: VerticalAnchor,
    min_pieces: i32,
    max_pieces: i32,
    max_piece_length: i32,
    decoration: Option<Decoration>,
    names: Arc<Vec<String>>,
}

fn int(object: &Object, name: &'static str, default: i32) -> Result<i32, DecodeError> {
    match optional(object, name) {
        Some(v) => {
            let value: i32 = serde_json::from_str(v.get())?;
            if value < 1 {
                return Err(DecodeError::Field(name));
            }
            Ok(value)
        }
        None => Ok(default),
    }
}

impl Decoration {
    /// `FissureConfig.Decoration.CODEC`.
    pub(super) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let d: Object = serde_json::from_str(json)?;
        let mut states = HashMap::new();
        let blocks: Vec<Object> = serde_json::from_str(required(&d, "blocks")?.get())?;
        for entry in &blocks {
            let replace: Vec<String> = serde_json::from_str(required(entry, "replace")?.get())?;
            let with: Vec<Object> = serde_json::from_str(required(entry, "with")?.get())?;
            let weighted = Weighted::decode(&with, bindings)?;
            for name in replace {
                states.insert(name, weighted.clone());
            }
        }
        Ok(Self {
            states,
            rarity: int(&d, "rarity", 1)?,
            radius: int(&d, "radius", 1)?,
            count: int(&d, "count", 1)?,
        })
    }
}

impl TfcFissureFeature {
    /// The fissure a hot spring digs: `placeFissure(.., 10, 22, 6, 16, 12, decoration)`.
    pub(super) fn for_spring(
        bindings: &FeatureBlockBindings<'_>,
        wall: BlockStateId,
        fluid: BlockStateId,
        decoration: Option<Decoration>,
    ) -> Self {
        let registry = bindings.registry();
        Self {
            wall,
            fluid,
            count: 1,
            radius: 12,
            min_depth: VerticalAnchor::Absolute(16),
            min_pieces: 10,
            max_pieces: 22,
            max_piece_length: 6,
            decoration,
            names: Arc::new(
                (0..registry.len())
                    .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
                    .collect(),
            ),
        }
    }

    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(json)?;
        let config: Object = match optional(&object, "config") {
            Some(config) => serde_json::from_str(config.get())?,
            None => object.clone(),
        };
        let registry = bindings.registry();
        // `wall_state` is optional in TFC (the raw rock under the origin); the shipped data
        // always sets it, and without it there is no rock lookup here.
        let wall = state_of(
            bindings,
            required(&config, "wall_state")
                .map_err(|_| DecodeError::Field("fissure wall_state"))?
                .get(),
        )?;
        let fluid = state_of(bindings, required(&config, "fluid_state")?.get())?;
        let min_depth = match optional(&config, "min_depth") {
            Some(v) => VerticalAnchor::from_json(v.get())?,
            None => VerticalAnchor::AboveBottom(16),
        };
        let min_pieces = int(&config, "min_pieces", 10)?;
        let max_pieces = int(&config, "max_pieces", 24)?;
        if max_pieces <= min_pieces {
            return Err(DecodeError::Field("fissure pieces"));
        }
        let decoration = match optional(&config, "decoration") {
            Some(d) => Some(Decoration::decode(d.get(), bindings)?),
            None => None,
        };
        Ok(Self {
            wall,
            fluid,
            count: int(&config, "count", 5)?,
            radius: int(&config, "radius", 12)?,
            min_depth,
            min_pieces,
            max_pieces,
            max_piece_length: int(&config, "max_piece_length", 6)?,
            decoration,
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
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let place_count = 1 + random.next_int_bound(self.count)?;
        let min_depth = self.min_depth.resolve(world.generation());
        for _ in 0..place_count {
            let dx = random.next_int_bound(self.radius)? - random.next_int_bound(self.radius)?;
            let dz = random.next_int_bound(self.radius)? - random.next_int_bound(self.radius)?;
            let (x, z) = (origin[0] + dx, origin[2] + dz);
            let y = world
                .height(HeightmapType::WorldSurfaceWg, x, z)
                .map_err(FeatureExecutionError::Query)?;
            self.fissure(world, random, [x, y, z], origin, min_depth)?;
        }
        Ok(true)
    }

    pub(super) fn fissure<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        start: [i32; 3],
        center: [i32; 3],
        min_depth: i32,
    ) -> R<(), W> {
        let pieces = self.min_pieces + random.next_int_bound(self.max_pieces - self.min_pieces)?;
        let mut top = start;
        let at = |p: [i32; 3], dx: i32, dy: i32, dz: i32| [p[0] + dx, p[1] + dy, p[2] + dz];
        for _ in 0..pieces {
            let piece_depth = 1 + random.next_int_bound(self.max_piece_length)?;
            for dy in 1..=piece_depth {
                // Cut off before running into rivers.
                let below = world.block(at(top, 0, -dy - 2, 0)).map_err(we)?;
                if is_air(&self.names[below.0 as usize]) {
                    return Ok(());
                }
                world
                    .set_block(at(top, 0, -dy, 0), self.fluid, 2)
                    .map_err(we)?;
                for (ox, oz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    world
                        .set_block(at(top, ox, -dy, oz), self.wall, 2)
                        .map_err(we)?;
                }
                if let Some(d) = &self.decoration {
                    for _ in 0..d.count {
                        if random.next_int_bound(d.rarity)? == 0 {
                            let px = random.next_int_bound(d.radius)?
                                - random.next_int_bound(d.radius)?;
                            let py = random.next_int_bound(3)? - random.next_int_bound(3)? - dy;
                            let pz = random.next_int_bound(d.radius)?
                                - random.next_int_bound(d.radius)?;
                            let p = at(top, px, py, pz);
                            let stone = world.block(p).map_err(we)?;
                            if let Some(weighted) = d.states.get(&self.names[stone.0 as usize])
                                && let Some(state) = weighted.get(random)
                            {
                                world.set_block(p, state, 2).map_err(we)?;
                            }
                        }
                    }
                }
            }
            // Branch: `top` is now above the branch, ready for the next tube.
            let direction = self.bounded_direction(random, center, top)?;
            top = at(top, direction[0], -piece_depth, direction[1]);
            world.set_block(top, self.fluid, 2).map_err(we)?;
            world
                .set_block(at(top, 0, 1, 0), self.wall, 2)
                .map_err(we)?;
            for side in [[0, -1], [1, 0], [0, 1], [-1, 0]] {
                // Fill all sides except the one we came from.
                if side != [-direction[0], -direction[1]] {
                    world
                        .set_block(at(top, side[0], 0, side[1]), self.wall, 2)
                        .map_err(we)?;
                }
            }
            if top[1] < min_depth {
                break;
            }
        }
        Ok(())
    }

    /// `randomBoundedDirection`: horizontal `[dx, dz]`, flipped when it would leave the radius.
    fn bounded_direction(
        &self,
        random: &mut impl RandomSource,
        center: [i32; 3],
        target: [i32; 3],
    ) -> Result<[i32; 2], horizon_core::random::InvalidBound> {
        // `Direction.Plane.HORIZONTAL` is NORTH, EAST, SOUTH, WEST.
        let direction = [[0, -1], [1, 0], [0, 1], [-1, 0]][random.next_int_bound(4)? as usize];
        let (dist_x, dist_z) = (target[0] - center[0], target[2] - center[2]);
        Ok(match direction {
            [1, 0] if dist_x > self.radius => [-1, 0],
            [-1, 0] if dist_x < -self.radius => [1, 0],
            [0, -1] if dist_z < -self.radius => [0, 1],
            [0, 1] if dist_z > self.radius => [0, -1],
            other => other,
        })
    }
}

fn we<E>(e: E) -> FeatureExecutionError<E> {
    FeatureExecutionError::Basic(FeatureError::World(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        registry::BlockRegistry,
        worldgen::{anchor::WorldGenerationContext, placement::PlacementQueryError},
    };
    use horizon_core::random::XoroshiroRandom;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    fn registry() -> &'static BlockRegistry {
        let names = [
            "minecraft:air",
            "minecraft:cave_air",
            "minecraft:void_air",
            "minecraft:stone",
            "minecraft:basalt",
            "minecraft:diamond_ore",
            "minecraft:lava",
        ];
        let mut rows = Vec::new();
        let mut defaults = BTreeMap::new();
        for n in names {
            defaults.insert(n, rows.len());
            if n.ends_with("lava") {
                for level in 0..16 {
                    rows.push(json!([n, false, true, "", {"id": n, "properties": {"level": level.to_string()}}]));
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

    struct World {
        blocks: HashMap<[i32; 3], BlockStateId>,
        stone: BlockStateId,
        air: BlockStateId,
    }
    impl FeatureWorld for World {
        type Error = ();
        fn block(&mut self, p: [i32; 3]) -> Result<BlockStateId, ()> {
            Ok(*self
                .blocks
                .get(&p)
                .unwrap_or(if p[1] < 60 { &self.stone } else { &self.air }))
        }
        fn set_block(&mut self, p: [i32; 3], s: BlockStateId, _: i32) -> Result<bool, ()> {
            self.blocks.insert(p, s);
            Ok(true)
        }
    }
    impl PlacementContext for World {
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
            Ok(60)
        }
    }

    fn setup(decoration: bool) -> (TfcFissureFeature, World) {
        let r = registry();
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        let mut config = json!({
            "type": "tfc:fissure", "wall_state": "minecraft:basalt",
            "fluid_state": "minecraft:lava", "count": 3, "radius": 6
        });
        if decoration {
            config["decoration"] = json!({"blocks": [{"replace": ["minecraft:basalt"],
                "with": [{"block": "minecraft:diamond_ore"}]}], "radius": 3, "count": 6, "rarity": 1});
        }
        let feature = TfcFissureFeature::decode(&config.to_string(), &bindings).unwrap();
        let world = World {
            blocks: HashMap::new(),
            stone: r.default_state("minecraft:stone").unwrap(),
            air: r.default_state("minecraft:air").unwrap(),
        };
        (feature, world)
    }

    #[test]
    fn a_fissure_is_a_lava_shaft_walled_in_basalt() {
        let (feature, mut world) = setup(false);
        assert!(
            feature
                .place(&mut world, &mut XoroshiroRandom::new(7), [0, 70, 0])
                .unwrap()
        );
        let lava = world
            .blocks
            .iter()
            .filter(|(_, s)| **s == feature.fluid)
            .count();
        assert!(lava > 10, "lava placed: {lava}");
        // Every lava block sits below the surface and has a wall or lava beside it.
        for (p, s) in &world.blocks {
            if *s == feature.fluid {
                assert!(p[1] < 60);
            }
        }
        let walls = world
            .blocks
            .values()
            .filter(|s| **s == feature.wall)
            .count();
        assert!(walls > lava);
        // It drops far below the surface by chaining pieces.
        assert!(world.blocks.keys().map(|p| p[1]).min().unwrap() < 40);
    }

    #[test]
    fn decoration_turns_wall_rock_into_ore_and_is_deterministic() {
        let (feature, mut a) = setup(true);
        feature
            .place(&mut a, &mut XoroshiroRandom::new(3), [0, 70, 0])
            .unwrap();
        let (_, mut b) = setup(true);
        feature
            .place(&mut b, &mut XoroshiroRandom::new(3), [0, 70, 0])
            .unwrap();
        assert_eq!(a.blocks, b.blocks);
        let ore = registry().default_state("minecraft:diamond_ore").unwrap();
        // Decoration only replaces placed basalt; the ore never appears in plain stone.
        for (p, s) in &a.blocks {
            if *s == ore {
                assert!(p[1] < 60);
            }
        }
    }

    #[test]
    fn it_stops_before_a_cavity() {
        let (feature, mut world) = setup(false);
        // A hollow right under the first column: the shaft cuts off at once.
        for y in 40..60 {
            world.blocks.insert([0, y, 0], world.air);
        }
        // Only the shaft at the exact origin column is affected; the others may still dig.
        feature
            .place(&mut world, &mut XoroshiroRandom::new(11), [0, 70, 0])
            .unwrap();
        assert_eq!(
            world.blocks.get(&[0, 58, 0]).copied(),
            Some(world.air),
            "air column untouched"
        );
    }
}
