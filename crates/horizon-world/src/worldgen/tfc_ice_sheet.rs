//! TerraFirmaCraft's `IceSheetSurfaceBuilder` (EUPL 1.2): glaciers of snow over packed and blue
//! ice, with moraines and bare stony peaks, in thirteen biome flavours.

use super::tfc_biome_noise::{
    glacial_base, glacial_cirques, glacial_cirques_ice_surface_height, glacial_oceanic_base,
    ice_sheet_surface_height, lake, montane_ice_sheet_surface_height,
    oceanic_ice_sheet_surface_height,
};
use super::tfc_noise::Noise2D;
use super::tfc_surface::{
    BuildInfo, Builder, Cell, Column, Layers, RockBlock, SEA_LEVEL, State, SurfaceCtx,
    altitude_slope_depth, build, build_with, fixed, snowable,
};
use super::tfc_volcano_strato::{put_state, state_in, volcanic_mid, volcanic_top};

/// The `SurfaceBuilderFactory` names, in the order of `IceSheetSurfaceBuilder`.
pub const NAMES: [&str; 13] = [
    "IceSheetSurfaceBuilder.NORMAL",
    "IceSheetSurfaceBuilder.EDGE",
    "IceSheetSurfaceBuilder.EDGE_LAKE",
    "IceSheetSurfaceBuilder.HIDDEN_LAKE",
    "IceSheetSurfaceBuilder.ICE_SHEET_MOUNTAINS",
    "IceSheetSurfaceBuilder.GLACIATED_MOUNTAINS",
    "IceSheetSurfaceBuilder.ICE_SHEET_VOLCANIC_MOUNTAINS",
    "IceSheetSurfaceBuilder.GLACIATED_VOLCANIC_MOUNTAINS",
    "IceSheetSurfaceBuilder.OCEANIC",
    "IceSheetSurfaceBuilder.ICE_SHEET_OCEANIC_MOUNTAINS",
    "IceSheetSurfaceBuilder.GLACIATED_OCEANIC_MOUNTAINS",
    "IceSheetSurfaceBuilder.ICE_SHEET_VOLCANIC_OCEANIC_MOUNTAINS",
    "IceSheetSurfaceBuilder.GLACIATED_VOLCANIC_OCEANIC_MOUNTAINS",
];

pub struct IceSheet {
    base: Noise2D,
    surface: Noise2D,
    has_moraines: bool,
    has_stony_peaks: bool,
    is_shore: bool,
    is_volcanic: bool,
}

impl IceSheet {
    /// Builds variant `index` of [`NAMES`].
    pub fn new(index: usize, seed: i64) -> Self {
        let v = |base: Noise2D, surface: Noise2D, m, p, s, vol| Self {
            base,
            surface,
            has_moraines: m,
            has_stony_peaks: p,
            is_shore: s,
            is_volcanic: vol,
        };
        let mountains_surface = || {
            montane_ice_sheet_surface_height(seed)
                .max(glacial_cirques_ice_surface_height(seed).add_constant(39.0))
        };
        let oceanic_mountains_surface =
            || oceanic_ice_sheet_surface_height(seed).max(glacial_cirques_ice_surface_height(seed));
        let cirques39 = || glacial_cirques(seed).add_constant(39.0);
        let cirques_ice39 = || glacial_cirques_ice_surface_height(seed).add_constant(39.0);
        match index {
            0 => v(
                glacial_base(seed),
                ice_sheet_surface_height(seed),
                true,
                false,
                false,
                false,
            ),
            1 => v(
                glacial_base(seed).add_constant(1.6),
                ice_sheet_surface_height(seed),
                true,
                false,
                false,
                false,
            ),
            2 => v(
                lake(seed),
                ice_sheet_surface_height(seed),
                false,
                false,
                false,
                false,
            ),
            3 => v(
                glacial_oceanic_base(seed),
                ice_sheet_surface_height(seed),
                false,
                false,
                false,
                false,
            ),
            4 => v(cirques39(), mountains_surface(), false, true, false, false),
            5 => v(cirques39(), cirques_ice39(), false, true, false, false),
            6 => v(cirques39(), mountains_surface(), false, true, false, true),
            7 => v(cirques39(), cirques_ice39(), false, true, false, true),
            8 => v(
                glacial_oceanic_base(seed),
                oceanic_ice_sheet_surface_height(seed),
                false,
                false,
                true,
                false,
            ),
            9 => v(
                glacial_cirques(seed),
                oceanic_mountains_surface(),
                false,
                true,
                true,
                false,
            ),
            10 => v(
                glacial_cirques(seed),
                glacial_cirques_ice_surface_height(seed),
                false,
                true,
                true,
                false,
            ),
            11 => v(
                glacial_cirques(seed),
                oceanic_mountains_surface(),
                false,
                true,
                true,
                true,
            ),
            _ => v(
                glacial_cirques(seed),
                glacial_cirques_ice_surface_height(seed),
                false,
                true,
                true,
                true,
            ),
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
        let mut ice_depth;
        // A low base groundwater leaves ice exposed where rivers cut into the sheet.
        if self.has_moraines && ctx.base_groundwater <= 20.0 {
            let moraine_crest_height = (0.5
                * f64::from(glacier_surface_height + glacier_base_height))
            .min(f64::from(glacier_base_height + 18));
            ice_depth = ((f64::from(start_y) - moraine_crest_height) * 2.0).max(0.0) as i32;
            // (`Math.max(int, 0)` of the truncated product)
            ice_depth = ice_depth.max(0);
        } else {
            ice_depth = 35;
        }

        if (self.has_stony_peaks && f64::from(start_y) > f64::from(glacier_surface_height) + 2.5)
            || f64::from(start_y) < f64::from(glacier_base_height) - 1.5
        {
            if self.is_volcanic {
                let basalt = fixed("tfc:gravel_basalt");
                let layers = Layers::three(
                    volcanic_top(basalt.clone()),
                    volcanic_mid(basalt),
                    State::Rock(RockBlock::Gravel),
                );
                build_with(column, ctx, start_y, end_y, -3, &layers);
            } else {
                build_with(column, ctx, start_y, end_y, -1, &Layers::grass());
            }
        } else if self.is_shore && start_y <= SEA_LEVEL {
            if let Some(mountains) = Builder::from_name("ShoreAndOceanSurfaceBuilder.MOUNTAINS") {
                build(&mountains, column, ctx, info, start_y, end_y);
            }
        } else {
            let snow = fixed("minecraft:snow_block");
            let ice = fixed("minecraft:packed_ice");
            let blue_ice = fixed("minecraft:blue_ice");
            let moraine_top = snowable(State::Moraine);
            let moraine = State::Moraine;
            let mut surface_depth = -1;
            let mut y = start_y;
            while y >= glacier_base_height - 2 {
                match state_in(column, y) {
                    Cell::Air => surface_depth = -1,
                    Cell::Rock => {
                        if surface_depth == -1 {
                            surface_depth = altitude_slope_depth(y, ctx.slope, -3, 5);
                            if surface_depth <= -1 {
                                // No snow on steep slopes.
                                if ice_depth < 1 {
                                    put_state(column, ctx, y, &moraine);
                                } else if y <= glacier_base_height {
                                    // Not on steep slopes where the glacier base meets the ground.
                                    ice_depth = 0;
                                } else {
                                    put_state(column, ctx, y, &ice);
                                }
                            } else if ice_depth == 0 || y <= SEA_LEVEL || y < glacier_base_height {
                                // No snow without a glacier, or under water; no ice below this.
                                put_state(column, ctx, y, &moraine);
                                ice_depth = 0;
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
                        } else {
                            let state = if y == start_y { &moraine_top } else { &moraine };
                            put_state(column, ctx, y, state);
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
    fn every_variant_has_a_matching_name_and_builds() {
        for i in 0..NAMES.len() {
            let sheet = IceSheet::new(i, 77);
            assert!(sheet.base.noise(10.0, 10.0).is_finite(), "{}", NAMES[i]);
            assert!(sheet.surface.noise(10.0, 10.0).is_finite(), "{}", NAMES[i]);
        }
    }

    #[test]
    fn mountain_variants_expose_stony_peaks() {
        assert!(IceSheet::new(4, 1).has_stony_peaks);
        assert!(!IceSheet::new(0, 1).has_stony_peaks);
        assert!(IceSheet::new(6, 1).is_volcanic && !IceSheet::new(4, 1).is_volcanic);
    }
}
