//! TerraFirmaCraft's cinder cone, tuya, tuff ring and atoll surface builders (EUPL 1.2). Each
//! wraps the next one in TFC (`CinderCone(Tuya(TuffRings(Atoll(Stratovolcano(biome)))))`), so
//! `build` is tried first and the rest of the chain runs when it returns `false`.

use super::tfc_biome_ext::extensions;
use super::tfc_noise::clamped_map;
use super::tfc_region::hash_double;
use super::tfc_surface::{
    Cell, Column, RockBlock, SoilType, State, SurfaceCtx, altitude_slope_depth, fixed,
    silty_surface_type,
};
use super::tfc_volcano::Volcanoes;
use super::tfc_volcano_strato::{put_name, put_state, state_in, volcanic_mid, volcanic_top};

const SEA_LEVEL: i32 = 63;

/// What the four builders need besides the column.
pub struct Args<'a> {
    pub volcanoes: &'a Volcanoes,
    /// Biomes at the middle of the chunk's cinder, tuff ring, tuya and atoll cells.
    pub biomes: [i32; 4],
    pub pre_volcanic: i32,
    /// The weight of the column's biome in the blend.
    pub weight: f64,
    /// The ocean floor height (`OCEAN_FLOOR_WG`).
    pub ocean_floor: i32,
}

/// Tries the cinder cone, tuya, tuff ring and atoll builders in TFC's order. Returns `true` when
/// one of them built the column.
pub fn build(
    column: &mut Column,
    ctx: &mut SurfaceCtx,
    start_y: i32,
    end_y: i32,
    a: &Args,
) -> bool {
    let ext = extensions();
    let (x, z) = (ctx.x, ctx.z);
    let v = a.volcanoes;

    // Cinder cones.
    let e = &ext[a.biomes[0] as usize];
    if e.cinder_cones {
        let easing = v.cinder_easing(x, z, e.feature_frequency);
        if easing > 0.6 && start_y > a.pre_volcanic {
            cinder_surface(column, ctx, start_y, a.pre_volcanic - 5, easing);
            return true;
        }
    }

    // Tuyas.
    let e = &ext[a.biomes[2] as usize];
    if e.tuyas {
        let easing = v.tuya_easing(x, z, e.feature_frequency);
        if 1.0 - easing < 0.16 {
            tuya_surface(column, start_y, end_y, a.pre_volcanic);
            return true;
        }
    }

    // Tuff rings.
    let e = &ext[a.biomes[1] as usize];
    if e.tuff_rings {
        let easing = v.tuff_easing(x, z, e.feature_frequency);
        if easing > 0.6 {
            if start_y < SEA_LEVEL + 3 {
                // `VOLCANIC_SHORE_SAND`: green sand in wet places, else black sand.
                tuff_surface(
                    column,
                    ctx,
                    start_y,
                    end_y,
                    a.weight,
                    &State::VolcanicShoreSand,
                    &State::VolcanicShoreSand,
                );
            } else {
                let local = State::Rock(RockBlock::Gravel);
                tuff_surface(
                    column,
                    ctx,
                    start_y,
                    end_y,
                    a.weight,
                    &volcanic_top(local.clone()),
                    &volcanic_mid(local),
                );
            }
            return true;
        }
    }

    // Atolls.
    let e = &ext[a.biomes[3] as usize];
    if e.atolls {
        let (easing, cell) = v.atoll_surface_easing(x, z, e.feature_frequency);
        if easing > 0.0 && atoll_surface(column, ctx, start_y, a, v, easing, &cell) {
            return true;
        }
    }
    false
}

/// `CinderConeSurfaceBuilder.buildVolcanicSurface`.
fn cinder_surface(
    column: &mut Column,
    ctx: &mut SurfaceCtx,
    start_y: i32,
    end_y: i32,
    easing: f32,
) {
    let mut surface_depth = -1;
    let mut y = start_y;
    while y >= end_y {
        let at = state_in(column, y);
        if at == Cell::Air {
            surface_depth = -1;
        } else if at == Cell::Rock {
            if surface_depth == -1 {
                surface_depth = altitude_slope_depth(y, ctx.slope, 4, 5);
                surface_depth = ((surface_depth as f32 * (easing - 0.6) / 0.4) as i32).clamp(2, 11);
                put_name(column, y, "tfc:raw_basalt".to_owned());
            } else if surface_depth > 0 {
                surface_depth -= 1;
                put_name(column, y, "tfc:raw_basalt".to_owned());
            }
        }
        y -= 1;
    }
}

/// `TuyaSurfaceBuilder.buildVolcanicSurface`.
fn tuya_surface(column: &mut Column, start_y: i32, end_y: i32, pre_volcanic: i32) {
    let mut surface_depth = -1;
    let mut y = start_y;
    while y >= end_y {
        let at = state_in(column, y);
        if at == Cell::Air {
            surface_depth = -1;
        } else if at == Cell::Rock && y > pre_volcanic {
            if surface_depth == -1 {
                surface_depth = 40;
                put_name(column, y, "tfc:raw_basalt".to_owned());
            } else if surface_depth > 0 {
                surface_depth -= 1;
                put_name(column, y, "tfc:raw_basalt".to_owned());
            }
        }
        y -= 1;
    }
}

/// `TuffRingsSurfaceBuilder.buildTuffSurface`.
fn tuff_surface(
    column: &mut Column,
    ctx: &mut SurfaceCtx,
    start_y: i32,
    end_y: i32,
    weight: f64,
    top_state: &State,
    mid_state: &State,
) {
    let tuff = fixed("tfc:raw_tuff");
    let tuff_gravel = fixed("tfc:gravel_tuff");
    let under_state = &tuff;
    let under_water_state = &tuff_gravel;
    let mut surface_depth = -1;
    let mut surface_y = 0;
    let (mut underwater_layer, mut first_layer) = (false, false);
    let mut surface_state = under_state;
    let mut tuff_depth = (20.0 * weight) as i32;
    let mut y = start_y;
    while y >= end_y {
        let at = state_in(column, y);
        if at == Cell::Air {
            surface_depth = -1;
        } else if at == Cell::Rock {
            if surface_depth == -1 {
                surface_y = y;
                first_layer = true;
                if y < SEA_LEVEL - 1 {
                    surface_depth = altitude_slope_depth(surface_y, ctx.slope, -1, 5);
                    if surface_depth < -1 {
                        surface_depth = 0;
                        put_state(column, ctx, y, &tuff);
                    } else if surface_depth == -1 {
                        surface_depth = 0;
                        put_state(column, ctx, y, under_water_state);
                    } else {
                        put_state(column, ctx, y, under_water_state);
                    }
                    surface_state = under_water_state;
                    underwater_layer = true;
                } else {
                    surface_depth = altitude_slope_depth(surface_y, ctx.slope, -3, 5);
                    if surface_depth < -1 {
                        put_state(column, ctx, y, &tuff);
                        surface_depth = 0;
                    } else if surface_depth == -1 {
                        surface_depth = 0;
                        put_state(column, ctx, y, under_state);
                    } else {
                        put_state(column, ctx, y, top_state);
                    }
                    surface_state = mid_state;
                    underwater_layer = false;
                }
            } else if surface_depth > 0 {
                surface_depth -= 1;
                put_state(column, ctx, y, surface_state);
                if surface_depth == 0 && first_layer {
                    first_layer = false;
                    surface_depth = altitude_slope_depth(surface_y, ctx.slope, 0, 5);
                    if !underwater_layer {
                        surface_state = under_state;
                    }
                }
            } else if tuff_depth > 0 {
                put_state(column, ctx, y, &tuff);
                tuff_depth -= 1;
            }
        }
        y -= 1;
    }
}

/// `AtollSurfaceBuilder.buildSurface` after its easing check. Returns `false` when it falls
/// through to the parent builder.
fn atoll_surface(
    column: &mut Column,
    ctx: &mut SurfaceCtx,
    start_y: i32,
    a: &Args,
    v: &Volcanoes,
    easing: f32,
    cell: &super::tfc_noise::Cell,
) -> bool {
    // The atoll wrapper of the column's own biome: each biome has its own `heightNoise`.
    let random_height = match ctx
        .noises
        .biome_atoll_height
        .get(ctx.original_biome as usize)
    {
        Some(noise) => noise.noise(f64::from(ctx.x), f64::from(ctx.z)),
        None => v.atoll_random_height(ctx.x, ctx.z),
    };
    let sea = f64::from(SEA_LEVEL);
    let volcano_height = clamped_map(
        f64::from(easing),
        0.4,
        0.7,
        sea - 70.0,
        sea - 8.0 + random_height,
    ) as i32;
    let max_depth = a.pre_volcanic.max(volcano_height);
    // Pink sand in wet places, else yellow, on a cell's far side white; the grass blends into it.
    let (grass, sand) = if hash_double(cell.noise, 6324) > 0.7 {
        if ctx.rainfall > 375.0 {
            (
                silty_surface_type(SoilType::Grass, fixed("tfc:sand/pink")),
                fixed("tfc:sand/pink"),
            )
        } else {
            (
                silty_surface_type(SoilType::Grass, fixed("tfc:sand/yellow")),
                fixed("tfc:sand/yellow"),
            )
        }
    } else {
        (
            silty_surface_type(SoilType::Grass, fixed("tfc:sand/white")),
            fixed("tfc:sand/white"),
        )
    };
    let rock = if hash_double(cell.noise, 624) > 0.7 {
        fixed("tfc:raw_dolomite")
    } else {
        fixed("tfc:raw_limestone")
    };
    if a.ocean_floor <= max_depth + 2 {
        return false;
    }
    let sea_i = SEA_LEVEL;
    let mut surface_depth = -1.0f64;
    let mut y = start_y;
    while y >= max_depth {
        let at = state_in(column, y);
        if at == Cell::Air {
            surface_depth = -1.0;
        } else if at == Cell::Rock {
            let above_sand_line = f64::from(y) > f64::from(sea_i) - 11.0 + random_height;
            if surface_depth == -1.0 {
                surface_depth = 0.0;
                if y > sea_i + 2 {
                    put_state(column, ctx, y, &grass);
                } else if above_sand_line {
                    put_state(column, ctx, y, &sand);
                } else {
                    put_state(column, ctx, y, &rock);
                }
            } else if surface_depth < 5.0 + random_height {
                if above_sand_line {
                    put_state(column, ctx, y, &sand);
                    surface_depth += 1.0;
                } else {
                    put_state(column, ctx, y, &rock);
                    surface_depth = 20.0;
                }
            } else {
                put_state(column, ctx, y, &rock);
            }
        }
        y -= 1;
    }
    true
}
