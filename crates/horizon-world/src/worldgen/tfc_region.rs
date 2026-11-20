//! TerraFirmaCraft's region generator (`world.region.*`, EUPL 1.2): the large-scale
//! continent / climate map that every TFC biome and terrain height is read from.
//! Ported in stages; this stage holds the grid units, `Seed`, regions and their
//! points, the continent and climate noises, and the `Init` task.
#![allow(clippy::too_many_arguments)]
use super::tfc_layers::{
    Area, ROCK_LAND, ROCK_OCEAN, ROCK_TYPE_BITS, ROCK_UPLIFT, ROCK_VOLCANIC, RockLayerSettings,
    create_uniform_layer,
};
use super::tfc_noise::clamped_map;
use super::tfc_noise::{Cell, Cellular2D, Noise2D, OpenSimplex2D};
use super::tfc_river::{self, RiverEdge};
use horizon_core::random::{RandomSource, XoroshiroRandom};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

/// `world.region.Units`.
pub mod units {
    pub const PARTITION_BITS: i32 = 5;
    pub const PARTITION_BIT_MASK: i32 = (1 << PARTITION_BITS) - 1;
    pub const PARTITION_WIDTH_IN_GRID: i32 = 3;
    pub const CELL_WIDTH_IN_PARTITION: i32 = 1 << PARTITION_BITS;
    pub const CELL_WIDTH_IN_GRID: i32 = CELL_WIDTH_IN_PARTITION * PARTITION_WIDTH_IN_GRID;
    pub const REGION_RADIUS_IN_GRID: i32 = CELL_WIDTH_IN_GRID + 5;
    pub const REGION_WIDTH_IN_GRID: i32 = 1 + 2 * REGION_RADIUS_IN_GRID;
    pub const QUART_BITS: i32 = 2;
    pub const GRID_BITS: i32 = 7;
    pub const GRID_WIDTH_IN_BLOCK: i32 = 1 << GRID_BITS;
    pub const GRID_WIDTH_IN_QUART: i32 = 1 << (GRID_BITS - QUART_BITS);

    pub fn cell_to_part(cell: i32) -> i32 {
        cell << PARTITION_BITS
    }
    pub fn part_to_cell(part: i32) -> i32 {
        part >> PARTITION_BITS
    }
    pub fn grid_to_cell(grid: i32) -> i32 {
        grid.div_euclid(CELL_WIDTH_IN_GRID)
    }
    pub fn cell_to_grid(cell: i32) -> i32 {
        cell.wrapping_mul(CELL_WIDTH_IN_GRID)
    }
    pub fn grid_to_part(grid: i32) -> i32 {
        grid.div_euclid(PARTITION_WIDTH_IN_GRID)
    }
    pub fn part_to_grid(part: i32) -> i32 {
        part.wrapping_mul(PARTITION_WIDTH_IN_GRID)
    }
    pub fn quart_to_grid(quart: i32) -> i32 {
        quart >> (GRID_BITS - QUART_BITS)
    }
    pub fn quart_to_grid_exact(quart: f64) -> f64 {
        quart / f64::from(GRID_WIDTH_IN_QUART)
    }
    pub fn grid_to_quart(grid: i32) -> i32 {
        grid << (GRID_BITS - QUART_BITS)
    }
    pub fn block_to_grid_exact(block: f64) -> f64 {
        block / f64::from(GRID_WIDTH_IN_BLOCK)
    }
    pub fn block_to_grid(block: i32) -> i32 {
        block >> GRID_BITS
    }
    pub fn grid_to_block(grid: i32) -> i32 {
        grid << GRID_BITS
    }
}

/// `world.settings.Settings`, the parts the region generator reads.
#[derive(Clone, Debug)]
pub struct Settings {
    pub temperature_scale: i32,
    pub temperature_constant: f32,
    pub rainfall_scale: i32,
    pub rainfall_constant: f32,
    pub continentalness: f32,
    pub finite_continents: bool,
}
impl Default for Settings {
    /// TFC's `tfc:overworld` world preset.
    fn default() -> Self {
        Self {
            temperature_scale: 20_000,
            temperature_constant: 0.0,
            rainfall_scale: 20_000,
            rainfall_constant: 0.0,
            continentalness: 0.5,
            finite_continents: false,
        }
    }
}

/// `world.Seed`: the level seed plus the stream of seeds derived from it.
pub struct Seed {
    seed: i64,
    next: XoroshiroRandom,
}
impl Seed {
    pub fn of(level_seed: i64) -> Self {
        Self {
            seed: level_seed,
            next: XoroshiroRandom::new(level_seed),
        }
    }
    pub fn seed(&self) -> i64 {
        self.seed
    }
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> i64 {
        self.next.next_long()
    }
}

/// `Helpers.mix64` and `Helpers.hashDouble`.
pub fn mix64(mut x: i64) -> i64 {
    let u = |v: i64| v as u64;
    x ^= (u(x) >> 33) as i64;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd_u64 as i64);
    x ^= (u(x) >> 33) as i64;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53_u64 as i64);
    x ^= (u(x) >> 33) as i64;
    x
}
pub fn hash_double(input: f64, index: i32) -> f64 {
    let bits = input.to_bits() as i64;
    let x = mix64(bits.wrapping_add(i64::from(index)));
    ((x as u64) >> 11) as f64 * f64::from_bits(0x3ca0_0000_0000_0000)
}

/// `Mth.clampedMap(float, float, float, float, float)`.
pub(super) fn clamped_map_f(
    value: f32,
    from_min: f32,
    from_max: f32,
    to_min: f32,
    to_max: f32,
) -> f32 {
    let t = (value - from_min) / (from_max - from_min);
    if t < 0.0 {
        to_min
    } else if t > 1.0 {
        to_max
    } else {
        to_min + t * (to_max - to_min)
    }
}

/// `FastNoiseLite.FastRound(double)`.
fn fast_round(f: f64) -> i32 {
    if f >= 0.0 {
        (f + 0.5) as i32
    } else {
        (f - 0.5) as i32
    }
}

/// `AnnotateBiomeAltitude.WIDTH`.
pub const BIOME_ALTITUDE_WIDTH: i32 = 4;

#[derive(Clone, Debug)]
pub struct Point {
    pub x: i32,
    pub z: i32,
    pub index: i32,
    pub distance_to_ocean: i8,
    pub distance_to_deep_ocean: i8,
    pub distance_to_edge: i8,
    pub distance_to_west_coast: i8,
    pub distance_to_land: i8,
    pub base_land_height: i8,
    pub biome_altitude: i8,
    pub ocean_depth: i8,
    pub rainfall: f32,
    pub rainfall_variance: f32,
    pub temperature: f32,
    pub divergence: f32,
    pub biome: i32,
    pub rock: i32,
    pub is_surface_rock_karst: bool,
    pub hot_spot_age: i8,
    flags: u16,
}
impl Point {
    const LAND: u16 = 0b1;
    const ISLAND: u16 = 0b10;
    const RIVER: u16 = 0b100;
    const LAKE: u16 = 0b1000;
    const MOUNTAIN: u16 = 0b1_0000;
    const COASTAL_MOUNTAIN: u16 = 0b10_0000;
    const VOLCANIC: u16 = 0b100_0000;
    const BARRIER_ISLAND: u16 = 0b1000_0000;
    fn new(x: i32, z: i32, index: i32) -> Self {
        Self {
            x,
            z,
            index,
            distance_to_ocean: 0,
            distance_to_deep_ocean: 0,
            distance_to_edge: 0,
            distance_to_west_coast: 0,
            distance_to_land: 0,
            base_land_height: 0,
            biome_altitude: 0,
            ocean_depth: 0,
            rainfall: 0.0,
            rainfall_variance: 0.0,
            temperature: 0.0,
            divergence: 0.0,
            biome: 0, // TFCLayers.OCEAN
            rock: 0,
            is_surface_rock_karst: false,
            hot_spot_age: 0,
            flags: 0,
        }
    }
    pub fn discrete_biome_altitude(&self) -> i32 {
        i32::from(self.biome_altitude).div_euclid(BIOME_ALTITUDE_WIDTH)
    }
    pub fn land(&self) -> bool {
        self.flags & Self::LAND != 0
    }
    pub fn island(&self) -> bool {
        self.flags & Self::ISLAND != 0
    }
    pub fn shore(&self) -> bool {
        self.distance_to_ocean == -2
    }
    pub fn river(&self) -> bool {
        self.flags & Self::RIVER != 0
    }
    pub fn lake(&self) -> bool {
        self.flags & Self::LAKE != 0
    }
    pub fn mountain(&self) -> bool {
        self.flags & Self::MOUNTAIN != 0
    }
    pub fn coastal_mountain(&self) -> bool {
        self.flags & Self::COASTAL_MOUNTAIN != 0
    }
    pub fn volcanic(&self) -> bool {
        self.flags & Self::VOLCANIC != 0
    }
    pub fn barrier_island(&self) -> bool {
        self.flags & Self::BARRIER_ISLAND != 0
    }
    pub fn set_land(&mut self) {
        self.flags |= Self::LAND
    }
    pub fn set_island(&mut self) {
        self.flags |= Self::ISLAND
    }
    pub fn set_shore(&mut self) {
        self.distance_to_ocean = -2
    }
    pub fn set_river(&mut self) {
        self.flags |= Self::RIVER
    }
    pub fn set_lake(&mut self) {
        self.flags |= Self::LAKE
    }
    pub fn set_mountain(&mut self) {
        self.flags |= Self::MOUNTAIN
    }
    pub fn set_coastal_mountain(&mut self) {
        self.flags |= Self::COASTAL_MOUNTAIN
    }
    pub fn set_volcanic(&mut self) {
        self.flags |= Self::VOLCANIC
    }
    pub fn set_barrier_island(&mut self) {
        self.flags |= Self::BARRIER_ISLAND
    }
}

/// `world.region.Region`: the grid points owned by one cellular-noise cell.
pub struct Region {
    pub cell_x: f64,
    pub cell_y: f64,
    pub noise: f64,
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
    pub size_x: i32,
    pub size_z: i32,
    pub data: Vec<Option<Point>>,
    /// `Region.rivers`, set by `AddRiversAndLakes`.
    pub rivers: Option<Vec<RiverEdge>>,
}
impl Region {
    pub fn new(cell: &Cell) -> Self {
        let cx = fast_round(cell.x);
        let cz = fast_round(cell.y);
        let r = units::REGION_RADIUS_IN_GRID;
        Self {
            cell_x: cell.x,
            cell_y: cell.y,
            noise: cell.noise,
            min_x: cx - r,
            min_z: cz - r,
            max_x: cx + r,
            max_z: cz + r,
            size_x: 1 + 2 * r,
            size_z: 1 + 2 * r,
            data: Vec::new(),
            rivers: None,
        }
    }
    pub fn size(&self) -> i32 {
        self.size_x * self.size_z
    }
    pub fn is_in(&self, x: i32, z: i32) -> bool {
        x >= self.min_x && x <= self.max_x && z >= self.min_z && z <= self.max_z
    }
    pub fn index(&self, x: i32, z: i32) -> i32 {
        debug_assert!(self.is_in(x, z));
        (x - self.min_x) + self.size_x * (z - self.min_z)
    }
    pub fn at(&self, x: i32, z: i32) -> Option<&Point> {
        if self.is_in(x, z) {
            self.data[self.index(x, z) as usize].as_ref()
        } else {
            None
        }
    }
    pub fn at_mut(&mut self, x: i32, z: i32) -> Option<&mut Point> {
        if self.is_in(x, z) {
            let i = self.index(x, z) as usize;
            self.data[i].as_mut()
        } else {
            None
        }
    }
    /// `Region.random`: a uniformly chosen array slot, which may be empty.
    pub fn random(&self, random: &mut impl RandomSource) -> Option<i32> {
        let i = random.next_int_bound(self.data.len() as i32).unwrap();
        self.data[i as usize].is_some().then_some(i)
    }
    pub fn at_index(&self, index: i32) -> Option<&Point> {
        self.data[index as usize].as_ref()
    }
    /// `atOffset`: the neighbour `(dx, dz)` away from the point at `index`.
    pub fn at_offset(&self, index: i32, dx: i32, dz: i32) -> Option<&Point> {
        let lx = dx + index % self.size_x;
        let lz = dz + index / self.size_x;
        if lx >= 0 && lx < self.size_x && lz >= 0 && lz < self.size_z {
            self.data[(lx + self.size_x * lz) as usize].as_ref()
        } else {
            None
        }
    }
    pub fn points(&self) -> impl Iterator<Item = &Point> {
        self.data.iter().flatten()
    }
    /// The array index of the neighbour `(dx, dz)` away from `index`, if inside the bounding box.
    pub fn offset_index(&self, index: i32, dx: i32, dz: i32) -> Option<i32> {
        let lx = dx + index % self.size_x;
        let lz = dz + index / self.size_x;
        (lx >= 0 && lx < self.size_x && lz >= 0 && lz < self.size_z).then(|| lx + self.size_x * lz)
    }
    fn set_region_area(&mut self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) {
        self.size_x = 1 + max_x - min_x;
        self.size_z = 1 + max_z - min_z;
        self.data = vec![None; (self.size_x * self.size_z) as usize];
        self.min_x = min_x;
        self.min_z = min_z;
        self.max_x = max_x;
        self.max_z = max_z;
    }
    fn init(&mut self, x: i32, z: i32) {
        let index = self.index(x, z);
        self.data[index as usize] = Some(Point::new(x, z, index));
    }
}

/// `RegionGenerator.Context`: one region being built, with its own random stream.
pub struct Context<'g> {
    pub generator: &'g RegionGenerator,
    pub region_cell: Cell,
    pub random: XoroshiroRandom,
    pub region: Region,
}

/// The ordered steps of `RegionGenerator.Task`; later stages append to this list.
pub type RegionTask = fn(&mut Context<'_>);
pub const TASKS: &[RegionTask] = &[
    init,
    annotate_distance_to_cell_edge,
    annotate_boundary_types,
    add_continents_and_set_ocean_depths,
    flood_fill_small_oceans,
    add_hotspots,
    annotate_distance_to_ocean,
    annotate_distance_to_deep_ocean,
    annotate_base_land_height,
    annotate_distance_to_west_coast,
    add_mountains_and_barrier_islands,
    add_islands,
    annotate_biome_altitude,
    annotate_climate,
    choose_rocks,
    annotate_karst_surface,
    add_rivers_and_lakes,
    super::tfc_biome::choose_biomes,
];

/// `world.region.Init`: keep the grid points inside the region's own cell.
fn init(context: &mut Context<'_>) {
    let r = units::REGION_RADIUS_IN_GRID;
    let (rx, rz) = (context.region.min_x, context.region.min_z);
    let total = context.region.size() as usize;
    let prev_size_x = 1 + 2 * r;
    let mut cell = vec![false; total];
    let (mut min_x, mut min_z) = (i32::MAX, i32::MAX);
    let (mut max_x, mut max_z) = (i32::MIN, i32::MIN);
    for dx in 0..=2 * r {
        for dz in 0..=2 * r {
            let (gx, gz) = (rx + dx, rz + dz);
            let index = context.region.index(gx, gz) as usize;
            let other = context.generator.sample_cell(gx, gz);
            if other.x == context.region_cell.x && other.y == context.region_cell.y {
                cell[index] = true;
                min_x = min_x.min(gx);
                min_z = min_z.min(gz);
                max_x = max_x.max(gx);
                max_z = max_z.max(gz);
            }
        }
    }
    let size_x = 1 + max_x - min_x;
    let size_z = 1 + max_z - min_z;
    let off_x = min_x - rx;
    let off_z = min_z - rz;
    context.region.set_region_area(min_x, min_z, max_x, max_z);
    for dx in 0..size_x {
        for dz in 0..size_z {
            if cell[((off_x + dx) + prev_size_x * (off_z + dz)) as usize] {
                context.region.init(min_x + dx, min_z + dz);
            }
        }
    }
}

/// `AnnotateDistanceToCellEdge`: breadth-first distance from the region's edge.
fn annotate_distance_to_cell_edge(context: &mut Context<'_>) {
    let region = &mut context.region;
    let mut explored = vec![false; region.size() as usize];
    let mut queue: VecDeque<i32> = VecDeque::new();
    for dx in 0..region.size_x {
        for dz in 0..region.size_z {
            let index = dx + region.size_x * dz;
            let unbounded =
                dx == 0 || dz == 0 || dx == region.size_x - 1 || dz == region.size_z - 1;
            let present = region.data[index as usize].is_some();
            if !present || unbounded {
                explored[index as usize] = true;
                queue.push_back(index);
                if let Some(p) = region.data[index as usize].as_mut() {
                    p.distance_to_edge = -1;
                }
            }
        }
    }
    while let Some(last) = queue.pop_front() {
        let next_distance = match region.data[last as usize].as_ref() {
            None => 0,
            Some(p) => i32::from(p.distance_to_edge) + 1,
        };
        for dx in -1..=1 {
            for dz in -1..=1 {
                let Some(i) = region.offset_index(last, dx, dz) else {
                    continue;
                };
                if let Some(p) = region.data[i as usize].as_mut() {
                    if p.distance_to_edge == 0 {
                        if !explored[p.index as usize] {
                            p.distance_to_edge = next_distance as i8;
                            explored[p.index as usize] = true;
                            queue.push_back(p.index);
                        }
                        explored[p.index as usize] = true;
                    }
                }
            }
        }
    }
}

/// `AnnotateBoundaryTypes`.
fn annotate_boundary_types(context: &mut Context<'_>) {
    let generator = context.generator;
    for p in context.region.data.iter_mut().flatten() {
        p.divergence = generator.get_divergence(p) as f32;
    }
}

/// `AddContinentsAndSetOceanDepths`.
fn add_continents_and_set_ocean_depths(context: &mut Context<'_>) {
    let generator = context.generator;
    for p in context.region.data.iter_mut().flatten() {
        let perlin = generator
            .continent_noise
            .noise(f64::from(p.x), f64::from(p.z));
        let finite = f64::from(generator.continent_factor(p));
        let tectonic = if p.distance_to_edge <= 5 && p.divergence > 0.0 {
            f64::from(5 - i32::from(p.distance_to_edge)) * -0.12
        } else {
            0.0
        };
        let continent = (perlin + tectonic) * finite;
        if continent > 4.4 {
            p.set_land();
        } else if p.divergence > 0.0 && p.distance_to_edge < 2 {
            p.ocean_depth = 3;
        } else if continent > 3.3 {
            p.ocean_depth = 2;
        } else if continent > 3.0 && p.divergence < 0.0 {
            p.ocean_depth = 5;
        } else if p.distance_to_edge < 2 && !(p.divergence < 0.0 && continent > 2.0) {
            p.ocean_depth = 3;
        } else {
            p.ocean_depth = 4;
        }
    }
}

const SMALL_OCEAN_FILL_THRESHOLD: usize = 180;

/// Flood fill from `start` over points that satisfy `passable`; fills them with land
/// when the area is small and does not touch the region's bounding box.
fn flood_fill(
    region: &mut Region,
    explored: &mut [bool],
    start: i32,
    passable: impl Fn(&Point) -> bool,
) {
    let mut values: HashSet<i32> = HashSet::new();
    let mut queue: VecDeque<i32> = VecDeque::new();
    queue.push_back(start);
    values.insert(start);
    let mut unbounded = false;
    while let Some(last) = queue.pop_front() {
        for dx in -1..=1 {
            for dz in -1..=1 {
                let Some(i) = region.offset_index(last, dx, dz) else {
                    unbounded = true;
                    continue;
                };
                let Some(p) = region.data[i as usize].as_ref() else {
                    unbounded = true;
                    continue;
                };
                if !passable(p) || explored[p.index as usize] {
                    continue;
                }
                explored[p.index as usize] = true;
                queue.push_back(p.index);
                values.insert(p.index);
            }
        }
    }
    if values.len() < SMALL_OCEAN_FILL_THRESHOLD && !unbounded {
        for i in values {
            if let Some(p) = region.data[i as usize].as_mut() {
                p.set_land();
            }
        }
    }
}

/// `FloodFillSmallOceans`.
fn flood_fill_small_oceans(context: &mut Context<'_>) {
    let region = &mut context.region;
    let mut explored = vec![false; region.size() as usize];
    let indices: Vec<i32> = region.points().map(|p| p.index).collect();
    for &i in &indices {
        let p = region.data[i as usize].as_ref().unwrap();
        if !p.land() && !explored[p.index as usize] {
            flood_fill(region, &mut explored, i, |q| !q.land());
        }
    }
    let mut explored = vec![false; region.size() as usize];
    for &i in &indices {
        let p = region.data[i as usize].as_ref().unwrap();
        if !p.land() && p.ocean_depth != 2 && !explored[p.index as usize] {
            flood_fill(region, &mut explored, i, |q| q.ocean_depth != 2);
        }
    }
}

fn shift(v: i32) -> f64 {
    f64::from(v) + 0.5
}

/// `AddHotspots`.
fn add_hotspots(context: &mut Context<'_>) {
    let g = context.generator;
    let region = &mut context.region;
    let mut queue: VecDeque<i32> = VecDeque::new();
    let indices: Vec<i32> = region.points().map(|p| p.index).collect();
    for &i in &indices {
        let p = region.data[i as usize].as_mut().unwrap();
        if g.continent_factor(p) > 0.5 {
            let cell = g.plate_region_noise.cell(f64::from(p.x), f64::from(p.z));
            let edge_dist = (cell.f1 - cell.f2).abs();
            let val = g.hot_spot_intensity_noise.noise(shift(p.x), shift(p.z));
            if val > 0.65 && edge_dist > 0.05 {
                let age = (g.hot_spot_age_noise.noise(shift(p.x), shift(p.z)) as i32) as i8;
                p.hot_spot_age = age;
                if age != 4 {
                    p.set_land();
                }
                queue.push_back(p.index);
            }
        }
    }
    while let Some(index) = queue.pop_front() {
        let last_age = region.data[index as usize].as_ref().unwrap().hot_spot_age;
        for dx in -1..=1 {
            for dz in -1..=1 {
                let Some(ni) = region.offset_index(index, dx, dz) else {
                    continue;
                };
                let Some(next) = region.data[ni as usize].as_mut() else {
                    continue;
                };
                if next.hot_spot_age == 0 {
                    if g.hot_spot_intensity_noise
                        .noise(shift(next.x), shift(next.z))
                        > 0.15
                    {
                        queue.push_back(next.index);
                        next.hot_spot_age = last_age;
                        if last_age != 4 {
                            next.set_land();
                        }
                    } else if !next.land()
                        && g.hot_spot_intensity_noise
                            .noise(shift(next.x) - f64::from(dx), shift(next.z) - f64::from(dz))
                            > 0.15
                    {
                        next.hot_spot_age = last_age;
                        if last_age != 4 {
                            next.set_land();
                        }
                    }
                }
            }
        }
    }
}

/// `AnnotateDistanceToOcean`.
fn annotate_distance_to_ocean(context: &mut Context<'_>) {
    let region = &mut context.region;
    let mut explored = vec![false; region.size() as usize];
    let mut queue: VecDeque<i32> = VecDeque::new();
    for p in region.data.iter_mut().flatten() {
        if !p.land() {
            p.distance_to_ocean = -1;
            queue.push_back(p.index);
            explored[p.index as usize] = true;
        }
    }
    while let Some(last) = queue.pop_front() {
        let next_distance = i32::from(
            region.data[last as usize]
                .as_ref()
                .unwrap()
                .distance_to_ocean,
        ) + 1;
        for dx in -1..=1 {
            for dz in -1..=1 {
                let Some(ni) = region.offset_index(last, dx, dz) else {
                    continue;
                };
                let Some(point) = region.data[ni as usize].as_ref() else {
                    continue;
                };
                if point.land() && point.distance_to_ocean == 0 {
                    let (pi, island) = (point.index, point.island());
                    let last_land = region.data[last as usize].as_ref().unwrap().land();
                    if !last_land && !island {
                        region.data[last as usize].as_mut().unwrap().set_shore();
                    }
                    if !explored[pi as usize] {
                        region.data[pi as usize].as_mut().unwrap().distance_to_ocean =
                            next_distance as i8;
                        queue.push_back(pi);
                    }
                    explored[pi as usize] = true;
                }
            }
        }
    }
}

/// `AnnotateDistanceToDeepOcean`.
fn annotate_distance_to_deep_ocean(context: &mut Context<'_>) {
    let region = &mut context.region;
    let mut explored = vec![false; region.size() as usize];
    let mut queue: VecDeque<i32> = VecDeque::new();
    for p in region.data.iter_mut().flatten() {
        if p.ocean_depth > 2 {
            p.distance_to_deep_ocean = -1;
            queue.push_back(p.index);
            explored[p.index as usize] = true;
        }
    }
    while let Some(last) = queue.pop_front() {
        let next_distance = i32::from(
            region.data[last as usize]
                .as_ref()
                .unwrap()
                .distance_to_deep_ocean,
        ) + 1;
        for dx in -1..=1 {
            for dz in -1..=1 {
                let Some(ni) = region.offset_index(last, dx, dz) else {
                    continue;
                };
                let Some(point) = region.data[ni as usize].as_mut() else {
                    continue;
                };
                if point.ocean_depth <= 2 && point.distance_to_deep_ocean == 0 {
                    if !explored[point.index as usize] {
                        point.distance_to_deep_ocean = next_distance as i8;
                        queue.push_back(point.index);
                    }
                    explored[point.index as usize] = true;
                }
            }
        }
    }
}

/// `AnnotateBaseLandHeight`.
fn annotate_base_land_height(context: &mut Context<'_>) {
    const ISLAND_SEED_DEPTH: i32 = 3;
    let random = &mut context.random;
    let region = &mut context.region;
    let mut explored = vec![false; region.size() as usize];
    let mut queue: VecDeque<i32> = VecDeque::new();
    let mut island_queue: Vec<i32> = Vec::new();
    for p in region.data.iter_mut().flatten() {
        if p.land() {
            p.base_land_height = p.distance_to_ocean;
            if p.base_land_height > p.distance_to_edge {
                let v =
                    0.3f32 * f32::from(p.base_land_height) + 0.7f32 * f32::from(p.distance_to_edge);
                p.base_land_height = (v as i32) as i8;
            }
            explored[p.index as usize] = true;
            if p.island() {
                p.distance_to_land = ISLAND_SEED_DEPTH as i8;
                island_queue.push(p.index);
            } else {
                p.distance_to_land = 0;
                queue.push_back(p.index);
            }
        }
    }
    while let Some(last) = queue.pop_front() {
        let last_distance = region.data[last as usize]
            .as_ref()
            .unwrap()
            .distance_to_land;
        let next_depth = i32::from(last_distance) + 1;
        if next_depth == ISLAND_SEED_DEPTH && !island_queue.is_empty() {
            queue.extend(island_queue.drain(..));
        }
        for dx in -1..=1 {
            for dz in -1..=1 {
                let Some(ni) = region.offset_index(last, dx, dz) else {
                    continue;
                };
                let Some(point) = region.data[ni as usize].as_mut() else {
                    continue;
                };
                if !point.land() && point.distance_to_land == 0 {
                    if !explored[point.index as usize] {
                        if random.next_int_bound(15).unwrap() == 0 {
                            point.distance_to_land = last_distance;
                            queue.push_front(point.index);
                        } else {
                            point.distance_to_land = next_depth as i8;
                            queue.push_back(point.index);
                        }
                    }
                    explored[point.index as usize] = true;
                }
            }
        }
    }
}

/// `AnnotateDistanceToWestCoast`.
fn annotate_distance_to_west_coast(context: &mut Context<'_>) {
    let g = context.generator;
    let region = &mut context.region;
    for dx in 0..region.size_x {
        for dz in 0..region.size_z {
            let index = dx + region.size_x * dz;
            if region.data[index as usize].is_none() {
                continue;
            }
            if dx == 0 {
                region.data[index as usize]
                    .as_mut()
                    .unwrap()
                    .distance_to_west_coast = 0;
                continue;
            }
            let last_center = region.data[(index - 1) as usize]
                .as_ref()
                .map(|p| p.distance_to_west_coast);
            let (land, dist_ocean, z) = {
                let p = region.data[index as usize].as_ref().unwrap();
                (p.land(), p.distance_to_ocean, p.z)
            };
            let value = match last_center {
                None => {
                    if land {
                        (25 + i32::from(dist_ocean)) as i8
                    } else {
                        0
                    }
                }
                Some(last_val) => {
                    let last_val = i32::from(last_val);
                    if !land {
                        (std::cmp::max(last_val - 2, 0)) as i8
                    } else {
                        let mut sum = 0i32;
                        let scale = g.settings.temperature_scale as f32;
                        let frequency = units::GRID_WIDTH_IN_BLOCK as f32 / (2.0f32 * scale);
                        let zf = z as f32;
                        let v = frequency * zf + 0.75f32;
                        let fl = v as i32;
                        let fl = if v < fl as f32 { fl - 1 } else { fl };
                        let function =
                            (4.0f32 * frequency * zf - 4.0f32 * fl as f32).abs() - 1.0f32;
                        let start = -2 + i32::from(function > 0.1);
                        let end = 2 - i32::from(function < -0.1);
                        for dz2 in start..=end {
                            match region.at_offset(index, -1, dz2) {
                                Some(last) => sum += i32::from(last.distance_to_west_coast),
                                None => sum += last_val,
                            }
                        }
                        let q = sum as f32 / (1.0f32 + end as f32 - start as f32);
                        let c = q as i32;
                        let c = if q > c as f32 { c + 1 } else { c };
                        (c + 1) as i8
                    }
                }
            };
            region.data[index as usize]
                .as_mut()
                .unwrap()
                .distance_to_west_coast = value;
        }
    }
    let mut explored = vec![false; region.size() as usize];
    let mut queue: VecDeque<i32> = VecDeque::new();
    for p in region.data.iter().flatten() {
        if p.land() {
            explored[p.index as usize] = true;
            queue.push_back(p.index);
        }
    }
    while let Some(last) = queue.pop_front() {
        let last_distance = i32::from(
            region.data[last as usize]
                .as_ref()
                .unwrap()
                .distance_to_west_coast,
        );
        let next_distance = last_distance + if last_distance > 40 { -1 } else { 1 };
        for dx in -1..=1 {
            for dz in -1..=1 {
                let Some(ni) = region.offset_index(last, dx, dz) else {
                    continue;
                };
                let Some(point) = region.data[ni as usize].as_mut() else {
                    continue;
                };
                if point.distance_to_west_coast == 0 {
                    if !explored[point.index as usize] {
                        point.distance_to_west_coast = next_distance as i8;
                        queue.push_back(point.index);
                    }
                    explored[point.index as usize] = true;
                }
            }
        }
    }
}

/// Breadth-first growth used by the mountain, arc and barrier placers.
/// `passable` decides whether a neighbour joins; `same_level` picks front or back of the queue.
fn grow(
    region: &Region,
    origin: i32,
    max_size: i32,
    reach: i32,
    passable: impl Fn(&Point) -> bool,
    level: impl Fn(&Point) -> i32,
) -> Vec<i32> {
    let mut explored = vec![false; region.size() as usize];
    let mut queue: VecDeque<i32> = VecDeque::new();
    let mut range: Vec<i32> = vec![origin];
    queue.push_back(origin);
    explored[origin as usize] = true;
    while let Some(last) = queue.pop_front() {
        if range.len() as i32 > max_size {
            break;
        }
        let last_level = level(region.data[last as usize].as_ref().unwrap());
        for dx in -reach..=reach {
            for dz in -reach..=reach {
                let Some(ni) = region.offset_index(last, dx, dz) else {
                    continue;
                };
                let Some(p) = region.data[ni as usize].as_ref() else {
                    continue;
                };
                if passable(p) && !explored[p.index as usize] {
                    if last_level != level(p) {
                        queue.push_back(p.index);
                    } else {
                        queue.push_front(p.index);
                    }
                    range.push(p.index);
                    explored[p.index as usize] = true;
                }
            }
        }
    }
    range
}

/// `AddMountainsAndBarrierIslands`.
fn add_mountains_and_barrier_islands(context: &mut Context<'_>) {
    let g = context.generator;
    let random = &mut context.random;
    let region = &mut context.region;
    for p in region.data.iter_mut().flatten() {
        if p.land()
            && p.divergence < 0.0
            && f64::from(p.distance_to_edge)
                < 1.5 * g.continent_noise.noise(f64::from(p.x), f64::from(p.z)) - 5.2
        {
            p.set_mountain();
        }
    }
    let (mut attempt, mut mtns, mut isles) = (0, 0, 0);
    while attempt < 40 && !(mtns >= 3 && isles >= 6) {
        attempt += 1;
        let Some(oi) = region.random(random) else {
            continue;
        };
        let o = region.data[oi as usize].as_ref().unwrap().clone();
        if mtns < 3
            && (o.land() && o.base_land_height <= 1
                || (o.base_land_height >= 4 && o.base_land_height <= 11))
        {
            let origin_height = std::cmp::max(1, i32::from(o.base_land_height));
            let max_size = 70 + random.next_int_bound(40).unwrap();
            let range = grow(
                region,
                oi,
                max_size,
                1,
                |p| {
                    p.land()
                        && i32::from(p.base_land_height) >= origin_height - 1
                        && i32::from(p.base_land_height) <= origin_height + 1
                        && (p.base_land_height > 2 || p.distance_to_ocean < 3)
                },
                |p| i32::from(p.base_land_height),
            );
            if range.len() > 45 {
                for i in range {
                    let p = region.data[i as usize].as_mut().unwrap();
                    p.set_mountain();
                    if o.divergence < 0.0 && o.base_land_height < 8 {
                        p.set_volcanic();
                    }
                    if o.base_land_height <= 2 {
                        p.set_coastal_mountain();
                    }
                }
                mtns += 1;
            }
        } else if isles < 6
            && !o.land()
            && o.ocean_depth == 2
            && o.divergence < 0.0
            && o.distance_to_deep_ocean <= 3
            && o.distance_to_land > 2
        {
            let origin_dist = std::cmp::max(1, i32::from(o.distance_to_deep_ocean));
            let max_size = 90 + random.next_int_bound(50).unwrap();
            let arc = grow(
                region,
                oi,
                max_size,
                2,
                |p| {
                    p.ocean_depth == 2
                        && p.divergence < 0.0
                        && i32::from(p.distance_to_deep_ocean) >= origin_dist - 1
                        && i32::from(p.distance_to_deep_ocean) <= origin_dist + 2
                },
                |p| i32::from(p.distance_to_deep_ocean),
            );
            if arc.len() > 45 {
                let contour = o.distance_to_deep_ocean;
                for i in arc {
                    let p = region.data[i as usize].as_mut().unwrap();
                    if p.distance_to_deep_ocean >= contour
                        && i32::from(p.distance_to_deep_ocean) <= i32::from(contour) + 1
                    {
                        p.set_barrier_island();
                    }
                    p.set_volcanic();
                    p.ocean_depth = 1;
                }
                isles += 2;
            }
        } else if isles < 6
            && !o.land()
            && o.ocean_depth == 2
            && o.divergence > 0.0
            && o.distance_to_land > 2
            && o.distance_to_land < 6
        {
            let origin_dist = std::cmp::max(1, i32::from(o.distance_to_land));
            let max_size = 70 + random.next_int_bound(40).unwrap();
            let barrier = grow(
                region,
                oi,
                max_size,
                1,
                |p| {
                    p.ocean_depth == 2
                        && i32::from(p.distance_to_land) >= origin_dist - 1
                        && i32::from(p.distance_to_land) <= origin_dist + 1
                },
                |p| i32::from(p.distance_to_land),
            );
            let start = std::cmp::max(1, i32::from(o.distance_to_land)) as i8;
            if barrier.len() > 45 {
                for i in barrier {
                    let p = region.data[i as usize].as_mut().unwrap();
                    if p.distance_to_land == start {
                        p.set_barrier_island();
                    }
                    p.ocean_depth = 1;
                }
                isles += 1;
            }
        }
    }
}

/// `AddIslands`.
fn add_islands(context: &mut Context<'_>) {
    let g = context.generator;
    let random = &mut context.random;
    let region = &mut context.region;
    let (mut attempt, mut placed) = (0, 0);
    while attempt < 130 && placed < 15 {
        attempt += 1;
        let Some(first) = region.random(random) else {
            continue;
        };
        let mut current = Some(first);
        {
            let p = region.data[first as usize].as_ref().unwrap();
            if p.land() || p.shore() || p.distance_to_edge <= 2 || g.continent_factor(p) <= 0.5 {
                continue;
            }
        }
        for _ in 0..12 {
            let Some(ci) = current else { break };
            let (x, z) = {
                let p = region.data[ci as usize].as_mut().unwrap();
                p.set_land();
                p.set_island();
                p.distance_to_ocean = 1;
                (p.x, p.z)
            };
            let nx = x + random.next_int_bound(4).unwrap() - random.next_int_bound(4).unwrap();
            let nz = z + random.next_int_bound(4).unwrap() - random.next_int_bound(4).unwrap();
            current =
                if region.is_in(nx, nz) && region.data[region.index(nx, nz) as usize].is_some() {
                    Some(region.index(nx, nz))
                } else {
                    None
                };
            match current.map(|i| region.data[i as usize].as_ref().unwrap()) {
                None => break,
                Some(p) => {
                    if (p.land() && !p.island()) || p.distance_to_edge <= 2 || p.ocean_depth == 1 {
                        break;
                    }
                }
            }
        }
        placed += 1;
    }
}

/// `AnnotateBiomeAltitude`.
fn annotate_biome_altitude(context: &mut Context<'_>) {
    const WIDTH: i32 = BIOME_ALTITUDE_WIDTH;
    let random = &mut context.random;
    let region = &mut context.region;
    let mut explored = vec![false; region.size() as usize];
    let mut queue: VecDeque<i32> = VecDeque::new();
    for p in region.data.iter_mut().flatten() {
        if p.land() && p.mountain() {
            p.biome_altitude = (3 * WIDTH) as i8;
            queue.push_back(p.index);
            explored[p.index as usize] = true;
        }
    }
    while let Some(last) = queue.pop_front() {
        let last_altitude = region.data[last as usize].as_ref().unwrap().biome_altitude;
        let next_altitude = i32::from(last_altitude) - 1;
        if next_altitude < 0 {
            continue;
        }
        for dx in -1..=1 {
            for dz in -1..=1 {
                let Some(ni) = region.offset_index(last, dx, dz) else {
                    continue;
                };
                let Some(p) = region.data[ni as usize].as_mut() else {
                    continue;
                };
                if p.land() && p.biome_altitude == 0 && !explored[p.index as usize] {
                    if random.next_int_bound(13).unwrap() == 0
                        && i32::from(last_altitude) != 3 * WIDTH
                    {
                        p.biome_altitude = last_altitude;
                        queue.push_front(p.index);
                    } else {
                        p.biome_altitude = next_altitude as i8;
                        queue.push_back(p.index);
                    }
                    explored[p.index as usize] = true;
                }
            }
        }
    }
    for p in region.data.iter_mut().flatten() {
        if p.land() && p.discrete_biome_altitude() == 0 && p.base_land_height >= 4 {
            if p.discrete_biome_altitude() == 0 && p.base_land_height >= 4 {
                p.biome_altitude = WIDTH as i8;
            }
            if p.discrete_biome_altitude() == 1 && p.base_land_height >= 11 {
                p.biome_altitude = (2 * WIDTH) as i8;
            }
        }
    }
}

/// `ChooseRocks`.
fn choose_rocks(context: &mut Context<'_>) {
    let g = context.generator;
    let region = &mut context.region;
    let mut area = g.rock_area.lock().unwrap();
    let indices: Vec<i32> = region.points().map(|p| p.index).collect();
    for i in indices {
        let (x, z) = {
            let p = region.data[i as usize].as_ref().unwrap();
            (p.x, p.z)
        };
        let kind = find_closest_rock_type(region, i);
        region.data[i as usize].as_mut().unwrap().rock = (area.get(x, z) << ROCK_TYPE_BITS) | kind;
    }
}
fn find_closest_rock_type(region: &Region, index: i32) -> i32 {
    let center = region.data[index as usize].as_ref().unwrap();
    let mut kind = if center.land() { ROCK_LAND } else { ROCK_OCEAN };
    let mut min_dist = i32::MAX;
    for dx in -2..=2i32 {
        for dz in -2..=2i32 {
            let Some(p) = region.at_offset(index, dx, dz) else {
                continue;
            };
            let dist = dx.abs() + dz.abs();
            if dist < min_dist {
                if (p.island() && dist < 4) || p.hot_spot_age > 0 || p.volcanic() {
                    kind = ROCK_VOLCANIC;
                    min_dist = dist;
                } else if (p.mountain() || p.coastal_mountain()) && dist < 3 {
                    kind = ROCK_UPLIFT;
                    min_dist = dist;
                }
            }
        }
    }
    kind
}

/// `KarstSurfaceRocks`.
fn annotate_karst_surface(context: &mut Context<'_>) {
    let g = context.generator;
    for p in context.region.data.iter_mut().flatten() {
        p.is_surface_rock_karst = g.rock_layers.sample_at_layer(p.rock, 0).karst;
    }
}

const RIVER_LENGTH: f32 = 2.7;
const RIVER_DEPTH: i32 = 17;
const RIVER_FEATHER: f32 = 0.8;

/// `AddRiversAndLakes.findBestStartingAngle`; `NaN` when no land neighbour qualifies.
fn find_best_starting_angle(region: &Region, random: &mut XoroshiroRandom, index: i32) -> f32 {
    let mut best_metric = f32::from_bits(1); // Float.MIN_VALUE
    let mut best_count = 0;
    let mut best_angle = f32::NAN;
    for dir_x in -1..=1 {
        for dir_z in -1..=1 {
            if dir_x == 0 && dir_z == 0 {
                continue;
            }
            if let Some(p) = region.at_offset(index, 4 * dir_x, 4 * dir_z) {
                if p.land() {
                    let metric =
                        (i32::from(p.distance_to_ocean) - i32::abs(dir_x) - i32::abs(dir_z)) as f32;
                    if metric > best_metric
                        || (metric == best_metric
                            && random.next_int_bound(1 + best_count).unwrap() == 0)
                    {
                        if metric > best_metric {
                            best_metric = metric;
                            best_count = 0;
                        }
                        best_count += 1;
                        best_angle =
                            horizon_core::jmath::atan2(f64::from(dir_z), f64::from(dir_x)) as f32;
                    }
                }
            }
        }
    }
    if !best_angle.is_nan() {
        best_angle += random.next_float() * 0.2f32 - 0.1f32;
    }
    best_angle
}

fn mark_river(region: &mut Region, x: i32, z: i32) {
    if let Some(p) = region.at_mut(x, z) {
        if p.land() {
            p.set_river();
            p.rainfall += 0.09f32 * (500f32 - p.rainfall);
        }
    }
}

fn place_lake_near(region: &mut Region, edge: &RiverEdge, offset_x: i32, offset_z: i32) {
    let grid_x = (edge.source.x + f64::from(0.3f32 * offset_x as f32)) as i32;
    let grid_z = (edge.source.y + f64::from(0.3f32 * offset_z as f32)) as i32;
    if let Some(p) = region.at_mut(grid_x, grid_z) {
        if p.land() && p.distance_to_ocean >= 2 && p.distance_to_edge >= 2 {
            p.set_lake();
            p.rainfall += 0.09f32 * (500f32 - p.rainfall);
        }
    }
}

fn annotate_river_valley(region: &mut Region, edge: &RiverEdge) {
    let ux = edge.source.x as i32;
    let uy = edge.source.y as i32;
    let vx = edge.drain.x as i32;
    let vy = edge.drain.y as i32;
    let dx = vx.wrapping_sub(ux);
    let dy = vy.wrapping_sub(uy);
    let mag = f64::from(dx.wrapping_mul(dx).wrapping_add(dy.wrapping_mul(dy))).sqrt();
    let unit_x = f64::from(dx) / mag;
    let unit_y = f64::from(dy) / mag;
    let mut i = 0.0f64;
    while i <= mag {
        let x = (f64::from(ux) + unit_x * i) as i32;
        let y = (f64::from(uy) + unit_y * i) as i32;
        mark_river(region, x, y);
        mark_river(region, x + 1, y);
        mark_river(region, x, y + 1);
        mark_river(region, x + 1, y + 1);
        i += 1.0;
    }
}

/// `world.region.AddRiversAndLakes`.
fn add_rivers_and_lakes(context: &mut Context<'_>) {
    let shore: Vec<i32> = context
        .region
        .points()
        .filter(|p| p.shore())
        .map(|p| p.index)
        .collect();
    let mut builders = Vec::new();
    for index in shore {
        let best_angle = find_best_starting_angle(&context.region, &mut context.random, index);
        if !best_angle.is_nan() {
            let rng = XoroshiroRandom::new(context.random.next_long());
            let p = context.region.at_index(index).unwrap();
            builders.push(tfc_river::Builder::new(
                rng,
                f64::from(p.x as f32 + 0.5f32),
                f64::from(p.z as f32 + 0.5f32),
                f64::from(best_angle),
                f64::from(RIVER_LENGTH),
                RIVER_DEPTH,
                f64::from(RIVER_FEATHER),
            ));
        }
    }
    let edges = {
        let region = &context.region;
        let vertex_point = |v: &tfc_river::Vertex| {
            region
                .at(tfc_river::java_round(v.x), tfc_river::java_round(v.y))
                .map(|p| (p.land(), p.distance_to_ocean))
        };
        let legal = |prev: &tfc_river::Vertex, vertex: &tfc_river::Vertex| match (
            vertex_point(prev),
            vertex_point(vertex),
        ) {
            (Some(prev_point), Some(new_point)) => {
                new_point.0
                    && new_point.1 >= prev_point.1
                    && i32::from(new_point.1) >= 3.min(prev.distance / 2)
            }
            _ => false,
        };
        tfc_river::build_rivers(builders, &legal)
    };
    let mut rivers: Vec<RiverEdge> = edges
        .iter()
        .map(|e| RiverEdge::new(e, &mut context.random))
        .collect();
    if !rivers.is_empty() {
        annotate_river_grid_scale(&mut context.region, &mut context.random, &mut rivers);
    }
    context.region.rivers = Some(rivers);
}

fn annotate_river_grid_scale(
    region: &mut Region,
    random: &mut XoroshiroRandom,
    rivers: &mut [RiverEdge],
) {
    let mut by_source = HashMap::new();
    for (i, edge) in rivers.iter().enumerate() {
        by_source.insert(edge.source.key(), i);
    }
    for i in 0..rivers.len() {
        let drain = by_source.get(&rivers[i].drain.key()).copied();
        rivers[i].drain_edge = drain;
        if let Some(d) = drain {
            rivers[d].source_edge = true;
        }
    }
    for i in 0..rivers.len() {
        if !rivers[i].source_edge {
            let mut width = tfc_river::MIN_WIDTH;
            let mut at = Some(i);
            while let Some(e) = at {
                rivers[e].width = rivers[e].width.max(width);
                at = rivers[e].drain_edge;
                width = (width + 2).min(tfc_river::MAX_WIDTH);
            }
        }
    }
    for i in 0..rivers.len() {
        if !rivers[i].source_edge && random.next_int_bound(3).unwrap() == 0 {
            place_lake_near(region, &rivers[i], 1, 1);
            place_lake_near(region, &rivers[i], -1, 1);
            place_lake_near(region, &rivers[i], 1, -1);
            place_lake_near(region, &rivers[i], -1, -1);
        } else if rivers[i].width > tfc_river::MIN_VALLEY_WIDTH {
            annotate_river_valley(region, &rivers[i]);
        }
    }
}

pub(super) fn lerp_f(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

/// `AnnotateClimate`.
fn annotate_climate(context: &mut Context<'_>) {
    let g = context.generator;
    for p in context.region.data.iter_mut().flatten() {
        let (x, z) = (f64::from(p.x), f64::from(p.z));
        p.temperature = g.temperature_noise.noise(x, z) as f32;
        p.rainfall = g.rainfall_noise.noise(x, z) as f32;
        let bias = if p.land() {
            let potential = clamped_map_f(f32::from(p.distance_to_edge), 2.0, 6.0, 0.0, 1.0);
            let ocean_proximity = clamped_map_f(f32::from(p.distance_to_ocean), 2.0, 6.0, 0.0, 1.0);
            potential.min(ocean_proximity)
        } else {
            0.0
        };
        p.rainfall_variance = clamped_map_f(
            f32::from(p.distance_to_west_coast) + g.rainfall_variance_noise.noise(x, z) as f32,
            0.0,
            80.0,
            -1.0,
            1.0,
        );
        let bias_target_temperature = lerp_f(bias, 5.0, p.temperature);
        let bias_target_rainfall = lerp_f(bias, (p.rainfall + 350.0f32).min(500.0f32), p.rainfall);
        let temp_delta = clamped_map_f(
            g.oceanic_influence_noise.noise(x, z) as f32,
            -0.8,
            0.9,
            -0.07,
            0.23,
        );
        let old_temp = p.temperature;
        p.temperature = lerp_f(temp_delta, old_temp, bias_target_temperature);
        let rain_delta = clamped_map_f(p.temperature - old_temp, -2.0, 2.0, 0.0, 0.25);
        p.rainfall = lerp_f(rain_delta, p.rainfall, bias_target_rainfall).clamp(0.0, 500.0);
        let edge_bias_scale = clamped_map_f(f32::from(p.distance_to_edge), 0.0, 12.0, 1.0, 0.0);
        p.rainfall_variance = lerp_f(edge_bias_scale, p.rainfall_variance, 0.0);
        p.rainfall_variance = p.rainfall_variance.clamp(-1.0, 1.0);
    }
}

fn triangle(frequency: f64, value: f64) -> f64 {
    let floor = {
        let v = frequency * value + f64::from(0.75f32);
        let i = v as i32;
        if f64::from(i) > v { i - 1 } else { i }
    };
    (4.0 * frequency * value + 1.0 - 4.0 * f64::from(floor)).abs() - 1.0
}
fn base_noise(axis_is_x: bool, scale: f32, constant: f32) -> Noise2D {
    let frequency = f64::from(units::GRID_WIDTH_IN_BLOCK as f32 / (2.0f32 * scale));
    if scale == 0.0 {
        Noise2D::constant(f64::from(constant))
    } else if axis_is_x {
        Noise2D::new(move |x, _| triangle(frequency, x))
    } else {
        Noise2D::new(move |_, z| triangle(frequency, z))
    }
}

/// `world.region.RegionGenerator`.
pub struct RegionGenerator {
    pub settings: Settings,
    pub cell_noise: Cellular2D,
    pub continent_noise: Noise2D,
    pub temperature_noise: Noise2D,
    pub oceanic_influence_noise: Noise2D,
    pub rainfall_noise: Noise2D,
    pub rainfall_variance_noise: Noise2D,
    pub hot_spot_age_noise: Noise2D,
    pub hot_spot_intensity_noise: Noise2D,
    pub plate_region_noise: Cellular2D,
    pub rock_layers: RockLayerSettings,
    pub biome_area: Mutex<Area>,
    rock_area: Mutex<Area>,
    seed: i64,
    /// Seeds `RegionChunkDataGenerator` draws: rock layer, layer height, skew X, skew Z, forest layer.
    pub chunk_data_seeds: [i64; 5],
    regions: Mutex<HashMap<(i32, i32), Arc<Region>>>,
}
impl RegionGenerator {
    pub fn new(settings: Settings, mut seed: Seed) -> Self {
        Self::from_seed(settings, &mut seed)
    }
    /// `new RegionGenerator(settings, seed)`: draws from the shared `Seed` stream in Java's
    /// order (noise seeds, the two uniform layers, then the chunk data generator's five).
    pub fn from_seed(settings: Settings, seed: &mut Seed) -> Self {
        let level_seed = seed.seed();
        let cell_noise = continent_cell_noise(1.0, level_seed);
        let min = settings.continentalness * 10.0f32 - 2.5f32;
        let continent_noise = cell_noise
            .clone()
            .then(|c| 1.0 - c.f1 / (f64::from(0.37f32) + c.f2))
            .lazy_product(Noise2D::from_simplex(
                OpenSimplex2D::new(seed.next())
                    .spread(f64::from(0.24f32))
                    .scaled(f64::from(min), f64::from(8.7f32))
                    .octaves(4),
            ));
        let temperature_noise = base_noise(
            false,
            settings.temperature_scale as f32,
            settings.temperature_constant,
        )
        .scaled(-20.0, 30.0)
        .add(Noise2D::from_simplex(
            OpenSimplex2D::new(seed.next())
                .octaves(2)
                .spread(f64::from(0.15f32))
                .scaled(-3.0, 3.0),
        ));
        let oceanic_influence_noise =
            Noise2D::from_simplex(OpenSimplex2D::new(seed.next()).spread(f64::from(0.02f32)));
        let rainfall_noise = base_noise(
            true,
            settings.rainfall_scale as f32,
            settings.rainfall_constant,
        )
        .scaled(0.0, 500.0)
        .add(Noise2D::from_simplex(
            OpenSimplex2D::new(seed.next())
                .octaves(2)
                .spread(f64::from(0.15f32))
                .scaled(-80.0, 40.0),
        ));
        let rainfall_variance_noise = Noise2D::from_simplex(
            OpenSimplex2D::new(seed.next())
                .octaves(2)
                .spread(f64::from(0.1f32))
                .scaled(0.0, 20.0),
        );
        let mut next_seed = || seed.next();
        let biome_area = create_uniform_layer(&mut next_seed, 2)();
        let rock_area = create_uniform_layer(&mut next_seed, 3)();
        // RegionChunkDataGenerator: rock layer, layer height, two skew noises, forest layer.
        let chunk_data_seeds = [
            seed.next(),
            seed.next(),
            seed.next(),
            seed.next(),
            seed.next(),
        ];
        Self {
            settings,
            cell_noise,
            continent_noise,
            temperature_noise,
            oceanic_influence_noise,
            rainfall_noise,
            rainfall_variance_noise,
            hot_spot_age_noise: hot_spot_age(level_seed).spread(128.0),
            hot_spot_intensity_noise: hot_spot_intensity(level_seed).spread(128.0),
            plate_region_noise: plate_regions(level_seed).spread(128.0),
            rock_layers: RockLayerSettings::tfc_default(),
            biome_area: Mutex::new(biome_area),
            rock_area: Mutex::new(rock_area),
            seed: level_seed,
            chunk_data_seeds,
            regions: Mutex::new(HashMap::new()),
        }
    }
    pub fn seed(&self) -> i64 {
        self.seed
    }
    pub fn sample_cell(&self, grid_x: i32, grid_z: i32) -> Cell {
        self.cell_noise.cell(f64::from(grid_x), f64::from(grid_z))
    }
    /// Positive near diverging tectonic boundaries, negative near converging ones.
    pub fn get_divergence(&self, p: &Point) -> f64 {
        let this = self.cell_noise.cell(f64::from(p.x), f64::from(p.z));
        let adjacent = self.cell_noise.cell(this.nx, this.ny);
        let vx = |n: f64| hash_double(n, 95274);
        let vy = |n: f64| hash_double(n, 894824);
        let div = |x0: f64, x1: f64, v0: f64, v1: f64| {
            let dv = v1 - v0;
            if x0 < x1 { dv } else { -dv }
        };
        div(this.x, adjacent.x, vx(this.noise), vx(adjacent.noise))
            + div(this.y, adjacent.y, vy(this.noise), vy(adjacent.noise))
    }
    /// 1 inside the finite-continent area, falling to 0 over 1.2x its scale.
    pub fn continent_factor(&self, p: &Point) -> f32 {
        if !self.settings.finite_continents {
            return 1.0;
        }
        let sx = self.settings.rainfall_scale;
        let sz = self.settings.temperature_scale;
        let fx = if sx == 0 {
            1.0
        } else {
            clamped_map_f(
                units::grid_to_block(p.x).abs() as f32,
                sx as f32,
                1.2f32 * sx as f32,
                1.0,
                0.0,
            )
        };
        let fz = if sz == 0 {
            1.0
        } else {
            clamped_map_f(
                (units::grid_to_block(p.z) as f32 - 0.5f32 * sz as f32).abs(),
                sz as f32,
                1.2f32 * sz as f32,
                1.0,
                0.0,
            )
        };
        fx.min(fz)
    }
    pub fn continent_cell_noise(&self, scale_factor: f32) -> Cellular2D {
        continent_cell_noise(scale_factor, self.seed)
    }
    pub fn get_or_create_region(&self, grid_x: i32, grid_z: i32) -> Arc<Region> {
        let cell = self.sample_cell(grid_x, grid_z);
        let key = (
            (cell.x as f32).to_bits() as i32,
            (cell.y as f32).to_bits() as i32,
        );
        if let Some(r) = self.regions.lock().unwrap().get(&key) {
            return r.clone();
        }
        let region = Arc::new(self.create_region(cell));
        self.regions.lock().unwrap().insert(key, region.clone());
        region
    }
    pub fn create_region(&self, cell: Cell) -> Region {
        let region_seed =
            self.seed ^ i64::from((cell.noise as f32).to_bits() as i32).wrapping_mul(7_189_234_123);
        let mut context = Context {
            generator: self,
            random: XoroshiroRandom::new(region_seed),
            region: Region::new(&cell),
            region_cell: cell,
        };
        for task in TASKS {
            task(&mut context);
        }
        context.region
    }
}

/// `BiomeNoise.continentCellNoise`.
pub fn continent_cell_noise(scale_factor: f32, seed: i64) -> Cellular2D {
    Cellular2D::with_sample(seed, 2)
        .spread(f64::from(scale_factor / units::CELL_WIDTH_IN_GRID as f32))
}

/// `BiomeNoise.plateRegions`.
pub fn plate_regions(seed: i64) -> Cellular2D {
    Cellular2D::new(seed).spread(f64::from(
        0.005_906_25f32 / units::CELL_WIDTH_IN_GRID as f32,
    ))
}
/// `BiomeNoise.activeHotSpots`.
fn active_hot_spots(seed: i64) -> Noise2D {
    Noise2D::from_simplex(OpenSimplex2D::new(seed))
        .map(|y| {
            let y = if y > 0.75 { y - 0.75 } else { 0.0 };
            y * 7.2
        })
        .octaves(3)
        .spread(0.003)
}
/// `BiomeNoise.hotSpotWarp`.
fn hot_spot_warp(
    noise_to_warp: Noise2D,
    warp_cells: Cellular2D,
    velocity_scale: i32,
    accel_scale: f64,
) -> Noise2D {
    Noise2D::new(move |x, z| {
        let ux = warp_cells.noise(x, z);
        let uz = (if (ux * 16.0).abs() % 1.0 > 0.5 {
            1.0
        } else {
            -1.0
        } * (ux * 256.0))
            % 1.0;
        let sx = if ux > 0.0 { 1 } else { -1 };
        let sz = if uz > 0.0 { 1 } else { -1 };
        let vx = (ux + f64::from(sx)) * f64::from(velocity_scale);
        let vz = (uz + f64::from(sz)) * f64::from(velocity_scale);
        let ax = -vz * accel_scale;
        let az = vx * accel_scale;
        let cell = warp_cells.cell(x, z);
        let scale = clamped_map(cell.f2 - cell.f1, 0.0, 0.002, 0.0, 1.0);
        noise_to_warp.noise(x + vx + ax, z + vz + az) * scale
    })
}
fn dormant_hot_spots(seed: i64) -> Noise2D {
    hot_spot_warp(active_hot_spots(seed), plate_regions(seed), 1024, 0.0)
        .map(|y| f64::max(y - 0.1, 0.0) * 1.111)
}
fn extinct_hot_spots(seed: i64) -> Noise2D {
    hot_spot_warp(active_hot_spots(seed), plate_regions(seed), 2048, 0.25)
        .map(|y| f64::max(y - 0.2, 0.0) * 1.25)
}
fn ancient_hot_spots(seed: i64) -> Noise2D {
    hot_spot_warp(active_hot_spots(seed), plate_regions(seed), 3072, 0.5)
        .map(|y| f64::max(y - 0.3, 0.0) * 1.4286)
}
/// `BiomeNoise.hotSpotIntensity`.
pub fn hot_spot_intensity(seed: i64) -> Noise2D {
    active_hot_spots(seed)
        .max(dormant_hot_spots(seed))
        .max(extinct_hot_spots(seed))
        .max(ancient_hot_spots(seed))
}
/// `BiomeNoise.hotSpotAge`.
pub fn hot_spot_age(seed: i64) -> Noise2D {
    let (active, young, old, oldest) = (
        active_hot_spots(seed),
        dormant_hot_spots(seed),
        extinct_hot_spots(seed),
        ancient_hot_spots(seed),
    );
    Noise2D::new(move |x, z| {
        let a = active.noise(x, z);
        let y = young.noise(x, z);
        let o = old.noise(x, z);
        let t = oldest.noise(x, z);
        if f64::max(f64::max(a, y), f64::max(o, t)) <= -0.8 {
            0.0
        } else if a > y && a > o && a > t {
            1.0
        } else if y > a && y > o && y > t {
            2.0
        } else if o > y && o > t && o > a {
            3.0
        } else if t > y && t > o && t > a {
            4.0
        } else {
            0.0
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected values come from TFC's own Region, Init, Cellular2D and noise classes run on Java 21.
    const NOISE: &[(i64, [[u64; 5]; 6])] = &[
        (
            1234567,
            [
                [
                    0x40121ed60431b903u64,
                    0x4014000000000000u64,
                    0x3a2762c0a0000000u64,
                    0x406cc00000000000u64,
                    0x4024000000000000u64,
                ],
                [
                    0x4003a51886008d3cu64,
                    0xc0306d45d7ac0000u64,
                    0xbfd8647c00000000u64,
                    0x4077ba93fbc38000u64,
                    0x402a30a24a000000u64,
                ],
                [
                    0x4014f9eb0200432du64,
                    0x403bddd774400000u64,
                    0xbfbbc577e0000000u64,
                    0x407a21d501110000u64,
                    0x401d51d4bc000000u64,
                ],
                [
                    0x401351d8f8e65b24u64,
                    0x402d1adf6ac00000u64,
                    0xbfc85089e0000000u64,
                    0x407a72f2263c0000u64,
                    0x402c5c7f9e000000u64,
                ],
                [
                    0x400e80cb979dd75fu64,
                    0x401bfe33bd8c0000u64,
                    0xbfda754aa0000000u64,
                    0x4072044775d0c000u64,
                    0x4028ab9b41000000u64,
                ],
                [
                    0x40161d7eff79ff4du64,
                    0x4029f91f8ac00000u64,
                    0xbfd25ba120000000u64,
                    0x4072a594c2dc0000u64,
                    0x40236ac3b9000000u64,
                ],
            ],
        ),
        (
            -987654321,
            [
                [
                    0x401321cdb88b0258u64,
                    0x4014000000000000u64,
                    0x3a19b593c0000000u64,
                    0x406cc00000000000u64,
                    0x4024000000000000u64,
                ],
                [
                    0x400795bc72654980u64,
                    0xc02e9f4dd6580000u64,
                    0x3f9a23b5a0000000u64,
                    0x4077bf927c038000u64,
                    0x40269c07cd000000u64,
                ],
                [
                    0x40100adcef114040u64,
                    0x4039cc60aa400000u64,
                    0x3fcc4483a0000000u64,
                    0x407a26336e410000u64,
                    0x40286f1a31000000u64,
                ],
                [
                    0x401110a4bb12eb47u64,
                    0x402eb216df000000u64,
                    0x3fc8541be0000000u64,
                    0x4079d8657b3c0000u64,
                    0x401c1e42ec000000u64,
                ],
                [
                    0x4011fc97b51cad44u64,
                    0x401b9afaab8c0000u64,
                    0x3f6311ae00000000u64,
                    0x4070b66623f8c000u64,
                    0x40207e218c000000u64,
                ],
                [
                    0x40141bf62bac4b28u64,
                    0x4029fafff3c00000u64,
                    0x3fd40e86e0000000u64,
                    0x4073add026340000u64,
                    0x4021273286000000u64,
                ],
            ],
        ),
    ];
    #[allow(clippy::type_complexity)]
    const REGIONS: &[(i64, [(i32, i32, u64, u64, i32, [i32; 4], i64, u64); 3])] = &[
        (
            1234567,
            [
                (
                    0,
                    0,
                    0x4044f203a9f72bfbu64,
                    0xc0025b3993a42c96u64,
                    10810,
                    [-30, -84, 106, 68],
                    -3542852276401754212i64,
                    0x2e7f3986126b9fb9u64,
                ),
                (
                    500,
                    -700,
                    0x4080476153b28dd8u64,
                    0xc08549f2e6d83713u64,
                    8922,
                    [434, -722, 573, -636],
                    -5377195465490694685i64,
                    0x7d448568ed4d1432u64,
                ),
                (
                    -1500,
                    2000,
                    0xc097b37ac55a2ce8u64,
                    0x409eeaa5a57104e5u64,
                    11194,
                    [-1610, 1929, -1489, 2045],
                    -6665443821611476638i64,
                    0xa0b480bad5e3de6u64,
                ),
            ],
        ),
        (
            -987654321,
            [
                (
                    0,
                    0,
                    0xc044ab7f1befb972u64,
                    0xc01c994d77e23924u64,
                    8095,
                    [-90, -48, 30, 48],
                    -8363135846671882885i64,
                    0x82f263f4aeb59b5eu64,
                ),
                (
                    500,
                    -700,
                    0x4080b211236705ebu64,
                    0xc08521b1066822e7u64,
                    12367,
                    [484, -723, 622, -600],
                    -4186306033132889055i64,
                    0xbc1bbbcb8db29c27u64,
                ),
                (
                    -1500,
                    2000,
                    0xc09774e8e01062e5u64,
                    0x409e5de406100768u64,
                    8987,
                    [-1574, 1873, -1453, 2004],
                    -1192852437222301032i64,
                    0x49d39c058225df9u64,
                ),
            ],
        ),
    ];

    #[test]
    fn region_noise_and_init_match_java_bit_for_bit() {
        let pts = [
            (0.0, 0.0),
            (37.0, -91.0),
            (-250.0, 400.0),
            (1000.0, -3000.0),
            (12.5, 7.25),
            (-600.0, -600.0),
        ];
        for ((seed, rows), (_, regions)) in NOISE.iter().zip(REGIONS) {
            let g = RegionGenerator::new(Settings::default(), Seed::of(*seed));
            for (p, want) in pts.iter().zip(rows) {
                let got = [
                    g.continent_noise.noise(p.0, p.1),
                    g.temperature_noise.noise(p.0, p.1),
                    g.oceanic_influence_noise.noise(p.0, p.1),
                    g.rainfall_noise.noise(p.0, p.1),
                    g.rainfall_variance_noise.noise(p.0, p.1),
                ]
                .map(f64::to_bits);
                assert_eq!(got, *want, "seed {seed} at {p:?}");
            }
            for (gx, gz, cx, cy, n, bx, h, rnd) in regions {
                let cell = g.sample_cell(*gx, *gz);
                assert_eq!((cell.x.to_bits(), cell.y.to_bits()), (*cx, *cy));
                let noise = cell.noise;
                let region = g.create_region(cell);
                let (mut hh, mut c) = (0i64, 0);
                for x in region.min_x..=region.max_x {
                    for z in region.min_z..=region.max_z {
                        if region.at(x, z).is_some() {
                            c += 1;
                            hh = hh
                                .wrapping_mul(31)
                                .wrapping_add(i64::from(x) * 7919 + i64::from(z));
                        }
                    }
                }
                assert_eq!(
                    (region.points().count() as i32, c, hh),
                    (*n, *n, *h),
                    "seed {seed} region {gx},{gz}"
                );
                assert_eq!(
                    [region.min_x, region.min_z, region.max_x, region.max_z],
                    *bx
                );
                let region_seed =
                    *seed ^ i64::from((noise as f32).to_bits() as i32).wrapping_mul(7_189_234_123);
                assert_eq!(XoroshiroRandom::new(region_seed).next_long() as u64, *rnd);
            }
        }
    }

    /// (finite, seed, grid x, grid z, hashes after edge / boundary / continents / flood fill, land counts)
    #[allow(clippy::type_complexity)]
    const STAGES: &[(bool, i64, i32, i32, i64, i64, i64, i32, i64, i32)] = &[
        (
            false,
            1234567,
            0,
            0,
            8801887011935692435,
            -4042370855878637932,
            -2178752459619497530,
            4966,
            3589755349602151592,
            5054,
        ),
        (
            false,
            1234567,
            500,
            -700,
            3971980630562074200,
            4538505417976577303,
            7202536876244830925,
            3593,
            8564790516587154549,
            3755,
        ),
        (
            false,
            1234567,
            -1500,
            2000,
            350266915201415230,
            -8678012991439069842,
            1238781151289884200,
            4656,
            5728644175492401640,
            4888,
        ),
        (
            false,
            1234567,
            3000,
            3100,
            5080070333284392267,
            188690744156804484,
            2385726837992297569,
            3417,
            8751648764249905597,
            3823,
        ),
        (
            true,
            424242,
            0,
            0,
            1729164461185720363,
            -3234612869276073369,
            -6817083990125069218,
            2708,
            6897044213247497857,
            2861,
        ),
        (
            true,
            424242,
            500,
            -700,
            5779417814977507328,
            -7196001143640071081,
            -7794953735411783084,
            0,
            -7794953735411783084,
            0,
        ),
        (
            true,
            424242,
            -1500,
            2000,
            -1106393768813654471,
            58291702036743492,
            -1059563736794366650,
            0,
            -1059563736794366650,
            0,
        ),
        (
            true,
            424242,
            3000,
            3100,
            5689658170217514519,
            6860338909623455931,
            -6165127884099058944,
            0,
            -6165127884099058944,
            0,
        ),
    ];

    #[test]
    fn region_steps_through_small_ocean_fill_match_java_bit_for_bit() {
        for (finite, seed, gx, gz, h1, h2, h3, land, h4, land2) in STAGES {
            let settings = Settings {
                finite_continents: *finite,
                ..Settings::default()
            };
            let g = RegionGenerator::new(settings, Seed::of(*seed));
            let cell = g.sample_cell(*gx, *gz);
            let mut ctx = Context {
                generator: &g,
                random: XoroshiroRandom::new(0),
                region: Region::new(&cell),
                region_cell: cell,
            };
            init(&mut ctx);
            annotate_distance_to_cell_edge(&mut ctx);
            let mut h = 0i64;
            for p in ctx.region.points() {
                h = h
                    .wrapping_mul(31)
                    .wrapping_add(i64::from(p.index) * 131 + i64::from(p.distance_to_edge));
            }
            assert_eq!(h, *h1, "edge {gx},{gz}");
            annotate_boundary_types(&mut ctx);
            let mut h = 0i64;
            for p in ctx.region.points() {
                h = h
                    .wrapping_mul(31)
                    .wrapping_add(i64::from(p.divergence.to_bits() as i32));
            }
            assert_eq!(h, *h2, "divergence {gx},{gz}");
            let hash = |region: &Region| {
                let (mut h, mut n) = (0i64, 0);
                for p in region.points() {
                    h = h
                        .wrapping_mul(31)
                        .wrapping_add(i64::from(p.ocean_depth) * 2 + i64::from(p.land()));
                    n += i32::from(p.land());
                }
                (h, n)
            };
            add_continents_and_set_ocean_depths(&mut ctx);
            assert_eq!(hash(&ctx.region), (*h3, *land), "continents {gx},{gz}");
            flood_fill_small_oceans(&mut ctx);
            assert_eq!(hash(&ctx.region), (*h4, *land2), "flood fill {gx},{gz}");
        }
    }

    #[allow(clippy::type_complexity)]
    const HOT_SPOTS: &[(i64, [(i32, i32); 6], [(u64, u64); 6])] = &[
        (
            1234567,
            [
                (-30000, -29023),
                (-30000, -28046),
                (-30000, -27069),
                (-30000, -13391),
                (-30000, -9483),
                (-30000, -6552),
            ],
            [
                (0x3fc3529140000000, 0x4008000000000000),
                (0x3fd17512b3333333, 0x3ff0000000000000),
                (0x3fc091f5cccccccd, 0x3ff0000000000000),
                (0x3fbc0c539999999a, 0x3ff0000000000000),
                (0x3fc5692066666667, 0x3ff0000000000000),
                (0x3fbafd81e6666667, 0x3ff0000000000000),
            ],
        ),
        (
            424242,
            [
                (-30000, -16322),
                (-30000, -10460),
                (-30000, 6149),
                (-30000, 24712),
                (-30000, 25689),
                (-30000, 27643),
            ],
            [
                (0x3fd5ca1ba0000000, 0x4008000000000000),
                (0x3fbfeab4cccccccd, 0x3ff0000000000000),
                (0x3fd581c5d999999a, 0x3ff0000000000000),
                (0x3fc29cb49999999a, 0x3ff0000000000000),
                (0x3fcf7d5a0d9e83e4, 0x4000000000000000),
                (0x3fcc447980000000, 0x3ff0000000000000),
            ],
        ),
    ];
    #[allow(clippy::type_complexity)]
    const LATE: &[(bool, i64, i32, i32, i64, i64, i64, i64, i64, u64)] = &[
        (
            false,
            1234567,
            -6000,
            -6000,
            -5579794246029765179,
            7432601116955863477,
            -6804809379254304221,
            -7055001567541326390,
            -8319610824847624215,
            0xb453429120f654f9,
        ),
        (
            false,
            1234567,
            -6000,
            -5856,
            -3158453489982493983,
            -1144348036859734720,
            4014663152779978766,
            8017502055088503537,
            -8483754391256735532,
            0xf1082a10144582cb,
        ),
        (
            false,
            1234567,
            -6000,
            -5712,
            -5528958366822614690,
            -3547411698200997294,
            8611930644968533049,
            -7071475422788490024,
            -4378550525004590465,
            0x985c8a74840347dc,
        ),
        (
            false,
            1234567,
            -6000,
            -5568,
            -9049911615908111438,
            -7097876507635391582,
            8437617567971603563,
            2434242640694005691,
            -890416570314085080,
            0x83c0ef4b3f85ea47,
        ),
        (
            true,
            424242,
            -240,
            48,
            -5512779981964214069,
            8154879693455154912,
            -1697907399963111089,
            3111332472940026984,
            -4490563100698021882,
            0xbcdf955801a67693,
        ),
        (
            true,
            424242,
            -96,
            -96,
            -2011136054650287095,
            7548788845611126772,
            -2653497853406903191,
            -3055349910670274246,
            384469941630972344,
            0x8272e74c6a523026,
        ),
        (
            true,
            424242,
            -96,
            48,
            -9005583320650176266,
            -7896786253510015924,
            8388786006587604256,
            4292980576448061983,
            7038898095953979141,
            0x1adce05c9a3058f8,
        ),
        (
            true,
            424242,
            -96,
            192,
            -5078291922393401334,
            4847955200440032195,
            -7823309271436219681,
            -5346836401964210995,
            4557167620668076910,
            0x7665dd59af5a3fba,
        ),
    ];

    #[test]
    fn hotspot_noise_matches_java_bit_for_bit() {
        for (seed, pts, vals) in HOT_SPOTS {
            let g = RegionGenerator::new(Settings::default(), Seed::of(*seed));
            for (p, want) in pts.iter().zip(vals) {
                let got = (
                    g.hot_spot_intensity_noise
                        .noise(f64::from(p.0), f64::from(p.1))
                        .to_bits(),
                    g.hot_spot_age_noise
                        .noise(f64::from(p.0), f64::from(p.1))
                        .to_bits(),
                );
                assert_eq!(got, *want, "seed {seed} at {p:?}");
            }
        }
    }

    #[test]
    fn region_steps_through_west_coast_match_java_bit_for_bit() {
        for (finite, seed, gx, gz, h0, h1, h2, h3, h4, rnd) in LATE {
            let settings = Settings {
                finite_continents: *finite,
                ..Settings::default()
            };
            let g = RegionGenerator::new(settings, Seed::of(*seed));
            let cell = g.sample_cell(*gx, *gz);
            let mut ctx = Context {
                generator: &g,
                random: XoroshiroRandom::new(
                    *seed
                        ^ i64::from((cell.noise as f32).to_bits() as i32)
                            .wrapping_mul(7_189_234_123),
                ),
                region: Region::new(&cell),
                region_cell: cell,
            };
            let steps: [(RegionTask, i64); 10] = [
                (init, 0),
                (annotate_distance_to_cell_edge, 0),
                (annotate_boundary_types, 0),
                (add_continents_and_set_ocean_depths, 0),
                (flood_fill_small_oceans, 0),
                (add_hotspots, *h0),
                (annotate_distance_to_ocean, *h1),
                (annotate_distance_to_deep_ocean, *h2),
                (annotate_base_land_height, *h3),
                (annotate_distance_to_west_coast, *h4),
            ];
            for (i, (task, want)) in steps.iter().enumerate() {
                task(&mut ctx);
                if i < 5 {
                    continue;
                }
                let mut h = 0i64;
                for p in ctx.region.points() {
                    let term = match i {
                        5 => i64::from(p.hot_spot_age) * 17 + i64::from(p.land()),
                        6 => i64::from(p.distance_to_ocean) * 3 + i64::from(p.shore()),
                        7 => i64::from(p.distance_to_deep_ocean),
                        8 => i64::from(p.base_land_height) * 7 + i64::from(p.distance_to_land),
                        _ => i64::from(p.distance_to_west_coast),
                    };
                    h = h.wrapping_mul(31).wrapping_add(term);
                }
                assert_eq!(h, *want, "step {i} at {gx},{gz}");
            }
            assert_eq!(
                ctx.random.next_long() as u64,
                *rnd,
                "random stream at {gx},{gz}"
            );
        }
    }

    #[allow(clippy::type_complexity)]
    const MOUNTAINS: &[(bool, i64, i32, i32, i64, i64, i64, u64)] = &[
        (
            false,
            1234567,
            -9000,
            -9000,
            -6685664866864675414,
            -821046793365411027,
            -7023159677019987856,
            0x16245261fc1b6b35,
        ),
        (
            false,
            1234567,
            -9000,
            -8700,
            7789548542466711711,
            1975891290751726273,
            841641281407568799,
            0x69b2f0fd15d2c9dc,
        ),
        (
            false,
            1234567,
            -9000,
            -8400,
            1814225827355951068,
            -4108763548864210853,
            513646170547340218,
            0xb3e45337a1cdff1,
        ),
        (
            true,
            424242,
            0,
            0,
            -4515734560919492701,
            -3103403077391170032,
            -9057317479259111104,
            0x698c8be56e187ec3,
        ),
    ];

    #[test]
    fn region_steps_through_biome_altitude_match_java_bit_for_bit() {
        for (finite, seed, gx, gz, h0, h1, h2, rnd) in MOUNTAINS {
            let settings = Settings {
                finite_continents: *finite,
                ..Settings::default()
            };
            let g = RegionGenerator::new(settings, Seed::of(*seed));
            let cell = g.sample_cell(*gx, *gz);
            let mut ctx = Context {
                generator: &g,
                random: XoroshiroRandom::new(
                    *seed
                        ^ i64::from((cell.noise as f32).to_bits() as i32)
                            .wrapping_mul(7_189_234_123),
                ),
                region: Region::new(&cell),
                region_cell: cell,
            };
            let hash = |region: &Region, mode: i32| {
                let mut h = 0i64;
                for p in region.points() {
                    let term = match mode {
                        0 => {
                            i64::from(p.mountain())
                                + i64::from(p.volcanic()) * 2
                                + i64::from(p.coastal_mountain()) * 4
                                + i64::from(p.barrier_island()) * 8
                                + i64::from(p.ocean_depth) * 16
                                + i64::from(p.land()) * 64
                        }
                        1 => {
                            i64::from(p.island())
                                + i64::from(p.land()) * 2
                                + i64::from(p.distance_to_ocean) * 4
                                + i64::from(p.ocean_depth) * 512
                        }
                        _ => i64::from(p.biome_altitude),
                    };
                    h = h.wrapping_mul(31).wrapping_add(term);
                }
                h
            };
            for task in &TASKS[..10] {
                task(&mut ctx);
            }
            for (i, want) in [h0, h1, h2].iter().enumerate() {
                TASKS[10 + i](&mut ctx);
                assert_eq!(
                    hash(&ctx.region, i as i32),
                    **want,
                    "step {} at {gx},{gz}",
                    10 + i
                );
            }
            assert_eq!(
                ctx.random.next_long() as u64,
                *rnd,
                "random stream at {gx},{gz}"
            );
        }
    }

    #[allow(clippy::type_complexity)]
    const CLIMATE: &[(bool, i64, i32, i32, i64, u64)] = &[
        (
            false,
            1234567,
            0,
            0,
            -1637862394967781820,
            0x7e50c547934ffd08,
        ),
        (
            false,
            1234567,
            500,
            -700,
            448990772756294801,
            0x6bf34f4f2f47cf53,
        ),
        (
            false,
            1234567,
            -1500,
            2000,
            1079012402734549046,
            0x8b33bcbeaeabf15,
        ),
        (
            false,
            1234567,
            3000,
            3100,
            -5694591904033129036,
            0x9d2851c33ab3d3ca,
        ),
        (true, 424242, 0, 0, -8539849837496704975, 0x698c8be56e187ec3),
        (
            true,
            424242,
            500,
            -700,
            -374646657737564930,
            0xef2beaadd7977f53,
        ),
        (
            true,
            424242,
            -1500,
            2000,
            -5117638192096930038,
            0xc00f09c1f830c205,
        ),
        (
            true,
            424242,
            3000,
            3100,
            -7452837881596463945,
            0xf3f204d951ec2e51,
        ),
    ];

    #[test]
    fn region_climate_matches_java_bit_for_bit() {
        for (finite, seed, gx, gz, want, rnd) in CLIMATE {
            let settings = Settings {
                finite_continents: *finite,
                ..Settings::default()
            };
            let g = RegionGenerator::new(settings, Seed::of(*seed));
            let cell = g.sample_cell(*gx, *gz);
            let mut ctx = Context {
                generator: &g,
                random: XoroshiroRandom::new(
                    *seed
                        ^ i64::from((cell.noise as f32).to_bits() as i32)
                            .wrapping_mul(7_189_234_123),
                ),
                region: Region::new(&cell),
                region_cell: cell,
            };
            for task in &TASKS[..13] {
                task(&mut ctx);
            }
            assert_eq!(
                ctx.random.next_long() as u64,
                *rnd,
                "random stream at {gx},{gz}"
            );
            annotate_climate(&mut ctx);
            let mut h = 0i64;
            for p in ctx.region.points() {
                h = h
                    .wrapping_mul(31)
                    .wrapping_add(i64::from(p.temperature.to_bits() as i32))
                    .wrapping_add(7 * i64::from(p.rainfall.to_bits() as i32))
                    .wrapping_add(13 * i64::from(p.rainfall_variance.to_bits() as i32));
            }
            assert_eq!(h, *want, "climate at {gx},{gz}");
        }
    }

    #[allow(clippy::type_complexity)]
    const ROCKS: &[(bool, i64, i32, i32, i64)] = &[
        (false, 1234567, 0, 0, -6422218088494182056),
        (false, 1234567, 500, -700, -4953473573441560208),
        (false, 1234567, -1500, 2000, 5409171330199893019),
        (false, 1234567, 3000, 3100, 6899528101689561489),
        (true, 424242, 0, 0, 2768334151548906574),
        (true, 424242, 500, -700, 3494099820131058868),
        (true, 424242, -1500, 2000, -8491928931170125088),
        (true, 424242, 3000, 3100, -2463185668008304748),
    ];

    #[test]
    fn region_rocks_and_karst_match_java_bit_for_bit() {
        for (finite, seed, gx, gz, want) in ROCKS {
            let settings = Settings {
                finite_continents: *finite,
                ..Settings::default()
            };
            let g = RegionGenerator::new(settings, Seed::of(*seed));
            let region = g.create_region(g.sample_cell(*gx, *gz));
            let mut h = 0i64;
            for p in region.points() {
                h = h.wrapping_mul(31).wrapping_add(
                    i64::from(p.rock)
                        + if p.is_surface_rock_karst {
                            1_000_003
                        } else {
                            0
                        },
                );
            }
            assert_eq!(h, *want, "rocks at {gx},{gz}");
        }
    }

    #[allow(clippy::type_complexity)]
    const RIVERS: &[(bool, i64, i32, i32, usize, [i64; 5], u64)] = &[
        (
            false,
            1234567,
            0,
            0,
            657,
            [
                5591603287693631024,
                6301721993817791609,
                -4405376875865406903,
                -862664798135499466,
                9214992022767573262,
            ],
            0xd6793746fffa3c19,
        ),
        (
            false,
            1234567,
            500,
            -700,
            492,
            [
                -7079790839055680377,
                7061014193211488291,
                7384602823298777815,
                8483504600276582734,
                -5053019004945317821,
            ],
            0x7e59e253786eef18,
        ),
        (
            false,
            1234567,
            -1500,
            2000,
            550,
            [
                -7016540324070130595,
                -5326560318002150259,
                3454734023446608948,
                -5817291665145391654,
                5947913200226729504,
            ],
            0x9b717047036beb54,
        ),
        (
            false,
            1234567,
            3000,
            3100,
            484,
            [
                1795403487339802010,
                1408350833281461361,
                -422701943195632141,
                -1256102399084681528,
                2327622954391074094,
            ],
            0x1598f9ec5f2c2a58,
        ),
        (
            false,
            1234567,
            -4000,
            -2500,
            708,
            [
                4622019864549884702,
                5439426534301868722,
                -3543394956660456354,
                -3376259459370052420,
                4867601333064814453,
            ],
            0x9d010f3893ebffc4,
        ),
        (
            false,
            1234567,
            7000,
            900,
            535,
            [
                2287029867404348139,
                4860346365930312129,
                7203955419756231226,
                -4826020804448741094,
                5091057111746216976,
            ],
            0xfed24bb1df2845c1,
        ),
        (
            true,
            424242,
            0,
            0,
            406,
            [
                -2900019002067653012,
                -3397339214544765957,
                -1265743968639731696,
                2211817328962482054,
                -7597360663228419981,
            ],
            0x71bc7a1b0b32d853,
        ),
        (
            true,
            424242,
            500,
            -700,
            0,
            [0, 7105090287618768127, 0, 0, 0],
            0xef2beaadd7977f53,
        ),
        (
            true,
            424242,
            -1500,
            2000,
            0,
            [0, 5072050579464521376, 0, 0, 0],
            0xc00f09c1f830c205,
        ),
        (
            true,
            424242,
            3000,
            3100,
            0,
            [0, 5236160752036253856, 0, 0, 0],
            0xf3f204d951ec2e51,
        ),
        (
            true,
            424242,
            -4000,
            -2500,
            0,
            [0, 4197559540469839878, 0, 0, 0],
            0xeca9e3c9d892dadf,
        ),
        (
            true,
            424242,
            7000,
            900,
            0,
            [0, 3676298798458626907, 0, 0, 0],
            0x8bef334b7aad8c4c,
        ),
    ];

    fn java_double_hash(d: f64) -> i64 {
        let b = d.to_bits();
        i64::from((b ^ (b >> 32)) as u32 as i32)
    }

    #[test]
    fn region_rivers_and_lakes_match_java_bit_for_bit() {
        for (finite, seed, gx, gz, n, want, rnd) in RIVERS {
            let settings = Settings {
                finite_continents: *finite,
                ..Settings::default()
            };
            let g = RegionGenerator::new(settings, Seed::of(*seed));
            let cell = g.sample_cell(*gx, *gz);
            let mut ctx = Context {
                generator: &g,
                random: XoroshiroRandom::new(
                    *seed
                        ^ i64::from((cell.noise as f32).to_bits() as i32)
                            .wrapping_mul(7_189_234_123),
                ),
                region: Region::new(&cell),
                region_cell: cell,
            };
            for task in &TASKS[..17] {
                task(&mut ctx);
            }
            let (mut hf, mut hr, mut hv, mut hw, mut hs) = (0i64, 0i64, 0i64, 0i64, 0i64);
            for p in ctx.region.points() {
                hf = hf
                    .wrapping_mul(31)
                    .wrapping_add(i64::from(p.river()) + 2 * i64::from(p.lake()));
                hr = hr
                    .wrapping_mul(31)
                    .wrapping_add(i64::from(p.rainfall.to_bits() as i32));
            }
            let rivers = ctx.region.rivers.as_ref().unwrap();
            assert_eq!(rivers.len(), *n, "river count at {gx},{gz}");
            for e in rivers {
                let t = java_double_hash(e.source.x)
                    .wrapping_add(i64::from(
                        7i32.wrapping_mul(java_double_hash(e.source.y) as i32),
                    ))
                    .wrapping_add(i64::from(
                        13i32.wrapping_mul(java_double_hash(e.drain.x) as i32),
                    ))
                    .wrapping_add(i64::from(
                        17i32.wrapping_mul(java_double_hash(e.drain.y) as i32),
                    ))
                    .wrapping_add(19 * i64::from(e.source.distance));
                hv = hv.wrapping_mul(31).wrapping_add(t);
                hw = hw
                    .wrapping_mul(31)
                    .wrapping_add(i64::from(e.width) + if e.source_edge { 1000 } else { 0 });
                for d in &e.fractal.segments {
                    hs = hs.wrapping_mul(31).wrapping_add(java_double_hash(*d));
                }
            }
            assert_eq!([hf, hr, hv, hw, hs], *want, "rivers at {gx},{gz}");
            assert_eq!(
                ctx.random.next_long() as u64,
                *rnd,
                "random stream at {gx},{gz}"
            );
        }
    }
}
