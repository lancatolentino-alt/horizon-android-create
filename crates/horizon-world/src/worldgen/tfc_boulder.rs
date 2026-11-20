//! TerraFirmaCraft's `tfc:boulder` and `tfc:baby_boulder` features (`BouldersFeature`,
//! `BabyBoulderFeature`, `Metaballs3D`; EUPL 1.2): rock piles that take their blocks from the
//! rock at the surface of the column they stand on.

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    density::identifier,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    optional,
    placement::PlacementContext,
    required, tfc_terrain,
    tfc_vein::{triangle, uniform_double, uniform_int},
};
use crate::registry::BlockStateId;
use horizon_core::random::{RandomSource, XoroshiroRandom};
use std::collections::HashMap;

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

#[derive(Clone, Debug)]
pub struct TfcBoulderFeature {
    baby: bool,
    /// Rock name (`granite`) to the blocks a boulder of that rock is made of.
    states: HashMap<String, Vec<BlockStateId>>,
}

/// `Metaballs3D`: signed weights summed over inverse squared distance.
struct Metaballs {
    balls: Vec<[f64; 4]>,
}

impl Metaballs {
    /// `new Metaballs3D(random, minBalls, maxBalls, minSize, maxSize, radius)`.
    fn new(
        random: &mut impl RandomSource,
        min_balls: i32,
        max_balls: i32,
        min_size: f64,
        max_size: f64,
        radius: f64,
    ) -> Result<Self, FeatureExecutionError<()>> {
        let count = uniform_int(random, min_balls, max_balls)?;
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

    fn inside(&self, x: f64, y: f64, z: f64) -> bool {
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

fn map_unit<W: FeatureWorld>(e: FeatureExecutionError<()>) -> FeatureExecutionError<W::Error> {
    match e {
        FeatureExecutionError::Random(r) => FeatureExecutionError::Random(r),
        _ => unreachable!("only random errors carry no world"),
    }
}

impl TfcBoulderFeature {
    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(json)?;
        let kind = identifier(&serde_json::from_str::<String>(
            required(&object, "type")?.get(),
        )?)?;
        let config: Object = match optional(&object, "config") {
            Some(config) => serde_json::from_str(config.get())?,
            None => object.clone(),
        };
        let registry = bindings.registry();
        let mut states = HashMap::new();
        let entries: Vec<Object> = serde_json::from_str(required(&config, "states")?.get())?;
        for entry in &entries {
            let rock: String = serde_json::from_str(required(entry, "rock")?.get())?;
            // `tfc:raw_granite` -> `granite`.
            let name = rock
                .strip_prefix("tfc:raw_")
                .ok_or(DecodeError::Field("boulder rock"))?
                .to_owned();
            let blocks: Vec<String> = serde_json::from_str(required(entry, "blocks")?.get())?;
            let mut ids = Vec::with_capacity(blocks.len());
            for block in blocks {
                ids.push(
                    registry
                        .default_state(&block)
                        .ok_or(DecodeError::Field("boulder block"))?,
                );
            }
            if ids.is_empty() {
                return Err(DecodeError::Field("boulder blocks"));
            }
            states.insert(name, ids);
        }
        let _ = optional(&config, "unused");
        match kind.as_str() {
            "tfc:boulder" => Ok(Self {
                baby: false,
                states,
            }),
            "tfc:baby_boulder" => Ok(Self { baby: true, states }),
            _ => Err(DecodeError::Field("boulder type")),
        }
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
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
        let Some(states) = self.states.get(rock) else {
            return Ok(false);
        };
        let we = |e| FeatureExecutionError::Basic(FeatureError::World(e));
        let rnd = |r: Result<i32, _>| r.map_err(FeatureExecutionError::Random);
        // `stateSupplier`: a single state draws nothing.
        macro_rules! pick {
            () => {
                if states.len() == 1 {
                    states[0]
                } else {
                    states[rnd(random.next_int_bound(states.len() as i32))? as usize]
                }
            };
        }
        let [ox, oy, oz] = origin;
        if !self.baby {
            let size = 6 + rnd(random.next_int_bound(4))?;
            let mut fork =
                XoroshiroRandom::from_state(random.next_long() as u64, random.next_long() as u64);
            let balls = Metaballs::new(
                &mut fork,
                6,
                8,
                f64::from(-0.12f32 * size as f32),
                f64::from(0.3f32 * size as f32),
                f64::from(0.3f32 * size as f32),
            )
            .map_err(map_unit::<W>)?;
            for x in -size..=size {
                for y in -size..=size {
                    for z in -size..=size {
                        if balls.inside(f64::from(x), f64::from(y), f64::from(z)) {
                            let state = pick!();
                            world
                                .set_block([ox + x, oy + y, oz + z], state, 2)
                                .map_err(we)?;
                        }
                    }
                }
            }
        } else if random.next_float() < 0.5 {
            // A small sphere-like boulder.
            let state = pick!();
            world.set_block(origin, state, 2).map_err(we)?;
            // `Helpers.DIRECTIONS`: down, up, north, south, west, east.
            for [dx, dy, dz] in [
                [0, -1, 0],
                [0, 1, 0],
                [0, 0, -1],
                [0, 0, 1],
                [-1, 0, 0],
                [1, 0, 0],
            ] {
                if random.next_float() < 0.8 {
                    let state = pick!();
                    world
                        .set_block([ox + dx, oy + dy, oz + dz], state, 2)
                        .map_err(we)?;
                }
            }
        } else if random.next_float() < 0.5 {
            // A short column of rock with an optional second column next to it.
            let height = 4 + rnd(random.next_int_bound(3))?;
            for i in 0..height {
                let state = pick!();
                world
                    .set_block([ox, oy - 2 + i, oz], state, 2)
                    .map_err(we)?;
            }
            // `Direction.Plane.HORIZONTAL.getRandomDirection`: north, east, south, west.
            let [dx, dz] =
                [[0, -1], [1, 0], [0, 1], [-1, 0]][rnd(random.next_int_bound(4))? as usize];
            for i in 0..height - 2 {
                let state = pick!();
                world
                    .set_block([ox + dx, oy - 2 + i, oz + dz], state, 2)
                    .map_err(we)?;
            }
        } else {
            // A single block with a single block underneath.
            let state = pick!();
            world.set_block(origin, state, 2).map_err(we)?;
            let state = pick!();
            world.set_block([ox, oy - 1, oz], state, 2).map_err(we)?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::placement::PlacementModifier;

    #[test]
    fn metaball_boulders_fill_a_compact_blob() {
        let mut random = XoroshiroRandom::new(42);
        let size = 7;
        let balls = Metaballs::new(
            &mut random,
            6,
            8,
            f64::from(-0.12f32 * size as f32),
            f64::from(0.3f32 * size as f32),
            f64::from(0.3f32 * size as f32),
        )
        .unwrap();
        let mut inside = 0;
        for x in -size..=size {
            for y in -size..=size {
                for z in -size..=size {
                    if balls.inside(f64::from(x), f64::from(y), f64::from(z)) {
                        inside += 1;
                    }
                }
            }
        }
        assert!(inside > 10 && inside < 15 * 15 * 15 / 2, "{inside}");
    }

    #[test]
    fn tfc_placement_modifiers_decode() {
        PlacementModifier::from_json(
            r#"{"type":"tfc:biome","biomes":["plateau","rolling_hills"]}"#,
        )
        .unwrap();
        assert!(
            PlacementModifier::from_json(r#"{"type":"tfc:biome","biomes":["nowhere"]}"#).is_err()
        );
    }
}
