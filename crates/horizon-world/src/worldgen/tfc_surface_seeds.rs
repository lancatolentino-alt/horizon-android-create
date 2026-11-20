//! The seeds TFC's `SurfaceManager` hands each biome's surface builder (EUPL 1.2).
//!
//! `collectSurfaceBuilders` walks `TFCBiomes.REGISTRY` in registration order and builds each
//! biome's builder from one shared `Seed` stream: lowlands and grassy dunes draw one `next()`,
//! badlands one `fork()` (two), and every biome's atoll wrapper and tuya wrapper draw one each.
//! Before it, the chunk generator has already drawn the region generator's seeds, the biome
//! layer's eleven and the noise sampler's one. So each biome has its own noise instance, with
//! its own seed: that is what this reproduces.
//!
//! Columns of river biomes get a builder TFC makes while generating, from whatever the stream is
//! up to by then; the biome the river replaced stands in for them here.

use super::tfc_biome_ext::{BIOME_COUNT, extensions};
use super::tfc_region::Seed;

/// Seeds drawn from the level's stream before `new SurfaceManager(seed)`.
pub const PRE_SURFACE_DRAWS: usize = 34;

/// `TFCBiomes.REGISTRY` in registration order.
const REGISTRY_ORDER: [&str; BIOME_COUNT] = [
    "ocean",
    "ocean_reef",
    "deep_ocean",
    "deep_ocean_trench",
    "oceanic_volcanic_arc",
    "ocean_atolls",
    "deep_ocean_atolls",
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
    "canyons",
    "mountains",
    "old_mountains",
    "oceanic_mountains",
    "volcanic_mountains",
    "volcanic_oceanic_mountains",
    "volcanic_mountain_islands",
    "guano_island",
    "volcanic_island",
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
    "old_mountain_lake",
    "oceanic_mountain_lake",
    "volcanic_mountain_lake",
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
    "tower_karst_hills",
    "tower_karst_canyons",
    "tower_karst_highlands",
    "tower_karst_lake",
    "tower_karst_bay",
    "burren_plateau",
    "burren_badlands",
    "burren_badlands_tall",
    "burren_plains",
    "burren_roche_moutonee",
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
    "ice_sheet_mountains_edge",
    "ice_sheet_oceanic_mountains_edge",
    "meltwater_lake",
    "ice_sheet_oceanic",
    "ice_sheet_shore",
    "glaciated_mountains",
    "glaciated_oceanic_mountains",
    "glaciated_volcanic_mountains",
    "glaciated_volcanic_oceanic_mountains",
    "glaciated_shield_volcano",
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

/// What the biome's own builder (before the volcano wrappers) drew.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    None,
    Lowlands(i64),
    GrassyDunes(i64),
    /// The two longs of the `fork()`ed `RandomSource`.
    Badlands(i64, i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BiomeSeeds {
    pub base: Base,
    /// `AtollSurfaceBuilder.heightNoise`'s seed.
    pub atoll: i64,
}

/// The seeds of every biome's surface builder, indexed by the biome's id.
pub fn compute(level_seed: i64) -> Vec<BiomeSeeds> {
    let mut seed = Seed::of(level_seed);
    for _ in 0..PRE_SURFACE_DRAWS {
        seed.next();
    }
    let ext = extensions();
    let mut out = vec![
        BiomeSeeds {
            base: Base::None,
            atoll: 0,
        };
        BIOME_COUNT
    ];
    for name in REGISTRY_ORDER {
        let id = ext
            .iter()
            .position(|e| e.name == name)
            .expect("registered biome has an extension");
        let surface = ext[id].surface;
        let base = if surface.starts_with("LowlandsSurfaceBuilder") {
            Base::Lowlands(seed.next())
        } else if surface.starts_with("GrassyDunesSurfaceBuilder") {
            Base::GrassyDunes(seed.next())
        } else if surface.starts_with("BadlandsSurfaceBuilder") {
            Base::Badlands(seed.next(), seed.next())
        } else {
            Base::None
        };
        // Stratovolcano, atoll, tuff ring, tuya, cinder cone: only the atoll and tuya draw.
        let atoll = seed.next();
        seed.next();
        out[id] = BiomeSeeds { base, atoll };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::tfc_biome_layers::create_region_biome_layer;
    use super::super::tfc_region::{RegionGenerator, Settings};
    use super::*;
    use std::sync::Arc;

    /// What `TFCChunkGenerator.initRandomState` draws before the `SurfaceManager`, counted by
    /// running the real region generator and biome layer on a stream and comparing positions.
    fn real_draws(level_seed: i64, settings: Settings) -> usize {
        let mut seed = Seed::of(level_seed);
        let generator = Arc::new(RegionGenerator::from_seed(settings, &mut seed));
        let _layer = create_region_biome_layer(generator, &mut seed);
        seed.next(); // `new NoiseSampler(seed.next(), ..)`
        let after = seed.next();
        let mut probe = Seed::of(level_seed);
        for n in 0..200 {
            if probe.next() == after {
                return n;
            }
        }
        panic!("stream position not found");
    }

    #[test]
    fn the_prefix_matches_what_the_chunk_generator_draws() {
        for level_seed in [1234567, 424242, -99] {
            for finite_continents in [false, true] {
                let settings = Settings {
                    finite_continents,
                    ..Settings::default()
                };
                assert_eq!(
                    real_draws(level_seed, settings),
                    PRE_SURFACE_DRAWS,
                    "seed {level_seed}"
                );
            }
        }
    }

    #[test]
    fn every_biome_gets_its_own_builder_seeds() {
        let seeds = compute(1234567);
        let id = |name: &str| {
            extensions()
                .iter()
                .position(|e| e.name == name)
                .expect("biome")
        };
        let (lowlands, marsh) = (id("lowlands"), id("salt_marsh"));
        let (Base::Lowlands(a), Base::Lowlands(b)) = (seeds[lowlands].base, seeds[marsh].base)
        else {
            panic!("lowlands biomes draw a seed");
        };
        assert_ne!(a, b);
        let badlands = [
            "badlands",
            "whorled_canyons",
            "stair_step_canyons",
            "mesas",
            "buttes",
            "hoodoos",
        ];
        let forks: Vec<_> = badlands
            .iter()
            .map(|n| match seeds[id(n)].base {
                Base::Badlands(lo, hi) => (lo, hi),
                other => panic!("{n} is {other:?}"),
            })
            .collect();
        for (i, f) in forks.iter().enumerate() {
            assert!(forks[i + 1..].iter().all(|g| g != f));
        }
        assert!(matches!(
            seeds[id("grassy_dunes")].base,
            Base::GrassyDunes(_)
        ));
        // The stream is Java's: the first biome in registration order draws right after the
        // prefix, so its atoll seed is the 35th (or later) value of the level seed's stream.
        let mut probe = Seed::of(1234567);
        let all: Vec<i64> = (0..PRE_SURFACE_DRAWS + 400).map(|_| probe.next()).collect();
        assert!(all[PRE_SURFACE_DRAWS..].contains(&seeds[lowlands].atoll));
    }
}
