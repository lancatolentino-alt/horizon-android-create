//! TerraFirmaCraft's `tfc:kelp_tree` (EUPL 1.2): `GiantKelpFeature` and the generation half of
//! `KelpTreeFlowerBlock` (`generatePlant`, `growTreeRecursive`).
//!
//! The mod's kelp is a vanilla look-alike: the trunk block is a single full block without the
//! connection properties of `KelpTreeBlock`, the flower has no `age`/`facing`, and the salt water
//! is `minecraft:water`. The shape of the tree, its random draws and its survival rules follow
//! the Java.

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    optional,
    placement::PlacementContext,
    required,
};
use crate::registry::BlockStateId;
use horizon_core::random::RandomSource;
use std::sync::Arc;

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

const SEA_LEVEL: i32 = 63;
const MAX_HORIZONTAL_DISTANCE: i32 = 8;
/// `Direction.Plane.HORIZONTAL`: north, east, south, west.
const HORIZONTAL: [[i32; 2]; 4] = [[0, -1], [1, 0], [0, 1], [-1, 0]];

#[derive(Clone, Debug)]
pub struct TfcKelpFeature {
    body: BlockStateId,
    flower: BlockStateId,
    names: Arc<Vec<String>>,
}

/// `#minecraft:dirt`, `#c:gravels` and `#c:sands` (`SEA_BUSH_PLANTABLE_ON`), by name.
fn plantable_on(name: &str) -> bool {
    let (namespace, path) = name.split_once(':').unwrap_or(("minecraft", name));
    if namespace == "minecraft" {
        return matches!(
            path,
            "dirt"
                | "grass_block"
                | "podzol"
                | "coarse_dirt"
                | "mycelium"
                | "rooted_dirt"
                | "moss_block"
                | "mud"
                | "muddy_mangrove_roots"
                | "sand"
                | "red_sand"
                | "suspicious_sand"
                | "gravel"
                | "suspicious_gravel"
        );
    }
    [
        "dirt/",
        "grass/",
        "sand/",
        "mud/",
        "rooted_dirt/",
        "coarse_dirt/",
        "gravel",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
        || path.ends_with("/gravel")
        || path.ends_with("kaolin_clay_grass")
}

fn offset(pos: [i32; 3], dx: i32, dy: i32, dz: i32) -> [i32; 3] {
    [pos[0] + dx, pos[1] + dy, pos[2] + dz]
}

impl TfcKelpFeature {
    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(json)?;
        let config: Object = match optional(&object, "config") {
            Some(config) => serde_json::from_str(config.get())?,
            None => object.clone(),
        };
        let flower_name: String = serde_json::from_str(required(&config, "block")?.get())?;
        let registry = bindings.registry();
        let flower = registry
            .default_state(&flower_name)
            .ok_or_else(|| DecodeError::Registry(format!("unknown block {flower_name}")))?;
        // `giant_kelp_flower` grows from the `giant_kelp` trunk block.
        let body_name = flower_name
            .strip_suffix("_flower")
            .ok_or(DecodeError::Field("kelp tree flower name"))?;
        let body = registry
            .default_state(body_name)
            .ok_or_else(|| DecodeError::Registry(format!("unknown block {body_name}")))?;
        Ok(Self {
            body,
            flower,
            names: Arc::new(
                (0..registry.len())
                    .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
                    .collect(),
            ),
        })
    }

    fn name<W: FeatureWorld>(&self, world: &mut W, pos: [i32; 3]) -> R<&str, W> {
        let state = world
            .block(pos)
            .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))?;
        Ok(self.names[state.0 as usize].as_str())
    }

    fn is_water<W: FeatureWorld>(&self, world: &mut W, pos: [i32; 3]) -> R<bool, W> {
        Ok(self.name(world, pos)? == "minecraft:water")
    }

    fn is_body<W: FeatureWorld>(&self, world: &mut W, pos: [i32; 3]) -> R<bool, W> {
        let state = world
            .block(pos)
            .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))?;
        Ok(state == self.body)
    }

    fn all_neighbors_empty<W: FeatureWorld>(
        &self,
        world: &mut W,
        pos: [i32; 3],
        excluding: Option<usize>,
    ) -> R<bool, W> {
        for (i, [dx, dz]) in HORIZONTAL.iter().enumerate() {
            if Some(i) != excluding && !self.is_water(world, offset(pos, *dx, 0, *dz))? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `KelpTreeFlowerBlock.canSurvive` for a flower that stands in water.
    fn flower_survives<W: FeatureWorld>(&self, world: &mut W, pos: [i32; 3]) -> R<bool, W> {
        let below = offset(pos, 0, -1, 0);
        if self.is_body(world, below)? || plantable_on(self.name(world, below)?) {
            return Ok(true);
        }
        if !self.is_water(world, below)? {
            return Ok(false);
        }
        let mut valid = false;
        for [dx, dz] in HORIZONTAL {
            let side = offset(pos, dx, 0, dz);
            if self.is_body(world, side)? {
                if valid {
                    return Ok(false);
                }
                valid = true;
            } else if !self.is_water(world, side)? {
                return Ok(false);
            }
        }
        Ok(valid)
    }

    fn set<W: FeatureWorld>(&self, world: &mut W, pos: [i32; 3], state: BlockStateId) -> R<(), W> {
        world
            .set_block(pos, state, 2)
            .map(|_| ())
            .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))
    }

    /// `placeGrownFlower`: only into water, and only where the flower survives.
    fn place_grown_flower<W: FeatureWorld>(&self, world: &mut W, pos: [i32; 3]) -> R<bool, W> {
        if !self.is_water(world, pos)? || !self.flower_survives(world, pos)? {
            return Ok(false);
        }
        self.set(world, pos, self.flower)?;
        Ok(true)
    }

    fn random_direction(random: &mut impl RandomSource) -> usize {
        random.next_int_bound(4).unwrap() as usize
    }

    fn grow_tree_recursive<W: FeatureWorld>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        branch: [i32; 3],
        original: [i32; 3],
        iterations: i32,
        sea_level: i32,
    ) -> R<bool, W> {
        let any = false;
        let mut height = random.next_int_bound(5).unwrap() + 1;
        if iterations == 0 {
            height += 1;
        }
        for j in 0..height {
            let pos = offset(branch, 0, j + 1, 0);
            if !self.is_water(world, pos)?
                || !self.all_neighbors_empty(world, pos, None)?
                || pos[1] >= sea_level - 2
            {
                return Ok(any);
            }
            if pos[1] == sea_level - 3 {
                if !self.place_grown_flower(world, pos)? {
                    return Ok(any);
                }
            } else {
                self.set(world, pos, self.body)?;
            }
            self.set(world, offset(pos, 0, -1, 0), self.body)?;
        }

        let top = offset(branch, 0, height, 0);
        let mut will_continue = false;
        if iterations < 4 {
            let mut attempts = random.next_int_bound(4).unwrap();
            if iterations == 0 {
                attempts += 1;
            }
            for _ in 0..attempts {
                let direction = Self::random_direction(random);
                let [dx, dz] = HORIZONTAL[direction];
                let next = offset(top, dx, 0, dz);
                if (next[0] - original[0]).abs() < MAX_HORIZONTAL_DISTANCE
                    && (next[2] - original[2]).abs() < MAX_HORIZONTAL_DISTANCE
                    && self.is_water(world, next)?
                    && self.is_water(world, offset(next, 0, -1, 0))?
                    && self.all_neighbors_empty(world, next, Some((direction + 2) % 4))?
                {
                    will_continue = true;
                    self.set(world, next, self.body)?;
                    self.set(world, offset(next, -dx, 0, -dz), self.body)?;
                    self.grow_tree_recursive(
                        world,
                        random,
                        next,
                        original,
                        iterations + 1,
                        sea_level,
                    )?;
                }
            }
        }
        if !will_continue {
            // The age draw happens whether or not the flower's age is kept.
            random.next_int_bound(10).unwrap();
            self.set(world, top, self.flower)?;
        }
        Ok(true)
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let at_origin = self.is_water(world, origin)?;
        let sea_level = SEA_LEVEL - random.next_int_bound(6).unwrap();
        if !at_origin {
            return Ok(false);
        }
        let original = world
            .block(origin)
            .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))?;
        self.set(world, origin, self.body)?;
        // The trunk block survives on the seabed.
        let below = offset(origin, 0, -1, 0);
        let rooted = plantable_on(self.name(world, below)?);
        if rooted && self.grow_tree_recursive(world, random, origin, origin, 0, sea_level)? {
            return Ok(true);
        }
        world
            .set_block(origin, original, 3)
            .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))?;
        Ok(false)
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
        let mut add = |name: &str, air: bool, fluid: bool, values: &[&str]| {
            defaults.insert(name.to_owned(), rows.len());
            if values.is_empty() {
                rows.push(json!([name, air, fluid, "", {"id": name, "properties": {}}]));
            }
            for v in values {
                rows.push(json!([name, air, fluid, "", {"id": name, "properties": {"level": v}}]));
            }
        };
        for n in ["air", "cave_air", "void_air"] {
            add(&format!("minecraft:{n}"), true, false, &[]);
        }
        add("minecraft:stone", false, false, &[]);
        add("minecraft:sand", false, false, &[]);
        let levels: Vec<String> = (0..16).map(|l| l.to_string()).collect();
        let levels: Vec<&str> = levels.iter().map(String::as_str).collect();
        add("minecraft:water", false, true, &levels);
        add("tfc:plant/giant_kelp", false, false, &[]);
        add("tfc:plant/giant_kelp_flower", false, false, &[]);
        let facts = json!({"states": rows, "defaults": defaults});
        Box::leak(Box::new(
            BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
        ))
    }

    fn feature() -> TfcKelpFeature {
        let r = registry();
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        TfcKelpFeature::decode(
            &json!({"type": "tfc:kelp_tree", "block": "tfc:plant/giant_kelp_flower"}).to_string(),
            &bindings,
        )
        .unwrap()
    }

    /// Sand at `floor`, water from just above it up to sea level, air above.
    fn ocean(floor: i32) -> Flat {
        let r = registry();
        let water = r.default_state("minecraft:water").unwrap();
        let sand = r.default_state("minecraft:sand").unwrap();
        let mut blocks = HashMap::new();
        for x in -12..=12 {
            for z in -12..=12 {
                blocks.insert([x, floor, z], sand);
                for y in floor + 1..=SEA_LEVEL {
                    blocks.insert([x, y, z], water);
                }
            }
        }
        Flat {
            blocks,
            surface: floor,
            rock: r.default_state("minecraft:stone").unwrap(),
            air: r.default_state("minecraft:air").unwrap(),
        }
    }

    fn count(w: &Flat, state: BlockStateId) -> usize {
        w.blocks.values().filter(|s| **s == state).count()
    }

    #[test]
    fn kelp_grows_from_the_seabed_and_stays_below_the_surface() {
        let r = registry();
        let (body, flower) = (
            r.default_state("tfc:plant/giant_kelp").unwrap(),
            r.default_state("tfc:plant/giant_kelp_flower").unwrap(),
        );
        let f = feature();
        let mut grown = 0;
        for seed in 0..40 {
            let mut w = ocean(30);
            let ok = f
                .place(&mut w, &mut XoroshiroRandom::new(seed), [0, 31, 0])
                .unwrap();
            if !ok {
                continue;
            }
            grown += 1;
            assert_eq!(
                w.blocks[&[0, 31, 0]],
                body,
                "the trunk starts at the origin"
            );
            assert!(count(&w, flower) >= 1, "every tree ends in a flower");
            for (pos, state) in &w.blocks {
                if *state == body || *state == flower {
                    assert!(
                        pos[1] < SEA_LEVEL - 2,
                        "kelp stays under the surface: {pos:?}"
                    );
                    assert!(
                        pos[0].abs() < MAX_HORIZONTAL_DISTANCE
                            && pos[2].abs() < MAX_HORIZONTAL_DISTANCE
                    );
                }
            }
        }
        assert!(grown > 20, "most seeds grow a tree, got {grown}");
    }

    #[test]
    fn kelp_needs_water_and_a_plantable_seabed() {
        let r = registry();
        let f = feature();
        // Air at the origin: nothing happens.
        let mut w = ocean(30);
        assert!(
            !f.place(&mut w, &mut XoroshiroRandom::new(1), [0, 70, 0])
                .unwrap()
        );
        // Stone seabed: the origin is put back.
        let mut w = ocean(30);
        let stone = r.default_state("minecraft:stone").unwrap();
        w.blocks.insert([0, 30, 0], stone);
        let water = w.blocks[&[0, 31, 0]];
        assert!(
            !f.place(&mut w, &mut XoroshiroRandom::new(1), [0, 31, 0])
                .unwrap()
        );
        assert_eq!(w.blocks[&[0, 31, 0]], water);
    }

    #[test]
    fn kelp_is_deterministic() {
        let f = feature();
        let run = |seed| {
            let mut w = ocean(30);
            f.place(&mut w, &mut XoroshiroRandom::new(seed), [0, 31, 0])
                .unwrap();
            let mut cells: Vec<_> = w.blocks.into_iter().collect();
            cells.sort();
            cells
        };
        assert_eq!(run(5), run(5));
    }
}
