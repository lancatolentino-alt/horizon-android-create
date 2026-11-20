//! Placement modifiers that depend on random state and generation bounds.
//! Block-query modifiers and actual configured features remain separate work.
mod chain;
mod layer;
use super::block_predicate::{BlockPredicate, PredicateBlockResolver};
use super::{
    DecodeError, Object, anchor::WorldGenerationContext, biome_temperature::biome_info_noise,
    density::identifier, double, height_provider::HeightProvider, int_range, integer, number_text,
    optional, required, value_provider::IntProvider,
};
use crate::heightmap::HeightmapType;
pub use chain::{FeaturePlacementError, FeaturePlacer};
use horizon_core::random::{InvalidBound, RandomSource};
use serde_json::value::RawValue;
mod context;
pub use context::{LayerBlock, PlacementContext, PlacementQueryError};

#[derive(Clone, Debug)]
pub enum PlacementModifier {
    Biome,
    BlockPredicate(BlockPredicate),
    EnvironmentScan {
        upwards: bool,
        target: BlockPredicate,
        allowed: BlockPredicate,
        max_steps: i32,
    },
    Heightmap(HeightmapType),
    SurfaceRelative {
        heightmap: HeightmapType,
        min: i32,
        max: i32,
    },
    SurfaceWaterDepth(i32),
    Count(IntProvider),
    CountOnEveryLayer(IntProvider),
    NoiseBasedCount {
        ratio: i32,
        factor: f64,
        offset: f64,
    },
    NoiseThresholdCount {
        level: f64,
        below: i32,
        above: i32,
    },
    InSquare,
    Cuboid {
        xz_size: IntProvider,
        y_size: IntProvider,
        include_edges: bool,
        include_interior: bool,
    },
    Offset([IntProvider; 3]),
    HeightRange(HeightProvider),
    Rarity(i32),
    /// Passes only where the TFC biome at the position is one of these (`tfc_biome` ids).
    TfcBiome(Vec<i32>),
    /// TFC's `tfc:flat_enough`: moves down until a `(2r+1)^2` square of non-air, non-fluid blocks
    /// holds more than `flatness` of its cells.
    FlatEnough {
        flatness: f32,
        radius: i32,
        max_depth: i32,
        solid: BlockPredicate,
    },
    /// TFC's `tfc:no_solid_neighbors`: passes if any horizontal neighbour is not replaceable.
    TfcNoSolidNeighbors(BlockPredicate),
    /// TFC's `tfc:near_fluid`: passes if a fluid matching `fluids` is within `radius` at or below.
    TfcNearFluid {
        radius: i32,
        fluid: BlockPredicate,
    },
    /// TFC's `tfc:underground`: passes below the `WORLD_SURFACE_WG` height.
    TfcUnderground,
    /// TFC's `tfc:on_top`: passes where the predicate holds one block below.
    TfcOnTop(BlockPredicate),
    /// TFC's `tfc:shallow_water`: passes above 1 to `max_depth` blocks of water, thinning with depth.
    TfcShallowWater {
        min_depth: i32,
        max_depth: i32,
        water: BlockPredicate,
    },
    /// TFC's `tfc:intertidal`.
    TfcIntertidal {
        min: i32,
        max: i32,
    },
    /// TFC's `tfc:climate`: passes where the column's climate is inside every range.
    TfcClimate(Box<TfcClimate>),
    /// TFC's `tfc:volcano`, `tfc:tuff_ring`, `tfc:tuya`, `tfc:atoll` and `tfc:stratovolcano`.
    TfcCentered(Box<super::tfc_volcano_placement::CenterPlacement>),
    /// TFC's `tfc:carving_mask`: every cell of the chunk's carving mask between two heights.
    TfcCarvingMask {
        min_y: super::anchor::VerticalAnchor,
        max_y: super::anchor::VerticalAnchor,
        air: bool,
    },
    /// TFC's `tfc:ocean_ridge`.
    TfcOceanRidge {
        min: i32,
        max: i32,
    },
    RandomChance(f32),
    Fixed(Vec<[i32; 3]>),
    RandomlySelected(Vec<Self>),
}
/// `ClimatePlacement`.
#[derive(Clone, Debug)]
pub struct TfcClimate {
    min_temperature: f32,
    max_temperature: f32,
    min_groundwater: f32,
    max_groundwater: f32,
    min_rain_variance: f32,
    max_rain_variance: f32,
    rain_variance_absolute: bool,
    min_forest: i32,
    max_forest: i32,
    forest_types: Vec<String>,
    min_elevation: i32,
    max_elevation: i32,
    fuzzy: bool,
    ignore_rivers: bool,
}

impl TfcClimate {
    fn decode(object: &Object) -> Result<Self, DecodeError> {
        let float = |name: &'static str, default: f32| -> Result<f32, DecodeError> {
            optional(object, name)
                .map(|v| {
                    number_text(v)
                        .and_then(|t| t.parse().map_err(|_| DecodeError::Field("climate")))
                })
                .transpose()
                .map(|v| v.unwrap_or(default))
        };
        let int = |name: &'static str, default: i32| -> Result<i32, DecodeError> {
            Ok(optional(object, name)
                .map(integer)
                .transpose()?
                .unwrap_or(default))
        };
        let flag = |name: &'static str| -> Result<bool, DecodeError> {
            Ok(optional(object, name)
                .map(|v| serde_json::from_str::<bool>(v.get()))
                .transpose()?
                .unwrap_or(false))
        };
        Ok(Self {
            min_temperature: float("min_temperature", f32::NEG_INFINITY)?,
            max_temperature: float("max_temperature", f32::INFINITY)?,
            min_groundwater: float("min_groundwater", f32::NEG_INFINITY)?,
            max_groundwater: float("max_groundwater", f32::INFINITY)?,
            min_rain_variance: float("min_rain_variance", -1.0)?,
            max_rain_variance: float("max_rain_variance", 1.0)?,
            rain_variance_absolute: flag("rain_variance_absolute")?,
            min_forest: int("min_forest", 0)?,
            max_forest: int("max_forest", 4)?,
            forest_types: optional(object, "forest_types")
                .map(|v| serde_json::from_str::<Vec<String>>(v.get()))
                .transpose()?
                .unwrap_or_default(),
            min_elevation: int("min_elevation", -64)?,
            max_elevation: int("max_elevation", 320)?,
            fuzzy: flag("fuzzy")?,
            ignore_rivers: flag("ignore_rivers")?,
        })
    }

    /// `ClimatePlacement.isValid`.
    fn is_valid<R: RandomSource>(
        &self,
        terrain: &super::tfc_terrain::TfcTerrain,
        random: &mut R,
        [x, y, z]: [i32; 3],
    ) -> Result<bool, InvalidBound> {
        let sample = terrain.climate_at(x, z);
        // `SolarCalculator.getInNorthernHemisphere`: southern hemispheres flip the rain variance.
        let scale = terrain.hemisphere_scale();
        let north = if scale == 0.0 {
            true
        } else {
            let adjusted = z.wrapping_sub((scale / 2.0) as i32);
            let pole_to_pole = (scale * 2.0) as i32;
            adjusted.rem_euclid(pole_to_pole * 2) > pole_to_pole
        };
        let rain_variance = if self.rain_variance_absolute {
            sample.rain_variance.abs()
        } else {
            sample.rain_variance * if north { 1.0 } else { -1.0 }
        };
        // `EnvironmentHelpers.adjustAvgTempForElev`.
        let temperature = if y > 63 {
            sample.sea_level_temperature - ((y - 63) as f32 * 0.16225).clamp(0.0, 17.822)
        } else {
            sample.sea_level_temperature
        };
        let groundwater = if self.ignore_rivers {
            sample.rainfall
        } else {
            sample.groundwater
        };
        let density = sample.forest.data().density;
        if !(y >= self.min_elevation
            && y <= self.max_elevation
            && self.min_temperature <= temperature
            && temperature <= self.max_temperature
            && self.min_groundwater <= groundwater
            && groundwater <= self.max_groundwater
            && self.min_rain_variance <= rain_variance
            && self.max_rain_variance >= rain_variance
            && self.min_forest <= density
            && density <= self.max_forest
            && (self.forest_types.is_empty()
                || self.forest_types.iter().any(|t| t == sample.forest.name())))
        {
            return Ok(false);
        }
        if self.fuzzy {
            let target_t = (self.min_temperature + self.max_temperature) / 2.0;
            let target_g = (self.min_groundwater + self.max_groundwater) / 2.0;
            let target_r = (self.min_rain_variance + self.max_rain_variance) / 2.0;
            let dt = (temperature - target_t).abs() / (self.max_temperature - self.min_temperature);
            let dg = (groundwater - target_g).abs() / (self.max_groundwater - self.min_groundwater);
            let dr = (rain_variance - target_r).abs()
                / (self.max_rain_variance - self.min_rain_variance);
            let a = random.next_float();
            let b = random.next_float();
            return Ok(a * b > dt.max(dg.max(dr)));
        }
        Ok(true)
    }
}

#[derive(Debug)]
pub enum PlacementError<E> {
    Random(InvalidBound),
    World(PlacementQueryError),
    Output(E),
    WorkLimit,
}
impl<E: std::fmt::Display> std::fmt::Display for PlacementError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Random(e) => e.fmt(f),
            Self::World(e) => e.fmt(f),
            Self::Output(e) => e.fmt(f),
            Self::WorkLimit => f.write_str("placement work limit exceeded"),
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for PlacementError<E> {}
impl<E> From<InvalidBound> for PlacementError<E> {
    fn from(value: InvalidBound) -> Self {
        Self::Random(value)
    }
}
impl<E> From<PlacementQueryError> for PlacementError<E> {
    fn from(value: PlacementQueryError) -> Self {
        Self::World(value)
    }
}
impl PlacementModifier {
    pub fn from_json(json: &str) -> Result<Self, DecodeError> {
        Self::decode(json, 0, &mut 65536)
    }
    pub(crate) fn decode(
        json: &str,
        depth: usize,
        remaining: &mut usize,
    ) -> Result<Self, DecodeError> {
        Self::decode_with_blocks(json, depth, remaining, None)
    }
    pub fn from_json_with_blocks(
        json: &str,
        blocks: &impl PredicateBlockResolver,
    ) -> Result<Self, DecodeError> {
        Self::decode_with_blocks(json, 0, &mut 65536, Some(blocks))
    }
    pub(crate) fn decode_with_blocks(
        json: &str,
        depth: usize,
        remaining: &mut usize,
        blocks: Option<&dyn PredicateBlockResolver>,
    ) -> Result<Self, DecodeError> {
        if depth > 256 || *remaining == 0 {
            return Err(DecodeError::Field("placement depth or node count"));
        }
        *remaining -= 1;
        let object: Object = serde_json::from_str(json)?;
        let kind = identifier(&serde_json::from_str::<String>(
            required(&object, "type")?.get(),
        )?)?;
        let provider = |name, min, max| {
            let value = IntProvider::from_json(required(&object, name)?.get())?;
            let bounds = value.bounds();
            if bounds[0] < min || bounds[1] > max {
                return Err(DecodeError::Field("placement provider bounds"));
            }
            Ok(value)
        };
        let heightmap = || {
            let name: String = serde_json::from_str(required(&object, "heightmap")?.get())?;
            HeightmapType::ALL
                .into_iter()
                .find(|kind| kind.name() == name)
                .ok_or(DecodeError::Field("heightmap"))
        };
        match kind.as_str() {
            "minecraft:biome" => Ok(Self::Biome),
            "minecraft:block_predicate_filter" => {
                Ok(Self::BlockPredicate(BlockPredicate::decode_with_blocks(
                    required(&object, "predicate")?.get(),
                    depth + 1,
                    remaining,
                    blocks,
                )?))
            }
            "minecraft:environment_scan" => {
                let direction: String =
                    serde_json::from_str(required(&object, "direction_of_search")?.get())?;
                let upwards = match direction.as_str() {
                    "up" => true,
                    "down" => false,
                    _ => return Err(DecodeError::Field("scan direction")),
                };
                Ok(Self::EnvironmentScan {
                    upwards,
                    target: BlockPredicate::decode_with_blocks(
                        required(&object, "target_condition")?.get(),
                        depth + 1,
                        remaining,
                        blocks,
                    )?,
                    allowed: optional(&object, "allowed_search_condition")
                        .map(|v| {
                            BlockPredicate::decode_with_blocks(
                                v.get(),
                                depth + 1,
                                remaining,
                                blocks,
                            )
                        })
                        .transpose()?
                        .unwrap_or(BlockPredicate::True),
                    max_steps: int_range(required(&object, "max_steps")?, "scan steps", 1, 32)?,
                })
            }
            "minecraft:heightmap" => Ok(Self::Heightmap(heightmap()?)),
            "minecraft:surface_relative_threshold_filter" => Ok(Self::SurfaceRelative {
                heightmap: heightmap()?,
                min: optional(&object, "min_inclusive")
                    .map(integer)
                    .transpose()?
                    .unwrap_or(i32::MIN),
                max: optional(&object, "max_inclusive")
                    .map(integer)
                    .transpose()?
                    .unwrap_or(i32::MAX),
            }),
            "minecraft:surface_water_depth_filter" => Ok(Self::SurfaceWaterDepth(integer(
                required(&object, "max_water_depth")?,
            )?)),
            "minecraft:noise_based_count" => Ok(Self::NoiseBasedCount {
                ratio: integer(required(&object, "noise_to_count_ratio")?)?,
                factor: double(required(&object, "noise_factor")?)?,
                offset: optional(&object, "noise_offset")
                    .map(double)
                    .transpose()?
                    .unwrap_or(0.),
            }),
            "minecraft:noise_threshold_count" => Ok(Self::NoiseThresholdCount {
                level: double(required(&object, "noise_level")?)?,
                below: integer(required(&object, "below_noise")?)?,
                above: integer(required(&object, "above_noise")?)?,
            }),
            "minecraft:count_on_every_layer" => {
                Ok(Self::CountOnEveryLayer(provider("count", 0, 256)?))
            }
            "minecraft:count" => Ok(Self::Count(provider("count", 0, 4096)?)),
            "minecraft:in_square" => Ok(Self::InSquare),
            "minecraft:cuboid" => Ok(Self::Cuboid {
                xz_size: provider("xz_size", 1, 16)?,
                y_size: provider("y_size", 1, 16)?,
                include_edges: optional(&object, "include_edges")
                    .map(|raw| serde_json::from_str(raw.get()))
                    .transpose()?
                    .unwrap_or(true),
                include_interior: optional(&object, "include_interior")
                    .map(|raw| serde_json::from_str(raw.get()))
                    .transpose()?
                    .unwrap_or(true),
            }),
            "minecraft:offset" => Ok(Self::Offset([
                provider("x", -16, 16)?,
                provider("y", -16, 16)?,
                provider("z", -16, 16)?,
            ])),
            "minecraft:height_range" => Ok(Self::HeightRange(HeightProvider::from_json(
                required(&object, "height")?.get(),
            )?)),
            "tfc:biome" => {
                let names: Vec<String> = serde_json::from_str(required(&object, "biomes")?.get())?;
                let mut ids = Vec::with_capacity(names.len());
                for name in &names {
                    let id = super::tfc_biome::BIOME_NAMES
                        .iter()
                        .position(|n| n == name)
                        .ok_or(DecodeError::Field("tfc biome name"))?;
                    ids.push(id as i32);
                }
                Ok(Self::TfcBiome(ids))
            }
            "tfc:no_solid_neighbors" => Ok(Self::TfcNoSolidNeighbors(
                BlockPredicate::decode_with_blocks(
                    r#"{"type":"minecraft:replaceable"}"#,
                    0,
                    &mut 4096,
                    blocks,
                )?,
            )),
            "tfc:near_fluid" => {
                let radius = optional(&object, "radius")
                    .map(integer)
                    .transpose()?
                    .unwrap_or(2);
                if radius < 0 {
                    return Err(DecodeError::Field("near fluid radius"));
                }
                let json = match optional(&object, "fluids") {
                    Some(fluids) => format!(
                        r#"{{"type":"minecraft:matching_fluids","fluids":{}}}"#,
                        fluids.get()
                    ),
                    None => r#"{"type":"minecraft:not","predicate":{"type":"minecraft:matching_fluids","fluids":"minecraft:empty"}}"#
                        .to_owned(),
                };
                Ok(Self::TfcNearFluid {
                    radius,
                    fluid: BlockPredicate::decode_with_blocks(&json, 0, &mut 4096, blocks)?,
                })
            }
            "tfc:underground" => Ok(Self::TfcUnderground),
            "tfc:on_top" => Ok(Self::TfcOnTop(BlockPredicate::decode_with_blocks(
                required(&object, "predicate")?.get(),
                depth + 1,
                remaining,
                blocks,
            )?)),
            "tfc:shallow_water" => {
                let depth_of = |name: &'static str, default: i32| -> Result<i32, DecodeError> {
                    Ok(optional(&object, name)
                        .map(integer)
                        .transpose()?
                        .unwrap_or(default))
                };
                let (min_depth, max_depth) = (depth_of("min_depth", 0)?, depth_of("max_depth", 5)?);
                if min_depth < 0 || max_depth < 0 {
                    return Err(DecodeError::Field("shallow water depth"));
                }
                Ok(Self::TfcShallowWater {
                    min_depth,
                    max_depth,
                    // `#tfc:any_infinite_water`: fresh and salt water, both `minecraft:water` here.
                    water: BlockPredicate::decode_with_blocks(
                        r#"{"type":"minecraft:matching_fluids","fluids":["minecraft:water"]}"#,
                        0,
                        &mut 4096,
                        blocks,
                    )?,
                })
            }
            "tfc:intertidal" => Ok(Self::TfcIntertidal {
                min: optional(&object, "min_elevation")
                    .map(integer)
                    .transpose()?
                    .unwrap_or(-64),
                max: optional(&object, "max_elevation")
                    .map(integer)
                    .transpose()?
                    .unwrap_or(320),
            }),
            "tfc:climate" => Ok(Self::TfcClimate(Box::new(TfcClimate::decode(&object)?))),
            "tfc:volcano" | "tfc:tuff_ring" | "tfc:tuya" | "tfc:atoll" | "tfc:stratovolcano" => {
                use super::tfc_volcano::CenterKind;
                let float = |name: &'static str, default: f32| -> Result<f32, DecodeError> {
                    optional(&object, name)
                        .map(|v| {
                            number_text(v).and_then(|t| {
                                t.parse()
                                    .map_err(|_| DecodeError::Field("centered feature"))
                            })
                        })
                        .transpose()
                        .map(|v| v.unwrap_or(default))
                };
                let flag = |name: &'static str| -> Result<bool, DecodeError> {
                    Ok(optional(&object, name)
                        .map(|v| serde_json::from_str::<bool>(v.get()))
                        .transpose()?
                        .unwrap_or(false))
                };
                let variant = optional(&object, "variant")
                    .map(|v| serde_json::from_str::<String>(v.get()))
                    .transpose()?
                    .filter(|v| v != "all");
                Ok(Self::TfcCentered(Box::new(
                    super::tfc_volcano_placement::CenterPlacement {
                        kind: match kind.as_str() {
                            "tfc:volcano" => CenterKind::Cinder,
                            "tfc:tuff_ring" => CenterKind::TuffRing,
                            "tfc:tuya" => CenterKind::Tuya,
                            "tfc:atoll" => CenterKind::Atoll,
                            _ => CenterKind::Stratovolcano,
                        },
                        center: flag("center")?,
                        use_offset_center: flag("use_offset_center")?,
                        variant,
                        min_easing: float("min_easing", 0.0)?,
                        max_easing: float("max_easing", 1.0)?,
                        hash_min: float("hash_min", 0.0)?,
                        hash_max: float("hash_max", 1.0)?,
                    },
                )))
            }
            "tfc:carving_mask" => {
                let anchor =
                    |name: &'static str,
                     default: super::anchor::VerticalAnchor|
                     -> Result<super::anchor::VerticalAnchor, DecodeError> {
                        optional(&object, name)
                            .map(|v| super::anchor::VerticalAnchor::from_json(v.get()))
                            .transpose()
                            .map(|v| v.unwrap_or(default))
                    };
                let step: String = serde_json::from_str(required(&object, "step")?.get())?;
                Ok(Self::TfcCarvingMask {
                    min_y: anchor("min_y", super::anchor::VerticalAnchor::BOTTOM)?,
                    max_y: anchor("max_y", super::anchor::VerticalAnchor::TOP)?,
                    air: match step.as_str() {
                        "air" => true,
                        "liquid" => false,
                        _ => return Err(DecodeError::Field("carving step")),
                    },
                })
            }
            "tfc:ocean_ridge" => Ok(Self::TfcOceanRidge {
                min: optional(&object, "min_distance")
                    .map(integer)
                    .transpose()?
                    .unwrap_or(0),
                max: optional(&object, "max_distance")
                    .map(integer)
                    .transpose()?
                    .unwrap_or(i32::MAX),
            }),
            "tfc:flat_enough" => {
                let flatness: f32 = optional(&object, "flatness")
                    .map(|v| {
                        number_text(v)
                            .and_then(|t| t.parse().map_err(|_| DecodeError::Field("flatness")))
                    })
                    .transpose()?
                    .unwrap_or(0.5);
                if !(0. ..=1.).contains(&flatness) {
                    return Err(DecodeError::Field("flatness"));
                }
                // `!stateAt.isAir() && stateAt.getFluidState().getType() == Fluids.EMPTY`
                let solid = BlockPredicate::decode_with_blocks(
                    r#"{"type":"minecraft:all_of","predicates":[
                    {"type":"minecraft:not","predicate":{"type":"minecraft:matching_blocks","blocks":["minecraft:air","minecraft:cave_air","minecraft:void_air"]}},
                    {"type":"minecraft:matching_fluids","fluids":"minecraft:empty"}]}"#,
                    0,
                    &mut 4096,
                    blocks,
                )?;
                Ok(Self::FlatEnough {
                    flatness,
                    radius: optional(&object, "radius")
                        .map(integer)
                        .transpose()?
                        .unwrap_or(2)
                        .max(1),
                    max_depth: optional(&object, "max_depth")
                        .map(integer)
                        .transpose()?
                        .unwrap_or(4)
                        .max(1),
                    solid,
                })
            }
            "minecraft:rarity_filter" => Ok(Self::Rarity(int_range(
                required(&object, "chance")?,
                "chance",
                1,
                i32::MAX,
            )?)),
            "minecraft:random_chance" => {
                let chance: f32 = number_text(required(&object, "chance")?)?
                    .parse()
                    .map_err(|_| DecodeError::Field("chance"))?;
                if !(0. ..=1.).contains(&chance) || chance.is_sign_negative() {
                    return Err(DecodeError::Field("chance"));
                }
                Ok(Self::RandomChance(chance))
            }
            "minecraft:fixed_placement" => {
                let values: Vec<Box<RawValue>> =
                    serde_json::from_str(required(&object, "positions")?.get())?;
                if values.len() > 8192 {
                    return Err(DecodeError::Field("fixed positions"));
                }
                let mut positions = Vec::with_capacity(values.len());
                for raw in values {
                    let coords: Vec<Box<RawValue>> = serde_json::from_str(raw.get())?;
                    if coords.len() != 3 {
                        return Err(DecodeError::Field("block position"));
                    }
                    positions.push([
                        integer(&coords[0])?,
                        integer(&coords[1])?,
                        integer(&coords[2])?,
                    ]);
                }
                Ok(Self::Fixed(positions))
            }
            "minecraft:randomly_selected" => {
                let values: Vec<Box<RawValue>> =
                    serde_json::from_str(required(&object, "placements")?.get())?;
                if values.is_empty() || values.len() > 8192 {
                    return Err(DecodeError::Field("selected placements"));
                }
                Ok(Self::RandomlySelected(
                    values
                        .iter()
                        .map(|v| Self::decode_with_blocks(v.get(), depth + 1, remaining, blocks))
                        .collect::<Result<_, _>>()?,
                ))
            }
            _ => Err(DecodeError::Field("placement modifier type")),
        }
    }
    /// Return the count for repeating modifiers without allocating or emitting
    /// positions. Sampling still consumes the count provider's random draws.
    pub fn repetitions(
        &self,
        random: &mut impl RandomSource,
        position: [i32; 3],
    ) -> Result<Option<i32>, InvalidBound> {
        let [x, _, z] = position;
        Ok(match self {
            Self::Count(provider) => Some(provider.sample(random)?),
            Self::NoiseBasedCount {
                ratio,
                factor,
                offset,
            } => {
                let noise = f64::from(biome_info_noise(
                    f64::from(x) / factor,
                    f64::from(z) / factor,
                ));
                Some(((noise + offset) * f64::from(*ratio)).ceil() as i32)
            }
            Self::NoiseThresholdCount {
                level,
                below,
                above,
            } => {
                let noise = f64::from(biome_info_noise(f64::from(x) / 200., f64::from(z) / 200.));
                Some(if noise < *level { *below } else { *above })
            }
            _ => None,
        })
    }
    /// Emit positions in native callback order. The callback shares the random
    /// stream; failures retain prior emissions and consumed draws.
    pub fn modify<R: RandomSource, E>(
        &self,
        mut context: WorldGenerationContext,
        random: &mut R,
        position: [i32; 3],
        emit: impl FnMut(&mut R, [i32; 3]) -> Result<(), E>,
    ) -> Result<(), PlacementError<E>> {
        self.modify_in(&mut context, random, position, emit)
    }
    pub fn modify_in<R: RandomSource, E>(
        &self,
        context: &mut impl PlacementContext,
        random: &mut R,
        position: [i32; 3],
        mut emit: impl FnMut(&mut R, [i32; 3]) -> Result<(), E>,
    ) -> Result<(), PlacementError<E>> {
        self.modify_inner(context, random, position, &mut emit, &mut 65536)
    }
    fn modify_inner<R: RandomSource, E>(
        &self,
        context: &mut impl PlacementContext,
        random: &mut R,
        position: [i32; 3],
        emit: &mut impl FnMut(&mut R, [i32; 3]) -> Result<(), E>,
        remaining: &mut usize,
    ) -> Result<(), PlacementError<E>> {
        if *remaining == 0 {
            return Err(PlacementError::WorkLimit);
        }
        *remaining -= 1;
        let [x, y, z] = position;
        match self {
            Self::Biome => {
                let feature = context
                    .top_feature()
                    .ok_or(PlacementQueryError::Unavailable)?;
                if context.biome_allows_feature(position, feature)? {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::BlockPredicate(predicate) => {
                if predicate.test_inner(context, position, 0, remaining)? {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::EnvironmentScan {
                upwards,
                target,
                allowed,
                max_steps,
            } => {
                let mut position = position;
                if !allowed.test_inner(context, position, 0, remaining)? {
                    return Ok(());
                }
                for _ in 0..*max_steps {
                    if target.test_inner(context, position, 0, remaining)? {
                        return emit(random, position).map_err(PlacementError::Output);
                    }
                    position[1] = position[1].wrapping_add(if *upwards { 1 } else { -1 });
                    if context.outside_build_height(position[1])? {
                        return Ok(());
                    }
                    if !allowed.test_inner(context, position, 0, remaining)? {
                        break;
                    }
                }
                if target.test_inner(context, position, 0, remaining)? {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::CountOnEveryLayer(count) => {
                layer::modify(count, context, random, position, emit, remaining)?
            }
            Self::Heightmap(kind) => {
                let height = context.height(*kind, x, z)?;
                if height > context.min_y()? {
                    emit(random, [x, height, z]).map_err(PlacementError::Output)?;
                }
            }
            Self::SurfaceRelative {
                heightmap,
                min,
                max,
            } => {
                let height = i64::from(context.height(*heightmap, x, z)?);
                if height + i64::from(*min) <= i64::from(y)
                    && i64::from(y) <= height + i64::from(*max)
                {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::SurfaceWaterDepth(max) => {
                let floor = context.height(HeightmapType::OceanFloor, x, z)?;
                let surface = context.height(HeightmapType::WorldSurface, x, z)?;
                if surface.wrapping_sub(floor) <= *max {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::Count(_) | Self::NoiseBasedCount { .. } | Self::NoiseThresholdCount { .. } => {
                let count = self
                    .repetitions(random, position)?
                    .expect("repeating modifier");
                if count > 4096 {
                    return Err(PlacementError::WorkLimit);
                }
                for _ in 0..count {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::Cuboid {
                xz_size,
                y_size,
                include_edges,
                include_interior,
            } => {
                let ny = y_size.sample(random)?;
                let nx = xz_size.sample(random)?;
                let nz = xz_size.sample(random)?;
                if nx < 0 || ny < 0 || nz < 0 {
                    return Ok(());
                }
                for dx in 0..=nx {
                    for dy in 0..=ny {
                        for dz in 0..=nz {
                            if *remaining == 0 {
                                return Err(PlacementError::WorkLimit);
                            }
                            *remaining -= 1;
                            let bx = dx == 0 || dx == nx;
                            let by = dy == 0 || dy == ny;
                            let bz = dz == 0 || dz == nz;
                            if !include_edges && (u8::from(bx) + u8::from(by) + u8::from(bz)) >= 2 {
                                continue;
                            }
                            if !(*include_interior || bx || by || bz) {
                                continue;
                            }
                            emit(
                                random,
                                [x.wrapping_add(dx), y.wrapping_add(dy), z.wrapping_add(dz)],
                            )
                            .map_err(PlacementError::Output)?;
                        }
                    }
                }
            }
            Self::InSquare => {
                let x = x.wrapping_add(random.next_int_bound(16)?);
                let z = z.wrapping_add(random.next_int_bound(16)?);
                emit(random, [x, y, z]).map_err(PlacementError::Output)?;
            }
            Self::Offset(offsets) => {
                let x = x.wrapping_add(offsets[0].sample(random)?);
                let y = y.wrapping_add(offsets[1].sample(random)?);
                let z = z.wrapping_add(offsets[2].sample(random)?);
                emit(random, [x, y, z]).map_err(PlacementError::Output)?;
            }
            Self::HeightRange(height) => {
                let y = height.sample(random, context.generation())?;
                emit(random, [x, y, z]).map_err(PlacementError::Output)?;
            }
            Self::TfcBiome(ids) => {
                let seed = context.world_seed()?;
                if let Some(terrain) = super::tfc_terrain::active_for(seed)
                    && ids.contains(&terrain.biome_at_quart(x >> 2, z >> 2))
                {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::TfcNoSolidNeighbors(replaceable) => {
                for [dx, dz] in [[0, -1], [0, 1], [-1, 0], [1, 0]] {
                    if !replaceable.test_inner(context, [x + dx, y, z + dz], 0, remaining)? {
                        return emit(random, position).map_err(PlacementError::Output);
                    }
                }
            }
            Self::TfcNearFluid { radius, fluid } => {
                for dx in -*radius..=*radius {
                    for dz in -*radius..=*radius {
                        for dy in -*radius..=0 {
                            if fluid.test_inner(context, [x + dx, y + dy, z + dz], 0, remaining)? {
                                return emit(random, position).map_err(PlacementError::Output);
                            }
                        }
                    }
                }
            }
            Self::TfcUnderground => {
                if y < context.height(HeightmapType::WorldSurfaceWg, x, z)? {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::TfcOnTop(predicate) => {
                if predicate.test_inner(context, [x, y - 1, z], 0, remaining)? {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::TfcShallowWater {
                min_depth,
                max_depth,
                water,
            } => {
                for i in 0..*max_depth {
                    if !water.test_inner(context, [x, y - 1 - i, z], 0, remaining)? {
                        if i < *min_depth {
                            return Ok(());
                        }
                        if f64::from(random.next_float()) > f64::from(i) / f64::from(*max_depth) {
                            emit(random, position).map_err(PlacementError::Output)?;
                        }
                        return Ok(());
                    }
                }
            }
            Self::TfcIntertidal { min, max } => {
                let seed = context.world_seed()?;
                if let Some(terrain) = super::tfc_terrain::active_for(seed) {
                    let diff = (f64::from(y) - terrain.tide_level(x, z)) as i32;
                    if *min <= diff && diff <= *max {
                        emit(random, position).map_err(PlacementError::Output)?;
                    }
                }
            }
            Self::TfcCentered(placement) => {
                let seed = context.world_seed()?;
                if let Some(terrain) = super::tfc_terrain::active_for(seed)
                    && let Some(found) = placement.position(&terrain, position)
                {
                    emit(random, found).map_err(PlacementError::Output)?;
                }
            }
            Self::TfcCarvingMask { min_y, max_y, air } => {
                // Only the air mask is ever marked; the liquid one stays empty.
                if *air {
                    let seed = context.world_seed()?;
                    if let Some(terrain) = super::tfc_terrain::active_for(seed) {
                        let generation = context.predicate_generation()?;
                        let (low, high) = (min_y.resolve(generation), max_y.resolve(generation));
                        for cell in terrain.air_carving_cells(x >> 4, z >> 4).iter() {
                            if cell[1] >= low && cell[1] <= high {
                                emit(random, *cell).map_err(PlacementError::Output)?;
                            }
                        }
                    }
                }
            }
            Self::TfcOceanRidge { min, max } => {
                let seed = context.world_seed()?;
                let distance = super::tfc_biome_noise::ocean_ridge_distance_at(seed, x, z);
                if f64::from(*min) <= distance && distance <= f64::from(*max) {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::TfcClimate(climate) => {
                let seed = context.world_seed()?;
                if let Some(terrain) = super::tfc_terrain::active_for(seed)
                    && climate.is_valid(&terrain, random, position)?
                {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::FlatEnough {
                flatness,
                radius,
                max_depth,
                solid,
            } => {
                let side = 1.0f32 + 2.0 * *radius as f32;
                for down in 0..*max_depth {
                    let mut flat = 0;
                    for dx in -*radius..=*radius {
                        for dz in -*radius..=*radius {
                            let at = [x + dx, y - down, z + dz];
                            if solid.test_inner(context, at, 0, remaining)? {
                                flat += 1;
                            }
                        }
                    }
                    if flat as f32 / (side * side) > *flatness {
                        return emit(random, [x, y - down, z]).map_err(PlacementError::Output);
                    }
                }
            }
            Self::Rarity(chance) => {
                if random.next_float() < 1. / *chance as f32 {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::RandomChance(chance) => {
                if random.next_float() < *chance {
                    emit(random, position).map_err(PlacementError::Output)?;
                }
            }
            Self::Fixed(positions) => {
                if positions.len() > 8192 {
                    return Err(PlacementError::WorkLimit);
                }
                for p in positions {
                    if p[0] >> 4 == x >> 4 && p[2] >> 4 == z >> 4 {
                        emit(random, *p).map_err(PlacementError::Output)?;
                    }
                }
            }
            Self::RandomlySelected(placements) => {
                if placements.len() > 8192 {
                    return Err(PlacementError::WorkLimit);
                }
                let index = random.next_int_bound(placements.len() as i32)? as usize;
                placements[index].modify_inner(context, random, position, emit, remaining)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tfc_climate_tests {
    use super::*;
    use horizon_core::random::XoroshiroRandom;

    #[test]
    fn climate_filter_follows_the_chunk_data_ranges() {
        let terrain = super::super::tfc_terrain::TfcTerrain::new(12345);
        let (x, z) = (200, -340);
        let s = terrain.climate_at(x, z);
        let near = |lo: f32, hi: f32| {
            format!(
                r#"{{"type":"tfc:climate","min_temperature":{},"max_temperature":{},"min_groundwater":{},"max_groundwater":{}}}"#,
                s.sea_level_temperature + lo,
                s.sea_level_temperature + hi,
                s.groundwater - 1.0,
                s.groundwater + 1.0
            )
        };
        let decode = |json: String| {
            let object: Object = serde_json::from_str(&json).unwrap();
            TfcClimate::decode(&object).unwrap()
        };
        let mut random = XoroshiroRandom::new(1);
        // At sea level the temperature is the layer's value, so a window around it passes...
        assert!(
            decode(near(-1.0, 1.0))
                .is_valid(&terrain, &mut random, [x, 63, z])
                .unwrap()
        );
        // ...a window above it does not...
        assert!(
            !decode(near(1.0, 2.0))
                .is_valid(&terrain, &mut random, [x, 63, z])
                .unwrap()
        );
        // ...and climbing 100 blocks cools the column by 16.225 degrees.
        assert!(
            !decode(near(-1.0, 1.0))
                .is_valid(&terrain, &mut random, [x, 163, z])
                .unwrap()
        );
    }
}

#[cfg(test)]
mod tfc_surface_placement_tests {
    use super::*;
    use crate::{
        registry::{BlockRegistry, BlockStateId},
        worldgen::block_feature::FeatureBlockBindings,
        worldgen::tfc_tide_pool::tests::Flat,
    };
    use horizon_core::random::XoroshiroRandom;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    struct Ctx(Flat);
    impl PlacementContext for Ctx {
        fn generation(&self) -> WorldGenerationContext {
            self.0.generation()
        }
        fn min_y(&mut self) -> Result<i32, PlacementQueryError> {
            self.0.min_y()
        }
        fn max_y(&mut self) -> Result<i32, PlacementQueryError> {
            self.0.max_y()
        }
        fn height(
            &mut self,
            kind: HeightmapType,
            x: i32,
            z: i32,
        ) -> Result<i32, PlacementQueryError> {
            self.0.height(kind, x, z)
        }
        fn block_state(&mut self, pos: [i32; 3]) -> Result<BlockStateId, PlacementQueryError> {
            Ok(*self.0.blocks.get(&pos).unwrap_or(&self.0.air))
        }
    }

    fn registry() -> &'static BlockRegistry {
        let names = [
            "minecraft:air",
            "minecraft:cave_air",
            "minecraft:void_air",
            "minecraft:water",
            "minecraft:lava",
            "minecraft:stone",
            "minecraft:sand",
        ];
        let rows: Vec<_> = names
            .iter()
            .map(|n| {
                let liquid = n.ends_with("water") || n.ends_with("lava");
                let props = if liquid {
                    json!({"level": "0"})
                } else {
                    json!({})
                };
                json!([n, n.ends_with("air"), liquid, "", {"id": n, "properties": props}])
            })
            .collect();
        let defaults: BTreeMap<_, _> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let facts = json!({"states": rows, "defaults": defaults});
        Box::leak(Box::new(
            BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
        ))
    }

    fn run(json: serde_json::Value, world: &mut Ctx, seed: i64, at: [i32; 3]) -> Vec<[i32; 3]> {
        let bindings = FeatureBlockBindings::new(registry(), BTreeSet::new()).unwrap();
        let modifier =
            PlacementModifier::from_json_with_blocks(&json.to_string(), &bindings).unwrap();
        let mut found = Vec::new();
        modifier
            .modify_in(world, &mut XoroshiroRandom::new(seed), at, |_, p| {
                found.push(p);
                Ok::<_, ()>(())
            })
            .unwrap();
        found
    }

    fn world(surface: i32) -> Ctx {
        let r = registry();
        let id = |n: &str| r.default_state(n).unwrap();
        Ctx(Flat {
            blocks: HashMap::new(),
            surface,
            rock: id("minecraft:stone"),
            air: id("minecraft:air"),
        })
    }

    #[test]
    fn underground_passes_only_below_the_surface() {
        let mut w = world(64);
        let m = json!({"type": "tfc:underground"});
        assert_eq!(run(m.clone(), &mut w, 1, [3, 40, 5]), vec![[3, 40, 5]]);
        assert!(run(m.clone(), &mut w, 1, [3, 64, 5]).is_empty());
        assert!(run(m, &mut w, 1, [3, 90, 5]).is_empty());
    }

    #[test]
    fn on_top_checks_the_block_below() {
        let r = registry();
        let mut w = world(0);
        w.0.blocks
            .insert([0, 9, 0], r.default_state("minecraft:sand").unwrap());
        w.0.blocks
            .insert([1, 9, 0], r.default_state("minecraft:stone").unwrap());
        let m = json!({"type": "tfc:on_top", "predicate": {
            "type": "minecraft:matching_blocks", "blocks": "minecraft:sand"}});
        assert_eq!(run(m.clone(), &mut w, 1, [0, 10, 0]), vec![[0, 10, 0]]);
        assert!(run(m.clone(), &mut w, 1, [1, 10, 0]).is_empty());
        // The block at the position itself does not matter.
        assert!(run(m, &mut w, 1, [0, 9, 0]).is_empty());
    }

    #[test]
    fn shallow_water_needs_water_and_thins_with_depth() {
        let r = registry();
        let water = r.default_state("minecraft:water").unwrap();
        let mut w = world(0);
        // Two blocks of water below y=10, then air.
        for y in [8, 9] {
            w.0.blocks.insert([0, y, 0], water);
        }
        let m = json!({"type": "tfc:shallow_water", "min_depth": 1, "max_depth": 5});
        // Depth 2: passes where nextFloat > 2/5, so about 60% of seeds.
        let passed = (0..400)
            .filter(|seed| !run(m.clone(), &mut w, *seed, [0, 10, 0]).is_empty())
            .count();
        assert!((200..280).contains(&passed), "{passed} of 400");
        // No water directly below: fewer than min_depth blocks, never passes.
        assert!(run(m.clone(), &mut w, 3, [5, 10, 0]).is_empty());
        // Deeper than max_depth: never passes.
        for y in 4..10 {
            w.0.blocks.insert([2, y, 0], water);
        }
        assert!(run(m, &mut w, 3, [2, 10, 0]).is_empty());
    }
}
