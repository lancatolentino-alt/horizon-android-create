//! TerraFirmaCraft's `tfc:powder_snow` (`PowderSnowFeature`, EUPL 1.2): a 3D metaball blob of
//! powder snow replacing the snow and ice of very cold surfaces above sea level.

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    optional,
    placement::PlacementContext,
    required,
    tfc_tree::state_of,
    tfc_vein::{triangle, uniform_double, uniform_int},
};
use crate::registry::BlockStateId;
use horizon_core::random::{RandomSource, XoroshiroRandom};
use std::sync::Arc;

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

const SEA_LEVEL: i32 = 63;

/// `Metaballs3D`.
struct Metaballs3D {
    balls: Vec<[f64; 4]>,
}

impl Metaballs3D {
    fn new(
        random: &mut impl RandomSource,
        (min_balls, max_balls): (i32, i32),
        min_size: f64,
        max_size: f64,
        radius: f64,
    ) -> Result<Self, horizon_core::random::InvalidBound> {
        let count = uniform_int(random, min_balls, max_balls).map_err(|e| match e {
            FeatureExecutionError::Random(r) => r,
            _ => unreachable!("only random errors carry no world"),
        })?;
        let negative = if min_size < 0.0 {
            (f64::from(count) * (-min_size / (max_size - min_size))) as i32
        } else {
            0
        };
        let mut balls = Vec::with_capacity(count as usize);
        for i in 0..count {
            let x = triangle(random, radius);
            let y = triangle(random, radius);
            let z = triangle(random, radius);
            let w = if i < negative {
                uniform_double(random, min_size, 0.0)
            } else {
                uniform_double(random, 0.0, max_size)
            };
            balls.push([x, y, z, w]);
        }
        Ok(Self { balls })
    }

    fn inside(&self, x: i32, y: i32, z: i32) -> bool {
        let (x, y, z) = (f64::from(x), f64::from(y), f64::from(z));
        let mut f = 0.0;
        for [bx, by, bz, w] in &self.balls {
            f += w * w.abs() / ((x - bx) * (x - bx) + (y - by) * (y - by) + (z - bz) * (z - bz));
            if f > 1.0 {
                return true;
            }
        }
        false
    }
}

#[derive(Clone, Debug)]
pub struct TfcPowderSnowFeature {
    state: BlockStateId,
    /// `#tfc:powder_snow_replaceable`: snow block, packed ice, blue ice.
    replaceable: Arc<Vec<bool>>,
}

impl TfcPowderSnowFeature {
    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(json)?;
        let config: Object = match optional(&object, "config") {
            Some(config) => serde_json::from_str(config.get())?,
            None => object.clone(),
        };
        let state = state_of(bindings, required(&config, "state")?.get())?;
        let registry = bindings.registry();
        let replaceable = (0..registry.len())
            .map(|i| {
                matches!(
                    registry
                        .state(BlockStateId(i as u32))
                        .map(|s| s.name.as_str()),
                    Some("minecraft:snow_block" | "minecraft:packed_ice" | "minecraft:blue_ice")
                )
            })
            .collect();
        Ok(Self {
            state,
            replaceable: Arc::new(replaceable),
        })
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let size = 6 + random.next_int_bound(4)?;
        let mut fork =
            XoroshiroRandom::from_state(random.next_long() as u64, random.next_long() as u64);
        let size_f = size as f32;
        let noise = Metaballs3D::new(
            &mut fork,
            (6, 8),
            f64::from(-0.15f32 * size_f),
            f64::from(0.4f32 * size_f),
            f64::from(0.3f32 * size_f),
        )?;
        for x in -size..=size {
            for y in -size..=size {
                for z in -size..=size {
                    if !noise.inside(x, y, z) {
                        continue;
                    }
                    let pos = [origin[0] + x, origin[1] + y, origin[2] + z];
                    // Do not replace below sea level where water might flood back in.
                    if pos[1] > SEA_LEVEL {
                        let found = world
                            .block(pos)
                            .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))?;
                        if self.replaceable[found.0 as usize] {
                            world.set_block(pos, self.state, 3).map_err(|e| {
                                FeatureExecutionError::Basic(FeatureError::World(e))
                            })?;
                        }
                    }
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
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    fn registry() -> &'static BlockRegistry {
        let names = [
            "minecraft:air",
            "minecraft:cave_air",
            "minecraft:void_air",
            "minecraft:stone",
            "minecraft:snow_block",
            "minecraft:packed_ice",
            "minecraft:powder_snow",
        ];
        let rows: Vec<_> = names
            .iter()
            .map(|n| json!([n, n.ends_with("air"), false, "", {"id": n, "properties": {}}]))
            .collect();
        let defaults: BTreeMap<_, _> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let facts = json!({"states": rows, "defaults": defaults});
        Box::leak(Box::new(
            BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
        ))
    }

    #[test]
    fn powder_snow_only_replaces_snow_and_ice_above_sea_level() {
        let r = registry();
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        let feature = TfcPowderSnowFeature::decode(
            r#"{"type":"tfc:powder_snow","state":{"Name":"minecraft:powder_snow"}}"#,
            &bindings,
        )
        .unwrap();
        let snow = r.default_state("minecraft:snow_block").unwrap();
        let ice = r.default_state("minecraft:packed_ice").unwrap();
        let stone = r.default_state("minecraft:stone").unwrap();
        let powder = r.default_state("minecraft:powder_snow").unwrap();
        let mut total = 0;
        for seed in 0..10 {
            let mut blocks = HashMap::new();
            for x in -10..=10 {
                for y in 50..=90 {
                    for z in -10..=10 {
                        let s = if (x + z) % 3 == 0 {
                            stone
                        } else if y % 2 == 0 {
                            snow
                        } else {
                            ice
                        };
                        blocks.insert([x, y, z], s);
                    }
                }
            }
            let mut world = Flat {
                blocks,
                surface: 0,
                rock: stone,
                air: r.default_state("minecraft:air").unwrap(),
            };
            assert!(
                feature
                    .place(&mut world, &mut XoroshiroRandom::new(seed), [0, 66, 0])
                    .unwrap()
            );
            for (p, s) in &world.blocks {
                if *s == powder {
                    total += 1;
                    assert!(p[1] > SEA_LEVEL, "{p:?} is below the sea");
                    assert!(
                        (p[0].abs()).max(p[1] - 66).max(p[2].abs()) <= 9,
                        "{p:?} too far"
                    );
                    assert_ne!((p[0] + p[2]).rem_euclid(3), 0, "stone must stay stone");
                }
            }
        }
        assert!(total > 50, "{total} powder snow blocks in 10 tries");
    }
}
