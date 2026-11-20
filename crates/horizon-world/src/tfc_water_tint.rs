//! TerraFirmaCraft's climate-tinted water (`TFCColors.getWaterColor`, EUPL 1.2). A content mod
//! that ships TFC's `water.png` colour map turns it on; until then nothing changes.

use std::sync::{Arc, RwLock};

static COLORMAP: RwLock<Option<Arc<Vec<u32>>>> = RwLock::new(None);
static FOG_COLORMAP: RwLock<Option<Arc<Vec<u32>>>> = RwLock::new(None);

/// Installs the underwater fog colour map (`water_fog.png`).
pub fn set_fog_colormap(colors: Vec<u32>) {
    if colors.len() == 256 * 256 {
        *FOG_COLORMAP.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(colors));
    }
}

/// Installs the 256 x 256 colour map (RGB, row-major, as `water.png` reads).
pub fn set_colormap(colors: Vec<u32>) {
    if colors.len() == 256 * 256 {
        *COLORMAP.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(colors));
    }
}

/// Forgets the colour map (tests).
pub fn clear_colormap() {
    *COLORMAP.write().unwrap_or_else(|e| e.into_inner()) = None;
    *FOG_COLORMAP.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// `getClimateColor(colorCache, temperature, groundwater)`.
pub fn climate_color(map: &[u32], temperature: f32, groundwater: f32) -> u32 {
    let temperature_index = 255 - (((temperature + 20.0) * 255.0 / 50.0) as i32).clamp(0, 255);
    let rainfall_index = 255 - ((groundwater * 255.0 / 500.0) as i32).clamp(0, 255);
    map[(temperature_index | rainfall_index << 8) as usize]
}

/// The water colour at a column of the running TFC world, or `None` while no mod provides the
/// colour map or the world is not a TFC one. It uses the average year: the calendar's month and
/// day swings are not applied.
pub fn water_color(x: i32, z: i32) -> Option<u32> {
    sample(&COLORMAP, x, z)
}

/// `TFCColors.getWaterFogColor`: the underwater fog colour at a column, likewise.
pub fn water_fog_color(x: i32, z: i32) -> Option<u32> {
    sample(&FOG_COLORMAP, x, z)
}

fn sample(slot: &RwLock<Option<Arc<Vec<u32>>>>, x: i32, z: i32) -> Option<u32> {
    let map = slot.read().ok()?.clone()?;
    let terrain = crate::worldgen::tfc_terrain::active()?;
    let (temperature, groundwater) = terrain.average_climate(x, z);
    Some(climate_color(&map, temperature, groundwater))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_is_hot_on_the_left_and_wet_at_the_top() {
        let map: Vec<u32> = (0..65536u32).collect();
        // 30 C and 500 mm: the top-left corner.
        assert_eq!(climate_color(&map, 30.0, 500.0), 0);
        // -20 C and dry: the bottom-right corner.
        assert_eq!(climate_color(&map, -20.0, 0.0), 65535);
        // Out-of-range values clamp.
        assert_eq!(climate_color(&map, 99.0, 9999.0), 0);
    }
}
