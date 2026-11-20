//! TerraFirmaCraft's `tfc:hot_spring` (`HotSpringFeature`, EUPL 1.2): a metaball-shaped basin of
//! spring water, lava or empty space ringed and floored with the local rock, with fissures dug
//! down from a few of its cells.

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    optional,
    placement::PlacementContext,
    required,
    tfc_fissure::{Decoration, TfcFissureFeature},
    tfc_tree::{Weighted, air_or_empty_fluid, is_air, replaceable, state_of},
    tfc_vein::{triangle, uniform_double, uniform_int},
};
use crate::{heightmap::HeightmapType, registry::BlockStateId};
use horizon_core::random::{RandomSource, XoroshiroRandom};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

const SEA_LEVEL_Y: i32 = 63;

#[derive(Clone, Debug)]
pub struct TfcHotSpringFeature {
    fluid: BlockStateId,
    /// The fluid state is air: an empty basin.
    fluid_is_air: bool,
    radius: i32,
    allow_underwater: bool,
    /// Rock name -> `(raw, gravel)`.
    rocks: Arc<HashMap<String, (BlockStateId, BlockStateId)>>,
    /// `replaces_on_fluid_contact`: block name -> weighted state, with the keys in map order.
    replacers: Option<(Vec<String>, HashMap<String, Weighted>)>,
    fissure: TfcFissureFeature,
    names: Arc<Vec<String>>,
    air: BlockStateId,
    water: BlockStateId,
}

/// `Metaballs2D.simple(random, radius)`.
pub(super) struct Metaballs {
    balls: Vec<[f64; 3]>,
}

impl Metaballs {
    pub(super) fn simple(
        random: &mut impl RandomSource,
        size: i32,
    ) -> Result<Self, horizon_core::random::InvalidBound> {
        let size = size as f32;
        let (min, max, radius) = (
            f64::from(0.1f32 * size),
            f64::from(0.3f32 * size),
            f64::from(0.5f32 * size),
        );
        let count = uniform_int(random, 3, 8).map_err(|e| match e {
            FeatureExecutionError::Random(r) => r,
            _ => unreachable!("only random errors carry no world"),
        })?;
        let mut balls = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let x = triangle(random, radius);
            let z = triangle(random, radius);
            let w = uniform_double(random, min, max);
            balls.push([x, z, w]);
        }
        Ok(Self { balls })
    }

    pub(super) fn inside(&self, x: i32, z: i32) -> bool {
        let (x, z) = (f64::from(x), f64::from(z));
        let mut f = 0.0;
        for [bx, bz, w] in &self.balls {
            f += w * w.abs() / ((x - bx) * (x - bx) + (z - bz) * (z - bz));
        }
        f > 1.0
    }
}

pub(super) fn cold_ocean_block(n: &str) -> bool {
    matches!(
        n,
        "tfc:sea_ice"
            | "minecraft:ice"
            | "minecraft:powder_snow"
            | "minecraft:snow_block"
            | "minecraft:packed_ice"
            | "minecraft:blue_ice"
    )
}

fn liquid(n: &str) -> bool {
    crate::fluid::is_water_block_name(n) || n == "minecraft:lava"
}

impl TfcHotSpringFeature {
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
        let names: Vec<String> = (0..registry.len())
            .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
            .collect();
        let fluid_json = required(&config, "fluid_state")?.get();
        let fluid = state_of(bindings, fluid_json)?;
        let fluid_is_air = is_air(&names[fluid.0 as usize]);
        let radius: i32 = match optional(&config, "radius") {
            Some(v) => serde_json::from_str(v.get())?,
            None => 14,
        };
        if !(1..=16).contains(&radius) {
            return Err(DecodeError::Field("hot spring radius"));
        }
        let allow_underwater = match optional(&config, "allow_underwater") {
            Some(v) => serde_json::from_str(v.get())?,
            None => false,
        };
        let decoration = match optional(&config, "decoration") {
            Some(d) => Some(Decoration::decode(d.get(), bindings)?),
            None => None,
        };
        let replacers = match optional(&config, "replaces_on_fluid_contact") {
            Some(list) => {
                let list: Vec<Object> = serde_json::from_str(list.get())?;
                let mut keys = Vec::new();
                let mut map = HashMap::new();
                for entry in &list {
                    let replace: Vec<String> =
                        serde_json::from_str(required(entry, "replace")?.get())?;
                    let with: Vec<Object> = serde_json::from_str(required(entry, "with")?.get())?;
                    let weighted = Weighted::decode(&with, bindings)?;
                    for name in replace {
                        if map.insert(name.clone(), weighted.clone()).is_none() {
                            keys.push(name);
                        }
                    }
                }
                Some((keys, map))
            }
            None => None,
        };
        let mut rocks = HashMap::new();
        for name in &names {
            if let Some(rock) = name.strip_prefix("tfc:raw_")
                && let (Some(raw), Some(gravel)) = (
                    registry.default_state(name),
                    registry.default_state(&format!("tfc:gravel_{rock}")),
                )
            {
                rocks.insert(rock.to_owned(), (raw, gravel));
            }
        }
        let air = registry
            .default_state("minecraft:air")
            .ok_or(DecodeError::Field("air"))?;
        let water = registry
            .default_state("minecraft:water")
            .ok_or(DecodeError::Field("water"))?;
        // The wall is replaced by the rock at the spring when it is placed.
        let fissure = TfcFissureFeature::for_spring(bindings, air, fluid, decoration);
        Ok(Self {
            fluid,
            fluid_is_air,
            radius,
            allow_underwater,
            rocks: Arc::new(rocks),
            replacers,
            fissure,
            names: Arc::new(names),
            air,
            water,
        })
    }

    fn empty_block(&self, name: &str) -> bool {
        if self.allow_underwater {
            air_or_empty_fluid(name)
        } else {
            is_air(name)
        }
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let radius = self.radius;
        let mut fork =
            XoroshiroRandom::from_state(random.next_long() as u64, random.next_long() as u64);
        let noise = Metaballs::simple(&mut fork, radius)?;
        let seed = world.world_seed().map_err(FeatureExecutionError::Query)?;
        let Some(terrain) = super::tfc_terrain::active_for(seed) else {
            return Ok(false);
        };
        let mut cache = super::tfc_terrain::TfcTerrain::rock_cache(origin[0] >> 4, origin[2] >> 4);
        let surface = terrain.surface_height_at(origin[0], origin[2]);
        let rock = terrain.rock(origin[0], 0, origin[2], surface, &mut cache);
        let Some(&(raw, gravel)) = self.rocks.get(rock) else {
            return Ok(false);
        };
        let raw_name = format!("tfc:raw_{rock}");
        // `replacers.map(map -> map.getOrDefault(rawBlock, Helpers.getRandomValue(map, random)))`:
        // the default is drawn even when the rock has an entry.
        let magma = match &self.replacers {
            Some((keys, map)) => {
                let default = random.next_int_bound(keys.len() as i32)? as usize;
                Some(map.get(&raw_name).unwrap_or(&map[&keys[default]]))
            }
            None => None,
        };
        let mut touched_water = false;
        let mut filled_empty: HashSet<[i32; 3]> = HashSet::new();
        let mut starts: Vec<[i32; 3]> = Vec::new();
        let at = |x: i32, y: i32, z: i32| [x, y, z];

        for x in -radius..=radius {
            for z in -radius..=radius {
                let (lx, lz) = (origin[0] + x, origin[2] + z);
                let mut y = world
                    .height(HeightmapType::OceanFloorWg, lx, lz)
                    .map_err(FeatureExecutionError::Query)?
                    - 1;
                // Disallow underwater locations.
                if (!self.allow_underwater && y <= SEA_LEVEL_Y) || !noise.inside(x, z) {
                    continue;
                }
                let name = self.name(world, at(lx, y, lz))?;
                if cold_ocean_block(&name) {
                    if self.allow_underwater {
                        let mut yy = 50;
                        while air_or_empty_fluid(&self.name(world, at(lx, yy, lz))?) {
                            yy -= 1;
                        }
                        y = yy;
                    } else {
                        continue;
                    }
                }
                let above = at(lx, y + 1, lz);
                let above_name = self.name(world, above)?;
                if !self.empty_block(&above_name) {
                    if replaceable(&above_name) {
                        // `stateAbove.getFluidState().createLegacyBlock()`.
                        let fill = if above_name == "minecraft:water" {
                            self.water
                        } else {
                            self.air
                        };
                        world.set_block(above, fill, 2).map_err(we)?;
                    } else {
                        continue; // Solid, non-replaceable block above.
                    }
                }
                let mut edge = false;
                if liquid(&self.name(world, at(lx, y + 1, lz))?) {
                    touched_water = true;
                }
                for [dx, dz] in [[0, -1], [1, 0], [0, 1], [-1, 0]] {
                    let p = at(lx + dx, y, lz + dz);
                    let state = self.name(world, p)?;
                    if liquid(&state) {
                        touched_water = true;
                    }
                    if self.empty_block(&state)
                        && (!self.fluid_is_air || !filled_empty.contains(&p))
                    {
                        edge = true;
                        break;
                    }
                }
                // The surface is deeper near the centre of the spring: [0.3, 1.0].
                let centre = 1.0
                    - 0.7f32
                        * ((2 * (x * x + z * z)) as f32 / (radius * radius) as f32).clamp(0.0, 1.0);
                let surface_depth = ((8 + random.next_int_bound(3)?) as f32 * centre) as i32;
                if edge {
                    // Rare holes let the water flow.
                    if random.next_int_bound(12)? == 0 {
                        world.set_block(at(lx, y, lz), self.air, 2).map_err(we)?;
                    }
                } else {
                    let here = at(lx, y, lz);
                    starts.push(here);
                    let to_place = match magma {
                        Some(m) if touched_water && self.allow_underwater => {
                            m.get(random).unwrap_or(self.fluid)
                        }
                        _ => self.fluid,
                    };
                    world.set_block(here, to_place, 2).map_err(we)?;
                    if self.fluid_is_air {
                        filled_empty.insert(here);
                    }
                    self.set_base(world, at(lx, y - 1, lz), gravel)?;
                }
                let mut dy = if edge { 0 } else { -2 };
                while dy >= -surface_depth {
                    if !self.set_base(world, at(lx, y + dy, lz), raw)? {
                        break;
                    }
                    dy -= 1;
                }
            }
        }
        if starts.is_empty() {
            return Ok(false);
        }
        let limit = random.next_int_bound((starts.len() as i32).clamp(1, 7))?;
        let count = 1 + random.next_int_bound(1 + limit)?;
        // `Helpers.uniqueRandomSample`: swap n random picks to the end of the list.
        let length = starts.len();
        for i in (length - count as usize..length).rev() {
            let j = random.next_int_bound(i as i32 + 1)? as usize;
            starts.swap(i, j);
        }
        let mut fissure = self.fissure.clone();
        fissure.wall = raw;
        for start in &starts[length - count as usize..] {
            fissure.fissure(world, random, *start, origin, 16)?;
        }
        Ok(true)
    }

    fn name<W: FeatureWorld>(&self, world: &mut W, p: [i32; 3]) -> R<String, W> {
        let s = world.block(p).map_err(we)?;
        Ok(self.names[s.0 as usize].clone())
    }

    /// `setFissureBaseBlock`.
    fn set_base<W: FeatureWorld>(
        &self,
        world: &mut W,
        p: [i32; 3],
        state: BlockStateId,
    ) -> R<bool, W> {
        if self.empty_block(&self.name(world, p)?) {
            return Ok(false);
        }
        world.set_block(p, state, 2).map_err(we)?;
        Ok(true)
    }
}

fn we<E>(e: E) -> FeatureExecutionError<E> {
    FeatureExecutionError::Basic(FeatureError::World(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::tfc_tide_pool::tests::{Flat, activate, registry};
    use serde_json::json;
    use std::collections::BTreeSet;

    fn spring(extra: serde_json::Value) -> (TfcHotSpringFeature, Flat) {
        activate();
        let r = registry();
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        let mut config =
            json!({"type": "tfc:hot_spring", "fluid_state": "minecraft:water", "radius": 8});
        for (k, v) in extra.as_object().unwrap() {
            config[k] = v.clone();
        }
        let f = TfcHotSpringFeature::decode(&config.to_string(), &bindings).unwrap();
        let world = Flat {
            blocks: HashMap::new(),
            surface: 80,
            rock: r.default_state("tfc:raw_granite").unwrap(),
            air: r.default_state("minecraft:air").unwrap(),
        };
        (f, world)
    }

    #[test]
    fn a_hot_spring_fills_a_basin_with_water_lined_in_the_local_rock() {
        let (f, mut world) = spring(json!({}));
        let mut random = XoroshiroRandom::new(17);
        assert!(f.place(&mut world, &mut random, [0, 80, 0]).unwrap());
        let water = f.fluid;
        let pools: Vec<_> = world.blocks.iter().filter(|(_, s)| **s == water).collect();
        assert!(pools.len() > 5, "water cells: {}", pools.len());
        // The basin water sits at the surface; fissures reach deep below it.
        assert!(pools.iter().any(|(p, _)| p[1] == 79));
        assert!(pools.iter().any(|(p, _)| p[1] < 70));
        // Placing again with the same seed gives the same result.
        let (_, mut again) = spring(json!({}));
        f.place(&mut again, &mut XoroshiroRandom::new(17), [0, 80, 0])
            .unwrap();
        assert_eq!(world.blocks, again.blocks);
    }

    #[test]
    fn a_spring_below_sea_level_is_not_placed_unless_underwater_is_allowed() {
        let (f, mut world) = spring(json!({}));
        world.surface = 60;
        assert!(
            !f.place(&mut world, &mut XoroshiroRandom::new(3), [0, 60, 0])
                .unwrap()
        );
        assert!(world.blocks.is_empty());
    }

    #[test]
    fn an_empty_spring_hollows_the_basin_out() {
        let (f, mut world) = spring(json!({"fluid_state": "minecraft:air"}));
        assert!(
            f.place(&mut world, &mut XoroshiroRandom::new(9), [0, 80, 0])
                .unwrap()
        );
        let air = world.air;
        assert!(world.blocks.iter().any(|(p, s)| *s == air && p[1] == 79));
    }
}
