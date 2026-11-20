//! TerraFirmaCraft's forest types (`world.chunkdata.ForestType`) and the forest area layer
//! (`TFCLayers.createOverworldForestLayer` with its four forest layers; EUPL 1.2).

use super::tfc_layers::{
    AreaFactory, Zoom, adjacent_layer, source_layer, transform_layer, zoom_layer,
};
use super::tfc_noise::{Noise2D, OpenSimplex2D};
use horizon_core::random::{LegacyRandom, RandomSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubType {
    None,
    Primary,
    Secondary,
    Edge,
    Dead,
    Savanna,
}

/// The `IntProvider`s a forest type uses: `ConstantInt` draws nothing, `UniformInt` one `nextInt`.
#[derive(Clone, Copy, Debug)]
pub enum IntProvider {
    Constant(i32),
    Uniform(i32, i32),
}

impl IntProvider {
    pub fn sample(self, random: &mut impl RandomSource) -> i32 {
        match self {
            Self::Constant(v) => v,
            Self::Uniform(min, max) => min + random.next_int_bound(max - min + 1).unwrap(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ForestTypeData {
    pub sub_type: SubType,
    pub density: i32,
    pub tree_count: IntProvider,
    pub groundcover_count: IntProvider,
    pub leaf_pile_count: IntProvider,
    pub bush_count: IntProvider,
    pub max_tree_types: i32,
    pub per_chunk_chance: f32,
    pub alternate_size: i32,
}

/// `ForestType`, in declaration order (the ordinal is what the layers store).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ForestType {
    Grassland,
    Clearing,
    Shrubland,
    Sparse,
    SavannaMonoculture,
    SavannaDiverse,
    SavannaAlternate,
    SavannaShrubMonoculture,
    SavannaShrubDiverse,
    SavannaShrubAlternate,
    PrimaryMonoculture,
    PrimaryDiverse,
    PrimaryAlternate,
    SecondaryMonoculture,
    SecondaryMonocultureTall,
    SecondaryDiverse,
    SecondaryBamboo,
    SecondaryDiverseTall,
    SecondaryDense,
    SecondaryDenseTall,
    SecondaryAlternate,
    EdgeMonoculture,
    EdgeDiverse,
    EdgeAlternate,
    EdgeBamboo,
    DeadMonoculture,
    DeadDiverse,
    DeadAlternate,
    DeadBamboo,
}

pub const FOREST_NAMES: [&str; 29] = [
    "grassland",
    "clearing",
    "shrubland",
    "sparse",
    "savanna_monoculture",
    "savanna_diverse",
    "savanna_alternate",
    "savanna_shrub_monoculture",
    "savanna_shrub_diverse",
    "savanna_shrub_alternate",
    "primary_monoculture",
    "primary_diverse",
    "primary_alternate",
    "secondary_monoculture",
    "secondary_monoculture_tall",
    "secondary_diverse",
    "secondary_bamboo",
    "secondary_diverse_tall",
    "secondary_dense",
    "secondary_dense_tall",
    "secondary_alternate",
    "edge_monoculture",
    "edge_diverse",
    "edge_alternate",
    "edge_bamboo",
    "dead_monoculture",
    "dead_diverse",
    "dead_alternate",
    "dead_bamboo",
];

const DATA: [ForestTypeData; 29] = [
    ForestTypeData {
        sub_type: SubType::None,
        density: 0,
        tree_count: IntProvider::Constant(0),
        groundcover_count: IntProvider::Constant(0),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Constant(0),
        max_tree_types: 2,
        per_chunk_chance: 0f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::None,
        density: 0,
        tree_count: IntProvider::Constant(0),
        groundcover_count: IntProvider::Constant(0),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Constant(0),
        max_tree_types: 2,
        per_chunk_chance: 0f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::None,
        density: 0,
        tree_count: IntProvider::Constant(0),
        groundcover_count: IntProvider::Constant(10),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Uniform(2, 7),
        max_tree_types: 2,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::None,
        density: 0,
        tree_count: IntProvider::Constant(2),
        groundcover_count: IntProvider::Constant(6),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(0, 2),
        max_tree_types: 2,
        per_chunk_chance: 0.08f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Savanna,
        density: 1,
        tree_count: IntProvider::Constant(3),
        groundcover_count: IntProvider::Constant(6),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(0, 2),
        max_tree_types: 1,
        per_chunk_chance: 0.55f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Savanna,
        density: 1,
        tree_count: IntProvider::Constant(3),
        groundcover_count: IntProvider::Constant(6),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(0, 2),
        max_tree_types: 2,
        per_chunk_chance: 0.65f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Savanna,
        density: 1,
        tree_count: IntProvider::Constant(3),
        groundcover_count: IntProvider::Constant(6),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(0, 2),
        max_tree_types: 3,
        per_chunk_chance: 0.40f32,
        alternate_size: 2,
    },
    ForestTypeData {
        sub_type: SubType::Savanna,
        density: 1,
        tree_count: IntProvider::Constant(1),
        groundcover_count: IntProvider::Constant(6),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(3, 6),
        max_tree_types: 1,
        per_chunk_chance: 0.9f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Savanna,
        density: 1,
        tree_count: IntProvider::Constant(1),
        groundcover_count: IntProvider::Constant(6),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(3, 6),
        max_tree_types: 2,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Savanna,
        density: 1,
        tree_count: IntProvider::Constant(1),
        groundcover_count: IntProvider::Constant(6),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(3, 6),
        max_tree_types: 3,
        per_chunk_chance: 0.8f32,
        alternate_size: 2,
    },
    ForestTypeData {
        sub_type: SubType::Primary,
        density: 3,
        tree_count: IntProvider::Constant(5),
        groundcover_count: IntProvider::Constant(25),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Constant(0),
        max_tree_types: 1,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Primary,
        density: 4,
        tree_count: IntProvider::Constant(7),
        groundcover_count: IntProvider::Constant(40),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Uniform(0, 3),
        max_tree_types: 2,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Primary,
        density: 4,
        tree_count: IntProvider::Constant(7),
        groundcover_count: IntProvider::Constant(40),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Uniform(0, 3),
        max_tree_types: 3,
        per_chunk_chance: 1f32,
        alternate_size: 2,
    },
    ForestTypeData {
        sub_type: SubType::Secondary,
        density: 3,
        tree_count: IntProvider::Constant(5),
        groundcover_count: IntProvider::Constant(25),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(1, 2),
        max_tree_types: 1,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Secondary,
        density: 3,
        tree_count: IntProvider::Constant(5),
        groundcover_count: IntProvider::Constant(25),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(1, 2),
        max_tree_types: 1,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Secondary,
        density: 3,
        tree_count: IntProvider::Constant(5),
        groundcover_count: IntProvider::Constant(25),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(1, 2),
        max_tree_types: 2,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Secondary,
        density: 3,
        tree_count: IntProvider::Constant(1),
        groundcover_count: IntProvider::Constant(25),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Uniform(0, 1),
        max_tree_types: 2,
        per_chunk_chance: 0.3f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Secondary,
        density: 3,
        tree_count: IntProvider::Constant(5),
        groundcover_count: IntProvider::Constant(25),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(1, 2),
        max_tree_types: 2,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Secondary,
        density: 4,
        tree_count: IntProvider::Constant(7),
        groundcover_count: IntProvider::Constant(40),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Constant(3),
        max_tree_types: 2,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Secondary,
        density: 4,
        tree_count: IntProvider::Constant(7),
        groundcover_count: IntProvider::Constant(40),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Constant(3),
        max_tree_types: 2,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Secondary,
        density: 3,
        tree_count: IntProvider::Constant(5),
        groundcover_count: IntProvider::Constant(25),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(1, 2),
        max_tree_types: 3,
        per_chunk_chance: 1f32,
        alternate_size: 2,
    },
    ForestTypeData {
        sub_type: SubType::Edge,
        density: 2,
        tree_count: IntProvider::Constant(2),
        groundcover_count: IntProvider::Constant(10),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Uniform(0, 1),
        max_tree_types: 1,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Edge,
        density: 2,
        tree_count: IntProvider::Constant(2),
        groundcover_count: IntProvider::Constant(10),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Uniform(0, 1),
        max_tree_types: 2,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Edge,
        density: 2,
        tree_count: IntProvider::Constant(2),
        groundcover_count: IntProvider::Constant(10),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Uniform(0, 1),
        max_tree_types: 3,
        per_chunk_chance: 1f32,
        alternate_size: 2,
    },
    ForestTypeData {
        sub_type: SubType::Edge,
        density: 2,
        tree_count: IntProvider::Constant(1),
        groundcover_count: IntProvider::Constant(10),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Uniform(0, 1),
        max_tree_types: 1,
        per_chunk_chance: 0.7f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Dead,
        density: 2,
        tree_count: IntProvider::Constant(5),
        groundcover_count: IntProvider::Constant(25),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(2, 4),
        max_tree_types: 1,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Dead,
        density: 2,
        tree_count: IntProvider::Constant(5),
        groundcover_count: IntProvider::Constant(25),
        leaf_pile_count: IntProvider::Constant(0),
        bush_count: IntProvider::Uniform(2, 4),
        max_tree_types: 2,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
    ForestTypeData {
        sub_type: SubType::Dead,
        density: 3,
        tree_count: IntProvider::Constant(4),
        groundcover_count: IntProvider::Constant(40),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Uniform(0, 3),
        max_tree_types: 3,
        per_chunk_chance: 1f32,
        alternate_size: 2,
    },
    ForestTypeData {
        sub_type: SubType::Dead,
        density: 3,
        tree_count: IntProvider::Constant(4),
        groundcover_count: IntProvider::Constant(25),
        leaf_pile_count: IntProvider::Uniform(0, 1),
        bush_count: IntProvider::Uniform(2, 4),
        max_tree_types: 2,
        per_chunk_chance: 1f32,
        alternate_size: 0,
    },
];

const ALL: [ForestType; 29] = [
    ForestType::Grassland,
    ForestType::Clearing,
    ForestType::Shrubland,
    ForestType::Sparse,
    ForestType::SavannaMonoculture,
    ForestType::SavannaDiverse,
    ForestType::SavannaAlternate,
    ForestType::SavannaShrubMonoculture,
    ForestType::SavannaShrubDiverse,
    ForestType::SavannaShrubAlternate,
    ForestType::PrimaryMonoculture,
    ForestType::PrimaryDiverse,
    ForestType::PrimaryAlternate,
    ForestType::SecondaryMonoculture,
    ForestType::SecondaryMonocultureTall,
    ForestType::SecondaryDiverse,
    ForestType::SecondaryBamboo,
    ForestType::SecondaryDiverseTall,
    ForestType::SecondaryDense,
    ForestType::SecondaryDenseTall,
    ForestType::SecondaryAlternate,
    ForestType::EdgeMonoculture,
    ForestType::EdgeDiverse,
    ForestType::EdgeAlternate,
    ForestType::EdgeBamboo,
    ForestType::DeadMonoculture,
    ForestType::DeadDiverse,
    ForestType::DeadAlternate,
    ForestType::DeadBamboo,
];

use ForestType::*;

const EDGE_DENSITY: [ForestType; 4] = [EdgeMonoculture, EdgeDiverse, EdgeAlternate, EdgeBamboo];
const SECONDARY_FORESTS: [ForestType; 7] = [
    SecondaryMonoculture,
    SecondaryDense,
    SecondaryDiverse,
    SecondaryDenseTall,
    SecondaryMonocultureTall,
    SecondaryDiverseTall,
    SecondaryBamboo,
];
const PRIMARY_FORESTS: [ForestType; 2] = [PrimaryDiverse, PrimaryMonoculture];
const DEAD_FORESTS: [ForestType; 4] = [DeadMonoculture, DeadDiverse, DeadAlternate, DeadBamboo];
const SAVANNA_FORESTS: [ForestType; 6] = [
    SavannaMonoculture,
    SavannaAlternate,
    SavannaDiverse,
    SavannaShrubMonoculture,
    SavannaShrubAlternate,
    SavannaShrubDiverse,
];

fn pick(random: &mut impl RandomSource, list: &[ForestType]) -> i32 {
    list[random.next_int_bound(list.len() as i32).unwrap() as usize] as i32
}

impl ForestType {
    pub fn get_edge_forest_type(random: &mut impl RandomSource) -> i32 {
        pick(random, &EDGE_DENSITY)
    }
    pub fn get_secondary_forest_type(random: &mut impl RandomSource) -> i32 {
        pick(random, &SECONDARY_FORESTS)
    }
    pub fn get_primary_forest_type(random: &mut impl RandomSource) -> i32 {
        pick(random, &PRIMARY_FORESTS)
    }
    pub fn get_dead_forest_type(random: &mut impl RandomSource) -> i32 {
        pick(random, &DEAD_FORESTS)
    }
    pub fn get_savanna_forest_type(random: &mut impl RandomSource) -> i32 {
        pick(random, &SAVANNA_FORESTS)
    }

    /// `ForestType.valueOf(int)`: out-of-range ordinals read as grassland.
    pub fn from_ordinal(i: i32) -> Self {
        if i >= 0 && (i as usize) < ALL.len() {
            ALL[i as usize]
        } else {
            Grassland
        }
    }

    pub fn data(self) -> &'static ForestTypeData {
        &DATA[self as usize]
    }
    pub fn name(self) -> &'static str {
        FOREST_NAMES[self as usize]
    }
    pub fn is_primary(self) -> bool {
        self.data().sub_type == SubType::Primary
    }
    pub fn is_edge(self) -> bool {
        self.data().sub_type == SubType::Edge
    }
    pub fn is_secondary(self) -> bool {
        self.data().sub_type == SubType::Secondary
    }
    pub fn is_dead(self) -> bool {
        self.data().sub_type == SubType::Dead
    }
    pub fn is_none(self) -> bool {
        self.data().sub_type == SubType::None
    }
    pub fn is_savanna(self) -> bool {
        self.data().sub_type == SubType::Savanna
    }
    pub fn density(self) -> i32 {
        self.data().density
    }
    pub fn is_as_or_more_dense_as(self, other: Self) -> bool {
        self.density() >= other.density()
    }

    pub fn alternate(self) -> Self {
        if self.is_savanna() {
            self
        } else if self.is_edge() {
            EdgeAlternate
        } else if self.is_secondary() {
            SecondaryAlternate
        } else if self.is_primary() {
            PrimaryAlternate
        } else if self.is_dead() {
            DeadAlternate
        } else {
            self
        }
    }
}

/// `ForestRandomizeLayer`.
fn forest_randomize(random: &mut impl RandomSource, value: i32) -> i32 {
    let current = ForestType::from_ordinal(value);
    if current.is_none() {
        let r = random.next_int_bound(20).unwrap();
        if r <= 2 {
            return Shrubland as i32;
        } else if r <= 5 {
            return ForestType::get_savanna_forest_type(random);
        } else if r == 6 {
            return ForestType::get_secondary_forest_type(random);
        }
    } else if current.is_savanna() {
        let r = random.next_int_bound(16).unwrap();
        if r <= 2 {
            return ForestType::get_secondary_forest_type(random);
        } else if r <= 6 {
            return Sparse as i32;
        } else if r <= 8 {
            return Shrubland as i32;
        }
    } else if current.is_primary() || current.is_secondary() {
        let r = random.next_int_bound(24).unwrap();
        if r == 0 {
            return SecondaryBamboo as i32;
        }
        if r == 1 && !current.is_secondary() {
            return ForestType::get_savanna_forest_type(random);
        } else if r == 2 {
            return Shrubland as i32;
        } else if r <= 6 {
            return ForestType::get_primary_forest_type(random);
        }
    }
    value
}

/// `ForestRandomizeSmallLayer`.
fn forest_randomize_small(random: &mut impl RandomSource, value: i32) -> i32 {
    let current = ForestType::from_ordinal(value);
    if current.is_primary() || current.is_secondary() {
        let r = random
            .next_int_bound(if current.is_secondary() { 40 } else { 25 })
            .unwrap();
        if r == 0 {
            return Clearing as i32;
        } else if r == 1 {
            return Sparse as i32;
        } else if r == 3 {
            return ForestType::get_dead_forest_type(random);
        }
    } else if current.is_savanna() || current.is_none() {
        let r = random.next_int_bound(30).unwrap();
        if r == 0 {
            return if current.is_savanna() {
                ForestType::get_secondary_forest_type(random)
            } else {
                ForestType::get_edge_forest_type(random)
            };
        }
    }
    if random.next_int_bound(10).unwrap() == 0 {
        return ForestType::from_ordinal(value).alternate() as i32;
    }
    value
}

fn is_full_forest(value: i32) -> bool {
    let t = ForestType::from_ordinal(value);
    t.is_primary() || t.is_secondary()
}

/// `TFCLayers.createOverworldForestLayer`: forest type ordinals. `seed` is the raw value
/// `RegionChunkDataGenerator` drew last (`RegionGenerator::chunk_data_seeds[4]`).
pub fn create_overworld_forest_layer(seed: i64) -> AreaFactory {
    let mut random = LegacyRandom::new(seed);
    let base_noise = Noise2D::from_simplex(
        OpenSimplex2D::from_int(random.next_int()).spread(f64::from(0.25f32)),
    );
    let mut layer = source_layer(random.next_long(), move |_, x, z| {
        let noise = base_noise.noise(f64::from(x), f64::from(z)) as f32;
        if noise < 0.0 {
            Grassland as i32
        } else {
            SecondaryDiverse as i32
        }
    });
    layer = transform_layer(random.next_long(), layer, |c, area, x, z| {
        forest_randomize(&mut c.random, area.get(x, z))
    });
    layer = zoom_layer(Zoom::Fuzzy, random.next_long(), layer);
    layer = transform_layer(random.next_long(), layer, |c, area, x, z| {
        forest_randomize(&mut c.random, area.get(x, z))
    });
    layer = zoom_layer(Zoom::Fuzzy, random.next_long(), layer);
    layer = zoom_layer(Zoom::Normal, random.next_long(), layer);
    layer = adjacent_layer(random.next_long(), layer, |c, n, e, s, w, center| {
        if is_full_forest(center)
            && (!is_full_forest(n)
                || !is_full_forest(e)
                || !is_full_forest(s)
                || !is_full_forest(w))
        {
            ForestType::get_edge_forest_type(&mut c.random)
        } else {
            center
        }
    });
    layer = transform_layer(random.next_long(), layer, |c, area, x, z| {
        forest_randomize_small(&mut c.random, area.get(x, z))
    });
    for _ in 0..2 {
        layer = zoom_layer(Zoom::Normal, random.next_long(), layer);
    }
    layer
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOREST: &[(i64, i32, i32, i64)] = &[
        (123456789, 0, 0, 7891415695368038410),
        (123456789, 1000, -2000, -2024148670310098245),
        (123456789, -513, 777, 712411987877959032),
        (-987654321, 0, 0, -291499520762516397),
        (-987654321, 1000, -2000, -559686006800912549),
        (-987654321, -513, 777, 2165024311406807352),
        (424242, 0, 0, -9042188710410381182),
        (424242, 1000, -2000, 1293776727163164017),
        (424242, -513, 777, -6083617699948962136),
        (77, 0, 0, 402931611858298671),
        (77, 1000, -2000, -8567142589331196007),
        (77, -513, 777, -5251969090831906461),
    ];

    #[test]
    fn forest_layer_matches_java_bit_for_bit() {
        let mut seeds: Vec<i64> = FOREST.iter().map(|r| r.0).collect();
        seeds.dedup();
        for seed in seeds {
            let mut area = create_overworld_forest_layer(seed)();
            for (_, ox, oz, want) in FOREST.iter().filter(|r| r.0 == seed) {
                let mut h = 0i64;
                for z in 0..48 {
                    for x in 0..48 {
                        h = h
                            .wrapping_mul(31)
                            .wrapping_add(i64::from(area.get(ox + x * 5, oz + z * 5)));
                    }
                }
                assert_eq!(h, *want, "forest layer, seed {seed} at {ox},{oz}");
            }
        }
    }

    #[test]
    fn forest_type_table_is_consistent() {
        assert_eq!(ForestType::from_ordinal(-1), Grassland);
        assert_eq!(ForestType::from_ordinal(99), Grassland);
        assert_eq!(
            ForestType::from_ordinal(SecondaryBamboo as i32),
            SecondaryBamboo
        );
        assert_eq!(PrimaryMonoculture.alternate(), PrimaryAlternate);
        assert_eq!(SavannaDiverse.alternate(), SavannaDiverse);
        assert!(EdgeDiverse.is_as_or_more_dense_as(Sparse));
    }
}
