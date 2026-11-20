//! TerraFirmaCraft's stratovolcanoes (EUPL 1.2): the height shapes of the five `VolcanoVariants`
//! (Fuji, Crater Lake, Tahoma, batholith and Kelimutu) and the variant choice of
//! `CenteredFeatureNoise.stratovolcano`. The blocks a volcano places are not ported here.

use super::tfc_noise::{Cell, Cellular1D, Cellular2D, OpenSimplex2D, clamped_map, diamond_angle};
use super::tfc_region::hash_double;
use super::tfc_surface::{
    Cell as SurfaceCell, Column, RockBlock, State, SurfaceCtx, altitude_slope_depth, blob, fixed,
    state_at, transition,
};

const SEA_LEVEL_Y: i32 = 63;
const MAX_RADIUS_SCALE: f64 = 0.45;

fn f(value: f32) -> f64 {
    f64::from(value)
}

/// `Mth.map(double, ...)`.
fn map(value: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    let t = (value - in_min) / (in_max - in_min);
    out_min + t * (out_max - out_min)
}

/// `Mth.sqrt((float) f1)`, which TFC feeds to `Mth.map`.
fn float_sqrt(value: f64) -> f64 {
    f64::from((value as f32).sqrt())
}

fn hyperbolic_section(x: f64, x_intercept: f64, y_intercept: f64) -> f64 {
    y_intercept * ((2.0 / (x / x_intercept + 1.0)) - 1.0)
}

fn scale_shape(shape: f64, base: f64, scale: f64) -> f64 {
    f64::from(SEA_LEVEL_Y) + base + shape * scale
}

fn wrap_angle(a: f64) -> f64 {
    if a >= 4.0 {
        a - 4.0
    } else if a < 0.0 {
        a + 4.0
    } else {
        a
    }
}

fn simple_radial_shape(r: f64, r_crater: f64, crater_depth_scale: f64) -> f64 {
    if r > r_crater {
        let x = map(r, r_crater, 1.0, 0.0, 1.0);
        hyperbolic_section(x, 1.0, 1.0)
    } else {
        let crater_base_height = 1.0 - crater_depth_scale * r_crater;
        hyperbolic_section(r_crater - r, r_crater, crater_depth_scale * r_crater)
            + crater_base_height
    }
}

fn simple_radial_shape_with_skirt(
    r: f64,
    r_crater: f64,
    crater_depth_scale: f64,
    f1: f64,
    f2: f64,
    skirt_slope: f64,
) -> f64 {
    if r >= 1.0 {
        (1.0 - r) * clamped_map(f2 - f1, 0.0, 0.1, skirt_slope, 1.0)
    } else {
        simple_radial_shape(r, r_crater, crater_depth_scale)
    }
}

fn simple_radial_shape_no_skirt(r: f64, r_crater: f64, crater_depth_scale: f64) -> f64 {
    if r >= 1.0 {
        0.0
    } else {
        simple_radial_shape(r, r_crater, crater_depth_scale)
    }
}

fn truncated_cone(r: f64, r_crater: f64) -> f64 {
    if r > 1.0 {
        1.0 - r
    } else if r > r_crater {
        hyperbolic_section(map(r, r_crater, 1.0, 0.0, 1.0), 1.0, 1.0)
    } else {
        1.0
    }
}

fn cone(r: f64, r_crater: f64) -> f64 {
    map(r, 1.0, r_crater, 0.0, 1.0)
}

fn crater(r: f64, r_crater: f64, crater_depth_scale: f64) -> f64 {
    let depth = r_crater * crater_depth_scale;
    (1.0 - depth) + depth * r * r
}

/// `calculateCircumferentialErosion`.
#[allow(clippy::too_many_arguments)]
fn circumferential_erosion(
    cell: &Cell,
    r_inner0: f64,
    r_inner1: f64,
    r_outer1: f64,
    r_outer0: f64,
    r: f64,
    min_ridge_count: i32,
    added_ridge_count: i32,
    ridge_warp: f64,
) -> f64 {
    let noise = hash_double(cell.noise, 213);
    let ridges = (noise * f64::from(added_ridge_count)) as i32 + min_ridge_count;
    let a = wrap_angle(cell.angle + ridge_warp / f64::from(ridges));
    let erosion = 2.0 - noise;
    let fluvial_shape = ((a * 0.5 * f64::from(ridges)) % 2.0 - 1.0).abs();
    let easing = if r <= r_inner1 {
        clamped_map(r, r_inner0, r_inner1, 0.0, 1.0)
    } else {
        clamped_map(r, r_outer0, r_outer1, 0.0, 1.0)
    };
    let ridge_scale = clamped_map(
        f64::from(ridges),
        f64::from(min_ridge_count),
        f64::from(min_ridge_count + added_ridge_count),
        1.5,
        0.5,
    );
    (fluvial_shape - 1.0) * erosion * easing * ridge_scale
}

/// `calculateOffsetCircumferentialErosion`, for a cone whose centre is not the cell centre.
#[allow(clippy::too_many_arguments)]
fn offset_circumferential_erosion(
    r_inner0: f64,
    r_inner1: f64,
    r_outer1: f64,
    r_outer0: f64,
    r: f64,
    a_in: f64,
    ridges: i32,
    ridge_warp: f64,
    noise_in: f64,
) -> f64 {
    let noise = hash_double(noise_in, 213);
    let a = wrap_angle(a_in + ridge_warp / f64::from(ridges));
    let erosion = 2.0 - noise;
    let fluvial_shape = ((a * 0.5 * f64::from(ridges)) % 2.0 - 1.0).abs();
    let easing = if r <= r_inner1 {
        clamped_map(r, r_inner0, r_inner1, 0.0, 1.0)
    } else {
        clamped_map(r, r_outer0, r_outer1, 0.0, 1.0)
    };
    let ridge_scale = clamped_map(f64::from(ridges), 3.0, 10.0, 1.5, 0.5);
    (fluvial_shape - 1.0) * erosion * easing * ridge_scale
}

/// `distortOceanicFootprint`: underwater volcanoes get an irregular, lobed outline.
#[allow(clippy::too_many_arguments)]
fn distort_oceanic_footprint(
    r: f64,
    terrain_height: f64,
    x: i32,
    z: i32,
    cell: &Cell,
    footprint: &OpenSimplex2D,
    warp_start: f64,
    warp_full: f64,
) -> f64 {
    let underwater = clamped_map(f64::from(SEA_LEVEL_Y) - terrain_height, 0.0, 18.0, 0.0, 1.0);
    if underwater <= 0.0 {
        return r;
    }
    let mut lower = clamped_map(r, warp_start, warp_full, 0.0, 1.0);
    lower = lower * lower * (3.0 - 2.0 * lower);
    if lower <= 0.0 {
        return r;
    }
    let angle = (f64::from(z) - cell.y).atan2(f64::from(x) - cell.x);
    let n = cell.noise;
    let tau = 2.0 * std::f64::consts::PI;
    let phase2 = tau * hash_double(n, 6101);
    let phase3 = tau * hash_double(n, 6102);
    let phase5 = tau * hash_double(n, 6103);
    let elongation = 0.02 + 0.015 * hash_double(n, 6104);
    let three = 0.035 + 0.02 * hash_double(n, 6105);
    let five = 0.01 + 0.01 * hash_double(n, 6106);
    let scale = 1.0
        + elongation * (2.0 * (angle - phase2)).cos()
        + three * (3.0 * (angle - phase3)).cos()
        + five * (5.0 * (angle - phase5)).cos()
        + footprint.noise(f64::from(x), f64::from(z));
    let scale = scale.clamp(0.86, 1.18);
    let distorted = r / scale;
    let t = underwater * lower;
    r + t * (distorted - r)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Fuji,
    CraterLake,
    Tahoma,
    Batholith,
    Kelimutu,
}

/// One variant with its noises (all derived from the level seed).
struct Variant {
    kind: Kind,
    ridge_warp: OpenSimplex2D,
    rim_warp: OpenSimplex2D,
    texture: OpenSimplex2D,
    skirt: OpenSimplex2D,
    footprint: OpenSimplex2D,
}

/// The inputs every variant's height function takes besides the position.
struct Shape {
    height_in: f64,
    x: i32,
    z: i32,
    max_diam: f64,
    scale: f64,
    base: f64,
}

impl Variant {
    fn new(kind: Kind, s: i64) -> Self {
        let n = |offset: i64| OpenSimplex2D::new(s.wrapping_add(offset));
        let ridge = |o: OpenSimplex2D| {
            o.octaves(2)
                .scaled(f(-0.4f32), f(0.4f32))
                .spread(f(0.09f32))
        };
        let skirt = |amp: f64| n(2982).octaves(3).spread(0.09).scaled(-amp, amp);
        match kind {
            Kind::Fuji => Self {
                kind,
                ridge_warp: ridge(n(23)),
                rim_warp: n(0),
                texture: n(0),
                skirt: skirt(0.05),
                footprint: n(6143).octaves(2).spread(0.0028).scaled(-0.06, 0.06),
            },
            Kind::CraterLake => Self {
                kind,
                ridge_warp: ridge(n(23)),
                rim_warp: n(1431)
                    .octaves(2)
                    .scaled(f(-0.08f32), f(0.08f32))
                    .spread(f(0.03f32)),
                texture: n(24482).octaves(3).spread(0.06).scaled(0.85, 1.08),
                skirt: skirt(0.09),
                footprint: n(26143).octaves(2).spread(0.0028).scaled(-0.05, 0.05),
            },
            Kind::Tahoma => Self {
                kind,
                ridge_warp: n(23)
                    .octaves(2)
                    .scaled(f(-0.5f32), f(0.5f32))
                    .spread(f(0.03f32)),
                rim_warp: n(0),
                texture: n(248_582).octaves(3).spread(0.06).scaled(0.94, 1.06),
                skirt: skirt(0.05),
                footprint: n(16143).octaves(2).spread(0.0035).scaled(-0.06, 0.06),
            },
            Kind::Batholith => Self {
                kind,
                // `radialWarpNoise`
                ridge_warp: n(133)
                    .octaves(2)
                    .scaled(0.0, f(0.006f32))
                    .spread(f(0.05f32)),
                rim_warp: n(0),
                texture: n(24852).octaves(4).spread(0.2).scaled(0.8, 1.2),
                skirt: n(0),
                footprint: n(0),
            },
            Kind::Kelimutu => Self {
                kind,
                ridge_warp: ridge(n(23)),
                rim_warp: n(0),
                texture: n(24482).octaves(3).spread(0.06).scaled(0.85, 1.08),
                skirt: skirt(0.05),
                footprint: n(36143).octaves(2).spread(0.003).scaled(-0.06, 0.06),
            },
        }
    }

    fn height(&self, p: &Shape, cell: &Cell) -> f64 {
        let land = self.land_height(p, cell);
        match self.kind {
            Kind::CraterLake | Kind::Kelimutu => land.max(self.fluid_height(p, cell)),
            _ => land,
        }
    }

    fn land_height(&self, p: &Shape, cell: &Cell) -> f64 {
        match self.kind {
            Kind::Fuji => self.fuji(p, cell),
            Kind::CraterLake => self.crater_lake(p, cell),
            Kind::Tahoma => self.tahoma(p, cell),
            Kind::Batholith => self.batholith(p, cell),
            Kind::Kelimutu => self.kelimutu(p, cell),
        }
    }

    fn fluid_height(&self, p: &Shape, cell: &Cell) -> f64 {
        if self.kind != Kind::CraterLake {
            return 0.0;
        }
        let r = map(
            float_sqrt(cell.f1),
            0.0,
            p.max_diam * MAX_RADIUS_SCALE,
            0.0,
            1.0,
        );
        let crater_size = 0.5 + self.rim_warp.noise(f64::from(p.x), f64::from(p.z));
        if r < crater_size {
            scale_shape(0.2, p.base, p.scale)
        } else {
            0.0
        }
    }

    fn fuji(&self, p: &Shape, cell: &Cell) -> f64 {
        let (x, z) = (f64::from(p.x), f64::from(p.z));
        let base_r = map(
            float_sqrt(cell.f1),
            0.0,
            p.max_diam * MAX_RADIUS_SCALE,
            0.0,
            1.0,
        );
        let r = distort_oceanic_footprint(
            base_r,
            p.height_in,
            p.x,
            p.z,
            cell,
            &self.footprint,
            0.58,
            0.94,
        );
        let crater_size = 0.04 + 0.1 * hash_double(cell.noise, 10);
        let mut shape =
            p.max_diam * simple_radial_shape_with_skirt(r, crater_size, 0.9, cell.f1, cell.f2, 2.0);
        shape *= 1.0
            - 0.1
                * circumferential_erosion(
                    cell,
                    crater_size,
                    0.2,
                    0.9,
                    1.0,
                    r,
                    3,
                    (p.max_diam * 16.0) as i32,
                    self.ridge_warp.noise(x, z),
                );
        if r > 0.65 {
            shape += self.skirt.noise(x, z) * clamped_map(r, 0.65, 1.2, 0.0, 1.0);
        }
        scale_shape(shape, p.base, p.scale).max(p.height_in)
    }

    fn crater_lake(&self, p: &Shape, cell: &Cell) -> f64 {
        let (x, z) = (f64::from(p.x), f64::from(p.z));
        let mut height_in = p.height_in;
        let f1 = cell.f1;
        let base_r = map(float_sqrt(f1), 0.0, p.max_diam * MAX_RADIUS_SCALE, 0.0, 1.0);
        let r = distort_oceanic_footprint(
            base_r,
            height_in,
            p.x,
            p.z,
            cell,
            &self.footprint,
            0.68,
            0.97,
        );
        let crater_size = 0.5 + self.rim_warp.noise(x, z);
        let rim_height = 0.42;
        let mut shape =
            rim_height * simple_radial_shape_with_skirt(r, crater_size, 2.0, f1, cell.f2, 5.0);
        let warp = self.ridge_warp.noise(x, z);
        shape *= 1.0
            - 0.12
                * circumferential_erosion(
                    cell,
                    crater_size,
                    crater_size + 0.06,
                    0.95,
                    1.0,
                    r,
                    24,
                    (p.max_diam * 32.0) as i32,
                    warp,
                );
        shape *= 1.0
            - 0.08
                * circumferential_erosion(
                    cell,
                    crater_size * 0.4,
                    crater_size * 0.8,
                    crater_size * 0.8,
                    crater_size,
                    r,
                    24,
                    (p.max_diam * 32.0) as i32,
                    warp,
                );
        if r < crater_size {
            // "Wizard Island": a small cone with a random height and offset.
            let noise = cell.noise;
            let apex_height = rim_height * (0.65 + 0.45 * hash_double(noise, 8));
            let x_offset = -45.0 + 90.0 * hash_double(noise, 6);
            let z_offset = -45.0 + 90.0 * hash_double(noise, 7);
            shape = self.add_offset_cone(
                shape,
                cell.x + x_offset,
                cell.y + z_offset,
                x,
                z,
                0.0,
                apex_height,
                apex_height * p.max_diam * 300.0,
                noise,
            );
            height_in = f64::from(SEA_LEVEL_Y) + 10.0 + p.base;
        } else if r > 1.0 {
            shape += self.skirt.noise(x, z) * clamped_map(r, 1.0, 1.2, 0.0, 1.0);
        }
        shape *= self.texture.noise(x, z);
        shape = scale_shape(shape, p.base, p.scale);
        if r > crater_size && height_in > shape {
            height_in = clamped_map(r, crater_size + 0.1, crater_size, height_in, shape);
        }
        shape.max(height_in)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_offset_cone(
        &self,
        shape_in: f64,
        x_center: f64,
        z_center: f64,
        x: f64,
        z: f64,
        base_height: f64,
        apex_height: f64,
        max_radius: f64,
        noise: f64,
    ) -> f64 {
        let max_diam = apex_height - base_height;
        let (dx, dz) = (x - x_center, z - z_center);
        let r = map((dx * dx + dz * dz).sqrt(), 0.0, max_radius, 0.0, 1.0);
        let a = diamond_angle(dx, dz) + hash_double(noise, 3213);
        let crater_size = 0.03 + 0.03 * hash_double(noise, 10);
        let mut shape = simple_radial_shape_no_skirt(r, crater_size, 1.1);
        let ridges = (3.0 + hash_double(noise, 1313) * max_diam * 12.0) as i32;
        shape *= 0.9
            + 0.1
                * offset_circumferential_erosion(
                    crater_size,
                    0.2,
                    0.9,
                    1.0,
                    r,
                    a,
                    ridges,
                    self.ridge_warp.noise(x, z),
                    noise,
                );
        shape = map(shape, 0.0, 1.0, base_height, apex_height);
        shape.max(shape_in)
    }

    fn tahoma(&self, p: &Shape, cell: &Cell) -> f64 {
        let (x, z) = (f64::from(p.x), f64::from(p.z));
        let noise = cell.noise;
        let crater_size = 0.1 + 0.15 * hash_double(noise, 1013);
        let transition_length = 0.15;
        let transition_center = crater_size + 2.0 * transition_length;
        let inner_transition = transition_center - transition_length;
        let outer_transition = transition_center + transition_length;
        let base_r = map(
            float_sqrt(cell.f1),
            0.0,
            p.max_diam * MAX_RADIUS_SCALE,
            0.0,
            1.0,
        );
        let r = distort_oceanic_footprint(
            base_r,
            p.height_in,
            p.x,
            p.z,
            cell,
            &self.footprint,
            0.60,
            0.95,
        );
        let mut shape =
            p.max_diam * simple_radial_shape_with_skirt(r, crater_size, 0.9, cell.f1, cell.f2, 2.0);
        let warp = self.ridge_warp.noise(x, z);
        let inner_ridge = if r > outer_transition {
            0.0
        } else {
            circumferential_erosion(
                cell,
                crater_size,
                crater_size + transition_length,
                inner_transition,
                outer_transition,
                r,
                3,
                (p.max_diam * 8.0) as i32,
                warp,
            )
        };
        let outer_ridge = if r < inner_transition {
            0.0
        } else {
            circumferential_erosion(
                cell,
                inner_transition,
                outer_transition,
                1.0 - transition_length,
                1.0,
                r,
                6,
                2 * (p.max_diam * 8.0) as i32,
                warp,
            )
        };
        shape -= clamped_map(r, crater_size, transition_center, 0.0, 0.12)
            * (1.6 + inner_ridge + outer_ridge);
        if r < 0.65 {
            let peaks = (1.0 + hash_double(noise, 978) * 3.0) as i32;
            shape *= 1.0
                - 0.52
                    * crater_size
                    * self.variable_crater_rim(
                        cell,
                        0.5 * crater_size,
                        crater_size,
                        2.0 * crater_size,
                        r,
                        peaks,
                        x,
                        z,
                    );
            if r < crater_size {
                let inner_cone_scale = 1.1 * (0.65 + 0.35 * hash_double(noise, 8973)) * crater_size;
                let crater_base_height = p.max_diam * (1.0 - 0.9 * crater_size);
                let inner_cone = crater_base_height
                    + inner_cone_scale
                        * simple_radial_shape_no_skirt(
                            clamped_map(r, 0.0, 0.9 * crater_size, 0.0, 1.0),
                            crater_size,
                            1.0,
                        );
                shape = shape.max(inner_cone);
            }
        } else {
            shape += self.skirt.noise(x, z) * clamped_map(r, 0.65, 1.2, 0.0, 1.0);
        }
        scale_shape(shape, p.base, p.scale).max(p.height_in)
    }

    #[allow(clippy::too_many_arguments)]
    fn variable_crater_rim(
        &self,
        cell: &Cell,
        r_inner: f64,
        r_crater: f64,
        r_outer: f64,
        r: f64,
        peak_count: i32,
        x: f64,
        z: f64,
    ) -> f64 {
        let noise = hash_double(cell.noise, 43);
        let a = wrap_angle(cell.angle + noise);
        let peak_shape = ((a * 0.5 * f64::from(peak_count)) % 2.0 - 1.0).abs();
        if r < r_inner || r > r_outer {
            return 0.0;
        }
        let easing = if r <= r_crater {
            map(r, r_inner, r_crater, 0.0, 1.0)
        } else {
            map(r, r_crater, r_outer, 1.0, 0.0)
        };
        peak_shape * easing * self.texture.noise(x, z)
    }

    fn batholith(&self, p: &Shape, cell: &Cell) -> f64 {
        let (x, z) = (f64::from(p.x), f64::from(p.z));
        let max_r = p.max_diam * 0.5 - 0.08 * hash_double(cell.noise, 1398);
        let erosion_warp = if hash_double(cell.noise, 1348) > 0.6 {
            self.ridge_warp.noise(x, z)
        } else {
            0.0
        };
        let r2 = map(cell.f1 + erosion_warp, 0.0, max_r * max_r, 0.0, 1.0);
        let vertical_scale = p.max_diam * (0.8 + 0.4 * hash_double(cell.noise, 83));
        let mut shape = vertical_scale * (1.0 - r2);
        shape *= self.texture.noise(x, z);
        scale_shape(shape, p.base, p.scale).max(p.height_in)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_offset_truncated_cone(
        &self,
        shape_in: f64,
        x_center: f64,
        z_center: f64,
        x: f64,
        z: f64,
        rim_height: f64,
        max_radius: f64,
        crater_size: f64,
        ridge_count: i32,
        noise: f64,
    ) -> f64 {
        let (dx, dz) = (x - x_center, z - z_center);
        let r = map((dx * dx + dz * dz).sqrt(), 0.0, max_radius, 0.0, 1.0);
        let a = diamond_angle(dx, dz) + hash_double(noise, 3213);
        let mut shape = rim_height * truncated_cone(r, crater_size);
        shape *= 1.0
            - 0.15
                * offset_circumferential_erosion(
                    crater_size,
                    0.2,
                    0.9,
                    1.0,
                    r,
                    a,
                    ridge_count,
                    self.ridge_warp.noise(x, z),
                    noise,
                );
        shape.max(shape_in)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_offset_crater(
        shape_in: f64,
        x_center: f64,
        z_center: f64,
        x: f64,
        z: f64,
        base_height: f64,
        rim_height: f64,
        max_radius: f64,
        crater_size: f64,
        crater_depth_scale: f64,
    ) -> f64 {
        let (dx, dz) = (x - x_center, z - z_center);
        let r = map(
            (dx * dx + dz * dz).sqrt(),
            0.0,
            crater_size * max_radius,
            0.0,
            1.0,
        );
        let mut shape = rim_height * crater(r, crater_size, crater_depth_scale);
        shape = map(shape, 0.0, 1.0, base_height, rim_height);
        shape.min(shape_in)
    }

    fn kelimutu(&self, p: &Shape, cell: &Cell) -> f64 {
        let (x, z) = (f64::from(p.x), f64::from(p.z));
        let noise = cell.noise;
        let f1 = cell.f1;
        let base_r = map(float_sqrt(f1), 0.0, p.max_diam * MAX_RADIUS_SCALE, 0.0, 1.0);
        let r = distort_oceanic_footprint(
            base_r,
            p.height_in,
            p.x,
            p.z,
            cell,
            &self.footprint,
            0.60,
            0.92,
        );
        let crater0_size = 0.06 + 0.06 * hash_double(noise, 67);
        let rim_height0 = 0.65;
        let mut shape = rim_height0 * truncated_cone(r, crater0_size);
        shape *= 1.0
            - 0.12
                * circumferential_erosion(
                    cell,
                    crater0_size,
                    crater0_size + 0.06,
                    0.95,
                    1.0,
                    r,
                    6,
                    (p.max_diam * 12.0) as i32,
                    self.ridge_warp.noise(x, z),
                );

        // Secondary cone with a random height and offset.
        let rand_x1 = 2.0 * (0.5 - hash_double(noise, 68));
        let rand_z1 = 2.0 * (0.5 - hash_double(noise, 69));
        let x_offset1 = if rand_x1 > 0.0 {
            20.0 + rand_x1 * 70.0
        } else {
            -20.0 + rand_x1 * 70.0
        };
        let z_offset1 = if rand_z1 > 0.0 {
            20.0 + rand_z1 * 70.0
        } else {
            -20.0 + rand_z1 * 70.0
        };
        let crater1_size = 0.06 + 0.08 * (rand_x1.abs() + rand_z1.abs());
        let rand_rim1 = hash_double(noise, 70);
        let rim_height1 = rim_height0 * (0.8 + 0.2 * rand_rim1);
        let ridge_count1 = 3 + (rim_height1 * 8.0 + hash_double(noise, 101) * 4.0) as i32;
        shape = self.add_offset_truncated_cone(
            shape,
            cell.x + x_offset1,
            cell.y + z_offset1,
            x,
            z,
            rim_height1,
            rim_height1 * p.max_diam * 300.0,
            crater1_size,
            ridge_count1,
            noise,
        );

        let mut crater_shape =
            rim_height0 * crater(r / crater0_size, crater0_size, 0.6 + hash_double(noise, 74));
        crater_shape = Self::add_offset_crater(
            crater_shape,
            cell.x + x_offset1,
            cell.y + z_offset1,
            x,
            z,
            0.0,
            rim_height1,
            rim_height1 * p.max_diam * 300.0,
            crater1_size,
            1.3 + rand_rim1,
        );

        // A third cone, most of the time.
        if hash_double(noise, 1066) > 0.3 {
            let rand_x2 = 2.0 * (0.5 - hash_double(noise, 71));
            let rand_z2 = 2.0 * (0.5 - hash_double(noise, 72));
            let x_offset2 = if rand_x2 > 0.0 {
                20.0 + rand_x2 * 70.0
            } else {
                -20.0 + rand_x2 * 73.0
            };
            let z_offset2 = if rand_z2 > 0.0 {
                20.0 + rand_z2 * 70.0
            } else {
                -20.0 + rand_z2 * 74.0
            };
            let crater2_size = 0.06 + 0.08 * (rand_x2.abs() + rand_z2.abs());
            let rand_rim2 = hash_double(noise, 73);
            let rim_height2 = rim_height0 * (0.7 + 0.3 * rand_rim2);
            let ridge_count2 = 3 + (rim_height2 * 8.0 + hash_double(noise, 101) * 4.0) as i32;
            shape = self.add_offset_truncated_cone(
                shape,
                cell.x + x_offset2,
                cell.y + z_offset2,
                x,
                z,
                rim_height2,
                rim_height2 * p.max_diam * 300.0,
                crater2_size,
                ridge_count2,
                noise,
            );
            crater_shape = Self::add_offset_crater(
                crater_shape,
                cell.x + x_offset2,
                cell.y + z_offset2,
                x,
                z,
                0.0,
                rim_height2,
                rim_height2 * p.max_diam * 300.0,
                crater2_size,
                1.3 + rand_rim2,
            );
        }

        shape = shape.min(crater_shape);
        let cratered_height_in = p.height_in.min(scale_shape(crater_shape, p.base, p.scale));
        shape *= self.texture.noise(x, z);
        if r > 0.7 {
            shape += self.skirt.noise(x, z) * clamped_map(r, 0.7, 1.0, 0.0, 1.0);
            shape -= clamped_map(cell.f2 - cell.f1, 0.0, 0.3, 0.2, 0.0);
        }
        scale_shape(shape, p.base, p.scale).max(cratered_height_in)
    }
}

/// `CenteredFeatureNoise.stratovolcano`: the cells and the five variants.
pub struct Stratovolcanoes {
    cells: Cellular2D,
    fuji: Variant,
    crater_lake: Variant,
    tahoma: Variant,
    batholith: Variant,
    kelimutu: Variant,
    strata: Cellular1D,
}

impl Stratovolcanoes {
    pub fn new(seed: i64) -> Self {
        Self {
            cells: Cellular2D::with_sample(seed, 2).spread(f(0.0021f32)),
            fuji: Variant::new(Kind::Fuji, seed),
            crater_lake: Variant::new(Kind::CraterLake, seed),
            tahoma: Variant::new(Kind::Tahoma, seed),
            batholith: Variant::new(Kind::Batholith, seed),
            kelimutu: Variant::new(Kind::Kelimutu, seed),
            strata: Cellular1D::new(seed).spread(0.25),
        }
    }

    pub fn cell(&self, x: f64, z: f64) -> Cell {
        self.cells.cell(x, z)
    }

    pub fn cellular(&self) -> &Cellular2D {
        &self.cells
    }

    pub fn variant_name(kind: Kind) -> &'static str {
        match kind {
            Kind::Fuji => "fuji",
            Kind::CraterLake => "crater_lake",
            Kind::Tahoma => "tahoma",
            Kind::Batholith => "batholith",
            Kind::Kelimutu => "kelimutu",
        }
    }

    /// `maxSafeDiameterSquared`.
    fn max_safe_diameter_squared(&self, cell: &Cell) -> f64 {
        let mut f2 = self.cells.cell(cell.x + 1.0, cell.y).f2;
        f2 = f2.min(self.cells.cell(cell.x - 1.0, cell.y).f2);
        f2 = f2.min(self.cells.cell(cell.x, cell.y + 1.0).f2);
        f2.min(self.cells.cell(cell.x, cell.y - 1.0).f2)
    }

    fn max_diameter(&self, cell: &Cell) -> f64 {
        self.max_safe_diameter_squared(cell).min(1.0).sqrt()
    }

    /// `getVolcanoVariant`.
    pub fn variant_kind(&self, cell: &Cell) -> Kind {
        let max_diameter = self.max_diameter(cell);
        let noise = hash_double(cell.noise, 317);
        if max_diameter >= 1.0 {
            if noise > 0.4 {
                Kind::Kelimutu
            } else {
                Kind::CraterLake
            }
        } else if max_diameter >= 0.7 {
            if noise > 0.7 {
                Kind::Kelimutu
            } else if noise > 0.45 {
                Kind::CraterLake
            } else if noise > 0.2 {
                Kind::Tahoma
            } else {
                Kind::Fuji
            }
        } else if max_diameter < 0.4 {
            Kind::Batholith
        } else if max_diameter < 0.55 {
            if noise > 0.5 {
                Kind::Batholith
            } else {
                Kind::Fuji
            }
        } else {
            Kind::Fuji
        }
    }

    fn variant(&self, kind: Kind) -> &Variant {
        match kind {
            Kind::Fuji => &self.fuji,
            Kind::CraterLake => &self.crater_lake,
            Kind::Tahoma => &self.tahoma,
            Kind::Batholith => &self.batholith,
            Kind::Kelimutu => &self.kelimutu,
        }
    }

    /// `modifyHeight`: the terrain height with this cell's stratovolcano on it.
    pub fn height(
        &self,
        cell: &Cell,
        x: i32,
        z: i32,
        base: i32,
        scale: i32,
        height_in: f64,
    ) -> f64 {
        let max_diam = self.max_diameter(cell);
        let kind = self.variant_kind(cell);
        let p = Shape {
            height_in,
            x,
            z,
            max_diam,
            scale: f64::from(scale),
            base: f64::from(base),
        };
        self.variant(kind).height(&p, cell)
    }
}

// ---------------------------------------------------------------------------------------------
// Surface: strata, volcanic soils and crater lakes (`VolcanoVariants.buildSurface`).
// ---------------------------------------------------------------------------------------------

/// What `StratovolcanoSurfaceBuilder` needs besides the column itself.
pub struct SurfaceArgs {
    /// `oceanFloorY`: the ocean floor under water, else the start height of the column.
    pub ocean_floor_y: i32,
    /// The height before any volcano raised the ground.
    pub pre_volcanic: i32,
    /// `getCenteredFeatureBaseHeight` and `getCenteredFeatureScaleHeight` of the volcano's biome.
    pub base: i32,
    pub scale: i32,
}

pub(super) fn state_in(column: &Column, y: i32) -> SurfaceCell {
    let i = y - column.min_y;
    if i < 0 || i as usize >= column.cells.len() {
        SurfaceCell::Air
    } else {
        column.cells[i as usize].clone()
    }
}

pub(super) fn put_name(column: &mut Column, y: i32, name: String) {
    let i = y - column.min_y;
    if i >= 0 && (i as usize) < column.cells.len() {
        column.cells[i as usize] = SurfaceCell::Placed(name);
    }
}

pub(super) fn put_state(column: &mut Column, ctx: &mut SurfaceCtx, y: i32, state: &State) {
    ctx.y = y;
    let name = state_at(state, ctx);
    put_name(column, y, name);
}

fn gravel_of(rock: &'static str) -> State {
    fixed(rock)
}

/// `SurfaceStates.VOLCANIC_TOP_GRASS_TO_*`: andisol soils are the vanilla grass and dirt here.
pub(super) fn volcanic_top(dry: State) -> State {
    let grass = || fixed("minecraft:grass_block");
    let coarse = || fixed("minecraft:coarse_dirt");
    let snow = || fixed("minecraft:snow_block");
    let mut regions = vec![
        snow(),
        snow(),
        transition(snow(), dry.clone()),
        dry.clone(),
        transition(dry, coarse()),
        coarse(),
        transition(coarse(), grass()),
    ];
    regions.extend((0..11).map(|_| grass()));
    State::Regions(regions)
}

/// `SurfaceStates.VOLCANIC_MID_DIRT_TO_*`.
pub(super) fn volcanic_mid(dry: State) -> State {
    let dirt = || fixed("minecraft:dirt");
    let coarse = || fixed("minecraft:coarse_dirt");
    let ice = || fixed("minecraft:packed_ice");
    let mut regions = vec![
        ice(),
        blob(ice(), dry.clone()),
        dry.clone(),
        dry.clone(),
        transition(dry, coarse()),
        coarse(),
        transition(coarse(), dirt()),
    ];
    regions.extend((0..11).map(|_| dirt()));
    State::Regions(regions)
}

fn round_to_int(value: f64) -> i32 {
    (value + 0.5).floor() as i32
}

/// The variant's own crater test and heights, handed to the shared builders.
enum Plan {
    /// `buildStrataSurfaceOnly` (inside a crater).
    StrataOnly { pre_volcanic: i32, land: i32 },
    /// `buildNormalSurfaceWithStrata`.
    Normal {
        pre_volcanic: i32,
        land: i32,
        water: i32,
        fluid: &'static str,
    },
    /// `buildNormalBatholithSurface`.
    Batholith { granite: bool },
}

impl Stratovolcanoes {
    fn strata_block(&self, unerodedy: i32, y: i32, rock_noise_in: f64, kind: RockBlock) -> String {
        let n = self.strata.noise(f64::from(unerodedy - y));
        let rock = if n > 0.3 {
            "tuff"
        } else if n < -0.8 {
            "basalt"
        } else {
            let rock_noise = hash_double(rock_noise_in, 9673);
            if rock_noise > 0.67 {
                "andesite"
            } else if rock_noise > 0.33 {
                "dacite"
            } else {
                "rhyolite"
            }
        };
        let prefix = if kind == RockBlock::Gravel {
            "gravel"
        } else {
            "raw"
        };
        format!("tfc:{prefix}_{rock}")
    }

    fn build_strata_only(
        &self,
        column: &mut Column,
        unerodedy: i32,
        base_y: i32,
        land_height: i32,
        cell_noise: f64,
    ) {
        let rock_noise = hash_double(cell_noise, 856);
        let mut y = land_height;
        while y >= base_y {
            if state_in(column, y) == SurfaceCell::Rock {
                let name = self.strata_block(unerodedy, y, rock_noise, RockBlock::Raw);
                put_name(column, y, name);
            }
            y -= 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_normal_with_strata(
        &self,
        column: &mut Column,
        ctx: &mut SurfaceCtx,
        unerodedy: i32,
        base_y: i32,
        land_height: i32,
        water_height: i32,
        cell_noise: f64,
        top: &State,
        mid: &State,
        fluid: &'static str,
    ) {
        let rock_noise = hash_double(cell_noise, 856);
        let strata = |this: &Self, y: i32, kind: RockBlock| {
            this.strata_block(unerodedy, y, rock_noise, kind)
        };
        let shore_level = 4 + ctx.tide as i32;
        let is_under_water = water_height > land_height;
        if is_under_water {
            let mut y = water_height;
            while y >= land_height {
                put_name(column, y, fluid.to_owned());
                y -= 1;
            }
            put_name(
                column,
                land_height,
                strata(self, land_height, RockBlock::Gravel),
            );
            let mut y = land_height - 1;
            while y >= land_height - 5 {
                put_name(column, y, strata(self, y, RockBlock::Raw));
                y -= 1;
            }
        }
        let mut surface_depth = -1;
        let mut surface_state: Option<&State> = None;
        let mut y = land_height;
        while y >= base_y {
            let at = state_in(column, y);
            if at == SurfaceCell::Air {
                surface_depth = -1;
            } else if at == SurfaceCell::Rock {
                if surface_depth == -1 {
                    let surface_y = y;
                    if y < shore_level || is_under_water {
                        surface_depth = altitude_slope_depth(surface_y, ctx.slope, -1, 5);
                        if surface_depth < -1 {
                            surface_depth = 0;
                            put_name(column, y, strata(self, y, RockBlock::Raw));
                        } else if surface_depth == -1 {
                            surface_depth = 0;
                            put_name(column, y, strata(self, y, RockBlock::Gravel));
                        } else {
                            put_name(column, y, strata(self, y, RockBlock::Gravel));
                        }
                    } else {
                        surface_depth = altitude_slope_depth(surface_y, ctx.slope, -3, 5);
                        if surface_depth < -1 {
                            surface_depth = 0;
                            put_name(column, y, strata(self, y, RockBlock::Raw));
                        } else if surface_depth == -1 {
                            surface_depth = 0;
                            put_name(column, y, strata(self, y, RockBlock::Gravel));
                        } else {
                            put_state(column, ctx, y, top);
                        }
                        surface_state = Some(mid);
                    }
                } else if surface_depth > 0 {
                    surface_depth -= 1;
                    match surface_state {
                        None => put_name(column, y, strata(self, y, RockBlock::Gravel)),
                        Some(state) => put_state(column, ctx, y, state),
                    }
                } else {
                    put_name(column, y, strata(self, y, RockBlock::Raw));
                }
            }
            y -= 1;
        }
    }

    /// `buildNormalBatholithSurface`.
    fn build_batholith(
        &self,
        column: &mut Column,
        ctx: &mut SurfaceCtx,
        start_y: i32,
        end_y: i32,
        granite: bool,
    ) {
        let (dry, rock) = if granite {
            ("tfc:gravel_granite", "tfc:raw_granite")
        } else {
            ("tfc:gravel_diorite", "tfc:raw_diorite")
        };
        let top = volcanic_top(gravel_of(dry));
        let mid = volcanic_mid(gravel_of(dry));
        let under = gravel_of(dry);
        let rock_state = fixed(rock);
        let raw = State::Rock(RockBlock::Raw);
        let mut surface_depth = -1;
        let mut surface_y = 0;
        let (mut underwater_layer, mut first_layer) = (false, false);
        let mut surface_state: &State = &raw;
        let mut y = start_y;
        while y >= end_y {
            let at = state_in(column, y);
            if at == SurfaceCell::Air {
                surface_depth = -1;
            } else if at == SurfaceCell::Rock {
                if surface_depth == -1 {
                    surface_y = y;
                    first_layer = true;
                    if y < SEA_LEVEL_Y - 1 {
                        surface_depth = altitude_slope_depth(surface_y, ctx.slope, -1, 5);
                        if surface_depth < -1 {
                            surface_depth = 0;
                            put_state(column, ctx, y, &rock_state);
                        } else if surface_depth == -1 {
                            surface_depth = 0;
                            put_state(column, ctx, y, &under);
                        } else {
                            put_state(column, ctx, y, &under);
                        }
                        surface_state = &under;
                        underwater_layer = true;
                    } else {
                        surface_depth = altitude_slope_depth(surface_y, ctx.slope, -3, 5);
                        if surface_depth < -1 {
                            surface_depth = 0;
                            put_state(column, ctx, y, &rock_state);
                        } else if surface_depth == -1 {
                            surface_depth = 0;
                            put_state(column, ctx, y, &under);
                        } else {
                            put_state(column, ctx, y, &top);
                        }
                        surface_state = &mid;
                        underwater_layer = false;
                    }
                } else if surface_depth > 0 {
                    surface_depth -= 1;
                    put_state(column, ctx, y, surface_state);
                    if surface_depth == 0 && first_layer {
                        first_layer = false;
                        surface_depth = altitude_slope_depth(surface_y, ctx.slope, 0, 5);
                        surface_state = &under;
                        let _ = underwater_layer;
                    }
                } else {
                    put_state(column, ctx, y, &rock_state);
                }
            }
            y -= 1;
        }
    }

    /// The chunk-centre volcano's biome decides whether this runs; `args` carry its heights.
    /// Returns `true` when the volcano built the column, so the biome's own builder is skipped.
    pub fn build_surface(
        &self,
        column: &mut Column,
        ctx: &mut SurfaceCtx,
        args: &SurfaceArgs,
    ) -> bool {
        let (x, z) = (ctx.x, ctx.z);
        let cell = self.cell(f64::from(x), f64::from(z));
        let kind = self.variant_kind(&cell);
        let max_diam = self.max_diameter(&cell);
        let variant = self.variant(kind);
        let noise = cell.noise;
        let (base, scale) = (f64::from(args.base), f64::from(args.scale));
        let ofh = args.ocean_floor_y;
        let mut pre = args.pre_volcanic;
        let unerodedy = variant.unerodedy(max_diam, scale, base, &cell) as i32;
        let r_of = |c: &Cell| map(float_sqrt(c.f1), 0.0, max_diam * MAX_RADIUS_SCALE, 0.0, 1.0);
        let plan = match kind {
            Kind::Fuji | Kind::Tahoma => {
                if ofh <= pre + 2 {
                    return false;
                }
                let (crater_size, warp_start, warp_full) = if kind == Kind::Fuji {
                    (0.04 + 0.1 * hash_double(noise, 10), 0.58, 0.94)
                } else {
                    (0.7 * (0.20 + 0.25 * hash_double(noise, 1013)), 0.60, 0.95)
                };
                let r = distort_oceanic_footprint(
                    r_of(&cell),
                    f64::from(pre),
                    x,
                    z,
                    &cell,
                    &variant.footprint,
                    warp_start,
                    warp_full,
                );
                if r > 1.2 {
                    return false;
                }
                let strata_only = if kind == Kind::Fuji {
                    r <= crater_size || (hash_double(noise, 3199) < 0.6 && r <= 2.0 * crater_size)
                } else {
                    r <= crater_size
                };
                if strata_only {
                    Plan::StrataOnly {
                        pre_volcanic: pre,
                        land: ofh,
                    }
                } else {
                    Plan::Normal {
                        pre_volcanic: pre,
                        land: ofh,
                        water: 0,
                        fluid: "minecraft:lava",
                    }
                }
            }
            Kind::CraterLake => {
                let p = Shape {
                    height_in: f64::from(pre),
                    x,
                    z,
                    max_diam,
                    scale,
                    base,
                };
                let land = round_to_int(variant.land_height(&p, &cell));
                let water = round_to_int(variant.fluid_height(&p, &cell));
                let r_main = r_of(&cell);
                let main_crater = 0.5 + variant.rim_warp.noise(f64::from(x), f64::from(z));
                if r_main > 1.2 * main_crater && ofh <= pre + 2 {
                    return false;
                }
                let mut r2 = f64::MAX;
                if r_main < 1.2 * main_crater {
                    pre = clamped_map(
                        r_main,
                        main_crater + 0.1,
                        main_crater,
                        f64::from(pre),
                        f64::from(SEA_LEVEL_Y + 10),
                    ) as i32;
                    if r_main < main_crater {
                        let x_center = cell.x + (-45.0 + 90.0 * hash_double(noise, 6));
                        let z_center = cell.y + (-45.0 + 90.0 * hash_double(noise, 7));
                        r2 = (f64::from(x) - x_center) * (f64::from(x) - x_center)
                            + (f64::from(z) - z_center) * (f64::from(z) - z_center);
                    }
                }
                if r2 < 81.0 {
                    Plan::StrataOnly {
                        pre_volcanic: pre,
                        land: ofh,
                    }
                } else {
                    // TFC's Kawah Ijen spring water is plain water here (the mod has no fluids).
                    Plan::Normal {
                        pre_volcanic: pre,
                        land: land.max(ofh.min(SEA_LEVEL_Y)),
                        water,
                        fluid: "minecraft:water",
                    }
                }
            }
            Kind::Batholith => {
                if ofh <= pre + 2 {
                    return false;
                }
                Plan::Batholith {
                    granite: noise > 0.0,
                }
            }
            Kind::Kelimutu => {
                let base_r = r_of(&cell);
                let surface_r = distort_oceanic_footprint(
                    base_r,
                    f64::from(pre),
                    x,
                    z,
                    &cell,
                    &variant.footprint,
                    0.60,
                    0.92,
                );
                if surface_r > 1.2 {
                    return false;
                }
                let (fx, fz) = (f64::from(x), f64::from(z));
                let crater0 = 0.06 + 0.06 * hash_double(noise, 67);
                let mut inside = false;
                if base_r < 2.0 * crater0 {
                    inside = base_r < crater0;
                    pre = pre.min(clamped_map(
                        base_r,
                        2.0 * crater0,
                        1.6 * crater0,
                        f64::from(ofh - 1),
                        f64::from(ofh - 15),
                    ) as i32);
                }
                let rim0 = 0.65;
                if !inside {
                    let rx = 2.0 * (0.5 - hash_double(noise, 68));
                    let rz = 2.0 * (0.5 - hash_double(noise, 69));
                    let xc = cell.x
                        + if rx > 0.0 {
                            20.0 + rx * 70.0
                        } else {
                            -20.0 + rx * 70.0
                        };
                    let zc = cell.y
                        + if rz > 0.0 {
                            20.0 + rz * 70.0
                        } else {
                            -20.0 + rz * 70.0
                        };
                    let rim1 = rim0 * (0.8 + 0.2 * hash_double(noise, 70));
                    let crater1 = 0.06 + 0.08 * (rx.abs() + rz.abs());
                    let r1 = map(
                        ((fx - xc) * (fx - xc) + (fz - zc) * (fz - zc)).sqrt(),
                        0.0,
                        rim1 * max_diam * 300.0,
                        0.0,
                        1.0,
                    );
                    inside = r1 < crater1;
                    if r1 < 2.0 * crater1 {
                        pre = pre.min(clamped_map(
                            r1,
                            2.0 * crater1,
                            1.6 * crater1,
                            f64::from(ofh - 1),
                            f64::from(ofh - 15),
                        ) as i32);
                    }
                }
                if !inside && hash_double(noise, 1066) > 0.3 {
                    let rx = 2.0 * (0.5 - hash_double(noise, 71));
                    let rz = 2.0 * (0.5 - hash_double(noise, 72));
                    let xc = cell.x
                        + if rx > 0.0 {
                            20.0 + rx * 70.0
                        } else {
                            -20.0 + rx * 73.0
                        };
                    let zc = cell.y
                        + if rz > 0.0 {
                            20.0 + rz * 70.0
                        } else {
                            -20.0 + rz * 74.0
                        };
                    let rim2 = rim0 * (0.7 + 0.3 * hash_double(noise, 73));
                    let crater2 = 0.06 + 0.08 * (rx.abs() + rz.abs());
                    let r2 = map(
                        ((fx - xc) * (fx - xc) + (fz - zc) * (fz - zc)).sqrt(),
                        0.0,
                        rim2 * max_diam * 300.0,
                        0.0,
                        1.0,
                    );
                    inside = r2 < crater2;
                    if r2 < 4.0 * crater2 {
                        pre = pre.min(clamped_map(
                            r2,
                            2.0 * crater2,
                            1.6 * crater2,
                            f64::from(ofh),
                            f64::from(ofh - 15),
                        ) as i32);
                    }
                }
                if ofh <= pre + 2 {
                    return false;
                }
                if inside {
                    Plan::StrataOnly {
                        pre_volcanic: pre,
                        land: ofh,
                    }
                } else {
                    Plan::Normal {
                        pre_volcanic: pre,
                        land: ofh,
                        water: 0,
                        fluid: "minecraft:water",
                    }
                }
            }
        };
        match plan {
            Plan::StrataOnly { pre_volcanic, land } => {
                self.build_strata_only(column, unerodedy, pre_volcanic, land, noise);
            }
            Plan::Normal {
                pre_volcanic,
                land,
                water,
                fluid,
            } => {
                let top = volcanic_top(gravel_of("tfc:gravel_tuff"));
                let mid = volcanic_mid(gravel_of("tfc:gravel_tuff"));
                self.build_normal_with_strata(
                    column,
                    ctx,
                    unerodedy,
                    pre_volcanic,
                    land,
                    water,
                    noise,
                    &top,
                    &mid,
                    fluid,
                );
            }
            Plan::Batholith { granite } => {
                self.build_batholith(column, ctx, ofh, pre, granite);
            }
        }
        true
    }
}

impl Variant {
    /// `getUnerodedHeight`: the height of the plain cone the strata are measured from.
    fn unerodedy(&self, max_diam: f64, scale: f64, base: f64, cell: &Cell) -> f64 {
        let r = map(
            float_sqrt(cell.f1),
            0.0,
            max_diam * MAX_RADIUS_SCALE,
            0.0,
            1.0,
        );
        let noise = cell.noise;
        let shape = match self.kind {
            Kind::Fuji => max_diam * cone(r, 0.04 + 0.1 * hash_double(noise, 10)),
            Kind::CraterLake => 0.42 * cone(r, 0.5),
            Kind::Tahoma => max_diam * cone(r, 0.20 + 0.25 * hash_double(noise, 1013)),
            Kind::Kelimutu => 0.65 * cone(r, 0.06 + 0.06 * hash_double(noise, 67)),
            Kind::Batholith => return 0.0,
        };
        scale_shape(shape, base, scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyperbolic_section_hits_both_intercepts() {
        assert!((hyperbolic_section(0.0, 1.0, 1.0) - 1.0).abs() < 1e-12);
        assert!(hyperbolic_section(1.0, 1.0, 1.0).abs() < 1e-12);
    }

    #[test]
    fn cone_rises_to_the_crater_rim_radius() {
        assert!(cone(1.0, 0.1).abs() < 1e-12);
        assert!((cone(0.1, 0.1) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn every_variant_stands_above_the_plain_near_a_cell_centre_and_never_below_it() {
        let volcanoes = Stratovolcanoes::new(0x051c_a9e5);
        let mut seen = std::collections::HashSet::new();
        let mut raised = 0;
        for i in 0..60 {
            let (x, z) = (i * 970 - 20_000, (i * 53) % 17 * 1_000 - 9_000);
            let cell = volcanoes.cell(f64::from(x), f64::from(z));
            let kind = volcanoes.variant_kind(&cell);
            seen.insert(format!("{kind:?}"));
            let (cx, cz) = (cell.x as i32, cell.y as i32);
            let plain = 80.0;
            let h = volcanoes.height(&cell, cx, cz, 0, 120, plain);
            assert!(
                h >= plain || kind == Kind::Kelimutu,
                "{kind:?} dips below the plain: {h}"
            );
            if h > plain + 1.0 {
                raised += 1;
            }
        }
        assert!(seen.len() >= 2, "expected several variants, got {seen:?}");
        assert!(raised > 10, "volcano centres should rise above the plain");
    }

    #[test]
    fn heights_are_deterministic_and_finite() {
        let a = Stratovolcanoes::new(77);
        let b = Stratovolcanoes::new(77);
        for i in 0..200 {
            let (x, z) = (i * 313 - 30_000, i * 171 - 8_000);
            let cell = a.cell(f64::from(x), f64::from(z));
            let ha = a.height(&cell, x, z, 0, 100, 70.0);
            let hb = b.height(&b.cell(f64::from(x), f64::from(z)), x, z, 0, 100, 70.0);
            assert!(ha.is_finite());
            assert_eq!(ha, hb);
        }
    }
}
