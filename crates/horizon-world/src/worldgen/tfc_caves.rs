//! TerraFirmaCraft's cave densities (EUPL 1.2): `tfc:noise_caves` and the noodle caves, evaluated
//! the way `ChunkNoiseFiller` does: sampled at the corners of 4x8x4 noise cells and interpolated
//! trilinearly. The formulas are TFC's `density_function/*.json` plus `Spaghetti2D`/`Spaghetti3D`.
//!
//! The vanilla noise definitions the formulas name (`minecraft:cave_cheese` and the rest) are
//! not in TFC's files; `NOISES` holds the values vanilla uses, written from memory because the
//! vanilla data is not available when this was ported. Check them against the game's
//! `worldgen/noise/*.json` before trusting the cave shapes to match.

use horizon_core::noise::{Noise, NoiseStack, NormalNoise};
use horizon_core::random::{
    PositionalRandom, RandomSource, XoroshiroPositionalRandom, XoroshiroRandom,
};
use std::collections::HashMap;

pub const CELL_WIDTH: i32 = 4;
pub const CELL_HEIGHT: i32 = 8;

/// `(name, first octave, amplitudes)`.
pub const NOISES: [(&str, i32, &[f64]); 18] = [
    (
        "cave_cheese",
        -8,
        &[0.5, 1.0, 2.0, 1.0, 2.0, 1.0, 0.0, 2.0, 0.0],
    ),
    ("cave_layer", -8, &[1.0]),
    ("cave_entrance", -7, &[0.4, 0.5, 1.0]),
    ("spaghetti_2d", -7, &[1.0]),
    ("spaghetti_2d_elevation", -8, &[1.0]),
    ("spaghetti_2d_modulator", -11, &[1.0]),
    ("spaghetti_2d_thickness", -11, &[1.0]),
    ("spaghetti_3d_1", -7, &[1.0]),
    ("spaghetti_3d_2", -7, &[1.0]),
    ("spaghetti_3d_rarity", -11, &[1.0]),
    ("spaghetti_3d_thickness", -8, &[1.0]),
    ("spaghetti_roughness", -5, &[1.0]),
    ("spaghetti_roughness_modulator", -2, &[1.0]),
    ("noodle", -8, &[1.0]),
    ("noodle_thickness", -8, &[1.0]),
    ("noodle_ridge_a", -7, &[1.0]),
    ("noodle_ridge_b", -7, &[1.0]),
    ("aquifer_barrier", -3, &[1.0]),
];

fn clamped_map(value: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    let t = (value - in_min) / (in_max - in_min);
    if t < 0.0 {
        out_min
    } else if t > 1.0 {
        out_max
    } else {
        out_min + t * (out_max - out_min)
    }
}

/// `minecraft:y_clamped_gradient`.
fn gradient(y: i32, from_y: f64, to_y: f64, from_value: f64, to_value: f64) -> f64 {
    clamped_map(f64::from(y), from_y, to_y, from_value, to_value)
}

fn rarity_2d(value: f64) -> f64 {
    if value < -0.75 {
        0.5
    } else if value < -0.5 {
        0.75
    } else if value < 0.5 {
        1.0
    } else if value < 0.75 {
        2.0
    } else {
        3.0
    }
}

fn rarity_3d(value: f64) -> f64 {
    if value < -0.5 {
        0.75
    } else if value < 0.0 {
        1.0
    } else if value < 0.5 {
        1.5
    } else {
        2.0
    }
}

/// The five interpolated cave functions at one point.
#[derive(Clone, Copy, Debug)]
pub struct CaveValues {
    pub noise_caves: f64,
    pub noodle_toggle: f64,
    pub noodle_thickness: f64,
    pub noodle_ridge_a: f64,
    pub noodle_ridge_b: f64,
}

pub struct CaveNoise {
    noises: HashMap<&'static str, NoiseStack>,
    fork: XoroshiroPositionalRandom,
}

impl CaveNoise {
    /// `NoiseSampler(seed)`: every noise is `NormalNoise.create(fork.fromHashOf("minecraft:<name>"), ...)`.
    pub fn new(noise_seed: i64) -> Self {
        let factory = XoroshiroRandom::new(noise_seed).fork_positional();
        let noises = NOISES
            .iter()
            .map(|(name, octave, amplitudes)| {
                let mut random = factory.from_hash_of(&format!("minecraft:{name}"));
                let noise = NormalNoise::create_parity(*octave, amplitudes)
                    .and_then(|n| n.create(&mut random))
                    .expect("cave noise parameters are valid");
                (*name, noise)
            })
            .collect();
        Self {
            noises,
            fork: factory,
        }
    }

    /// `NoiseSampler.positionalRandomFactory`.
    pub fn fork(&self) -> XoroshiroPositionalRandom {
        self.fork
    }

    /// `barrierNoise.getValue(x, y, z)`.
    pub fn barrier_at(&self, x: f64, y: f64, z: f64) -> f64 {
        f64::from(self.barrier().get3(x, y, z))
    }

    /// The barrier noise the aquifer uses (not wired yet).
    pub fn barrier(&self) -> &NoiseStack {
        &self.noises["aquifer_barrier"]
    }

    fn noise(&self, name: &str, x: f64, y: f64, z: f64, xz_scale: f64, y_scale: f64) -> f64 {
        f64::from(self.noises[name].get3(x * xz_scale, y * y_scale, z * xz_scale))
    }

    fn noodle(&self, name: &str, y: i32, x: f64, z: f64, scale: f64, outside: f64) -> f64 {
        if (-60..320).contains(&y) {
            self.noise(name, x, f64::from(y), z, scale, scale)
        } else {
            outside
        }
    }

    /// The five functions evaluated exactly at a block position (no interpolation).
    pub fn compute(&self, bx: i32, y: i32, bz: i32) -> CaveValues {
        let (x, z) = (f64::from(bx), f64::from(bz));
        let fy = f64::from(y);
        let cheese = (0.27 + self.noise("cave_cheese", x, fy, z, 1.0, 2.0 / 3.0)).clamp(-1.0, 1.0);
        let layer = self.noise("cave_layer", x, fy, z, 1.0, 8.0);
        let big = cheese + 4.0 * (layer * layer);
        let roughness = self.noise("spaghetti_roughness", x, fy, z, 1.0, 1.0);
        let modulator = self.noise("spaghetti_roughness_modulator", x, fy, z, 1.0, 1.0);
        let rough = (0.4 - roughness.abs()) * (0.05 + 0.05 * modulator);

        // tfc:spaghetti2d
        let elevation = 0.0 + 8.0 * self.noise("spaghetti_2d_elevation", x, fy, z, 1.0, 0.0);
        let rarity = rarity_2d(self.noise("spaghetti_2d_modulator", x, fy, z, 2.0, 1.0));
        let thickness =
            0.95 + 0.35000000000000003 * self.noise("spaghetti_2d_thickness", x, fy, z, 2.0, 1.0);
        let rarity_noise =
            f64::from(self.noises["spaghetti_2d"].get3(x / rarity, fy / rarity, z / rarity));
        let left = (elevation - fy / 8.0).abs() - thickness;
        let right = (rarity * rarity_noise).abs() - 0.083 * thickness;
        let spaghetti_2d = (left * left * left).max(right).clamp(-1.0, 1.0);

        // tfc:spaghetti3d
        let rarity = rarity_3d(self.noise("spaghetti_3d_rarity", x, fy, z, 2.0, 1.0));
        let thickness = 0.0765
            + 0.011499999999999996 * self.noise("spaghetti_3d_thickness", x, fy, z, 1.0, 1.0);
        let one =
            f64::from(self.noises["spaghetti_3d_1"].get3(x / rarity, fy / rarity, z / rarity));
        let two =
            f64::from(self.noises["spaghetti_3d_2"].get3(x / rarity, fy / rarity, z / rarity));
        let spaghetti_3d =
            ((rarity * one).abs().max((rarity * two).abs()) - thickness).clamp(-1.0, 1.0);

        let entrance = gradient(y, -10.0, 30.0, 0.3, 0.0)
            + (0.37 + self.noise("cave_entrance", x, fy, z, 0.75, 0.5));
        let inner = (rough + spaghetti_2d.min(spaghetti_3d)).min(entrance);
        let cave = big.min(inner).clamp(-1.0, 1.0);
        let slide = gradient(y, 20.0, 320.0, 0.0, 1.0);
        let noise_caves = cave * (1.0 - slide) + 2.5 * slide;

        CaveValues {
            noise_caves,
            noodle_toggle: if (-60..320).contains(&y) {
                self.noise("noodle", x, fy, z, 1.0, 1.0)
            } else {
                -1.0
            },
            noodle_thickness: self.noodle("noodle_thickness", y, x, z, 1.0, 0.0),
            noodle_ridge_a: self.noodle("noodle_ridge_a", y, x, z, 2.6666666666666665, 1.0),
            noodle_ridge_b: self.noodle("noodle_ridge_b", y, x, z, 2.6666666666666665, 1.0),
        }
    }
}

/// Trilinear interpolation over cell corners for one chunk, as `TrilinearInterpolator` does.
pub struct CaveInterpolator<'a> {
    noise: &'a CaveNoise,
    corners: HashMap<(i32, i32, i32), CaveValues>,
}

impl<'a> CaveInterpolator<'a> {
    pub fn new(noise: &'a CaveNoise) -> Self {
        Self {
            noise,
            corners: HashMap::new(),
        }
    }

    fn corner(&mut self, cx: i32, cy: i32, cz: i32) -> CaveValues {
        let noise = self.noise;
        *self
            .corners
            .entry((cx, cy, cz))
            .or_insert_with(|| noise.compute(cx * CELL_WIDTH, cy * CELL_HEIGHT, cz * CELL_WIDTH))
    }

    /// The functions at block `(x, y, z)`.
    pub fn at(&mut self, x: i32, y: i32, z: i32) -> CaveValues {
        let (cell_x, cell_y, cell_z) = (
            x.div_euclid(CELL_WIDTH),
            y.div_euclid(CELL_HEIGHT),
            z.div_euclid(CELL_WIDTH),
        );
        let dx = f64::from(x.rem_euclid(CELL_WIDTH)) / f64::from(CELL_WIDTH);
        let dy = f64::from(y.rem_euclid(CELL_HEIGHT)) / f64::from(CELL_HEIGHT);
        let dz = f64::from(z.rem_euclid(CELL_WIDTH)) / f64::from(CELL_WIDTH);
        let c = |me: &mut Self, ox, oy, oz| me.corner(cell_x + ox, cell_y + oy, cell_z + oz);
        let corners = [
            c(self, 0, 0, 0),
            c(self, 1, 0, 0),
            c(self, 0, 0, 1),
            c(self, 1, 0, 1),
            c(self, 0, 1, 0),
            c(self, 1, 1, 0),
            c(self, 0, 1, 1),
            c(self, 1, 1, 1),
        ];
        let lerp = |t: f64, a: f64, b: f64| a + t * (b - a);
        let pick = |f: fn(&CaveValues) -> f64| {
            let [n000, n100, n001, n101, n010, n110, n011, n111] = corners.map(|v| f(&v));
            let x00 = lerp(dx, n000, n100);
            let x10 = lerp(dx, n010, n110);
            let x01 = lerp(dx, n001, n101);
            let x11 = lerp(dx, n011, n111);
            let x0z = lerp(dz, x00, x01);
            let x1z = lerp(dz, x10, x11);
            lerp(dy, x0z, x1z)
        };
        CaveValues {
            noise_caves: pick(|v| v.noise_caves),
            noodle_toggle: pick(|v| v.noodle_toggle),
            noodle_thickness: pick(|v| v.noodle_thickness),
            noodle_ridge_a: pick(|v| v.noodle_ridge_a),
            noodle_ridge_b: pick(|v| v.noodle_ridge_b),
        }
    }
}

/// `calculateBlockStateAtNoise` before the aquifer: terrain density cut by noodle and noise caves.
pub fn with_caves(terrain: f64, caves: &CaveValues) -> f64 {
    let mut density = terrain;
    if caves.noodle_toggle >= 0.0 {
        let thickness = clamped_map(caves.noodle_thickness, -1.0, 1.0, 0.05, 0.1);
        let ridge_a = (1.5 * caves.noodle_ridge_a).abs() - thickness;
        let ridge_b = (1.5 * caves.noodle_ridge_b).abs() - thickness;
        density = density.min(ridge_a.max(ridge_b));
    }
    density.min(caves.noise_caves)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caves_are_deterministic_and_bounded() {
        let a = CaveNoise::new(99);
        let b = CaveNoise::new(99);
        for (x, y, z) in [(0, 10, 0), (100, -40, -80), (-33, 200, 17)] {
            let (va, vb) = (a.compute(x, y, z), b.compute(x, y, z));
            assert_eq!(va.noise_caves.to_bits(), vb.noise_caves.to_bits());
            assert!(va.noise_caves.is_finite());
            assert!(va.noodle_toggle.is_finite() && va.noodle_thickness.is_finite());
        }
    }

    #[test]
    fn high_up_there_are_no_noise_caves() {
        // The slide pushes the value to 2.5 at y = 320: solid ground never carves up there.
        let noise = CaveNoise::new(7);
        assert!(noise.compute(5, 319, 5).noise_caves > 2.0);
    }

    #[test]
    fn interpolation_hits_the_corners_exactly() {
        let noise = CaveNoise::new(3);
        let mut interpolator = CaveInterpolator::new(&noise);
        let direct = noise.compute(8, 16, -12);
        let blended = interpolator.at(8, 16, -12);
        assert_eq!(direct.noise_caves.to_bits(), blended.noise_caves.to_bits());
    }

    #[test]
    fn some_underground_space_opens_up() {
        let noise = CaveNoise::new(2024);
        let mut interpolator = CaveInterpolator::new(&noise);
        let open = (0..40)
            .flat_map(|i| (0..30).map(move |j| (i * 5, -50 + j * 6, i * 3)))
            .filter(|&(x, y, z)| with_caves(1.0, &interpolator.at(x, y, z)) <= 0.0)
            .count();
        assert!(open > 0, "no cave air in 1200 samples");
    }
}
