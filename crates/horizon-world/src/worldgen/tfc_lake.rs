//! TerraFirmaCraft's `tfc:flood_fill_lake` (`FloodFillLakeFeature`, EUPL 1.2): fills a natural
//! depression with a localised flood fill, limited to the 3x3 chunks around the origin; a lake that
//! would leak out of that area, or reach a border block, is not placed. Solid, leaf and bamboo
//! tests match block names.

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    optional,
    placement::PlacementContext,
    required,
    tfc_tree::{is_air, is_fluid_block, is_leaves, replaceable},
};
use crate::registry::BlockStateId;
use horizon_core::random::RandomSource;
use std::{collections::HashSet, sync::Arc};

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

/// Safety bound on one flood fill; the 3x3 chunk box already limits it far below this.
const MAX_FILLED: usize = 1_000_000;

const DIRECTIONS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

#[derive(Clone, Debug)]
pub struct TfcLakeFeature {
    fill: BlockStateId,
    replace: Vec<String>,
    overfill: bool,
    dirt: Option<BlockStateId>,
    names: Arc<Vec<String>>,
}

fn state_name(object: &Object) -> Result<String, DecodeError> {
    let value: serde_json::Value = serde_json::from_str(required(object, "state")?.get())?;
    match value {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Object(o) => o
            .get("Name")
            .and_then(|n| n.as_str())
            .map(str::to_owned)
            .ok_or(DecodeError::Field("lake state")),
        _ => Err(DecodeError::Field("lake state")),
    }
}

impl TfcLakeFeature {
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
        let fill = registry
            .default_state(&state_name(&config)?)
            .ok_or(DecodeError::Field("lake fluid"))?;
        let replace: Vec<String> = match optional(&config, "replace_fluids") {
            Some(list) => serde_json::from_str(list.get())?,
            None => Vec::new(),
        };
        let overfill = match optional(&config, "overfill") {
            Some(v) => serde_json::from_str(v.get())?,
            None => false,
        };
        Ok(Self {
            fill,
            replace,
            overfill,
            dirt: registry.default_state("minecraft:dirt"),
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
        let mut p = Filler {
            feature: self,
            world,
            min: [(origin[0] >> 4 << 4) - 14, (origin[2] >> 4 << 4) - 14],
            max: [
                (origin[0] >> 4 << 4) + 15 + 14,
                (origin[2] >> 4 << 4) + 15 + 14,
            ],
        };
        p.run(origin)
    }
}

struct Filler<'a, W> {
    feature: &'a TfcLakeFeature,
    world: &'a mut W,
    min: [i32; 2],
    max: [i32; 2],
}

fn we<E>(e: E) -> FeatureExecutionError<E> {
    FeatureExecutionError::Basic(FeatureError::World(e))
}

impl<W: FeatureWorld + PlacementContext> Filler<'_, W> {
    fn at(&mut self, p: [i32; 3]) -> R<String, W> {
        let s = self.world.block(p).map_err(we)?;
        Ok(self.feature.names[s.0 as usize].clone())
    }

    /// `!state.isSolid() && !leaves && config.shouldReplace(fluid)`.
    fn fillable(&self, n: &str) -> bool {
        if is_leaves(n) {
            return false;
        }
        if is_air(n) {
            return true;
        }
        if is_fluid_block(n) {
            return self.feature.replace.iter().any(|f| f == n);
        }
        replaceable(n)
    }

    fn inside(&self, p: [i32; 3]) -> bool {
        p[0] >= self.min[0] && p[0] <= self.max[0] && p[2] >= self.min[1] && p[2] <= self.max[1]
    }

    fn run(&mut self, origin: [i32; 3]) -> R<bool, W> {
        // First make sure we're at the lowest point in the column.
        let mut cursor = origin;
        while is_air(&self.at(cursor)?) && cursor[1] > 11 {
            cursor[1] -= 1;
        }
        let start = [cursor[0], cursor[1] + 1, cursor[2]];
        let mut filled: HashSet<[i32; 3]> = HashSet::new();
        if !self.fill(start, &mut filled)? || filled.len() < 20 {
            return Ok(false);
        }
        // `HashSet` order is unspecified, but every block is written independently.
        let fill = self.feature.fill;
        for p in filled.iter().copied() {
            self.world.set_block(p, fill, 2).map_err(we)?;
            let below = [p[0], p[1] - 1, p[2]];
            if !filled.contains(&below) && self.at(below)? == "minecraft:grass_block" {
                if let Some(dirt) = self.feature.dirt {
                    self.world.set_block(below, dirt, 2).map_err(we)?;
                }
            }
        }
        Ok(true)
    }

    fn fill(&mut self, start: [i32; 3], filled: &mut HashSet<[i32; 3]>) -> R<bool, W> {
        if !self.layer(start, filled)? {
            return Ok(false);
        }
        if !self.feature.overfill {
            return Ok(true);
        }
        // Overfill upwards until a solid ceiling, an illegal border or a leak.
        let mut next = filled.clone();
        let mut start = [start[0], start[1] + 1, start[2]];
        while self.layer(start, &mut next)? {
            filled.extend(next.iter().copied());
            start[1] += 1;
        }
        Ok(true)
    }

    fn layer(&mut self, start: [i32; 3], filled: &mut HashSet<[i32; 3]>) -> R<bool, W> {
        let n = self.at(start)?;
        if !self.fillable(&n) {
            return Ok(false);
        }
        let max_y = start[1];
        let mut stack = vec![start];
        filled.insert(start);
        while let Some(at) = stack.pop() {
            for d in DIRECTIONS {
                let p = [at[0] + d[0], at[1] + d[1], at[2] + d[2]];
                if filled.contains(&p) || p[1] > max_y {
                    continue;
                }
                let n = self.at(p)?;
                if self.fillable(&n) {
                    if !self.inside(p) {
                        return Ok(false);
                    }
                    filled.insert(p);
                    stack.push(p);
                    if filled.len() > MAX_FILLED {
                        return Ok(false);
                    }
                } else if n.contains("bamboo") {
                    return Ok(false);
                }
            }
        }
        Ok(!filled.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        heightmap::HeightmapType,
        registry::BlockRegistry,
        worldgen::{anchor::WorldGenerationContext, placement::PlacementQueryError},
    };
    use horizon_core::random::XoroshiroRandom;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    fn registry() -> &'static BlockRegistry {
        let names = [
            "minecraft:air",
            "minecraft:cave_air",
            "minecraft:void_air",
            "minecraft:stone",
            "minecraft:water",
            "minecraft:lava",
        ];
        let mut rows = Vec::new();
        let mut defaults = BTreeMap::new();
        for n in names {
            defaults.insert(n, rows.len());
            if n.ends_with("water") || n.ends_with("lava") {
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
        air: BlockStateId,
        stone: BlockStateId,
    }
    impl FeatureWorld for World {
        type Error = ();
        fn block(&mut self, p: [i32; 3]) -> Result<BlockStateId, ()> {
            if let Some(s) = self.blocks.get(&p) {
                return Ok(*s);
            }
            // Solid ground below y = 60, air above.
            Ok(if p[1] < 60 { self.stone } else { self.air })
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
            Err(PlacementQueryError::Unavailable)
        }
    }

    fn feature(overfill: bool) -> TfcLakeFeature {
        let bindings = FeatureBlockBindings::new(registry(), BTreeSet::new()).unwrap();
        TfcLakeFeature::decode(
            &json!({"type": "tfc:flood_fill_lake", "state": "minecraft:water", "replace_fluids": [], "overfill": overfill}).to_string(),
            &bindings,
        )
        .unwrap()
    }

    fn world() -> World {
        let r = registry();
        World {
            blocks: HashMap::new(),
            air: r.default_state("minecraft:air").unwrap(),
            stone: r.default_state("minecraft:stone").unwrap(),
        }
    }

    #[test]
    fn a_bowl_fills_to_its_rim_and_a_flat_plain_does_not() {
        let r = registry();
        // A flat plain leaks over the open box, so nothing is placed.
        let mut flat = world();
        assert!(
            !feature(false)
                .place(&mut flat, &mut XoroshiroRandom::new(1), [8, 62, 8])
                .unwrap()
        );
        // A 7x7 bowl dug 2 deep at y 58..=59 with a rim at y = 60.
        let mut bowl = world();
        let air = r.default_state("minecraft:air").unwrap();
        let stone = r.default_state("minecraft:stone").unwrap();
        for x in 4..=12 {
            for z in 4..=12 {
                let edge = x == 4 || x == 12 || z == 4 || z == 12;
                for y in 58..=62 {
                    bowl.blocks.insert(
                        [x, y, z],
                        if edge && y <= 62 {
                            stone
                        } else if y <= 59 {
                            air
                        } else {
                            air
                        },
                    );
                }
            }
        }
        let mut overfilled = World {
            blocks: bowl.blocks.clone(),
            air,
            stone,
        };
        let water = r.default_state("minecraft:water").unwrap();
        // Without overfill only the lowest layer is filled.
        assert!(
            feature(false)
                .place(&mut bowl, &mut XoroshiroRandom::new(1), [8, 61, 8])
                .unwrap()
        );
        assert_eq!(bowl.blocks[&[8, 58, 8]], water);
        assert_ne!(bowl.blocks.get(&[8, 59, 8]), Some(&water));
        // With overfill the bowl fills to the top of its walls.
        assert!(
            feature(true)
                .place(&mut overfilled, &mut XoroshiroRandom::new(1), [8, 61, 8])
                .unwrap()
        );
        assert_eq!(overfilled.blocks[&[8, 62, 8]], water);
        assert_ne!(overfilled.blocks.get(&[8, 63, 8]), Some(&water));
        assert_ne!(overfilled.blocks[&[4, 59, 4]], water);
    }
}
