//! TerraFirmaCraft's biome and rock area layers (`TFCLayers.createRegionBiomeLayer`,
//! `createOverworldRockLayer` and the layers they chain; EUPL 1.2). Both produce lazily
//! sampled integer grids: the biome layer at quart scale, the rock layer at block scale.

use super::tfc_biome::*;
use super::tfc_layers::{Area, AreaFactory, Zoom, adjacent_layer, smooth_layer, zoom_layer};
use super::tfc_region::{Point, RegionGenerator, Seed, units};
use std::sync::Arc;

/// `RegionLayer` followed by `RegionBiomeLayer` or `RegionRockLayer`: reads one value of the
/// region point at each grid coordinate.
fn region_value_layer(generator: Arc<RegionGenerator>, value: fn(&Point) -> i32) -> AreaFactory {
    Arc::new(move || {
        let generator = generator.clone();
        Area::new(
            move |x, z| {
                let region = generator.get_or_create_region(x, z);
                value(
                    region
                        .at(x, z)
                        .expect("region does not contain the grid point"),
                )
            },
            1024,
        )
    })
}

fn any_of(north: i32, east: i32, south: i32, west: i32, test: impl Fn(i32) -> bool) -> bool {
    test(north) || test(east) || test(south) || test(west)
}

/// `RegionEdgeBiomeLayer`.
fn region_edge_biome(north: i32, east: i32, south: i32, west: i32, center: i32) -> i32 {
    let m = |test: &dyn Fn(i32) -> bool| any_of(north, east, south, west, test);
    if is_low(center) {
        if m(&is_ocean) && m(&is_mountains) {
            return OCEANIC_MOUNTAINS;
        } else if m(&is_ocean) && m(&|i| i == LOWLANDS) {
            return SALT_MARSH;
        }
    }
    if is_flats(center) && m(&is_ocean) && m(&is_flats) {
        return CANYONS;
    }
    if center == PLATEAU || center == BADLANDS {
        if m(&|i| i == LOW_CANYONS || i == LOWLANDS) {
            return HILLS;
        } else if m(&|i| i == PLAINS || i == HILLS) {
            return ROLLING_HILLS;
        }
    } else if is_mountains(center) {
        if m(&is_low) {
            return ROLLING_HILLS;
        }
    } else if center == LOWLANDS || center == LOW_CANYONS || center == PLAINS || center == HILLS {
        if m(&|i| i == PLATEAU || i == BADLANDS) {
            return HILLS;
        } else if m(&is_mountains) {
            return ROLLING_HILLS;
        }
    } else if center == DEEP_OCEAN_TRENCH && m(&|i| !is_ocean(i)) {
        return OCEAN;
    }
    center
}

/// `ShoreAndRiverLayer`.
fn shore_and_river(north: i32, east: i32, south: i32, west: i32, center: i32) -> i32 {
    let m = |test: &dyn Fn(i32) -> bool| any_of(north, east, south, west, test);
    if has_shore(center) {
        if m(&is_ocean) {
            return shore_for(center);
        }
    } else if center == RIVER_VALLEY {
        if m(&is_mountains) {
            return PLATEAU_WIDE;
        } else if m(&is_ocean) {
            return SALT_MARSH;
        }
    }
    center
}

/// `MoreShoresLayer`.
fn more_shores(north: i32, east: i32, south: i32, west: i32, center: i32) -> i32 {
    let m = |test: &dyn Fn(i32) -> bool| any_of(north, east, south, west, test);
    if m(&is_ocean) {
        if m(&|l| l == TERRACE_LOWER) {
            return TERRACE_UPPER;
        }
        if m(&|l| l == SEA_STACKS) {
            return SEA_STACKS;
        }
        if m(&|l| l == TIDAL_FLATS) {
            return SHORE;
        }
        if m(&|l| l == COASTAL_DUNES) {
            return COASTAL_DUNES;
        }
        if m(&|l| l == SETBACK_CLIFFS) {
            return SETBACK_CLIFFS;
        }
        if m(&|l| l == ROCKY_SHORES) {
            return ROCKY_SHORES;
        }
        if m(&|l| l == EMBAYMENTS) {
            return EMBAYMENTS;
        }
    }
    center
}

/// `IceSheetEdgeLayer.isNotIceSheet`.
pub fn is_not_ice_sheet(value: i32) -> bool {
    value != ICE_SHEET
        && value != ICE_SHEET_TUYAS
        && value != SUBGLACIAL_LAKE
        && value != ICE_SHEET_MOUNTAINS
        && value != ICE_SHEET_OCEANIC_MOUNTAINS
        && value != ICE_SHEET_SHIELD_VOLCANO
        && value != ICE_SHEET_VOLCANIC_MOUNTAINS
        && value != ICE_SHEET_VOLCANIC_OCEANIC_MOUNTAINS
}

/// `IceSheetEdgeLayer.isNotIceSheetOrGlaciated`.
pub fn is_not_ice_sheet_or_glaciated(value: i32) -> bool {
    is_not_ice_sheet(value)
        && value != GLACIATED_MOUNTAINS
        && value != GLACIATED_OCEANIC_MOUNTAINS
        && value != GLACIATED_SHIELD_VOLCANO
        && value != GLACIATED_VOLCANIC_MOUNTAINS
        && value != GLACIATED_VOLCANIC_OCEANIC_MOUNTAINS
}

/// `IceSheetEdgeLayer`.
fn ice_sheet_edge(north: i32, east: i32, south: i32, west: i32, center: i32) -> i32 {
    let m = |test: &dyn Fn(i32) -> bool| any_of(north, east, south, west, test);
    let mountain_ice = |i: i32| {
        i == ICE_SHEET_MOUNTAINS
            || i == ICE_SHEET_MOUNTAINS_EDGE
            || i == ICE_SHEET_OCEANIC_MOUNTAINS
            || i == ICE_SHEET_OCEANIC
    };
    if center == KNOB_AND_KETTLE
        || center == PATTERNED_GROUND
        || center == INVERTED_PATTERNED_GROUND
        || center == STONE_CIRCLES
    {
        if m(&|i| i == ICE_SHEET_TUYAS) {
            return ICE_SHEET_TUYAS_EDGE;
        } else if m(&is_flat_ice_sheet) {
            return ICE_SHEET_EDGE;
        }
    }
    if (center == ICE_SHEET_OCEANIC_MOUNTAINS || center == ICE_SHEET_VOLCANIC_OCEANIC_MOUNTAINS)
        && m(&is_not_ice_sheet)
    {
        return ICE_SHEET_OCEANIC_MOUNTAINS_EDGE;
    }
    if (center == ICE_SHEET_MOUNTAINS || center == ICE_SHEET_VOLCANIC_MOUNTAINS)
        && m(&is_not_ice_sheet)
    {
        return ICE_SHEET_MOUNTAINS_EDGE;
    }
    if center == ICE_SHEET_EDGE && m(&mountain_ice) {
        return KNOB_AND_KETTLE;
    }
    if center == LAKE && m(&is_flat_ice_sheet) && !m(&mountain_ice) {
        return SUBGLACIAL_LAKE;
    }
    if is_flat_ice_sheet(center) && m(&|i| i == MELTWATER_LAKE) && m(&is_not_ice_sheet) {
        return SUBGLACIAL_LAKE;
    }
    if is_flat_ice_sheet(center)
        && m(&|i| {
            i == OCEAN
                || i == OCEAN_REEF
                || i == DEEP_OCEAN
                || i == DEEP_OCEAN_TRENCH
                || i == ICE_SHEET_SHORE
        })
    {
        return ICE_SHEET_OCEANIC;
    }
    if is_not_ice_sheet_or_glaciated(center) {
        if m(&|i| i == GLACIATED_MOUNTAINS) {
            return GLACIALLY_CARVED_MOUNTAINS;
        } else if m(&|i| i == GLACIATED_OCEANIC_MOUNTAINS) {
            return GLACIALLY_CARVED_OCEANIC_MOUNTAINS;
        }
    }
    if (center == PLATEAU
        || center == BADLANDS
        || center == BURREN_BADLANDS
        || center == BURREN_BADLANDS_TALL
        || center == GLACIATED_SHIELD_VOLCANO)
        && m(&|i| i == ICE_SHEET_OCEANIC_MOUNTAINS_EDGE)
    {
        return GLACIATED_OCEANIC_MOUNTAINS;
    }
    if (center == ICE_SHEET || center == ICE_SHEET_TUYAS || center == ICE_SHEET_SHIELD_VOLCANO)
        && m(&|i| i == ICE_SHEET_OCEANIC_MOUNTAINS_EDGE)
    {
        return ICE_SHEET_OCEANIC;
    }
    if center == ICE_SHEET_MOUNTAINS && m(&|i| i == ICE_SHEET_OCEANIC_MOUNTAINS_EDGE) {
        return ICE_SHEET_OCEANIC_MOUNTAINS;
    }
    center
}

/// `TFCLayers.createRegionBiomeLayer`: biome ids at quart resolution. Draws eleven seeds from
/// `seed` (which must already have been used to build `generator`, in Java's order).
pub fn create_region_biome_layer(generator: Arc<RegionGenerator>, seed: &mut Seed) -> AreaFactory {
    let _region_layer_seed = seed.next();
    let mut layer = region_value_layer(generator, |p| p.biome);
    layer = adjacent_layer(seed.next(), layer, |_, n, e, s, w, c| {
        region_edge_biome(n, e, s, w, c)
    });
    layer = zoom_layer(Zoom::Normal, seed.next(), layer);
    layer = adjacent_layer(seed.next(), layer, |_, n, e, s, w, c| {
        shore_and_river(n, e, s, w, c)
    });
    layer = adjacent_layer(seed.next(), layer, |_, n, e, s, w, c| {
        more_shores(n, e, s, w, c)
    });
    layer = adjacent_layer(seed.next(), layer, |_, n, e, s, w, c| {
        ice_sheet_edge(n, e, s, w, c)
    });
    for _ in 0..4 {
        layer = zoom_layer(Zoom::Normal, seed.next(), layer);
    }
    smooth_layer(seed.next(), layer)
}

/// `TFCLayers.createOverworldRockLayer`: region rock values zoomed to block resolution.
/// `seed` is the raw value `RegionChunkDataGenerator` drew first (`RegionGenerator::chunk_data_seeds[0]`).
pub fn create_overworld_rock_layer(generator: Arc<RegionGenerator>, seed: i64) -> AreaFactory {
    let mut layer = region_value_layer(generator, |p| p.rock);
    for _ in 0..units::GRID_BITS - 1 {
        layer = zoom_layer(Zoom::Normal, seed, layer);
    }
    layer = smooth_layer(seed, layer);
    layer = zoom_layer(Zoom::Normal, seed, layer);
    smooth_layer(seed, layer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::tfc_region::Settings;

    #[allow(clippy::type_complexity)]
    const LAYERS: &[(bool, i64, i32, i32, i64, i64)] = &[
        (
            false,
            1234567,
            0,
            0,
            -4789645822472437541,
            -6488107101272750080,
        ),
        (false, 1234567, 500, -700, 0, 1439819098403564200),
        (
            false,
            1234567,
            -1500,
            2000,
            7667944593557769768,
            3196428984749717504,
        ),
        (
            false,
            1234567,
            3000,
            3100,
            5831073320154142720,
            -4569939838378156079,
        ),
        (
            false,
            1234567,
            -4000,
            -2500,
            7160175705115141160,
            4104967164662804032,
        ),
        (
            false,
            1234567,
            7000,
            900,
            3254188272590163935,
            -3450403749266784908,
        ),
        (true, 424242, 0, 0, 0, 2168436716528570368),
        (
            true,
            424242,
            500,
            -700,
            -8428225675015574528,
            8105742907210629120,
        ),
        (
            true,
            424242,
            -1500,
            2000,
            -8428225675015574528,
            -4492651332761760672,
        ),
        (
            true,
            424242,
            3000,
            3100,
            -8428225675015574528,
            6807328963421081600,
        ),
        (
            true,
            424242,
            -4000,
            -2500,
            -8428225675015574528,
            8930618257394520064,
        ),
        (
            true,
            424242,
            7000,
            900,
            -8428225675015574528,
            4961628431386263864,
        ),
    ];

    #[test]
    fn biome_and_rock_layers_match_java_bit_for_bit() {
        for finite in [false, true] {
            let level_seed = if finite { 424242 } else { 1234567 };
            let settings = Settings {
                finite_continents: finite,
                ..Settings::default()
            };
            let mut seed = Seed::of(level_seed);
            let generator = Arc::new(RegionGenerator::from_seed(settings, &mut seed));
            let rock_seed = generator.chunk_data_seeds[0];
            let mut biome = create_region_biome_layer(generator.clone(), &mut seed)();
            let mut rock = create_overworld_rock_layer(generator, rock_seed)();
            for (f, ls, gx, gz, want_biome, want_rock) in LAYERS.iter().filter(|r| r.0 == finite) {
                assert_eq!(*ls, level_seed);
                let (mut hm, mut hr) = (0i64, 0i64);
                for dz in 0..40 {
                    for dx in 0..40 {
                        hm = hm
                            .wrapping_mul(31)
                            .wrapping_add(i64::from(biome.get(gx * 32 + dx - 8, gz * 32 + dz - 8)));
                    }
                }
                for dz in 0..40 {
                    for dx in 0..40 {
                        hr = hr.wrapping_mul(31).wrapping_add(i64::from(
                            rock.get(gx * 128 + dx * 3 - 20, gz * 128 + dz * 3 - 20),
                        ));
                    }
                }
                assert_eq!(hm, *want_biome, "biome layer at {gx},{gz} finite={f}");
                assert_eq!(hr, *want_rock, "rock layer at {gx},{gz} finite={f}");
            }
        }
    }
}
