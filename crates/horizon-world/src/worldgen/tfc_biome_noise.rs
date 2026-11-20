//! TerraFirmaCraft's biome height noises (`BiomeNoise`, EUPL 1.2), ported function by function.
//! Every float literal of the Java source is written as `f(0.3f32)` so it widens to `f64` the
//! way Java widens it, and chains follow Java's types: `OpenSimplex2D` methods until the first
//! `Noise2D` combinator.

use super::tfc_noise::{Cell, Cellular2D, Noise2D, OpenSimplex2D, OpenSimplex3D, clamped_map};
use super::tfc_region::{hash_double, units::CELL_WIDTH_IN_GRID};
use super::tfc_river::java_round;
use horizon_core::jmath;
use horizon_core::random::{LegacyRandom, RandomSource};

pub const SEA_LEVEL_Y: i32 = 63;
const SEA: f64 = 63.0;

fn f(x: f32) -> f64 {
    f64::from(x)
}
fn sea(offset: i32) -> f64 {
    f64::from(SEA_LEVEL_Y + offset)
}
fn simplex(seed: i64) -> OpenSimplex2D {
    OpenSimplex2D::new(seed)
}
fn n(o: OpenSimplex2D) -> Noise2D {
    Noise2D::from_simplex(o)
}
/// `Mth.map(double...)`.
fn mth_map(v: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    let t = (v - from_min) / (from_max - from_min);
    to_min + t * (to_max - to_min)
}
fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}
fn mth_floor(d: f64) -> i32 {
    let i = d as i32;
    if d < f64::from(i) { i - 1 } else { i }
}
/// `Helpers.triangle(amplitude, midpoint, frequency, value)`.
fn triangle(amplitude: f64, midpoint: f64, frequency: f64, value: f64) -> f64 {
    midpoint
        + amplitude
            * ((4.0 * frequency * value + 1.0
                - 4.0 * f64::from(mth_floor(frequency * value + 0.75)))
            .abs()
                - 1.0)
}
fn positive_modulo(numerator: f64, denominator: f64) -> f64 {
    (numerator % denominator + denominator) % denominator
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

pub fn connected_valley_base_noise(seed: i64) -> Noise2D {
    n(simplex(seed).spread(0.0025))
}

pub fn connected_valley_noise(seed: i64) -> Noise2D {
    connected_valley_base_noise(seed).abs()
}

pub fn badlands(seed: i64, height: i32, depth: f32) -> Noise2D {
    n(simplex(seed)
        .octaves(4)
        .spread(f(0.025f32))
        .scaled(sea(height), sea(height + 10)))
    .add(
        n(simplex(seed + 1).octaves(4).spread(f(0.04f32)))
            .ridged()
            .map(|x| f(1.3f32) * -(if x > 0.0 { x * x * x } else { f(0.5f32) * x }))
            .scaled4(-1.0, f(0.3f32), -1.0, 1.0)
            .terraces(15)
            .scaled(f64::from(-depth), 0.0),
    )
    .map(|x| {
        if x < SEA {
            SEA - f(0.3f32) * (SEA - x)
        } else {
            x
        }
    })
}

pub fn bryce_canyon(seed: i64) -> Noise2D {
    let mut generator = LegacyRandom::new(seed);
    let mut noise = n(simplex(generator.next_long())
        .octaves(4)
        .spread(f(0.1f32))
        .scaled(sea(2), sea(14)));
    for layer in 0..3 {
        let threshold = f(0.25f32);
        let delta = f(0.015f32);
        let a = simplex(generator.next_long())
            .octaves(3)
            .spread(f(0.02f32) + f(0.01f32) * f64::from(layer));
        let b = simplex(generator.next_long())
            .octaves(4)
            .spread(f(0.1f32))
            .scaled(5.0, 11.0);
        noise = noise.add(
            n(a).abs()
                .affine(1.0, f64::from(-0.05f32 * layer as f32))
                .map(move |t| clamped_map(t, threshold, threshold + delta, 0.0, 1.0))
                .lazy_product(n(b)),
        );
    }
    noise
}

pub fn canyons(seed: i64, min_height: i32, max_height: i32) -> Noise2D {
    let warp = simplex(seed)
        .octaves(4)
        .spread(f(0.03f32))
        .scaled(f(-100f32), f(100f32));
    n(simplex(seed + 1).octaves(4).spread(f(0.06f32)))
        .warped(warp)
        .map(|x| if x > 0.4 { x - f(0.8f32) } else { -x })
        .scaled4(f(-0.4f32), f(0.8f32), sea(min_height), sea(max_height))
}

pub fn hills(seed: i64, min_height: i32, max_height: i32) -> Noise2D {
    n(simplex(seed)
        .octaves(4)
        .spread(f(0.05f32))
        .scaled(sea(min_height), sea(max_height)))
}

pub fn constant(height: i32) -> Noise2D {
    Noise2D::constant(sea(height))
}

pub fn sharp_hills_map(input: f64) -> f64 {
    let (in0, in1, in2, in3, in4, in5) = (
        f(1.0f32),
        f(0.67f32),
        f(0.15f32),
        f(-0.15f32),
        f(-0.67f32),
        f(-1.0f32),
    );
    let (out0, out1, out2, out3, out4, out5) = (
        f(1.0f32),
        f(0.7f32),
        f(0.5f32),
        f(-0.5f32),
        f(-0.7f32),
        f(-1.0f32),
    );
    if input > in1 {
        mth_map(input, in1, in0, out1, out0)
    } else if input > in2 {
        mth_map(input, in2, in1, out2, out1)
    } else if input > in3 {
        mth_map(input, in3, in2, out3, out2)
    } else if input > in4 {
        mth_map(input, in4, in3, out4, out3)
    } else {
        mth_map(input, in5, in4, out5, out4)
    }
}

pub fn sharp_hills(seed: i64, min_height: f32, max_height: f32) -> Noise2D {
    let base = n(simplex(seed).octaves(4).spread(f(0.08f32)));
    let lerp_noise = n(simplex(seed + 7198234123)
        .spread(f(0.013f32))
        .scaled(f(-0.3f32), f(1.6f32)))
    .clamped(0.0, 1.0);
    let lerp_mapped = Noise2D::new(move |x, z| {
        let input = base.noise(x, z);
        lerp(lerp_noise.noise(x, z), input, sharp_hills_map(input))
    });
    let variance = simplex(seed + 67981832123)
        .octaves(3)
        .spread(f(0.06f32))
        .scaled(f(-0.2f32), f(0.2f32));
    lerp_mapped.add(n(variance)).scaled4(
        f(-0.75f32),
        f(0.7f32),
        sea(0) - f64::from(min_height),
        sea(0) + f64::from(max_height),
    )
}

pub fn glacial_surface_texture(seed: i64) -> Noise2D {
    let warp = n(simplex(seed + 413).spread(0.02).scaled(-12.0, 12.0));
    Noise2D::new(move |x, z| {
        let y_of_x = triangle(25.0, 18.0, 0.035, x + warp.noise(x, z)).min(0.0);
        let y_of_z = triangle(40.0, 30.0, 0.025, z + warp.noise(z, x)).min(0.0);
        y_of_x.min(y_of_z)
    })
}

pub fn glacial_base(seed: i64) -> Noise2D {
    knob_and_kettle(seed).add_constant(1.5)
}

pub fn glacial_oceanic_base(_seed: i64) -> Noise2D {
    Noise2D::constant(sea(-4))
}

pub fn ice_sheet_surface_height(seed: i64) -> Noise2D {
    hills(seed, 23, 38)
}

pub fn montane_ice_sheet_surface_height(seed: i64) -> Noise2D {
    hills(seed, 40, 48)
}

pub fn oceanic_ice_sheet_surface_height(seed: i64) -> Noise2D {
    hills(seed, 18, 26)
}

pub fn glacial_cirques_ice_surface_height(seed: i64) -> Noise2D {
    connected_valley_noise(seed)
        .map(|y| {
            if y < 0.38 {
                -100.0
            } else if y < 0.43 {
                mth_map(y, 0.38, 0.43, -50.0, 0.0)
            } else {
                mth_map(y, 0.43, 1.0, 0.0, 32.0)
            }
        })
        .add(hills(seed, 15, 23).add(glacial_cirques_cliffs_scale(seed)))
}

pub fn glacial_cirques_cliffs_start_height(seed: i64) -> Noise2D {
    connected_valley_noise(seed)
        .map(|y| {
            if y < 0.43 {
                mth_map(y, 0.0, 0.43, 32.0, 0.0)
            } else {
                mth_map(y, 0.43, 1.0, 0.0, 32.0)
            }
        })
        .add(hills(seed, 18, 26))
}

pub fn glacial_cirques_cliffs_scale(seed: i64) -> Noise2D {
    n(simplex(seed + 78267).spread(0.015))
        .add(glacial_valley_shape_noise(seed))
        .scaled(-10.0, 8.0)
        .clamped(0.0, 7.0)
}

pub fn glacial_valley_shape_noise(seed: i64) -> Noise2D {
    connected_valley_base_noise(seed)
        .map(|y| (6.0 * y * y).min(0.75 + 0.25 * y))
        .add(n(simplex(seed + 5287)
            .octaves(4)
            .spread(0.06)
            .scaled(-0.2, 0.2)))
}

pub fn glacial_cirques(seed: i64) -> Noise2D {
    let shape = glacial_valley_shape_noise(seed);
    let shape_map = connected_valley_noise(seed);
    let cells = Cellular2D::with_sample(seed, 2).spread(0.010);
    let warp = n(simplex(seed).spread(0.02))
        .add(shape_map.clone())
        .scaled4(-1.0, 2.0, -0.25, 0.2);
    let rough_peaks = n(simplex(seed).octaves(3).spread(0.08).scaled(0.6, 1.6));
    let cliff_scale = n(simplex(seed + 785267).spread(0.01).scaled(-12.0, 15.0)).clamped(0.0, 10.0);
    let cliff_start_height = oceanic_ice_sheet_surface_height(seed).add_constant(-8.0);
    let shape_map_c = shape_map.clone();
    let cirques = Noise2D::new(move |x, z| {
        let cell: Cell = cells.cell(x, z);
        let (f1, f2) = (cell.f1, cell.f2);
        let f2f1 = if f1 > 0.0 { f2 - f1 } else { 1.0 };
        let shape_at_center = shape_map_c.noise(cell.x, cell.y);
        if shape_at_center > 0.60 {
            let mut y = f2f1 + warp.noise(x, z);
            let rough = rough_peaks.noise(x, z);
            let scale = lerp(2.0 * y, 1.0, rough).min(rough);
            y = 1.0 + scale * y;
            y
        } else {
            let mut y = 1.0 - (f2f1 - warp.noise(x, z));
            y = 0.5 * (1.0 + y * y);
            let shape_at_point = shape_map_c.noise(x, z);
            let valley_closeness = (shape_at_point - shape_at_center).min(0.0);
            y + clamped_map(f2f1, 0.0, 0.1, 0.0, valley_closeness)
        }
    });
    cirques
        .scaled4(0.0, 1.0, 12.0, 64.0)
        .lazy_product(shape)
        .add_constant(sea(-15))
        .cliff_map(cliff_start_height, cliff_scale)
        .cliff_map(
            glacial_cirques_cliffs_start_height(seed),
            glacial_cirques_cliffs_scale(seed),
        )
}

pub fn patterned_ground(seed: i64) -> Noise2D {
    let cells = Cellular2D::with(seed, 0.25f32, 1).spread(0.05);
    Noise2D::new(move |x, z| {
        let cell = cells.cell(x, z);
        if cell.f2 - cell.f1 < f(0.12f32) {
            -1.0
        } else {
            0.0
        }
    })
}

pub fn inverted_patterned_ground(seed: i64) -> Noise2D {
    let base = hills(seed, -4, 3);
    let cells = Cellular2D::with(seed, 0.25f32, 1).spread(0.05);
    Noise2D::new(move |x, z| {
        let height = base.noise(x, z);
        let cell = cells.cell(x, z);
        let f2f1 = cell.f2 - cell.f1;
        if height >= SEA {
            height + if f2f1 < 0.12 { 1.0 } else { 0.0 }
        } else if f2f1 < 0.12 {
            SEA - 1.0
        } else if f2f1 < 0.22 {
            SEA - 2.0
        } else {
            SEA - 3.0
        }
    })
}

pub fn sea_ice_noise(seed: i64) -> Noise2D {
    let cells = Cellular2D::with(seed, 0.21f32, 1).spread(0.03);
    let wiggle = n(simplex(seed).scaled(-0.04, 0.04).spread(0.12));
    Noise2D::new(move |x, z| {
        let cell = cells.cell(x, z);
        cell.f2 - cell.f1 + wiggle.noise(x, z)
    })
}

pub fn stone_circles(seed: i64) -> Noise2D {
    let cells = Cellular2D::with(seed, 0.26f32, 1).spread(0.09);
    Noise2D::new(move |x, z| {
        let f1 = cells.cell(x, z).f1;
        if f1 > f(0.06f32) && f1 < f(0.13f32) {
            1.0
        } else {
            0.0
        }
    })
}

pub fn knob_and_kettle(seed: i64) -> Noise2D {
    n(simplex(seed).octaves(2).spread(f(0.03f32)))
        .map(|y| {
            if y > 0.3 {
                y - 0.3
            } else if y < -0.3 {
                y + 0.3
            } else {
                0.0
            }
        })
        .scaled(-12.0, 10.0)
        .add(hills(seed, -3, 3))
}

pub fn drumlins(seed: i64) -> Noise2D {
    n(simplex(seed)
        .octaves(3)
        .spread(f(0.04f32))
        .scaled(sea(-16), sea(32)))
    .stretch_z(2.5)
}

pub fn burren_crevices(seed: i64) -> Noise2D {
    n(simplex(seed + 398767567).octaves(2).spread(f(0.08f32))).abs()
}

pub fn burren(seed: i64, base: Noise2D, scale: f64) -> Noise2D {
    let min_height = sea(2);
    let crevices = burren_crevices(seed).map(move |y| {
        if y < f(0.15f32) {
            -scale
        } else if y < 0.4 {
            (y - 0.4) * scale
        } else {
            0.0
        }
    });
    crevices.add(base).map(move |y| y.max(min_height))
}

pub fn shilin_ridges(seed: i64) -> Noise2D {
    let width_top = 0.1;
    let width_bot = 0.2;
    let ridges = n(simplex(seed + 398767567).octaves(2).spread(f(0.06f32))).map(move |y| {
        let y = y.abs();
        if y < width_top {
            1.0
        } else if y < width_bot {
            1.0 + (0.67 * (y - width_top) / (width_top - width_bot))
        } else {
            0.0
        }
    });
    let cuts = n(simplex(seed + 45764379).octaves(2).spread(f(0.03f32))).map(move |y| {
        let y = y.abs();
        let y = if y < width_top * 0.65 {
            1.0
        } else if y < width_bot * 1.2 {
            1.0 + ((y - width_top * 0.65) / (width_top * 0.65 - width_bot * 1.2))
        } else {
            0.0
        };
        1.0 - y
    });
    ridges.lazy_product(cuts)
}

pub fn shilin(seed: i64, base: Noise2D, scale: f64) -> Noise2D {
    let min_height = sea(2);
    let ridges = shilin_ridges(seed);
    let bumps = n(simplex(seed + 83436545633).spread(0.16).scaled(0.6, 1.0));
    ridges
        .lazy_product(bumps)
        .scaled(SEA, SEA + scale)
        .max(base)
        .map(move |y| y.max(min_height))
}

pub fn fengcong_plains(seed: i64, base_height: f64, scale: f64) -> Noise2D {
    let layer0 = n(simplex(seed).spread(0.06).octaves(4))
        .abs()
        .scaled4(0.25, 1.0, 0.0, scale);
    layer0
        .max(Noise2D::constant(0.0))
        .add_constant(base_height + SEA)
}

pub fn fenglin_plains(seed: i64, base_height: f64, scale: f64) -> Noise2D {
    let cliff_compare1 = n(simplex(seed)
        .spread(0.04)
        .octaves(3)
        .scaled(0.0, 0.2 * scale))
    .add_constant(base_height + SEA);
    let cliff_compare2 = n(simplex(seed)
        .spread(0.04)
        .octaves(3)
        .scaled(0.15 * scale, 0.3 * scale))
    .add_constant(base_height + SEA);
    let cliff_height = n(simplex(seed)
        .spread(0.08)
        .octaves(3)
        .scaled(0.0, 0.1 * scale));
    fengcong_plains(seed, base_height, 0.9 * scale)
        .cliff_map(cliff_compare2, cliff_height.clone())
        .cliff_map(cliff_compare1, cliff_height)
}

pub fn mogotes(seed: i64, base_height: f64, scale: f64) -> Noise2D {
    let layer0 = n(simplex(seed).spread(0.03).octaves(4))
        .abs()
        .scaled4(0.09, 1.0, 0.0, scale);
    layer0
        .max(Noise2D::constant(0.0))
        .add_constant(base_height + SEA)
}

pub fn mogote_plateau(seed: i64) -> Noise2D {
    let layer0 = n(simplex(seed).spread(0.04).octaves(3))
        .abs()
        .scaled4(0.05, 1.0, 0.0, 30.0);
    layer0.add_constant(f64::from(21 + SEA_LEVEL_Y))
}

pub fn fengcong(seed: i64, base: Noise2D) -> Noise2D {
    let scale = 37.0;
    let cones = n(simplex(seed).octaves(3).spread(0.06)).map(move |y| {
        let y = -0.5 * jmath::cos(std::f64::consts::PI * y.abs()) + 0.5;
        let y = (y.max(0.25) - 0.25) / 0.75;
        scale * y
    });
    base.add(cones)
}

pub fn fenglin_cliff_map(base: Noise2D, compare: Noise2D, addend: Noise2D) -> Noise2D {
    Noise2D::new(move |x, z| {
        let b = base.noise(x, z);
        if b > compare.noise(x, z) {
            let a = addend.noise(x, z);
            b * (1.0 - a) + a
        } else {
            b
        }
    })
}

pub fn fenglin(seed: i64, base: Noise2D, scale: f64) -> Noise2D {
    let cliff_scale = n(simplex(seed + 78535267).spread(0.06).scaled(0.0, 0.25));
    let cliff_start = n(simplex(seed + 390798).spread(0.06).scaled(0.0, 0.7));
    let cliff_base = n(simplex(seed).octaves(2).spread(0.05)).map(|y| {
        let y = y.abs() - 0.45;
        if y > 0.0 { (y / 0.55).sqrt() } else { 0.0 }
    });
    let towers = fenglin_cliff_map(cliff_base, cliff_start, cliff_scale).map(move |y| scale * y);
    base.add(towers)
}

pub fn bowl_dolines(seed: i64, base: Noise2D, scale: f64) -> Noise2D {
    let bowls = n(simplex(seed).octaves(3).spread(0.72 / scale)).map(move |x| {
        let x = -0.5 * jmath::cos(std::f64::consts::PI * x) + 0.5;
        let x = x.max(0.1) - 0.1;
        -scale * x
    });
    base.add(bowls)
}

pub fn tiankeng(seed: i64, base: Noise2D) -> Noise2D {
    let cliff_scale = n(simplex(seed + 78535267).spread(0.04).scaled(0.0, 0.04));
    let cliff_start = n(simplex(seed + 390798).spread(0.04).scaled(0.0, 0.7));
    let wide = n(simplex(seed).octaves(2).spread(0.02)).map(|y| {
        let y = y.abs() - 0.3;
        if y > 0.0 { (y / 0.7).sqrt() } else { 0.0 }
    });
    let deep = n(simplex(seed).octaves(2).spread(0.02)).map(|y| {
        let y = y.abs() - 0.65;
        if y > 0.0 { (y / 0.35).sqrt() } else { 0.0 }
    });
    Noise2D::new(move |x, z| {
        let compare = cliff_start.noise(x, z);
        let addend = cliff_scale.noise(x, z);
        let wide_base = wide.noise(x, z);
        let deep_base = deep.noise(x, z);
        base.noise(x, z)
            + -22.0
                * (if wide_base > compare {
                    wide_base * (1.0 - addend) + addend
                } else {
                    wide_base
                })
            + -24.0
                * (if deep_base > compare {
                    deep_base * (1.0 - addend) + addend
                } else {
                    deep_base
                })
    })
}

pub fn lake(seed: i64) -> Noise2D {
    n(simplex(seed)
        .octaves(4)
        .spread(f(0.15f32))
        .scaled(sea(-12), sea(2)))
    .add(
        n(simplex(seed + 1).octaves(5).spread(f(0.1f32)))
            .map(|v| v * v * v * v)
            .scaled(-2.0, 2.0)
            .clamped(0.0, 2.0),
    )
}

pub fn lowlands(seed: i64) -> Noise2D {
    hills(seed, -3, -2).add(
        n(simplex(seed + 1)
            .octaves(6)
            .spread(f(0.55f32))
            .scaled(-2.0, 2.0))
        .clamped(-2.0, 1.0),
    )
}

pub fn flats(seed: i64) -> Noise2D {
    n(simplex(seed)
        .octaves(4)
        .spread(f(0.03f32))
        .scaled(sea(-12), sea(8)))
    .clamped(SEA, sea(2))
}

pub fn salt_flats(seed: i64) -> Noise2D {
    n(simplex(seed)
        .octaves(4)
        .spread(f(0.05f32))
        .scaled(sea(-16), sea(10)))
    .clamped(sea(-2), SEA)
}

pub fn dunes(seed: i64, min_height: i32, max_height: i32) -> Noise2D {
    n(simplex(seed).spread(0.02).scaled(-3.0, 3.0))
        .add(Noise2D::new(|x, z| x / 6.0 + 20.0 * jmath::sin(z / 240.0)))
        .map(|value| {
            1.3 * ((((value % 5.0) - 1.0).abs())
                * (if (value % 5.0) - (value % 1.0) > 0.0 {
                    0.5
                } else {
                    2.0
                })
                - 1.0)
        })
        .clamped(-1.0, 1.0)
        .lazy_product(n(simplex(seed).octaves(4).spread(0.1).scaled(-1.0, 2.0)).clamped(0.4, 1.0))
        .scaled(sea(min_height), sea(max_height))
}

pub fn canyon_base_noise(
    seed: i64,
    min_height: i32,
    max_height: i32,
    valley_width: f64,
) -> Noise2D {
    let valley_edge = valley_width + 0.28;
    n(simplex(seed + 1).octaves(4).spread(f(0.03f32)))
        .abs()
        .clamped_scaled4(
            valley_width,
            valley_edge,
            f64::from(min_height),
            f64::from(max_height),
        )
}

pub fn stair_step_cliffs(seed: i64, input: Noise2D) -> Noise2D {
    stair_step_cliffs_with(seed, input, 5, 12, 7)
}

pub fn stair_step_cliffs_with(
    seed: i64,
    input: Noise2D,
    min_cliff_start: i32,
    max_cliff_start: i32,
    cliff_height: i32,
) -> Noise2D {
    let ch = f64::from(cliff_height);
    let start1 = n(simplex(seed + 3)
        .octaves(2)
        .spread(f(0.008f32))
        .scaled(sea(min_cliff_start), sea(max_cliff_start)));
    let cliff1 = n(simplex(seed + 7).spread(f(0.003f32)).scaled(-ch, 2.0 * ch)).clamped(0.0, ch);
    let double = cliff1.clone().add(cliff1.clone());
    let start2 = start1.clone().add(double.clone());
    let cliff2 = n(simplex(seed + 19).spread(f(0.003f32)).scaled(-ch, 2.0 * ch)).clamped(0.0, ch);
    let start3 = start2.clone().add(double);
    let cliff3 = n(simplex(seed + 25).spread(f(0.003f32)).scaled(-ch, 2.0 * ch)).clamped(0.0, ch);
    let slope = n(simplex(seed + 33).spread(0.008).scaled(-2.0, 2.0));
    let slope_of = |c: &Noise2D| c.clone().scaled4(0.0, 7.0, 3.0, 6.0).add(slope.clone());
    input
        .sloped_cliff_map(start1, cliff1.clone(), slope_of(&cliff1))
        .sloped_cliff_map(start2, cliff2, slope_of(&cliff1))
        .sloped_cliff_map(start3, cliff3, slope_of(&cliff1))
}

fn bumpy(seed: i64, amount: f64) -> Noise2D {
    n(simplex(seed)
        .octaves(3)
        .spread(f(0.08f32))
        .scaled(-amount, amount))
}

pub fn stair_canyons(seed: i64) -> Noise2D {
    stair_step_cliffs(
        seed,
        canyon_base_noise(seed, SEA_LEVEL_Y + 4, SEA_LEVEL_Y + 22, 0.05),
    )
    .add(bumpy(seed, 5.0))
}

pub fn mesas(seed: i64) -> Noise2D {
    stair_step_cliffs(
        seed,
        canyon_base_noise(seed, SEA_LEVEL_Y + 4, SEA_LEVEL_Y + 22, 0.12),
    )
    .add(bumpy(seed, 5.0))
}

pub fn buttes(seed: i64) -> Noise2D {
    stair_step_cliffs(
        seed,
        canyon_base_noise(seed, SEA_LEVEL_Y + 4, SEA_LEVEL_Y + 22, 0.18),
    )
    .add(bumpy(seed, 5.0))
}

pub fn hoodoos(seed: i64) -> Noise2D {
    let (min_height, max_height) = (SEA_LEVEL_Y + 4, SEA_LEVEL_Y + 22);
    let max_base = canyon_base_noise(seed, min_height, max_height, 0.03);
    let min_base = canyon_base_noise(seed, min_height, max_height, 0.20);
    let hoodoo = n(simplex(seed + 1).octaves(3).spread(f(0.12f32)))
        .abs()
        .clamped_scaled4(0.20, 0.60, f64::from(min_height), f64::from(max_height));
    stair_step_cliffs_with(seed, max_base.min(hoodoo).max(min_base), 5, 8, 7).add(bumpy(seed, 3.0))
}

pub fn table_mountains(seed: i64) -> Noise2D {
    stair_step_cliffs_with(
        seed,
        canyon_base_noise(seed, SEA_LEVEL_Y + 16, SEA_LEVEL_Y + 48, 0.1),
        20,
        30,
        10,
    )
}

fn cliffed(
    base: Noise2D,
    cliff_noise: Noise2D,
    cliff_height_noise: Noise2D,
    threshold: f64,
) -> Noise2D {
    Noise2D::new(move |x, z| {
        let mut height = base.noise(x, z);
        if height > threshold {
            let cliff_height = cliff_height_noise.noise(x, z) - height;
            if cliff_height < 0.0 {
                let mapped = clamped_map(cliff_height, 0.0, -1.0, 0.0, 1.0);
                height += mapped * cliff_noise.noise(x, z);
            }
        }
        height
    })
}

pub fn mountains(seed: i64, base_height: i32, scale_height: i32, spread_factor: f32) -> Noise2D {
    let base = n(simplex(seed).octaves(6).spread(f(0.14f32 * spread_factor)))
        .add(
            n(simplex(seed + 1)
                .octaves(4)
                .spread(f(0.02f32 * spread_factor))
                .scaled(f(-0.9f32), f(0.9f32)))
            .ridged(),
        )
        .map(move |x| {
            let x0 = x + 1.0;
            let x1 = f(0.125f32) * x0 * x0 * x0;
            f64::from(SEA_LEVEL_Y + base_height) + f64::from(scale_height) * x1
        });
    let cliff = n(simplex(seed + 2)
        .octaves(2)
        .spread(f(0.01f32 * spread_factor))
        .scaled(-25.0, 25.0))
    .map(|x| if x > 0.0 { x } else { 0.0 });
    let cliff_height = n(simplex(seed + 3)
        .octaves(2)
        .spread(f(0.01f32 * spread_factor))
        .scaled(f64::from(140 - 20), f64::from(140 + 20)));
    cliffed(base, cliff, cliff_height, 120.0)
}

pub fn ridge_mountains(
    seed: i64,
    base_height: f64,
    scale_height: f64,
    spread_factor: f32,
    cliff_start_height: i32,
    cliff_start_variance: i32,
) -> Noise2D {
    let ridges =
        n(simplex(seed + 3987677).octaves(4).spread(f(0.022f32))).map(|y| 1.0 - 2.8 * y * y);
    let passes = n(simplex(seed + 454379).octaves(2).spread(f(0.003f32))).map(|y| 16.0 * y * y);
    let pass_height = ridges.clone().map(|y| clamped_map(y, 0.3, 0.9, 0.3, 0.5));
    let carved = ridges.clone().min(passes.add(pass_height));
    let warp = simplex(seed)
        .octaves(3)
        .spread(f(0.025f32))
        .scaled(f(-50f32), f(50f32));
    let peaks = n(simplex(seed + 4242)
        .octaves(3)
        .spread(0.045)
        .scaled(-0.6, 1.0))
    .warped(warp)
    .ease_in(0.4, 0.8, 0.1, 1.0, carved.clone());
    let scale = n(simplex(seed + 245).octaves(4).spread(0.012))
        .ease_in(0.3, 0.9, 0.0, 1.0, ridges)
        .map(|y| 1.0 + 0.35 * y);
    let flat_valleys = hills(
        seed + 525,
        (base_height - 15.0) as i32,
        (base_height + 15.0) as i32,
    );
    let texture = n(simplex(seed + 5).octaves(6).spread(0.4).scaled(-30.0, 30.0));
    let base = carved
        .add(peaks)
        .lazy_product(scale)
        .scaled4(
            0.0,
            1.0,
            SEA + base_height,
            SEA + base_height + scale_height,
        )
        .max(flat_valleys)
        .add(texture)
        .spread(f(spread_factor));
    let cliff = n(simplex(seed + 2)
        .octaves(2)
        .spread(f(0.01f32 * spread_factor))
        .scaled(-25.0, 25.0))
    .map(|x| if x > 0.0 { x } else { 0.0 });
    let cliff_height = n(simplex(seed + 3)
        .octaves(2)
        .spread(f(0.01f32 * spread_factor))
        .scaled(
            f64::from(cliff_start_height - cliff_start_variance),
            f64::from(cliff_start_height + cliff_start_variance),
        ));
    let cliffed_noise = cliffed(
        base,
        cliff,
        cliff_height,
        f64::from(cliff_start_height - cliff_start_variance),
    );
    Noise2D::new(move |x, z| {
        let height = cliffed_noise.noise(x, z);
        if height > 260.0 {
            clamped_map(height, 260.0, 340.0, 260.0, 300.0)
        } else {
            height
        }
    })
}

pub fn rocky_islands(seed: i64) -> Noise2D {
    let base = n(simplex(seed).octaves(4).spread(f(0.14f32))).map(|x| {
        let x0 = f(0.125f32) * (x + 1.0) * (x + 1.0) * (x + 1.0);
        SEA - 15.0 + 50.0 * x0
    });
    let cliff = n(simplex(seed + 2)
        .octaves(2)
        .spread(f(0.01f32))
        .scaled(-10.0, 18.0))
    .map(|x| if x > 0.0 { x } else { 0.0 });
    let cliff_height = n(simplex(seed + 3)
        .octaves(2)
        .spread(f(0.01f32))
        .scaled(sea(-5), sea(5)));
    cliffed(base, cliff, cliff_height, SEA - 10.0)
}

pub fn ocean(seed: i64, depth_min: i32, depth_max: i32) -> Noise2D {
    let warp = simplex(seed)
        .octaves(2)
        .spread(f(0.015f32))
        .scaled(-30.0, 30.0);
    n(simplex(seed + 1)
        .octaves(4)
        .spread(f(0.11f32))
        .scaled(sea(depth_min), sea(depth_max)))
    .warped(warp)
}

fn continent_cell_noise(scale_factor: f32, seed: i64) -> Cellular2D {
    Cellular2D::with_sample(seed, 2).spread(f(scale_factor / CELL_WIDTH_IN_GRID as f32))
}

/// `(warped edge distance, distance to the fault)` of `getOceanRidgeWarpedEdgeDistanceAndScale`.
struct RidgeSampler {
    cells: Cellular2D,
    faulting: Noise2D,
    warp: OpenSimplex2D,
    big_warp: Noise2D,
}

impl RidgeSampler {
    fn new(seed: i64) -> Self {
        let faulting = n(simplex(seed)
            .octaves(2)
            .spread(f(0.0018f32))
            .scaled(-4.5, 4.5));
        Self {
            cells: continent_cell_noise(1.0f32 / 128f32, seed),
            big_warp: faulting.clone().map(|w| f64::from(30 * java_round(w))),
            faulting,
            warp: simplex(seed)
                .octaves(2)
                .spread(f(0.05f32))
                .scaled(0.0, 20.0),
        }
    }
    fn sample(&self, x: f64, y: f64) -> (f64, f64) {
        let cell = self.cells.cell(x, y);
        let x_trench = 0.5 * (cell.x + cell.nx);
        let y_trench = 0.5 * (cell.y + cell.ny);
        let sample_dx = x - x_trench;
        let sample_dy = y - y_trench;
        let parallel_dx = y_trench - cell.y;
        let parallel_dy = cell.x - x_trench;
        let dot = (sample_dx * parallel_dx + sample_dy * parallel_dy)
            / (parallel_dx * parallel_dx + parallel_dy * parallel_dy);
        let x_projected = x_trench + dot * parallel_dx;
        let y_projected = y_trench + dot * parallel_dy;
        let edge_dist =
            ((x - x_projected) * (x - x_projected) + (y - y_projected) * (y - y_projected)).sqrt();
        let small_warp = if cell.nx > cell.x {
            -self.warp.noise(x, y)
        } else {
            self.warp.noise(x, y)
        };
        let big_warp = if cell.nx > cell.x {
            -self.big_warp.noise(x_projected, y_projected)
        } else {
            self.big_warp.noise(x_projected, y_projected)
        };
        let dist_to_fault =
            (positive_modulo(self.faulting.noise(x_projected, y_projected), 1.0) - 0.5).abs();
        ((edge_dist + small_warp + big_warp).abs(), dist_to_fault)
    }
}

/// The warped distance to the ocean ridge's edge (`getOceanRidgeWarpedEdgeDistanceAndScale(...).x`).
pub fn ocean_ridge_edge_distance(seed: i64) -> Noise2D {
    let sampler = RidgeSampler::new(seed);
    Noise2D::new(move |x, z| sampler.sample(x, z).0)
}

pub fn ocean_ridge(seed: i64) -> Noise2D {
    let abyssal = ocean(seed, -46, -30);
    let sampler = RidgeSampler::new(seed);
    Noise2D::new(move |x, z| {
        let (warped_edge_dist, fault) = sampler.sample(x, z);
        let scale = clamped_map(fault, 0.0, f(0.05f32), 0.0, 1.0);
        let abyssal_elev = abyssal.noise(x, z);
        let raw_ridge = if warped_edge_dist < 27.0 {
            mth_map(warped_edge_dist, 0.0, 27.0, sea(-36), sea(-12))
        } else {
            clamped_map(warped_edge_dist, 27.0, 120.0, sea(-12), sea(-50))
        };
        if raw_ridge <= abyssal_elev {
            return abyssal_elev;
        }
        mth_map(scale, 0.0, 1.0, abyssal_elev, raw_ridge)
    })
}

pub fn ocean_trench(seed: i64, depth_min: i32, depth_max: i32) -> Noise2D {
    let warp = simplex(seed)
        .octaves(2)
        .spread(f(0.015f32))
        .scaled(-30.0, 30.0);
    let ridge = n(simplex(seed + 1).octaves(4).spread(f(0.015f32)))
        .ridged()
        .map(|x| {
            if x > f(-0.3f32) {
                let x = (x + f(0.3f32)) / f(1.3f32);
                let x = x * x * x;
                f(-16f32) * x
            } else {
                0.0
            }
        });
    n(simplex(seed + 2)
        .octaves(4)
        .spread(f(0.11f32))
        .scaled(sea(depth_min), sea(depth_max)))
    .add(ridge)
    .warped(warp)
}

pub fn shore(seed: i64) -> Noise2D {
    n(simplex(seed)
        .octaves(4)
        .spread(f(0.17f32))
        .scaled(SEA, SEA + f(5f32)))
}

pub fn tidal_flats(seed: i64) -> Noise2D {
    n(simplex(seed)
        .octaves(4)
        .spread(f(0.17f32))
        .scaled(SEA, SEA + f(1.8f32)))
}

pub fn rift_valley(seed: i64, min_height_in: i32, edge_height_in: i32, is_lake: bool) -> Noise2D {
    let min_height = f64::from(SEA_LEVEL_Y + min_height_in);
    let edge_height = f64::from(SEA_LEVEL_Y + edge_height_in);
    let cells = continent_cell_noise(1.0f32 / 128f32, seed);
    let width_noise = simplex(seed + 8424)
        .octaves(2)
        .spread(f(0.005f32))
        .scaled(1.0, 1.4);
    let texture_warp = simplex(seed + 2456)
        .octaves(2)
        .spread(f(0.05f32))
        .scaled(-10.0, 10.0);
    let wiggle = simplex(seed + 94312)
        .octaves(3)
        .spread(f(0.006f32))
        .scaled(-130.0, 130.0);
    let roughness = n(simplex(seed)
        .octaves(3)
        .spread(f(0.04f32))
        .scaled(-8.0, 8.0));
    let range_scale_noise = n(simplex(seed + 48993)
        .octaves(2)
        .spread(0.006)
        .scaled(-2.0, 3.0))
    .clamped(0.0, 1.0);
    let valley_noise = n(simplex(seed + 3245).octaves(2))
        .ridged()
        .scaled4(-1.0, 1.0, edge_height + 60.0, edge_height + 3.0)
        .spread(0.01);
    roughness.add(Noise2D::new(move |x, y| {
        let cell = cells.cell(x, y);
        let x_centroid = 0.5 * (cell.x + cell.nx);
        let y_centroid = 0.5 * (cell.y + cell.ny);
        let sample_dx = x - x_centroid;
        let sample_dy = y - y_centroid;
        let parallel_dx = y_centroid - cell.y;
        let parallel_dy = cell.x - x_centroid;
        let dot = (sample_dx * parallel_dx + sample_dy * parallel_dy)
            / (parallel_dx * parallel_dx + parallel_dy * parallel_dy);
        let x_projected = x_centroid + dot * parallel_dx;
        let y_projected = y_centroid + dot * parallel_dy;
        let edge_dist =
            ((x - x_projected) * (x - x_projected) + (y - y_projected) * (y - y_projected)).sqrt();
        let width_warp = width_noise.noise(x_projected, y_projected);
        let wiggle_warp = if cell.nx > cell.x {
            -wiggle.noise(x_projected, y_projected)
        } else {
            wiggle.noise(x_projected, y_projected)
        };
        let texture = texture_warp.noise(x, y);
        let edge_warped = (edge_dist * width_warp + wiggle_warp + texture).abs();
        let mut profile;
        if edge_warped < 80.0 {
            if is_lake {
                profile = clamped_map(edge_warped, 0.0, 80.0, min_height - 15.0, min_height);
            } else {
                let range_scale = range_scale_noise.noise(x, y);
                if range_scale > 0.0 {
                    let range_height = if edge_warped < 48.0 {
                        clamped_map(edge_warped, 28.0, 48.0, min_height, min_height + 20.0)
                    } else {
                        clamped_map(edge_warped, 48.0, 80.0, min_height + 20.0, min_height)
                    };
                    profile = min_height.max(range_height * range_scale);
                } else {
                    profile = min_height;
                }
            }
        } else {
            if edge_warped < 192.0 {
                if hash_double(cell.noise, 6353) < 0.6 {
                    if edge_warped < 128.0 {
                        profile =
                            clamped_map(edge_warped, 96.0, 128.0, min_height, edge_height + 2.0);
                    } else if edge_warped < 160.0 {
                        profile = clamped_map(
                            edge_warped,
                            128.0,
                            160.0,
                            edge_height + 2.0,
                            edge_height - 2.0,
                        );
                    } else {
                        profile = clamped_map(
                            edge_warped,
                            160.0,
                            192.0,
                            edge_height - 2.0,
                            edge_height + 32.0,
                        );
                    }
                } else {
                    profile =
                        clamped_map(edge_warped, 128.0, 192.0, min_height, edge_height + 32.0);
                }
            } else {
                profile = clamped_map(edge_warped, 192.0, 280.0, edge_height + 32.0, edge_height)
                    .max(edge_height);
            }
            profile = profile.min(valley_noise.noise(x, y));
        }
        profile
    }))
}

fn volcano_surface(seed_offset: i64, seed: i64, spread: f32, lo: f64, hi: f64) -> Noise2D {
    let warp = simplex(seed + seed_offset)
        .octaves(4)
        .spread(f(0.03f32))
        .scaled(f(-100f32), f(100f32));
    n(simplex(seed + seed_offset + 1).octaves(4).spread(f(spread)))
        .warped(warp)
        .map(|x| if x > 0.4 { x - f(0.8f32) } else { -x })
        .scaled4(f(-0.4f32), f(0.8f32), lo, hi)
}

pub fn lava_flow(seed: i64) -> Noise2D {
    n(simplex(seed + 23891)).ridged().spread(0.01)
}

pub fn active_shield_volcano(seed: i64, hotspot: Noise2D) -> Noise2D {
    let edge = sea(1);
    let caldera_edge = sea(115);
    let cliff_edge = sea(90);
    let caldera_center = sea(60);
    let volcano = hotspot.map(move |y| {
        if y < 0.75 {
            mth_map(y, 0.0, 0.75, edge, caldera_edge)
        } else if y < 0.78 {
            mth_map(y, 0.75, 0.78, caldera_edge, cliff_edge)
        } else {
            mth_map(y, 0.78, 1.0, cliff_edge, caldera_center)
        }
    });
    let flows = lava_flow(seed).map(|y| if y < 0.45 { 0.0 } else { 1.0 });
    let warp = simplex(seed)
        .octaves(4)
        .spread(f(0.03f32))
        .scaled(f(-100f32), f(100f32));
    let surface = n(simplex(seed + 1).octaves(4).spread(f(0.06f32)))
        .warped(warp)
        .map(|x| if x > 0.4 { x - f(0.8f32) } else { -x })
        .scaled4(f(-0.4f32), f(0.8f32), -8.0, 8.0);
    volcano.add(flows).add(surface)
}

pub fn dormant_shield_volcano(seed: i64, hotspot: Noise2D) -> Noise2D {
    let (sea_e, base, rim, cliff, center) = (sea(9), sea(40), sea(70), sea(50), sea(15));
    let volcano = hotspot.map(move |y| {
        if y < 0.45 {
            mth_map(y, 0.0, 0.45, sea_e, base)
        } else if y < 0.7 {
            mth_map(y, 0.45, 0.7, base, rim)
        } else if y < 0.73 {
            mth_map(y, 0.7, 0.73, rim, cliff)
        } else if y < 0.85 {
            mth_map(y, 0.73, 0.85, cliff, center)
        } else {
            center
        }
    });
    volcano.add(volcano_surface(43, seed, 0.06f32, -12.0, 12.0))
}

pub fn extinct_shield_volcano(seed: i64, hotspot: Noise2D) -> Noise2D {
    let (sea_e, base, rim, cliff, center) = (sea(6), sea(25), sea(55), sea(25), sea(-10));
    let volcano = hotspot.map(move |y| {
        if y < 0.4 {
            mth_map(y, 0.0, 0.4, sea_e, base)
        } else if y < 0.6 {
            mth_map(y, 0.4, 0.6, base, rim)
        } else if y < 0.62 {
            mth_map(y, 0.6, 0.62, rim, cliff)
        } else if y < 0.75 {
            mth_map(y, 0.62, 0.75, cliff, center)
        } else {
            center
        }
    });
    volcano.add(volcano_surface(43, seed, 0.06f32, -9.0, 9.0))
}

pub fn glaciated_shield_volcano(seed: i64, hotspot: Noise2D) -> Noise2D {
    let (sea_e, base, rim, cliff, center) = (sea(15), sea(70), sea(100), sea(60), sea(50));
    let volcano = hotspot.map(move |y| {
        if y < 0.45 {
            mth_map(y, 0.0, 0.45, sea_e, base)
        } else if y < 0.72 {
            mth_map(y, 0.45, 0.72, base, rim)
        } else if y < 0.74 {
            mth_map(y, 0.72, 0.74, rim, cliff)
        } else if y < 0.85 {
            mth_map(y, 0.74, 0.85, cliff, center)
        } else {
            center
        }
    });
    volcano.add(volcano_surface(43, seed, 0.02f32, -48.0, 32.0))
}

pub fn shield_volcano_ice_sheet_surface(seed: i64, hotspot: Noise2D) -> Noise2D {
    hotspot
        .map(|y| {
            if y < 0.9 {
                mth_map(y, 0.0, 0.9, 0.0, 51.0)
            } else {
                51.0
            }
        })
        .add(ice_sheet_surface_height(seed))
}

pub fn shield_volcano_glacier_surface(seed: i64, hotspot: Noise2D) -> Noise2D {
    let base = n(simplex(seed)
        .octaves(3)
        .spread(f(0.05f32))
        .scaled(-5.0, 5.0));
    let (low, edge, rim, center) = (sea(-60), sea(75), sea(92), sea(98));
    hotspot
        .map(move |y| {
            if y < 0.40 {
                low
            } else if y < 0.58 {
                mth_map(y, 0.40, 0.58, low, edge)
            } else if y < 0.72 {
                mth_map(y, 0.58, 0.72, edge, rim)
            } else if y < 0.9 {
                mth_map(y, 0.72, 0.9, rim, center)
            } else {
                center
            }
        })
        .add(base)
}

pub fn sunken_shield_volcano(seed: i64, hotspot: Noise2D) -> Noise2D {
    let volcano = hotspot.map(|y| {
        if y < 0.25 {
            50.0
        } else if y < 0.45 {
            mth_map(y, 0.25, 0.45, 50.0, SEA)
        } else if y < 0.6 {
            mth_map(y, 0.45, 0.6, SEA, 95.0)
        } else if y < 0.62 {
            mth_map(y, 0.6, 0.62, 94.0, 80.0)
        } else if y < 0.75 {
            mth_map(y, 0.62, 0.75, 80.0, 52.0)
        } else {
            52.0
        }
    });
    let scale = n(simplex(seed + 789913)
        .octaves(2)
        .spread(f(0.008f32))
        .scaled(0.45, 1.0));
    volcano
        .lazy_product(scale)
        .add(volcano_surface(43, seed, 0.06f32, -6.0, 6.0))
}

pub fn ancient_shield_volcano(
    seed: i64,
    min_elev: f64,
    max_elev: f64,
    hotspot: Noise2D,
) -> Noise2D {
    let volcano = hotspot.map(|y| {
        if y < 0.15 {
            90.0
        } else if y < 0.6 {
            mth_map(y, 0.15, 0.6, 90.0, 130.0)
        } else if y < 0.63 {
            mth_map(y, 0.6, 0.63, 129.0, 108.0)
        } else if y < 0.7 {
            mth_map(y, 0.63, 0.7, 108.0, 90.0)
        } else {
            90.0
        }
    });
    let valleys = n(simplex(seed + 90183).spread(0.01))
        .ridged()
        .octaves(3)
        .scaled(max_elev * 2.2, min_elev);
    let scale = n(simplex(seed + 789913)
        .octaves(2)
        .spread(f(0.008f32))
        .scaled(0.6, 1.0));
    volcano
        .lazy_product(scale)
        .min(valleys)
        .add(volcano_surface(43, seed, 0.06f32, -20.0, 0.0))
}

pub fn lava_flow_material(seed: i64) -> Noise2D {
    n(simplex(seed).octaves(2).spread(0.25))
}

pub fn active_hot_spots(seed: i64) -> Noise2D {
    let (horizontal_scale, cutoff, rescale) = (0.003, 0.75, 7.2);
    n(simplex(seed))
        .map(move |y| {
            let y = if y > cutoff { y - cutoff } else { 0.0 };
            y * rescale
        })
        .octaves(3)
        .spread(horizontal_scale)
}

pub fn dormant_hot_spots(seed: i64) -> Noise2D {
    hot_spot_warp(active_hot_spots(seed), plate_regions(seed), 1024, 0.0)
        .map(|y| (y - 0.1).max(0.0) * 1.111)
}

pub fn extinct_hot_spots(seed: i64) -> Noise2D {
    hot_spot_warp(active_hot_spots(seed), plate_regions(seed), 2048, 0.25)
        .map(|y| (y - 0.2).max(0.0) * 1.25)
}

pub fn ancient_hot_spots(seed: i64) -> Noise2D {
    hot_spot_warp(active_hot_spots(seed), plate_regions(seed), 3072, 0.5)
        .map(|y| (y - 0.3).max(0.0) * 1.4286)
}

pub fn hot_spot_intensity(seed: i64) -> Noise2D {
    active_hot_spots(seed)
        .max(dormant_hot_spots(seed))
        .max(extinct_hot_spots(seed))
        .max(ancient_hot_spots(seed))
}

pub fn hot_spot_warp(
    to_warp: Noise2D,
    cells: Cellular2D,
    velocity_scale: i32,
    accel_scale: f64,
) -> Noise2D {
    Noise2D::new(move |x, z| {
        let ux = cells.noise(x, z);
        let uz = (if (ux * 16.0).abs() % 1.0 > 0.5 {
            1.0
        } else {
            -1.0
        } * (ux * 256.0))
            % 1.0;
        let sx = if ux > 0.0 { 1.0 } else { -1.0 };
        let sz = if uz > 0.0 { 1.0 } else { -1.0 };
        let vx = (ux + sx) * f64::from(velocity_scale);
        let vz = (uz + sz) * f64::from(velocity_scale);
        let ax = -vz * accel_scale;
        let az = vx * accel_scale;
        let cell = cells.cell(x, z);
        let scale = clamped_map(cell.f2 - cell.f1, 0.0, 0.002, 0.0, 1.0);
        to_warp.noise(x + vx + ax, z + vz + az) * scale
    })
}

pub fn hot_spot_age(seed: i64) -> Noise2D {
    map_ages(
        active_hot_spots(seed),
        dormant_hot_spots(seed),
        extinct_hot_spots(seed),
        ancient_hot_spots(seed),
    )
}

pub fn map_ages(
    active_noise: Noise2D,
    young_noise: Noise2D,
    old_noise: Noise2D,
    oldest_noise: Noise2D,
) -> Noise2D {
    Noise2D::new(move |x, z| {
        let active = active_noise.noise(x, z);
        let young = young_noise.noise(x, z);
        let old = old_noise.noise(x, z);
        let oldest = oldest_noise.noise(x, z);
        if active.max(young).max(old.max(oldest)) <= -0.8 {
            0.0
        } else if active > young && active > old && active > oldest {
            1.0
        } else if young > active && young > old && young > oldest {
            2.0
        } else if old > young && old > oldest && old > active {
            3.0
        } else if oldest > young && oldest > old && oldest > active {
            4.0
        } else {
            0.0
        }
    })
}

pub fn plate_regions(seed: i64) -> Cellular2D {
    Cellular2D::new(seed).spread(f(0.00590625f32 / CELL_WIDTH_IN_GRID as f32))
}

pub fn cliff_noise(level_seed: i64) -> OpenSimplex3D {
    OpenSimplex3D::new(level_seed).octaves(2).spread(f(0.1f32))
}

pub fn lower_terrace_noise(seed: i64) -> Noise2D {
    hills(seed, 3, 11)
}

pub fn upper_terrace_noise(seed: i64) -> Noise2D {
    hills(seed, 18, 30)
}

pub fn shoreline_tide_level_noise(seed: i64) -> Noise2D {
    n(simplex(seed)
        .octaves(3)
        .spread(f(0.005f32))
        .scaled(sea(-6), sea(6)))
    .clamped(SEA, sea(4))
    .add(n(simplex(seed).spread(0.03)))
}

/// `BiomeNoiseSampler`: a height per column and a density per block for carved biomes.
pub trait BiomeNoiseSampler: Send {
    fn set_column(&mut self, x: i32, z: i32);
    fn height(&self) -> f64;
    fn noise(&mut self, y: i32) -> f64;
}

pub const SOLID: f64 = 0.0;
pub const AIR_THRESHOLD: f64 = 0.4;

struct HeightOnly {
    noise: Noise2D,
    height: f32,
}

impl BiomeNoiseSampler for HeightOnly {
    fn set_column(&mut self, x: i32, z: i32) {
        self.height = self.noise.noise(f64::from(x), f64::from(z)) as f32;
    }
    fn height(&self) -> f64 {
        f64::from(self.height)
    }
    fn noise(&mut self, _y: i32) -> f64 {
        SOLID
    }
}

pub fn sampler_from_height_noise(noise: Noise2D) -> Box<dyn BiomeNoiseSampler> {
    Box::new(HeightOnly { noise, height: 0.0 })
}

struct UndergroundLakes {
    height_noise: Noise2D,
    blobs: Noise2D,
    depth: Noise2D,
    center_noise: Noise2D,
    surface_height: f64,
    center: f64,
    lake_height: f64,
}

pub fn underground_lakes(seed: i64, height_noise: Noise2D) -> Box<dyn BiomeNoiseSampler> {
    Box::new(UndergroundLakes {
        height_noise,
        blobs: n(simplex(seed + 1).spread(f(0.04f32))).abs(),
        depth: n(simplex(seed + 2)
            .octaves(4)
            .scaled(2.0, 18.0)
            .spread(f(0.2f32))),
        center_noise: n(simplex(seed + 3)
            .octaves(2)
            .spread(f(0.06f32))
            .scaled(sea(-4), sea(4))),
        surface_height: 0.0,
        center: 0.0,
        lake_height: 0.0,
    })
}

impl BiomeNoiseSampler for UndergroundLakes {
    fn set_column(&mut self, x: i32, z: i32) {
        let (x, z) = (f64::from(x), f64::from(z));
        let h0 = clamp(
            (f(0.7f32) - self.blobs.noise(x, z)) * f64::from(1.0f32 / 0.3f32),
            0.0,
            1.0,
        );
        let h1 = self.depth.noise(x, z);
        self.surface_height = self.height_noise.noise(x, z);
        self.center = self.center_noise.noise(x, z);
        self.lake_height = h0 * h1;
    }
    fn height(&self) -> f64 {
        self.surface_height
    }
    fn noise(&mut self, y: i32) -> f64 {
        let delta = (self.center - f64::from(y)).abs();
        clamp(
            f(0.2f32) + f(0.05f32) * (self.lake_height - delta),
            0.0,
            1.0,
        )
    }
}

struct Cenotes {
    cells: Cellular2D,
    height_noise: Noise2D,
    opening_height_noise: Noise2D,
    tunnel_center_noise: Noise2D,
    tunnel_depth_noise: Noise2D,
    tunnel_size_noise: Noise2D,
    cliff_noise: OpenSimplex3D,
    x: i32,
    z: i32,
    surface_height: f64,
    tunnel_center_dist: f64,
    tunnel_depth: f64,
    tunnel_size: f64,
    noise: f64,
    scale: f64,
    max_radius: f64,
    cenote_center_dist: f64,
    opening_height: f64,
}

pub fn cenotes(seed: i64, height_noise: Noise2D) -> Box<dyn BiomeNoiseSampler> {
    Box::new(Cenotes {
        cells: Cellular2D::with_sample(seed + 432, 2).spread(0.012),
        height_noise,
        opening_height_noise: n(simplex(seed + 1432)
            .octaves(2)
            .spread(0.04)
            .scaled(-10.0, 10.0)),
        tunnel_center_noise: n(simplex(seed + 1112).octaves(3)).abs().spread(0.05),
        tunnel_depth_noise: n(simplex(seed + 41)
            .octaves(3)
            .spread(0.05)
            .scaled(-10.0, -35.0)),
        tunnel_size_noise: n(simplex(seed + 331)
            .octaves(2)
            .spread(0.07)
            .scaled(5.0, 12.0)),
        cliff_noise: cliff_noise(seed),
        x: 0,
        z: 0,
        surface_height: 0.0,
        tunnel_center_dist: 0.0,
        tunnel_depth: 0.0,
        tunnel_size: 0.0,
        noise: 0.0,
        scale: 0.0,
        max_radius: 0.0,
        cenote_center_dist: 0.0,
        opening_height: 0.0,
    })
}

impl BiomeNoiseSampler for Cenotes {
    fn set_column(&mut self, x: i32, z: i32) {
        let (fx, fz) = (f64::from(x), f64::from(z));
        let cell = self.cells.cell(fx, fz);
        self.surface_height = self.height_noise.noise(fx, fz);
        self.tunnel_center_dist = self.tunnel_center_noise.noise(fx, fz);
        self.tunnel_depth = self.tunnel_depth_noise.noise(fx, fz);
        self.tunnel_size = self.tunnel_size_noise.noise(fx, fz);
        self.x = x;
        self.z = z;
        self.noise = cell.noise;
        if self.noise > 0.0 {
            let (f1, f2) = (cell.f1, cell.f2);
            self.scale = (self.noise * 0.4 + 0.6)
                * clamped_map(self.surface_height, SEA, SEA + 30.0, 0.4, 1.0);
            self.max_radius = 0.05 * self.scale;
            self.cenote_center_dist = f1 + clamped_map(f2 - f1, 0.0, 0.1, self.max_radius, 0.0);
            self.opening_height = self.opening_height_noise.noise(fx, fz);
        }
    }
    fn height(&self) -> f64 {
        self.surface_height
    }
    fn noise(&mut self, y: i32) -> f64 {
        let fy = f64::from(y);
        let mut cenote_noise = 0.0;
        if self.noise > 0.0 && self.cenote_center_dist < self.max_radius {
            let cenote_height = self.scale * 45.0;
            let depth = 0.0f64.max(self.opening_height + self.surface_height - fy);
            let radius = if depth < cenote_height / 3.0 {
                self.max_radius * 3.0 * depth / cenote_height
            } else {
                clamped_map(
                    depth,
                    0.9 * cenote_height,
                    cenote_height,
                    self.max_radius,
                    0.0,
                )
            };
            cenote_noise = 100.0 * (radius - self.cenote_center_dist)
                + 2.0
                    * self
                        .cliff_noise
                        .noise(f64::from(self.x), fy, f64::from(self.z));
        }
        let mut tunnel_noise = 0.0;
        if self.tunnel_center_dist < 0.15 {
            let center_height = self.surface_height + self.tunnel_depth;
            let vertical = clamped_map((fy - center_height).abs(), 0.0, 5.0, 1.0, 0.0);
            let horizontal = clamped_map(self.tunnel_center_dist, 0.0, 0.15, 1.0, 0.0);
            tunnel_noise = vertical * horizontal * self.tunnel_size;
        }
        cenote_noise + tunnel_noise
    }
}

/// `getOceanRidgeWarpedEdgeDistanceAndScale(x, z, seed, false).x`, with the sampler cached per seed.
pub fn ocean_ridge_distance_at(seed: i64, x: i32, z: i32) -> f64 {
    static CACHE: std::sync::Mutex<Option<(i64, Noise2D)>> = std::sync::Mutex::new(None);
    let noise = {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        match &*cache {
            Some((s, n)) if *s == seed => n.clone(),
            _ => {
                let n = ocean_ridge_edge_distance(seed);
                *cache = Some((seed, n.clone()));
                n
            }
        }
    };
    noise.noise(f64::from(x), f64::from(z))
}
