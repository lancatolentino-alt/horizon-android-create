//! TerraFirmaCraft's area layer framework (`world.layer.*`, EUPL 1.2): lazily sampled
//! integer grids that are zoomed, smoothed and merged. Includes the uniform layer used
//! to pick rock layers, and TFC's default rock layer settings.
use horizon_core::random::{RandomSource, XoroshiroRandom};
use std::sync::Arc;

/// `HashCommon.murmurHash3` / `Helpers.mix64`.
fn murmur_hash3(mut x: i64) -> i64 {
    x ^= ((x as u64) >> 33) as i64;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd_u64 as i64);
    x ^= ((x as u64) >> 33) as i64;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53_u64 as i64);
    x ^= ((x as u64) >> 33) as i64;
    x
}
/// `HashCommon.mix(long)`.
fn mix(x: i64) -> i64 {
    let mut h = x.wrapping_mul(0x9E37_79B9_7F4A_7C15_u64 as i64);
    h ^= ((h as u64) >> 32) as i64;
    h ^ (((h as u64) >> 16) as i64)
}

/// `AreaContext`: a per-layer random source, reseeded for each sampled position.
pub struct AreaContext {
    seed: i64,
    pub random: XoroshiroRandom,
}
impl AreaContext {
    pub fn new(seed: i64) -> Self {
        Self {
            seed: murmur_hash3(seed),
            random: XoroshiroRandom::new(seed),
        }
    }
    pub fn set_seed(&mut self, x: i64, z: i64) {
        let s = (x.wrapping_mul(501_125_321) ^ z.wrapping_mul(1_136_930_381) ^ self.seed)
            .wrapping_mul(0x27d4_eb2d);
        self.random.set_seed(s);
    }
    pub fn choose2(&mut self, first: i32, second: i32) -> i32 {
        if self.random.next_boolean() {
            first
        } else {
            second
        }
    }
    pub fn choose4(&mut self, first: i32, second: i32, third: i32, fourth: i32) -> i32 {
        match self.random.next_int_bound(4).unwrap() {
            0 => first,
            1 => second,
            2 => third,
            _ => fourth,
        }
    }
}

/// `Area`: a direct-mapped cache in front of a position-pure sampling function.
pub struct Area {
    source: Box<dyn FnMut(i32, i32) -> i32 + Send>,
    keys: Vec<i64>,
    values: Vec<i32>,
    mask: usize,
}
impl Area {
    pub fn new(
        source: impl FnMut(i32, i32) -> i32 + Send + 'static,
        max_cache_size: usize,
    ) -> Self {
        let size = max_cache_size.next_power_of_two();
        Self {
            source: Box::new(source),
            keys: vec![i64::MIN; size],
            values: vec![0; size],
            mask: size - 1,
        }
    }
    pub fn get(&mut self, x: i32, z: i32) -> i32 {
        let key = (i64::from(x) & 0xFFFF_FFFF) | ((i64::from(z) & 0xFFFF_FFFF) << 32);
        let index = (mix(key) as i32 as usize) & self.mask;
        if self.keys[index] == key {
            self.values[index]
        } else {
            let value = (self.source)(x, z);
            self.values[index] = value;
            self.keys[index] = key;
            value
        }
    }
}

pub type AreaFactory = Arc<dyn Fn() -> Area + Send + Sync>;

/// `SourceLayer.apply(seed)`.
pub fn source_layer(
    seed: i64,
    f: impl Fn(&mut AreaContext, i32, i32) -> i32 + Send + Sync + 'static,
) -> AreaFactory {
    let f = Arc::new(f);
    Arc::new(move || {
        let mut context = AreaContext::new(seed);
        let f = f.clone();
        Area::new(
            move |x, z| {
                context.set_seed(i64::from(x), i64::from(z));
                f(&mut context, x, z)
            },
            1024,
        )
    })
}

/// `TransformLayer.apply(seed, prev)`.
pub fn transform_layer(
    seed: i64,
    prev: AreaFactory,
    f: impl Fn(&mut AreaContext, &mut Area, i32, i32) -> i32 + Send + Sync + 'static,
) -> AreaFactory {
    let f = Arc::new(f);
    Arc::new(move || {
        let mut context = AreaContext::new(seed);
        let mut prev_area = prev();
        let f = f.clone();
        Area::new(
            move |x, z| {
                context.set_seed(i64::from(x), i64::from(z));
                f(&mut context, &mut prev_area, x, z)
            },
            1024,
        )
    })
}

/// `MergeLayer.apply(seed, first, second)`.
pub fn merge_layer(
    seed: i64,
    first: AreaFactory,
    second: AreaFactory,
    f: impl Fn(&mut AreaContext, &mut Area, &mut Area, i32, i32) -> i32 + Send + Sync + 'static,
) -> AreaFactory {
    let f = Arc::new(f);
    Arc::new(move || {
        let mut context = AreaContext::new(seed);
        let (mut a, mut b) = (first(), second());
        let f = f.clone();
        Area::new(
            move |x, z| {
                context.set_seed(i64::from(x), i64::from(z));
                f(&mut context, &mut a, &mut b, x, z)
            },
            1024,
        )
    })
}

/// `AdjacentTransformLayer`: the callback gets (north, east, south, west, center).
pub fn adjacent_layer(
    seed: i64,
    prev: AreaFactory,
    f: impl Fn(&mut AreaContext, i32, i32, i32, i32, i32) -> i32 + Send + Sync + 'static,
) -> AreaFactory {
    transform_layer(seed, prev, move |c, area, x, z| {
        let (n, e, s, w, m) = (
            area.get(x, z - 1),
            area.get(x + 1, z),
            area.get(x, z + 1),
            area.get(x - 1, z),
            area.get(x, z),
        );
        f(c, n, e, s, w, m)
    })
}

/// `UniformLayer`: every position is an independent random int.
pub fn uniform_layer(seed: i64) -> AreaFactory {
    source_layer(seed, |c, _, _| c.random.next_int())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    Normal,
    Fuzzy,
}
impl Zoom {
    fn choose(self, c: &mut AreaContext, first: i32, second: i32, third: i32, fourth: i32) -> i32 {
        match self {
            Zoom::Fuzzy => c.choose4(first, second, third, fourth),
            Zoom::Normal => {
                if first == second {
                    if first == third || third != fourth {
                        first
                    } else {
                        c.choose2(first, third)
                    }
                } else if first == third {
                    if second != fourth {
                        first
                    } else {
                        c.choose2(first, second)
                    }
                } else if first == fourth {
                    if second != third {
                        first
                    } else {
                        c.choose2(first, second)
                    }
                } else if second == third || second == fourth {
                    second
                } else if third == fourth {
                    third
                } else {
                    c.choose4(first, second, third, fourth)
                }
            }
        }
    }
}
/// `ZoomLayer.apply(seed, prev)`.
pub fn zoom_layer(kind: Zoom, seed: i64, prev: AreaFactory) -> AreaFactory {
    transform_layer(seed, prev, move |c, area, x, z| {
        let (px, pz) = (x >> 1, z >> 1);
        let (ox, oz) = (x & 1, z & 1);
        let north_west = area.get(px, pz);
        c.set_seed(i64::from(px), i64::from(pz));
        if ox == 0 && oz == 0 {
            north_west
        } else if ox == 0 {
            c.choose2(north_west, area.get(px, pz + 1))
        } else if oz == 0 {
            c.choose2(north_west, area.get(px + 1, pz))
        } else {
            let (b, d, e) = (
                area.get(px, pz + 1),
                area.get(px + 1, pz),
                area.get(px + 1, pz + 1),
            );
            kind.choose(c, north_west, b, d, e)
        }
    })
}
/// `SmoothLayer`. Parameter names follow the Java signature, which swaps east and west.
pub fn smooth_layer(seed: i64, prev: AreaFactory) -> AreaFactory {
    adjacent_layer(seed, prev, |c, north, west, south, east, center| {
        let equal_x = west == east;
        let equal_z = north == south;
        if equal_x == equal_z {
            if equal_x {
                return c.choose2(east, north);
            }
            return center;
        }
        if equal_x { east } else { north }
    })
}

/// `TFCLayers.createUniformLayer`, with `next_seed` standing in for `Seed.next()`.
pub fn create_uniform_layer(mut next_seed: impl FnMut() -> i64, zoom_levels: i32) -> AreaFactory {
    let mut layer = uniform_layer(next_seed());
    for _ in 0..zoom_levels {
        layer = zoom_layer(Zoom::Normal, next_seed(), layer);
        layer = smooth_layer(next_seed(), layer);
    }
    layer
}

/// `RockSettings`, reduced to what the region generator reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rock {
    pub name: &'static str,
    pub karst: bool,
}

/// One rock in a layer list; `next` indexes `RockLayerSettings::lists`.
#[derive(Clone, Debug)]
pub struct RockLayer {
    pub rock: usize,
    pub next: usize,
}

/// Region point rock kinds, `ChooseRocks`.
pub const ROCK_OCEAN: i32 = 0;
pub const ROCK_VOLCANIC: i32 = 1;
pub const ROCK_LAND: i32 = 2;
pub const ROCK_UPLIFT: i32 = 3;
pub const ROCK_TYPE_BITS: i32 = 2;
pub const ROCK_TYPE_MASK: i32 = (1 << ROCK_TYPE_BITS) - 1;

/// `RockLayerSettings` for the `tfc:overworld` preset.
pub struct RockLayerSettings {
    pub rocks: Vec<Rock>,
    /// Named layer lists; index 0 is `bottom`, whose entries lead back to itself.
    pub lists: Vec<Vec<RockLayer>>,
    ocean_floor: Vec<RockLayer>,
    land: Vec<RockLayer>,
    volcanic: Vec<RockLayer>,
    uplift: Vec<RockLayer>,
}

const KARST: [&str; 4] = ["chalk", "dolomite", "limestone", "marble"];
const ROCKS: [&str; 20] = [
    "andesite",
    "basalt",
    "chalk",
    "chert",
    "claystone",
    "conglomerate",
    "dacite",
    "diorite",
    "dolomite",
    "gabbro",
    "gneiss",
    "granite",
    "limestone",
    "marble",
    "phyllite",
    "quartzite",
    "rhyolite",
    "schist",
    "shale",
    "slate",
];
/// (id, [(rock, next layer id)]) in file order.
#[allow(clippy::type_complexity)]
const LAYERS: &[(&str, &[(&str, &str)])] = &[
    ("felsic", &[("granite", "bottom")]),
    ("intermediate", &[("diorite", "bottom")]),
    ("mafic", &[("gabbro", "bottom")]),
    (
        "igneous_extrusive",
        &[
            ("andesite", "intermediate"),
            ("basalt", "mafic"),
            ("dacite", "intermediate"),
            ("rhyolite", "felsic"),
        ],
    ),
    (
        "igneous_extrusive_x2",
        &[
            ("andesite", "igneous_extrusive"),
            ("basalt", "igneous_extrusive"),
            ("dacite", "igneous_extrusive"),
            ("rhyolite", "igneous_extrusive"),
        ],
    ),
    (
        "igneous_intrusive",
        &[
            ("diorite", "intermediate"),
            ("gabbro", "mafic"),
            ("granite", "felsic"),
        ],
    ),
    ("high_grade", &[("gneiss", "bottom"), ("schist", "bottom")]),
    (
        "low_grade",
        &[("phyllite", "high_grade"), ("slate", "high_grade")],
    ),
    ("marble", &[("marble", "bottom")]),
    ("quartzite", &[("quartzite", "bottom")]),
    (
        "sedimentary",
        &[
            ("chalk", "marble"),
            ("chert", "quartzite"),
            ("claystone", "low_grade"),
            ("conglomerate", "low_grade"),
            ("dolomite", "marble"),
            ("limestone", "marble"),
            ("shale", "low_grade"),
        ],
    ),
    (
        "uplift",
        &[
            ("diorite", "low_grade"),
            ("gabbro", "low_grade"),
            ("gneiss", "high_grade"),
            ("granite", "low_grade"),
            ("marble", "bottom"),
            ("phyllite", "high_grade"),
            ("quartzite", "bottom"),
            ("schist", "high_grade"),
            ("slate", "high_grade"),
        ],
    ),
];

impl RockLayerSettings {
    pub fn tfc_default() -> Self {
        let rocks: Vec<Rock> = ROCKS
            .iter()
            .map(|&name| Rock {
                name,
                karst: KARST.contains(&name),
            })
            .collect();
        let idx = |name: &str| ROCKS.iter().position(|r| *r == name).unwrap();
        let mut names: Vec<&str> = vec!["bottom"];
        let mut lists: Vec<Vec<RockLayer>> = vec![
            ["gneiss", "schist", "diorite", "granite", "gabbro"]
                .iter()
                .map(|r| RockLayer {
                    rock: idx(r),
                    next: 0,
                })
                .collect(),
        ];
        for (id, entries) in LAYERS {
            let baked = entries
                .iter()
                .map(|(rock, next)| RockLayer {
                    rock: idx(rock),
                    next: names.iter().position(|n| n == next).unwrap(),
                })
                .collect();
            names.push(id);
            lists.push(baked);
        }
        let top = |ids: &[&str]| -> Vec<RockLayer> {
            ids.iter()
                .flat_map(|id| {
                    lists[names.iter().position(|n| n == id).unwrap()]
                        .iter()
                        .cloned()
                })
                .collect()
        };
        let (ocean_floor, land, volcanic, uplift) = (
            top(&["igneous_extrusive"]),
            top(&[
                "sedimentary",
                "sedimentary",
                "sedimentary",
                "igneous_extrusive",
            ]),
            top(&[
                "igneous_extrusive",
                "igneous_extrusive_x2",
                "igneous_intrusive",
            ]),
            top(&["uplift", "uplift", "uplift", "sedimentary"]),
        );
        Self {
            rocks,
            lists,
            ocean_floor,
            land,
            volcanic,
            uplift,
        }
    }

    /// `sampleAtLayer(pointRock, layerN)`.
    pub fn sample_at_layer(&self, point_rock: i32, layer_n: i32) -> &Rock {
        let mut source = XoroshiroRandom::new(i64::from(point_rock >> ROCK_TYPE_BITS));
        let root = match point_rock & ROCK_TYPE_MASK {
            ROCK_OCEAN => &self.ocean_floor,
            ROCK_VOLCANIC => &self.volcanic,
            ROCK_LAND => &self.land,
            _ => &self.uplift,
        };
        let mut layer = &root[source.next_int_bound(root.len() as i32).unwrap() as usize];
        for _ in 0..layer_n {
            let next = &self.lists[layer.next];
            layer = &next[source.next_int_bound(next.len() as i32).unwrap() as usize];
        }
        &self.rocks[layer.rock]
    }
}
