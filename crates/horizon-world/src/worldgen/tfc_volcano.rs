//! TerraFirmaCraft's centered features (EUPL 1.2): the height shapes of cinder cones, tuff rings,
//! atolls and tuyas from `CenteredFeatureNoise`. Each is a cellular-noise cell with one feature at
//! its centre, blended into the terrain height after rivers and shores.
//!
//! Stratovolcanoes are in `tfc_volcano_strato`. The blocks volcanoes place are not ported yet.

use super::tfc_biome::BIOME_NAMES;
use super::tfc_biome_ext::extensions;
use super::tfc_biome_noise::{active_hot_spots, active_shield_volcano, rift_valley};
use super::tfc_height::TfcWorld;
use super::tfc_noise::{Cell, Cellular2D, Noise2D, OpenSimplex2D, clamped_map};
use super::tfc_region::{Seed, hash_double};
use super::tfc_volcano_strato::Stratovolcanoes;

const SEA_LEVEL_Y: i32 = 63;
const NOT_PRESENT: f64 = -2_147_483_648.0;

fn f(value: f32) -> f64 {
    f64::from(value)
}

/// `Mth.map(double, ...)`.
fn map(value: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    let t = (value - in_min) / (in_max - in_min);
    out_min + t * (out_max - out_min)
}

/// `Mth.map(float, float, float, float, float)`.
fn map_f32(value: f32, in_min: f32, in_max: f32, out_min: f32, out_max: f32) -> f32 {
    let t = (value - in_min) / (in_max - in_min);
    out_min + t * (out_max - out_min)
}

fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

/// The shared easing every feature but the atoll uses.
fn calculate_easing(f1: f32) -> f32 {
    map_f32(f1, 0.0, 0.23, 1.0, 0.0)
}

fn rarity_ok(cell: &Cell, frequency: f32) -> bool {
    frequency != 0.0 && cell.noise.abs() <= f64::from(frequency)
}

struct Cinder {
    cells: Cellular2D,
    jitter: OpenSimplex2D,
    shield: Noise2D,
    rift: Noise2D,
}

struct TuffRing {
    cells: Cellular2D,
    jitter: OpenSimplex2D,
    added_cliff: OpenSimplex2D,
    everywhere: OpenSimplex2D,
}

struct Atoll {
    cells: Cellular2D,
    height: OpenSimplex2D,
    rim_warp: OpenSimplex2D,
    thickness: OpenSimplex2D,
}

struct Tuya {
    cells: Cellular2D,
    jitter: OpenSimplex2D,
    added: OpenSimplex2D,
}

/// The four ported samplers, built the way `createVolcanoSamplersForChunk` does.
pub struct Volcanoes {
    cinder: Cinder,
    tuff: TuffRing,
    atoll: Atoll,
    tuya: Tuya,
    strato: Stratovolcanoes,
    /// The atoll surface builder's own `heightNoise`.
    atoll_surface: OpenSimplex2D,
    shield_id: i32,
    rift_id: i32,
}

impl Volcanoes {
    pub fn new(level_seed: i64) -> Self {
        let mut seed = Seed::of(level_seed);
        let s = seed.seed();
        let cinder = Cinder {
            cells: Cellular2D::new(s).spread(f(0.009f32)),
            jitter: OpenSimplex2D::new(s.wrapping_add(8_179_234_123))
                .octaves(2)
                .scaled(f(-0.0016f32), f(0.0016f32))
                .spread(f(0.128f32)),
            shield: active_shield_volcano(s, active_hot_spots(s)),
            rift: rift_valley(s, 2, 30, false),
        };
        // `seed.next()` is drawn while the tuff ring is built, after the cinder cones.
        let tuff = TuffRing {
            cells: Cellular2D::with(s, 0.2f32, 1).spread(f(0.003f32)),
            jitter: OpenSimplex2D::new(s.wrapping_add(1_234_123))
                .octaves(2)
                .scaled(f(-0.032f32), f(0.032f32))
                .spread(f(0.064f32)),
            added_cliff: OpenSimplex2D::new(s)
                .octaves(2)
                .spread(0.1)
                .scaled(-2.0, 10.0),
            everywhere: OpenSimplex2D::new(seed.next())
                .octaves(3)
                .spread(0.03)
                .scaled(-10.0, 10.0),
        };
        let atoll = Atoll {
            cells: Cellular2D::with(s, 0.5f32, 2).spread(f(0.0023f32)),
            height: OpenSimplex2D::new(s)
                .octaves(4)
                .spread(f(0.03f32))
                .scaled(f64::from(SEA_LEVEL_Y - 3), f64::from(SEA_LEVEL_Y + 7)),
            rim_warp: OpenSimplex2D::new(s.wrapping_add(1431))
                .octaves(3)
                .scaled(0.0, f(0.3f32))
                .spread(f(0.029f32)),
            thickness: OpenSimplex2D::new(s.wrapping_add(131))
                .octaves(3)
                .spread(f(0.02f32)),
        };
        let tuya = Tuya {
            cells: Cellular2D::with(s, 0.21f32, 1).spread(f(0.0033f32)),
            jitter: OpenSimplex2D::new(s.wrapping_add(1_234_123))
                .octaves(2)
                .scaled(f(-0.016f32), f(0.016f32))
                .spread(f(0.128f32)),
            added: OpenSimplex2D::new(s)
                .octaves(2)
                .spread(0.1)
                .scaled(-2.0, 3.0),
        };
        let id = |name: &str| BIOME_NAMES.iter().position(|n| *n == name).expect("biome") as i32;
        Self {
            cinder,
            tuff,
            atoll,
            tuya,
            strato: Stratovolcanoes::new(s),
            atoll_surface: OpenSimplex2D::new(level_seed.wrapping_add(0x41_746f_6c6c))
                .octaves(2)
                .spread(f(0.1f32))
                .scaled(-4.0, 4.0),
            shield_id: id("active_shield_volcano"),
            rift_id: id("rift_valley"),
        }
    }

    /// `CenteredFeatureNoise.getCenterBiome`: the biome at the centre of the stratovolcano cell
    /// that holds a block.
    pub fn stratovolcano_biome(&self, x: i32, z: i32, world: &TfcWorld) -> i32 {
        let cell = self.strato.cell(f64::from(x), f64::from(z));
        world.biome((cell.x as i32) >> 2, (cell.y as i32) >> 2)
    }

    /// The biomes at the centres of the cinder cone, tuff ring, tuya and atoll cells under the
    /// middle of a chunk (`createVolcanoSamplersForChunk`).
    pub fn center_biomes(&self, x: i32, z: i32, world: &TfcWorld) -> [i32; 4] {
        let at = |cells: &Cellular2D| {
            let cell = cells.cell(f64::from(x), f64::from(z));
            world.biome((cell.x as i32) >> 2, (cell.y as i32) >> 2)
        };
        [
            at(&self.cinder.cells),
            at(&self.tuff.cells),
            at(&self.tuya.cells),
            at(&self.atoll.cells),
        ]
    }

    fn clamped_easing(cells: &Cellular2D, x: i32, z: i32, frequency: f32) -> f32 {
        let cell = cells.cell(f64::from(x), f64::from(z));
        if rarity_ok(&cell, frequency) {
            calculate_easing(cell.f1 as f32).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// `CenteredFeatureNoiseSampler.calculateEasing` of the cinder cones.
    pub fn cinder_easing(&self, x: i32, z: i32, frequency: f32) -> f32 {
        Self::clamped_easing(&self.cinder.cells, x, z, frequency)
    }

    pub fn tuff_easing(&self, x: i32, z: i32, frequency: f32) -> f32 {
        Self::clamped_easing(&self.tuff.cells, x, z, frequency)
    }

    pub fn tuya_easing(&self, x: i32, z: i32, frequency: f32) -> f32 {
        Self::clamped_easing(&self.tuya.cells, x, z, frequency)
    }

    /// The atoll easing and the atoll cell at a block.
    pub fn atoll_surface_easing(&self, x: i32, z: i32, frequency: f32) -> (f32, Cell) {
        let (fx, fz) = (f64::from(x), f64::from(z));
        let cell = self.atoll.cells.cell(fx, fz);
        let easing = if rarity_ok(&cell, frequency) {
            self.atoll_easing(&cell, fx, fz)
        } else {
            0.0
        };
        (easing, cell)
    }

    pub fn atoll_random_height(&self, x: i32, z: i32) -> f64 {
        self.atoll_surface.noise(f64::from(x), f64::from(z))
    }

    pub fn stratovolcanoes(&self) -> &Stratovolcanoes {
        &self.strato
    }

    /// `adjustHeightForVolcanic`.
    pub fn adjust_height(&self, height_in: f64, x: i32, z: i32, world: &TfcWorld) -> f64 {
        let ext = extensions();
        let biome_at = |cell: &Cell| world.biome((cell.x as i32) >> 2, (cell.y as i32) >> 2);
        let (fx, fz) = (f64::from(x), f64::from(z));
        let mut best = NOT_PRESENT;

        // Cinder cones.
        let cell = self.cinder.cells.cell(fx, fz);
        let (xc, zc) = (cell.x as i32, cell.y as i32);
        let biome = world.biome(xc >> 2, zc >> 2);
        let e = &ext[biome as usize];
        if e.cinder_cones && rarity_ok(&cell, e.feature_frequency) {
            let base = if biome == self.shield_id {
                self.cinder.shield.noise(f64::from(xc), f64::from(zc)) as i32 - 11
            } else if biome == self.rift_id {
                self.cinder.rift.noise(f64::from(xc), f64::from(zc)) as i32 - 11
            } else {
                SEA_LEVEL_Y + e.feature_base_height
            };
            best = best.max(self.cinder_height(
                &cell,
                fx,
                fz,
                base,
                e.feature_scale_height,
                height_in,
            ));
        }

        // Tuff rings.
        let cell = self.tuff.cells.cell(fx, fz);
        let e = &ext[biome_at(&cell) as usize];
        if e.tuff_rings && rarity_ok(&cell, e.feature_frequency) {
            best = best.max(self.tuff_height(
                &cell,
                fx,
                fz,
                e.feature_base_height,
                e.feature_scale_height,
                height_in,
            ));
        }

        // Atolls.
        let cell = self.atoll.cells.cell(fx, fz);
        let e = &ext[biome_at(&cell) as usize];
        if e.atolls && rarity_ok(&cell, e.feature_frequency) {
            best = best.max(self.atoll_height(&cell, fx, fz, height_in));
        }

        // Tuyas.
        let cell = self.tuya.cells.cell(fx, fz);
        let e = &ext[biome_at(&cell) as usize];
        if e.tuyas && rarity_ok(&cell, e.feature_frequency) {
            best = best.max(self.tuya_height(
                &cell,
                fx,
                fz,
                e.feature_base_height,
                e.feature_scale_height,
                e.feature_ice,
                height_in,
            ));
        }

        // Stratovolcanoes.
        let cell = self.strato.cell(fx, fz);
        let e = &ext[biome_at(&cell) as usize];
        if e.stratovolcanoes && rarity_ok(&cell, e.feature_frequency) {
            best = best.max(self.strato.height(
                &cell,
                x,
                z,
                e.feature_base_height,
                e.feature_scale_height,
                height_in,
            ));
        }

        if best == NOT_PRESENT { height_in } else { best }
    }

    fn cinder_height(
        &self,
        cell: &Cell,
        x: f64,
        z: f64,
        base_height: i32,
        scale_height: i32,
        height_in: f64,
    ) -> f64 {
        let f1 = cell.f1;
        let easing = (f64::from(calculate_easing(f1 as f32)) + self.cinder.jitter.noise(x, z))
            .clamp(0.0, 1.0);
        let rim = 0.025;
        let shape = cinder_shape(1.0 - easing, rim);
        let volcano_height = f64::from(base_height) + shape * f64::from(scale_height);
        let weight = f64::from(10.0f32) * (f64::from(cell.f2 as f32) - f1).clamp(0.0, f(0.1f32));
        let adjusted = easing * weight;
        if adjusted > 1.0 - rim {
            volcano_height
        } else {
            map(adjusted, 0.0, 1.0 - rim, height_in, volcano_height)
        }
    }

    fn tuff_height(
        &self,
        cell: &Cell,
        x: f64,
        z: f64,
        base_height: i32,
        scale_height: i32,
        height_in: f64,
    ) -> f64 {
        let f1 = cell.f1;
        let easing =
            (f64::from(calculate_easing(f1 as f32)) + self.tuff.jitter.noise(x, z)).clamp(0.0, 1.0);
        let shape = tuff_shape(1.0 - easing);
        let ring = shape * f64::from(scale_height)
            + if shape > 0.5 {
                self.tuff.added_cliff.noise(x, z)
            } else {
                0.0
            };
        let gap_adjusted = (ring * gap_vertical_easing(cell)).min(ring);
        let ring_height = f64::from(SEA_LEVEL_Y)
            + f64::from(base_height)
            + gap_adjusted
            + self.tuff.everywhere.noise(x, z);
        let delta = 25.0 * (cell.f2 - f1).clamp(0.0, 0.04);
        lerp(delta, height_in, ring_height.max(height_in))
    }

    fn tuya_height(
        &self,
        cell: &Cell,
        x: f64,
        z: f64,
        base_height: i32,
        scale_height: i32,
        icy: bool,
        height_in: f64,
    ) -> f64 {
        let easing = (f64::from(calculate_easing(cell.f1 as f32)) + self.tuya.jitter.noise(x, z))
            .clamp(0.0, 1.0);
        let shape = if icy {
            tuya_icy_shape(1.0 - easing)
        } else {
            tuya_shape(1.0 - easing)
        };
        let additional =
            f64::from((shape * f64::from(scale_height) + self.tuya.added.noise(x, z)) as f32);
        let height = f64::from(SEA_LEVEL_Y) + f64::from(base_height) + additional;
        lerp(
            easing,
            height_in,
            f(0.5f32) * (height + height.max(height_in + f(0.4f32) * additional)),
        )
    }

    fn atoll_easing(&self, cell: &Cell, x: f64, z: f64) -> f32 {
        let (f1, f2) = (cell.f1, cell.f2);
        let easing = (f2.sqrt() - f1.sqrt() + self.atoll.rim_warp.noise(x, z)) as f32;
        if f2 - f1 > 0.18 {
            easing
        } else {
            easing * map(f2 - f1, 0.0, 0.18, 0.0, 1.0) as f32
        }
    }

    fn atoll_height(&self, cell: &Cell, x: f64, z: f64, height_in: f64) -> f64 {
        let (min_thickness, max_thickness) = (-0.3, 0.4);
        let amplitude = max_thickness - min_thickness;
        let integrity = (0.4 + hash_double(cell.noise, 523)).min(1.0);
        let thickness_noise = self.atoll.thickness.noise(x, z);
        let unclamped = map(thickness_noise, -1.0, 1.0, min_thickness, max_thickness);
        let easing = f64::from(self.atoll_easing(cell, x, z));
        let lagoon = 0.8 + 0.12 * hash_double(cell.noise, 15413);
        let reef = reef_shape(
            easing,
            lagoon,
            unclamped,
            integrity,
            min_thickness,
            max_thickness,
            amplitude,
        );
        let mut max_height = self.atoll.height.noise(x, z);
        if integrity < 1.0 {
            max_height = clamped_map(
                thickness_noise + integrity,
                -1.0,
                1.0,
                lagoon * max_height,
                max_height,
            );
        }
        let atoll = map(reef, 0.0, 1.0, f64::from(SEA_LEVEL_Y - 60), max_height);
        height_in.max(atoll)
    }
}

fn cinder_shape(t: f64, rim: f64) -> f64 {
    if t > rim {
        (5.0 / (9.0 * t + 1.0) - 0.5) * 0.279_173_646_008
    } else {
        let a = t * 9.0 + 0.05;
        (8.0 * a * a + 2.976_632_653_06) * 0.279_173_646_008
    }
}

fn tuff_shape(t: f64) -> f64 {
    if t < f(0.03f32) {
        0.0
    } else if t < f(0.10f32) {
        t * f(7.0f32) - f(0.21f32)
    } else if t < f(0.15f32) {
        t * f(6.6667f32)
    } else if t < f(0.20f32) {
        1.0 - (t - f(0.15f32)) * f(6.6667f32)
    } else {
        f(1.5f32) - (5.0 * t)
    }
}

fn tuya_shape(t: f64) -> f64 {
    if t < f(0.015f32) {
        map(t, 0.0, f(0.015f32), f(0.8f32), 1.0)
    } else if t < f(0.125f32) {
        map(t, f(0.025f32), f(0.125f32), 1.0, f(0.75f32))
    } else {
        clamped_map(t, f(0.125f32), f(0.16f32), f(0.75f32), 0.0)
    }
}

fn tuya_icy_shape(t: f64) -> f64 {
    if t < f(0.015f32) {
        map(t, 0.0, f(0.015f32), f(0.8f32), 1.0)
    } else if t < f(0.125f32) {
        map(t, f(0.025f32), f(0.125f32), 1.0, f(0.75f32))
    } else if t < 0.16 {
        map(t, f(0.125f32), f(0.16f32), f(0.75f32), 0.0)
    } else {
        clamped_map(t, f(0.16f32), f(0.19f32), 0.0, f(0.5f32))
    }
}

/// `getGapVerticalEasing`.
fn gap_vertical_easing(cell: &Cell) -> f64 {
    let gap_size = 2.0 * hash_double(cell.noise, 112);
    let mut easing = 1.0;
    if gap_size > 0.0 {
        let gap_angle = 4.0 * hash_double(cell.noise, 113);
        let mut angle_to_gap = (cell.angle - gap_angle).abs();
        angle_to_gap = angle_to_gap.min(4.0 - angle_to_gap);
        if angle_to_gap < gap_size {
            let to_edge = (angle_to_gap + gap_size).min(angle_to_gap - gap_size).abs();
            easing = clamped_map(to_edge, 0.0, (0.5 * gap_size).max(0.25), 1.0, 0.0);
        }
    }
    easing
}

fn reef_shape(
    easing: f64,
    lagoon_depth: f64,
    unclamped: f64,
    integrity: f64,
    min_t: f64,
    max_t: f64,
    amplitude: f64,
) -> f64 {
    let edge = 0.47;
    let beach = edge + 0.11;
    let clamped = unclamped.clamp(0.0, 0.3);
    let lagoon_dist = beach + 0.19 + clamped;
    let base = if easing < edge {
        return map(easing, 0.0, edge, 0.0, lagoon_depth);
    } else if easing < beach {
        map(easing, edge, beach, lagoon_depth, 1.0)
    } else {
        clamped_map(easing, beach, lagoon_dist, 1.0, lagoon_depth)
    };
    if integrity >= 1.0 {
        base
    } else {
        clamped_map(
            unclamped,
            min_t - amplitude * integrity,
            max_t - amplitude * integrity,
            lagoon_depth,
            base,
        )
    }
}

/// Which `CenteredFeatureNoiseSampler` a placement modifier asks about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CenterKind {
    Cinder,
    TuffRing,
    Tuya,
    Atoll,
    Stratovolcano,
}

impl Volcanoes {
    fn center_cells(&self, kind: CenterKind) -> &Cellular2D {
        match kind {
            CenterKind::Cinder => &self.cinder.cells,
            CenterKind::TuffRing => &self.tuff.cells,
            CenterKind::Tuya => &self.tuya.cells,
            CenterKind::Atoll => &self.atoll.cells,
            CenterKind::Stratovolcano => self.strato.cellular(),
        }
    }

    /// `isValidBiome`.
    pub fn center_valid_biome(kind: CenterKind, biome: i32) -> bool {
        let Some(e) = extensions().get(biome as usize) else {
            return false;
        };
        match kind {
            CenterKind::Cinder => e.cinder_cones,
            CenterKind::TuffRing => e.tuff_rings,
            CenterKind::Tuya => e.tuyas,
            CenterKind::Atoll => e.atolls,
            CenterKind::Stratovolcano => e.stratovolcanoes,
        }
    }

    /// `getFrequency(biome)`.
    pub fn center_frequency(biome: i32) -> f32 {
        extensions()[biome as usize].feature_frequency
    }

    /// `calculateEasing(x, z, frequency)`.
    pub fn center_easing(&self, kind: CenterKind, x: i32, z: i32, frequency: f32) -> f32 {
        match kind {
            CenterKind::Atoll => self.atoll_surface_easing(x, z, frequency).0,
            _ => Self::clamped_easing(self.center_cells(kind), x, z, frequency),
        }
    }

    /// `calculateCenter(x, y, z, frequency)`.
    pub fn center_of(
        &self,
        kind: CenterKind,
        x: i32,
        y: i32,
        z: i32,
        frequency: f32,
    ) -> Option<[i32; 3]> {
        let cells = self.center_cells(kind);
        let cell = cells.cell(f64::from(x), f64::from(z));
        if !rarity_ok(&cell, frequency) {
            return None;
        }
        if kind == CenterKind::Cinder {
            // `maxSafeDiameter` samples the cell noise at the sign of the offset, as written.
            let dx = x - cell.x as i32;
            let dz = z - cell.y as i32;
            let f2 = cells
                .cell(f64::from(dx.signum()), f64::from(dz.signum()))
                .f2;
            if f2 <= 0.46 {
                return None;
            }
        }
        Some([cell.x as i32, y, cell.y as i32])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cinder_shape_is_continuous_at_the_rim() {
        let rim = 0.025;
        let (above, below) = (cinder_shape(rim + 1e-9, rim), cinder_shape(rim - 1e-9, rim));
        assert!((above - below).abs() < 0.01, "{above} vs {below}");
    }

    #[test]
    fn tuff_ring_rises_to_a_ridge_then_falls_away() {
        assert_eq!(tuff_shape(0.01), 0.0);
        assert!(tuff_shape(0.12) > 0.7);
        assert!(tuff_shape(0.15) > tuff_shape(0.19));
        assert!(tuff_shape(0.4) < 0.0);
    }

    #[test]
    fn reef_is_deep_outside_and_crests_at_the_beach() {
        let (min_t, max_t) = (-0.3, 0.4);
        let outside = reef_shape(0.0, 0.8, 0.2, 1.0, min_t, max_t, 0.7);
        let crest = reef_shape(0.58, 0.8, 0.2, 1.0, min_t, max_t, 0.7);
        assert_eq!(outside, 0.0);
        assert!((crest - 1.0).abs() < 1e-9);
    }

    #[test]
    fn tuya_has_a_flat_top_and_a_cliff_then_ice_edge() {
        assert!(tuya_shape(0.015) > 0.99);
        assert!(tuya_shape(0.1) < tuya_shape(0.03));
        assert_eq!(tuya_shape(0.5), 0.0);
        assert!(tuya_icy_shape(0.5) > 0.4);
    }
}
