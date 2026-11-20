//! TerraFirmaCraft's cave, deep cave and canyon carvers (EUPL 1.2). TFC reuses the vanilla cave
//! and canyon carvers with its own `carveBlock`: carved cells become lava at and below the lava
//! level, and otherwise whatever the aquifer says. Every TFC biome lists the same three carvers,
//! so one fixed set serves all chunks.

use super::anchor::WorldGenerationContext;
use super::carver::{WorldCarver, generate_positional_carving_mask};
use super::tfc_aquifer::Fluid;
use crate::registry::BlockStateId;
use horizon_core::position::ChunkPos;
use std::sync::{Arc, OnceLock};

const CAVE: &str = r##"{"type":"minecraft:cave","probability":0.02,
"y":{"type":"minecraft:uniform","min_inclusive":{"absolute":-56},"max_inclusive":{"absolute":126}},
"room_vertical_radius_multiplier":{"type":"minecraft:uniform","min_inclusive":0.1,"max_exclusive":0.9},
"count":{"type":"minecraft:very_biased_to_bottom","min_inclusive":0,"max_inclusive":14},
"thickness":{"type":"minecraft:trapezoid","min":0.0,"max":3.0,"plateau":1.0},
"weird_thickness_bias":true,

"lava_level":{"above_bottom":8},"aquifers_enabled":true,
"horizontal_radius_multiplier":{"type":"minecraft:uniform","min_inclusive":1.5,"max_exclusive":2.5},
"vertical_radius_multiplier":{"type":"minecraft:uniform","min_inclusive":1.5,"max_exclusive":2.5},
"floor_level":{"type":"minecraft:uniform","min_inclusive":-1.0,"max_exclusive":0.3},
"replaceable":"#minecraft:overworld_carver_replaceables"}"##;

const DEEP_CAVE: &str = r##"{"type":"minecraft:cave","probability":0.08,
"y":{"type":"minecraft:uniform","min_inclusive":{"absolute":-56},"max_inclusive":{"absolute":48}},
"room_vertical_radius_multiplier":{"type":"minecraft:uniform","min_inclusive":0.1,"max_exclusive":0.9},
"count":{"type":"minecraft:very_biased_to_bottom","min_inclusive":0,"max_inclusive":14},
"thickness":{"type":"minecraft:trapezoid","min":0.0,"max":3.0,"plateau":1.0},
"weird_thickness_bias":true,

"lava_level":{"above_bottom":8},"aquifers_enabled":true,
"horizontal_radius_multiplier":{"type":"minecraft:uniform","min_inclusive":2.0,"max_exclusive":3.5},
"vertical_radius_multiplier":{"type":"minecraft:uniform","min_inclusive":2.0,"max_exclusive":3.5},
"floor_level":{"type":"minecraft:uniform","min_inclusive":-1.0,"max_exclusive":0.3},
"replaceable":"#minecraft:overworld_carver_replaceables"}"##;

const CANYON: &str = r##"{"type":"minecraft:canyon","probability":0.01,
"y":{"type":"minecraft:uniform","min_inclusive":{"absolute":10},"max_inclusive":{"absolute":67}},
"lava_level":{"above_bottom":8},"aquifers_enabled":true,
"vertical_rotation":{"type":"minecraft:uniform","min_inclusive":-0.125,"max_exclusive":0.125},
"shape":{"y_scale":3.0,"distance_factor":{"type":"minecraft:uniform","min_inclusive":0.75,"max_exclusive":1.0},
"thickness":{"type":"minecraft:trapezoid","min":0.0,"max":6.0,"plateau":2.0},
"width_smoothness":3,
"horizontal_radius_factor":{"type":"minecraft:uniform","min_inclusive":0.75,"max_exclusive":1.0},
"vertical_radius_default_factor":1.0,"vertical_radius_center_factor":0.0},
"replaceable":"#minecraft:overworld_carver_replaceables"}"##;

/// The configs use the current carver format (TFC ships the older one, which Horizon's
/// pack upgrader maps to the same `count`, `thickness` and `y_scale` values used here).
/// The `tfc:cave`, `tfc:deep_cave` and `tfc:canyon` carvers, in the biome files' order.
pub fn carvers() -> &'static [Arc<WorldCarver>] {
    static SET: OnceLock<Vec<Arc<WorldCarver>>> = OnceLock::new();
    SET.get_or_init(|| {
        [CAVE, DEEP_CAVE, CANYON]
            .iter()
            .map(|json| Arc::new(WorldCarver::from_json(json).expect("TFC carver config decodes")))
            .collect()
    })
}

/// The block ids the carving pass distinguishes.
pub struct Ids {
    pub air: BlockStateId,
    pub water: BlockStateId,
    pub lava: BlockStateId,
    pub bedrock: BlockStateId,
    /// Blocks outside `#minecraft:overworld_carver_replaceables`, which carving leaves alone.
    pub unreplaceable: Vec<BlockStateId>,
    /// A content mod's salt water, when one is loaded: coastal columns hold it where the aquifer
    /// says water.
    pub salt_water: Option<BlockStateId>,
}

/// Surface blocks the terrain emits that TFC's carver tag does not list.
pub const UNREPLACEABLE: [&str; 4] = [
    "minecraft:blue_ice",
    "tfc:sea_ice",
    "tfc:hardened_clay",
    "tfc:halite",
];

/// Carves the chunk at `pos`. `lava_level` is the carver's `above_bottom(8)` resolved against the
/// world's minimum Y. Each carved cell takes the state TFC's aquifer gives it. Returns how many
/// cells were carved.
pub fn carve<E>(
    seed: i64,
    pos: ChunkPos,
    generation: WorldGenerationContext,
    min_y: i32,
    ids: &Ids,
    salty: impl Fn(i32, i32) -> bool,
    aquifer: &mut super::tfc_aquifer::TfcAquifer<'_>,
    mut get: impl FnMut(i32, i32, i32) -> BlockStateId,
    mut set: impl FnMut(i32, i32, i32, BlockStateId) -> Result<(), E>,
) -> Result<usize, E> {
    let set_carvers = carvers();
    let mask = match generate_positional_carving_mask(seed, pos, generation, false, |_| {
        Ok::<_, std::convert::Infallible>(set_carvers)
    }) {
        Ok(mask) => mask,
        // The mask only fails on out of range bounds or exhausted work budgets; leave the terrain.
        Err(_) => return Ok(0),
    };
    let lava_level = min_y + 8;
    let (x0, z0) = (pos.x.wrapping_shl(4), pos.z.wrapping_shl(4));
    let mut carved = 0;
    for column in mask.columns() {
        let (x, z) = (x0 + column.x, z0 + column.z);
        for y in (column.min_y..=column.max_y).rev() {
            let at = get(x, y, z);
            if at == ids.air
                || at == ids.water
                || Some(at) == ids.salt_water
                || at == ids.lava
                || at == ids.bedrock
                || ids.unreplaceable.contains(&at)
            {
                continue;
            }
            // `CaveWorldCarver.getCarveState`: lava at the bottom, else whatever TFC's aquifer
            // holds there (density 0); a barrier leaves the rock alone.
            let state = if y <= lava_level {
                ids.lava
            } else {
                match aquifer.sample_state(x, y, z, 0.0) {
                    Some(Fluid::Air) => ids.air,
                    Some(Fluid::Water) => match ids.salt_water {
                        Some(salt) if salty(x, z) => salt,
                        _ => ids.water,
                    },
                    Some(Fluid::Lava) => ids.lava,
                    None => continue,
                }
            };
            set(x, y, z, state)?;
            carved += 1;
        }
    }
    Ok(carved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carvers_decode_and_cut_solid_rock() {
        assert_eq!(carvers().len(), 3);
        let ids = Ids {
            air: BlockStateId(1),
            water: BlockStateId(2),
            lava: BlockStateId(3),
            bedrock: BlockStateId(4),
            unreplaceable: vec![BlockStateId(8)],
            salt_water: None,
        };
        let generation = WorldGenerationContext::new(-64, 384, 63);
        let (mut carved, mut lava) = (0, 0);
        for cx in -12..12 {
            let caves = super::super::tfc_caves::CaveNoise::new(7);
            let mut aquifer =
                super::super::tfc_aquifer::TfcAquifer::new(cx, 5, -64, 63, &caves, [-100; 16]);
            let n = carve::<()>(
                1234,
                ChunkPos::new(cx, 5),
                generation,
                -64,
                &ids,
                |_, _| false,
                &mut aquifer,
                |_, _, _| BlockStateId(9),
                |_, y, _, id| {
                    if id == BlockStateId(3) {
                        assert!(y <= -55);
                        lava += 1;
                    }
                    Ok(())
                },
            )
            .unwrap();
            carved += n;
        }
        assert!(carved > 0, "no carving in 24 chunks of solid rock");
        // The same chunks made wholly of an unreplaceable block are left untouched.
        let mut touched = 0;
        for cx in -12..12 {
            let caves = super::super::tfc_caves::CaveNoise::new(7);
            let mut aquifer =
                super::super::tfc_aquifer::TfcAquifer::new(cx, 5, -64, 63, &caves, [-100; 16]);
            touched += carve::<()>(
                1234,
                ChunkPos::new(cx, 5),
                generation,
                -64,
                &ids,
                |_, _| false,
                &mut aquifer,
                |_, _, _| BlockStateId(8),
                |_, _, _, _| Ok(()),
            )
            .unwrap();
        }
        assert_eq!(touched, 0);
        // Salt water already in the column is left alone like water.
        let salty_ids = Ids {
            salt_water: Some(BlockStateId(9)),
            ..Ids {
                air: ids.air,
                water: ids.water,
                lava: ids.lava,
                bedrock: ids.bedrock,
                unreplaceable: vec![],
                salt_water: None,
            }
        };
        let caves = super::super::tfc_caves::CaveNoise::new(7);
        let mut aquifer =
            super::super::tfc_aquifer::TfcAquifer::new(0, 5, -64, 63, &caves, [-100; 16]);
        let carved_into_salt = carve::<()>(
            1234,
            ChunkPos::new(0, 5),
            generation,
            -64,
            &salty_ids,
            |_, _| true,
            &mut aquifer,
            |_, _, _| BlockStateId(9),
            |_, _, _, _| Ok(()),
        )
        .unwrap();
        assert_eq!(carved_into_salt, 0);
        let _ = lava;
    }

    #[test]
    fn tfc_carvers_use_the_positional_stream() {
        use super::super::carver::generate_carving_mask;
        let generation = WorldGenerationContext::new(-64, 384, 63);
        let set = carvers();
        let (mut same, mut total) = (0, 0);
        for cx in 0..6 {
            let pos = ChunkPos::new(cx, 9);
            let tfc = |p| {
                generate_positional_carving_mask(777, p, generation, false, |_| {
                    Ok::<_, std::convert::Infallible>(set)
                })
                .unwrap()
            };
            let legacy = generate_carving_mask(777, pos, generation, false, |_| {
                Ok::<_, std::convert::Infallible>(set)
            })
            .unwrap();
            let a: Vec<_> = tfc(pos)
                .columns()
                .map(|c| (c.x, c.z, c.min_y, c.max_y))
                .collect();
            let b: Vec<_> = tfc(pos)
                .columns()
                .map(|c| (c.x, c.z, c.min_y, c.max_y))
                .collect();
            assert_eq!(a, b, "the positional stream is deterministic");
            let l: Vec<_> = legacy
                .columns()
                .map(|c| (c.x, c.z, c.min_y, c.max_y))
                .collect();
            total += 1;
            same += usize::from(a == l);
        }
        assert!(
            same < total,
            "TFC's stream carves differently from the legacy one"
        );
    }
}
