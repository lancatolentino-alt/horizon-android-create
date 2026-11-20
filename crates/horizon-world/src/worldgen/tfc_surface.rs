//! TerraFirmaCraft's surface layering (EUPL 1.2): the climate-driven soil regions of
//! `SoilSurfaceState` and `NormalSurfaceBuilder`, which lays grass, dirt and gravel down a column by
//! slope and altitude.
//!
//! The `.hmod` has rock blocks only, so soils map to vanilla blocks (grass, dirt, coarse dirt, mud,
//! sand). Which state is chosen, and how deep it goes, follows TFC exactly.
//! Only the Normal and Rocky builders are ported; other biomes keep the simpler layering.

use super::tfc_badlands::{Badlands, Variant as BadlandsVariant};
use super::tfc_biome_noise::{
    burren_crevices, hills, ocean_ridge_edge_distance, patterned_ground, shilin_ridges,
    shoreline_tide_level_noise, stone_circles,
};
use super::tfc_ice_sheet::{self, IceSheet};
use super::tfc_noise::{Noise2D, OpenSimplex2D};
use super::tfc_shield_volcano::{self, IceShield, Kind as ShieldKind2};

const PRIME_X: i64 = 501_125_321;
const PRIME_Y: i64 = 1_136_930_381;
pub const SEA_LEVEL: i32 = 63;

/// `Helpers.hash(salt, x, y, z)`.
pub fn hash(salt: i64, x: i32, y: i32, z: i32) -> i32 {
    let mut h = salt
        ^ (i64::from(x).wrapping_mul(PRIME_X))
        ^ (i64::from(y).wrapping_mul(PRIME_Y))
        ^ i64::from(z);
    h = h.wrapping_mul(0x27d4_eb2d);
    h as i32
}

fn lerp_f(delta: f32, start: f32, end: f32) -> f32 {
    start + delta * (end - start)
}

/// `Mth.clampedMap` with float arguments.
fn clamped_map_f(value: f32, in_min: f32, in_max: f32, out_min: f32, out_max: f32) -> f32 {
    let t = (value - in_min) / (in_max - in_min);
    if t < 0.0 {
        out_min
    } else if t > 1.0 {
        out_max
    } else {
        lerp_f(t, out_min, out_max)
    }
}

/// `Mth.clampedMap` with double arguments.
fn clamped_map_d(value: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    let t = (value - in_min) / (in_max - in_min);
    if t < 0.0 {
        out_min
    } else if t > 1.0 {
        out_max
    } else {
        out_min + t * (out_max - out_min)
    }
}

/// `Helpers.adjustAverageTemperatureByElevation`.
pub fn adjust_temperature_by_elevation(y: i32, average: f32, sea_level: f32) -> f32 {
    if y > 0 {
        return average - ((y as f32 - sea_level) * 0.16225f32).clamp(0.0, 17.822);
    }
    let influence = (y as f32 - (-64.0)) / (0.0 - (-64.0));
    lerp_f(influence, 15.0, average)
}

/// What a surface state needs to know about the block it fills.
pub struct SurfaceCtx<'a> {
    /// `(flow material, flow)` noise for this column when a builder made anew for it has its own
    /// seed (a volcano shore under a river).
    pub lava_sample: Option<(f64, f64)>,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Rock names at a height, for this column.
    pub rock: &'a mut dyn FnMut(i32) -> &'static str,
    pub temperature: f32,
    pub base_groundwater: f32,
    pub rainfall: f32,
    pub rain_variance: f32,
    pub slope: f64,
    pub noises: &'a Noises,
    /// Rock at sea level in this column.
    pub sea_level_rock: &'static str,
    /// The biome (TFC id) before rivers replaced it.
    pub original_biome: i32,
    /// Sea-level temperature noise value for the shore (`BiomeNoise.shoreTideLevelNoise`).
    pub tide: f64,
    /// The weight of this column's biome in the blend (`SurfaceBuilderContext.weight()`).
    pub weight: f64,
}

/// The noise maps surface states read.
pub struct Noises {
    pub patch: OpenSimplex2D,
    pub sand_variant: Noise2D,
    pub sand_gravel_beach: Noise2D,
    pub tide: Noise2D,
    pub burren: Noise2D,
    pub stone_circles: Noise2D,
    pub patterned: Noise2D,
    pub shilin: Noise2D,
    pub hills: Noise2D,
    pub ridge_distance: Noise2D,
    /// `LowlandsSurfaceBuilder.surfaceMaterialNoise`.
    pub lowlands: OpenSimplex2D,
    /// `GrassyDunesSurfaceBuilder.grassHeightVariationNoise`.
    pub grassy_dunes: OpenSimplex2D,
    /// Per-biome builder instances (`tfc_surface_seeds`), by biome id. TFC gives every biome its
    /// own noises; the plain fields stand in for columns no table entry covers.
    pub biome_lowlands: Vec<Option<OpenSimplex2D>>,
    pub biome_grassy_dunes: Vec<Option<OpenSimplex2D>>,
    pub biome_badlands: Vec<Option<Badlands>>,
    pub biome_atoll_height: Vec<OpenSimplex2D>,
    /// Normal, mesas, hoodoos and warped badlands.
    pub badlands: [Badlands; 4],
    pub ice_sheets: Vec<IceSheet>,
    pub ice_shields: Vec<IceShield>,
    pub lava_flow: Noise2D,
    pub lava_flow_material: Noise2D,
    /// `ShoreAndOceanSurfaceBuilder`'s frozen ocean extension.
    pub sea_ice: super::tfc_sea_ice::SeaIce,
}

impl Noises {
    pub fn new(level_seed: i64) -> Self {
        use super::tfc_surface_seeds::{Base, compute};
        use horizon_core::random::XoroshiroRandom;
        let seeds = compute(level_seed);
        let lowlands = |seed: i64| {
            OpenSimplex2D::new(seed)
                .octaves(2)
                .spread(f64::from(0.04f32))
        };
        let dunes = |seed: i64| {
            OpenSimplex2D::new(seed)
                .octaves(2)
                .scaled(f64::from(SEA_LEVEL + 8), f64::from(SEA_LEVEL + 14))
                .spread(f64::from(0.08f32))
        };
        let variant_of = |surface: &str| match surface {
            "BadlandsSurfaceBuilder.MESAS" => BadlandsVariant::Mesas,
            "BadlandsSurfaceBuilder.HOODOOS" => BadlandsVariant::Hoodoos,
            "BadlandsSurfaceBuilder.WARPED" => BadlandsVariant::Warped,
            _ => BadlandsVariant::Normal,
        };
        let ext = super::tfc_biome_ext::extensions();
        Self {
            biome_lowlands: seeds
                .iter()
                .map(|s| match s.base {
                    Base::Lowlands(seed) => Some(lowlands(seed)),
                    _ => None,
                })
                .collect(),
            biome_grassy_dunes: seeds
                .iter()
                .map(|s| match s.base {
                    Base::GrassyDunes(seed) => Some(dunes(seed)),
                    _ => None,
                })
                .collect(),
            biome_badlands: seeds
                .iter()
                .enumerate()
                .map(|(id, s)| match s.base {
                    Base::Badlands(lo, hi) => Some(Badlands::new(
                        variant_of(ext[id].surface),
                        XoroshiroRandom::from_state(lo as u64, hi as u64),
                    )),
                    _ => None,
                })
                .collect(),
            biome_atoll_height: seeds
                .iter()
                .map(|s| {
                    OpenSimplex2D::new(s.atoll)
                        .octaves(2)
                        .spread(f64::from(0.1f32))
                        .scaled(-4.0, 4.0)
                })
                .collect(),
            patch: OpenSimplex2D::new(18_273_952_837_592)
                .octaves(2)
                .spread(f64::from(0.04f32)),
            sand_variant: Noise2D::from_simplex(
                OpenSimplex2D::new(36_263_276)
                    .octaves(5)
                    .spread(f64::from(0.0003f32)),
            )
            .abs(),
            sand_gravel_beach: Noise2D::from_simplex(
                OpenSimplex2D::new(124_154)
                    .octaves(3)
                    .spread(f64::from(0.00002f32)),
            ),
            tide: shoreline_tide_level_noise(level_seed),
            burren: burren_crevices(level_seed),
            stone_circles: stone_circles(level_seed),
            patterned: patterned_ground(level_seed),
            shilin: shilin_ridges(level_seed),
            hills: hills(level_seed, 22, 32),
            ridge_distance: ocean_ridge_edge_distance(level_seed),
            // TFC seeds these two from `seed.next()`, whose position depends on how many noises
            // the biomes before them made; fixed offsets of the level seed stand in.
            lowlands: OpenSimplex2D::new(level_seed.wrapping_add(0x4c6f_776c))
                .octaves(2)
                .spread(f64::from(0.04f32)),
            badlands: [
                Badlands::new(
                    BadlandsVariant::Normal,
                    XoroshiroRandom::new(
                        level_seed.wrapping_add(0x6261_646c + BadlandsVariant::Normal as i64),
                    ),
                ),
                Badlands::new(
                    BadlandsVariant::Mesas,
                    XoroshiroRandom::new(
                        level_seed.wrapping_add(0x6261_646c + BadlandsVariant::Mesas as i64),
                    ),
                ),
                Badlands::new(
                    BadlandsVariant::Hoodoos,
                    XoroshiroRandom::new(
                        level_seed.wrapping_add(0x6261_646c + BadlandsVariant::Hoodoos as i64),
                    ),
                ),
                Badlands::new(
                    BadlandsVariant::Warped,
                    XoroshiroRandom::new(
                        level_seed.wrapping_add(0x6261_646c + BadlandsVariant::Warped as i64),
                    ),
                ),
            ],
            ice_shields: (0..2).map(|i| IceShield::new(i, level_seed)).collect(),
            lava_flow: tfc_shield_volcano::flow_noises(level_seed).0,
            lava_flow_material: tfc_shield_volcano::flow_noises(level_seed).1,
            ice_sheets: (0..tfc_ice_sheet::NAMES.len())
                .map(|i| IceSheet::new(i, level_seed))
                .collect(),
            grassy_dunes: OpenSimplex2D::new(level_seed.wrapping_add(0x4772_6173))
                .octaves(2)
                .scaled(f64::from(SEA_LEVEL + 8), f64::from(SEA_LEVEL + 14))
                .spread(f64::from(0.08f32)),
            sea_ice: super::tfc_sea_ice::SeaIce::new(
                level_seed,
                super::tfc_region::Settings::default().temperature_scale as f32,
            ),
        }
    }
}

impl SurfaceCtx<'_> {
    pub fn ground_water(&self) -> f32 {
        self.base_groundwater + self.rainfall
    }
    fn patch_noise(&self) -> f64 {
        self.noises
            .patch
            .noise(f64::from(self.x), f64::from(self.z))
    }
    /// A stand-in for the chunk's `RandomSource.nextFloat()`: a position hash in `[0, 1)`.
    fn random_float(&self) -> f32 {
        (hash(917_301_551, self.x, self.y, self.z) & 0xff_ffff) as f32 / 16_777_216.0
    }

    fn dither_hash(&self) -> i32 {
        hash(729_375_982, self.x, self.y, self.z)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SoilType {
    Grass,
    Dirt,
    Mud,
}

/// The `SoilBlockType.Variant`s the surface rules place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    Entisol,
    Aridisol,
    Oxisol,
    Fluvisol,
    Andisol,
}

impl Variant {
    fn name(self) -> &'static str {
        match self {
            Variant::Entisol => "entisol",
            Variant::Aridisol => "aridisol",
            Variant::Oxisol => "oxisol",
            Variant::Fluvisol => "fluvisol",
            Variant::Andisol => "andisol",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RockBlock {
    Raw,
    Cobble,
    Gravel,
    Sand,
}

/// A `SurfaceState`.
#[derive(Clone, Debug)]
pub enum State {
    Rock(RockBlock),
    /// `SurfaceStates.MORAINE`: mostly gravel with some cobble.
    Moraine,
    /// `SurfaceStates.BASALT_MORAINE`.
    BasaltMoraine,
    /// `SurfaceStates.VOLCANIC_SHORE_SAND`: green sand in wet places, else black sand.
    VolcanicShoreSand,
    /// `SurfaceStates.VOLCANIC_SHORE_SANDSTONE`.
    VolcanicShoreSandstone,
    /// `SurfaceStates.SAND_AND_GRAVEL`.
    SandAndGravel,
    Fixed(&'static str),
    /// `SoilSurfaceState.soil(type, variant)`.
    Soil(SoilType, Variant),
    /// `SurfaceStates.COARSE_<variant>_BASE`.
    Coarse(Variant),
    TransitioningSoil(SoilType),
    Transition(Box<State>, Box<State>),
    BlobTransition(Box<State>, Box<State>),
    Regions(Vec<State>),
    ShoreSand,
    ShoreSandstone,
    ShoreSurface,
    ShoreUnderlayer,
    OceanMud,
    RiverSand,
}

fn soil_name(kind: SoilType, variant: Variant) -> String {
    let family = match kind {
        SoilType::Grass => "grass",
        SoilType::Dirt => "dirt",
        SoilType::Mud => "mud",
    };
    format!("tfc:{family}/{}", variant.name())
}

pub fn fixed(name: &'static str) -> State {
    State::Fixed(name)
}
pub fn transition(a: State, b: State) -> State {
    State::Transition(Box::new(a), Box::new(b))
}
pub fn blob(a: State, b: State) -> State {
    State::BlobTransition(Box::new(a), Box::new(b))
}

const SNOW: &str = "minecraft:snow_block";
const PACKED_ICE: &str = "minecraft:packed_ice";

fn soil(kind: SoilType, variant: Variant) -> State {
    State::Soil(kind, variant)
}

/// `SoilSurfaceState.buildSurfaceType(type, dry)`.
pub fn surface_type(kind: SoilType, dry: RockBlock) -> State {
    let dry = State::Rock(dry);
    let coarse = || State::Coarse(Variant::Aridisol);
    let arid = || soil(kind, Variant::Aridisol);
    let ts = || State::TransitioningSoil(kind);
    let mut regions = vec![
        fixed(SNOW),
        fixed(SNOW),
        transition(fixed(SNOW), dry.clone()),
        dry.clone(),
        transition(dry, coarse()),
        coarse(),
        transition(coarse(), arid()),
        arid(),
        blob(arid(), ts()),
    ];
    regions.extend((0..9).map(|_| ts()));
    State::Regions(regions)
}

/// `buildVolcanicSurfaceType(type, dry)`: andisol soil.
pub fn volcanic_surface_type(kind: SoilType, dry: RockBlock) -> State {
    let dry = State::Rock(dry);
    let coarse = || State::Coarse(Variant::Andisol);
    let and = || soil(kind, Variant::Andisol);
    let mut regions = vec![
        fixed(SNOW),
        fixed(SNOW),
        transition(fixed(SNOW), dry.clone()),
        dry.clone(),
        transition(dry, coarse()),
        coarse(),
        transition(coarse(), and()),
    ];
    regions.extend((0..11).map(|_| and()));
    State::Regions(regions)
}

/// `buildSiltySurfaceType(type, dry)`: snow, then `dry`, then coarse fluvisol, then fluvisol soil,
/// for atoll beaches.
pub fn silty_surface_type(kind: SoilType, dry: State) -> State {
    let coarse = || State::Coarse(Variant::Fluvisol);
    let flu = || soil(kind, Variant::Fluvisol);
    let mut regions = vec![
        fixed(SNOW),
        fixed(SNOW),
        transition(fixed(SNOW), dry.clone()),
        dry.clone(),
        transition(dry, coarse()),
        coarse(),
        transition(coarse(), flu()),
    ];
    regions.extend((0..11).map(|_| flu()));
    State::Regions(regions)
}

/// `SoilSurfaceState.buildSnowableSurface(snow, typical)`.
pub fn snowable(typical: State) -> State {
    let snow = || fixed(SNOW);
    let mut regions = vec![snow(), snow(), transition(snow(), typical.clone())];
    regions.extend((0..15).map(|_| typical.clone()));
    State::Regions(regions)
}

/// `SoilSurfaceState.buildMidType(type, dry)`.
pub fn mid_type(kind: SoilType, dry: RockBlock) -> State {
    let dry = State::Rock(dry);
    let coarse = || State::Coarse(Variant::Aridisol);
    let arid = || soil(kind, Variant::Aridisol);
    let ts = || State::TransitioningSoil(kind);
    let mut regions = vec![
        fixed(PACKED_ICE),
        blob(fixed(PACKED_ICE), dry.clone()),
        dry.clone(),
        dry.clone(),
        transition(dry, coarse()),
        coarse(),
        transition(coarse(), arid()),
        arid(),
        blob(arid(), ts()),
    ];
    regions.extend((0..9).map(|_| ts()));
    State::Regions(regions)
}

/// `buildVolcanicMidType(type, dry)`.
pub fn volcanic_mid_type(kind: SoilType, dry: RockBlock) -> State {
    let dry = State::Rock(dry);
    let coarse = || State::Coarse(Variant::Andisol);
    let and = || soil(kind, Variant::Andisol);
    let mut regions = vec![
        fixed(PACKED_ICE),
        blob(fixed(PACKED_ICE), dry.clone()),
        dry.clone(),
        dry.clone(),
        transition(dry, coarse()),
        coarse(),
        transition(coarse(), and()),
    ];
    regions.extend((0..11).map(|_| and()));
    State::Regions(regions)
}

/// `SoilSurfaceState.buildUnderType()`.
pub fn under_type() -> State {
    let gravel = || State::Rock(RockBlock::Gravel);
    let mut regions = vec![
        State::Rock(RockBlock::Raw),
        State::Rock(RockBlock::Raw),
        blob(State::Rock(RockBlock::Raw), gravel()),
    ];
    regions.extend((0..15).map(|_| gravel()));
    State::Regions(regions)
}

/// The sand colour of a rock's `RockSettings.sand` (`worldgen/rock_settings`).
pub fn rock_sand_colour(rock: &str) -> &'static str {
    match rock {
        "basalt" | "gabbro" => "black",
        "chalk" | "dolomite" | "limestone" | "marble" | "quartzite" => "white",
        "claystone" | "conglomerate" | "shale" => "brown",
        "gneiss" | "granite" | "phyllite" | "rhyolite" | "schist" | "slate" => "yellow",
        "tuff" => "green",
        _ => "red",
    }
}

/// `RockSettings.mafic()`.
pub fn is_mafic(rock: &str) -> bool {
    matches!(rock, "basalt" | "gabbro")
}

/// `SurfaceStates.RARE_SHORE_SAND` (`sandstone = false`) and `RARE_SHORE_SANDSTONE`.
fn rare_shore(ctx: &SurfaceCtx, sandstone: bool) -> String {
    let colour = if ctx.ground_water() > 300.0 && ctx.temperature > 15.0 {
        "pink"
    } else if is_mafic(ctx.sea_level_rock) {
        if ctx.ground_water() > 300.0 {
            "green"
        } else {
            "black"
        }
    } else {
        "white"
    };
    coloured(colour, sandstone)
}

/// A `tfc:sand/<colour>` or `tfc:raw_sandstone/<colour>`.
pub fn coloured(colour: &str, sandstone: bool) -> String {
    if sandstone {
        format!("tfc:raw_sandstone/{colour}")
    } else {
        format!("tfc:sand/{colour}")
    }
}

/// `SurfaceState.getState`: the block name a state gives at this position.
pub fn state_at(state: &State, ctx: &mut SurfaceCtx) -> String {
    match state {
        State::Fixed(name) => (*name).to_owned(),
        State::Rock(kind) => {
            let rock = (ctx.rock)(ctx.y);
            match kind {
                RockBlock::Raw => format!("tfc:raw_{rock}"),
                RockBlock::Cobble => format!("tfc:cobble_{rock}"),
                RockBlock::Gravel => format!("tfc:gravel_{rock}"),
                RockBlock::Sand => coloured(rock_sand_colour(rock), false),
            }
        }
        State::Moraine => {
            let kind = if (hash(729_375_982, ctx.x, ctx.y, ctx.z) & 127) > 96 {
                RockBlock::Cobble
            } else {
                RockBlock::Gravel
            };
            state_at(&State::Rock(kind), ctx)
        }
        State::BasaltMoraine => {
            let kind = if (hash(729_375_982, ctx.x, ctx.y, ctx.z) & 127) > 96 {
                "tfc:cobble_basalt"
            } else {
                "tfc:gravel_basalt"
            };
            kind.to_owned()
        }
        State::VolcanicShoreSand => {
            let colour = if ctx.ground_water() > 420.0 {
                "green"
            } else {
                "black"
            };
            coloured(colour, false)
        }
        State::VolcanicShoreSandstone => {
            let colour = if ctx.ground_water() > 420.0 {
                "green"
            } else {
                "black"
            };
            coloured(colour, true)
        }
        State::SandAndGravel => {
            let kind = if (hash(728_275_914, ctx.x, ctx.y, ctx.z) & 127) > 48 {
                RockBlock::Sand
            } else {
                RockBlock::Gravel
            };
            state_at(&State::Rock(kind), ctx)
        }
        State::Soil(kind, variant) => soil_name(*kind, *variant),
        State::Coarse(variant) => format!("tfc:coarse_dirt/{}", variant.name()),
        State::TransitioningSoil(kind) => {
            // `SoilSurfaceState.transitioningSoil`: silt beside a flooding river, otherwise entisol
            // below 16 C, oxisol above 16.7 C and a patchy mix between.
            let variant = if ctx.base_groundwater > 25.0 && ctx.rain_variance.abs() > 0.5 {
                Variant::Fluvisol
            } else if ctx.temperature < 16.0 {
                Variant::Entisol
            } else if ctx.temperature > 16.7 || ctx.patch_noise() > 0.0 {
                // Hot enough, or in the hot patches of the 16 - 16.7 C transition.
                Variant::Oxisol
            } else {
                Variant::Entisol
            };
            soil_name(*kind, variant)
        }
        State::Transition(a, b) => {
            if (ctx.dither_hash() & 127) > 63 {
                state_at(a, ctx)
            } else {
                state_at(b, ctx)
            }
        }
        State::BlobTransition(a, b) => {
            if ctx.patch_noise() > 0.0 {
                state_at(a, ctx)
            } else {
                state_at(b, ctx)
            }
        }
        State::OceanMud => soil_name(SoilType::Mud, Variant::Fluvisol),
        State::RiverSand => coloured(rock_sand_colour(ctx.sea_level_rock), false),
        State::ShoreSand => {
            let v = ctx
                .noises
                .sand_variant
                .noise(f64::from(ctx.x), f64::from(ctx.z)) as f32;
            if v > 0.55 {
                rare_shore(ctx, false)
            } else if v > 0.2 {
                coloured("yellow", false)
            } else if v > 0.1 {
                coloured("brown", false)
            } else {
                coloured("red", false)
            }
        }
        State::ShoreSandstone => {
            let v = ctx
                .noises
                .sand_variant
                .noise(f64::from(ctx.x), f64::from(ctx.z)) as f32;
            if v > 0.8 {
                rare_shore(ctx, true)
            } else if v > 0.4 {
                coloured("yellow", true)
            } else if v > 0.2 {
                coloured("red", true)
            } else {
                coloured("brown", true)
            }
        }
        State::ShoreSurface | State::ShoreUnderlayer => {
            let v = ctx
                .noises
                .sand_gravel_beach
                .noise(f64::from(ctx.x), f64::from(ctx.z)) as f32;
            let cutoff = clamped_map_d(f64::from(ctx.temperature), -15.0, 25.0, -0.7, 0.7);
            let gravel = f64::from(v) > cutoff;
            match (state, gravel) {
                (State::ShoreSurface, true) => state_at(&State::Rock(RockBlock::Gravel), ctx),
                (State::ShoreSurface, false) => state_at(&State::ShoreSand, ctx),
                (_, true) => state_at(&State::Rock(RockBlock::Raw), ctx),
                (_, false) => state_at(&State::ShoreSandstone, ctx),
            }
        }
        State::Regions(regions) => {
            let rainfall = ctx.ground_water();
            let temperature =
                adjust_temperature_by_elevation(ctx.y, ctx.temperature, SEA_LEVEL as f32);
            let noise = ctx.patch_noise() as f32;
            let dither = (ctx.dither_hash() & 127) as f32 * (1.0f32 / 64.0) - 1.0;
            let top = regions.len() as f32 - 0.01f32;
            let rain_index = clamped_map_f(
                rainfall + 15.0 * noise + 10.0 * dither,
                35.0,
                450.0,
                3.0,
                top,
            ) as i32;
            let temp_index = clamped_map_d(
                f64::from(temperature),
                -19.0 + 0.5 * f64::from(noise) + 0.5 * f64::from(dither),
                -4.0,
                0.0,
                f64::from(top),
            ) as i32;
            state_at(&regions[rain_index.min(temp_index).max(0) as usize], ctx)
        }
    }
}

/// `SurfaceBuilderContext.calculateAltitudeSlopeSurfaceDepth(y, min, maxDepth)`.
pub fn altitude_slope_depth(y: i32, slope: f64, minimum: i32, max_depth: i32) -> i32 {
    let slope_factor = 1.0 - (slope / 15.0).clamp(0.0, 1.0);
    let sea = f64::from(SEA_LEVEL);
    let sea_factor = if f64::from(y) < sea {
        clamped_map_d((sea - f64::from(y)) / 15.0, 0.0, 0.4, 1.0, 1.4)
    } else {
        1.0
    };
    let max_elevation = if y < SEA_LEVEL + 7 {
        max_depth
    } else {
        clamped_map_d(
            f64::from(y),
            sea + 7.0,
            sea + 67.0,
            f64::from(max_depth),
            2.0,
        ) as i32
    };
    let t = slope_factor * sea_factor;
    let lerped = f64::from(minimum) + t * f64::from(max_elevation - minimum);
    // `Mth.clamp` is `min(max(v, lo), hi)`: no panic when `lo > hi`, as with a minimum of 4.
    (lerped as i32).max(minimum).min(max_elevation)
}

/// What a column holds while a builder runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    Air,
    Water,
    /// Unmodified rock.
    Rock,
    Placed(String),
}

/// A column of cells from `min_y` upward.
pub struct Column {
    pub min_y: i32,
    pub cells: Vec<Cell>,
}

impl Column {
    pub fn get(&self, y: i32) -> &Cell {
        &self.cells[(y - self.min_y) as usize]
    }
    pub fn set(&mut self, y: i32, name: String) {
        self.cells[(y - self.min_y) as usize] = Cell::Placed(name);
    }
}

/// The states one `NormalSurfaceBuilder.buildSurface` call is given.
#[derive(Clone)]
pub struct Layers {
    pub top: State,
    pub mid: State,
    pub under: State,
    pub under_water: State,
    pub thin_under_water: State,
    pub top_cave: State,
    pub mid_cave: State,
    pub under_cave: State,
    pub cave_height: i32,
}

impl Layers {
    pub fn five(top: State, mid: State, under: State, under_water: State, thin: State) -> Self {
        Self {
            top_cave: top.clone(),
            mid_cave: mid.clone(),
            under_cave: under.clone(),
            top,
            mid,
            under,
            under_water,
            thin_under_water: thin,
            cave_height: SEA_LEVEL,
        }
    }
    pub fn three(top: State, mid: State, under: State) -> Self {
        let gravel = State::Rock(RockBlock::Gravel);
        Self::five(top, mid, under, gravel.clone(), gravel)
    }
    /// The default grass-to-gravel layering.
    pub fn grass() -> Self {
        Self::three(
            surface_type(SoilType::Grass, RockBlock::Gravel),
            mid_type(SoilType::Dirt, RockBlock::Gravel),
            under_type(),
        )
    }
}

/// `NormalSurfaceBuilder.buildSurface(context, startY, endY, states...)`.
pub fn build_with(
    column: &mut Column,
    ctx: &mut SurfaceCtx,
    start_y: i32,
    end_y: i32,
    min_depth: i32,
    layers: &Layers,
) {
    let raw = State::Rock(RockBlock::Raw);
    let (mut top, mut mid, mut under) = (&layers.top, &layers.mid, &layers.under);
    let (mut surface_depth, mut surface_y) = (-1, 0);
    let (mut underwater_layer, mut first_layer, mut has_placed_first_surface) =
        (false, false, false);
    let mut surface_state: &State = &raw;
    let mut y = start_y;
    while y >= end_y {
        ctx.y = y;
        let at = column.get(y).clone();
        if at == Cell::Air {
            surface_depth = -1;
            if y <= layers.cave_height {
                top = &layers.top_cave;
                mid = &layers.mid_cave;
                under = &layers.under_cave;
            } else if has_placed_first_surface {
                top = surface_state;
                mid = surface_state;
                under = surface_state;
            }
        } else if at == Cell::Rock {
            if surface_depth == -1 {
                surface_y = y;
                first_layer = true;
                if y < SEA_LEVEL - 1 {
                    surface_depth = altitude_slope_depth(surface_y, ctx.slope, -1, 5);
                    if surface_depth < -1 {
                        surface_depth = 0;
                    } else if surface_depth == -1 {
                        surface_depth = 0;
                        let name = state_at(&layers.thin_under_water, ctx);
                        column.set(y, name);
                    } else {
                        let name = state_at(&layers.under_water, ctx);
                        column.set(y, name);
                    }
                    surface_state = &layers.under_water;
                    underwater_layer = true;
                } else {
                    surface_depth = altitude_slope_depth(surface_y, ctx.slope, min_depth, 5);
                    if surface_depth < -1 {
                        surface_depth = 0;
                    } else if surface_depth == -1 {
                        surface_depth = 0;
                        let name = state_at(under, ctx);
                        column.set(y, name);
                    } else {
                        let name = state_at(top, ctx);
                        column.set(y, name);
                    }
                    surface_state = mid;
                    underwater_layer = false;
                }
            } else if surface_depth > 0 {
                has_placed_first_surface = true;
                surface_depth -= 1;
                let name = state_at(surface_state, ctx);
                column.set(y, name);
                if surface_depth == 0 && first_layer {
                    first_layer = false;
                    surface_depth = altitude_slope_depth(surface_y, ctx.slope, 0, 5);
                    surface_state = if underwater_layer {
                        &layers.thin_under_water
                    } else {
                        under
                    };
                }
            }
        }
        y -= 1;
    }
}

/// A TFC surface builder, by the name the biome table gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Builder {
    Normal,
    Rocky,
    /// `ShoreAndOceanSurfaceBuilder` with its sand height and which builder takes over on land.
    Shore {
        kind: ShoreKind,
        sand_height: i32,
        land: Box<Builder>,
    },
    /// `SimpleSurfaceBuilder`: a fixed top/mid/water with the Normal or Rocky builder.
    Simple {
        kind: SimpleKind,
        rocky: bool,
    },
    River,
    /// `IceSheetSurfaceBuilder`, by its index in `tfc_ice_sheet::NAMES`.
    IceSheet(usize),
    /// `IceSheetShieldVolcanoSurfaceBuilder`: `0` ice sheet, `1` glaciated.
    IceShield(usize),
    /// `ShieldVolcanoSurfaceBuilder`.
    Shield(ShieldKind2),
    /// The smaller biome-specific builders.
    Other(Other),
}

/// Builders that only pick states for `NormalSurfaceBuilder` from noise or weights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Other {
    Lowlands,
    Burren,
    StoneCircles,
    FlatsMuddy,
    FlatsSalty,
    Patterned,
    Shilin,
    SandyAlternate,
    RockyPlateau,
    Dune,
    GrassyDunes,
    Badlands(BadlandsVariant),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShoreKind {
    Shore,
    Sandy,
    Gravelly,
    Ocean,
    RockyShore,
    Mountains,
    OceanRidge,
    /// `OLD_SHIELD_VOLCANO`: the shore keeps its sandy shield builder.
    OldShield,
    /// `ACTIVE_SHIELD_VOLCANO`: lava flows run onto the beach.
    ActiveShield,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimpleKind {
    RockyShore,
    VolcanicSoil,
    OceanMud,
}

impl Builder {
    /// The builder for a biome's `surface` string, when this module ports it.
    pub fn from_name(name: &str) -> Option<Builder> {
        use ShoreKind::*;
        let shore = |kind, sand_height, land: Builder| Builder::Shore {
            kind,
            sand_height,
            land: Box::new(land),
        };
        Some(match name {
            "NormalSurfaceBuilder.INSTANCE" => Builder::Normal,
            "NormalSurfaceBuilder.ROCKY" => Builder::Rocky,
            "ShoreAndOceanSurfaceBuilder.OCEAN" => shore(
                Ocean,
                6,
                Builder::Simple {
                    kind: SimpleKind::OceanMud,
                    rocky: false,
                },
            ),
            "ShoreAndOceanSurfaceBuilder.SANDY" => shore(Sandy, 6, Builder::Rocky),
            "ShoreAndOceanSurfaceBuilder.SEA_CLIFFS" => shore(Shore, 2, Builder::Rocky),
            "ShoreAndOceanSurfaceBuilder.MOUNTAINS" => shore(Mountains, 2, Builder::Rocky),
            "ShoreAndOceanSurfaceBuilder.VOLCANIC_MOUNTAINS" => shore(
                Mountains,
                2,
                Builder::Simple {
                    kind: SimpleKind::VolcanicSoil,
                    rocky: true,
                },
            ),
            "ShoreAndOceanSurfaceBuilder.ROCKY_SHORE" => shore(
                RockyShore,
                6,
                Builder::Simple {
                    kind: SimpleKind::RockyShore,
                    rocky: true,
                },
            ),
            "SimpleSurfaceBuilder.ROCKY_VOLCANIC_SOIL" => Builder::Simple {
                kind: SimpleKind::VolcanicSoil,
                rocky: true,
            },
            "SimpleSurfaceBuilder.VOLCANIC_SOIL" => Builder::Simple {
                kind: SimpleKind::VolcanicSoil,
                rocky: true,
            },
            "RiverSurfaceBuilder.INSTANCE" => Builder::River,
            "ShoreAndOceanSurfaceBuilder.OLD_SHIELD_VOLCANO" => {
                shore(OldShield, 6, Builder::Shield(ShieldKind2::Active))
            }
            "ShoreAndOceanSurfaceBuilder.ACTIVE_SHIELD_VOLCANO" => {
                shore(ActiveShield, 2, Builder::Shield(ShieldKind2::Dormant))
            }
            "ShieldVolcanoSurfaceBuilder.ACTIVE" => Builder::Shield(ShieldKind2::Active),
            "ShieldVolcanoSurfaceBuilder.DORMANT" => Builder::Shield(ShieldKind2::Dormant),
            "ShieldVolcanoSurfaceBuilder.SHORE" => Builder::Shield(ShieldKind2::Shore),
            "IceSheetShieldVolcanoSurfaceBuilder.ICE_SHEET" => Builder::IceShield(0),
            "IceSheetShieldVolcanoSurfaceBuilder.GLACIATED" => Builder::IceShield(1),
            "ShoreAndOceanSurfaceBuilder.OCEAN_RIDGE" => shore(
                OceanRidge,
                6,
                Builder::Simple {
                    kind: SimpleKind::RockyShore,
                    rocky: true,
                },
            ),
            name if tfc_ice_sheet::NAMES.contains(&name) => Builder::IceSheet(
                tfc_ice_sheet::NAMES
                    .iter()
                    .position(|n| *n == name)
                    .unwrap_or(0),
            ),
            "LowlandsSurfaceBuilder.INSTANCE" => Builder::Other(Other::Lowlands),
            "BurrenSurfaceBuilder.INSTANCE" => Builder::Other(Other::Burren),
            "StoneCirclesSurfaceBuilder.INSTANCE" => Builder::Other(Other::StoneCircles),
            "FlatsSurfaceBuilder.MUDDY" => Builder::Other(Other::FlatsMuddy),
            "FlatsSurfaceBuilder.SALTY" => Builder::Other(Other::FlatsSalty),
            "PatternedGroundSurfaceBuilder.INSTANCE" => Builder::Other(Other::Patterned),
            "ShilinSurfaceBuilder.INSTANCE" => Builder::Other(Other::Shilin),
            "NormalAlternateSurfaceBuilder.SANDY" => Builder::Other(Other::SandyAlternate),
            "RockyPlateauSurfaceBuilder.INSTANCE" => Builder::Other(Other::RockyPlateau),
            "DuneSurfaceBuilder.INSTANCE" => Builder::Other(Other::Dune),
            "GrassyDunesSurfaceBuilder.INSTANCE" => Builder::Other(Other::GrassyDunes),
            "BadlandsSurfaceBuilder.NORMAL" => {
                Builder::Other(Other::Badlands(BadlandsVariant::Normal))
            }
            "BadlandsSurfaceBuilder.MESAS" => {
                Builder::Other(Other::Badlands(BadlandsVariant::Mesas))
            }
            "BadlandsSurfaceBuilder.HOODOOS" => {
                Builder::Other(Other::Badlands(BadlandsVariant::Hoodoos))
            }
            "BadlandsSurfaceBuilder.WARPED" => {
                Builder::Other(Other::Badlands(BadlandsVariant::Warped))
            }
            _ => return None,
        })
    }
}

/// What a builder needs to know about the column beyond the context.
pub struct BuildInfo {
    /// Height of the ocean floor in this column.
    pub ocean_floor: i32,
    /// Builder of the original (pre-river) biome, for the river builder.
    pub original: Option<Builder>,
    pub original_is_shore: bool,
    pub sandy_river_shores: bool,
}

/// `SurfaceBuilder.buildSurface` for a ported builder.
pub fn build(
    builder: &Builder,
    column: &mut Column,
    ctx: &mut SurfaceCtx,
    info: &BuildInfo,
    start_y: i32,
    end_y: i32,
) {
    match builder {
        Builder::Normal => build_with(column, ctx, start_y, end_y, -1, &Layers::grass()),
        Builder::Rocky => build_with(column, ctx, start_y, end_y, -3, &Layers::grass()),
        Builder::Simple { kind, rocky } => {
            let depth = if *rocky { -3 } else { -1 };
            let layers = match kind {
                SimpleKind::RockyShore => Layers::five(
                    State::Rock(RockBlock::Raw),
                    State::Rock(RockBlock::Raw),
                    State::Rock(RockBlock::Raw),
                    State::Rock(RockBlock::Gravel),
                    State::Rock(RockBlock::Gravel),
                ),
                SimpleKind::VolcanicSoil => {
                    let mid = volcanic_mid_type(SoilType::Dirt, RockBlock::Gravel);
                    Layers::five(
                        volcanic_surface_type(SoilType::Grass, RockBlock::Gravel),
                        mid.clone(),
                        mid,
                        State::Rock(RockBlock::Gravel),
                        State::Rock(RockBlock::Gravel),
                    )
                }
                SimpleKind::OceanMud => Layers::five(
                    State::OceanMud,
                    State::OceanMud,
                    State::OceanMud,
                    State::Rock(RockBlock::Gravel),
                    State::Rock(RockBlock::Gravel),
                ),
            };
            build_with(column, ctx, start_y, end_y, depth, &layers);
        }
        Builder::Shore {
            kind,
            sand_height,
            land,
        } => {
            let tide = ctx.tide as i32;
            let sand_absolute = tide + sand_height;
            let ocean_floor = if start_y > SEA_LEVEL {
                start_y
            } else {
                info.ocean_floor
            };
            let (surface, subsurface) = match kind {
                ShoreKind::Shore | ShoreKind::Ocean | ShoreKind::OceanRidge => {
                    (State::ShoreSurface, State::ShoreUnderlayer)
                }
                ShoreKind::Sandy => (State::ShoreSand, State::ShoreSandstone),
                ShoreKind::Gravelly | ShoreKind::Mountains => {
                    (State::Rock(RockBlock::Gravel), State::Rock(RockBlock::Raw))
                }
                ShoreKind::RockyShore => (State::Rock(RockBlock::Raw), State::Rock(RockBlock::Raw)),
                ShoreKind::OldShield | ShoreKind::ActiveShield => {
                    (State::VolcanicShoreSand, State::VolcanicShoreSandstone)
                }
            };
            if ocean_floor < tide - 5 && *kind == ShoreKind::OceanRidge {
                let distance = ctx
                    .noises
                    .ridge_distance
                    .noise(f64::from(ctx.x), f64::from(ctx.z));
                if distance >= 50.0 {
                    // More mud away from the ridge.
                    let end_out =
                        clamped_map_d(distance, 50.0, 200.0, f64::from(start_y), f64::from(end_y))
                            as i32;
                    let ocean_mud = Builder::Simple {
                        kind: SimpleKind::OceanMud,
                        rocky: false,
                    };
                    build(&ocean_mud, column, ctx, info, start_y, end_out);
                }
            } else if ocean_floor < tide - 5 {
                let mud = State::OceanMud;
                let layers = Layers::five(mud.clone(), mud.clone(), mud.clone(), mud.clone(), mud);
                build_with(column, ctx, start_y, end_y, -1, &layers);
            // `ACTIVE_SHIELD_VOLCANO` passes `shieldVolcano = false, activeShieldVolcano = true`, so
            // Java's lava-flow beach (`if (isShieldVolcano) { if (isActive) ..`) is unreachable and
            // the active volcano's shore is the plain volcanic sand below.
            } else if ocean_floor <= sand_absolute && *kind == ShoreKind::OldShield {
                tfc_shield_volcano::build_shield(ShieldKind2::Shore, column, ctx, start_y, end_y);
            } else if ocean_floor <= sand_absolute {
                let layers = Layers::five(
                    surface.clone(),
                    surface.clone(),
                    subsurface,
                    surface.clone(),
                    surface,
                );
                build_with(column, ctx, start_y, end_y, -1, &layers);
            } else {
                build(land, column, ctx, info, start_y, end_y);
            }
            // Frozen oceans: sea ice and icebergs once the shore or ocean is built.
            let noises = ctx.noises;
            noises
                .sea_ice
                .extend(column, ctx, start_y, end_y, ocean_floor);
        }
        Builder::IceSheet(index) => {
            let noises = ctx.noises;
            noises.ice_sheets[*index].build(column, ctx, info, start_y, end_y);
        }
        Builder::IceShield(index) => {
            let noises = ctx.noises;
            noises.ice_shields[*index].build(column, ctx, info, start_y, end_y);
        }
        Builder::Shield(kind) => {
            tfc_shield_volcano::build_shield(*kind, column, ctx, start_y, end_y)
        }
        Builder::Other(other) => build_other(*other, column, ctx, start_y, end_y),
        Builder::River => {
            if info.original_is_shore {
                if let Some(original) = &info.original {
                    // `RiverSurfaceBuilder` builds the shore's builder anew for every column
                    // from a seed that keeps advancing, so an ice shore's noise is white. A
                    // hash of the position stands in for that sequence.
                    if let Builder::IceSheet(index) = original {
                        IceSheet::new(*index, column_hash(ctx.x, ctx.z))
                            .build(column, ctx, info, start_y, end_y);
                        return;
                    }
                    // A volcano shore likewise reads lava noise of a fresh seed.
                    if let Builder::Shore { land, .. } = original
                        && matches!(**land, Builder::Shield(_))
                    {
                        let hash = column_hash(ctx.x, ctx.z);
                        let x = f64::from(ctx.x);
                        let z = f64::from(ctx.z);
                        ctx.lava_sample = Some((
                            super::tfc_biome_noise::lava_flow_material(hash).noise(x, z),
                            super::tfc_biome_noise::lava_flow(hash).noise(x, z),
                        ));
                        build(original, column, ctx, info, start_y, end_y);
                        ctx.lava_sample = None;
                        return;
                    }
                    build(original, column, ctx, info, start_y, end_y);
                    return;
                }
            }
            if !info.sandy_river_shores {
                build_with(column, ctx, start_y, end_y, -1, &Layers::grass());
            } else {
                let top = if ctx.slope < 2.0 {
                    surface_type(SoilType::Grass, RockBlock::Gravel)
                } else if ctx.slope < 5.0 {
                    State::RiverSand
                } else {
                    State::Rock(RockBlock::Gravel)
                };
                let gravel = State::Rock(RockBlock::Gravel);
                build_with(
                    column,
                    ctx,
                    start_y,
                    end_y,
                    -1,
                    &Layers::three(top, gravel.clone(), gravel),
                );
            }
        }
    }
}

/// A seed for a column: stands in for the shared `Seed` Java advances for every shore builder it
/// builds under a river.
fn column_hash(x: i32, z: i32) -> i64 {
    let mut hash =
        (i64::from(x) << 32 | i64::from(z) & 0xFFFF_FFFF).wrapping_mul(-0x61c8_8646_80b5_83eb);
    hash ^= hash >> 29;
    hash
}

fn raw() -> State {
    State::Rock(RockBlock::Raw)
}
fn gravel() -> State {
    State::Rock(RockBlock::Gravel)
}
fn sand() -> State {
    State::Rock(RockBlock::Sand)
}
fn top_grass_to_gravel() -> State {
    surface_type(SoilType::Grass, RockBlock::Gravel)
}
fn top_grass_to_sand() -> State {
    surface_type(SoilType::Grass, RockBlock::Sand)
}
fn mid_dirt_to_gravel() -> State {
    mid_type(SoilType::Dirt, RockBlock::Gravel)
}
fn mid_dirt_to_sand() -> State {
    mid_type(SoilType::Dirt, RockBlock::Sand)
}
/// `SurfaceStates.DRY_MUD` (hardened clay) and `SALTED_EARTH` (halite).
fn dry_mud() -> State {
    fixed("tfc:hardened_clay")
}
fn salted_earth() -> State {
    fixed("tfc:halite")
}

/// The builders that choose states for `NormalSurfaceBuilder`; `rocky` ones use a minimum depth
/// of -3, the others -1.
fn build_other(other: Other, column: &mut Column, ctx: &mut SurfaceCtx, start_y: i32, end_y: i32) {
    let (x, z) = (f64::from(ctx.x), f64::from(ctx.z));
    let weight = ctx.weight;
    let normal = |column: &mut Column, ctx: &mut SurfaceCtx, layers: Layers| {
        build_with(column, ctx, start_y, end_y, -1, &layers);
    };
    let rocky = |column: &mut Column, ctx: &mut SurfaceCtx, layers: Layers| {
        build_with(column, ctx, start_y, end_y, -3, &layers);
    };
    match other {
        Other::Lowlands => {
            let noise = ctx
                .noises
                .biome_lowlands
                .get(ctx.original_biome as usize)
                .and_then(Option::as_ref)
                .unwrap_or(&ctx.noises.lowlands)
                .noise(x, z) as f32
                * 0.9f32
                + ctx.random_float() * 0.1f32;
            let mud_state = surface_type(SoilType::Mud, RockBlock::Gravel);
            let mud = if ctx.ground_water() < 130.0 {
                dry_mud()
            } else {
                mud_state
            };
            normal(
                column,
                ctx,
                Layers::five(
                    if noise < 0.0 {
                        top_grass_to_gravel()
                    } else {
                        mud.clone()
                    },
                    mud.clone(),
                    mid_dirt_to_gravel(),
                    if noise > 0.0 {
                        mid_dirt_to_gravel()
                    } else {
                        mud.clone()
                    },
                    mud,
                ),
            );
        }
        Other::Burren => {
            let layers = if ctx.noises.burren.noise(x, z) + 0.3 * weight <= 0.40 {
                Layers::three(top_grass_to_gravel(), raw(), raw())
            } else {
                Layers::three(snowable(raw()), raw(), raw())
            };
            rocky(column, ctx, layers);
        }
        Other::StoneCircles => {
            let layers = if ctx.noises.stone_circles.noise(x, z) * weight <= 0.60 {
                Layers::three(
                    snowable(State::SandAndGravel),
                    State::SandAndGravel,
                    gravel(),
                )
            } else {
                Layers::three(
                    snowable(State::Rock(RockBlock::Cobble)),
                    State::Moraine,
                    gravel(),
                )
            };
            rocky(column, ctx, layers);
        }
        Other::FlatsMuddy | Other::FlatsSalty => {
            if start_y < 66 && ctx.base_groundwater < 25.0 {
                let top = if other == Other::FlatsSalty {
                    salted_earth()
                } else {
                    dry_mud()
                };
                let water = surface_type(SoilType::Mud, RockBlock::Gravel);
                normal(
                    column,
                    ctx,
                    Layers::five(top, dry_mud(), dry_mud(), water.clone(), water),
                );
            } else {
                normal(
                    column,
                    ctx,
                    Layers::three(top_grass_to_sand(), mid_dirt_to_sand(), under_type()),
                );
            }
        }
        Other::Patterned => {
            if ctx.noises.patterned.noise(x, z) * weight >= -0.60 {
                normal(column, ctx, Layers::grass());
            } else {
                let mud = || surface_type(SoilType::Mud, RockBlock::Gravel);
                normal(
                    column,
                    ctx,
                    Layers::five(mud(), mud(), gravel(), mud(), mud()),
                );
            }
        }
        Other::Shilin => {
            let val = ctx.noises.shilin.noise(x, z);
            let layers = if val > 0.18 {
                Layers::three(snowable(raw()), raw(), raw())
            } else if val > 0.09 {
                Layers::three(snowable(gravel()), gravel(), gravel())
            } else {
                Layers::grass()
            };
            rocky(column, ctx, layers);
        }
        Other::SandyAlternate => normal(
            column,
            ctx,
            Layers::three(top_grass_to_sand(), mid_dirt_to_sand(), under_type()),
        ),
        Other::RockyPlateau => {
            let layers = if weight > 0.9 && start_y < 86 && ctx.base_groundwater == 0.0 {
                Layers::three(salted_earth(), dry_mud(), raw())
            } else if f64::from(start_y - 2) > ctx.noises.hills.noise(x, z) {
                Layers::three(raw(), raw(), raw())
            } else {
                Layers::three(top_grass_to_sand(), mid_dirt_to_sand(), raw())
            };
            rocky(column, ctx, layers);
        }
        Other::Badlands(variant) => {
            let noises = ctx.noises;
            let badlands = noises
                .biome_badlands
                .get(ctx.original_biome as usize)
                .and_then(Option::as_ref)
                .unwrap_or(&noises.badlands[variant as usize]);
            badlands.build(column, ctx, start_y, end_y);
        }
        Other::Dune => {
            ctx.slope *= 1.0 - weight;
            normal(
                column,
                ctx,
                Layers::five(snowable(sand()), sand(), sand(), sand(), sand()),
            );
        }
        Other::GrassyDunes => {
            let height_variation = ctx
                .noises
                .biome_grassy_dunes
                .get(ctx.original_biome as usize)
                .and_then(Option::as_ref)
                .unwrap_or(&ctx.noises.grassy_dunes)
                .noise(x, z);
            let true_slope = ctx.slope;
            ctx.slope = true_slope * (1.0 - weight);
            let top = if f64::from(start_y) > height_variation && true_slope < 5.0 {
                top_grass_to_sand()
            } else {
                snowable(sand())
            };
            normal(
                column,
                ctx,
                Layers::five(top, sand(), sand(), sand(), sand()),
            );
        }
    }
}

/// `SurfaceManager.sampleSlope`: the slope at a block from the 6x6 corner map.
pub fn sample_slope(slope_map: &[f64; 36], x: i32, z: i32) -> f64 {
    let (offset_x, offset_z) = (x + 2, z + 2);
    let (cell_x, cell_z) = (offset_x >> 2, offset_z >> 2);
    let delta_x = f64::from(offset_x - (cell_x << 2)) * 0.25;
    let delta_z = f64::from(offset_z - (cell_z << 2)) * 0.25;
    let at = |dx: i32, dz: i32| slope_map[((cell_x + dx) + 6 * (cell_z + dz)) as usize];
    let mut slope = 0.0;
    slope += at(0, 0) * (1.0 - delta_x) * (1.0 - delta_z);
    slope += at(1, 0) * delta_x * (1.0 - delta_z);
    slope += at(0, 1) * (1.0 - delta_x) * delta_z;
    slope += at(1, 1) * delta_x * delta_z;
    slope * f64::from(0.8f32)
}

/// `ChunkNoiseFiller.createSlopeMap` from the 7x7 grid of quart-corner heights.
pub fn slope_map(quart_heights: &[i32; 49]) -> [f64; 36] {
    let mut map = [0.0; 36];
    for x in 0..6usize {
        for z in 0..6usize {
            let nw = f64::from(quart_heights[x + 7 * z]);
            let ne = f64::from(quart_heights[x + 1 + 7 * z]);
            let sw = f64::from(quart_heights[x + 7 * (z + 1)]);
            let se = f64::from(quart_heights[x + 1 + 7 * (z + 1)]);
            let center = (nw + ne + sw + se) / 4.0;
            map[x + 6 * z] = (nw - center).abs()
                + (ne - center).abs()
                + (sw - center).abs()
                + (se - center).abs();
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch() -> Noises {
        Noises::new(1234)
    }

    fn ctx<'a>(
        rock: &'a mut dyn FnMut(i32) -> &'static str,
        patch: &'a Noises,
        temp: f32,
        rain: f32,
        slope: f64,
    ) -> SurfaceCtx<'a> {
        SurfaceCtx {
            lava_sample: None,
            x: 10,
            y: 70,
            z: 12,
            rock,
            temperature: temp,
            base_groundwater: 0.0,
            rainfall: rain,
            rain_variance: 0.0,
            slope,
            noises: patch,
            sea_level_rock: "granite",
            original_biome: 0,
            tide: 63.0,
            weight: 1.0,
        }
    }

    fn info() -> BuildInfo {
        BuildInfo {
            ocean_floor: 0,
            original: None,
            original_is_shore: false,
            sandy_river_shores: false,
        }
    }

    fn column(height: i32) -> Column {
        let min_y = -64;
        let cells = (min_y..=100)
            .map(|y| {
                if y <= height {
                    Cell::Rock
                } else if y <= SEA_LEVEL {
                    Cell::Water
                } else {
                    Cell::Air
                }
            })
            .collect();
        Column { min_y, cells }
    }

    #[test]
    fn temperate_flat_land_is_grass_over_dirt() {
        let patch = patch();
        let mut rock = |_: i32| "granite";
        let mut c = ctx(&mut rock, &patch, 12.0, 300.0, 0.0);
        let mut col = column(70);
        build(&Builder::Normal, &mut col, &mut c, &info(), 71, 50);
        // 12 C is below 16 C: the cold soil variant, entisol.
        assert_eq!(col.get(70), &Cell::Placed("tfc:grass/entisol".into()));
        assert_eq!(col.get(69), &Cell::Placed("tfc:dirt/entisol".into()));
        assert_eq!(col.get(40), &Cell::Rock);
    }

    #[test]
    fn soil_variant_follows_temperature_and_flooding_rivers() {
        let patch = patch();
        let mut rock = |_: i32| "granite";
        let top = |temperature: f32, groundwater: f32, variance: f32| {
            let mut rock = |_: i32| "granite";
            let mut c = ctx(&mut rock, &patch, temperature, 300.0, 0.0);
            c.base_groundwater = groundwater;
            c.rain_variance = variance;
            state_at(&State::TransitioningSoil(SoilType::Grass), &mut c)
        };
        let _ = &mut rock;
        assert_eq!(top(10.0, 0.0, 0.0), "tfc:grass/entisol");
        assert_eq!(top(20.0, 0.0, 0.0), "tfc:grass/oxisol");
        // Beside a flooding river (groundwater over 25, rain variance beyond 0.5): silt.
        assert_eq!(top(10.0, 30.0, 0.8), "tfc:grass/fluvisol");
        assert_eq!(top(10.0, 30.0, 0.2), "tfc:grass/entisol");
        // Between 16 and 16.7 C it is a patchwork of the two.
        let mixed = top(16.3, 0.0, 0.0);
        assert!(mixed == "tfc:grass/entisol" || mixed == "tfc:grass/oxisol");
    }

    #[test]
    fn volcanic_soil_is_andisol() {
        let mut rock = |_: i32| "basalt";
        let patch = patch();
        let mut c = ctx(&mut rock, &patch, 20.0, 300.0, 0.0);
        let State::Regions(r) = volcanic_surface_type(SoilType::Grass, RockBlock::Gravel) else {
            panic!("regions")
        };
        assert_eq!(r.len(), 18);
        assert_eq!(state_at(&r[10], &mut c), "tfc:grass/andisol");
        assert_eq!(state_at(&r[5], &mut c), "tfc:coarse_dirt/andisol");
        let State::Regions(m) = surface_type(SoilType::Grass, RockBlock::Gravel) else {
            panic!("regions")
        };
        assert_eq!(m.len(), 18);
        assert_eq!(state_at(&m[7], &mut c), "tfc:grass/aridisol");
        assert_eq!(state_at(&m[5], &mut c), "tfc:coarse_dirt/aridisol");
    }

    #[test]
    fn steep_slopes_lose_their_soil() {
        let patch = patch();
        let mut rock = |_: i32| "granite";
        let mut c = ctx(&mut rock, &patch, 12.0, 300.0, 60.0);
        let mut col = column(70);
        build(&Builder::Normal, &mut col, &mut c, &info(), 71, 50);
        let top = col.get(70).clone();
        assert_ne!(top, Cell::Placed("tfc:grass/entisol".into()));
    }

    #[test]
    fn deep_water_floor_is_gravel_of_the_local_rock() {
        let patch = patch();
        let mut rock = |_: i32| "basalt";
        let mut c = ctx(&mut rock, &patch, 12.0, 300.0, 0.0);
        let mut col = column(40);
        build(&Builder::Normal, &mut col, &mut c, &info(), 64, 20);
        assert_eq!(col.get(40), &Cell::Placed("tfc:gravel_basalt".into()));
    }

    #[test]
    fn a_beach_column_is_sand_and_deep_water_is_mud() {
        let noises = patch();
        let sandy = Builder::from_name("ShoreAndOceanSurfaceBuilder.SANDY").unwrap();
        let mut rock = |_: i32| "granite";
        let mut c = ctx(&mut rock, &noises, 25.0, 300.0, 0.0);
        let mut col = column(64);
        let beach = BuildInfo {
            ocean_floor: 64,
            ..info()
        };
        build(&sandy, &mut col, &mut c, &beach, 65, 40);
        assert!(
            matches!(col.get(64), Cell::Placed(n) if n.contains("sand")),
            "{:?}",
            col.get(64)
        );
        let mut c = ctx(&mut rock, &noises, 25.0, 300.0, 0.0);
        let mut deep = column(30);
        let ocean = Builder::from_name("ShoreAndOceanSurfaceBuilder.OCEAN").unwrap();
        build(
            &ocean,
            &mut deep,
            &mut c,
            &BuildInfo {
                ocean_floor: 30,
                ..info()
            },
            63,
            10,
        );
        assert_eq!(deep.get(30), &Cell::Placed("tfc:mud/fluvisol".into()));
    }

    #[test]
    fn altitude_depth_shrinks_on_high_ground() {
        assert_eq!(altitude_slope_depth(70, 0.0, 0, 5), 5);
        assert_eq!(altitude_slope_depth(140, 0.0, 0, 5), 2);
    }

    #[test]
    fn hash_matches_a_hand_computed_value() {
        // 729375982 ^ (3 * 501125321) ^ (4 * 1136930381) ^ 5, times 0x27d4eb2d, truncated to 32 bits.
        let expected = {
            let v: i64 = 729_375_982 ^ (3 * 501_125_321) ^ (4 * 1_136_930_381) ^ 5;
            v.wrapping_mul(0x27d4_eb2d) as i32
        };
        assert_eq!(hash(729_375_982, 3, 4, 5), expected);
    }

    fn top_of(builder: &Builder, temp: f32, rain: f32, slope: f64) -> Cell {
        let patch = patch();
        let mut rock = |_: i32| "granite";
        let mut c = ctx(&mut rock, &patch, temp, rain, slope);
        let mut col = column(70);
        build(builder, &mut col, &mut c, &info(), 71, 50);
        col.get(70).clone()
    }

    #[test]
    fn sand_takes_the_rocks_colour_and_rare_beaches_follow_climate() {
        // `rock_settings`: basalt black, limestone white, shale brown, tuff green, granite yellow.
        assert_eq!(rock_sand_colour("basalt"), "black");
        assert_eq!(rock_sand_colour("limestone"), "white");
        assert_eq!(rock_sand_colour("shale"), "brown");
        assert_eq!(rock_sand_colour("tuff"), "green");
        assert_eq!(rock_sand_colour("granite"), "yellow");
        assert_eq!(rock_sand_colour("andesite"), "red");
        assert!(is_mafic("gabbro") && !is_mafic("granite"));
        let patch = patch();
        let mut rock = |_: i32| "granite";
        let mut hot_wet = ctx(&mut rock, &patch, 20.0, 400.0, 0.0);
        hot_wet.sea_level_rock = "granite";
        assert_eq!(rare_shore(&hot_wet, false), "tfc:sand/pink");
        assert_eq!(rare_shore(&hot_wet, true), "tfc:raw_sandstone/pink");
        let mut rock = |_: i32| "basalt";
        let mut cold_wet = ctx(&mut rock, &patch, 5.0, 400.0, 0.0);
        cold_wet.sea_level_rock = "basalt";
        assert_eq!(rare_shore(&cold_wet, false), "tfc:sand/green");
        let mut dry = ctx(&mut rock, &patch, 5.0, 100.0, 0.0);
        dry.sea_level_rock = "basalt";
        assert_eq!(rare_shore(&dry, false), "tfc:sand/black");
        dry.sea_level_rock = "granite";
        assert_eq!(rare_shore(&dry, false), "tfc:sand/white");
    }

    #[test]
    fn dunes_are_sand_and_burrens_are_bare_rock_or_snow() {
        let dune = Builder::from_name("DuneSurfaceBuilder.INSTANCE").unwrap();
        assert_eq!(
            top_of(&dune, 15.0, 200.0, 0.0),
            Cell::Placed("tfc:sand/yellow".into())
        );
        assert_eq!(
            top_of(&dune, -30.0, 200.0, 0.0),
            Cell::Placed("minecraft:snow_block".into())
        );
    }

    #[test]
    fn rocky_plateau_and_shilin_expose_stone_on_steep_ground() {
        for name in [
            "ShilinSurfaceBuilder.INSTANCE",
            "RockyPlateauSurfaceBuilder.INSTANCE",
        ] {
            let builder = Builder::from_name(name).unwrap();
            let top = top_of(&builder, 12.0, 300.0, 14.0);
            // Bare rock stays `Rock`; gravel and cobble are placed as TFC rock blocks.
            match top {
                Cell::Rock => {}
                Cell::Placed(block) => assert!(block.starts_with("tfc:"), "{name}: {block}"),
                other => panic!("{name}: {other:?}"),
            }
        }
    }
}
