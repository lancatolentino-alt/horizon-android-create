//! TerraFirmaCraft's frozen ocean extension (EUPL 1.2): `ShoreAndOceanSurfaceBuilder
//! .frozenOceanExtension` and `placeSeaIce`. Cold oceans get a skin of sea ice and, where the
//! iceberg noise is high enough, packed ice and snow icebergs.
//!
//! The noises are the ones every `ShoreAndOceanSurfaceBuilder` builds from the stable seed, so one
//! set serves all of them. TFC draws the iceberg's snow and holes from the generator's shared
//! `RandomSource`, whose state depends on the order chunks happen to generate in; a hash of the
//! column's position stands in.

use super::tfc_biome_noise::sea_ice_noise;
use super::tfc_noise::Noise2D;
use super::tfc_surface::{Cell, Column, SEA_LEVEL, SurfaceCtx, hash};
use horizon_core::noise::{Noise, NoiseStack, NormalNoise};
use horizon_core::random::{RandomSource, XoroshiroRandom};

const ICE: &str = "tfc:sea_ice";
const PACKED_ICE: &str = "minecraft:packed_ice";
const SNOW: &str = "minecraft:snow_block";

pub struct SeaIce {
    pillar: NoiseStack,
    pillar_roof: NoiseStack,
    surface: NoiseStack,
    patterned: Noise2D,
    hemisphere_scale: f32,
}

fn clamped_map(value: f32, in_min: f32, in_max: f32, out_min: f32, out_max: f32) -> f32 {
    let t = ((value - in_min) / (in_max - in_min)).clamp(0.0, 1.0);
    out_min + t * (out_max - out_min)
}

impl SeaIce {
    pub fn new(level_seed: i64, hemisphere_scale: f32) -> Self {
        // `seed.forkStable().fork()`: a fresh sequence of the level seed, two longs per fork.
        let mut stable = XoroshiroRandom::new(level_seed);
        let (lo, hi) = (stable.next_long(), stable.next_long());
        let mut random = XoroshiroRandom::from_state(lo as u64, hi as u64);
        let mut make = |octave: i32, amplitudes: &[f64]| {
            NormalNoise::create_parity(octave, amplitudes)
                .and_then(|n| n.create(&mut random))
                .expect("iceberg noise parameters are valid")
        };
        let pillar = make(-6, &[1.0, 1.0, 1.0, 1.0]);
        let pillar_roof = make(-3, &[1.0]);
        let surface = make(-6, &[1.0, 1.0, 1.0]);
        // `BiomeNoise.seaIceNoise(seed.forkStable().next())`.
        let patterned = sea_ice_noise(XoroshiroRandom::new(level_seed).next_long());
        Self {
            pillar,
            pillar_roof,
            surface,
            patterned,
            hemisphere_scale,
        }
    }

    /// `SolarCalculator.getInNorthernHemisphere(z, hemisphereScale)`.
    fn northern(&self, z: i32) -> bool {
        let scale = self.hemisphere_scale;
        if scale == 0.0 {
            return true;
        }
        let adjusted = z.wrapping_sub((scale / 2.0) as i32);
        let pole_to_pole = (scale * 2.0) as i32;
        let normalized = adjusted.rem_euclid(pole_to_pole * 2);
        normalized > pole_to_pole
    }

    /// `OverworldClimateModel.getAverageMonthlyTemperature(z, seaLevel, average, 1, true)`: at
    /// sea level the month's full swing is added to the average.
    pub fn max_annual_temperature(&self, z: i32, average: f32) -> f32 {
        let factor = if self.northern(z) { 1.0f32 } else { -1.0 };
        let scale = self.hemisphere_scale;
        let monthly = if scale == 0.0 {
            0.0
        } else {
            // `Helpers.triangle(-18, 0, 1 / (4 * scale), z - scale / 2)`.
            let frequency = 1.0 / (4.0 * scale);
            let value = z as f32 - scale / 2.0;
            let floor = (frequency * value + 0.75).floor();
            0.0 + -18.0 * ((4.0 * frequency * value + 1.0 - 4.0 * floor).abs() - 1.0)
        };
        average + factor * monthly
    }

    /// `frozenOceanExtension`, run after a shore or ocean column is built.
    pub fn extend(
        &self,
        column: &mut Column,
        ctx: &SurfaceCtx,
        start_y: i32,
        end_y: i32,
        ocean_floor_y: i32,
    ) {
        let sea_level = SEA_LEVEL;
        if start_y > sea_level {
            return;
        }
        let (x, z) = (ctx.x, ctx.z);
        let max_temperature = self.max_annual_temperature(z, ctx.temperature);
        if max_temperature > 2.0 {
            return;
        }
        let (fx, fz) = (f64::from(x), f64::from(z));
        let base_noise = (f64::from(self.surface.get3(fx, 0.0, fz)) * 8.25)
            .abs()
            .min(f64::from(self.pillar.get3(fx * 1.28, 0.0, fz * 1.28)) * 15.0);
        if base_noise <= 1.8 {
            self.place_sea_ice(column, ctx, max_temperature);
            return;
        }
        let temperature_factor = clamped_map(max_temperature, -1.0, 2.0, 1.0, 0.5);
        let depth_factor = clamped_map(
            ocean_floor_y as f32,
            (sea_level - 20) as f32,
            (sea_level - 6) as f32,
            1.0,
            0.0,
        );
        let pillar_noise =
            (f64::from(self.pillar_roof.get3(fx * 1.17, 0.0, fz * 1.17)) * 1.5).abs();
        let mut iceberg_max_y = f64::from(temperature_factor)
            * f64::from(depth_factor)
            * (base_noise * base_noise * 1.2).min((pillar_noise * 30.0).ceil() + 11.0);
        if iceberg_max_y < 2.0 {
            self.place_sea_ice(column, ctx, max_temperature);
            return;
        }
        let mut random = XoroshiroRandom::new(i64::from(hash(0x1ce_b3e6, x, 0, z)));
        let iceberg_min_y = f64::from(sea_level) - iceberg_max_y - 7.0;
        iceberg_max_y += f64::from(sea_level);
        let snow_depth = 2 + random.next_int_bound(4).unwrap();
        let snow_boundary_y = sea_level + 18 + random.next_int_bound(10).unwrap();
        let top = start_y.max(iceberg_max_y as i32 + 1);
        // Icebergs rise above the terrain, which the column stops at.
        while column.min_y + column.cells.len() as i32 <= top {
            column.cells.push(Cell::Air);
        }
        let mut placed_snow = 0;
        let mut y = top;
        while y >= end_y {
            let cell = column.get(y).clone();
            let air_hit =
                cell == Cell::Air && f64::from(y) < iceberg_max_y && random.next_double() > 0.01;
            let water_hit = !air_hit
                && cell == Cell::Water
                && y > iceberg_min_y as i32
                && y < sea_level
                && iceberg_min_y != 0.0
                && random.next_double() > 0.15;
            if air_hit || water_hit {
                if placed_snow <= snow_depth && y > snow_boundary_y {
                    column.set(y, SNOW.to_string());
                    placed_snow += 1;
                } else {
                    column.set(y, PACKED_ICE.to_string());
                }
            }
            y -= 1;
        }
    }

    /// `placeSeaIce`: a cellular pattern of ice that fills in as the temperature falls.
    fn place_sea_ice(&self, column: &mut Column, ctx: &SurfaceCtx, max_temperature: f32) {
        const ICE_START: f64 = 1.5;
        const SOLID_ICE_START: f64 = -0.5;
        let temperature = f64::from(max_temperature);
        let place = if temperature < SOLID_ICE_START {
            true
        } else if temperature > ICE_START {
            false
        } else {
            let t = ((temperature - ICE_START) / (SOLID_ICE_START - ICE_START)).clamp(0.0, 1.0);
            let threshold = 0.3 + t * (0.04 - 0.3);
            self.patterned.noise(f64::from(ctx.x), f64::from(ctx.z)) > threshold
        };
        if place {
            let y = SEA_LEVEL - 1;
            if y >= column.min_y
                && ((y - column.min_y) as usize) < column.cells.len()
                && *column.get(y) == Cell::Water
            {
                column.set(y, ICE.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(water_to: i32) -> Column {
        let min_y = 0;
        let cells = (min_y..=water_to + 1)
            .map(|y| {
                if y < 20 {
                    Cell::Rock
                } else if y <= water_to {
                    Cell::Water
                } else {
                    Cell::Air
                }
            })
            .collect();
        Column { min_y, cells }
    }

    #[test]
    fn monthly_swing_has_opposite_signs_in_the_two_hemispheres() {
        let ice = SeaIce::new(7, 20_000.0);
        // Same latitude distance from the equator, opposite hemispheres: opposite season.
        let north = ice.max_annual_temperature(5_000, 0.0);
        let south = ice.max_annual_temperature(5_000 + 20_000, 0.0);
        assert!(north != south);
        // The swing never exceeds the triangle's amplitude.
        assert!(north.abs() <= 18.0 && south.abs() <= 18.0);
    }

    #[test]
    fn warm_columns_stay_open_water() {
        let ice = SeaIce::new(7, 20_000.0);
        let ctx_noises = super::super::tfc_surface::Noises::new(7);
        let mut rock = |_y: i32| "granite";
        let mut ctx = test_ctx(&ctx_noises, &mut rock, 25.0);
        ctx.x = 10;
        ctx.z = 10;
        let mut col = column(62);
        ice.extend(&mut col, &ctx, 63, 0, 40);
        assert!(col.cells.iter().all(|c| !matches!(c, Cell::Placed(_))));
    }

    #[test]
    fn freezing_columns_get_ice_somewhere_and_only_on_the_surface_water() {
        let ice = SeaIce::new(7, 20_000.0);
        let noises = super::super::tfc_surface::Noises::new(7);
        let mut frozen = 0;
        for i in 0..400 {
            let mut rock = |_y: i32| "granite";
            let mut ctx = test_ctx(&noises, &mut rock, -30.0);
            ctx.x = 1000 + i * 3;
            ctx.z = 500 + i * 7;
            let mut col = column(62);
            ice.extend(&mut col, &ctx, 63, 0, 40);
            for (y, cell) in col.cells.iter().enumerate() {
                if let Cell::Placed(name) = cell {
                    if name == ICE {
                        assert_eq!(
                            y as i32,
                            SEA_LEVEL - 1,
                            "sea ice only replaces the top water"
                        );
                    }
                    frozen += 1;
                }
            }
        }
        // At -30 C every column is solid ice at least; icebergs add more.
        assert!(frozen >= 400);
    }

    #[test]
    fn deep_freezing_water_grows_icebergs_above_the_surface() {
        let ice = SeaIce::new(7, 20_000.0);
        let noises = super::super::tfc_surface::Noises::new(7);
        let mut above = 0;
        let mut below = 0;
        for i in 0..6000 {
            let mut rock = |_y: i32| "granite";
            let mut ctx = test_ctx(&noises, &mut rock, -30.0);
            ctx.x = i * 5;
            ctx.z = i * 3 - 4000;
            let mut col = column(62);
            ice.extend(&mut col, &ctx, 63, 0, 40);
            for (y, cell) in col.cells.iter().enumerate() {
                if let Cell::Placed(name) = cell
                    && name != ICE
                {
                    assert!(name == PACKED_ICE || name == SNOW);
                    if y as i32 >= SEA_LEVEL {
                        above += 1;
                    } else {
                        below += 1;
                    }
                }
            }
        }
        assert!(above > 0, "no iceberg rose above the sea");
        assert!(below > 0, "no iceberg reached under the sea");
    }

    fn test_ctx<'a>(
        noises: &'a super::super::tfc_surface::Noises,
        rock: &'a mut dyn FnMut(i32) -> &'static str,
        temperature: f32,
    ) -> SurfaceCtx<'a> {
        SurfaceCtx {
            lava_sample: None,
            x: 0,
            y: 63,
            z: 0,
            rock,
            temperature,
            base_groundwater: 0.0,
            rainfall: 100.0,
            rain_variance: 0.0,
            slope: 0.0,
            noises,
            sea_level_rock: "granite",
            original_biome: 0,
            tide: 0.0,
            weight: 1.0,
        }
    }
}
