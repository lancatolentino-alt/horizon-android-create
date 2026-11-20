//! TerraFirmaCraft's `tfc:cluster_vein`, `tfc:disc_vein` and `tfc:pipe_vein`
//! features, ported from TFC's `world.feature.vein` and `world.noise.Metaballs*`
//! (TerraFirmaCraft, EUPL 1.2). Random draw order and float/double mixing follow
//! the Java source so a vein lands where it does in the original.
#![allow(clippy::too_many_arguments, clippy::excessive_precision)]
use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    density::identifier,
    double,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    integer, optional,
    placement::PlacementContext,
    required,
    tfc_noise::OpenSimplex3D,
};
use crate::heightmap::HeightmapType;
use crate::registry::BlockStateId;
use horizon_core::geometry::trig;
use horizon_core::random::{RandomSource, XoroshiroRandom, seed_from_hash};
use std::collections::HashMap;

const NO_ORE: i32 = i32::MIN;
const PRIME_X: i32 = 501_125_321;
const PRIME_Y: i32 = 1_136_930_381;

/// `Helpers.hash(salt, x, y, z)`.
fn hash(salt: i64, x: i32, y: i32, z: i32) -> i32 {
    let mut h = salt
        ^ i64::from(x).wrapping_mul(i64::from(PRIME_X))
        ^ i64::from(y).wrapping_mul(i64::from(PRIME_Y))
        ^ i64::from(z);
    h = h.wrapping_mul(0x27d4_eb2d);
    h as i32
}

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

pub(super) fn uniform_int(
    random: &mut impl RandomSource,
    min: i32,
    max: i32,
) -> Result<i32, FeatureExecutionError<()>> {
    if min == max {
        Ok(min)
    } else {
        random
            .next_int_bound(max - min)
            .map(|v| min + v)
            .map_err(FeatureExecutionError::Random)
    }
}
/// `Helpers.uniform(random, double, double)`.
pub(super) fn uniform_double(random: &mut impl RandomSource, min: f64, max: f64) -> f64 {
    random.next_double() * (max - min) + min
}
/// `Helpers.triangle(random, double delta)`.
pub(super) fn triangle(random: &mut impl RandomSource, delta: f64) -> f64 {
    (random.next_double() - random.next_double()) * delta
}
fn bound<W: FeatureWorld>(random: &mut impl RandomSource, bound: i32) -> R<i32, W> {
    random
        .next_int_bound(bound)
        .map_err(FeatureExecutionError::Random)
}
/// `IWeighted<BlockState>`: a singleton ignores weights, otherwise a cumulative
/// `TreeMap` with `higherEntry`. Both consume one `nextDouble`.
#[derive(Clone, Debug)]
struct Weighted {
    cumulative: Vec<(f64, BlockStateId)>,
    total: f64,
    singleton: bool,
}
impl Weighted {
    fn decode(raw: &str, bindings: &FeatureBlockBindings<'_>) -> Result<Self, DecodeError> {
        let list: Vec<Object> = serde_json::from_str(raw)?;
        let mut out = Self {
            cumulative: Vec::new(),
            total: 0.0,
            singleton: list.len() == 1,
        };
        for entry in &list {
            let state = bindings
                .codec
                .decode(required(entry, "block")?.get(), false)
                .map_err(|e| DecodeError::Registry(e.to_string()))?;
            let weight = match optional(entry, "weight") {
                Some(w) => double(w)?,
                None => 1.0,
            };
            if weight > 0.0 {
                out.total += weight;
                out.cumulative.push((out.total, state));
            } else if out.singleton {
                out.cumulative.push((1.0, state));
            }
        }
        Ok(out)
    }
    fn get(&self, random: &mut impl RandomSource) -> Option<BlockStateId> {
        let r = random.next_double();
        if self.singleton {
            return self.cumulative.first().map(|e| e.1);
        }
        let value = r * self.total;
        self.cumulative
            .iter()
            .find(|e| e.0 > value)
            .or(self.cumulative.last())
            .map(|e| e.1)
    }
}

#[derive(Clone, Debug)]
struct Indicator {
    depth: i32,
    rarity: i32,
    underground_rarity: i32,
    underground_count: i32,
    states: Weighted,
}

#[derive(Clone, Debug)]
struct Common {
    states: HashMap<BlockStateId, Weighted>,
    indicator: Option<Indicator>,
    rarity: i32,
    density: f32,
    min_y: i32,
    max_y: i32,
    project: bool,
    project_offset: bool,
    seed: i64,
    near_lava: bool,
    air: BlockStateId,
    lava: Option<BlockStateId>,
    /// `KaolinDiscVeinFeature`: the clay chosen by depth replaces the stone's own ore table.
    kaolin: Option<std::sync::Arc<Kaolin>>,
}

/// What `KaolinDiscVeinFeature.getStateToGenerate` needs, by state id.
#[derive(Debug)]
struct Kaolin {
    grass: Vec<bool>,
    replaceable: Vec<bool>,
    grass_state: BlockStateId,
    red: BlockStateId,
    pink: BlockStateId,
    white: BlockStateId,
}

/// `#tfc:kaolin_clay_replaceable` by name: dirt, stones, gravels and snow.
fn kaolin_replaceable(n: &str) -> bool {
    matches!(
        n,
        "minecraft:dirt"
            | "minecraft:grass_block"
            | "minecraft:coarse_dirt"
            | "minecraft:podzol"
            | "minecraft:rooted_dirt"
            | "minecraft:mud"
            | "minecraft:mycelium"
            | "minecraft:gravel"
            | "minecraft:snow_block"
            | "minecraft:stone"
            | "minecraft:granite"
            | "minecraft:diorite"
            | "minecraft:andesite"
            | "minecraft:deepslate"
            | "minecraft:tuff"
            | "minecraft:calcite"
    ) || n.starts_with("tfc:raw_")
        || n.starts_with("tfc:gravel_")
        || n.starts_with("tfc:dirt/")
        || n.starts_with("tfc:grass/")
}

#[derive(Clone, Debug)]
enum Shape {
    Cluster {
        size: i32,
    },
    Disc {
        size: i32,
        height: i32,
    },
    Pipe {
        height: i32,
        radius: i32,
        min_skew: i32,
        max_skew: i32,
        min_slant: i32,
        max_slant: i32,
        sign: f32,
        edge: Box<OpenSimplex3D>,
    },
}

#[derive(Clone, Debug)]
pub struct TfcVeinFeature {
    common: Common,
    shape: Shape,
}

enum Vein {
    Cluster {
        pos: [i32; 3],
        balls: Vec<[f64; 4]>,
    },
    Disc {
        pos: [i32; 3],
        balls: Vec<[f64; 3]>,
    },
    Pipe {
        pos: [i32; 3],
        sign: i32,
        skew_x: f32,
        skew_z: f32,
        skew: i32,
        slant: i32,
    },
}
impl Vein {
    fn pos(&self) -> [i32; 3] {
        match self {
            Vein::Cluster { pos, .. } | Vein::Disc { pos, .. } | Vein::Pipe { pos, .. } => *pos,
        }
    }
}

/// `Mth.clampedMap(float, float, float, float, float)`.
fn clamped_map_f(value: f32, from_min: f32, from_max: f32, to_min: f32, to_max: f32) -> f32 {
    let t = (value - from_min) / (from_max - from_min);
    if t < 0.0 {
        to_min
    } else if t > 1.0 {
        to_max
    } else {
        to_min + t * (to_max - to_min)
    }
}
/// `Mth.map(double, ...)`.
fn map_d(value: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    let t = (value - from_min) / (from_max - from_min);
    to_min + t * (to_max - to_min)
}

impl TfcVeinFeature {
    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(json)?;
        let kind = identifier(&serde_json::from_str::<String>(
            required(&object, "type")?.get(),
        )?)?;
        // TFC's 1.21 codecs flatten VeinConfig into the feature's own "config" object; 26.3 data
        // has no wrapper, so the feature's own fields are the config.
        let config: Object = match optional(&object, "config") {
            Some(config) => serde_json::from_str(config.get())?,
            None => object.clone(),
        };
        let registry = bindings.registry();
        let mut states = HashMap::new();
        let blocks: Vec<Object> = serde_json::from_str(required(&config, "blocks")?.get())?;
        for entry in &blocks {
            let weighted = Weighted::decode(required(entry, "with")?.get(), bindings)?;
            let replace: Vec<String> = serde_json::from_str(required(entry, "replace")?.get())?;
            for name in replace {
                if let Some(state) = registry.default_state(&name) {
                    states.insert(state, weighted.clone());
                }
            }
        }
        let indicator = match optional(&config, "indicator") {
            None => None,
            Some(raw) => {
                let ind: Object = serde_json::from_str(raw.get())?;
                Some(Indicator {
                    depth: integer(required(&ind, "depth")?)?,
                    rarity: integer(required(&ind, "rarity")?)?,
                    underground_rarity: integer(required(&ind, "underground_rarity")?)?,
                    underground_count: integer(required(&ind, "underground_count")?)?,
                    states: Weighted::decode(required(&ind, "blocks")?.get(), bindings)?,
                })
            }
        };
        let rarity = integer(required(&config, "rarity")?)?;
        if rarity < 1 {
            return Err(DecodeError::Field("vein rarity"));
        }
        let density: f32 = super::number_text(required(&config, "density")?)?
            .parse()
            .map_err(|_| DecodeError::Field("vein density"))?;
        let flag =
            |name: &str| -> bool { optional(&config, name).is_some_and(|v| v.get() == "true") };
        // `random_name` is a long, or a string hashed with the 128-bit seed hash.
        let seed = {
            let raw = required(&config, "random_name")?.get();
            if let Ok(name) = serde_json::from_str::<String>(raw) {
                let [lo, hi] = seed_from_hash(&name);
                (lo ^ hi) as i64
            } else {
                raw.parse::<i64>()
                    .map_err(|_| DecodeError::Field("vein random_name"))?
            }
        };
        let shape = match kind.as_str() {
            "tfc:cluster_vein" => Shape::Cluster {
                size: integer(required(&config, "size")?)?,
            },
            "tfc:disc_vein" | "tfc:kaolin_disc_vein" => Shape::Disc {
                size: integer(required(&config, "size")?)?,
                height: integer(required(&config, "height")?)?,
            },
            "tfc:pipe_vein" => Shape::Pipe {
                height: integer(required(&config, "height")?)?,
                radius: integer(required(&config, "radius")?)?,
                min_skew: integer(required(&config, "min_skew")?)?,
                max_skew: integer(required(&config, "max_skew")?)?,
                min_slant: integer(required(&config, "min_slant")?)?,
                max_slant: integer(required(&config, "max_slant")?)?,
                sign: super::number_text(required(&config, "sign")?)?
                    .parse()
                    .map_err(|_| DecodeError::Field("vein sign"))?,
                edge: Box::new(OpenSimplex3D::new(15_189_385).octaves(2).scaled(0.0, -9.0)),
            },
            _ => return Err(DecodeError::Field("tfc vein type")),
        };
        Ok(Self {
            common: Common {
                states,
                indicator,
                rarity,
                density,
                min_y: integer(required(&config, "min_y")?)?,
                max_y: integer(required(&config, "max_y")?)?,
                project: flag("project"),
                project_offset: flag("project_offset"),
                seed,
                near_lava: flag("near_lava"),
                air: bindings.air[0],
                lava: registry.default_state("minecraft:lava"),
                kaolin: if kind == "tfc:kaolin_disc_vein" {
                    let names: Vec<String> = (0..registry.len())
                        .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
                        .collect();
                    let get = |n: &str| {
                        registry
                            .default_state(n)
                            .ok_or_else(|| DecodeError::Registry(format!("unknown block {n}")))
                    };
                    Some(std::sync::Arc::new(Kaolin {
                        grass: names.iter().map(|n| n == "minecraft:grass_block").collect(),
                        replaceable: names.iter().map(|n| kaolin_replaceable(n)).collect(),
                        grass_state: get("tfc:kaolin_clay_grass")?,
                        red: get("tfc:red_kaolin_clay")?,
                        pink: get("tfc:pink_kaolin_clay")?,
                        white: get("tfc:white_kaolin_clay")?,
                    }))
                } else {
                    None
                },
            },
            shape,
        })
    }

    fn chunk_radius(&self) -> i32 {
        match &self.shape {
            Shape::Cluster { size } | Shape::Disc { size, .. } => 1 + (size >> 4),
            Shape::Pipe { radius, .. } => 1 + (radius >> 4),
        }
    }
    fn default_y(
        &self,
        shrink: i32,
        random: &mut impl RandomSource,
    ) -> Result<i32, FeatureExecutionError<()>> {
        let c = &self.common;
        let range = c.max_y - c.min_y - 2 * shrink;
        if range > 0 {
            Ok(c.min_y
                + shrink
                + random
                    .next_int_bound(range)
                    .map_err(FeatureExecutionError::Random)?)
        } else {
            Ok((c.min_y + c.max_y) / 2)
        }
    }
    fn default_pos(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        shrink: i32,
        random: &mut impl RandomSource,
    ) -> Result<[i32; 3], FeatureExecutionError<()>> {
        let x = chunk_x
            + random
                .next_int_bound(16)
                .map_err(FeatureExecutionError::Random)?;
        let y = self.default_y(shrink, random)?;
        let z = chunk_z
            + random
                .next_int_bound(16)
                .map_err(FeatureExecutionError::Random)?;
        Ok([x, y, z])
    }

    fn create_vein(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        random: &mut impl RandomSource,
    ) -> Result<Vein, FeatureExecutionError<()>> {
        match &self.shape {
            Shape::Cluster { size } => {
                let pos = self.default_pos(chunk_x, chunk_z, *size, random)?;
                // Metaballs3D.simple: 5..7 balls, sizes 0.1f*size..0.3f*size, radius 0.5f*size
                let max_size = f64::from(0.3f32 * *size as f32);
                let radius = f64::from(0.5f32 * *size as f32);
                let count = uniform_int(random, 5, 7)?;
                let mut balls = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let x = triangle(random, radius);
                    let y = triangle(random, radius);
                    let z = triangle(random, radius);
                    let w = uniform_double(random, 0.0, max_size);
                    balls.push([x, y, z, w]);
                }
                Ok(Vein::Cluster { pos, balls })
            }
            Shape::Disc { size, .. } => {
                let pos = self.default_pos(chunk_x, chunk_z, *size, random)?;
                let min_size = f64::from(0.1f32 * *size as f32);
                let max_size = f64::from(0.3f32 * *size as f32);
                let radius = f64::from(0.5f32 * *size as f32);
                let count = uniform_int(random, 3, 8)?;
                let mut balls = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let x = triangle(random, radius);
                    let z = triangle(random, radius);
                    let w = uniform_double(random, min_size, max_size);
                    balls.push([x, z, w]);
                }
                Ok(Vein::Disc { pos, balls })
            }
            Shape::Pipe {
                height,
                sign,
                min_skew,
                max_skew,
                min_slant,
                max_slant,
                ..
            } => {
                let angle = random.next_float() * std::f32::consts::PI * 2.0;
                let pos = self.default_pos(chunk_x, chunk_z, *height, random)?;
                let s = if *sign < random.next_float() { 1 } else { -1 };
                let skew_x = trig::cos(f64::from(angle));
                let skew_z = trig::sin(f64::from(angle));
                let skew = uniform_int(random, *min_skew, 1 + *max_skew)?;
                let slant = uniform_int(random, *min_slant, 1 + *max_slant)?;
                Ok(Vein::Pipe {
                    pos,
                    sign: s,
                    skew_x,
                    skew_z,
                    skew,
                    slant,
                })
            }
        }
    }

    fn chance(&self, x: i32, y: i32, z: i32, vein: &Vein) -> f32 {
        let density = self.common.density;
        match (&self.shape, vein) {
            (Shape::Cluster { .. }, Vein::Cluster { balls, .. }) => {
                let (fx, fy, fz) = (f64::from(x), f64::from(y), f64::from(z));
                let mut f = 0.0f64;
                for b in balls {
                    f += b[3] * b[3].abs()
                        / ((fx - b[0]) * (fx - b[0])
                            + (fy - b[1]) * (fy - b[1])
                            + (fz - b[2]) * (fz - b[2]));
                    if f > 1.0 {
                        return density;
                    }
                }
                0.0
            }
            (Shape::Disc { height, .. }, Vein::Disc { balls, .. }) => {
                let (fx, fz) = (f64::from(x), f64::from(z));
                let mut f = 0.0f64;
                for b in balls {
                    f +=
                        b[2] * b[2].abs() / ((fx - b[0]) * (fx - b[0]) + (fz - b[1]) * (fz - b[1]));
                }
                let sample = f as f32;
                if self.common.kaolin.is_some() {
                    // `KaolinDiscVeinFeature.getChanceToGenerate`: no fade with height.
                    return if y.abs() <= *height && sample > 1.0 {
                        density * clamped_map_f(sample, 2.0, 1.0, 1.0, 0.6)
                    } else {
                        0.0
                    };
                }
                if y.abs() <= *height && sample > 1.0 {
                    let h = *height as f32;
                    density
                        * clamped_map_f(y.abs() as f32, 0.7f32 * h, h, 1.0, 0.4)
                        * clamped_map_f(sample, 2.0, 1.0, 1.0, 0.6)
                } else {
                    0.0
                }
            }
            (
                Shape::Pipe {
                    height,
                    radius,
                    edge,
                    ..
                },
                Vein::Pipe {
                    sign,
                    skew_x,
                    skew_z,
                    skew,
                    slant,
                    ..
                },
            ) => {
                let edge_noise = edge.noise(
                    0.25 * f64::from(x),
                    0.05 * f64::from(y),
                    0.25 * f64::from(z),
                );
                let y_scaled = f64::from(y) / f64::from(*height);
                let x = (f64::from(x) + f64::from(*skew as f32 * *skew_x) * y_scaled) as i32;
                let z = (f64::from(z) + f64::from(*skew as f32 * *skew_z) * y_scaled) as i32;
                let y_factor = f64::from(*sign) * y_scaled + 0.5;
                let abs_y = y.abs();
                if abs_y < *height {
                    let r = f64::from(*radius);
                    let mut true_radius =
                        r * (1.0 - y_factor) + f64::from(*radius - *slant) * y_factor + edge_noise;
                    let taper_start = *height - 3 * *radius;
                    if abs_y > taper_start {
                        true_radius = map_d(
                            f64::from(abs_y),
                            f64::from(taper_start),
                            f64::from(*height),
                            true_radius,
                            -r,
                        );
                    }
                    if f64::from(x.wrapping_mul(x).wrapping_add(z.wrapping_mul(z)))
                        < true_radius * true_radius
                    {
                        return density;
                    }
                }
                0.0
            }
            _ => 0.0,
        }
    }

    /// Bounding box around the vein, relative to its position: (min, max) per axis.
    fn bounding_box(&self, vein: &Vein) -> ([i32; 3], [i32; 3]) {
        match (&self.shape, vein) {
            (Shape::Cluster { size }, _) => ([-size; 3], [*size; 3]),
            (Shape::Disc { size, height }, _) => ([-size, -height, -size], [*size, *height, *size]),
            (Shape::Pipe { height, radius, .. }, Vein::Pipe { skew, .. }) => {
                let e = radius + skew;
                ([-e, -height, -e], [e, *height, e])
            }
            _ => ([0; 3], [0; 3]),
        }
    }

    fn near_lava<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        x: i32,
        z: i32,
    ) -> R<bool, W> {
        let Some(lava) = self.common.lava else {
            return Ok(false);
        };
        for lx in x - 4..=x + 4 {
            for lz in z - 4..=z + 4 {
                let s = world
                    .block([lx, -55, lz])
                    .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))?;
                if s == lava {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let world_seed = world.world_seed().map_err(FeatureExecutionError::Query)?;
        let chunk_x = origin[0] >> 4;
        let chunk_z = origin[2] >> 4;
        let radius = self.chunk_radius();
        let mut veins = Vec::new();
        for cx in chunk_x - radius..=chunk_x + radius {
            for cz in chunk_z - radius..=chunk_z + radius {
                let lo = world_seed ^ i64::from(cx).wrapping_mul(61_728_364_132);
                let hi = self.common.seed ^ i64::from(cz).wrapping_mul(16_298_364_123);
                let mut vr = XoroshiroRandom::from_state(lo as u64, hi as u64);
                let first = vr
                    .next_int_bound(self.common.rarity)
                    .map_err(FeatureExecutionError::Random)?;
                if first == 0 {
                    let vein = self
                        .create_vein(cx << 4, cz << 4, &mut vr)
                        .map_err(map_unit::<W>)?;
                    veins.push(vein);
                }
            }
        }
        if veins.is_empty() {
            return Ok(false);
        }
        for vein in &veins {
            self.place_vein(world, random, chunk_x << 4, chunk_z << 4, vein)?;
        }
        Ok(true)
    }

    fn place_vein<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        block_x: i32,
        block_z: i32,
        vein: &Vein,
    ) -> R<(), W> {
        let c = &self.common;
        let we = |e| FeatureExecutionError::Basic(FeatureError::World(e));
        let qe = FeatureExecutionError::Query;
        let pos = vein.pos();
        let (bmin, bmax) = self.bounding_box(vein);
        let (off_x, off_z) = if c.project_offset {
            let mut o =
                XoroshiroRandom::new(i64::from(hash(182_739_412_341, pos[0], pos[1], pos[2])));
            let ox = bound::<W>(&mut o, 16)? - bound::<W>(&mut o, 16)?;
            let oz = bound::<W>(&mut o, 16)? - bound::<W>(&mut o, 16)?;
            (ox, oz)
        } else {
            (0, 0)
        };
        let min_x = block_x.max(bmin[0] + pos[0]);
        let max_x = (block_x + 15).min(bmax[0] + pos[0]);
        let min_y = c.min_y.max(bmin[1] + pos[1]);
        let max_y = c.max_y.min(bmax[1] + pos[1]);
        let min_z = block_z.max(bmin[2] + pos[2]);
        let max_z = (block_z + 15).min(bmax[2] + pos[2]);
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                let mut max_vein_y = NO_ORE;
                let projected = if c.project {
                    world
                        .height(HeightmapType::OceanFloorWg, off_x + x, off_z + z)
                        .map_err(qe)?
                } else {
                    0
                };
                if c.near_lava && !self.near_lava(world, x, z)? {
                    continue;
                }
                for y in min_y..=max_y {
                    let chance = self.chance(x - pos[0], y - pos[1], z - pos[2], vein);
                    if random.next_float() < chance {
                        let at = [x, y + projected, z];
                        let stone = world.block(at).map_err(we)?;
                        let ore = if let Some(k) = &c.kaolin {
                            let rel_y = y - pos[1];
                            if k.grass[stone.0 as usize] {
                                Some(k.grass_state)
                            } else if k.replaceable[stone.0 as usize] {
                                Some(if rel_y > -2 {
                                    k.red
                                } else if rel_y > -4 {
                                    k.pink
                                } else {
                                    k.white
                                })
                            } else {
                                None
                            }
                        } else {
                            match c.states.get(&stone) {
                                Some(w) => w.get(random),
                                None => None,
                            }
                        };
                        if let Some(ore) = ore {
                            world.set_block(at, ore, 3).map_err(we)?;
                            max_vein_y = y + projected;
                        }
                    }
                }
                let Some(ind) = &c.indicator else { continue };
                if max_vein_y == NO_ORE {
                    continue;
                }
                if ind.rarity > 0 && bound::<W>(random, ind.rarity)? == 0 {
                    let ix = x + bound::<W>(random, 15)? - bound::<W>(random, 15)?;
                    let iz = z + bound::<W>(random, 15)? - bound::<W>(random, 15)?;
                    let iy = world
                        .height(HeightmapType::OceanFloorWg, ix, iz)
                        .map_err(qe)?;
                    if (i64::from(iy) - i64::from(max_vein_y)).abs() < i64::from(ind.depth) {
                        self.place_indicator(world, random, ind, [ix, iy, iz])?;
                    }
                }
                for _ in 0..ind.underground_count {
                    if ind.underground_rarity == 1
                        || bound::<W>(random, ind.underground_rarity)? == 0
                    {
                        let ix = x + bound::<W>(random, 15)? - bound::<W>(random, 15)?;
                        let span = if max_y > min_y {
                            bound::<W>(random, max_y - min_y)?
                        } else {
                            0
                        };
                        let iy = min_y + span + bound::<W>(random, 32)? - bound::<W>(random, 8)?;
                        let iz = z + bound::<W>(random, 15)? - bound::<W>(random, 15)?;
                        let ground = world
                            .height(HeightmapType::OceanFloorWg, ix, iz)
                            .map_err(qe)?;
                        if iy <= ground - 5 {
                            self.place_indicator(world, random, ind, [ix, iy, iz])?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Indicator placement: the state must be replaceable (air here) and survive.
    fn place_indicator<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        ind: &Indicator,
        at: [i32; 3],
    ) -> R<(), W> {
        let state = ind.states.get(random);
        let here = world
            .block(at)
            .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))?;
        if let Some(state) = state {
            if here == self.common.air
                && world
                    .block_survives(state, at)
                    .map_err(FeatureExecutionError::Query)?
            {
                world
                    .set_block(at, state, 3)
                    .map_err(|e| FeatureExecutionError::Basic(FeatureError::World(e)))?;
            }
        }
        Ok(())
    }
}

fn map_unit<W: FeatureWorld>(e: FeatureExecutionError<()>) -> FeatureExecutionError<W::Error> {
    match e {
        FeatureExecutionError::Random(r) => FeatureExecutionError::Random(r),
        _ => FeatureExecutionError::InvalidState,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected values come from TFC's own `Metaballs2D/3D`, `OpenSimplex3D` and the
    /// `*VeinFeature` shape code compiled with Java 21 (see the oracle in the PR notes).
    const JAVA: [(&str, &str, i32); 12] = [
        ("cluster", "53,-17,-75", -176530006),
        ("disc", "53,-54,-75", 5066615),
        ("pipe", "53,3,-77 sg=1 skew=9 slant=2", 1773733376),
        ("cluster", "-20,13,112", 1052315560),
        ("disc", "-20,-54,122", -618865482),
        ("pipe", "-22,-24,122 sg=1 skew=7 slant=3", 777540856),
        ("cluster", "13,39,5", -499371484),
        ("disc", "13,-54,15", 587711616),
        ("pipe", "15,3,7 sg=1 skew=4 slant=3", 397319576),
        ("cluster", "651,-38,-523", 1859142035),
        ("disc", "651,-54,-527", 1439202230),
        ("pipe", "641,7,-519 sg=1 skew=5 slant=4", -1515781816),
    ];

    fn feature(kind: usize) -> TfcVeinFeature {
        let (shape, min_y, max_y, density) = match kind {
            0 => (Shape::Cluster { size: 18 }, -64, 60, 0.8f32),
            1 => (
                Shape::Disc {
                    size: 18,
                    height: 5,
                },
                -64,
                -45,
                0.6f32,
            ),
            _ => (
                Shape::Pipe {
                    height: 40,
                    radius: 10,
                    min_skew: 3,
                    max_skew: 9,
                    min_slant: 1,
                    max_slant: 4,
                    sign: 0.3,
                    edge: Box::new(OpenSimplex3D::new(15_189_385).octaves(2).scaled(0.0, -9.0)),
                },
                -64,
                100,
                0.98f32,
            ),
        };
        TfcVeinFeature {
            common: Common {
                states: HashMap::new(),
                indicator: None,
                rarity: 1,
                density,
                min_y,
                max_y,
                project: false,
                project_offset: false,
                seed: 987_654_321,
                near_lava: false,
                air: BlockStateId(0),
                lava: None,
                kaolin: None,
            },
            shape,
        }
    }

    #[test]
    fn vein_shapes_match_java_bit_for_bit() {
        let world_seed = 123_456_789i64;
        let chunks = [[3, -5], [-2, 7], [0, 0], [40, -33]];
        let mut row = 0;
        for [cx, cz] in chunks {
            for kind in 0..3 {
                let f = feature(kind);
                let lo = world_seed ^ i64::from(cx).wrapping_mul(61_728_364_132);
                let hi = f.common.seed ^ i64::from(cz).wrapping_mul(16_298_364_123);
                let mut r = XoroshiroRandom::from_state(lo as u64, hi as u64);
                assert_eq!(r.next_int_bound(1).unwrap(), 0);
                let vein = f.create_vein(cx << 4, cz << 4, &mut r).ok().unwrap();
                let mut h = 0i32;
                let mut mix = |c: f32| h = h.wrapping_mul(31).wrapping_add(c.to_bits() as i32);
                match kind {
                    0 => {
                        for x in -24..24 {
                            for y in -24..24 {
                                for z in -24..24 {
                                    mix(f.chance(x, y, z, &vein));
                                }
                            }
                        }
                    }
                    1 => {
                        for x in -24..24 {
                            for y in -6..=6 {
                                for z in -24..24 {
                                    mix(f.chance(x, y, z, &vein));
                                }
                            }
                        }
                    }
                    _ => {
                        for x in -30..30 {
                            for y in -44..44 {
                                for z in -30..30 {
                                    mix(f.chance(x, y, z, &vein));
                                }
                            }
                        }
                    }
                }
                let p = vein.pos();
                let desc = match &vein {
                    Vein::Pipe {
                        sign, skew, slant, ..
                    } => {
                        format!(
                            "{},{},{} sg={sign} skew={skew} slant={slant}",
                            p[0], p[1], p[2]
                        )
                    }
                    _ => format!("{},{},{}", p[0], p[1], p[2]),
                };
                let (name, want_desc, want_h) = JAVA[row];
                assert_eq!(
                    (["cluster", "disc", "pipe"][kind], desc.as_str(), h),
                    (name, want_desc, want_h),
                    "row {row}"
                );
                row += 1;
            }
        }
    }
    #[test]
    fn kaolin_discs_pick_the_clay_by_depth_and_turn_grass_to_kaolin_grass() {
        use crate::{registry::BlockRegistry, worldgen::tfc_tide_pool::tests::Flat};
        use serde_json::json;
        let names = [
            "minecraft:air",
            "minecraft:stone",
            "minecraft:grass_block",
            "tfc:kaolin_clay_grass",
            "tfc:red_kaolin_clay",
            "tfc:pink_kaolin_clay",
            "tfc:white_kaolin_clay",
            "minecraft:bedrock",
        ];
        let rows: Vec<_> = names
            .iter()
            .map(|n| json!([n, n.ends_with("air"), false, "", {"id": n, "properties": {}}]))
            .collect();
        let defaults: std::collections::BTreeMap<_, _> =
            names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let facts = json!({"states": rows, "defaults": defaults});
        let registry = BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap();
        let id = |n: &str| registry.default_state(n).unwrap();
        let mut feature = feature(1);
        feature.common.min_y = -8;
        feature.common.max_y = 2;
        feature.common.density = 1.0;
        feature.common.air = id("minecraft:air");
        feature.common.kaolin = Some(std::sync::Arc::new(Kaolin {
            grass: names
                .iter()
                .map(|n| *n == "minecraft:grass_block")
                .collect(),
            replaceable: names.iter().map(|n| *n == "minecraft:stone").collect(),
            grass_state: id("tfc:kaolin_clay_grass"),
            red: id("tfc:red_kaolin_clay"),
            pink: id("tfc:pink_kaolin_clay"),
            white: id("tfc:white_kaolin_clay"),
        }));
        let mut blocks = HashMap::new();
        blocks.insert([0, 1, 0], id("minecraft:grass_block"));
        blocks.insert([0, -6, 0], id("minecraft:bedrock"));
        let mut world = Flat {
            blocks,
            surface: 1,
            rock: id("minecraft:stone"),
            air: id("minecraft:air"),
        };
        let vein = Vein::Disc {
            pos: [0, -1, 0],
            balls: vec![[0.0, 0.0, 6.0]],
        };
        feature
            .place_vein(&mut world, &mut XoroshiroRandom::new(5), 0, 0, &vein)
            .unwrap();
        let at = |y: i32| world.blocks.get(&[0, y, 0]).copied().unwrap_or(world.rock);
        // Relative to the vein's y of -1: > -2 is red, > -4 pink, below that white.
        assert_eq!(at(1), id("tfc:kaolin_clay_grass"));
        assert_eq!(at(0), id("tfc:red_kaolin_clay"));
        assert_eq!(at(-2), id("tfc:red_kaolin_clay"));
        assert_eq!(at(-3), id("tfc:pink_kaolin_clay"));
        assert_eq!(at(-4), id("tfc:pink_kaolin_clay"));
        assert_eq!(at(-5), id("tfc:white_kaolin_clay"));
        assert_eq!(
            at(-6),
            id("minecraft:bedrock"),
            "not in the replaceable set"
        );
    }
}
