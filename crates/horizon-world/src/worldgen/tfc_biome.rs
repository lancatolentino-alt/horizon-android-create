//! TerraFirmaCraft's biome ids (`TFCLayers`, in registration order), the biome
//! classification helpers, and the `ChooseBiomes` region task (EUPL 1.2).

use super::tfc_region::{Context, Point};
use horizon_core::random::RandomSource;

pub const OCEAN: i32 = 0;
pub const OCEAN_ATOLLS: i32 = 1;
pub const OCEAN_REEF: i32 = 2;
pub const DEEP_OCEAN: i32 = 3;
pub const DEEP_OCEAN_ATOLLS: i32 = 4;
pub const DEEP_OCEAN_TRENCH: i32 = 5;
pub const OCEANIC_VOLCANIC_ARC: i32 = 6;
pub const OCEAN_RIDGE: i32 = 7;
pub const RIFT_VALLEY: i32 = 8;
pub const RIFT_LAKE: i32 = 9;
pub const COLLISIONAL_MOUNTAINS: i32 = 10;
pub const PLAINS: i32 = 11;
pub const HILLS: i32 = 12;
pub const LOWLANDS: i32 = 13;
pub const SALT_MARSH: i32 = 14;
pub const LOW_CANYONS: i32 = 15;
pub const RIVER_VALLEY: i32 = 16;
pub const ROLLING_HILLS: i32 = 17;
pub const HIGHLANDS: i32 = 18;
pub const BADLANDS: i32 = 19;
pub const PLATEAU: i32 = 20;
pub const PLATEAU_WIDE: i32 = 21;
pub const OLD_MOUNTAINS: i32 = 22;
pub const MOUNTAINS: i32 = 23;
pub const VOLCANIC_MOUNTAINS: i32 = 24;
pub const OCEANIC_MOUNTAINS: i32 = 25;
pub const VOLCANIC_OCEANIC_MOUNTAINS: i32 = 26;
pub const CANYONS: i32 = 27;
pub const GUANO_ISLAND: i32 = 28;
pub const VOLCANIC_ISLAND: i32 = 29;
pub const VOLCANIC_MOUNTAIN_ISLANDS: i32 = 30;
pub const SHORE: i32 = 31;
pub const TIDAL_FLATS: i32 = 32;
pub const SEA_STACKS: i32 = 33;
pub const TERRACE_UPPER: i32 = 34;
pub const TERRACE_LOWER: i32 = 35;
pub const SETBACK_CLIFFS: i32 = 36;
pub const COASTAL_DUNES: i32 = 37;
pub const ROCKY_SHORES: i32 = 38;
pub const EMBAYMENTS: i32 = 39;
pub const LAKE: i32 = 40;
pub const RIVER: i32 = 41;
pub const MOUNTAIN_LAKE: i32 = 42;
pub const VOLCANIC_MOUNTAIN_LAKE: i32 = 43;
pub const OLD_MOUNTAIN_LAKE: i32 = 44;
pub const OCEANIC_MOUNTAIN_LAKE: i32 = 45;
pub const VOLCANIC_OCEANIC_MOUNTAIN_LAKE: i32 = 46;
pub const PLATEAU_LAKE: i32 = 47;
pub const MUD_FLATS: i32 = 48;
pub const SALT_FLATS: i32 = 49;
pub const DUNE_SEA: i32 = 50;
pub const GRASSY_DUNES: i32 = 51;
pub const WHORLED_CANYONS: i32 = 52;
pub const STAIR_STEP_CANYONS: i32 = 53;
pub const MESAS: i32 = 54;
pub const BUTTES: i32 = 55;
pub const HOODOOS: i32 = 56;
pub const ROCKY_PLATEAU: i32 = 57;
pub const TOWER_KARST_PLAINS: i32 = 58;
pub const TOWER_KARST_CANYONS: i32 = 59;
pub const TOWER_KARST_HILLS: i32 = 60;
pub const TOWER_KARST_HIGHLANDS: i32 = 61;
pub const TOWER_KARST_LAKE: i32 = 62;
pub const TOWER_KARST_BAY: i32 = 63;
pub const BURREN_PLATEAU: i32 = 64;
pub const BURREN_BADLANDS: i32 = 65;
pub const BURREN_BADLANDS_TALL: i32 = 66;
pub const BURREN_ROCHE_MOUTONEE: i32 = 67;
pub const BURREN_PLAINS: i32 = 68;
pub const SHILIN_PLAINS: i32 = 69;
pub const SHILIN_CANYONS: i32 = 70;
pub const SHILIN_HILLS: i32 = 71;
pub const SHILIN_HIGHLANDS: i32 = 72;
pub const SHILIN_PLATEAU: i32 = 73;
pub const DOLINE_PLAINS: i32 = 74;
pub const DOLINE_HILLS: i32 = 75;
pub const DOLINE_ROLLING_HILLS: i32 = 76;
pub const DOLINE_HIGHLANDS: i32 = 77;
pub const DOLINE_PLATEAU: i32 = 78;
pub const DOLINE_CANYONS: i32 = 79;
pub const CENOTE_PLAINS: i32 = 80;
pub const CENOTE_HILLS: i32 = 81;
pub const CENOTE_ROLLING_HILLS: i32 = 82;
pub const CENOTE_CANYONS: i32 = 83;
pub const CENOTE_HIGHLANDS: i32 = 84;
pub const CENOTE_PLATEAU: i32 = 85;
pub const EXTREME_DOLINE_PLATEAU: i32 = 86;
pub const EXTREME_DOLINE_MOUNTAINS: i32 = 87;
pub const ACTIVE_SHIELD_VOLCANO: i32 = 88;
pub const DORMANT_SHIELD_VOLCANO: i32 = 89;
pub const EXTINCT_SHIELD_VOLCANO: i32 = 90;
pub const ANCIENT_SHIELD_VOLCANO: i32 = 91;
pub const SUNKEN_SHIELD_VOLCANO: i32 = 92;
pub const SHIELD_VOLCANO_SHORE: i32 = 93;
pub const OLD_SHIELD_VOLCANO_SHORE: i32 = 94;
pub const ICE_SHEET: i32 = 95;
pub const ICE_SHEET_MOUNTAINS: i32 = 96;
pub const ICE_SHEET_OCEANIC_MOUNTAINS: i32 = 97;
pub const ICE_SHEET_VOLCANIC_MOUNTAINS: i32 = 98;
pub const ICE_SHEET_VOLCANIC_OCEANIC_MOUNTAINS: i32 = 99;
pub const ICE_SHEET_SHIELD_VOLCANO: i32 = 100;
pub const ICE_SHEET_TUYAS: i32 = 101;
pub const SUBGLACIAL_LAKE: i32 = 102;
pub const ICE_SHEET_EDGE: i32 = 103;
pub const ICE_SHEET_TUYAS_EDGE: i32 = 104;
pub const ICE_SHEET_OCEANIC: i32 = 105;
pub const ICE_SHEET_OCEANIC_MOUNTAINS_EDGE: i32 = 106;
pub const ICE_SHEET_MOUNTAINS_EDGE: i32 = 107;
pub const GLACIATED_MOUNTAINS: i32 = 108;
pub const GLACIATED_OCEANIC_MOUNTAINS: i32 = 109;
pub const GLACIATED_VOLCANIC_MOUNTAINS: i32 = 110;
pub const GLACIATED_VOLCANIC_OCEANIC_MOUNTAINS: i32 = 111;
pub const MELTWATER_LAKE: i32 = 112;
pub const GLACIATED_SHIELD_VOLCANO: i32 = 113;
pub const ICE_SHEET_SHORE: i32 = 114;
pub const GLACIALLY_CARVED_MOUNTAINS: i32 = 115;
pub const GLACIALLY_CARVED_OCEANIC_MOUNTAINS: i32 = 116;
pub const GLACIALLY_CARVED_VOLCANIC_MOUNTAINS: i32 = 117;
pub const GLACIALLY_CARVED_VOLCANIC_OCEANIC_MOUNTAINS: i32 = 118;
pub const DRUMLINS: i32 = 119;
pub const TUYAS: i32 = 120;
pub const KNOB_AND_KETTLE: i32 = 121;
pub const PATTERNED_GROUND: i32 = 122;
pub const INVERTED_PATTERNED_GROUND: i32 = 123;
pub const STONE_CIRCLES: i32 = 124;

/// The registry path of every biome id, in id order (`TFCBiomes.register`).
pub const BIOME_NAMES: [&str; 125] = [
    "ocean",
    "ocean_atolls",
    "ocean_reef",
    "deep_ocean",
    "deep_ocean_atolls",
    "deep_ocean_trench",
    "oceanic_volcanic_arc",
    "ocean_ridge",
    "rift_valley",
    "rift_lake",
    "collisional_mountains",
    "plains",
    "hills",
    "lowlands",
    "salt_marsh",
    "low_canyons",
    "river_valley",
    "rolling_hills",
    "highlands",
    "badlands",
    "plateau",
    "plateau_wide",
    "old_mountains",
    "mountains",
    "volcanic_mountains",
    "oceanic_mountains",
    "volcanic_oceanic_mountains",
    "canyons",
    "guano_island",
    "volcanic_island",
    "volcanic_mountain_islands",
    "shore",
    "tidal_flats",
    "sea_stacks",
    "terrace_upper",
    "terrace_lower",
    "setback_cliffs",
    "coastal_dunes",
    "rocky_shores",
    "embayments",
    "lake",
    "river",
    "mountain_lake",
    "volcanic_mountain_lake",
    "old_mountain_lake",
    "oceanic_mountain_lake",
    "volcanic_oceanic_mountain_lake",
    "plateau_lake",
    "mud_flats",
    "salt_flats",
    "dune_sea",
    "grassy_dunes",
    "whorled_canyons",
    "stair_step_canyons",
    "mesas",
    "buttes",
    "hoodoos",
    "rocky_plateau",
    "tower_karst_plains",
    "tower_karst_canyons",
    "tower_karst_hills",
    "tower_karst_highlands",
    "tower_karst_lake",
    "tower_karst_bay",
    "burren_plateau",
    "burren_badlands",
    "burren_badlands_tall",
    "burren_roche_moutonee",
    "burren_plains",
    "shilin_plains",
    "shilin_canyons",
    "shilin_hills",
    "shilin_highlands",
    "shilin_plateau",
    "doline_plains",
    "doline_hills",
    "doline_rolling_hills",
    "doline_highlands",
    "doline_plateau",
    "doline_canyons",
    "cenote_plains",
    "cenote_hills",
    "cenote_rolling_hills",
    "cenote_canyons",
    "cenote_highlands",
    "cenote_plateau",
    "extreme_doline_plateau",
    "extreme_doline_mountains",
    "active_shield_volcano",
    "dormant_shield_volcano",
    "extinct_shield_volcano",
    "ancient_shield_volcano",
    "sunken_shield_volcano",
    "shield_volcano_shore",
    "old_shield_volcano_shore",
    "ice_sheet",
    "ice_sheet_mountains",
    "ice_sheet_oceanic_mountains",
    "ice_sheet_volcanic_mountains",
    "ice_sheet_volcanic_oceanic_mountains",
    "ice_sheet_shield_volcano",
    "ice_sheet_tuyas",
    "subglacial_lake",
    "ice_sheet_edge",
    "ice_sheet_tuyas_edge",
    "ice_sheet_oceanic",
    "ice_sheet_oceanic_mountains_edge",
    "ice_sheet_mountains_edge",
    "glaciated_mountains",
    "glaciated_oceanic_mountains",
    "glaciated_volcanic_mountains",
    "glaciated_volcanic_oceanic_mountains",
    "meltwater_lake",
    "glaciated_shield_volcano",
    "ice_sheet_shore",
    "glacially_carved_mountains",
    "glacially_carved_oceanic_mountains",
    "glacially_carved_volcanic_mountains",
    "glacially_carved_volcanic_oceanic_mountains",
    "drumlins",
    "tuyas",
    "knob_and_kettle",
    "patterned_ground",
    "inverted_patterned_ground",
    "stone_circles",
];

pub fn has_shore(value: i32) -> bool {
    !is_ocean(value)
        && value != LOW_CANYONS
        && value != CANYONS
        && value != OCEANIC_MOUNTAINS
        && value != VOLCANIC_OCEANIC_MOUNTAINS
        && value != TOWER_KARST_BAY
        && value != SUNKEN_SHIELD_VOLCANO
        && value != GLACIALLY_CARVED_OCEANIC_MOUNTAINS
        && value != ICE_SHEET_OCEANIC_MOUNTAINS_EDGE
        && value != ICE_SHEET_SHIELD_VOLCANO
        && value != GLACIATED_SHIELD_VOLCANO
        && value != GUANO_ISLAND
        && value != GLACIALLY_CARVED_VOLCANIC_OCEANIC_MOUNTAINS
        && value != VOLCANIC_MOUNTAIN_ISLANDS
}

pub fn shore_for(value: i32) -> i32 {
    if value == LOWLANDS || value == SALT_MARSH {
        return SALT_MARSH;
    }
    if value == MOUNTAINS || value == COLLISIONAL_MOUNTAINS {
        return OCEANIC_MOUNTAINS;
    }
    if value == VOLCANIC_MOUNTAINS {
        return VOLCANIC_OCEANIC_MOUNTAINS;
    }
    if value == TOWER_KARST_LAKE {
        return TOWER_KARST_BAY;
    }
    if value == ACTIVE_SHIELD_VOLCANO {
        return SHIELD_VOLCANO_SHORE;
    }
    if value == DORMANT_SHIELD_VOLCANO
        || value == EXTINCT_SHIELD_VOLCANO
        || value == ANCIENT_SHIELD_VOLCANO
    {
        return OLD_SHIELD_VOLCANO_SHORE;
    }
    if is_flat_ice_sheet(value) || value == ICE_SHEET_EDGE || value == ICE_SHEET_OCEANIC {
        return ICE_SHEET_SHORE;
    }
    if value == ICE_SHEET_OCEANIC_MOUNTAINS || value == ICE_SHEET_VOLCANIC_OCEANIC_MOUNTAINS {
        return ICE_SHEET_OCEANIC_MOUNTAINS_EDGE;
    }
    if value == GLACIATED_OCEANIC_MOUNTAINS
        || value == GLACIATED_MOUNTAINS
        || value == GLACIALLY_CARVED_MOUNTAINS
    {
        return GLACIALLY_CARVED_OCEANIC_MOUNTAINS;
    }
    if value == GLACIATED_VOLCANIC_OCEANIC_MOUNTAINS
        || value == GLACIATED_VOLCANIC_MOUNTAINS
        || value == GLACIALLY_CARVED_VOLCANIC_MOUNTAINS
    {
        return GLACIALLY_CARVED_VOLCANIC_OCEANIC_MOUNTAINS;
    }
    if value == OLD_MOUNTAINS || value == EXTREME_DOLINE_MOUNTAINS {
        return TERRACE_LOWER;
    }
    if value == PLATEAU
        || value == EXTREME_DOLINE_PLATEAU
        || value == BURREN_PLATEAU
        || value == SHILIN_PLATEAU
    {
        return SEA_STACKS;
    }
    if value == PLATEAU_WIDE || value == ROCKY_PLATEAU || value == DOLINE_PLATEAU {
        return SETBACK_CLIFFS;
    }
    if value == HIGHLANDS
        || value == CENOTE_HIGHLANDS
        || value == DOLINE_HIGHLANDS
        || value == SHILIN_HIGHLANDS
        || value == TOWER_KARST_HIGHLANDS
    {
        return ROCKY_SHORES;
    }
    if value == ROLLING_HILLS || value == DOLINE_ROLLING_HILLS || value == CENOTE_ROLLING_HILLS {
        return EMBAYMENTS;
    }
    if value == HILLS
        || value == CENOTE_HILLS
        || value == DOLINE_HILLS
        || value == SHILIN_HILLS
        || value == TOWER_KARST_HILLS
        || value == GRASSY_DUNES
        || value == DUNE_SEA
    {
        return COASTAL_DUNES;
    }
    if value == VOLCANIC_ISLAND {
        return SHORE;
    }
    TIDAL_FLATS
}

pub fn has_lake(value: i32) -> bool {
    !is_ocean(value)
        && !is_lake(value)
        && value != BADLANDS
        && value != ACTIVE_SHIELD_VOLCANO
        && value != DORMANT_SHIELD_VOLCANO
        && value != EXTINCT_SHIELD_VOLCANO
        && value != ANCIENT_SHIELD_VOLCANO
        && value != ICE_SHEET_MOUNTAINS
        && value != ICE_SHEET_MOUNTAINS_EDGE
        && value != ICE_SHEET_OCEANIC_MOUNTAINS
        && value != ICE_SHEET_OCEANIC_MOUNTAINS_EDGE
        && value != ICE_SHEET_SHIELD_VOLCANO
        && value != ICE_SHEET_SHORE
        && value != GLACIATED_SHIELD_VOLCANO
        && value != GLACIATED_MOUNTAINS
        && value != GLACIATED_OCEANIC_MOUNTAINS
        && value != GLACIALLY_CARVED_MOUNTAINS
        && value != GLACIALLY_CARVED_OCEANIC_MOUNTAINS
        && value != SALT_FLATS
        && value != MUD_FLATS
}

pub fn lake_for(value: i32) -> i32 {
    if value == MOUNTAINS || value == COLLISIONAL_MOUNTAINS {
        return MOUNTAIN_LAKE;
    }
    if value == VOLCANIC_MOUNTAINS {
        return VOLCANIC_MOUNTAIN_LAKE;
    }
    if value == OLD_MOUNTAINS || value == EXTREME_DOLINE_MOUNTAINS {
        return OLD_MOUNTAIN_LAKE;
    }
    if value == OCEANIC_MOUNTAINS {
        return OCEANIC_MOUNTAIN_LAKE;
    }
    if value == VOLCANIC_OCEANIC_MOUNTAINS {
        return VOLCANIC_OCEANIC_MOUNTAIN_LAKE;
    }
    if value == PLATEAU
        || value == PLATEAU_WIDE
        || value == ROCKY_PLATEAU
        || value == BURREN_PLATEAU
        || value == SHILIN_PLATEAU
        || value == DOLINE_PLATEAU
        || value == CENOTE_PLATEAU
        || value == EXTREME_DOLINE_PLATEAU
    {
        return PLATEAU_LAKE;
    }
    if is_flat_ice_sheet(value) {
        return SUBGLACIAL_LAKE;
    }
    if value == ICE_SHEET_EDGE {
        return MELTWATER_LAKE;
    }
    if value == TOWER_KARST_CANYONS
        || value == TOWER_KARST_HIGHLANDS
        || value == TOWER_KARST_HILLS
        || value == TOWER_KARST_PLAINS
    {
        return TOWER_KARST_LAKE;
    }
    LAKE
}

pub fn is_ocean(value: i32) -> bool {
    value == OCEAN
        || value == DEEP_OCEAN
        || value == DEEP_OCEAN_TRENCH
        || value == OCEAN_REEF
        || value == OCEAN_RIDGE
        || value == OCEANIC_VOLCANIC_ARC
        || value == OCEAN_ATOLLS
        || value == DEEP_OCEAN_ATOLLS
}

pub fn is_mountains(value: i32) -> bool {
    value == MOUNTAINS
        || value == OCEANIC_MOUNTAINS
        || value == OLD_MOUNTAINS
        || value == VOLCANIC_MOUNTAINS
        || value == VOLCANIC_OCEANIC_MOUNTAINS
        || value == COLLISIONAL_MOUNTAINS
}

pub fn is_low(value: i32) -> bool {
    value == PLAINS
        || value == HILLS
        || value == LOW_CANYONS
        || value == LOWLANDS
        || value == SALT_MARSH
        || value == MUD_FLATS
        || value == SALT_FLATS
        || value == DUNE_SEA
        || value == RIFT_VALLEY
}

pub fn is_flats(value: i32) -> bool {
    value == MUD_FLATS || value == SALT_FLATS
}

pub fn is_flat_ice_sheet(value: i32) -> bool {
    value == ICE_SHEET || value == ICE_SHEET_TUYAS || value == SUBGLACIAL_LAKE
}

pub fn is_lake(value: i32) -> bool {
    value == LAKE
        || value == MELTWATER_LAKE
        || value == MOUNTAIN_LAKE
        || value == OCEANIC_MOUNTAIN_LAKE
        || value == PLATEAU_LAKE
        || value == OLD_MOUNTAIN_LAKE
        || value == SUBGLACIAL_LAKE
        || value == RIFT_LAKE
        || value == TOWER_KARST_LAKE
        || value == VOLCANIC_OCEANIC_MOUNTAIN_LAKE
        || value == VOLCANIC_MOUNTAIN_LAKE
}

const ALTITUDE_BIOMES: [&[i32]; 3] = [
    &[
        PLAINS,
        PLAINS,
        HILLS,
        HILLS,
        ROLLING_HILLS,
        LOW_CANYONS,
        LOWLANDS,
        LOWLANDS,
    ],
    &[
        PLAINS,
        HILLS,
        ROLLING_HILLS,
        ROLLING_HILLS,
        ROLLING_HILLS,
        HIGHLANDS,
        BUTTES,
        MESAS,
        BADLANDS,
        PLATEAU_WIDE,
        CANYONS,
        CANYONS,
        LOW_CANYONS,
    ],
    &[
        HIGHLANDS,
        HIGHLANDS,
        HIGHLANDS,
        HIGHLANDS,
        ROLLING_HILLS,
        ROLLING_HILLS,
        BADLANDS,
        BADLANDS,
        STAIR_STEP_CANYONS,
        PLATEAU,
        PLATEAU,
        PLATEAU,
        PLATEAU_WIDE,
        OLD_MOUNTAINS,
        OLD_MOUNTAINS,
        OLD_MOUNTAINS,
        OLD_MOUNTAINS,
    ],
];
const ICE_SHEET_ALTITUDE_BIOMES: [&[i32]; 3] = [
    &[
        ICE_SHEET,
        ICE_SHEET,
        ICE_SHEET,
        ICE_SHEET,
        ICE_SHEET,
        ICE_SHEET_TUYAS,
    ],
    &[
        ICE_SHEET,
        ICE_SHEET,
        ICE_SHEET,
        ICE_SHEET,
        ICE_SHEET,
        ICE_SHEET_TUYAS,
        ICE_SHEET,
        ICE_SHEET_TUYAS,
    ],
    &[
        ICE_SHEET,
        ICE_SHEET,
        ICE_SHEET,
        ICE_SHEET,
        ICE_SHEET_TUYAS,
        ICE_SHEET_TUYAS,
        ICE_SHEET_MOUNTAINS,
        ICE_SHEET_MOUNTAINS,
    ],
];
const PALEO_ICE_SHEET_ALTITUDE_BIOMES: [&[i32]; 3] = [
    &[
        PATTERNED_GROUND,
        INVERTED_PATTERNED_GROUND,
        KNOB_AND_KETTLE,
        KNOB_AND_KETTLE,
        KNOB_AND_KETTLE,
        DRUMLINS,
        TUYAS,
        LOWLANDS,
        LOWLANDS,
    ],
    &[
        PATTERNED_GROUND,
        KNOB_AND_KETTLE,
        DRUMLINS,
        DRUMLINS,
        DRUMLINS,
        DRUMLINS,
        TUYAS,
        TUYAS,
    ],
    &[
        DRUMLINS,
        DRUMLINS,
        DRUMLINS,
        BADLANDS,
        BADLANDS,
        PLATEAU,
        PLATEAU,
        PLATEAU,
        PLATEAU_WIDE,
        ICE_SHEET_MOUNTAINS,
    ],
];
const DESERT_ALTITUDE_BIOMES: [&[i32]; 3] = [
    &[
        BUTTES,
        GRASSY_DUNES,
        DUNE_SEA,
        DUNE_SEA,
        DUNE_SEA,
        SALT_FLATS,
        SALT_FLATS,
    ],
    &[
        DUNE_SEA,
        BUTTES,
        BUTTES,
        HOODOOS,
        MESAS,
        MESAS,
        STAIR_STEP_CANYONS,
        BADLANDS,
        PLATEAU,
        CANYONS,
        WHORLED_CANYONS,
    ],
    &[
        HOODOOS,
        MESAS,
        STAIR_STEP_CANYONS,
        STAIR_STEP_CANYONS,
        ROCKY_PLATEAU,
        ROCKY_PLATEAU,
        OLD_MOUNTAINS,
        OLD_MOUNTAINS,
        OLD_MOUNTAINS,
        WHORLED_CANYONS,
    ],
];
const SEMI_ARID_ALTITUDE_BIOMES: [&[i32]; 3] = [
    &[
        PLAINS,
        HILLS,
        HILLS,
        GRASSY_DUNES,
        GRASSY_DUNES,
        GRASSY_DUNES,
        GRASSY_DUNES,
        LOW_CANYONS,
        LOWLANDS,
        LOWLANDS,
        MUD_FLATS,
    ],
    &[
        PLAINS,
        HILLS,
        ROLLING_HILLS,
        ROLLING_HILLS,
        HIGHLANDS,
        HIGHLANDS,
        BADLANDS,
        BADLANDS,
        PLATEAU_WIDE,
        CANYONS,
        LOW_CANYONS,
        WHORLED_CANYONS,
        BUTTES,
        MESAS,
        MESAS,
        HOODOOS,
    ],
    &[
        HIGHLANDS,
        HIGHLANDS,
        MESAS,
        HOODOOS,
        ROLLING_HILLS,
        ROLLING_HILLS,
        BADLANDS,
        BADLANDS,
        PLATEAU_WIDE,
        ROCKY_PLATEAU,
        STAIR_STEP_CANYONS,
        STAIR_STEP_CANYONS,
        OLD_MOUNTAINS,
        OLD_MOUNTAINS,
        WHORLED_CANYONS,
    ],
];
const KNOB_AND_KETTLE_BIOMES: &[i32] =
    &[KNOB_AND_KETTLE, PATTERNED_GROUND, INVERTED_PATTERNED_GROUND];
const ISLAND_BIOMES: &[i32] = &[
    ROLLING_HILLS,
    VOLCANIC_OCEANIC_MOUNTAINS,
    VOLCANIC_MOUNTAIN_ISLANDS,
    GUANO_ISLAND,
];
const RIFT_VALLEY_BIOMES: &[i32] = &[RIFT_VALLEY, RIFT_VALLEY, RIFT_VALLEY, RIFT_LAKE, RIFT_LAKE];
const VOLCANIC_ARC_BIOMES: &[i32] = &[
    VOLCANIC_OCEANIC_MOUNTAINS,
    VOLCANIC_OCEANIC_MOUNTAINS,
    VOLCANIC_OCEANIC_MOUNTAINS,
    VOLCANIC_ISLAND,
    OCEANIC_VOLCANIC_ARC,
];
const BARRIER_ISLAND_BIOMES: &[i32] = &[PLAINS, PLAINS, PLAINS, SALT_MARSH, TIDAL_FLATS];

/// `randomSeededFrom`.
fn random_seeded_from(rng_seed: i64, area_seed: i32, choices: &[i32]) -> i32 {
    choices[(rng_seed ^ i64::from(area_seed)).rem_euclid(choices.len() as i64) as usize]
}

/// The four-way glacial split used for every mountain flavour: the first threshold the
/// temperature falls under picks the biome.
fn glacial_tier(temp: f32, max_ice_sheet_temp: f32, thresholds: [f32; 3], ids: [i32; 4]) -> i32 {
    if temp < max_ice_sheet_temp + thresholds[0] {
        ids[0]
    } else if temp < max_ice_sheet_temp + thresholds[1] {
        ids[1]
    } else if temp < max_ice_sheet_temp + thresholds[2] {
        ids[2]
    } else {
        ids[3]
    }
}

fn hot_spot_biome(age: i8) -> i32 {
    match age {
        4 => ANCIENT_SHIELD_VOLCANO,
        3 => EXTINCT_SHIELD_VOLCANO,
        2 => DORMANT_SHIELD_VOLCANO,
        1 => ACTIVE_SHIELD_VOLCANO,
        _ => PLAINS,
    }
}

fn tower_karst_biome(biome: i32) -> i32 {
    if biome == SALT_MARSH {
        TOWER_KARST_BAY
    } else if biome == LOWLANDS {
        TOWER_KARST_LAKE
    } else if biome == PLAINS || biome == LOW_CANYONS {
        TOWER_KARST_PLAINS
    } else if biome == CANYONS {
        TOWER_KARST_CANYONS
    } else if biome == HILLS || biome == ROLLING_HILLS || biome == BADLANDS {
        TOWER_KARST_HILLS
    } else if biome == HIGHLANDS {
        TOWER_KARST_HIGHLANDS
    } else if biome == PLATEAU || biome == PLATEAU_WIDE {
        EXTREME_DOLINE_PLATEAU
    } else if biome == OLD_MOUNTAINS || biome == MOUNTAINS || biome == OCEANIC_MOUNTAINS {
        EXTREME_DOLINE_MOUNTAINS
    } else {
        biome
    }
}

fn shilin_biome(biome: i32) -> i32 {
    if biome == PLAINS {
        SHILIN_PLAINS
    } else if biome == CANYONS {
        SHILIN_CANYONS
    } else if biome == ROLLING_HILLS || biome == BADLANDS {
        SHILIN_HILLS
    } else if biome == PLATEAU || biome == PLATEAU_WIDE {
        SHILIN_PLATEAU
    } else if biome == HIGHLANDS {
        SHILIN_HIGHLANDS
    } else {
        biome
    }
}

fn burren_biome(biome: i32) -> i32 {
    if biome == PLAINS || biome == CANYONS {
        BURREN_PLAINS
    } else if biome == BADLANDS || biome == HILLS || biome == ROLLING_HILLS {
        BURREN_BADLANDS
    } else if biome == DRUMLINS {
        BURREN_ROCHE_MOUTONEE
    } else if biome == HIGHLANDS {
        BURREN_BADLANDS_TALL
    } else if biome == PLATEAU || biome == PLATEAU_WIDE {
        BURREN_PLATEAU
    } else {
        biome
    }
}

fn doline_biome(biome: i32) -> i32 {
    if biome == CANYONS {
        DOLINE_CANYONS
    } else if biome == PLAINS || biome == LOW_CANYONS {
        DOLINE_PLAINS
    } else if biome == HILLS {
        DOLINE_HILLS
    } else if biome == ROLLING_HILLS {
        DOLINE_ROLLING_HILLS
    } else if biome == HIGHLANDS {
        DOLINE_HIGHLANDS
    } else if biome == PLATEAU || biome == PLATEAU_WIDE {
        DOLINE_PLATEAU
    } else {
        biome
    }
}

fn cenote_biome(biome: i32) -> i32 {
    if biome == CANYONS {
        CENOTE_CANYONS
    } else if biome == PLAINS || biome == LOW_CANYONS {
        CENOTE_PLAINS
    } else if biome == HILLS {
        CENOTE_HILLS
    } else if biome == ROLLING_HILLS {
        CENOTE_ROLLING_HILLS
    } else if biome == HIGHLANDS {
        CENOTE_HIGHLANDS
    } else if biome == PLATEAU || biome == PLATEAU_WIDE {
        CENOTE_PLATEAU
    } else {
        biome
    }
}

fn mountain_biome(context_noise: f64, p: &Point) -> i32 {
    let temp = p.temperature;
    if p.divergence < 0.0 && f64::from(p.distance_to_edge) < 1.2 * context_noise - 5.2 {
        let max = -16f32 + 0.006f32 * p.rainfall;
        if p.distance_to_ocean < 3 {
            glacial_tier(
                temp,
                max,
                [2.0, 6.0, 10.0],
                [
                    ICE_SHEET_OCEANIC_MOUNTAINS,
                    GLACIATED_OCEANIC_MOUNTAINS,
                    GLACIALLY_CARVED_OCEANIC_MOUNTAINS,
                    OCEANIC_MOUNTAINS,
                ],
            )
        } else {
            glacial_tier(
                temp,
                max,
                [4.0, 8.0, 11.0],
                [
                    ICE_SHEET_MOUNTAINS,
                    GLACIATED_MOUNTAINS,
                    GLACIALLY_CARVED_MOUNTAINS,
                    COLLISIONAL_MOUNTAINS,
                ],
            )
        }
    } else if p.coastal_mountain() {
        let max = -16f32 + 0.006f32 * p.rainfall;
        if p.volcanic() {
            glacial_tier(
                temp,
                max,
                [2.0, 6.0, 10.0],
                [
                    ICE_SHEET_VOLCANIC_OCEANIC_MOUNTAINS,
                    GLACIATED_VOLCANIC_OCEANIC_MOUNTAINS,
                    GLACIALLY_CARVED_VOLCANIC_OCEANIC_MOUNTAINS,
                    VOLCANIC_OCEANIC_MOUNTAINS,
                ],
            )
        } else {
            glacial_tier(
                temp,
                max,
                [2.0, 6.0, 10.0],
                [
                    ICE_SHEET_OCEANIC_MOUNTAINS,
                    GLACIATED_OCEANIC_MOUNTAINS,
                    GLACIALLY_CARVED_OCEANIC_MOUNTAINS,
                    OCEANIC_MOUNTAINS,
                ],
            )
        }
    } else {
        let max = -14f32 + 0.006f32 * p.rainfall;
        if p.volcanic() {
            glacial_tier(
                temp,
                max,
                [4.0, 8.0, 11.0],
                [
                    ICE_SHEET_VOLCANIC_MOUNTAINS,
                    GLACIATED_VOLCANIC_MOUNTAINS,
                    GLACIALLY_CARVED_VOLCANIC_MOUNTAINS,
                    VOLCANIC_MOUNTAINS,
                ],
            )
        } else {
            glacial_tier(
                temp,
                max,
                [4.0, 8.0, 11.0],
                [
                    ICE_SHEET_MOUNTAINS,
                    GLACIATED_MOUNTAINS,
                    GLACIALLY_CARVED_MOUNTAINS,
                    MOUNTAINS,
                ],
            )
        }
    }
}

fn land_biome(rng_seed: i64, area_seed: i32, p: &Point) -> i32 {
    let rain = p.rainfall;
    let max_ice_sheet_temp = -17f32 + 0.006f32 * rain;
    let temp = p.temperature;
    let alt = p.discrete_biome_altitude() as usize;
    if temp < max_ice_sheet_temp {
        let biome = random_seeded_from(rng_seed, area_seed, ICE_SHEET_ALTITUDE_BIOMES[alt]);
        if p.distance_to_ocean < 3 && is_flat_ice_sheet(biome) {
            ICE_SHEET_OCEANIC
        } else {
            biome
        }
    } else if temp < max_ice_sheet_temp + 1.0 {
        ICE_SHEET_EDGE
    } else if f64::from(temp) < f64::from(max_ice_sheet_temp) + 2.5 {
        random_seeded_from(rng_seed, area_seed, KNOB_AND_KETTLE_BIOMES)
    } else if temp < max_ice_sheet_temp + 6.0 {
        random_seeded_from(rng_seed, area_seed, PALEO_ICE_SHEET_ALTITUDE_BIOMES[alt])
    } else if rain < 60.0 {
        random_seeded_from(rng_seed, area_seed, DESERT_ALTITUDE_BIOMES[alt])
    } else if rain < 155.0 {
        random_seeded_from(rng_seed, area_seed, SEMI_ARID_ALTITUDE_BIOMES[alt])
    } else {
        random_seeded_from(rng_seed, area_seed, ALTITUDE_BIOMES[alt])
    }
}

/// `world.region.ChooseBiomes`.
pub fn choose_biomes(context: &mut Context<'_>) {
    let generator = context.generator;
    let mut blob_area = generator.biome_area.lock().unwrap();
    let rng_seed = context.random.next_long();
    let climate_seed = context.random.next_long();
    for p in context.region.data.iter_mut().flatten() {
        let area_seed = blob_area.get(p.x, p.z);
        if p.island() {
            p.biome = random_seeded_from(rng_seed, area_seed, ISLAND_BIOMES);
        } else if p.distance_to_edge < 3 && p.divergence > 0.0 && p.land() {
            p.biome = if p.distance_to_ocean > 2 {
                random_seeded_from(rng_seed, area_seed, RIFT_VALLEY_BIOMES)
            } else {
                RIFT_VALLEY
            };
        } else if p.river()
            && p.distance_to_edge > 4
            && p.temperature > -16f32 + 0.006f32 * p.rainfall
        {
            p.biome = RIVER_VALLEY;
        } else if p.mountain() {
            let noise = generator
                .continent_noise
                .noise(f64::from(p.x), f64::from(p.z));
            p.biome = mountain_biome(noise, p);
        } else if p.land() {
            p.biome = land_biome(rng_seed, area_seed, p);
        } else if p.barrier_island() {
            p.biome = if p.volcanic() {
                random_seeded_from(rng_seed, area_seed, VOLCANIC_ARC_BIOMES)
            } else {
                random_seeded_from(rng_seed, area_seed, BARRIER_ISLAND_BIOMES)
            };
        } else if p.ocean_depth == 1 {
            p.biome = if p.volcanic() {
                OCEANIC_VOLCANIC_ARC
            } else {
                OCEAN_REEF
            };
        } else if p.ocean_depth == 2 {
            p.biome = if p.temperature > 12.0 && p.distance_to_land > 4 {
                OCEAN_ATOLLS
            } else {
                OCEAN
            };
        } else if p.ocean_depth == 3 {
            p.biome = OCEAN_RIDGE;
        } else if p.ocean_depth == 5 {
            p.biome = DEEP_OCEAN_TRENCH;
        } else {
            p.biome = if p.temperature > 12.0 && p.distance_to_land > 3 {
                DEEP_OCEAN_ATOLLS
            } else {
                DEEP_OCEAN
            };
        }

        let age = p.hot_spot_age;
        if age > 0 {
            if age == 4
                && (p.biome == OCEAN
                    || p.biome == DEEP_OCEAN
                    || p.biome == OCEAN_REEF
                    || p.biome == DEEP_OCEAN_TRENCH)
            {
                p.biome = SUNKEN_SHIELD_VOLCANO;
            } else {
                p.biome = hot_spot_biome(age);
            }
        }

        let mixed = (i64::from(area_seed) ^ climate_seed).rem_euclid(40) as i32;
        let min_rain_for_low_fresh_water_biomes = 90f32 + mixed as f32;
        let rainfall = p.rainfall;
        let temperature = p.temperature;
        if rainfall < min_rain_for_low_fresh_water_biomes && rainfall <= 55.0 {
            if p.biome == LOWLANDS || p.biome == LOW_CANYONS {
                p.biome = SALT_FLATS;
            } else if p.biome == HILLS || p.biome == ROLLING_HILLS || p.biome == PLATEAU {
                p.biome = DUNE_SEA;
            }
        }
        if rainfall < 145.0 && (p.biome == PATTERNED_GROUND || p.biome == INVERTED_PATTERNED_GROUND)
        {
            p.biome = STONE_CIRCLES;
        }
        let max_rainfall_for_badlands = 420f32 + mixed as f32;
        if rainfall > max_rainfall_for_badlands && p.biome == BADLANDS {
            p.biome = HIGHLANDS;
        }
        let max_ice_sheet_temp = -14f32 + 0.006f32 * rainfall;
        let shield = p.biome == ACTIVE_SHIELD_VOLCANO
            || p.biome == DORMANT_SHIELD_VOLCANO
            || p.biome == EXTINCT_SHIELD_VOLCANO;
        if p.land() && temperature < max_ice_sheet_temp {
            if shield {
                p.biome = ICE_SHEET_SHIELD_VOLCANO;
            }
        } else if temperature < max_ice_sheet_temp + 4.0 && shield {
            p.biome = GLACIATED_SHIELD_VOLCANO;
        }

        if p.is_surface_rock_karst {
            if rainfall > 375.0 {
                p.biome = if rainfall > 425.0 && rainfall + 10.0 * temperature > 500.0 {
                    tower_karst_biome(p.biome)
                } else if temperature > 9.0 {
                    shilin_biome(p.biome)
                } else if temperature < 0.0 {
                    burren_biome(p.biome)
                } else {
                    doline_biome(p.biome)
                };
            } else if rainfall > 250.0 {
                p.biome = if temperature > 5.0 {
                    cenote_biome(p.biome)
                } else {
                    doline_biome(p.biome)
                };
            }
        }
        if p.distance_to_ocean <= 2
            && p.biome == LOWLANDS
            && p.rainfall > 220.0
            && p.temperature > 18.0
        {
            p.biome = SALT_MARSH;
        }
        if p.lake() && has_lake(p.biome) {
            p.biome = lake_for(p.biome);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::tfc_region::{Region, RegionGenerator, Seed, Settings};
    use horizon_core::random::XoroshiroRandom;

    #[test]
    fn classification_helpers_match_java() {
        let fns: [fn(i32) -> i64; 10] = [
            |v| i64::from(has_shore(v)),
            shore_for_i64,
            |v| i64::from(has_lake(v)),
            lake_for_i64,
            |v| i64::from(is_ocean(v)),
            |v| i64::from(is_mountains(v)),
            |v| i64::from(is_low(v)),
            |v| i64::from(is_flats(v)),
            |v| i64::from(is_flat_ice_sheet(v)),
            |v| i64::from(is_lake(v)),
        ];
        let want: [i64; 10] = [
            -6761168514477125474,
            934398707491135915,
            9183372662370121630,
            3843288238814350150,
            -8628208862537023616,
            4739245425957236034,
            -3580494365370147327,
            -3017510808341599264,
            -1307104644591266881,
            -4676618124029925501,
        ];
        for (f, w) in fns.iter().zip(want) {
            let h = (0..125).fold(0i64, |h, v| h.wrapping_mul(31).wrapping_add(f(v)));
            assert_eq!(h, w);
        }
    }

    fn shore_for_i64(v: i32) -> i64 {
        i64::from(shore_for(v))
    }

    fn lake_for_i64(v: i32) -> i64 {
        i64::from(lake_for(v))
    }

    #[allow(clippy::type_complexity)]
    const BIOMES: &[(bool, i64, i32, i32, i64, i64, u64)] = &[
        (
            false,
            1234567,
            0,
            0,
            -8811967129691468793,
            16471610,
            0x7774aa9f9e4ce014,
        ),
        (
            false,
            1234567,
            500,
            -700,
            5851951211250839618,
            32066330,
            0xb742388588a81377,
        ),
        (
            false,
            1234567,
            -1500,
            2000,
            -4817035621370684258,
            11227372,
            0xa6b750abde2b7d40,
        ),
        (
            false,
            1234567,
            3000,
            3100,
            -5537939675467869207,
            23129664,
            0x4629c932f8a7bdce,
        ),
        (
            false,
            1234567,
            -4000,
            -2500,
            -7844622685145130366,
            9555242,
            0x7ccbfbcd07e5a22e,
        ),
        (
            false,
            1234567,
            7000,
            900,
            -7003595965382022169,
            12466460,
            0x71a44cd277d1b00e,
        ),
        (
            true,
            424242,
            0,
            0,
            2653235007299634052,
            9797830,
            0x87deff969d25873f,
        ),
        (
            true,
            424242,
            500,
            -700,
            1254744873920467960,
            295892,
            0x6cd117896aea81e6,
        ),
        (
            true,
            424242,
            -1500,
            2000,
            -4679406717536197081,
            387062,
            0xe4911d5d05204214,
        ),
        (
            true,
            424242,
            3000,
            3100,
            -2851869595608791648,
            310852,
            0x1ffc21aeb9ef8732,
        ),
        (
            true,
            424242,
            -4000,
            -2500,
            -8454163333778828652,
            341228,
            0x7dc81d9e6db85c4,
        ),
        (
            true,
            424242,
            7000,
            900,
            -4771139095254845152,
            269748,
            0x250f20595763f3b0,
        ),
    ];

    #[test]
    fn choose_biomes_matches_java_bit_for_bit() {
        for (finite, seed, gx, gz, want, checksum, rnd) in BIOMES {
            let settings = Settings {
                finite_continents: *finite,
                ..Settings::default()
            };
            let g = RegionGenerator::new(settings, Seed::of(*seed));
            let cell = g.sample_cell(*gx, *gz);
            let mut ctx = Context {
                generator: &g,
                random: XoroshiroRandom::new(
                    *seed
                        ^ i64::from((cell.noise as f32).to_bits() as i32)
                            .wrapping_mul(7_189_234_123),
                ),
                region: Region::new(&cell),
                region_cell: cell,
            };
            for task in crate::worldgen::tfc_region::TASKS {
                task(&mut ctx);
            }
            let (mut h, mut c) = (0i64, 0i64);
            let mut hist = [0i64; 125];
            for p in ctx.region.points() {
                h = h.wrapping_mul(31).wrapping_add(i64::from(p.biome));
                hist[p.biome as usize] += 1;
            }
            for (i, n) in hist.iter().enumerate() {
                c += i as i64 * n * (i as i64 + 7);
            }
            assert_eq!(h, *want, "biomes at {gx},{gz}");
            assert_eq!(c, *checksum, "histogram at {gx},{gz}");
            assert_eq!(
                ctx.random.next_long() as u64,
                *rnd,
                "random stream at {gx},{gz}"
            );
        }
    }
}
