//! TerraFirmaCraft's shore terrain shapes (`ShoreNoise`, `ShoreBlendType`, EUPL 1.2): each blend
//! type reshapes the height where shore biomes meet land and ocean, and carves the 3D pass.

use super::tfc_biome_ext::ShoreBlendType;
use super::tfc_biome_noise::{
    SEA_LEVEL_Y, cliff_noise, lower_terrace_noise, shoreline_tide_level_noise, upper_terrace_noise,
};
use super::tfc_noise::{Cellular2D, Noise2D, OpenSimplex2D, OpenSimplex3D, clamped_map};

const SEA: f64 = SEA_LEVEL_Y as f64;

fn f(x: f32) -> f64 {
    f64::from(x)
}
fn n(o: OpenSimplex2D) -> Noise2D {
    Noise2D::from_simplex(o)
}
fn clamp(v: f64, min: f64, max: f64) -> f64 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}
fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}
fn mth_map(v: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    lerp((v - from_min) / (from_max - from_min), to_min, to_max)
}

/// Everything a shore sampler is told about the column.
#[derive(Clone, Copy, Debug)]
pub struct ShoreColumn {
    pub height_in: f64,
    pub x: i32,
    pub z: i32,
    pub ocean_weight: f64,
    pub land_weight: f64,
    pub shore_weight: f64,
    pub this_weight: f64,
    pub shore_base_height: i32,
    pub shore_height: f64,
    pub normal_height: f64,
}

pub trait ShoreNoiseSampler: Send {
    fn set_column_and_sample_height(&mut self, column: &ShoreColumn) -> f64 {
        column.height_in
    }
    fn noise(&mut self, _y: i32, noise_in: f64) -> f64 {
        noise_in
    }
}

struct NoShore;
impl ShoreNoiseSampler for NoShore {}

fn width_function(inverted: bool, y: f64, height: f64, base_width: f64, top_width: f64) -> f64 {
    let curve = base_width + (y * y / (height * height)) * (top_width - base_width);
    if inverted {
        curve.max(top_width)
    } else {
        curve.min(top_width)
    }
}

fn simple_beach(tide_level: f64, height_in: f64, land_weight: f64, ocean_weight: f64) -> f64 {
    let land_height = tide_level + 2.0;
    let self_height = tide_level - 2.0;
    let ocean_height = tide_level - 4.0;
    if ocean_weight > 0.10 {
        clamped_map(ocean_weight, 0.1, 0.25, self_height, ocean_height)
    } else {
        let land_derived = if land_weight < 0.3 {
            mth_map(
                land_weight,
                0.3,
                0.0,
                height_in.min(land_height),
                self_height,
            )
        } else {
            clamped_map(land_weight, 0.3, 0.5, height_in.min(land_height), height_in)
        };
        clamped_map(ocean_weight, 0.0, 0.1, land_derived, self_height)
    }
}

fn simple_beach_no_land_blend(tide_level: f64, ocean_weight: f64) -> f64 {
    clamped_map(ocean_weight, 0.0, 0.5, tide_level - 1.0, tide_level - 4.0)
}

struct SandyBeach {
    tide: Noise2D,
    sand_height: f64,
    weight: f64,
}
impl ShoreNoiseSampler for SandyBeach {
    fn set_column_and_sample_height(&mut self, c: &ShoreColumn) -> f64 {
        let tide = self.tide.noise(f64::from(c.x), f64::from(c.z));
        self.sand_height = simple_beach(tide, c.height_in, c.land_weight, c.ocean_weight);
        self.weight = c.this_weight;
        self.sand_height
    }
    fn noise(&mut self, y: i32, _noise_in: f64) -> f64 {
        let y = f64::from(y);
        if y <= self.sand_height || self.weight > 0.5 {
            return 0.0;
        }
        let multiplier = clamp((y - self.sand_height) * 0.125, 0.0, 1.0);
        multiplier * mth_map(self.weight, 0.0, 0.5, 0.7, 0.0)
    }
}

struct SetbackCliffs {
    tide: Noise2D,
    bank_noise: Noise2D,
    cliff: OpenSimplex3D,
    bank_height: f64,
    land_weight: f64,
    x: i32,
    z: i32,
    y_top: f64,
}
const CLIFF_BASE_WEIGHT: f64 = 0.25;
const CLIFF_TOP_WEIGHT: f64 = 0.45;
impl ShoreNoiseSampler for SetbackCliffs {
    fn set_column_and_sample_height(&mut self, c: &ShoreColumn) -> f64 {
        let (fx, fz) = (f64::from(c.x), f64::from(c.z));
        self.x = c.x;
        self.z = c.z;
        self.land_weight = c.land_weight;
        let tide = self.tide.noise(fx, fz);
        let sand_height = simple_beach_no_land_blend(tide, c.ocean_weight);
        self.y_top = c.height_in;
        let tide_at_ocean = tide - 4.0;
        let shore_bank_height = self.bank_noise.noise(fx, fz) + sand_height;
        if c.land_weight >= CLIFF_BASE_WEIGHT {
            self.bank_height = shore_bank_height;
            return c.height_in;
        }
        self.bank_height = if c.ocean_weight > 0.0 {
            clamped_map(c.ocean_weight, 0.0, 0.25, shore_bank_height, tide_at_ocean)
        } else {
            shore_bank_height
        };
        self.bank_height = clamped_map(
            c.this_weight + c.land_weight / 2.0,
            0.5,
            1.0,
            sand_height,
            self.bank_height,
        );
        self.bank_height
    }
    fn noise(&mut self, y: i32, _noise_in: f64) -> f64 {
        let fy = f64::from(y);
        if fy <= self.bank_height {
            return 0.0;
        }
        if self.land_weight >= CLIFF_BASE_WEIGHT {
            let percent_height = clamped_map(fy, self.bank_height, self.y_top, 0.30, 1.0);
            let percent_depth = clamped_map(
                self.land_weight,
                CLIFF_BASE_WEIGHT,
                CLIFF_TOP_WEIGHT,
                0.0,
                1.0,
            );
            let edge = |input: f64| (3.0 - 6.0 * (input - 0.5).abs()).min(1.0);
            let cliff_value = edge(percent_height)
                * edge(percent_depth)
                * self.cliff.noise(f64::from(self.x), fy, f64::from(self.z));
            return (percent_height - cliff_value - percent_depth) * 3.0;
        }
        0.7
    }
}

struct Dunes {
    tide: Noise2D,
    dune: Noise2D,
}
impl ShoreNoiseSampler for Dunes {
    fn set_column_and_sample_height(&mut self, c: &ShoreColumn) -> f64 {
        let (fx, fz) = (f64::from(c.x), f64::from(c.z));
        let tide = self.tide.noise(fx, fz);
        let sand_height = simple_beach(tide, c.height_in, c.land_weight, c.ocean_weight);
        let tide_at_ocean = tide - 4.0;
        let full = self.dune.noise(fx, fz) + sand_height;
        let dune_height = if c.ocean_weight > 0.0 {
            clamped_map(c.ocean_weight, 0.0, 0.25, full, tide_at_ocean)
        } else {
            full
        };
        clamped_map(c.this_weight, 0.5, 1.0, sand_height, dune_height)
    }
}

/// The shared noises of the two rocky shore types.
struct ShelfNoises {
    rock_shelf_intensity: Noise2D,
    steepness: Noise2D,
    roughness: Noise2D,
    tidepool: Noise2D,
}
impl ShelfNoises {
    fn new(seed: i64) -> Self {
        Self {
            rock_shelf_intensity: n(OpenSimplex2D::new(seed + 5252)
                .octaves(3)
                .spread(0.03)
                .scaled(-0.8, 1.0)),
            steepness: n(OpenSimplex2D::new(seed + 224).octaves(2).spread(0.11))
                .clamped_scaled4(-0.7, 0.7, 0.0, 0.3),
            roughness: n(OpenSimplex2D::new(seed)
                .octaves(3)
                .spread(0.09)
                .scaled(-3.0, 3.0)),
            tidepool: n(OpenSimplex2D::new(seed + 492).octaves(3).spread(0.09))
                .clamped_scaled4(-1.0, 0.0, -5.0, 0.0),
        }
    }
    fn shelf_progress(&self, c: &ShoreColumn) -> f64 {
        let base = self
            .rock_shelf_intensity
            .noise(f64::from(c.x), f64::from(c.z));
        if c.ocean_weight > 0.10 {
            clamped_map(c.ocean_weight, 0.1, 0.3, base, base - c.ocean_weight * 3.0)
        } else {
            let land_influence =
                clamped_map(c.land_weight, 0.4, 0.0, base + c.land_weight * 2.0, base);
            clamped_map(c.ocean_weight, 0.0, 0.1, base + land_influence, base)
        }
    }
}
const TOP_SHELF_EDGE: f64 = 0.7;
const MID_SHELF_EDGE: f64 = 0.35;
const LOW_SHELF_EDGE: f64 = 0.10;

struct Embayments {
    tide: Noise2D,
    shelf: ShelfNoises,
    cave: OpenSimplex3D,
    x: i32,
    z: i32,
    tide_level: f64,
    sand_height: f64,
    shelf_progress: f64,
    height: f64,
    mid_shelf_height: f64,
}
impl ShoreNoiseSampler for Embayments {
    fn set_column_and_sample_height(&mut self, c: &ShoreColumn) -> f64 {
        let (fx, fz) = (f64::from(c.x), f64::from(c.z));
        self.tide_level = self.tide.noise(fx, fz);
        self.sand_height =
            simple_beach(self.tide_level, c.height_in, c.land_weight, c.ocean_weight);
        self.x = c.x;
        self.z = c.z;
        let roughness = self.shelf.roughness.noise(fx, fz) + self.shelf.tidepool.noise(fx, fz);
        let top = SEA + roughness + 15.0;
        self.mid_shelf_height = SEA + roughness + 9.0;
        self.shelf_progress = self.shelf.shelf_progress(c);
        let steepness = self.shelf.steepness.noise(fx, fz);
        let p = self.shelf_progress;
        self.height = if p > TOP_SHELF_EDGE + steepness {
            top
        } else if p > TOP_SHELF_EDGE {
            mth_map(
                p,
                TOP_SHELF_EDGE,
                TOP_SHELF_EDGE + steepness,
                self.mid_shelf_height,
                top,
            )
        } else if p > MID_SHELF_EDGE + steepness {
            self.mid_shelf_height
        } else if p > MID_SHELF_EDGE {
            mth_map(
                p,
                MID_SHELF_EDGE,
                MID_SHELF_EDGE + steepness,
                self.sand_height,
                self.mid_shelf_height,
            )
        } else {
            self.sand_height
        };
        self.height
    }
    fn noise(&mut self, y: i32, _noise_in: f64) -> f64 {
        let fy = f64::from(y);
        if fy <= self.sand_height {
            return -0.6;
        }
        if self.height <= self.mid_shelf_height {
            let depth = self.cave.noise(f64::from(self.x), fy, f64::from(self.z));
            let progress = width_function(
                true,
                fy - self.tide_level,
                self.mid_shelf_height - self.tide_level,
                MID_SHELF_EDGE + depth,
                MID_SHELF_EDGE,
            );
            return 0.4 + 3.0 * (progress - self.shelf_progress);
        }
        if fy <= self.height {
            return 0.0;
        }
        0.7
    }
}

struct RockyShores {
    tide: Noise2D,
    shelf: ShelfNoises,
    cells: Cellular2D,
    punchbowl_size: Noise2D,
    x: i32,
    z: i32,
    ocean_edge_height: f64,
    shelf_progress: f64,
    height: f64,
    top_shelf_height: f64,
    low_shelf_height: f64,
}
impl ShoreNoiseSampler for RockyShores {
    fn set_column_and_sample_height(&mut self, c: &ShoreColumn) -> f64 {
        let (fx, fz) = (f64::from(c.x), f64::from(c.z));
        let tide_level = self.tide.noise(fx, fz);
        self.ocean_edge_height = tide_level - 4.0;
        self.x = c.x;
        self.z = c.z;
        let roughness = self.shelf.roughness.noise(fx, fz) + self.shelf.tidepool.noise(fx, fz);
        self.top_shelf_height = SEA + roughness + 15.0;
        let mid = SEA + roughness + 9.0;
        self.low_shelf_height = SEA + roughness + 3.0;
        self.shelf_progress = self.shelf.shelf_progress(c);
        let steepness = self.shelf.steepness.noise(fx, fz);
        let p = self.shelf_progress;
        self.height = if p > TOP_SHELF_EDGE + steepness {
            self.top_shelf_height
        } else if p > TOP_SHELF_EDGE {
            mth_map(
                p,
                TOP_SHELF_EDGE,
                TOP_SHELF_EDGE + steepness,
                mid,
                self.top_shelf_height,
            )
        } else if p > MID_SHELF_EDGE + steepness {
            mid
        } else if p > MID_SHELF_EDGE {
            mth_map(
                p,
                MID_SHELF_EDGE,
                MID_SHELF_EDGE + steepness,
                self.low_shelf_height,
                mid,
            )
        } else if p > LOW_SHELF_EDGE - steepness {
            self.low_shelf_height
        } else {
            self.ocean_edge_height
        };
        self.height
    }
    fn noise(&mut self, y: i32, _noise_in: f64) -> f64 {
        let fy = f64::from(y);
        if fy <= self.ocean_edge_height {
            return 0.0;
        }
        if fy >= self.height {
            return 0.7;
        }
        let (fx, fz) = (f64::from(self.x), f64::from(self.z));
        let cell = self.cells.cell(fx, fz);
        let punchbowl_radius = if cell.noise > 0.25 && cell.f2 >= 0.5 {
            1.0
        } else {
            cell.f1
        };
        let mut return_noise = 0.0f64;
        if self.height <= self.low_shelf_height {
            let progress = width_function(
                true,
                fy - self.ocean_edge_height,
                self.low_shelf_height - self.ocean_edge_height,
                LOW_SHELF_EDGE + 0.11,
                LOW_SHELF_EDGE,
            );
            return_noise = 0.4 + 3.0 * (progress - self.shelf_progress);
        }
        if punchbowl_radius < 0.25 {
            let shift = self.punchbowl_size.noise(fx, fz);
            let edge = width_function(
                true,
                fy - self.ocean_edge_height,
                self.top_shelf_height - self.ocean_edge_height,
                0.25 - shift,
                0.12 - shift,
            );
            return_noise = return_noise.max(3.0 * (edge - punchbowl_radius));
        }
        return_noise
    }
}

struct Classic {
    shore_noise: Noise2D,
}
impl ShoreNoiseSampler for Classic {
    fn set_column_and_sample_height(&mut self, c: &ShoreColumn) -> f64 {
        let adjustment = f64::from(c.shore_base_height);
        let cliff_influence = clamp(
            self.shore_noise.noise(f64::from(c.x), f64::from(c.z))
                + mth_map(c.height_in, adjustment, adjustment + 20.0, 0.0, 0.6),
            0.0,
            1.0,
        );
        let adjusted_influence = 1.0 - (1.0 - cliff_influence) * (1.0 - cliff_influence);
        let x2 = lerp(adjusted_influence, 0.8, 0.515);
        let y2 = 1.15 - 0.3 * x2;
        let adjusted_shore_weight = if c.shore_weight < x2 {
            mth_map(c.shore_weight, 0.5, x2, 0.5, y2)
        } else {
            mth_map(c.shore_weight, x2, 1.0, y2, 1.0)
        };
        let normal_weight = 1.0 - c.shore_weight;
        let adjusted_normal_weight = 1.0 - adjusted_shore_weight;
        let adjusted_height = ((adjusted_shore_weight / c.shore_weight) * c.shore_height
            + (adjusted_normal_weight / normal_weight) * c.normal_height)
            .max(adjustment);
        if adjusted_height < c.height_in {
            adjusted_height
        } else {
            c.height_in
        }
    }
}

struct SeaStacks {
    tide: Noise2D,
    distribution: Noise2D,
    cells: Cellular2D,
    cliff: OpenSimplex3D,
    stack_noise_value: f64,
    sand_height: f64,
    land_weight: f64,
    tide_level: f64,
    ocean_weight_factor: f64,
    x: i32,
    z: i32,
}
const MIN_STACK_DENSITY: f64 = 0.85;
const STACK_NOISE_SCALE: f64 = 3.0;
impl SeaStacks {
    fn f2_minus_f1(&self, x: f64, z: f64) -> f64 {
        let cell = self.cells.cell(x, z);
        if cell.f1 > 0.0 {
            cell.f2 - cell.f1
        } else {
            1.0
        }
    }
    fn f1(&self, x: f64, z: f64) -> f64 {
        let cell = self.cells.cell(x, z);
        if self.distribution.noise(cell.x, cell.y) < MIN_STACK_DENSITY {
            STACK_NOISE_SCALE
        } else {
            cell.f1
        }
    }
}
impl ShoreNoiseSampler for SeaStacks {
    fn set_column_and_sample_height(&mut self, c: &ShoreColumn) -> f64 {
        let (fx, fz) = (f64::from(c.x), f64::from(c.z));
        self.x = c.x;
        self.z = c.z;
        self.land_weight = c.land_weight;
        self.ocean_weight_factor = clamped_map(c.ocean_weight, 0.1, 0.25, 1.0, 0.0);
        self.tide_level = self.tide.noise(fx, fz);
        let typical = simple_beach(self.tide_level, c.height_in, c.land_weight, c.ocean_weight);
        self.sand_height = clamped_map(
            c.this_weight,
            0.5,
            0.8,
            typical,
            typical.min(self.tide_level - 1.0),
        );
        let f2f1 = self.f2_minus_f1(fx, fz);
        let output_min = clamped_map(self.distribution.noise(fx, fz), 0.2, 0.6, 3.0, 1.0);
        self.stack_noise_value =
            self.f1(fx, fz) * clamped_map(f2f1.abs(), 0.0, 0.25, output_min, 1.0);
        clamped_map(
            c.ocean_weight,
            0.0,
            0.5,
            c.height_in,
            c.shore_height / c.shore_weight,
        )
    }
    fn noise(&mut self, y_in: i32, _noise_in: f64) -> f64 {
        let fy_in = f64::from(y_in);
        if fy_in <= self.sand_height {
            return 0.0;
        }
        let overhang_height = SEA + 14.0;
        let stack_base_height = self.tide_level + 1.0;
        let y = fy_in - stack_base_height;
        let modifier = 0.04
            * self
                .cliff
                .noise(f64::from(self.x), y, f64::from(self.z))
                .abs();
        let stack_min_width = (0.06 + modifier) * self.ocean_weight_factor;
        let stack_max_width = (0.18 + modifier) * self.ocean_weight_factor;
        let border_top = 0.22 + modifier;
        let border_base = 0.26 + modifier;
        let height = overhang_height - stack_base_height;
        let stack_width = width_function(false, y, height, stack_min_width, stack_max_width);
        let stack_output = clamp((self.stack_noise_value - stack_width) * 10.0, 0.0, 1.0)
            * clamped_map(
                fy_in,
                stack_base_height,
                overhang_height,
                STACK_NOISE_SCALE,
                0.75,
            );
        if self.land_weight >= border_top {
            let border_weight = width_function(true, y, height, border_base, border_top);
            (clamp((border_weight - self.land_weight) * 10.0, 0.0, 1.0) * STACK_NOISE_SCALE)
                .min(stack_output)
        } else {
            stack_output
        }
    }
}

struct UpperTerrace {
    tide: Noise2D,
    cliff: OpenSimplex3D,
    lower: Noise2D,
    upper: Noise2D,
    land_weight: f64,
    ocean_weight: f64,
    sand_height: f64,
    x: i32,
    z: i32,
}
impl ShoreNoiseSampler for UpperTerrace {
    fn set_column_and_sample_height(&mut self, c: &ShoreColumn) -> f64 {
        let (fx, fz) = (f64::from(c.x), f64::from(c.z));
        self.x = c.x;
        self.z = c.z;
        self.land_weight = c.land_weight;
        self.ocean_weight = c.ocean_weight;
        let tide = self.tide.noise(fx, fz);
        self.sand_height = simple_beach(tide, c.height_in, c.land_weight, c.ocean_weight);
        self.upper.noise(fx, fz)
    }
    fn noise(&mut self, y_in: i32, _noise_in: f64) -> f64 {
        let fy_in = f64::from(y_in);
        let lower_height = self.lower.noise(f64::from(self.x), f64::from(self.z));
        let (fx, fz) = (f64::from(self.x), f64::from(self.z));
        if fy_in >= lower_height {
            let overhang_height = SEA + 25.0;
            let cliff_base_height = SEA + 11.0;
            let y = fy_in - cliff_base_height;
            let modifier = 0.04 * self.cliff.noise(fx, y, fz).abs();
            let border_top = 0.32 + modifier;
            let border_base = 0.36 + modifier;
            let height = overhang_height - cliff_base_height;
            if self.land_weight >= border_top {
                let border_weight = width_function(true, y, height, border_base, border_top);
                clamp((border_weight - self.land_weight) * 10.0, 0.0, 1.0) * STACK_NOISE_SCALE
            } else {
                clamped_map(
                    fy_in,
                    lower_height,
                    lower_height + 6.0,
                    0.0,
                    STACK_NOISE_SCALE,
                )
            }
        } else {
            if fy_in <= self.sand_height {
                return 0.0;
            }
            terrace_lower_noise(fy_in, fx, fz, self.ocean_weight, &self.cliff)
        }
    }
}

fn terrace_lower_noise(
    fy_in: f64,
    fx: f64,
    fz: f64,
    ocean_weight: f64,
    cliff: &OpenSimplex3D,
) -> f64 {
    let overhang_height = SEA + 11.0;
    let cliff_base_height = SEA - 4.0;
    let y = fy_in - cliff_base_height;
    let modifier = 0.12 * cliff.noise(fx, y, fz).abs();
    let border_top = 0.26 - modifier;
    let border_base = 0.22 - modifier;
    let height = overhang_height - cliff_base_height;
    if ocean_weight >= border_base {
        let border_weight = width_function(false, y, height, border_base, border_top);
        clamp((ocean_weight - border_weight) * 20.0, 0.0, 1.0) * STACK_NOISE_SCALE
    } else {
        0.0
    }
}

struct LowerTerrace {
    tide: Noise2D,
    cliff: OpenSimplex3D,
    lower: Noise2D,
    ocean_weight: f64,
    sand_height: f64,
    x: i32,
    z: i32,
}
impl ShoreNoiseSampler for LowerTerrace {
    fn set_column_and_sample_height(&mut self, c: &ShoreColumn) -> f64 {
        let (fx, fz) = (f64::from(c.x), f64::from(c.z));
        self.x = c.x;
        self.z = c.z;
        self.ocean_weight = c.ocean_weight;
        let lower_height = self.lower.noise(fx, fz);
        let tide = self.tide.noise(fx, fz);
        self.sand_height = simple_beach(tide, c.height_in, c.land_weight, c.ocean_weight);
        lower_height
    }
    fn noise(&mut self, y_in: i32, _noise_in: f64) -> f64 {
        let fy_in = f64::from(y_in);
        if fy_in <= self.sand_height {
            return 0.0;
        }
        terrace_lower_noise(
            fy_in,
            f64::from(self.x),
            f64::from(self.z),
            self.ocean_weight,
            &self.cliff,
        )
    }
}

/// `ShoreBlendType.createNoiseSampler(seed)`; `seed` is the level seed (`Seed.seed()`).
pub fn create_sampler(kind: ShoreBlendType, seed: i64) -> Box<dyn ShoreNoiseSampler> {
    let tide = shoreline_tide_level_noise(seed);
    match kind {
        ShoreBlendType::None => Box::new(NoShore),
        ShoreBlendType::Classic => Box::new(Classic {
            shore_noise: n(OpenSimplex2D::new(seed + 8719234132)
                .octaves(2)
                .spread(f(0.003f32))
                .scaled(-0.1, 1.1)),
        }),
        ShoreBlendType::Sandy => Box::new(SandyBeach {
            tide,
            sand_height: 0.0,
            weight: 0.0,
        }),
        ShoreBlendType::Embayments => Box::new(Embayments {
            tide,
            shelf: ShelfNoises::new(seed),
            cave: cliff_noise(seed).scaled(0.0, 0.12),
            x: 0,
            z: 0,
            tide_level: 0.0,
            sand_height: 0.0,
            shelf_progress: 0.0,
            height: 0.0,
            mid_shelf_height: 0.0,
        }),
        ShoreBlendType::UpperTerrace => Box::new(UpperTerrace {
            tide,
            cliff: cliff_noise(seed),
            lower: lower_terrace_noise(seed),
            upper: upper_terrace_noise(seed),
            land_weight: 0.0,
            ocean_weight: 0.0,
            sand_height: 0.0,
            x: 0,
            z: 0,
        }),
        ShoreBlendType::LowerTerrace => Box::new(LowerTerrace {
            tide,
            cliff: cliff_noise(seed),
            lower: lower_terrace_noise(seed),
            ocean_weight: 0.0,
            sand_height: 0.0,
            x: 0,
            z: 0,
        }),
        ShoreBlendType::SetbackCliffs => Box::new(SetbackCliffs {
            tide,
            bank_noise: n(OpenSimplex2D::new(seed).spread(0.03))
                .abs()
                .scaled(-1.0, 8.0),
            cliff: cliff_noise(seed),
            bank_height: 0.0,
            land_weight: 0.0,
            x: 0,
            z: 0,
            y_top: 0.0,
        }),
        ShoreBlendType::Dunes => {
            let warp = OpenSimplex2D::new(seed).scaled(-10.0, 10.0).spread(0.05);
            let dune = n(OpenSimplex2D::new(seed).spread(0.03))
                .abs()
                .scaled(-2.0, 6.5)
                .warped(warp);
            Box::new(Dunes { tide, dune })
        }
        ShoreBlendType::RockyShores => Box::new(RockyShores {
            tide,
            shelf: ShelfNoises::new(seed),
            cells: Cellular2D::new(seed + 323).spread(0.022),
            punchbowl_size: n(OpenSimplex2D::new(seed).octaves(3).spread(0.06))
                .clamped_scaled4(-0.7, 0.7, -0.05, 0.15),
            x: 0,
            z: 0,
            ocean_edge_height: 0.0,
            shelf_progress: 0.0,
            height: 0.0,
            top_shelf_height: 0.0,
            low_shelf_height: 0.0,
        }),
        ShoreBlendType::SeaStacks => Box::new(SeaStacks {
            tide,
            distribution: n(OpenSimplex2D::new(seed + 5424).octaves(2))
                .map(|y| 1.0 - y.abs())
                .spread(0.015),
            cells: Cellular2D::new(seed + 323).spread(0.05),
            cliff: cliff_noise(seed),
            stack_noise_value: 0.0,
            sand_height: 0.0,
            land_weight: 0.0,
            tide_level: 0.0,
            ocean_weight_factor: 0.0,
            x: 0,
            z: 0,
        }),
    }
}
