//! A playable TerraFirmaCraft overworld built on the exact region, biome, rock and chunk-data
//! ports: continents, biome ids, climate and rock layers come from the TFC algorithms; the
//! terrain comes from the exact biome height noises, `ChunkHeightFiller` blending and the
//! per-block density of `ChunkNoiseFiller` (without vanilla's noise caves and aquifer pockets).
//!
//! Nothing here depends on Horizon's block registry: it answers heights, TFC biome ids, vanilla
//! stand-in biome names and rock names, and `generator.rs` turns them into blocks.

use super::tfc_aquifer::{Fluid, TfcAquifer};
use super::tfc_biome::BIOME_NAMES;
use super::tfc_biome_layers::create_region_biome_layer;
use super::tfc_caves::{CaveInterpolator, CaveNoise, with_caves};
use super::tfc_chunk_data::{ChunkClimate, ChunkRockDataCache, RegionChunkDataGenerator};
use super::tfc_height::{ChunkHeightFiller, TfcWorld};
use super::tfc_region::{RegionGenerator, Seed, Settings};
use super::tfc_surface::{
    BuildInfo, Builder, Cell, Column, Noises, SurfaceCtx, build, sample_slope, slope_map,
};
use super::tfc_volcano_strato::SurfaceArgs;
use std::sync::Arc;

pub const MIN_Y: i32 = -64;
pub const SEA_LEVEL: i32 = 63;

/// What a chunk needs to be built.
pub struct ChunkPlan {
    /// Surface height of every column, indexed `x | z << 4`.
    pub heights: [i32; 256],
    /// TFC biome id of every 4x4 quart column, indexed `qx | qz << 2`.
    pub biomes: [i32; 16],
    pub climate: ChunkClimate,
    /// Exact biome of every column (rivers included), indexed `x | z << 4`.
    pub column_biomes: [i32; 256],
    /// `Flow` ordinal of the river water in every column (`tfc_flow::NONE` outside rivers).
    pub river_flow: [u8; 256],
    /// The same before rivers replaced it.
    pub column_biomes_no_river: [i32; 256],
    /// Surface slope of every column for the surface builders, indexed `x | z << 4`.
    pub slopes: [f64; 256],
    /// Terrain of every column from `MIN_Y` up to the highest filled block: 0 air, 1 rock, 2 water, 3 lava.
    pub cells: Vec<Vec<u8>>,
    /// Height of every column before volcanoes raised it, indexed `x | z << 4`.
    pub pre_volcanic: [i32; 256],
    /// The biome at the centre of the stratovolcano cell under the chunk's middle.
    pub strato_biome: i32,
    /// Cinder cone, tuff ring, tuya and atoll centre biomes under the chunk's middle.
    pub volcano_biomes: [i32; 4],
    /// The weight of each column's biome in the blend.
    pub biome_weight: [f64; 256],
    /// Whether each column's water is salt (`ChunkBaseBlockSource.useAccurateBiome`), indexed
    /// `x | z << 4`.
    pub salty: [bool; 256],
}

/// The terrain of the running TFC world, for features that need TFC's chunk data (`ChunkData.get`).
static ACTIVE: std::sync::Mutex<Option<(i64, Arc<TfcTerrain>)>> = std::sync::Mutex::new(None);

/// Records the terrain of the world being generated.
pub fn set_active(seed: i64, terrain: Arc<TfcTerrain>) {
    if let Ok(mut active) = ACTIVE.lock() {
        *active = Some((seed, terrain));
    }
}

/// The terrain recorded for `seed`, if that world is the active one.
pub fn active_for(seed: i64) -> Option<Arc<TfcTerrain>> {
    let active = ACTIVE.lock().ok()?;
    match active.as_ref() {
        Some((s, terrain)) if *s == seed => Some(terrain.clone()),
        _ => None,
    }
}

/// The terrain of the running TFC world, whatever its seed.
pub fn active() -> Option<Arc<TfcTerrain>> {
    ACTIVE.lock().ok()?.as_ref().map(|(_, t)| t.clone())
}

pub struct TfcTerrain {
    /// The last few chunks' climate, for `average_climate`.
    climate_cache: std::sync::Mutex<Vec<((i32, i32), Arc<ChunkClimate>)>>,
    world: TfcWorld,
    chunks: RegionChunkDataGenerator,
    noises: Noises,
    caves: CaveNoise,
    tide: super::tfc_noise::Noise2D,
    /// Each planned chunk's aquifer surface heights, so the carvers use the same ones as the fill.
    aquifer_heights: std::sync::Mutex<std::collections::HashMap<(i32, i32), [i32; 16]>>,
    /// The last few chunks' air carving cells (`air_carving_cells`).
    carving_cache: std::sync::Mutex<Vec<((i32, i32), Arc<Vec<[i32; 3]>>)>>,
    /// The last few chunks' surface heights (`surface_height_at`).
    height_cache: std::sync::Mutex<Vec<((i32, i32), Arc<[i32; 256]>)>>,
    level_seed: i64,
}

impl TfcTerrain {
    /// Builds the generators in the order TFC draws its seeds.
    pub fn new(level_seed: i64) -> Self {
        let mut seed = Seed::of(level_seed);
        let region = Arc::new(RegionGenerator::from_seed(Settings::default(), &mut seed));
        let biome_layer = create_region_biome_layer(region.clone(), &mut seed)();
        let chunks = RegionChunkDataGenerator::new(region.clone());
        let noise_seed = seed.next();
        Self {
            climate_cache: std::sync::Mutex::new(Vec::new()),
            world: TfcWorld::new(level_seed, region, biome_layer),
            chunks,
            noises: Noises::new(level_seed),
            caves: CaveNoise::new(noise_seed),
            tide: super::tfc_biome_noise::shoreline_tide_level_noise(level_seed),
            aquifer_heights: std::sync::Mutex::default(),
            carving_cache: std::sync::Mutex::default(),
            height_cache: std::sync::Mutex::default(),
            level_seed,
        }
    }

    /// Builds the terrain and records it as the active world's, for features that read chunk data.
    pub fn shared(level_seed: i64) -> Arc<Self> {
        let terrain = Arc::new(Self::new(level_seed));
        set_active(level_seed, terrain.clone());
        terrain
    }

    /// The ported surface builder for a TFC biome, if there is one.
    pub fn surface_builder(biome: i32) -> Option<Builder> {
        Builder::from_name(super::tfc_biome_ext::extensions()[biome as usize].surface)
    }

    /// Runs a surface builder down one column. `rock(y)` names the rock at a height. Returns the
    /// block names placed by the builder, from `min_y` up, `None` where the rock is untouched.
    pub fn build_surface(
        &self,
        plan: &ChunkPlan,
        local: usize,
        block: [i32; 2],
        min_y: i32,
        builder: Option<&Builder>,
        rock: &mut dyn FnMut(i32) -> &'static str,
    ) -> Option<Vec<Option<String>>> {
        let height = plan.heights[local];
        let shape = &plan.cells[local];
        let top = shape
            .iter()
            .rposition(|&c| c != 0)
            .map_or(height.max(SEA_LEVEL), |i| i as i32 + MIN_Y);
        let ocean_floor = shape
            .iter()
            .rposition(|&c| c == 1)
            .map_or(MIN_Y, |i| i as i32 + MIN_Y);
        let cells = (min_y..=top + 1)
            .map(
                |y| match shape.get((y - MIN_Y) as usize).copied().unwrap_or(0) {
                    1 => Cell::Rock,
                    2 => Cell::Water,
                    _ => Cell::Air,
                },
            )
            .collect();
        let mut column = Column { min_y, cells };
        let (fx, fz) = (
            ((block[0] & 15) as f32) / 16.0,
            ((block[1] & 15) as f32) / 16.0,
        );
        let sea_level_rock = rock(SEA_LEVEL);
        let original = plan.column_biomes_no_river[local];
        let original_ext = &super::tfc_biome_ext::extensions()[original as usize];
        let info = BuildInfo {
            ocean_floor,
            original: Self::surface_builder(original),
            original_is_shore: original_ext.shore,
            sandy_river_shores: original_ext.sandy_river_shores,
        };
        let mut ctx = SurfaceCtx {
            lava_sample: None,
            x: block[0],
            y: top + 1,
            z: block[1],
            rock,
            temperature: plan.climate.temperature.get_value(fx, fz),
            base_groundwater: plan.climate.base_groundwater.get_value(fx, fz),
            rainfall: plan.climate.rainfall.get_value(fx, fz),
            rain_variance: plan.climate.rainfall_variance.get_value(fx, fz),
            slope: plan.slopes[local],
            noises: &self.noises,
            sea_level_rock,
            original_biome: original,
            tide: self
                .noises
                .tide
                .noise(f64::from(block[0]), f64::from(block[1])),
            weight: plan.biome_weight[local],
        };
        let end_y = min_y.max((top + 1).min(ocean_floor) - 20);
        // The cinder cone, tuya, tuff ring and atoll builders wrap the stratovolcano one.
        let built_by_outer = super::tfc_volcano_surface::build(
            &mut column,
            &mut ctx,
            top + 1,
            end_y,
            &super::tfc_volcano_surface::Args {
                volcanoes: self.world.volcanoes(),
                biomes: plan.volcano_biomes,
                pre_volcanic: plan.pre_volcanic[local],
                weight: plan.biome_weight[local],
                ocean_floor,
            },
        );
        // `StratovolcanoSurfaceBuilder` wraps every biome's builder.
        let volcano_ext = &super::tfc_biome_ext::extensions()[plan.strato_biome as usize];
        let built_by_volcano = built_by_outer
            || volcano_ext.stratovolcanoes
                && self.world.stratovolcanoes().build_surface(
                    &mut column,
                    &mut ctx,
                    &SurfaceArgs {
                        ocean_floor_y: if top + 1 == SEA_LEVEL {
                            ocean_floor
                        } else {
                            top + 1
                        },
                        pre_volcanic: plan.pre_volcanic[local],
                        base: volcano_ext.feature_base_height,
                        scale: volcano_ext.feature_scale_height,
                    },
                );
        if !built_by_volcano {
            let builder = builder?;
            build(
                builder,
                &mut column,
                &mut ctx,
                &info,
                top + 1,
                min_y.max((top + 1).min(ocean_floor) - 20),
            );
        }
        Some(
            column
                .cells
                .into_iter()
                .map(|c| {
                    if let Cell::Placed(name) = c {
                        Some(name)
                    } else {
                        None
                    }
                })
                .collect(),
        )
    }

    /// The cells TFC marks in a chunk's air carving mask: air or fluid under the column's top
    /// solid block (`ChunkNoiseFiller`), and every cell the cave, deep cave and canyon carvers
    /// reach (`applyCarvers`). Listed in the mask's own order: x, then z, then y, chunk-local
    /// coordinates turned into world ones.
    pub fn air_carving_cells(&self, chunk_x: i32, chunk_z: i32) -> Arc<Vec<[i32; 3]>> {
        {
            let cache = self.carving_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, cells)) = cache.iter().find(|(k, _)| *k == (chunk_x, chunk_z)) {
                return cells.clone();
            }
        }
        let plan = self.plan(chunk_x, chunk_z);
        let mut set = std::collections::BTreeSet::new();
        for (index, column) in plan.cells.iter().enumerate() {
            let (lx, lz) = ((index & 15) as i32, (index >> 4) as i32);
            let Some(top) = column.iter().rposition(|c| *c == 1) else {
                continue;
            };
            for (i, cell) in column[..top].iter().enumerate() {
                if *cell != 1 {
                    set.insert((lx, lz, MIN_Y + i as i32));
                }
            }
        }
        let generation = super::anchor::WorldGenerationContext::new(MIN_Y, 384, SEA_LEVEL);
        let carved = super::carver::generate_positional_carving_mask(
            self.level_seed,
            horizon_core::position::ChunkPos::new(chunk_x, chunk_z),
            generation,
            false,
            |_| Ok::<_, std::convert::Infallible>(super::tfc_carve::carvers()),
        );
        if let Ok(mask) = carved {
            for column in mask.columns() {
                for y in column.min_y..=column.max_y {
                    set.insert((column.x, column.z, y));
                }
            }
        }
        let cells: Arc<Vec<[i32; 3]>> = Arc::new(
            set.into_iter()
                .map(|(x, z, y)| [(chunk_x << 4) + x, y, (chunk_z << 4) + z])
                .collect(),
        );
        let mut cache = self.carving_cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= 8 {
            cache.remove(0);
        }
        cache.push(((chunk_x, chunk_z), cells.clone()));
        cells
    }

    /// The terrain's surface height at a block column (`RockData.surfaceHeight`).
    pub fn surface_height_at(&self, x: i32, z: i32) -> i32 {
        let key = (x >> 4, z >> 4);
        let index = ((x & 15) | ((z & 15) << 4)) as usize;
        {
            let cache = self.height_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, heights)) = cache.iter().find(|(k, _)| *k == key) {
                return heights[index];
            }
        }
        let heights = Arc::new(self.plan(key.0, key.1).heights);
        let mut cache = self.height_cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= 8 {
            cache.remove(0);
        }
        cache.push((key, heights.clone()));
        heights[index]
    }

    pub fn volcanoes(&self) -> &super::tfc_volcano::Volcanoes {
        self.world.volcanoes()
    }

    /// The TFC biome id at a quart position.
    pub fn biome_at_quart(&self, quart_x: i32, quart_z: i32) -> i32 {
        self.world.biome(quart_x, quart_z)
    }

    /// Biome ids per quart column and the climate of a chunk.
    pub fn biomes(&self, chunk_x: i32, chunk_z: i32) -> ([i32; 16], ChunkClimate) {
        let mut biomes = [0i32; 16];
        for qz in 0..4 {
            for qx in 0..4 {
                biomes[qx | qz << 2] =
                    self.biome_at_quart((chunk_x << 2) + qx as i32, (chunk_z << 2) + qz as i32);
            }
        }
        (biomes, self.chunks.generate(chunk_x << 4, chunk_z << 4))
    }

    /// `(average sea level temperature, average groundwater)` at a block, the inputs of TFC's
    /// climate colour maps without the calendar's swings.
    pub fn average_climate(&self, x: i32, z: i32) -> (f32, f32) {
        let key = (x >> 4, z >> 4);
        let cached = self.climate_cache.lock().ok().and_then(|cache| {
            cache
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, c)| c.clone())
        });
        let climate = cached.unwrap_or_else(|| {
            let climate = Arc::new(self.chunks.generate(key.0 << 4, key.1 << 4));
            if let Ok(mut cache) = self.climate_cache.lock() {
                if cache.len() >= 256 {
                    cache.remove(0);
                }
                cache.push((key, climate.clone()));
            }
            climate
        });
        let (fx, fz) = ((x & 15) as f32 / 16.0, (z & 15) as f32 / 16.0);
        let groundwater = (climate.base_groundwater.get_value(fx, fz)
            + climate.rainfall.get_value(fx, fz))
        .min(500.0);
        (climate.temperature.get_value(fx, fz), groundwater)
    }

    /// Heights, biomes and climate for chunk `(chunk_x, chunk_z)`.
    pub fn plan(&self, chunk_x: i32, chunk_z: i32) -> ChunkPlan {
        let (min_x, min_z) = (chunk_x << 4, chunk_z << 4);
        let mut filler = ChunkHeightFiller::new(&self.world, chunk_x, chunk_z);
        let mut heights = [0i32; 256];
        let mut column_biomes = [0i32; 256];
        let mut column_biomes_no_river = [0i32; 256];
        let mut pre_volcanic = [0i32; 256];
        let mut biome_weight = [0f64; 256];
        let mut salty = [false; 256];
        let mut river_infos: [Option<super::tfc_river_noise::RiverInfo>; 256] = [None; 256];
        let mut cells: Vec<Vec<u8>> = Vec::with_capacity(256);
        let mut caves = CaveInterpolator::new(&self.caves);
        let surface_heights = self.aquifer_surface_heights(&mut filler, chunk_x, chunk_z);
        {
            let mut cache = self.aquifer_heights.lock().unwrap();
            if cache.len() > 4096 {
                cache.clear();
            }
            cache.insert((chunk_x, chunk_z), surface_heights);
        }
        let mut aquifer = TfcAquifer::new(
            chunk_x,
            chunk_z,
            MIN_Y,
            SEA_LEVEL,
            &self.caves,
            surface_heights,
        );
        for z in 0..16usize {
            for x in 0..16usize {
                let info = filler.sample_column(min_x + x as i32, min_z + z as i32);
                heights[x | z << 4] = (info.height as i32).clamp(-60, 310);
                column_biomes[x | z << 4] = info.biome;
                river_infos[x | z << 4] = info.river;
                column_biomes_no_river[x | z << 4] = info.biome_no_river;
                pre_volcanic[x | z << 4] = info.pre_volcanic_height as i32;
                biome_weight[x | z << 4] = info
                    .weights
                    .iter()
                    .find(|(id, _)| *id == info.biome)
                    .map_or(0.0, |(_, w)| *w);
                // `ChunkNoiseFiller.updateLocalCaches`: coastal columns are always salt; elsewhere
                // a salty biome nearby makes a column salt unless it is a river or well inside a
                // land biome.
                let ocean_weight: f64 = info
                    .weights
                    .iter()
                    .filter(|(id, _)| {
                        super::tfc_biome_ext::extensions()[*id as usize].blend
                            == super::tfc_biome_ext::BiomeBlendType::Ocean
                    })
                    .map(|(_, w)| *w)
                    .sum();
                let in_river = info.biome == super::tfc_biome::RIVER;
                let force =
                    !in_river && info.height <= f64::from(SEA_LEVEL) && ocean_weight >= 0.05;
                let own_weight = info
                    .weights
                    .iter()
                    .find(|(id, _)| *id == info.biome)
                    .map_or(0.5, |(_, w)| *w);
                salty[x | z << 4] = force
                    || (info.any_salty_nearby
                        && (super::tfc_biome_ext::extensions()[info.biome as usize].salty
                            || (own_weight <= 0.5 && !in_river)));
                let top = (info.height as i32).clamp(SEA_LEVEL, 319);
                let mut column = vec![0u8; (top - MIN_Y + 1) as usize];
                for y in MIN_Y..=top {
                    let terrain = filler.column_noise(&info, y);
                    let density =
                        with_caves(terrain, &caves.at(min_x + x as i32, y, min_z + z as i32));
                    column[(y - MIN_Y) as usize] = match aquifer.sample_state(
                        min_x + x as i32,
                        y,
                        min_z + z as i32,
                        density,
                    ) {
                        Some(Fluid::Air) => 0,
                        Some(Fluid::Water) => 2,
                        Some(Fluid::Lava) => 3,
                        None => 1,
                    };
                }
                cells.push(column);
            }
        }
        let mut quart = [0i32; 49];
        for qz in 0..7usize {
            for qx in 0..7usize {
                quart[qx + 7 * qz] = if (1..=4).contains(&qx) && (1..=4).contains(&qz) {
                    heights[((qx - 1) << 2) | ((qz - 1) << 2) << 4]
                } else {
                    filler.sample_corner_height(chunk_x, chunk_z, qx, qz)
                };
            }
        }
        // `sampleRiverData`: flow is sampled every quart (5 x 5 including the far edge) and
        // interpolated per column.
        let mut quart_flows = [super::tfc_flow::NONE; 25];
        for qz in 0..5usize {
            for qx in 0..5usize {
                let info = if qx < 4 && qz < 4 {
                    river_infos[(qx << 2) | (qz << 2) << 4]
                } else {
                    filler.sample_river_edge(min_x + (qx << 2) as i32, min_z + (qz << 2) as i32)
                };
                quart_flows[qx + 5 * qz] = match info {
                    Some(info) if info.norm_dist_sq() < 0.28 => info.flow,
                    _ => super::tfc_flow::NONE,
                };
            }
        }
        let mut river_flow = [super::tfc_flow::NONE; 256];
        for z in 0..16usize {
            for x in 0..16usize {
                if !super::tfc_biome_ext::extensions()[column_biomes[x | z << 4] as usize].rivers {
                    continue;
                }
                let (cx, cz) = (x >> 2, z >> 2);
                river_flow[x | z << 4] = super::tfc_flow::lerp4(
                    quart_flows[cx + 5 * cz],
                    quart_flows[cx + 1 + 5 * cz],
                    quart_flows[cx + 5 * (cz + 1)],
                    quart_flows[cx + 1 + 5 * (cz + 1)],
                    (x & 3) as f32 / 4.0,
                    (z & 3) as f32 / 4.0,
                );
            }
        }
        let map = slope_map(&quart);
        let mut slopes = [0.0f64; 256];
        for z in 0..16usize {
            for x in 0..16usize {
                slopes[x | z << 4] = sample_slope(&map, x as i32, z as i32);
            }
        }
        let mut biomes = [0i32; 16];
        for qz in 0..4 {
            for qx in 0..4 {
                biomes[qx | qz << 2] =
                    self.biome_at_quart((chunk_x << 2) + qx as i32, (chunk_z << 2) + qz as i32);
            }
        }
        ChunkPlan {
            heights,
            biomes,
            climate: self.chunks.generate(min_x, min_z),
            column_biomes,
            river_flow,
            column_biomes_no_river,
            cells,
            slopes,
            pre_volcanic,
            strato_biome: self.world.stratovolcano_biome(min_x + 8, min_z + 8),
            volcano_biomes: self.world.volcano_center_biomes(min_x + 8, min_z + 8),
            biome_weight,
            salty,
        }
    }

    /// The name of the rock at a block (`generateRock`), through the chunk's cache.
    pub fn rock(
        &self,
        x: i32,
        y: i32,
        z: i32,
        surface_y: i32,
        cache: &mut ChunkRockDataCache,
    ) -> &'static str {
        self.chunks
            .generate_rock(x, y, z, surface_y, Some(cache))
            .name
    }

    /// `ChunkData.getForestType` at a block position.
    pub fn forest_density(&self, x: i32, z: i32) -> i32 {
        self.chunks.generate(x, z).forest_type.data().density
    }

    /// The climate `ChunkData` reports at a block column: the layers are read at `((x & 15) / 16, (z & 15) / 16)`.
    pub fn climate_at(&self, x: i32, z: i32) -> ClimateSample {
        let chunk = self.chunks.generate(x, z);
        let (dx, dz) = ((x & 15) as f32 / 16.0, (z & 15) as f32 / 16.0);
        let rainfall = chunk.rainfall.get_value(dx, dz);
        ClimateSample {
            rainfall,
            rain_variance: chunk.rainfall_variance.get_value(dx, dz),
            // `getAverageGroundwater`: base groundwater plus rainfall, capped at 500.
            groundwater: (chunk.base_groundwater.get_value(dx, dz) + rainfall).min(500.0),
            sea_level_temperature: chunk.temperature.get_value(dx, dz),
            forest: chunk.forest_type,
        }
    }

    /// The temperature scale the region generator runs with (`hemisphereScale` of the overworld climate model).
    pub fn hemisphere_scale(&self) -> f32 {
        super::tfc_region::Settings::default().temperature_scale as f32
    }

    /// The chunk's aquifer, as the fill built it (`getOrCreateAquifer`), for the carvers.
    pub fn aquifer(&self, chunk_x: i32, chunk_z: i32) -> TfcAquifer<'_> {
        let cached = self
            .aquifer_heights
            .lock()
            .unwrap()
            .get(&(chunk_x, chunk_z))
            .copied();
        let heights = cached.unwrap_or_else(|| {
            let mut filler = ChunkHeightFiller::new(&self.world, chunk_x, chunk_z);
            self.aquifer_surface_heights(&mut filler, chunk_x, chunk_z)
        });
        TfcAquifer::new(chunk_x, chunk_z, MIN_Y, SEA_LEVEL, &self.caves, heights)
    }

    /// `sampleRiverDistSq`: squared block distance to the nearest river edge in the partition.
    fn river_dist_sq(&self, block_x: i32, block_z: i32) -> f64 {
        use super::tfc_region::units::{GRID_WIDTH_IN_BLOCK, block_to_grid_exact};
        let point = self.world.partition_point(block_x, block_z);
        let (x, z) = (
            block_to_grid_exact(f64::from(block_x)),
            block_to_grid_exact(f64::from(block_z)),
        );
        let mut min = f64::from(f32::MAX);
        for edge in &point {
            if edge.fractal.maybe_intersect(x, z, min) {
                let dist = edge.fractal.intersect_distance(x, z);
                if dist < min {
                    min = dist;
                }
            }
        }
        min * f64::from(GRID_WIDTH_IN_BLOCK) * f64::from(GRID_WIDTH_IN_BLOCK)
    }

    /// `ChunkNoiseFiller.sampleAquiferSurfaceHeight`: the 4x4 grid of surface heights the aquifer
    /// uses, each the lowest of a 5x5 neighbourhood sampled every 8 blocks.
    pub fn aquifer_surface_heights(
        &self,
        filler: &mut ChunkHeightFiller<'_>,
        chunk_x: i32,
        chunk_z: i32,
    ) -> [i32; 16] {
        use super::tfc_biome_ext::AquiferHeight;
        let extensions = super::tfc_biome_ext::extensions();
        let sea = f64::from(SEA_LEVEL);
        let mut sampled = [0.0f64; 121];
        for x in 0..11i32 {
            for z in 0..11i32 {
                let bx = (chunk_x << 4) - 32 + (x << 3);
                let bz = (chunk_z << 4) - 32 + (z << 3);
                let biome = self.world.biome_no_river(bx >> 2, bz >> 2);
                let ext = &extensions[biome as usize];
                let mut height = match ext.aquifer {
                    AquiferHeight::SeaMinus16 => sea - 16.0,
                    AquiferHeight::Surface | AquiferHeight::Offset(_) => {
                        let offset = match ext.aquifer {
                            AquiferHeight::Offset(d) => d,
                            _ => 0.0,
                        };
                        match filler.samplers.biomes[biome as usize].as_mut() {
                            Some(sampler) => {
                                sampler.set_column(bx, bz);
                                sampler.height() + offset
                            }
                            None => sea,
                        }
                    }
                };
                if ext.rivers && height > sea - 24.0 && self.river_dist_sq(bx, bz) < 15.0 * 15.0 {
                    height = sea - 24.0;
                }
                if height > sea {
                    height = 0.3 * sea + 0.7 * height;
                }
                sampled[(x + 11 * z) as usize] = height;
            }
        }
        let mut heights = [0i32; 16];
        for x in 0..4i32 {
            for z in 0..4i32 {
                let (xi, zi) = ((1 + x) << 1, (1 + z) << 1);
                let mut min = f64::MAX;
                for dx in -2..=2 {
                    for dz in -2..=2 {
                        min = min.min(sampled[((xi + dx) + 11 * (zi + dz)) as usize]);
                    }
                }
                heights[(x + 4 * z) as usize] = min as i32;
            }
        }
        heights
    }

    /// `BiomeNoise.shoreTideLevelNoise` at a column.
    pub fn tide_level(&self, x: i32, z: i32) -> f64 {
        self.tide.noise(f64::from(x), f64::from(z))
    }

    /// `RockData.getSurfaceRock`: the rock a column has at the top (`generateRock(x, 0, z, 0)`).
    pub fn surface_rock(&self, x: i32, z: i32) -> &'static str {
        self.chunks.generate_rock(x, 0, z, 0, None).name
    }

    pub fn rock_cache(chunk_x: i32, chunk_z: i32) -> ChunkRockDataCache {
        ChunkRockDataCache::new(chunk_x, chunk_z)
    }

    /// Where to start: the nearest dry land to the origin, scanning outward in a square spiral.
    pub fn find_spawn(&self) -> [i32; 2] {
        let step = 64;
        for ring in 0..200i32 {
            for dx in -ring..=ring {
                for dz in -ring..=ring {
                    if dx.abs().max(dz.abs()) != ring {
                        continue;
                    }
                    let (x, z) = (dx * step, dz * step);
                    let id = self.biome_at_quart(x >> 2, z >> 2) as usize;
                    let name = BIOME_NAMES[id];
                    let wet =
                        name.contains("ocean") || name.contains("lake") || name.contains("river");
                    if wet {
                        continue;
                    }
                    let plan = self.plan(x >> 4, z >> 4);
                    let local = ((x & 15) | (z & 15) << 4) as usize;
                    let top = plan.cells[local].last().copied();
                    if plan.heights[local] > SEA_LEVEL + 1 && top == Some(0) {
                        return [x, z];
                    }
                }
            }
        }
        [0, 0]
    }
}

/// The vanilla biome that stands in for a TFC biome, picked by name and local climate.
pub fn vanilla_biome(tfc_id: i32, temperature: f32, rainfall: f32) -> &'static str {
    let name = BIOME_NAMES[tfc_id as usize];
    let cold = temperature < 0.0;
    if name.contains("ocean") {
        let deep = name.contains("deep");
        return match (cold, temperature > 20.0, deep) {
            (true, _, true) => "minecraft:deep_frozen_ocean",
            (true, _, false) => "minecraft:frozen_ocean",
            (_, true, true) => "minecraft:deep_ocean",
            (_, true, false) => "minecraft:warm_ocean",
            (_, _, true) => "minecraft:deep_ocean",
            _ => "minecraft:ocean",
        };
    }
    if name.contains("river") || name.contains("lake") {
        return if cold {
            "minecraft:frozen_river"
        } else {
            "minecraft:river"
        };
    }
    if name.contains("shore")
        || name.contains("tidal")
        || name.contains("embayment")
        || name.contains("coastal")
    {
        return if cold {
            "minecraft:snowy_beach"
        } else {
            "minecraft:beach"
        };
    }
    if name.contains("ice_sheet") || name.contains("glaci") {
        return "minecraft:snowy_plains";
    }
    if name.contains("mountains") || name.contains("volcan") {
        return if temperature < 5.0 {
            "minecraft:frozen_peaks"
        } else {
            "minecraft:windswept_hills"
        };
    }
    if name.contains("desert") || name.contains("dune") || name.contains("salt_flats") {
        return "minecraft:desert";
    }
    if name.contains("badlands")
        || name.contains("canyon")
        || name.contains("mesa")
        || name.contains("butte")
        || name.contains("hoodoo")
        || name.contains("plateau")
    {
        return if rainfall < 150.0 {
            "minecraft:badlands"
        } else {
            "minecraft:wooded_badlands"
        };
    }
    if name.contains("marsh") || name.contains("mud") {
        return "minecraft:swamp";
    }
    if temperature < -8.0 {
        "minecraft:snowy_plains"
    } else if temperature < 4.0 {
        if rainfall > 150.0 {
            "minecraft:snowy_taiga"
        } else {
            "minecraft:snowy_plains"
        }
    } else if temperature < 12.0 {
        "minecraft:taiga"
    } else if rainfall > 330.0 {
        if temperature > 22.0 {
            "minecraft:jungle"
        } else {
            "minecraft:dark_forest"
        }
    } else if rainfall > 190.0 {
        "minecraft:forest"
    } else if rainfall < 90.0 {
        "minecraft:desert"
    } else if temperature > 20.0 && rainfall < 160.0 {
        "minecraft:savanna"
    } else {
        "minecraft:plains"
    }
}

/// The top block layers of a column: (surface block, filler block, filler depth).
pub fn soil(
    tfc_id: i32,
    temperature: f32,
    rainfall: f32,
    height: i32,
) -> (&'static str, &'static str, i32) {
    let name = BIOME_NAMES[tfc_id as usize];
    if name.contains("ocean") || name.contains("river") || name.contains("lake") {
        return if height < SEA_LEVEL - 8 {
            ("minecraft:gravel", "minecraft:gravel", 3)
        } else {
            ("minecraft:sand", "minecraft:sand", 3)
        };
    }
    if name.contains("shore")
        || name.contains("tidal")
        || name.contains("dune")
        || name.contains("salt_flats")
        || name.contains("coastal")
    {
        return ("minecraft:sand", "minecraft:sand", 4);
    }
    if name.contains("mountains") && height > 120 || name.contains("volcan") {
        return ("", "", 0);
    }
    if rainfall < 90.0 || name.contains("badlands") || name.contains("canyon") && rainfall < 150.0 {
        return ("minecraft:sand", "minecraft:sand", 3);
    }
    if temperature < -10.0 {
        return ("minecraft:snow_block", "minecraft:dirt", 3);
    }
    ("minecraft:grass_block", "minecraft:dirt", 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_is_deterministic_and_in_range() {
        let a = TfcTerrain::new(4242);
        let b = TfcTerrain::new(4242);
        for (cx, cz) in [(0, 0), (3, -7), (-40, 25)] {
            let (pa, pb) = (a.plan(cx, cz), b.plan(cx, cz));
            assert_eq!(pa.heights, pb.heights);
            assert_eq!(pa.biomes, pb.biomes);
            assert!(pa.heights.iter().all(|h| (4..=300).contains(h)));
            assert!(
                pa.biomes
                    .iter()
                    .all(|b| (0..BIOME_NAMES.len() as i32).contains(b))
            );
        }
    }

    #[test]
    fn columns_are_rock_below_air_and_water_to_sea_level() {
        let terrain = TfcTerrain::new(4242);
        for (cx, cz) in [(0, 0), (3, -7), (-40, 25), (90, 90)] {
            let plan = terrain.plan(cx, cz);
            for column in plan.cells.iter() {
                assert!(column.len() as i32 > SEA_LEVEL - MIN_Y);
                let sea = (SEA_LEVEL - MIN_Y) as usize;
                // Water only fills below sea level.
                assert!(column.iter().skip(sea).all(|&c| c != 2));
            }
        }
    }

    #[test]
    fn most_of_the_underground_is_solid_with_some_caves() {
        let terrain = TfcTerrain::new(4242);
        let (mut rock, mut open) = (0usize, 0usize);
        for (cx, cz) in [(0, 0), (3, -7), (-40, 25), (90, 90)] {
            for column in terrain.plan(cx, cz).cells {
                for &cell in column.iter().take(60) {
                    if cell == 1 {
                        rock += 1;
                    } else {
                        open += 1;
                    }
                }
            }
        }
        assert!(rock > open * 4, "{rock} rock, {open} open");
        assert!(open > 0, "no caves at all");
    }

    #[test]
    fn every_ocean_water_column_is_salt() {
        let terrain = TfcTerrain::new(4242);
        let mut salt_oceans = 0;
        for cx in (-60..60).step_by(12) {
            for cz in (-60..60).step_by(12) {
                let plan = terrain.plan(cx, cz);
                for i in 0..256 {
                    let name = BIOME_NAMES[plan.column_biomes_no_river[i] as usize];
                    let wet = plan.cells[i].get((SEA_LEVEL - 1 - MIN_Y) as usize) == Some(&2);
                    if name.contains("ocean") && wet {
                        assert!(plan.salty[i], "{name} water must be salt");
                        salt_oceans += 1;
                    }
                }
            }
        }
        assert!(salt_oceans > 0);
    }

    #[test]
    fn rivers_get_a_flow_only_inside_the_channel() {
        let terrain = TfcTerrain::new(4242);
        let river = super::super::tfc_biome::RIVER;
        // Chunks whose centre quart is a river, found without planning anything.
        let mut chunks = Vec::new();
        'search: for cx in -120..120 {
            for cz in -120..120 {
                if terrain.biome_at_quart((cx << 2) + 2, (cz << 2) + 2) == river {
                    chunks.push((cx, cz));
                    if chunks.len() == 6 {
                        break 'search;
                    }
                }
            }
        }
        assert!(!chunks.is_empty(), "no river found");
        let mut flowing = 0;
        for (cx, cz) in chunks {
            let plan = terrain.plan(cx, cz);
            for i in 0..256 {
                let flow = plan.river_flow[i];
                assert!(flow <= super::super::tfc_flow::NONE);
                if flow != super::super::tfc_flow::NONE {
                    assert!(
                        super::super::tfc_biome_ext::extensions()[plan.column_biomes[i] as usize]
                            .rivers
                    );
                    flowing += 1;
                }
            }
        }
        assert!(flowing > 0, "no river flow found");
    }

    #[test]
    fn spawn_is_on_dry_land() {
        let terrain = TfcTerrain::new(1234567);
        let [x, z] = terrain.find_spawn();
        let name = BIOME_NAMES[terrain.biome_at_quart(x >> 2, z >> 2) as usize];
        assert!(!name.contains("ocean"), "spawn in {name}");
    }

    #[test]
    fn rock_comes_from_the_ported_layers() {
        let terrain = TfcTerrain::new(1234567);
        let mut cache = TfcTerrain::rock_cache(0, 0);
        let top = terrain.rock(5, 70, 9, 72, &mut cache);
        assert!(!top.is_empty());
    }

    /// Looks for chunks under a stratovolcano and checks that the volcano's strata and volcanic
    /// gravel appear in the columns it builds.
    #[test]
    fn stratovolcanoes_lay_strata_and_volcanic_soil() {
        let terrain = TfcTerrain::new(5);
        let exts = super::super::tfc_biome_ext::extensions();
        let strata = ["tuff", "basalt", "andesite", "dacite", "rhyolite"];
        let mut volcano_chunks = 0;
        let mut strata_blocks = 0;
        for cz in (-400..400).step_by(16) {
            for cx in (-400..400).step_by(16) {
                let centre = terrain
                    .world
                    .stratovolcano_biome((cx << 4) + 8, (cz << 4) + 8);
                if !exts[centre as usize].stratovolcanoes {
                    continue;
                }
                let plan = terrain.plan(cx, cz);
                volcano_chunks += 1;
                for local in (0..256).step_by(5) {
                    let (x, z) = (
                        (cx << 4) + (local & 15) as i32,
                        (cz << 4) + (local >> 4) as i32,
                    );
                    let builder = TfcTerrain::surface_builder(plan.column_biomes[local]);
                    let mut rock = |_y: i32| "granite";
                    let Some(cells) = terrain.build_surface(
                        &plan,
                        local,
                        [x, z],
                        MIN_Y,
                        builder.as_ref(),
                        &mut rock,
                    ) else {
                        continue;
                    };
                    strata_blocks += cells
                        .iter()
                        .flatten()
                        .filter(|name| strata.iter().any(|r| name.ends_with(&format!("_{r}"))))
                        .count();
                }
                if strata_blocks > 0 {
                    break;
                }
            }
            if strata_blocks > 0 {
                break;
            }
        }
        assert!(volcano_chunks > 0, "no stratovolcano biome found");
        assert!(
            strata_blocks > 0,
            "no strata laid in {volcano_chunks} volcano chunks"
        );
    }

    /// Scans for chunks whose volcano cell biome allows a feature and counts the blocks the
    /// matching surface builder lays.
    fn count_volcano_blocks(
        terrain: &TfcTerrain,
        feature: impl Fn(&super::super::tfc_biome_ext::BiomeExtension) -> bool,
        slot: usize,
        wanted: &str,
    ) -> usize {
        let exts = super::super::tfc_biome_ext::extensions();
        let mut found = 0;
        for cz in (-300..300).step_by(3) {
            for cx in (-300..300).step_by(3) {
                let centre = terrain
                    .world
                    .volcano_center_biomes((cx << 4) + 8, (cz << 4) + 8);
                if !feature(&exts[centre[slot] as usize]) {
                    continue;
                }
                let plan = terrain.plan(cx, cz);
                for local in (0..256).step_by(3) {
                    let (x, z) = (
                        (cx << 4) + (local & 15) as i32,
                        (cz << 4) + (local >> 4) as i32,
                    );
                    let builder = TfcTerrain::surface_builder(plan.column_biomes[local]);
                    let mut rock = |_y: i32| "granite";
                    if let Some(cells) = terrain.build_surface(
                        &plan,
                        local,
                        [x, z],
                        MIN_Y,
                        builder.as_ref(),
                        &mut rock,
                    ) {
                        found += cells
                            .iter()
                            .flatten()
                            .filter(|n| n.as_str() == wanted)
                            .count();
                    }
                }
                if found > 0 {
                    return found;
                }
            }
        }
        found
    }

    #[test]
    fn cinder_cones_are_basalt_and_tuff_rings_are_tuff() {
        let terrain = TfcTerrain::new(5);
        let cinder = count_volcano_blocks(&terrain, |e| e.cinder_cones, 0, "tfc:raw_basalt");
        assert!(cinder > 0, "no basalt laid on a cinder cone");
        let tuff = count_volcano_blocks(&terrain, |e| e.tuff_rings, 1, "tfc:raw_tuff");
        assert!(tuff > 0, "no tuff laid on a tuff ring");
    }
}

/// What a `tfc:climate` placement reads from `ChunkData` at a column.
#[derive(Clone, Copy, Debug)]
pub struct ClimateSample {
    pub rainfall: f32,
    pub rain_variance: f32,
    pub groundwater: f32,
    pub sea_level_temperature: f32,
    pub forest: super::tfc_forest::ForestType,
}

#[cfg(test)]
mod carving_tests {
    use super::*;

    #[test]
    fn air_carving_cells_are_ordered_inside_the_chunk_and_cached() {
        let terrain = TfcTerrain::new(4242);
        let mut total = 0;
        for cx in -3..3 {
            for cz in -3..3 {
                let cells = terrain.air_carving_cells(cx, cz);
                total += cells.len();
                let keys: Vec<_> = cells.iter().map(|c| (c[0], c[2], c[1])).collect();
                assert!(
                    keys.windows(2).all(|w| w[0] < w[1]),
                    "ordered x, z, y without repeats"
                );
                for c in cells.iter() {
                    assert_eq!(c[0] >> 4, cx);
                    assert_eq!(c[2] >> 4, cz);
                    assert!((MIN_Y..MIN_Y + 384).contains(&c[1]));
                }
                // The second call returns the cached list.
                assert!(Arc::ptr_eq(&cells, &terrain.air_carving_cells(cx, cz)));
            }
        }
        assert!(total > 0, "no cave cells in 36 chunks");
    }

    #[test]
    fn carving_mask_modifier_filters_by_height() {
        use crate::worldgen::placement::PlacementModifier;
        let json = r#"{"type":"tfc:carving_mask","step":"air","min_y":{"absolute":0},"max_y":{"absolute":40}}"#;
        assert!(matches!(
            PlacementModifier::from_json(json),
            Ok(PlacementModifier::TfcCarvingMask { air: true, .. })
        ));
        assert!(PlacementModifier::from_json(r#"{"type":"tfc:carving_mask","step":"x"}"#).is_err());
    }
}
