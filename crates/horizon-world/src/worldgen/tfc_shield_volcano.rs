//! TerraFirmaCraft's `ShieldVolcanoSurfaceBuilder` and `IceSheetShieldVolcanoSurfaceBuilder`
//! (EUPL 1.2), plus the lava-flow beaches of the shield volcano shores.

use super::tfc_biome_noise::{
    glaciated_shield_volcano, hot_spot_intensity, ice_sheet_surface_height, lava_flow,
    lava_flow_material, shield_volcano_glacier_surface, shield_volcano_ice_sheet_surface,
};
use super::tfc_noise::Noise2D;
use super::tfc_surface::{
    BuildInfo, Builder, Cell, Column, Layers, SEA_LEVEL, State, SurfaceCtx, altitude_slope_depth,
    build, build_with, fixed, snowable,
};
use super::tfc_volcano_strato::{put_state, state_in, volcanic_mid, volcanic_top};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Fresh lava flows over the volcano.
    Active,
    Dormant,
    /// The sandy shore of a shield volcano.
    Shore,
}

fn basalt() -> State {
    fixed("tfc:raw_basalt")
}
fn basalt_cobble() -> State {
    fixed("tfc:cobble_basalt")
}
fn basalt_gravel() -> State {
    fixed("tfc:gravel_basalt")
}

/// `ShieldVolcanoSurfaceBuilder.buildSurface`: picks the states, lava flows included.
pub fn build_shield(
    kind: Kind,
    column: &mut Column,
    ctx: &mut SurfaceCtx,
    start_y: i32,
    end_y: i32,
) {
    let (top, mid, bot, under) = if kind == Kind::Shore {
        (
            State::VolcanicShoreSand,
            State::VolcanicShoreSand,
            State::VolcanicShoreSandstone,
            State::VolcanicShoreSand,
        )
    } else {
        (
            volcanic_top(basalt_gravel()),
            volcanic_mid(basalt_gravel()),
            basalt_gravel(),
            basalt_gravel(),
        )
    };
    if kind != Kind::Active {
        layers(column, ctx, start_y, end_y, &top, &mid, &bot, &under);
        return;
    }
    let (x, z) = (f64::from(ctx.x), f64::from(ctx.z));
    let (noise, flow) = ctx.lava_sample.unwrap_or_else(|| {
        (
            ctx.noises.lava_flow_material.noise(x, z),
            ctx.noises.lava_flow.noise(x, z),
        )
    });
    let snowy = |s: State| snowable(s);
    if flow < 0.40 {
        layers(column, ctx, start_y, end_y, &top, &mid, &bot, &under);
    } else if flow < 0.50 {
        if noise > 0.0 {
            layers(
                column,
                ctx,
                start_y,
                end_y,
                &snowy(basalt_gravel()),
                &basalt_gravel(),
                &basalt(),
                &basalt_gravel(),
            );
        } else {
            layers(column, ctx, start_y, end_y, &top, &mid, &bot, &under);
        }
    } else if flow < 0.75 {
        if noise > 0.0 {
            layers(
                column,
                ctx,
                start_y,
                end_y,
                &snowy(basalt_gravel()),
                &basalt_gravel(),
                &basalt(),
                &basalt_gravel(),
            );
        } else {
            layers(
                column,
                ctx,
                start_y,
                end_y,
                &snowy(basalt_cobble()),
                &basalt_cobble(),
                &basalt(),
                &basalt_cobble(),
            );
        }
    } else if noise > -0.6 {
        layers(
            column,
            ctx,
            start_y,
            end_y,
            &snowy(basalt()),
            &basalt(),
            &basalt(),
            &basalt_cobble(),
        );
    } else {
        layers(
            column,
            ctx,
            start_y,
            end_y,
            &snowy(basalt_cobble()),
            &basalt_cobble(),
            &basalt(),
            &basalt_cobble(),
        );
    }
}

/// `ShieldVolcanoSurfaceBuilder.buildSurface(context, startY, endY, top, mid, under, underWater)`.
#[allow(clippy::too_many_arguments)]
fn layers(
    column: &mut Column,
    ctx: &mut SurfaceCtx,
    start_y: i32,
    end_y: i32,
    top_state: &State,
    mid_state: &State,
    under_state: &State,
    under_water_state: &State,
) {
    let bedrock = basalt();
    let mut surface_depth = -1;
    let mut surface_y = 0;
    let (mut underwater_layer, mut first_layer) = (false, false);
    let mut surface_state = &bedrock;
    let mut basalt_depth = (20.0 * ctx.weight) as i32;
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
                        put_state(column, ctx, y, &bedrock);
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
                        put_state(column, ctx, y, &bedrock);
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
                    if underwater_layer {
                        surface_state = under_state;
                    }
                }
            } else if basalt_depth > 0 {
                put_state(column, ctx, y, &bedrock);
                basalt_depth -= 1;
            }
        }
        y -= 1;
    }
}

/// The lava flow beaches of `ShoreAndOceanSurfaceBuilder.buildLavaFlowSurface`. Java never reaches
/// it (see the shore builder in `tfc_surface.rs`), so nothing calls this.
#[allow(dead_code)]
pub fn build_lava_flow_shore(column: &mut Column, ctx: &mut SurfaceCtx, start_y: i32, end_y: i32) {
    let (x, z) = (f64::from(ctx.x), f64::from(ctx.z));
    let noise = ctx.noises.lava_flow_material.noise(x, z);
    let flow = ctx.noises.lava_flow.noise(x, z);
    let surface = State::VolcanicShoreSand;
    let subsurface = State::VolcanicShoreSandstone;
    let l = |t: State, m: State, u: State, w: State, thin: State| Layers::five(t, m, u, w, thin);
    let layers = if flow < 0.40 || (flow < 0.50 && noise <= 0.0) {
        l(
            surface.clone(),
            surface.clone(),
            subsurface,
            surface.clone(),
            surface,
        )
    } else if flow < 0.50 {
        l(
            snowable(basalt_gravel()),
            surface.clone(),
            subsurface,
            surface.clone(),
            surface,
        )
    } else if flow < 0.75 {
        if noise > 0.0 {
            l(
                snowable(basalt_gravel()),
                basalt_gravel(),
                basalt(),
                basalt_gravel(),
                surface,
            )
        } else {
            l(
                snowable(basalt_cobble()),
                basalt_cobble(),
                basalt(),
                basalt_cobble(),
                surface,
            )
        }
    } else if noise > -0.6 {
        l(
            snowable(basalt()),
            basalt(),
            basalt(),
            basalt(),
            basalt_cobble(),
        )
    } else {
        l(
            snowable(basalt_cobble()),
            basalt_cobble(),
            basalt(),
            basalt(),
            basalt_cobble(),
        )
    };
    build_with(column, ctx, start_y, end_y, -1, &layers);
}

/// The two noises the volcano flow builders read.
pub fn flow_noises(seed: i64) -> (Noise2D, Noise2D) {
    (lava_flow(seed), lava_flow_material(seed))
}

/// `IceSheetShieldVolcanoSurfaceBuilder`.
pub struct IceShield {
    base: Noise2D,
    surface: Noise2D,
    min_freezing_height: i32,
}

impl IceShield {
    /// `0` is `ICE_SHEET`, anything else `GLACIATED`.
    pub fn new(index: usize, seed: i64) -> Self {
        let hot = || hot_spot_intensity(seed);
        let base = glaciated_shield_volcano(seed, hot());
        if index == 0 {
            Self {
                base,
                surface: ice_sheet_surface_height(seed)
                    .max(shield_volcano_ice_sheet_surface(seed, hot())),
                min_freezing_height: SEA_LEVEL,
            }
        } else {
            Self {
                base,
                surface: ice_sheet_surface_height(seed)
                    .max(shield_volcano_glacier_surface(seed, hot())),
                min_freezing_height: SEA_LEVEL + 30,
            }
        }
    }

    pub fn build(
        &self,
        column: &mut Column,
        ctx: &mut SurfaceCtx,
        info: &BuildInfo,
        start_y: i32,
        end_y: i32,
    ) {
        let (x, z) = (f64::from(ctx.x), f64::from(ctx.z));
        let glacier_base_height = self.base.noise(x, z).ceil() as i32;
        let glacier_surface_height = self.surface.noise(x, z).ceil() as i32;
        // This builder has no moraines, so the ice is always 35 deep.
        let mut ice_depth = 35;

        if start_y <= SEA_LEVEL {
            if let Some(shore) =
                Builder::from_name("ShoreAndOceanSurfaceBuilder.OLD_SHIELD_VOLCANO")
            {
                build(&shore, column, ctx, info, start_y, end_y);
            }
        } else if start_y < self.min_freezing_height
            || f64::from(start_y) > f64::from(glacier_surface_height) + 2.5
            || f64::from(start_y) < f64::from(glacier_base_height) - 1.5
        {
            build_shield(Kind::Dormant, column, ctx, start_y, end_y);
        } else {
            let snow = fixed("minecraft:snow_block");
            let ice = fixed("minecraft:packed_ice");
            let blue_ice = fixed("minecraft:blue_ice");
            let moraine_top = snowable(State::BasaltMoraine);
            let moraine = State::BasaltMoraine;
            let bedrock = basalt();
            let mut surface_depth = -1;
            let mut y = start_y;
            while y >= glacier_base_height - 22 {
                match state_in(column, y) {
                    Cell::Air => surface_depth = -1,
                    Cell::Rock => {
                        if surface_depth == -1 {
                            surface_depth = altitude_slope_depth(y, ctx.slope, -3, 5);
                            if surface_depth <= -1 {
                                if ice_depth < 1 {
                                    put_state(column, ctx, y, &moraine);
                                } else if y <= glacier_base_height {
                                    ice_depth = 0;
                                } else {
                                    put_state(column, ctx, y, &ice);
                                }
                            } else if ice_depth == 0 || y <= SEA_LEVEL || y < glacier_base_height {
                                put_state(column, ctx, y, &moraine);
                            } else {
                                put_state(column, ctx, y, &snow);
                            }
                            surface_depth = 1;
                        } else if ice_depth > 0 && y > glacier_base_height {
                            ice_depth -= 1;
                            let state = if y < glacier_surface_height - 16 {
                                &blue_ice
                            } else {
                                &ice
                            };
                            put_state(column, ctx, y, state);
                        } else if y > glacier_base_height {
                            let state = if y == start_y { &moraine_top } else { &moraine };
                            put_state(column, ctx, y, state);
                        } else {
                            put_state(column, ctx, y, &bedrock);
                        }
                    }
                    _ => {}
                }
                y -= 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ice_shield_variants_differ_in_freezing_height() {
        let ice = IceShield::new(0, 5);
        let glaciated = IceShield::new(1, 5);
        assert_eq!(ice.min_freezing_height + 30, glaciated.min_freezing_height);
        assert!(ice.base.noise(3.0, 4.0).is_finite());
        assert!(glaciated.surface.noise(3.0, 4.0).is_finite());
    }

    #[test]
    fn flow_noises_are_finite() {
        let (flow, material) = flow_noises(9);
        assert!(flow.noise(100.0, -50.0).is_finite());
        assert!(material.noise(100.0, -50.0).is_finite());
    }
}
