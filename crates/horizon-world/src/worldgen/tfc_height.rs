//! TerraFirmaCraft's terrain height pipeline (EUPL 1.2): the region partition that finds rivers,
//! `ChunkBiomeSampler`'s biome weights and `ChunkHeightFiller`, which blends every nearby biome's
//! height noise and then reshapes it for shores and rivers.
//!
//! Biome weight maps keep insertion order. Java's own maps iterate by identity hash, so their sums
//! only agree with each other to the last bit or two; this order is a deterministic stand-in.
//! Volcanoes (`CenteredFeatureNoise`) are not ported yet and leave the height unchanged.

use super::tfc_biome::BIOME_NAMES;
use super::tfc_biome_ext::{
    BiomeBlendType, BiomeExtension, RiverBlendType, ShoreBlendType, extensions,
};
use super::tfc_biome_noise::{BiomeNoiseSampler, shoreline_tide_level_noise};
use super::tfc_layers::Area;
use super::tfc_noise::{Noise2D, clamped_map};
use super::tfc_region::units::{
    self, CELL_WIDTH_IN_PARTITION, GRID_WIDTH_IN_BLOCK, PARTITION_BIT_MASK, PARTITION_BITS,
};
use super::tfc_region::{RegionGenerator, Seed};
use super::tfc_river::{MidpointFractal, project_along_line};
use super::tfc_river_noise::{
    RiverInfo, RiverNoiseSampler, create_sampler as create_river_sampler,
};
use super::tfc_shore_noise::{
    ShoreColumn, ShoreNoiseSampler, create_sampler as create_shore_sampler,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const SEA_LEVEL: i32 = 63;

/// One river edge as the partition stores it.
pub struct PartRiver {
    pub fractal: MidpointFractal,
    pub width: i32,
    pub drain_width: i32,
    pub source: (f64, f64),
    pub drain_y: f64,
}

impl PartRiver {
    pub fn width_sq(&self) -> i32 {
        self.width * self.width
    }
    /// `RiverEdge.widthSq(exactGridX, exactGridZ)`, including the original's `drain().y()` used twice.
    pub fn width_sq_at(&self, x: f64, z: f64) -> f64 {
        let t = project_along_line(
            self.source.0,
            self.source.1,
            self.drain_y,
            self.drain_y,
            x,
            z,
        );
        let w = f64::from(self.width) + (f64::from(self.drain_width) - f64::from(self.width)) * t;
        w * w
    }
}

type Partition = Vec<Vec<Arc<PartRiver>>>;

/// The world's shared lookups: the region generator, the biome layer and the river partitions.
pub struct TfcWorld {
    pub level_seed: i64,
    pub region: Arc<RegionGenerator>,
    biome_layer: Mutex<Area>,
    partitions: Mutex<HashMap<(i32, i32), Arc<Partition>>>,
    pub tide: Noise2D,
    river_id: i32,
    volcanoes: super::tfc_volcano::Volcanoes,
}

impl TfcWorld {
    pub fn new(level_seed: i64, region: Arc<RegionGenerator>, biome_layer: Area) -> Self {
        let river_id = BIOME_NAMES
            .iter()
            .position(|n| *n == "river")
            .expect("river biome") as i32;
        Self {
            level_seed,
            region,
            biome_layer: Mutex::new(biome_layer),
            partitions: Mutex::new(HashMap::new()),
            tide: shoreline_tide_level_noise(level_seed),
            river_id,
            volcanoes: super::tfc_volcano::Volcanoes::new(level_seed),
        }
    }

    pub fn river_id(&self) -> i32 {
        self.river_id
    }

    /// `getBiomeExtensionNoRiver(quartX, quartZ)`.
    /// The biome at the centre of the stratovolcano cell holding a block.
    pub fn stratovolcano_biome(&self, x: i32, z: i32) -> i32 {
        self.volcanoes.stratovolcano_biome(x, z, self)
    }

    pub fn volcanoes(&self) -> &super::tfc_volcano::Volcanoes {
        &self.volcanoes
    }

    pub fn volcano_center_biomes(&self, x: i32, z: i32) -> [i32; 4] {
        self.volcanoes.center_biomes(x, z, self)
    }

    pub fn stratovolcanoes(&self) -> &super::tfc_volcano_strato::Stratovolcanoes {
        self.volcanoes.stratovolcanoes()
    }

    pub fn biome_no_river(&self, quart_x: i32, quart_z: i32) -> i32 {
        self.biome_layer.lock().unwrap().get(quart_x, quart_z)
    }

    /// `getBiomeExtension(quartX, quartZ)`: the layer's biome, or `river` where a river crosses it.
    pub fn biome(&self, quart_x: i32, quart_z: i32) -> i32 {
        let biome = self.biome_no_river(quart_x, quart_z);
        if extensions()[biome as usize].rivers {
            let point = self.partition_point(quart_x << 2, quart_z << 2);
            let (x, z) = (
                units::quart_to_grid_exact(f64::from(quart_x)),
                units::quart_to_grid_exact(f64::from(quart_z)),
            );
            if point
                .iter()
                .any(|edge| edge.fractal.intersect(x, z, f64::from(0.08f32)))
            {
                return self.river_id;
            }
        }
        biome
    }

    /// `getPartition(blockX, blockZ)`: the rivers that can reach a partition square.
    pub fn partition_point(&self, block_x: i32, block_z: i32) -> Vec<Arc<PartRiver>> {
        let (grid_x, grid_z) = (units::block_to_grid(block_x), units::block_to_grid(block_z));
        let (cell_x, cell_z) = (units::grid_to_cell(grid_x), units::grid_to_cell(grid_z));
        let partition = {
            let cached = self
                .partitions
                .lock()
                .unwrap()
                .get(&(cell_x, cell_z))
                .cloned();
            match cached {
                Some(p) => p,
                None => {
                    let p = Arc::new(self.create_partition(cell_x, cell_z));
                    self.partitions
                        .lock()
                        .unwrap()
                        .insert((cell_x, cell_z), p.clone());
                    p
                }
            }
        };
        let min_part_x = units::cell_to_part(cell_x);
        let min_part_z = units::cell_to_part(cell_z);
        let (part_x, part_z) = (
            units::grid_to_part(grid_x) - min_part_x,
            units::grid_to_part(grid_z) - min_part_z,
        );
        partition[((part_x & PARTITION_BIT_MASK)
            | ((part_z & PARTITION_BIT_MASK) << PARTITION_BITS)) as usize]
            .clone()
    }

    fn create_partition(&self, cell_x: i32, cell_z: i32) -> Partition {
        let size = (CELL_WIDTH_IN_PARTITION * CELL_WIDTH_IN_PARTITION) as usize;
        let mut partition: Partition = vec![Vec::new(); size];
        let (min_part_x, min_part_z) = (units::cell_to_part(cell_x), units::cell_to_part(cell_z));
        for dx in -1..=1 {
            for dz in -1..=1 {
                let region = self.region.get_or_create_region(
                    units::cell_to_grid(cell_x + dx),
                    units::cell_to_grid(cell_z + dz),
                );
                let Some(rivers) = &region.rivers else {
                    continue;
                };
                for edge in rivers {
                    let drain_width = edge.drain_edge.map_or(edge.width, |i| rivers[i].width);
                    let river = Arc::new(PartRiver {
                        fractal: edge.fractal.clone(),
                        width: edge.width,
                        drain_width,
                        source: (edge.source.x, edge.source.y),
                        drain_y: edge.drain.y,
                    });
                    for part_x in edge.min_part_x..=edge.max_part_x {
                        for part_z in edge.min_part_z..=edge.max_part_z {
                            let (lx, lz) = (part_x - min_part_x, part_z - min_part_z);
                            if lx >= 0
                                && lz >= 0
                                && lx < CELL_WIDTH_IN_PARTITION
                                && lz < CELL_WIDTH_IN_PARTITION
                            {
                                partition[((part_x & PARTITION_BIT_MASK)
                                    | ((part_z & PARTITION_BIT_MASK) << PARTITION_BITS))
                                    as usize]
                                    .push(river.clone());
                            }
                        }
                    }
                }
            }
        }
        partition
    }
}

/// Biome weights, in insertion order.
pub type Weights = Vec<(i32, f64)>;

fn merge(weights: &mut Weights, biome: i32, weight: f64) {
    match weights.iter_mut().find(|(b, _)| *b == biome) {
        Some((_, w)) => *w += weight,
        None => weights.push((biome, weight)),
    }
}

/// `ChunkBiomeSampler.KERNEL_9x9`.
fn kernel() -> &'static [f64; 81] {
    static KERNEL: std::sync::OnceLock<[f64; 81]> = std::sync::OnceLock::new();
    KERNEL.get_or_init(|| {
        let mut values = [0.0; 81];
        for x in 0..9 {
            for z in 0..9 {
                let (dx, dz) = (f64::from(x - 4), f64::from(z - 4));
                values[(x + z * 9) as usize] =
                    0.0211640211641 * (1.0 - 0.03125 * (dz * dz + dx * dx));
            }
        }
        values
    })
}

fn sample_at_position_with_kernel(
    weights: &mut Weights,
    world: &TfcWorld,
    kernel_bits: i32,
    chunk_x: i32,
    chunk_z: i32,
    x_offset: i32,
    z_offset: i32,
) {
    let values = kernel();
    for dx in -4..=4 {
        for dz in -4..=4 {
            let weight = values[((dx + 4) + (dz + 4) * 9) as usize];
            let block_x = chunk_x + ((x_offset + dx) << kernel_bits);
            let block_z = chunk_z + ((z_offset + dz) << kernel_bits);
            merge(
                weights,
                world.biome_no_river(block_x >> 2, block_z >> 2),
                weight,
            );
        }
    }
}

fn corner_contribution(accumulator: &mut Weights, corner: &Weights, t: f64) {
    if t > 0.0 {
        for (biome, weight) in corner {
            merge(accumulator, *biome, weight * t);
        }
    }
}

fn compose_sample_weights(weight_map: &mut Weights, group_weights: &Weights) {
    let group = |biome: i32| extensions()[biome as usize].blend as usize;
    let mut max_weights = [0.0f64; 3];
    for (biome, weight) in group_weights {
        max_weights[group(*biome)] += weight;
    }
    let mut actual = [0.0f64; 3];
    weight_map.retain(|(biome, weight)| {
        actual[group(*biome)] += weight;
        false
    });
    for (biome, weight) in group_weights {
        let g = group(*biome);
        if actual[g] > 0.0 && max_weights[g] > 0.0 {
            weight_map.push((*biome, weight * actual[g] / max_weights[g]));
        }
    }
}

/// `ChunkBiomeSampler.sampleBiomes`: a 7x7 grid of quart-resolution weights around the chunk,
/// indexed `x + 7 * z` with `(1, 1)` at the chunk origin.
pub fn sample_biomes(world: &TfcWorld, chunk_x: i32, chunk_z: i32) -> Vec<Weights> {
    let (chunk_block_x, chunk_block_z) = (chunk_x << 4, chunk_z << 4);
    let mut chunk_weights: Vec<Weights> = vec![Vec::new(); 16];
    for x in 0..4 {
        for z in 0..4 {
            let mut weights = Vec::new();
            sample_at_position_with_kernel(
                &mut weights,
                world,
                4,
                chunk_block_x,
                chunk_block_z,
                x - 1,
                z - 1,
            );
            chunk_weights[(x | (z << 2)) as usize] = weights;
        }
    }
    let mut quart_weights: Vec<Weights> = vec![Vec::new(); 49];
    for x in 0..7 {
        for z in 0..7 {
            let mut quart = Vec::new();
            let mut chunk = Vec::new();
            sample_at_position_with_kernel(
                &mut quart,
                world,
                2,
                chunk_block_x,
                chunk_block_z,
                x - 1,
                z - 1,
            );
            let x1 = chunk_block_x + ((x - 1) << 2);
            let z1 = chunk_block_z + ((z - 1) << 2);
            let (coord_x, coord_z) = (x1 >> 4, z1 >> 4);
            let lerp_x = f64::from(x1 - (coord_x << 4)) * (1.0 / 16.0);
            let lerp_z = f64::from(z1 - (coord_z << 4)) * (1.0 / 16.0);
            let index_x = ((x1 - chunk_block_x) >> 4) + 1;
            let index_z = ((z1 - chunk_block_z) >> 4) + 1;
            corner_contribution(
                &mut chunk,
                &chunk_weights[(index_x | (index_z << 2)) as usize],
                (1.0 - lerp_x) * (1.0 - lerp_z),
            );
            corner_contribution(
                &mut chunk,
                &chunk_weights[((index_x + 1) | (index_z << 2)) as usize],
                lerp_x * (1.0 - lerp_z),
            );
            corner_contribution(
                &mut chunk,
                &chunk_weights[(index_x | ((index_z + 1) << 2)) as usize],
                (1.0 - lerp_x) * lerp_z,
            );
            corner_contribution(
                &mut chunk,
                &chunk_weights[((index_x + 1) | ((index_z + 1) << 2)) as usize],
                lerp_x * lerp_z,
            );
            compose_sample_weights(&mut quart, &chunk);
            quart_weights[(x + 7 * z) as usize] = quart;
        }
    }
    quart_weights
}

/// `sampleBiomesColumn`: the weights at a block column, interpolated between four quart corners.
pub fn sample_biomes_column(corners: &[Weights], local_x: i32, local_z: i32) -> Weights {
    let index_x = (local_x >> 2) + 1;
    let index_z = (local_z >> 2) + 1;
    let lerp_x = f64::from(local_x - ((local_x >> 2) << 2)) * (1.0 / 4.0);
    let lerp_z = f64::from(local_z - ((local_z >> 2) << 2)) * (1.0 / 4.0);
    let mut accumulator = Vec::new();
    corner_contribution(
        &mut accumulator,
        &corners[(index_x + index_z * 7) as usize],
        (1.0 - lerp_x) * (1.0 - lerp_z),
    );
    corner_contribution(
        &mut accumulator,
        &corners[(index_x + 1 + index_z * 7) as usize],
        lerp_x * (1.0 - lerp_z),
    );
    corner_contribution(
        &mut accumulator,
        &corners[(index_x + (index_z + 1) * 7) as usize],
        (1.0 - lerp_x) * lerp_z,
    );
    corner_contribution(
        &mut accumulator,
        &corners[(index_x + 1 + (index_z + 1) * 7) as usize],
        lerp_x * lerp_z,
    );
    accumulator
}

/// What the height filler learns about a column.
#[derive(Clone, Debug)]
pub struct ColumnInfo {
    pub height: f64,
    pub biome: i32,
    /// The biome before rivers override it.
    pub biome_no_river: i32,
    pub river: Option<RiverInfo>,
    pub river_weights: [f64; 11],
    pub shore_weights: [f64; 10],
    pub pre_volcanic_height: f64,
    pub surface_integrity_depth: i32,
    pub any_salty_nearby: bool,
    pub weights: Weights,
}

/// The samplers a chunk's columns share (`createBiomeSamplersForChunk` and friends).
pub struct Samplers {
    pub biomes: Vec<Option<Box<dyn BiomeNoiseSampler>>>,
    pub rivers: Vec<Box<dyn RiverNoiseSampler>>,
    pub shores: Vec<Box<dyn ShoreNoiseSampler>>,
}

impl Samplers {
    pub fn new(level_seed: i64) -> Self {
        let biomes = extensions()
            .iter()
            .map(|e: &BiomeExtension| e.sampler(level_seed))
            .collect();
        let mut river_seed = Seed::of(level_seed);
        let rivers = ALL_RIVERS
            .iter()
            .map(|kind| create_river_sampler(*kind, &mut river_seed))
            .collect();
        let shores = ALL_SHORES
            .iter()
            .map(|kind| create_shore_sampler(*kind, level_seed))
            .collect();
        Self {
            biomes,
            rivers,
            shores,
        }
    }
}

pub const ALL_RIVERS: [RiverBlendType; 11] = [
    RiverBlendType::None,
    RiverBlendType::Banked,
    RiverBlendType::TallBanked,
    RiverBlendType::Floodplain,
    RiverBlendType::Wide,
    RiverBlendType::WideDeep,
    RiverBlendType::Canyon,
    RiverBlendType::TallCanyon,
    RiverBlendType::Talus,
    RiverBlendType::Terraces,
    RiverBlendType::Cave,
];
pub const ALL_SHORES: [ShoreBlendType; 10] = [
    ShoreBlendType::None,
    ShoreBlendType::Classic,
    ShoreBlendType::Sandy,
    ShoreBlendType::Embayments,
    ShoreBlendType::UpperTerrace,
    ShoreBlendType::LowerTerrace,
    ShoreBlendType::SetbackCliffs,
    ShoreBlendType::Dunes,
    ShoreBlendType::RockyShores,
    ShoreBlendType::SeaStacks,
];

pub struct ChunkHeightFiller<'w> {
    world: &'w TfcWorld,
    weights: Vec<Weights>,
    pub samplers: Samplers,
}

impl<'w> ChunkHeightFiller<'w> {
    pub fn new(world: &'w TfcWorld, chunk_x: i32, chunk_z: i32) -> Self {
        Self {
            world,
            weights: sample_biomes(world, chunk_x, chunk_z),
            samplers: Samplers::new(world.level_seed),
        }
    }

    /// `sampleHeight(blockX, blockZ)`.
    pub fn sample_height(&mut self, block_x: i32, block_z: i32) -> f64 {
        self.sample_column(block_x, block_z).height
    }

    /// `sampleColumnHeightAndBiome`: the column's height, biome and the weights the 3D pass needs.
    pub fn sample_column(&mut self, block_x: i32, block_z: i32) -> ColumnInfo {
        let weights = sample_biomes_column(&self.weights, block_x & 15, block_z & 15);
        self.sample_column_with(block_x, block_z, weights)
    }

    /// `calculateNoiseAtHeight`: terrain density at a height in the column just sampled, positive
    /// where solid, in `[-1, 1]`. Call it straight after `sample_column` for the same column.
    pub fn column_noise(&mut self, info: &ColumnInfo, y: i32) -> f64 {
        let mut noise = 0.0;
        for &(biome, weight) in &info.weights {
            if let Some(sampler) = self.samplers.biomes[biome as usize].as_mut() {
                noise += sampler.noise(y) * weight;
            }
        }
        let initial = noise;
        noise = 0.0;
        for (i, weight) in info.shore_weights.iter().copied().enumerate() {
            if i == ShoreBlendType::None as usize {
                noise += weight * initial;
            } else if weight > 0.0 {
                noise += weight * self.samplers.shores[i].noise(y, initial);
            }
        }
        let surface_height = info.height as i32;
        if info.surface_integrity_depth > 0 {
            let carve_below = surface_height - info.surface_integrity_depth;
            noise = clamped_map(
                f64::from(y),
                f64::from(carve_below - 10),
                f64::from(carve_below),
                noise,
                0.0,
            );
        }
        let post_shore = noise;
        noise = 0.0;
        for (i, weight) in info.river_weights.iter().copied().enumerate() {
            if i == RiverBlendType::None as usize {
                noise += weight * post_shore;
            } else if weight > 0.0 {
                noise += weight * self.samplers.rivers[i].noise(y, post_shore);
            }
        }
        noise = super::tfc_biome_noise::AIR_THRESHOLD - noise;
        if y > surface_height {
            noise -= f64::from(y - surface_height) * f64::from(0.2f32);
        }
        noise.clamp(-1.0, 1.0)
    }

    /// The height at a quart corner of the 7x7 grid around the chunk (`(1, 1)` is the chunk origin).
    pub fn sample_corner_height(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
        qx: usize,
        qz: usize,
    ) -> i32 {
        let block_x = (chunk_x << 4) + ((qx as i32 - 1) << 2);
        let block_z = (chunk_z << 4) + ((qz as i32 - 1) << 2);
        let weights = self.weights[qx + 7 * qz].clone();
        self.sample_column_with(block_x, block_z, weights).height as i32
    }

    fn sample_column_with(&mut self, block_x: i32, block_z: i32, weights: Weights) -> ColumnInfo {
        let ext = extensions();
        let (mut height, mut normal_height, mut shore_height) = (0.0f64, 0.0f64, 0.0f64);
        let (mut shore_weight, mut ocean_weight) = (0.0f64, 0.0f64);
        let (mut normal_biome, mut shore_biome, mut ocean_biome): (
            Option<i32>,
            Option<i32>,
            Option<i32>,
        ) = (None, None, None);
        let (mut max_normal, mut max_shore, mut max_ocean) = (0.0f64, 0.0f64, 0.0f64);
        let mut any_salty = false;
        for &(biome, weight) in &weights {
            let e = &ext[biome as usize];
            if e.salty {
                any_salty = true;
            }
            let biome_height = match self.samplers.biomes[biome as usize].as_mut() {
                Some(sampler) => {
                    sampler.set_column(block_x, block_z);
                    weight * sampler.height()
                }
                None => 0.0,
            };
            height += biome_height;
            if e.shore {
                shore_height += biome_height;
                shore_weight += weight;
                if max_shore < weight {
                    shore_biome = Some(biome);
                    max_shore = weight;
                }
            } else if e.blend == BiomeBlendType::Ocean {
                ocean_weight += weight;
                if max_ocean < weight {
                    ocean_biome = Some(biome);
                    max_ocean = weight;
                }
            } else {
                normal_height += biome_height;
                if max_normal < weight {
                    normal_biome = Some(biome);
                    max_normal = weight;
                }
            }
        }
        let mut biome_at = normal_biome;
        let mut shore_weights = [0.0f64; 10];
        for &(biome, weight) in &weights {
            shore_weights[ext[biome as usize].shore_blend as usize] += weight;
        }
        let land_weight = 1.0 - ocean_weight - shore_weight;
        if shore_weight > 0.0 {
            if let Some(shore_biome) = shore_biome {
                height = self.adjust_for_shore(
                    &shore_weights,
                    ShoreColumn {
                        height_in: height,
                        x: block_x,
                        z: block_z,
                        ocean_weight,
                        land_weight,
                        shore_weight,
                        this_weight: max_shore,
                        shore_base_height: ext[shore_biome as usize].shore_base_height,
                        shore_height,
                        normal_height,
                    },
                );
                if shore_weight > 0.5 {
                    biome_at = Some(shore_biome);
                }
            }
        }
        let biome_at = biome_at.or(ocean_biome).unwrap_or(0);
        if ocean_weight >= 0.25 {
            let tide_adjusted_sea_edge = self
                .world
                .tide
                .noise(f64::from(block_x), f64::from(block_z))
                - 4.0;
            height = clamped_map(
                land_weight,
                0.32,
                0.36,
                height.min(tide_adjusted_sea_edge),
                height,
            );
        }
        let mut river_weights = [0.0f64; 11];
        for &(biome, weight) in &weights {
            river_weights[ext[biome as usize].river as usize] += weight;
        }
        let river = self.sample_river_edge(block_x, block_z);
        height =
            self.adjust_for_river(height, river.as_ref(), &mut river_weights, block_x, block_z);
        let pre_volcanic_height = height;
        let no_river_biome = biome_at;
        let biome_at = if height <= f64::from(SEA_LEVEL + 1)
            && river.as_ref().is_some_and(|r| r.norm_dist_sq() < 1.1)
            && ext[biome_at as usize].rivers
        {
            self.world.river_id()
        } else {
            biome_at
        };
        height = self
            .world
            .volcanoes
            .adjust_height(height, block_x, block_z, self.world);
        let delta = height - pre_volcanic_height;
        let surface_integrity_depth = if delta > 0.0 { (delta as i32) * 3 } else { 0 };
        ColumnInfo {
            height,
            biome: biome_at,
            biome_no_river: no_river_biome,
            river,
            river_weights,
            shore_weights,
            pre_volcanic_height,
            surface_integrity_depth,
            any_salty_nearby: any_salty,
            weights,
        }
    }

    fn adjust_for_shore(&mut self, shore_weights: &[f64; 10], column: ShoreColumn) -> f64 {
        let mut blend_height = 0.0;
        for (i, weight) in shore_weights.iter().copied().enumerate() {
            if i == ShoreBlendType::None as usize {
                blend_height += weight * column.height_in;
            } else if weight > 0.0 {
                blend_height +=
                    weight * self.samplers.shores[i].set_column_and_sample_height(&column);
            }
        }
        blend_height
    }

    fn adjust_for_river(
        &mut self,
        height: f64,
        info: Option<&RiverInfo>,
        river_weights: &mut [f64; 11],
        x: i32,
        z: i32,
    ) -> f64 {
        match info {
            Some(info) => {
                let mut blend_height = 0.0;
                for (i, weight) in river_weights.iter().copied().enumerate() {
                    if i == RiverBlendType::None as usize {
                        blend_height += weight * height;
                    } else if weight > 0.0 {
                        blend_height += weight
                            * self.samplers.rivers[i]
                                .set_column_and_sample_height(info, x, z, height, weight);
                    }
                }
                blend_height
            }
            None => {
                *river_weights = [0.0; 11];
                river_weights[RiverBlendType::None as usize] = 1.0;
                height
            }
        }
    }

    /// `sampleRiverEdge`: the closest river within 50 blocks, if any.
    pub fn sample_river_edge(&self, block_x: i32, block_z: i32) -> Option<RiverInfo> {
        let grid_block = f64::from(GRID_WIDTH_IN_BLOCK);
        let limit_dist_in_grid_sq =
            f64::from(50.0f32 * 50.0f32 / (GRID_WIDTH_IN_BLOCK * GRID_WIDTH_IN_BLOCK) as f32);
        let mut min_dist = limit_dist_in_grid_sq;
        let mut min_dist_adjusted = f64::from(f32::MAX);
        let mut min_edge: Option<Arc<PartRiver>> = None;
        let exact_x = units::block_to_grid_exact(f64::from(block_x));
        let exact_z = units::block_to_grid_exact(f64::from(block_z));
        for edge in self.world.partition_point(block_x, block_z) {
            if edge.fractal.maybe_intersect(exact_x, exact_z, min_dist) {
                let dist = edge.fractal.intersect_distance(exact_x, exact_z);
                if dist < limit_dist_in_grid_sq {
                    let adjusted = dist / f64::from(edge.width_sq());
                    if adjusted < min_dist_adjusted {
                        min_dist = dist;
                        min_dist_adjusted = adjusted;
                        min_edge = Some(edge.clone());
                    }
                }
            }
        }
        let edge = min_edge?;
        let real_width = edge.width_sq_at(exact_x, exact_z);
        min_dist *= grid_block * grid_block;
        Some(RiverInfo {
            dist_sq: min_dist,
            width_sq: real_width,
            flow: edge.fractal.calculate_flow(exact_x, exact_z),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::tfc_biome_layers::create_region_biome_layer;
    use crate::worldgen::tfc_region::Settings;

    fn world(seed: i64) -> TfcWorld {
        let mut s = Seed::of(seed);
        let region = Arc::new(RegionGenerator::from_seed(Settings::default(), &mut s));
        let layer = create_region_biome_layer(region.clone(), &mut s)();
        TfcWorld::new(seed, region, layer)
    }

    #[test]
    fn weights_sum_to_one() {
        let world = world(8675309);
        for (cx, cz) in [(0, 0), (40, -17), (-300, 250)] {
            let corners = sample_biomes(&world, cx, cz);
            for (lx, lz) in [(0, 0), (7, 9), (15, 15)] {
                let weights = sample_biomes_column(&corners, lx, lz);
                let total: f64 = weights.iter().map(|(_, w)| w).sum();
                assert!(
                    (total - 1.0).abs() < 1e-9,
                    "chunk ({cx}, {cz}) column ({lx}, {lz}) sums to {total}"
                );
            }
        }
    }

    #[test]
    fn heights_are_plausible_and_deterministic() {
        let world = world(8675309);
        let mut a = ChunkHeightFiller::new(&world, 12, -30);
        let mut b = ChunkHeightFiller::new(&world, 12, -30);
        let mut seen = std::collections::BTreeSet::new();
        for x in (0..16).step_by(5) {
            for z in (0..16).step_by(5) {
                let (bx, bz) = ((12 << 4) + x, (-30 << 4) + z);
                let info = a.sample_column(bx, bz);
                assert_eq!(info.height.to_bits(), b.sample_height(bx, bz).to_bits());
                assert!(
                    info.height > -64.0 && info.height < 400.0,
                    "{}",
                    info.height
                );
                seen.insert(info.biome);
            }
        }
        assert!(!seen.is_empty());
    }

    #[test]
    fn rivers_are_found_somewhere() {
        let world = world(8675309);
        let mut found = 0;
        for gx in -12..12 {
            for gz in -12..12 {
                if world.biome(gx * 32, gz * 32) == world.river_id() {
                    found += 1;
                }
            }
        }
        assert!(found > 0, "no river quarts in a 24x24 grid of samples");
    }
}
