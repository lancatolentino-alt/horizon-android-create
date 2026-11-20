//! TerraFirmaCraft's per-chunk data generator (`RegionChunkDataGenerator`, `LerpFloatLayer`,
//! `ChunkRockDataCache`; EUPL 1.2): climate corners, river groundwater, forest type and the
//! skewed rock layers under every column.

use super::tfc_biome_layers::create_overworld_rock_layer;
use super::tfc_forest::{ForestType, create_overworld_forest_layer};
use super::tfc_layers::{Area, Rock};
use super::tfc_noise::OpenSimplex2D;
use super::tfc_region::{Region, RegionGenerator, clamped_map_f, lerp_f, units};
use horizon_core::random::{RandomSource, XoroshiroRandom};
use std::sync::{Arc, Mutex};

const LAYER_OFFSET_BITS: i32 = 3;
const LAYER_OFFSET_MASK: i32 = (1 << LAYER_OFFSET_BITS) - 1;
const DELTA_Y_OFFSET: f32 = 12.0;
const MIN_RIVER_WIDTH: i32 = 12;
/// `(float) Units.blockToGridExact(40)`.
const RIVER_INFLUENCE: f32 = 40.0 / 128.0;
const RIVER_INFLUENCE_SQ: f32 = RIVER_INFLUENCE * RIVER_INFLUENCE;

fn layer_offsets() -> &'static [i32; 1 << (LAYER_OFFSET_BITS + 1)] {
    static OFFSETS: std::sync::OnceLock<[i32; 16]> = std::sync::OnceLock::new();
    OFFSETS.get_or_init(|| {
        let mut random = XoroshiroRandom::new(1_923_874_192_341);
        let mut out = [0; 16];
        for slot in &mut out {
            *slot = random.next_int_bound(100_000).unwrap();
        }
        out
    })
}

fn offset_x(layer: i32) -> i32 {
    layer_offsets()[((layer & LAYER_OFFSET_MASK) << 1) as usize]
}

fn offset_z(layer: i32) -> i32 {
    layer_offsets()[(((layer & LAYER_OFFSET_MASK) << 1) | 1) as usize]
}

fn chunk_index(x: i32, z: i32) -> usize {
    ((x & 15) | ((z & 15) << 4)) as usize
}

/// `Helpers.lerp4`.
fn lerp4(v00: f32, v01: f32, v10: f32, v11: f32, delta0: f32, delta1: f32) -> f32 {
    let value0 = lerp_f(delta1, v00, v01);
    let value1 = lerp_f(delta1, v10, v11);
    lerp_f(delta0, value0, value1)
}

/// `LerpFloatLayer`: four corner values interpolated bilinearly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LerpFloatLayer {
    pub value00: f32,
    pub value01: f32,
    pub value10: f32,
    pub value11: f32,
}

impl LerpFloatLayer {
    pub fn new(value00: f32, value01: f32, value10: f32, value11: f32) -> Self {
        Self {
            value00,
            value01,
            value10,
            value11,
        }
    }

    pub fn get_value(&self, delta_x: f32, delta_z: f32) -> f32 {
        lerp4(
            self.value00,
            self.value01,
            self.value10,
            self.value11,
            delta_x,
            delta_z,
        )
    }

    pub fn scaled(&self, origin_x: f32, origin_z: f32, width: f32) -> Self {
        Self::new(
            self.get_value(origin_x, origin_z),
            self.get_value(origin_x, origin_z + width),
            self.get_value(origin_x + width, origin_z),
            self.get_value(origin_x + width, origin_z + width),
        )
    }

    pub fn apply(&self, f: impl Fn(f32) -> f32) -> Self {
        Self::new(
            f(self.value00),
            f(self.value01),
            f(self.value10),
            f(self.value11),
        )
    }
}

/// What `RegionChunkDataGenerator.generate` stores for a chunk.
#[derive(Clone, Debug)]
pub struct ChunkClimate {
    pub rainfall: LerpFloatLayer,
    pub rainfall_variance: LerpFloatLayer,
    pub base_groundwater: LerpFloatLayer,
    pub temperature: LerpFloatLayer,
    pub forest_type: ForestType,
}

/// `ChunkRockDataCache`: lazily filled layer heights and skews for one chunk.
#[derive(Default)]
pub struct ChunkRockDataCache {
    min_block_x: i32,
    min_block_z: i32,
    layer_height: Vec<[f32; 256]>,
    layer_skew: Vec<[f32; 512]>,
}

impl ChunkRockDataCache {
    pub fn new(chunk_x: i32, chunk_z: i32) -> Self {
        Self {
            min_block_x: chunk_x << 4,
            min_block_z: chunk_z << 4,
            ..Self::default()
        }
    }

    fn layers(&self) -> i32 {
        self.layer_height.len() as i32
    }
}

pub struct RegionChunkDataGenerator {
    region_generator: Arc<RegionGenerator>,
    rock_layer_area: Mutex<Area>,
    forest_type_layer: Mutex<Area>,
    layer_height_noise: OpenSimplex2D,
    layer_skew_x_noise: OpenSimplex2D,
    layer_skew_z_noise: OpenSimplex2D,
}

impl RegionChunkDataGenerator {
    /// Builds the generator from the five seeds `RegionGenerator::from_seed` recorded.
    pub fn new(region_generator: Arc<RegionGenerator>) -> Self {
        let seeds = region_generator.chunk_data_seeds;
        Self {
            rock_layer_area: Mutex::new(create_overworld_rock_layer(
                region_generator.clone(),
                seeds[0],
            )()),
            layer_height_noise: OpenSimplex2D::new(seeds[1])
                .octaves(3)
                .scaled(43.0, 63.0)
                .spread(f64::from(0.014f32)),
            layer_skew_x_noise: OpenSimplex2D::new(seeds[2])
                .octaves(2)
                .scaled(f64::from(-1.8f32), f64::from(1.8f32))
                .spread(f64::from(0.01f32)),
            layer_skew_z_noise: OpenSimplex2D::new(seeds[3])
                .octaves(2)
                .scaled(f64::from(-1.8f32), f64::from(1.8f32))
                .spread(f64::from(0.01f32)),
            forest_type_layer: Mutex::new(create_overworld_forest_layer(seeds[4])()),
            region_generator,
        }
    }

    /// Regions' river edges whose partition cells cover `(grid_x, grid_z)`: the same edges
    /// `RegionGenerator.getOrCreatePartitionPoint(...).rivers()` lists, in a different order.
    fn rivers_near(&self, grid_x: i32, grid_z: i32) -> Vec<(Arc<Region>, usize)> {
        let cell_x = units::grid_to_cell(grid_x);
        let cell_z = units::grid_to_cell(grid_z);
        let part_x = units::grid_to_part(grid_x);
        let part_z = units::grid_to_part(grid_z);
        let mut out = Vec::new();
        for dx in -1..=1 {
            for dz in -1..=1 {
                let region = self.region_generator.get_or_create_region(
                    units::cell_to_grid(cell_x + dx),
                    units::cell_to_grid(cell_z + dz),
                );
                for (i, edge) in region.rivers.iter().flatten().enumerate() {
                    if edge.min_part_x <= part_x
                        && part_x <= edge.max_part_x
                        && edge.min_part_z <= part_z
                        && part_z <= edge.max_part_z
                    {
                        out.push((region.clone(), i));
                    }
                }
            }
        }
        out
    }

    /// `RegionChunkDataGenerator.generate` for the chunk whose minimum block is `(block_x, block_z)`.
    pub fn generate(&self, block_x: i32, block_z: i32) -> ChunkClimate {
        let g = &self.region_generator;
        let grid_x = units::block_to_grid(block_x);
        let grid_z = units::block_to_grid(block_z);
        let point = |dx: i32, dz: i32| {
            let region = g.get_or_create_region(grid_x + dx, grid_z + dz);
            region
                .at(grid_x + dx, grid_z + dz)
                .expect("region does not contain the grid point")
                .clone()
        };
        let (p00, p01, p10, p11) = (point(0, 0), point(0, 1), point(1, 0), point(1, 1));
        let rainfall_grid =
            LerpFloatLayer::new(p00.rainfall, p01.rainfall, p10.rainfall, p11.rainfall);
        let variance_grid = LerpFloatLayer::new(
            p00.rainfall_variance,
            p01.rainfall_variance,
            p10.rainfall_variance,
            p11.rainfall_variance,
        );
        let temperature_grid = LerpFloatLayer::new(
            p00.temperature,
            p01.temperature,
            p10.temperature,
            p11.temperature,
        );

        let exact_grid_x = f64::from(block_x) / 128.0;
        let exact_grid_z = f64::from(block_z) / 128.0;
        let delta_x = (exact_grid_x - f64::from(grid_x)) as f32;
        let delta_z = (exact_grid_z - f64::from(grid_z)) as f32;
        let d_g = (16.0f64 / 128.0) as f32;

        let rainfall = rainfall_grid.scaled(delta_x, delta_z, d_g);
        let rainfall_variance = variance_grid.scaled(delta_x, delta_z, d_g);
        let temperature = temperature_grid.scaled(delta_x, delta_z, d_g);

        let (mut gw00, mut gw01, mut gw10, mut gw11) = (0f32, 0f32, 0f32, 0f32);
        for (region, i) in self.rivers_near(grid_x, grid_z) {
            let edge = &region.rivers.as_ref().unwrap()[i];
            if edge.width >= MIN_RIVER_WIDTH
                && edge.fractal.maybe_intersect(
                    exact_grid_x,
                    exact_grid_z,
                    f64::from(RIVER_INFLUENCE),
                )
            {
                let width_influence = {
                    // Mth.map(edge.width, MIN_RIVER_WIDTH, MAX_WIDTH, 0, 1)
                    let t = (edge.width as f32 - 12.0) / (24.0 - 12.0);
                    0.0 + t * (1.0 - 0.0)
                };
                let adjust = |current: f32, x: f64, z: f64| {
                    let distance = edge.fractal.intersect_distance(x, z) as f32;
                    let distance_influence =
                        clamped_map_f(distance, 0.0, RIVER_INFLUENCE_SQ, 1.0, 0.0);
                    current.max(distance_influence * width_influence * 300.0)
                };
                let dg = f64::from(d_g);
                gw00 = adjust(gw00, exact_grid_x, exact_grid_z);
                gw01 = adjust(gw01, exact_grid_x, exact_grid_z + dg);
                gw10 = adjust(gw10, exact_grid_x + dg, exact_grid_z);
                gw11 = adjust(gw11, exact_grid_x + dg, exact_grid_z + dg);
            }
        }
        let clamp = |v: f32| if v < 0.0 { 0.0 } else { v.min(500.0) };
        let base_groundwater = LerpFloatLayer::new(gw00, gw01, gw10, gw11).apply(clamp);
        let forest_type = ForestType::from_ordinal(
            self.forest_type_layer
                .lock()
                .unwrap()
                .get(block_x >> 4, block_z >> 4),
        );
        ChunkClimate {
            rainfall,
            rainfall_variance,
            base_groundwater,
            temperature,
            forest_type,
        }
    }

    /// `populateLayerInCache`, including the original's use of `layer` (not the layer being
    /// filled) for the noise offsets.
    fn populate_layer_in_cache(&self, cache: &mut ChunkRockDataCache, layer: i32) {
        if cache.layers() <= layer {
            let (chunk_x, chunk_z) = (cache.min_block_x, cache.min_block_z);
            for _ in cache.layers()..=layer {
                let mut heights = [0f32; 256];
                let mut skews = [0f32; 512];
                let layer_x = chunk_x.wrapping_add(offset_x(layer));
                let layer_z = chunk_z.wrapping_add(offset_z(layer));
                for dx in 0..16 {
                    for dz in 0..16 {
                        let (ox, oz) = (f64::from(layer_x + dx), f64::from(layer_z + dz));
                        let i = chunk_index(dx, dz);
                        heights[i] = self.layer_height_noise.noise(ox, oz) as f32;
                        skews[i << 1] = self.layer_skew_x_noise.noise(ox, oz) as f32;
                        skews[(i << 1) | 1] = self.layer_skew_z_noise.noise(ox, oz) as f32;
                    }
                }
                cache.layer_height.push(heights);
                cache.layer_skew.push(skews);
            }
        }
    }

    /// `RegionChunkDataGenerator.generateRock`: the rock at a block, given the column's surface height.
    pub fn generate_rock(
        &self,
        x: i32,
        y: i32,
        z: i32,
        surface_y: i32,
        mut cache: Option<&mut ChunkRockDataCache>,
    ) -> &Rock {
        let mut adjusted_surface_y = surface_y as f32;
        if adjusted_surface_y > 125.0 {
            adjusted_surface_y = 125.0 + 0.3f32 * (surface_y - 125) as f32;
        }
        let mut layer = 0;
        let mut delta_y = adjusted_surface_y - y as f32;
        let mut layer_height;
        loop {
            if let Some(cache) = cache.as_deref_mut() {
                self.populate_layer_in_cache(cache, layer);
                layer_height = cache.layer_height[layer as usize][chunk_index(x, z)];
            } else {
                let layer_x = x.wrapping_add(offset_x(layer));
                let layer_z = z.wrapping_add(offset_z(layer));
                layer_height =
                    self.layer_height_noise
                        .noise(f64::from(layer_x), f64::from(layer_z)) as f32;
            }
            if delta_y <= layer_height {
                break;
            }
            delta_y -= layer_height;
            layer += 1;
            if delta_y <= 0.0 {
                break;
            }
        }
        let (skew_noise_x, skew_noise_z) = if let Some(cache) = cache.as_deref() {
            let i = chunk_index(x, z) << 1;
            (
                cache.layer_skew[layer as usize][i],
                cache.layer_skew[layer as usize][i | 1],
            )
        } else {
            let ox = f64::from(x.wrapping_add(offset_x(layer)));
            let oz = f64::from(z.wrapping_add(offset_z(layer)));
            (
                self.layer_skew_x_noise.noise(ox, oz) as f32,
                self.layer_skew_z_noise.noise(ox, oz) as f32,
            )
        };
        let skew_x = x.wrapping_add((skew_noise_x * (delta_y + DELTA_Y_OFFSET)) as i32);
        let skew_z = z.wrapping_add((skew_noise_z * (delta_y + DELTA_Y_OFFSET)) as i32);
        let point = self.rock_layer_area.lock().unwrap().get(skew_x, skew_z);
        self.region_generator
            .rock_layers
            .sample_at_layer(point, layer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::tfc_region::{Seed, Settings};

    fn java_string_hash(s: &str) -> i32 {
        s.encode_utf16()
            .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
    }

    #[allow(clippy::type_complexity)]
    const CHUNKS: &[(bool, i64, i32, i32, i64, i64, i64)] = &[
        (
            false,
            1234567,
            0,
            0,
            -6415651988253165661,
            4767721271912173580,
            -7260890063175057722,
        ),
        (
            false,
            1234567,
            500,
            -700,
            1118004782217016123,
            -5089266271215157145,
            8288552271642303646,
        ),
        (
            false,
            1234567,
            -1500,
            2000,
            2131737397053100451,
            -6337971418169188238,
            -1178403375198090793,
        ),
        (
            false,
            1234567,
            3000,
            3100,
            -3286054337241944926,
            -3182692471052103558,
            -5365238889417780740,
        ),
        (
            false,
            1234567,
            -4000,
            -2500,
            1852905039310247075,
            6074399455432246904,
            -5211326751462526272,
        ),
        (
            false,
            1234567,
            7000,
            900,
            4362896792258024957,
            2476943502208407700,
            2652446145843684775,
        ),
        (
            true,
            424242,
            0,
            0,
            2104853443775615342,
            8032093862394621504,
            -3650220583129667093,
        ),
        (
            true,
            424242,
            500,
            -700,
            4876658385246128780,
            -2659853151378481226,
            5640492055756572117,
        ),
        (
            true,
            424242,
            -1500,
            2000,
            8818354771575137862,
            -751764632019908848,
            -8464869783601168504,
        ),
        (
            true,
            424242,
            3000,
            3100,
            -5864583757071528419,
            -33887651124844840,
            3529274240552443864,
        ),
        (
            true,
            424242,
            -4000,
            -2500,
            -2888225778130975837,
            2229466183412025776,
            5847686824582766012,
        ),
        (
            true,
            424242,
            7000,
            900,
            -8702836975145503772,
            -6527909263540966757,
            5193586154362846016,
        ),
    ];

    #[test]
    fn chunk_climate_and_rock_layers_match_java_bit_for_bit() {
        for finite in [false, true] {
            let level_seed = if finite { 424242 } else { 1234567 };
            let settings = Settings {
                finite_continents: finite,
                ..Settings::default()
            };
            let generator = Arc::new(RegionGenerator::from_seed(
                settings,
                &mut Seed::of(level_seed),
            ));
            let chunks = RegionChunkDataGenerator::new(generator);
            for (_, ls, gx, gz, want_climate, want_cached, want_plain) in
                CHUNKS.iter().filter(|r| r.0 == finite)
            {
                assert_eq!(*ls, level_seed);
                let (mut hc, mut hr, mut hn) = (0i64, 0i64, 0i64);
                for dz in 0..10 {
                    for dx in 0..10 {
                        let (cx, cz) = (gx * 8 + dx, gz * 8 + dz);
                        let c = chunks.generate(cx * 16, cz * 16);
                        for layer in [
                            c.rainfall,
                            c.rainfall_variance,
                            c.base_groundwater,
                            c.temperature,
                        ] {
                            for v in [layer.value00, layer.value01, layer.value10, layer.value11] {
                                hc = hc
                                    .wrapping_mul(31)
                                    .wrapping_add(i64::from(v.to_bits() as i32));
                            }
                        }
                        let mut cache = ChunkRockDataCache::new(cx, cz);
                        for sy in [70, 100, 160, 230] {
                            for y in [-20, 30, 64, 120] {
                                let (x, z) = (cx * 16 + 5, cz * 16 + 9);
                                let cached =
                                    chunks.generate_rock(x, y, z, sy, Some(&mut cache)).name;
                                hr = hr
                                    .wrapping_mul(31)
                                    .wrapping_add(i64::from(java_string_hash(cached)));
                                let plain = chunks.generate_rock(x + 40, y, z - 17, sy, None).name;
                                hn = hn
                                    .wrapping_mul(31)
                                    .wrapping_add(i64::from(java_string_hash(plain)));
                            }
                        }
                    }
                }
                assert_eq!(
                    hc, *want_climate,
                    "chunk climate at {gx},{gz} finite={finite}"
                );
                assert_eq!(hr, *want_cached, "cached rock at {gx},{gz} finite={finite}");
                assert_eq!(
                    hn, *want_plain,
                    "uncached rock at {gx},{gz} finite={finite}"
                );
            }
        }
    }
}
