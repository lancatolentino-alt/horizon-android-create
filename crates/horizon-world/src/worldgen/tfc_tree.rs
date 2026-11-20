//! TerraFirmaCraft's trees and forests (EUPL 1.2): `tfc:random_tree`, `tfc:overlay_tree`,
//! `tfc:stacked_tree` (`world.feature.tree.*` with `TreeHelpers`), `tfc:krummholz`, `tfc:forest`
//! (`ForestFeature`, with its `ForestConfig.Entry` climate selection) and the soil features a
//! forest places first: `tfc:soil_disc` and `tfc:soil_forest_area`.
//!
//! TFC's block tags are matched by block name (the datapack's tag files are generated, not
//! shipped): "tree grows on" is the dirt tag (grass, dirt, coarse dirt, mud, rooted dirt, duff).

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    block_predicate::PredicateBlockResolver,
    density::identifier,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    integer, number_text, optional,
    placement::PlacementContext,
    required,
    structure::{Mirror, Rotation, StructureResources, StructureTemplate},
    tfc_noise::OpenSimplex2D,
    tfc_terrain,
    tfc_vein::{triangle, uniform_double, uniform_int},
};
use crate::{heightmap::HeightmapType, registry::BlockStateId};
use horizon_core::random::{RandomSource, XoroshiroRandom};
use std::{collections::HashMap, sync::Arc};

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

const SEA_LEVEL: i32 = 63;
const MIRRORS: [Mirror; 3] = [Mirror::None, Mirror::LeftRight, Mirror::FrontBack];

// ---------------------------------------------------------------- block name tests

pub(super) fn is_air(n: &str) -> bool {
    matches!(
        n,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}
pub(super) fn is_fluid_block(n: &str) -> bool {
    n == "minecraft:water" || n == "minecraft:lava" || n.starts_with("tfc:fluid/")
}
/// `FluidHelpers.isAirOrEmptyFluid`.
pub(super) fn air_or_empty_fluid(n: &str) -> bool {
    is_air(n) || is_fluid_block(n)
}
fn is_water(n: &str) -> bool {
    n == "minecraft:water" || n.starts_with("tfc:fluid/") && n.contains("water")
}
pub(super) fn is_leaves(n: &str) -> bool {
    n.ends_with("_leaves")
}
fn is_sapling(n: &str) -> bool {
    n.ends_with("_sapling") || n.starts_with("tfc:wood/sapling/")
}
pub(super) fn is_sand(n: &str) -> bool {
    (n.contains("sand") && !n.contains("sandstone")) && !n.contains("soul")
}
pub(super) fn is_gravel(n: &str) -> bool {
    n.contains("gravel")
}
/// `BlockTags.DIRT` as TFC extends it (grass, dirt, coarse dirt, mud) plus rooted dirt and duff.
pub(super) fn is_dirt(n: &str) -> bool {
    n.starts_with("tfc:grass/")
        || n.starts_with("tfc:dirt/")
        || n.starts_with("tfc:coarse_dirt/")
        || n.starts_with("tfc:mud/")
        || n.starts_with("tfc:rooted_dirt/")
        || n.starts_with("tfc:duff/")
        || matches!(
            n,
            "minecraft:dirt"
                | "minecraft:grass_block"
                | "minecraft:podzol"
                | "minecraft:coarse_dirt"
                | "minecraft:mycelium"
                | "minecraft:rooted_dirt"
                | "minecraft:moss_block"
                | "minecraft:mud"
                | "minecraft:muddy_mangrove_roots"
        )
}
pub(super) fn is_bush_plantable_on(n: &str) -> bool {
    is_dirt(n) || n.contains("farmland") || n.contains("clay") || n.contains("kaolin")
}
fn is_sea_bush_plantable_on(n: &str) -> bool {
    is_dirt(n) || is_gravel(n) || is_sand(n)
}
/// `TFCTags.Blocks.SINGLE_BLOCK_REPLACEABLE`: small flowers, short grasses, humus, fallen leaves.
pub(super) fn single_block_replaceable(n: &str) -> bool {
    n.starts_with("tfc:wood/fallen_leaves/")
        || n == "tfc:groundcover/humus"
        || matches!(
            n,
            "minecraft:dandelion"
                | "minecraft:poppy"
                | "minecraft:blue_orchid"
                | "minecraft:allium"
                | "minecraft:azure_bluet"
                | "minecraft:red_tulip"
                | "minecraft:orange_tulip"
                | "minecraft:white_tulip"
                | "minecraft:pink_tulip"
                | "minecraft:oxeye_daisy"
                | "minecraft:cornflower"
                | "minecraft:lily_of_the_valley"
                | "minecraft:torchflower"
                | "minecraft:wither_rose"
        )
        || [
            "beachgrass",
            "bluegrass",
            "bromegrass",
            "fountain_grass",
            "manatee_grass",
            "orchard_grass",
            "ryegrass",
            "scutch_grass",
            "star_grass",
            "timothy_grass",
            "raddia_grass",
            "red_oat_grass",
            "turtle_grass",
        ]
        .iter()
        .any(|g| n.strip_prefix("tfc:plant/") == Some(*g))
}
/// `EnvironmentHelpers.isWorldgenReplaceable`.
pub(super) fn replaceable(n: &str) -> bool {
    air_or_empty_fluid(n) || single_block_replaceable(n)
}
/// `isFaceSturdy(UP)`, approximated: not air, fluid, foliage or a plant.
pub(super) fn sturdy(n: &str) -> bool {
    !(air_or_empty_fluid(n)
        || is_leaves(n)
        || n.starts_with("tfc:plant/")
        || n.starts_with("tfc:wood/fallen_leaves/")
        || n.starts_with("tfc:wood/twig/")
        || n.ends_with("_slab")
        || n.contains("carpet")
        || n.ends_with("grass")
        || n.ends_with("fern"))
}

// ---------------------------------------------------------------- shared decode helpers

fn text(object: &Object, key: &'static str) -> Result<String, DecodeError> {
    Ok(serde_json::from_str(required(object, key)?.get())?)
}
fn int(object: &Object, key: &'static str, default: i32) -> Result<i32, DecodeError> {
    Ok(optional(object, key)
        .map(integer)
        .transpose()?
        .unwrap_or(default))
}
fn float(object: &Object, key: &'static str, default: f32) -> Result<f32, DecodeError> {
    optional(object, key)
        .map(|v| {
            number_text(v).and_then(|t| t.parse().map_err(|_| DecodeError::Field("tree float")))
        })
        .transpose()
        .map(|v| v.unwrap_or(default))
}
fn flag(object: &Object, key: &'static str, default: bool) -> Result<bool, DecodeError> {
    Ok(optional(object, key)
        .map(|v| serde_json::from_str::<bool>(v.get()))
        .transpose()?
        .unwrap_or(default))
}
fn config_of(json: &str) -> Result<(String, Object), DecodeError> {
    let object: Object = serde_json::from_str(json)?;
    let kind = identifier(&serde_json::from_str::<String>(
        required(&object, "type")?.get(),
    )?)?;
    let config = match optional(&object, "config") {
        Some(config) => serde_json::from_str(config.get())?,
        None => object,
    };
    Ok((kind, config))
}

/// A block state from JSON (a name or `{Name, Properties}`). Properties the registry lacks (TFC's
/// `branch_direction`, which the mod's log blocks do not have) are dropped.
pub(super) fn state_of(
    bindings: &FeatureBlockBindings<'_>,
    json: &str,
) -> Result<BlockStateId, DecodeError> {
    if let Ok(state) = bindings.predicate_state(json) {
        return Ok(state);
    }
    let value: serde_json::Value = serde_json::from_str(json)?;
    let name = value
        .as_str()
        .or_else(|| value.get("Name").and_then(|n| n.as_str()))
        .or_else(|| value.get("id").and_then(|n| n.as_str()))
        .ok_or(DecodeError::Field("tree block state"))?;
    let registry = bindings.registry();
    let default = registry
        .default_state(name)
        .ok_or_else(|| DecodeError::Registry(format!("unknown block {name}")))?;
    let mut state = registry.state(default).unwrap().clone();
    if let Some(props) = value
        .get("Properties")
        .or_else(|| value.get("properties"))
        .and_then(|p| p.as_object())
    {
        for (k, v) in props {
            if let (Some(slot), Some(v)) = (state.properties.get_mut(k), v.as_str()) {
                let previous = std::mem::replace(slot, v.to_owned());
                if registry.id(&state).is_none() {
                    *state.properties.get_mut(k).unwrap() = previous;
                }
            }
        }
    }
    Ok(registry.id(&state).unwrap_or(default))
}

/// `IWeighted<BlockState>`: a singleton ignores weights; every `get` draws one `nextDouble`.
#[derive(Clone, Debug)]
pub(super) struct Weighted {
    cumulative: Vec<(f64, BlockStateId)>,
    total: f64,
    singleton: bool,
}
impl Weighted {
    pub(super) fn decode(
        list: &[Object],
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let mut out = Self {
            cumulative: Vec::new(),
            total: 0.0,
            singleton: list.len() == 1,
        };
        for entry in list {
            let state = state_of(bindings, required(entry, "block")?.get())?;
            let weight = match optional(entry, "weight") {
                Some(w) => number_text(w)?
                    .parse::<f64>()
                    .map_err(|_| DecodeError::Field("weight"))?,
                None => 1.0,
            };
            if weight > 0.0 {
                out.total += weight;
                out.cumulative.push((out.total, state));
            } else if out.singleton {
                out.cumulative.push((1.0, state));
            }
        }
        Ok(out)
    }
    pub(super) fn get(&self, random: &mut impl RandomSource) -> Option<BlockStateId> {
        let r = random.next_double();
        if self.singleton {
            return self.cumulative.first().map(|e| e.1);
        }
        let value = r * self.total;
        self.cumulative.iter().find(|e| e.0 > value).map(|e| e.1)
    }
}

// ---------------------------------------------------------------- tree configs

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ground {
    Normal,
    Sand,
    ShallowWater,
    Submerged,
    ShallowSalt,
    SubmergedSalt,
    Floating,
}
#[derive(Clone, Debug)]
struct Placement {
    width: i32,
    height: i32,
    ground: Ground,
}
impl Placement {
    fn decode(config: &Object) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(required(config, "placement")?.get())?;
        let ground = match optional(&object, "ground_type")
            .map(|v| serde_json::from_str::<String>(v.get()))
            .transpose()?
            .as_deref()
        {
            None | Some("normal") => Ground::Normal,
            Some("sand") => Ground::Sand,
            Some("shallow_water") => Ground::ShallowWater,
            Some("submerged") => Ground::Submerged,
            Some("shallow_allow_saltwater") => Ground::ShallowSalt,
            Some("submerged_allow_saltwater") => Ground::SubmergedSalt,
            Some("floating") => Ground::Floating,
            Some(_) => return Err(DecodeError::Field("tree ground type")),
        };
        Ok(Self {
            width: int(&object, "width", 1)?,
            height: int(&object, "height", 1)?,
            ground,
        })
    }
    fn may_place_in_water(&self) -> bool {
        matches!(
            self.ground,
            Ground::ShallowWater | Ground::Submerged | Ground::SubmergedSalt
        )
    }
    fn may_place_underwater(&self) -> bool {
        matches!(self.ground, Ground::Submerged | Ground::SubmergedSalt)
    }
}
#[derive(Clone, Debug)]
struct Trunk {
    state: BlockStateId,
    min: i32,
    max: i32,
    wide: bool,
}
impl Trunk {
    fn decode(
        value: &serde_json::value::RawValue,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(value.get())?;
        let (min, max) = (
            int(&object, "min_height", 1)?,
            int(&object, "max_height", 1)?,
        );
        if min > max {
            return Err(DecodeError::Field("trunk height"));
        }
        Ok(Self {
            state: state_of(bindings, required(&object, "state")?.get())?,
            min,
            max,
            wide: flag(&object, "wide", false)?,
        })
    }
}
#[derive(Clone, Debug)]
struct Roots {
    blocks: HashMap<String, Weighted>,
    width: i32,
    height: i32,
    tries: i32,
    required: bool,
    /// `SpecialRootPlacer`: the chance to skew a root sideways, and the `tfc:tree_roots` stand-in
    /// (dry, waterlogged).
    special: Option<(f32, [BlockStateId; 2])>,
}
impl Roots {
    fn decode(
        value: &serde_json::value::RawValue,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(value.get())?;
        let special = match optional(&object, "special_placer") {
            None => None,
            Some(raw) => {
                let placer: Object = serde_json::from_str(raw.get())?;
                let skew: f32 = serde_json::from_str(required(&placer, "skew_chance")?.get())?;
                // `TFCBlocks.TREE_ROOTS` sounds as mangrove roots do.
                let registry = bindings.registry();
                let dry = registry
                    .default_state("minecraft:mangrove_roots")
                    .ok_or(DecodeError::Field("tree roots block"))?;
                let mut wet_state = registry.state(dry).unwrap().clone();
                let wet = if wet_state.properties.contains_key("waterlogged") {
                    wet_state
                        .properties
                        .insert("waterlogged".into(), "true".into());
                    registry.id(&wet_state).unwrap_or(dry)
                } else {
                    dry
                };
                Some((skew, [dry, wet]))
            }
        };
        let mut blocks = HashMap::new();
        let list: Vec<Object> = serde_json::from_str(required(&object, "blocks")?.get())?;
        for entry in &list {
            let with: Vec<Object> = serde_json::from_str(required(entry, "with")?.get())?;
            let weighted = Weighted::decode(&with, bindings)?;
            let replace: Vec<String> = serde_json::from_str(required(entry, "replace")?.get())?;
            for name in replace {
                blocks.insert(name, weighted.clone());
            }
        }
        Ok(Self {
            blocks,
            width: int(&object, "width", 4)?,
            height: int(&object, "height", 3)?,
            tries: int(&object, "tries", 5)?,
            required: flag(&object, "required", false)?,
            special,
        })
    }
}
#[derive(Clone, Debug)]
struct Layer {
    templates: Vec<Arc<StructureTemplate>>,
    min: i32,
    max: i32,
}
#[derive(Clone, Debug)]
enum TreeKind {
    Random(Vec<Arc<StructureTemplate>>),
    Overlay {
        base: Arc<StructureTemplate>,
        overlay: Arc<StructureTemplate>,
        integrity: f32,
    },
    Stacked(Vec<Layer>),
}
#[derive(Clone, Debug)]
struct Tree {
    kind: TreeKind,
    trunk: Option<Trunk>,
    placement: Placement,
    roots: Option<Roots>,
}
#[derive(Clone, Debug)]
struct Krummholz {
    block: BlockStateId,
    min: i32,
    max: i32,
    on_stone: bool,
    on_gravel: bool,
}

#[derive(Clone, Debug)]
enum Planted {
    Tree(Tree),
    Krummholz(Krummholz),
}

#[derive(Clone, Debug)]
struct Soil {
    states: HashMap<String, BlockStateId>,
    height: i32,
    shape: SoilShape,
}
#[derive(Clone, Debug)]
enum SoilShape {
    Disc {
        min_radius: i32,
        max_radius: i32,
        integrity: f32,
    },
    ForestArea {
        min_forest: i32,
        max_forest: i32,
        spread: f32,
        noise_scale: f32,
        inverted: bool,
    },
}

#[derive(Clone, Debug)]
struct Climate {
    min_temperature: f32,
    max_temperature: f32,
    min_groundwater: f32,
    max_groundwater: f32,
    min_rain_variance: f32,
    max_rain_variance: f32,
    absolute: bool,
    min_elevation: i32,
    max_elevation: i32,
}
#[derive(Clone, Debug)]
struct Entry {
    climate: Climate,
    bush_log: Option<BlockStateId>,
    bush_leaves: Option<BlockStateId>,
    fallen_log: Option<BlockStateId>,
    fallen_leaves: Option<BlockStateId>,
    groundcover: Option<Weighted>,
    normal: Planted,
    dead: Planted,
    old_growth: Option<Planted>,
    krummholz: Option<Planted>,
    soil: Option<Soil>,
    old_growth_chance: i32,
    spoiler_chance: i32,
    fallen_chance: i32,
    dead_chance: i32,
    floating: bool,
}
impl Entry {
    fn valid(&self, temperature: f32, groundwater: f32, rain_var: f32, elevation: f32) -> bool {
        let c = &self.climate;
        let rv = if c.absolute { rain_var.abs() } else { rain_var };
        groundwater >= c.min_groundwater
            && groundwater <= c.max_groundwater
            && rv >= c.min_rain_variance
            && rv <= c.max_rain_variance
            && temperature >= c.min_temperature
            && temperature <= c.max_temperature
            && elevation >= c.min_elevation as f32
            && elevation <= c.max_elevation as f32
    }
    /// `distanceFromMean`, with TFC's own (not actually mean) "averages".
    fn distance(&self, temperature: f32, groundwater: f32, rain_var: f32, elevation: f32) -> f32 {
        let c = &self.climate;
        let rv = if c.absolute { rain_var.abs() } else { rain_var };
        let avg_temp = (c.max_temperature - c.min_temperature) / 2.0;
        let avg_water = (c.max_groundwater - c.min_groundwater) / 2.0;
        let avg_rain = (c.max_rain_variance - c.min_rain_variance) / 2.0;
        let avg_elev = (c.max_elevation - c.min_elevation) as f32 / 2.0;
        (temperature - avg_temp) * 10.0
            + (groundwater - avg_water)
            + (rv - avg_rain) * 250.0
            + (elevation - avg_elev) * 5.0
    }
}

#[derive(Clone, Debug)]
enum Kind {
    Tree(Tree),
    Krummholz(Krummholz),
    Soil(Soil),
    Forest(Vec<Entry>),
}

#[derive(Clone, Debug)]
pub struct TfcTreeFeature {
    kind: Kind,
    resources: Arc<StructureResources>,
}

fn templates(
    resources: &StructureResources,
    names: &[String],
) -> Result<Vec<Arc<StructureTemplate>>, DecodeError> {
    names
        .iter()
        .map(|id| {
            resources
                .template(id)
                .ok_or_else(|| DecodeError::Registry(format!("missing tree template {id}")))
        })
        .collect()
}

fn decode_tree(
    kind: &str,
    config: &Object,
    resources: &StructureResources,
    bindings: &FeatureBlockBindings<'_>,
) -> Result<Tree, DecodeError> {
    let trunk = optional(config, "trunk")
        .map(|t| Trunk::decode(t, bindings))
        .transpose()?;
    let roots = optional(config, "root_system")
        .map(|r| Roots::decode(r, bindings))
        .transpose()?;
    let placement = Placement::decode(config)?;
    let one = |key: &'static str| -> Result<Arc<StructureTemplate>, DecodeError> {
        Ok(templates(resources, &[text(config, key)?])?.remove(0))
    };
    let kind = match kind {
        "tfc:random_tree" => {
            let names: Vec<String> = serde_json::from_str(required(config, "structures")?.get())?;
            if names.is_empty() {
                return Err(DecodeError::Field("tree structures"));
            }
            TreeKind::Random(templates(resources, &names)?)
        }
        "tfc:overlay_tree" => TreeKind::Overlay {
            base: one("base")?,
            overlay: one("overlay")?,
            integrity: float(config, "overlay_integrity", 0.5)?,
        },
        "tfc:stacked_tree" => {
            if trunk.is_none() {
                return Err(DecodeError::Field("stacked tree trunk"));
            }
            let layers: Vec<Object> = serde_json::from_str(required(config, "layers")?.get())?;
            let mut out = Vec::new();
            for layer in &layers {
                let names: Vec<String> = serde_json::from_str(required(layer, "templates")?.get())?;
                if names.is_empty() {
                    return Err(DecodeError::Field("layer templates"));
                }
                let (min, max) = (int(layer, "min_count", 1)?, int(layer, "max_count", 1)?);
                if max < min {
                    return Err(DecodeError::Field("layer count"));
                }
                out.push(Layer {
                    templates: templates(resources, &names)?,
                    min,
                    max,
                });
            }
            TreeKind::Stacked(out)
        }
        _ => return Err(DecodeError::Field("tree type")),
    };
    Ok(Tree {
        kind,
        trunk,
        placement,
        roots,
    })
}

fn decode_krummholz(
    config: &Object,
    bindings: &FeatureBlockBindings<'_>,
) -> Result<Krummholz, DecodeError> {
    let registry = bindings.registry();
    let height: Object = serde_json::from_str(required(config, "height")?.get())?;
    let (min, max) = match optional(&height, "min_inclusive") {
        Some(_) => (
            int(&height, "min_inclusive", 1)?,
            int(&height, "max_inclusive", 1)?,
        ),
        None => {
            let v = int(&height, "value", 1)?;
            (v, v)
        }
    };
    Ok(Krummholz {
        block: registry
            .default_state(&text(config, "block")?)
            .ok_or(DecodeError::Field("krummholz block"))?,
        min,
        max,
        on_stone: flag(config, "spawns_on_stone", false)?,
        on_gravel: flag(config, "spawns_on_gravel", false)?,
    })
}

fn decode_planted(
    json: &str,
    resources: &StructureResources,
    bindings: &FeatureBlockBindings<'_>,
) -> Result<Planted, DecodeError> {
    let (kind, config) = config_of(json)?;
    if kind == "tfc:krummholz" {
        Ok(Planted::Krummholz(decode_krummholz(&config, bindings)?))
    } else {
        Ok(Planted::Tree(decode_tree(
            &kind, &config, resources, bindings,
        )?))
    }
}

fn decode_soil(
    kind: &str,
    config: &Object,
    bindings: &FeatureBlockBindings<'_>,
) -> Result<Soil, DecodeError> {
    let list: Vec<Object> = serde_json::from_str(required(config, "states")?.get())?;
    let mut states = HashMap::new();
    for entry in &list {
        let from = text(entry, "replace")?;
        states.insert(from, state_of(bindings, required(entry, "with")?.get())?);
    }
    let height = int(config, "height", 0)?;
    let shape = if kind == "tfc:soil_disc" {
        let (min_radius, max_radius) =
            (int(config, "min_radius", 1)?, int(config, "max_radius", 1)?);
        if max_radius < min_radius {
            return Err(DecodeError::Field("soil disc radius"));
        }
        SoilShape::Disc {
            min_radius,
            max_radius,
            integrity: float(config, "integrity", 1.0)?,
        }
    } else {
        let (min_forest, max_forest) =
            (int(config, "min_forest", 1)?, int(config, "max_forest", 1)?);
        if max_forest < min_forest {
            return Err(DecodeError::Field("soil forest range"));
        }
        SoilShape::ForestArea {
            min_forest,
            max_forest,
            spread: float(config, "spread", 0.05)?,
            noise_scale: float(config, "noise_scale", 1.0)?,
            inverted: flag(config, "inverted", false)?,
        }
    };
    Ok(Soil {
        states,
        height,
        shape,
    })
}

fn decode_entry(
    json: &str,
    resources: &StructureResources,
    bindings: &FeatureBlockBindings<'_>,
) -> Result<Entry, DecodeError> {
    let (kind, c) = config_of(json)?;
    if kind != "tfc:forest_entry" {
        return Err(DecodeError::Field("forest entry type"));
    }
    let climate: Object = serde_json::from_str(required(&c, "climate")?.get())?;
    let climate = Climate {
        min_temperature: float(&climate, "min_temperature", f32::NEG_INFINITY)?,
        max_temperature: float(&climate, "max_temperature", f32::INFINITY)?,
        min_groundwater: float(&climate, "min_groundwater", f32::NEG_INFINITY)?,
        max_groundwater: float(&climate, "max_groundwater", f32::INFINITY)?,
        min_rain_variance: float(&climate, "min_rain_variance", -1.0)?,
        max_rain_variance: float(&climate, "max_rain_variance", 1.0)?,
        absolute: flag(&climate, "rain_variance_absolute", false)?,
        min_elevation: int(&climate, "min_elevation", -64)?,
        max_elevation: int(&climate, "max_elevation", 320)?,
    };
    let state = |key: &'static str| -> Result<Option<BlockStateId>, DecodeError> {
        optional(&c, key)
            .map(|v| state_of(bindings, v.get()))
            .transpose()
    };
    let planted = |key: &'static str| -> Result<Option<Planted>, DecodeError> {
        optional(&c, key)
            .map(|v| decode_planted(v.get(), resources, bindings))
            .transpose()
    };
    let groundcover = optional(&c, "groundcover")
        .map(|v| {
            let list: Vec<Object> = serde_json::from_str(v.get())?;
            Weighted::decode(&list, bindings)
        })
        .transpose()?;
    let soil = optional(&c, "soil_disc")
        .map(|v| {
            let (kind, config) = config_of(v.get())?;
            decode_soil(&kind, &config, bindings)
        })
        .transpose()?;
    Ok(Entry {
        climate,
        bush_log: state("bush_log")?,
        bush_leaves: state("bush_leaves")?,
        fallen_log: state("fallen_log")?,
        fallen_leaves: state("fallen_leaves")?,
        groundcover,
        normal: planted("normal_tree")?.ok_or(DecodeError::Field("normal_tree"))?,
        dead: planted("dead_tree")?.ok_or(DecodeError::Field("dead_tree"))?,
        old_growth: planted("old_growth_tree")?,
        krummholz: planted("krummholz")?,
        soil,
        old_growth_chance: int(&c, "old_growth_chance", 6)?,
        spoiler_chance: int(&c, "spoiler_old_growth_chance", 200)?,
        fallen_chance: int(&c, "fallen_tree_chance", 14)?,
        dead_chance: int(&c, "dead_chance", 75)?,
        floating: flag(&c, "floating", false)?,
    })
}

impl TfcTreeFeature {
    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let resources = bindings
            .structures
            .clone()
            .ok_or(DecodeError::Field("tree template bindings"))?;
        let (kind, config) = config_of(json)?;
        let kind = match kind.as_str() {
            "tfc:random_tree" | "tfc:overlay_tree" | "tfc:stacked_tree" => {
                Kind::Tree(decode_tree(&kind, &config, &resources, bindings)?)
            }
            "tfc:krummholz" => Kind::Krummholz(decode_krummholz(&config, bindings)?),
            "tfc:soil_disc" | "tfc:soil_forest_area" => {
                Kind::Soil(decode_soil(&kind, &config, bindings)?)
            }
            "tfc:forest" => {
                let list: Vec<Box<serde_json::value::RawValue>> =
                    serde_json::from_str(required(&config, "entries")?.get())?;
                Kind::Forest(
                    list.iter()
                        .map(|e| decode_entry(e.get(), &resources, bindings))
                        .collect::<Result<_, _>>()?,
                )
            }
            _ => return Err(DecodeError::Field("tree feature type")),
        };
        Ok(Self { kind, resources })
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let mut p = Placer {
            res: &self.resources,
            world,
        };
        match &self.kind {
            Kind::Tree(tree) => p.tree(tree, random, origin),
            Kind::Krummholz(k) => p.krummholz(k, random, origin),
            Kind::Soil(soil) => p.soil(soil, random, origin),
            Kind::Forest(entries) => p.forest(entries, random, origin),
        }
    }
}

// ---------------------------------------------------------------- placement

struct Placer<'a, W> {
    res: &'a StructureResources,
    world: &'a mut W,
}

fn we<E>(e: E) -> FeatureExecutionError<E> {
    FeatureExecutionError::Basic(FeatureError::World(e))
}
fn rnd<T, E>(
    r: Result<T, horizon_core::random::InvalidBound>,
) -> Result<T, FeatureExecutionError<E>> {
    r.map_err(FeatureExecutionError::Random)
}
fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn transformed(p: [i32; 3], mirror: Mirror, rotation: Rotation) -> [i32; 3] {
    StructureTemplate::transform(p, mirror, rotation, [0; 3])
}
/// `Mth.nextInt`.
fn next_between<E>(
    random: &mut impl RandomSource,
    min: i32,
    max: i32,
) -> Result<i32, FeatureExecutionError<E>> {
    if min >= max {
        Ok(min)
    } else {
        Ok(rnd(random.next_int_bound(max - min + 1))? + min)
    }
}

impl<W: FeatureWorld + PlacementContext> Placer<'_, W> {
    fn name(&self, state: BlockStateId) -> &str {
        self.res
            .registry
            .state(state)
            .map_or("minecraft:air", |s| s.name.as_str())
    }
    fn at(&mut self, p: [i32; 3]) -> R<String, W> {
        let s = self.world.block(p).map_err(we)?;
        Ok(self.name(s).to_owned())
    }
    fn put(&mut self, p: [i32; 3], state: BlockStateId, flags: i32) -> R<(), W> {
        self.world.set_block(p, state, flags).map_err(we)?;
        Ok(())
    }
    fn height(&mut self, kind: HeightmapType, x: i32, z: i32) -> R<i32, W> {
        self.world
            .height(kind, x, z)
            .map_err(FeatureExecutionError::Query)
    }

    // ---- TreeHelpers

    fn valid_ground(
        &mut self,
        pos: [i32; 3],
        mirror: Mirror,
        rotation: Rotation,
        pl: &Placement,
    ) -> R<bool, W> {
        for x in (1 - pl.width) / 2..=pl.width / 2 {
            for z in (1 - pl.width) / 2..=pl.width / 2 {
                let p = add(pos, transformed([x, 0, z], mirror, rotation));
                if pl.ground == Ground::Floating {
                    return self.valid_floating(p);
                }
                let ok = if pl.may_place_underwater() {
                    self.valid_underwater(p, pl)?
                } else {
                    self.valid_position(p, pl)?
                };
                if !ok {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
    fn grows_on(&self, below: &str, pl: &Placement) -> bool {
        is_dirt(below) || (pl.ground == Ground::Sand && is_sand(below))
    }
    fn valid_position(&mut self, p: [i32; 3], pl: &Placement) -> R<bool, W> {
        let at = self.at(p)?;
        let in_water = is_water(&at);
        if !(pl.may_place_in_water() && air_or_empty_fluid(&at) && in_water)
            && !is_air(&at)
            && !is_sapling(&at)
        {
            return Ok(false);
        }
        let below = self.at([p[0], p[1] - 1, p[2]])?;
        let mut ok = self.grows_on(&below, pl);
        if !ok && pl.may_place_in_water() && in_water {
            ok = is_sea_bush_plantable_on(&below);
        }
        Ok(ok)
    }
    fn valid_floating(&mut self, p: [i32; 3]) -> R<bool, W> {
        let mut p = p;
        for _ in 0..=3 {
            if !replaceable(&self.at(p)?) {
                return Ok(false);
            }
            p[1] -= 1;
            let below = self.at(p)?;
            if is_bush_plantable_on(&below) || is_sea_bush_plantable_on(&below) || is_water(&below)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn valid_underwater(&mut self, p: [i32; 3], pl: &Placement) -> R<bool, W> {
        let at = self.at(p)?;
        let water = is_water(&at);
        let below = self.at([p[0], p[1] - 1, p[2]])?;
        if water && air_or_empty_fluid(&at) {
            return Ok(is_sea_bush_plantable_on(&below));
        }
        if !is_fluid_block(&at) && (is_air(&at) || is_sapling(&at)) {
            return Ok(self.grows_on(&below, pl));
        }
        Ok(false)
    }
    fn valid_trunk(
        &mut self,
        pos: [i32; 3],
        mirror: Mirror,
        rotation: Rotation,
        pl: &Placement,
    ) -> R<bool, W> {
        for x in (1 - pl.width) / 2..=pl.width / 2 {
            for z in (1 - pl.width) / 2..=pl.width / 2 {
                for y in 1..pl.height {
                    let p = add(pos, transformed([x, y, z], mirror, rotation));
                    let n = self.at(p)?;
                    let free = if pl.may_place_underwater() {
                        air_or_empty_fluid(&n)
                    } else {
                        is_air(&n)
                    };
                    if !free {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }
    /// `SpecialRootPlacer.canPlaceRoot`.
    fn can_place_root(&mut self, roots: &Roots, p: [i32; 3]) -> R<bool, W> {
        let n = self.at(p)?;
        Ok(replaceable(&n) || roots.blocks.contains_key(&n))
    }

    /// `SpecialRootPlacer.potentialRootPositions`. The branches that give the same list differ in
    /// whether they draw, so they stay apart.
    #[allow(clippy::if_same_then_else)]
    fn potential_root_positions(
        pos: [i32; 3],
        direction: [i32; 2],
        random: &mut impl RandomSource,
        origin: [i32; 3],
        width: i32,
        skew: f32,
    ) -> Vec<[i32; 3]> {
        let below = [pos[0], pos[1] - 1, pos[2]];
        let relative = [pos[0] + direction[0], pos[1], pos[2] + direction[1]];
        let relative_below = [relative[0], relative[1] - 1, relative[2]];
        let dist =
            (pos[0] - origin[0]).abs() + (pos[1] - origin[1]).abs() + (pos[2] - origin[2]).abs();
        if dist > width - 3 && dist <= width {
            if random.next_float() < skew {
                vec![below, relative_below]
            } else {
                vec![below]
            }
        } else if dist > width {
            vec![below]
        } else if random.next_float() < skew {
            vec![below]
        } else if random.next_boolean() {
            vec![relative]
        } else {
            vec![below]
        }
    }

    /// `SpecialRootPlacer.simulateRoots`.
    #[allow(clippy::too_many_arguments)]
    fn simulate_roots(
        &mut self,
        roots: &Roots,
        skew: f32,
        random: &mut impl RandomSource,
        pos: [i32; 3],
        direction: [i32; 2],
        origin: [i32; 3],
        used: &mut Vec<[i32; 3]>,
        length: i32,
    ) -> R<bool, W> {
        let max = roots.height;
        if length == max || used.len() as i32 > max {
            return Ok(false);
        }
        for p in Self::potential_root_positions(pos, direction, random, origin, roots.width, skew) {
            if self.can_place_root(roots, p)? {
                used.push(p);
                if !self.simulate_roots(
                    roots,
                    skew,
                    random,
                    p,
                    direction,
                    origin,
                    used,
                    length + 1,
                )? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// `SpecialRootPlacer.placeRoots`: lifts the trunk base by up to three blocks, then grows
    /// roots out from its four sides and down. Always succeeds.
    fn place_special_roots(
        &mut self,
        base: &mut [i32; 3],
        roots: &Roots,
        (skew, tree_roots): (f32, [BlockStateId; 2]),
        random: &mut impl RandomSource,
    ) -> R<bool, W> {
        // `Direction.Plane.HORIZONTAL`: north, east, south, west.
        const HORIZONTAL: [[i32; 2]; 4] = [[0, -1], [1, 0], [0, 1], [-1, 0]];
        base[1] += rnd(random.next_int_bound(4))?;
        let mut positions = vec![[base[0], base[1] - 1, base[2]]];
        let guaranteed = rnd(random.next_int_bound(4))? as usize;
        for (i, d) in HORIZONTAL.iter().enumerate() {
            if i == guaranteed || rnd(random.next_int_bound(3))? > 0 {
                let relative = [base[0] + d[0], base[1], base[2] + d[1]];
                let mut used = Vec::new();
                self.simulate_roots(roots, skew, random, relative, *d, *base, &mut used, 0)?;
                positions.extend(used);
            }
            positions.push([base[0] + d[0], base[1], base[2] + d[1]]);
        }
        for p in positions {
            if !self.can_place_root(roots, p)? {
                continue;
            }
            let at = self.at(p)?;
            let state = match roots.blocks.get(&at) {
                Some(weighted) => weighted.get(random),
                None => Some(tree_roots[0]),
            };
            if let Some(mut state) = state {
                // `fillWithFluid`: only the stand-in roots can hold the water around them.
                if state == tree_roots[0] && at == "minecraft:water" {
                    state = tree_roots[1];
                }
                self.put(p, state, 19)?;
            }
        }
        Ok(true)
    }

    fn place_roots(
        &mut self,
        base: &mut [i32; 3],
        roots: &Roots,
        random: &mut impl RandomSource,
    ) -> R<bool, W> {
        if let Some(special) = roots.special {
            return self.place_special_roots(base, roots, special, random);
        }
        let origin = [base[0], base[1] - 1, base[2]];
        for _ in 0..roots.tries {
            let dx =
                rnd(random.next_int_bound(roots.width))? - rnd(random.next_int_bound(roots.width))?;
            let dz =
                rnd(random.next_int_bound(roots.width))? - rnd(random.next_int_bound(roots.width))?;
            let dy = -rnd(random.next_int_bound(roots.height))?;
            let p = add(origin, [dx, dy, dz]);
            let n = self.at(p)?;
            if let Some(weighted) = roots.blocks.get(&n) {
                if let Some(state) = weighted.get(random) {
                    self.put(p, state, 3)?;
                }
            }
        }
        Ok(true)
    }
    fn place_trunk(
        &mut self,
        pos: [i32; 3],
        trunk: &Trunk,
        mirror: Mirror,
        rotation: Rotation,
        random: &mut impl RandomSource,
    ) -> R<i32, W> {
        let height = if trunk.max == trunk.min {
            trunk.min
        } else {
            trunk.min + rnd(random.next_int_bound(1 + trunk.max - trunk.min))?
        };
        let cells: &[[i32; 2]] = if trunk.wide {
            &[[0, 0], [0, 1], [1, 0], [1, 1]]
        } else {
            &[[0, 0]]
        };
        for y in 0..height {
            for c in cells {
                let p = add(pos, transformed([c[0], y, c[1]], mirror, rotation));
                self.put(p, trunk.state, 3)?;
            }
        }
        Ok(height)
    }
    /// `TreeHelpers.placeTemplate`: the template's non-air blocks replace air, replaceable plants
    /// and leaves inside the origin chunk's 3x3 neighbourhood.
    #[allow(clippy::too_many_arguments)]
    fn place_template(
        &mut self,
        template: &StructureTemplate,
        mirror: Mirror,
        rotation: Rotation,
        pos: [i32; 3],
        chunk: [i32; 2],
        rot: Option<f32>,
        random: &mut impl RandomSource,
    ) -> R<(), W> {
        if template.palettes().is_empty() {
            return Ok(());
        }
        let palette = template
            .palette_index(random)
            .min(template.palettes().len() - 1);
        let (min_y, max_y) = (
            self.world.min_y().map_err(FeatureExecutionError::Query)?,
            self.world.max_y().map_err(FeatureExecutionError::Query)?,
        );
        let low = [chunk[0] * 16 - 16, chunk[1] * 16 - 16];
        let high = [chunk[0] * 16 + 31, chunk[1] * 16 + 31];
        let blocks = template.palettes()[palette].blocks().to_vec();
        for block in &blocks {
            let name = self.name(block.state);
            if name == "minecraft:air" || name == "minecraft:structure_block" {
                continue;
            }
            if let Some(integrity) = rot {
                if random.next_float() > integrity {
                    continue;
                }
            }
            let p = add(pos, transformed(block.pos, mirror, rotation));
            if p[0] < low[0]
                || p[0] > high[0]
                || p[2] < low[1]
                || p[2] > high[1]
                || p[1] < min_y
                || p[1] > max_y
            {
                continue;
            }
            let at = self.at(p)?;
            if replaceable(&at) || is_leaves(&at) {
                let t = &self.res.transforms;
                let state = t.rotate(t.mirror(block.state, mirror), rotation);
                self.put(p, state, 2)?;
            }
        }
        Ok(())
    }

    fn tree(
        &mut self,
        tree: &Tree,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let chunk = [origin[0] >> 4, origin[2] >> 4];
        // `getPlacementSettings`: rotation first, then mirror.
        let rotation = Rotation::random(random);
        let mirror = MIRRORS[rnd(random.next_int_bound(3))? as usize];
        let mut pos = origin;
        match &tree.kind {
            TreeKind::Random(list) => {
                let template =
                    list[rnd(random.next_int_bound(list.len() as i32))? as usize].clone();
                if !(self.valid_ground(origin, mirror, rotation, &tree.placement)?
                    && self.valid_trunk(origin, mirror, rotation, &tree.placement)?)
                {
                    return Ok(false);
                }
                if !self.roots_and_trunk(tree, &mut pos, mirror, rotation, random)? {
                    return Ok(false);
                }
                let centre = transformed(
                    [(template.size[0] - 1) / 2, 0, (template.size[2] - 1) / 2],
                    mirror,
                    rotation,
                );
                let at = [pos[0] - centre[0], pos[1] - centre[1], pos[2] - centre[2]];
                self.place_template(&template, mirror, rotation, at, chunk, None, random)?;
                Ok(true)
            }
            TreeKind::Overlay {
                base,
                overlay,
                integrity,
            } => {
                if !(self.valid_ground(origin, mirror, rotation, &tree.placement)?
                    && self.valid_trunk(origin, mirror, rotation, &tree.placement)?)
                {
                    return Ok(false);
                }
                if !self.roots_and_trunk(tree, &mut pos, mirror, rotation, random)? {
                    return Ok(false);
                }
                for (template, rot) in [(base, None), (overlay, Some(*integrity))] {
                    let centre = transformed(
                        [(template.size[0] - 1) / 2, 0, (template.size[2] - 1) / 2],
                        mirror,
                        rotation,
                    );
                    let at = [pos[0] - centre[0], pos[1] - centre[1], pos[2] - centre[2]];
                    self.place_template(template, mirror, rotation, at, chunk, rot, random)?;
                }
                Ok(true)
            }
            TreeKind::Stacked(layers) => {
                if !self.valid_ground(origin, mirror, rotation, &tree.placement)? {
                    return Ok(false);
                }
                if !self.roots_and_trunk(tree, &mut pos, mirror, rotation, random)? {
                    return Ok(false);
                }
                for layer in layers {
                    let count = if layer.max == layer.min {
                        layer.min
                    } else {
                        layer.min + rnd(random.next_int_bound(1 + layer.max - layer.min))?
                    };
                    for _ in 0..count {
                        let template = layer.templates
                            [rnd(random.next_int_bound(layer.templates.len() as i32))? as usize]
                            .clone();
                        let centre = transformed(
                            [(template.size[0] - 1) / 2, 0, (template.size[2] - 1) / 2],
                            mirror,
                            rotation,
                        );
                        let at = [pos[0] - centre[0], pos[1] - centre[1], pos[2] - centre[2]];
                        self.place_template(&template, mirror, rotation, at, chunk, None, random)?;
                        pos[1] += template.size[1];
                    }
                }
                Ok(true)
            }
        }
    }
    /// The root system, then the trunk; moves `pos` up the trunk. `false` when required roots failed.
    fn roots_and_trunk(
        &mut self,
        tree: &Tree,
        pos: &mut [i32; 3],
        mirror: Mirror,
        rotation: Rotation,
        random: &mut impl RandomSource,
    ) -> R<bool, W> {
        if let Some(roots) = &tree.roots {
            if !(self.place_roots(pos, roots, random)? || !roots.required) {
                return Ok(false);
            }
        }
        if let Some(trunk) = &tree.trunk {
            let height = self.place_trunk(*pos, trunk, mirror, rotation, random)?;
            pos[1] += height;
        }
        Ok(true)
    }

    fn krummholz(
        &mut self,
        k: &Krummholz,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let height = if k.max > k.min {
            k.min + rnd(random.next_int_bound(k.max - k.min + 1))?
        } else {
            k.min
        };
        let below = self.at([origin[0], origin[1] - 1, origin[2]])?;
        let mut valid = is_dirt(&below);
        if k.on_stone {
            valid |= below.contains("stone") || below.starts_with("tfc:raw_");
        }
        if k.on_gravel {
            valid |= is_gravel(&below);
        }
        if !valid || is_fluid_block(&self.at(origin)?) {
            return Ok(false);
        }
        let mut max_height = 0;
        for i in 0..height {
            if !replaceable(&self.at([origin[0], origin[1] + i, origin[2]])?) {
                break;
            }
            max_height = i + 1;
        }
        if max_height == 0 {
            return Ok(false);
        }
        let registry = self.res.registry;
        let variant = |tip: bool, bottom: bool| {
            let mut state = registry.state(k.block).unwrap().clone();
            for (key, on) in [("tip", tip), ("bottom", bottom)] {
                if let Some(slot) = state.properties.get_mut(key) {
                    *slot = on.to_string();
                }
            }
            registry.id(&state).unwrap_or(k.block)
        };
        for i in 0..max_height {
            let state = variant(i == max_height - 1, i == 0);
            self.put([origin[0], origin[1] + i, origin[2]], state, 2)?;
        }
        Ok(true)
    }

    fn planted(
        &mut self,
        planted: &Planted,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        match planted {
            Planted::Tree(tree) => self.tree(tree, random, origin),
            Planted::Krummholz(k) => self.krummholz(k, random, origin),
        }
    }

    // ---- soil

    fn soil(
        &mut self,
        soil: &Soil,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        match &soil.shape {
            SoilShape::Disc {
                min_radius,
                max_radius,
                integrity,
            } => {
                let radius = if max_radius > min_radius {
                    min_radius + rnd(random.next_int_bound(max_radius - min_radius))?
                } else {
                    *min_radius
                };
                // `Metaballs2D.simple(fork(random), radius * 2)`.
                let mut fork = XoroshiroRandom::from_state(
                    random.next_long() as u64,
                    random.next_long() as u64,
                );
                let size = (radius * 2) as f32;
                let (min_size, max_size, ball_radius) = (
                    f64::from(0.1f32 * size),
                    f64::from(0.3f32 * size),
                    f64::from(0.5f32 * size),
                );
                let count = uniform_int(&mut fork, 3, 8).map_err(unit)?;
                let balls: Vec<[f64; 3]> = (0..count)
                    .map(|_| {
                        [
                            triangle(&mut fork, ball_radius),
                            triangle(&mut fork, ball_radius),
                            uniform_double(&mut fork, min_size, max_size),
                        ]
                    })
                    .collect();
                let inside = |x: f64, z: f64| {
                    let mut f = 0.0;
                    for b in &balls {
                        f +=
                            b[2] * b[2].abs() / ((x - b[0]) * (x - b[0]) + (z - b[1]) * (z - b[1]));
                    }
                    f > f64::from(1.0f32)
                };
                let mut placed = false;
                for x in origin[0] - radius..=origin[0] + radius {
                    for z in origin[2] - radius..=origin[2] + radius {
                        if !inside(f64::from(x - origin[0]), f64::from(z - origin[2])) {
                            continue;
                        }
                        for y in origin[1] - soil.height..=origin[1] + soil.height {
                            if random.next_float() < *integrity {
                                let n = self.at([x, y, z])?;
                                if let Some(state) = soil.states.get(&n) {
                                    self.put([x, y, z], *state, 2)?;
                                    placed = true;
                                }
                            }
                        }
                    }
                }
                Ok(placed)
            }
            SoilShape::ForestArea {
                min_forest,
                max_forest,
                spread,
                noise_scale,
                inverted,
            } => {
                let seed = self
                    .world
                    .world_seed()
                    .map_err(FeatureExecutionError::Query)?;
                let Some(terrain) = tfc_terrain::active_for(seed) else {
                    return Ok(false);
                };
                let noise = OpenSimplex2D::new(913703);
                let (cx, cz) = (origin[0] >> 4 << 4, origin[2] >> 4 << 4);
                let d = |dx: i32, dz: i32| f64::from(terrain.forest_density(cx + dx, cz + dz));
                let centre = d(0, 0);
                let (north, south, east, west) = (d(0, -16), d(0, 16), d(16, 0), d(-16, 0));
                let d00 = 0.25 * (centre + west + north + d(-16, -16));
                let d10 = 0.25 * (centre + east + north + d(16, -16));
                let d01 = 0.25 * (centre + west + south + d(-16, 16));
                let d11 = 0.25 * (centre + east + south + d(16, 16));
                let (delta_x, delta_z, delta_xz) = (d10 - d00, d01 - d00, d00 - d10 - d01 + d11);
                let poly = |x: f64| 3.0 * x * x - 2.0 * x * x * x;
                let mut placed = false;
                for dx in -3..=18 {
                    let dxc = dx.clamp(0, 15);
                    let var_dx = poly(f64::from(dxc) * (1.0 / 15.0));
                    let term_dx = delta_x * var_dx;
                    for dz in -3..=18 {
                        let dzc = dz.clamp(0, 15);
                        let var_dz = poly(f64::from(dzc) * (1.0 / 15.0));
                        let density = d00 + term_dx + delta_z * var_dz + delta_xz * var_dx * var_dz;
                        let (x, z) = (cx + dx, cz + dz);
                        let blob = noise
                            .noise(f64::from(x as f32 * spread), f64::from(z as f32 * spread))
                            * f64::from(*noise_scale);
                        let mut v = clamped_map(
                            density + blob,
                            f64::from(*min_forest) - 0.25,
                            f64::from(*max_forest) + 0.25,
                            0.0,
                            1.0,
                        );
                        if *inverted {
                            v = 1.0 - v;
                        }
                        if dxc != dx || dzc != dz {
                            let edge = 0.5 - noise.noise(f64::from(x), f64::from(z)).abs();
                            if dxc != dx {
                                v = clamped_map(f64::from((dxc - dx).abs()), 0.0, 4.0, v, edge);
                            }
                            if dzc != dz {
                                v = clamped_map(f64::from((dzc - dz).abs()), 0.0, 4.0, v, edge);
                            }
                        }
                        if v >= 0.5 {
                            let surface = self.height(HeightmapType::OceanFloorWg, x, z)? - 1;
                            for y in surface - soil.height..=surface {
                                let n = self.at([x, y, z])?;
                                if let Some(state) = soil.states.get(&n) {
                                    self.put([x, y, z], *state, 2)?;
                                    placed = true;
                                }
                            }
                        }
                    }
                }
                Ok(placed)
            }
        }
    }

    // ---- forest

    /// `ForestFeature.getTrees`: the entries valid at `pos`, nearest the "mean" first.
    fn entries_at<'e>(
        &mut self,
        entries: &'e [Entry],
        x: i32,
        y: i32,
        z: i32,
    ) -> R<Vec<&'e Entry>, W> {
        let seed = self
            .world
            .world_seed()
            .map_err(FeatureExecutionError::Query)?;
        let Some(terrain) = tfc_terrain::active_for(seed) else {
            return Ok(Vec::new());
        };
        let sample = terrain.climate_at(x, z);
        let scale = terrain.hemisphere_scale();
        let north = if scale == 0.0 {
            true
        } else {
            let adjusted = z.wrapping_sub((scale / 2.0) as i32);
            let pole_to_pole = (scale * 2.0) as i32;
            adjusted.rem_euclid(pole_to_pole * 2) > pole_to_pole
        };
        let rain_var = sample.rain_variance * if north { 1.0 } else { -1.0 };
        let temperature = adjust_temp(y, sample.sea_level_temperature);
        let elevation = y as f32;
        let mut list: Vec<(&Entry, f32)> = entries
            .iter()
            .filter(|e| e.valid(temperature, sample.groundwater, rain_var, elevation))
            .map(|e| {
                (
                    e,
                    e.distance(temperature, sample.groundwater, rain_var, elevation),
                )
            })
            .collect();
        list.sort_by(|a, b| f64::from(a.1).total_cmp(&f64::from(b.1)));
        Ok(list.into_iter().map(|e| e.0).collect())
    }
    /// The `maxSize` / alternate trimming shared by `getTree` and the soil discs.
    fn trim(list: &mut Vec<&Entry>, max: i32, alternate: i32) {
        list.truncate((max.max(0)) as usize);
        let mut alternate = alternate;
        while list.len() > 1 && alternate > 0 {
            list.remove(0);
            alternate -= 1;
        }
    }
    fn pick_tree<'e>(
        &mut self,
        entries: &'e [Entry],
        random: &mut impl RandomSource,
        pos: [i32; 3],
        ty: &super::tfc_forest::ForestTypeData,
    ) -> R<Option<&'e Entry>, W> {
        let mut list = self.entries_at(entries, pos[0], pos[1], pos[2])?;
        if list.is_empty() {
            return Ok(None);
        }
        if list.len() == 1 {
            return Ok(Some(list[0]));
        }
        Self::trim(&mut list, ty.max_tree_types, ty.alternate_size);
        let mut index = 0;
        while index < list.len() - 1 && random.next_float() < 0.6 {
            index += 1;
        }
        Ok(Some(list[index]))
    }
    fn column(
        &mut self,
        origin: [i32; 3],
        random: &mut impl RandomSource,
        kind: HeightmapType,
    ) -> R<[i32; 3], W> {
        let x = origin[0] + rnd(random.next_int_bound(16))?;
        let z = origin[2] + rnd(random.next_int_bound(16))?;
        Ok([x, self.height(kind, x, z)?, z])
    }

    fn forest(
        &mut self,
        entries: &[Entry],
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let seed = self
            .world
            .world_seed()
            .map_err(FeatureExecutionError::Query)?;
        let Some(terrain) = tfc_terrain::active_for(seed) else {
            return Ok(false);
        };
        let forest = terrain.climate_at(origin[0], origin[2]).forest;
        let ty = *forest.data();

        // placeSoilDisc: before the chance check.
        let y = self.height(HeightmapType::OceanFloorWg, origin[0], origin[2])?;
        let mut list = self.entries_at(entries, origin[0], y, origin[2])?;
        let original = list.len();
        for _ in ty.max_tree_types.max(0) as usize..original {
            list.pop();
        }
        let mut alternate = ty.alternate_size;
        while list.len() > 1 && alternate > 0 {
            list.remove(0);
            alternate -= 1;
        }
        for entry in &list {
            if let Some(soil) = &entry.soil {
                self.soil(soil, random, [origin[0], y, origin[2]])?;
            }
        }

        if random.next_float() > ty.per_chunk_chance {
            return Ok(false);
        }
        let tree_count = ty.tree_count.sample(random);
        let bush_count = ty.bush_count.sample(random);
        let (mut trees, mut bushes) = (false, false);
        for _ in 0..tree_count {
            trees |= self.forest_tree(
                entries,
                random,
                origin,
                &ty,
                forest.is_dead(),
                forest.is_primary(),
            )?;
        }
        for _ in 0..bush_count {
            bushes |= self.forest_bush(entries, random, origin, &ty)?;
        }
        if trees {
            let tries = ty.groundcover_count.sample(random);
            self.groundcover(entries, random, origin, &ty, tries)?;
            let tries = ty.leaf_pile_count.sample(random);
            self.leaf_piles(entries, random, origin, &ty, tries)?;
            self.fallen_tree(entries, random, origin, &ty)?;
        }
        Ok(trees || bushes)
    }

    fn forest_tree(
        &mut self,
        entries: &[Entry],
        random: &mut impl RandomSource,
        origin: [i32; 3],
        ty: &super::tfc_forest::ForestTypeData,
        dead: bool,
        primary: bool,
    ) -> R<bool, W> {
        let mut pos = self.column(origin, random, HeightmapType::OceanFloorWg)?;
        let Some(entry) = self.pick_tree(entries, random, pos, ty)? else {
            return Ok(false);
        };
        if entry.floating {
            if pos[1] < SEA_LEVEL - 4 {
                return Ok(false);
            }
            pos[1] = self.height(HeightmapType::WorldSurfaceWg, pos[0], pos[2])?
                + rnd(random.next_int_bound(2))?;
        }
        let seed = self
            .world
            .world_seed()
            .map_err(FeatureExecutionError::Query)?;
        let Some(terrain) = tfc_terrain::active_for(seed) else {
            return Ok(false);
        };
        let sea_temp = terrain.climate_at(pos[0], pos[2]).sea_level_temperature;
        let block_temp = adjust_temp(pos[1], sea_temp);
        let min_temp = entry.climate.min_temperature;
        let krum_chance = if block_temp > min_temp + 5.0 {
            0.0
        } else {
            clamped_map(
                f64::from(block_temp),
                f64::from(min_temp + 2.5),
                f64::from(min_temp + 5.0),
                1.0,
                0.0,
            ) as f32
        };
        let krummholz = match &entry.krummholz {
            Some(k) if random.next_float() < krum_chance => Some(k),
            _ => None,
        };
        let planted = if let Some(k) = krummholz {
            k
        } else if primary
            && entry.old_growth_chance > 0
            && rnd(random.next_int_bound(entry.old_growth_chance))? == 0
        {
            entry.old_growth.as_ref().unwrap_or(&entry.normal)
        } else if entry.dead_chance > 0
            && (rnd(random.next_int_bound(entry.dead_chance))? == 0 || dead)
        {
            &entry.dead
        } else if entry.spoiler_chance > 0 && rnd(random.next_int_bound(entry.spoiler_chance))? == 0
        {
            entry.old_growth.as_ref().unwrap_or(&entry.normal)
        } else {
            &entry.normal
        };
        self.planted(planted, random, pos)
    }

    fn forest_bush(
        &mut self,
        entries: &[Entry],
        random: &mut impl RandomSource,
        origin: [i32; 3],
        ty: &super::tfc_forest::ForestTypeData,
    ) -> R<bool, W> {
        let mut pos = self.column(origin, random, HeightmapType::WorldSurfaceWg)?;
        let Some(entry) = self.pick_tree(entries, random, pos, ty)? else {
            return Ok(false);
        };
        let at = self.at(pos)?;
        let below = self.at([pos[0], pos[1] - 1, pos[2]])?;
        if !(replaceable(&at) && !is_fluid_block(&at) && is_bush_plantable_on(&below)) {
            return Ok(false);
        }
        if let (Some(log), Some(leaves)) = (entry.bush_log, entry.bush_leaves) {
            self.bush_part(pos, log, leaves, 1.0, random, true)?;
            for _ in 0..5 {
                if rnd(random.next_int_bound(4))? == 0 {
                    let dir =
                        [[0, -1], [1, 0], [0, 1], [-1, 0]][rnd(random.next_int_bound(4))? as usize];
                    pos = add(pos, [dir[0], 0, dir[1]]);
                    self.bush_part(pos, leaves, leaves, 0.7, random, false)?;
                    if rnd(random.next_int_bound(6))? == 0 {
                        pos[1] += 1;
                        self.bush_part(pos, leaves, leaves, 0.6, random, false)?;
                        break;
                    }
                }
            }
        }
        Ok(true)
    }
    fn bush_part(
        &mut self,
        pos: [i32; 3],
        log: BlockStateId,
        leaves: BlockStateId,
        decay: f32,
        random: &mut impl RandomSource,
        needs_empty_centre: bool,
    ) -> R<(), W> {
        if replaceable(&self.at(pos)?) {
            self.put(pos, log, 2)?;
        } else if needs_empty_centre {
            return Ok(());
        }
        // Up, north, south, west, east.
        for d in [[0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]] {
            let p = add(pos, d);
            if replaceable(&self.at(p)?) && random.next_float() < decay {
                self.put(p, leaves, 2)?;
            }
        }
        Ok(())
    }

    fn groundcover(
        &mut self,
        entries: &[Entry],
        random: &mut impl RandomSource,
        origin: [i32; 3],
        ty: &super::tfc_forest::ForestTypeData,
        tries: i32,
    ) -> R<(), W> {
        if tries == 0 {
            return Ok(());
        }
        let pos = self.column(origin, random, HeightmapType::OceanFloor)?;
        let Some(entry) = self.pick_tree(entries, random, pos, ty)? else {
            return Ok(());
        };
        let Some(cover) = &entry.groundcover else {
            return Ok(());
        };
        for _ in 0..tries {
            let state = cover.get(random);
            let p = self.column(origin, random, HeightmapType::OceanFloor)?;
            let at = self.at(p)?;
            let below = self.at([p[0], p[1] - 1, p[2]])?;
            if let Some(state) = state {
                if !is_fluid_block(&at) && replaceable(&at) && sturdy(&below) {
                    self.put(p, state, 2)?;
                }
            }
        }
        Ok(())
    }

    fn leaf_piles(
        &mut self,
        entries: &[Entry],
        random: &mut impl RandomSource,
        origin: [i32; 3],
        ty: &super::tfc_forest::ForestTypeData,
        tries: i32,
    ) -> R<(), W> {
        let pos = self.column(origin, random, HeightmapType::OceanFloor)?;
        let Some(entry) = self.pick_tree(entries, random, pos, ty)? else {
            return Ok(());
        };
        let Some(leaves) = entry.fallen_leaves else {
            return Ok(());
        };
        let registry = self.res.registry;
        for _ in 0..tries {
            let centre = self.column(origin, random, HeightmapType::OceanFloor)?;
            for _ in 0..8 {
                let dx = next_between(random, -2, 2)?;
                let dz = next_between(random, -2, 2)?;
                let p = add(centre, [dx, 0, dz]);
                let at = self.at(p)?;
                let below = self.at([p[0], p[1] - 1, p[2]])?;
                if !is_fluid_block(&at) && sturdy(&below) && replaceable(&at) {
                    let layers = next_between(random, 1, 5)?;
                    let mut state = registry.state(leaves).unwrap().clone();
                    if let Some(slot) = state.properties.get_mut("layers") {
                        let previous = std::mem::replace(slot, layers.to_string());
                        if registry.id(&state).is_none() {
                            *state.properties.get_mut("layers").unwrap() = previous;
                        }
                    }
                    let id = registry.id(&state).unwrap_or(leaves);
                    self.put(p, id, 3)?;
                }
            }
        }
        Ok(())
    }

    fn fallen_tree(
        &mut self,
        entries: &[Entry],
        random: &mut impl RandomSource,
        origin: [i32; 3],
        ty: &super::tfc_forest::ForestTypeData,
    ) -> R<(), W> {
        let pos = self.column(origin, random, HeightmapType::OceanFloor)?;
        let below = self.at([pos[0], pos[1] - 1, pos[2]])?;
        if !(is_bush_plantable_on(&below) || is_sea_bush_plantable_on(&below)) {
            return Ok(());
        }
        let Some(entry) = self.pick_tree(entries, random, pos, ty)? else {
            return Ok(());
        };
        if entry.fallen_chance <= 0 || rnd(random.next_int_bound(entry.fallen_chance))? != 0 {
            return Ok(());
        }
        let Some(log) = entry.fallen_log else {
            return Ok(());
        };
        let dir = [[0, -1], [1, 0], [0, 1], [-1, 0]][rnd(random.next_int_bound(4))? as usize];
        let registry = self.res.registry;
        let mut state = registry.state(log).unwrap().clone();
        if let Some(slot) = state.properties.get_mut("axis") {
            *slot = if dir[0] != 0 { "x" } else { "z" }.into();
        }
        let log = registry.id(&state).unwrap_or(log);
        let length = 4 + rnd(random.next_int_bound(10))?;
        let mut moment = vec![false; length as usize];
        let mut p = pos;
        let mut valid = 0usize;
        while valid < length as usize {
            let n = self.at(p)?;
            if replaceable(&n) || is_leaves(&n) {
                let under = self.at([p[0], p[1] - 1, p[2]])?;
                moment[valid] = sturdy(&under);
            } else {
                break;
            }
            p = add(p, [dir[0], 0, dir[1]]);
            valid += 1;
        }
        let mut left = 0usize;
        while left < moment.len() && !moment[left] {
            left += 1;
        }
        let mut right = valid as i64 - 1;
        while right >= 0 && !moment[right as usize] {
            right -= 1;
        }
        let half = (valid / 2) as i64;
        if left as i64 <= half && right >= half && valid >= 3 {
            let mut p = pos;
            for _ in 0..valid {
                self.put(p, log, 2)?;
                p = add(p, [dir[0], 0, dir[1]]);
            }
        }
        Ok(())
    }
}

fn unit<E>(e: FeatureExecutionError<()>) -> FeatureExecutionError<E> {
    match e {
        FeatureExecutionError::Random(r) => FeatureExecutionError::Random(r),
        _ => unreachable!("only random errors carry no world"),
    }
}

/// `Mth.clampedMap` (the double overload).
fn clamped_map(value: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    let t = ((value - in_min) / (in_max - in_min)).clamp(0.0, 1.0);
    out_min + t * (out_max - out_min)
}
/// `EnvironmentHelpers.adjustAvgTempForElev`.
fn adjust_temp(y: i32, sea_level_temperature: f32) -> f32 {
    sea_level_temperature - ((y - SEA_LEVEL) as f32 * 0.16225).clamp(0.0, 17.822)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        nbt::{self, Limits, NamedTag, Tag},
        registry::BlockRegistry,
        worldgen::{
            anchor::WorldGenerationContext, placement::PlacementQueryError,
            structure::StateTransforms,
        },
    };
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use std::io::Write;

    const NAMES: [&str; 10] = [
        "minecraft:dirt",
        "minecraft:rooted_dirt",
        "minecraft:mangrove_roots",
        "minecraft:air",
        "tfc:grass/entisol",
        "tfc:wood/log/oak",
        "tfc:wood/leaves/oak",
        "minecraft:stone",
        "minecraft:cave_air",
        "minecraft:void_air",
    ];

    fn registry() -> &'static BlockRegistry {
        let rows: Vec<_> = NAMES
            .iter()
            .map(|n| json!([n, n.ends_with("air"), false, "", {"id": n, "properties": {}}]))
            .collect();
        let defaults: BTreeMap<_, _> = NAMES.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let facts = json!({"states": rows, "defaults": defaults});
        Box::leak(Box::new(
            BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
        ))
    }

    /// A 1x2x1 template: a log under a leaf block, plus an air block that must not be placed.
    fn resources() -> Arc<StructureResources> {
        let dir = std::env::temp_dir().join(format!("tfc_tree_test_{}", std::process::id()));
        let path = dir.join("tfc/oak");
        std::fs::create_dir_all(&path).unwrap();
        let block = |y: i32, state: i32| {
            Tag::compound([
                (
                    "pos",
                    Tag::List(vec![Tag::Int(0), Tag::Int(y), Tag::Int(0)]),
                ),
                ("state", Tag::Int(state)),
            ])
        };
        let root = Tag::compound([
            (
                "size",
                Tag::List(vec![Tag::Int(1), Tag::Int(3), Tag::Int(1)]),
            ),
            (
                "palette",
                Tag::List(vec![
                    Tag::compound([("Name", Tag::String("tfc:wood/log/oak".into()))]),
                    Tag::compound([("Name", Tag::String("tfc:wood/leaves/oak".into()))]),
                    Tag::compound([("Name", Tag::String("minecraft:air".into()))]),
                ]),
            ),
            (
                "blocks",
                Tag::List(vec![block(0, 0), block(1, 1), block(2, 2)]),
            ),
        ]);
        let bytes = nbt::write_named(
            &NamedTag {
                name: "".into(),
                tag: root,
            },
            Limits::templates(),
        )
        .unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&bytes).unwrap();
        std::fs::write(path.join("1.nbt"), gz.finish().unwrap()).unwrap();
        Arc::new(StructureResources::new(
            registry(),
            StateTransforms::default(),
            &dir,
            BTreeMap::new(),
            &BTreeMap::new(),
        ))
    }

    struct World {
        blocks: HashMap<[i32; 3], BlockStateId>,
        air: BlockStateId,
    }
    impl FeatureWorld for World {
        type Error = ();
        fn block(&mut self, p: [i32; 3]) -> Result<BlockStateId, ()> {
            Ok(*self.blocks.get(&p).unwrap_or(&self.air))
        }
        fn set_block(&mut self, p: [i32; 3], s: BlockStateId, _: i32) -> Result<bool, ()> {
            self.blocks.insert(p, s);
            Ok(true)
        }
    }
    impl PlacementContext for World {
        fn generation(&self) -> WorldGenerationContext {
            WorldGenerationContext::new(-64, 384, 63)
        }
        fn min_y(&mut self) -> Result<i32, PlacementQueryError> {
            Ok(-64)
        }
        fn max_y(&mut self) -> Result<i32, PlacementQueryError> {
            Ok(319)
        }
        fn height(&mut self, _: HeightmapType, _: i32, _: i32) -> Result<i32, PlacementQueryError> {
            Err(PlacementQueryError::Unavailable)
        }
    }

    fn setup(ground: &str) -> (TfcTreeFeature, World) {
        let registry = registry();
        let bindings = FeatureBlockBindings::new(registry, BTreeSet::new())
            .unwrap()
            .with_structures(resources());
        let feature = TfcTreeFeature::decode(
            &json!({
                "type": "tfc:random_tree",
                "structures": ["tfc:oak/1"],
                "placement": {"width": 1, "height": 3},
                "trunk": {"state": "tfc:wood/log/oak", "min_height": 2, "max_height": 2, "wide": false}
            })
            .to_string(),
            &bindings,
        )
        .unwrap();
        let air = registry.default_state("minecraft:air").unwrap();
        let mut world = World {
            blocks: HashMap::new(),
            air,
        };
        world
            .blocks
            .insert([0, -1, 0], registry.default_state(ground).unwrap());
        (feature, world)
    }

    #[test]
    fn a_tree_grows_on_grass_with_a_trunk_and_template() {
        let (feature, mut world) = setup("tfc:grass/entisol");
        let mut random = XoroshiroRandom::new(7);
        assert!(feature.place(&mut world, &mut random, [0, 0, 0]).unwrap());
        let registry = registry();
        let (log, leaves) = (
            registry.default_state("tfc:wood/log/oak").unwrap(),
            registry.default_state("tfc:wood/leaves/oak").unwrap(),
        );
        // The trunk is two logs, then the template's log and leaf sit on top; its air is skipped.
        for y in 0..3 {
            assert_eq!(world.blocks[&[0, y, 0]], log, "log at {y}");
        }
        assert_eq!(world.blocks[&[0, 3, 0]], leaves);
        assert!(!world.blocks.contains_key(&[0, 4, 0]));
    }

    #[test]
    fn mangrove_roots_lift_the_trunk_and_ring_its_base() {
        let registry = registry();
        let bindings = FeatureBlockBindings::new(registry, BTreeSet::new())
            .unwrap()
            .with_structures(resources());
        let feature = TfcTreeFeature::decode(
            &json!({
                "type": "tfc:random_tree",
                "structures": ["tfc:oak/1"],
                "placement": {"width": 1, "height": 3},
                "trunk": {"state": "tfc:wood/log/oak", "min_height": 2, "max_height": 2, "wide": false},
                "root_system": {
                    "blocks": [{"replace": ["minecraft:dirt"], "with": [{"block": "minecraft:rooted_dirt"}]}],
                    "width": 15, "height": 8, "tries": 1, "required": true,
                    "special_placer": {"skew_chance": 0.2}
                }
            })
            .to_string(),
            &bindings,
        )
        .unwrap();
        let (roots, rooted) = (
            registry.default_state("minecraft:mangrove_roots").unwrap(),
            registry.default_state("minecraft:rooted_dirt").unwrap(),
        );
        let log = registry.default_state("tfc:wood/log/oak").unwrap();
        let mut lifts = BTreeSet::new();
        for seed in 0..30 {
            let (_, mut world) = setup("minecraft:dirt");
            assert!(
                feature
                    .place(&mut world, &mut XoroshiroRandom::new(seed), [0, 0, 0])
                    .unwrap()
            );
            // The trunk starts where the roots left the base: up to three blocks above the ground.
            let base = (0..4).find(|y| world.blocks.get(&[0, *y, 0]) == Some(&log));
            let Some(base) = base else {
                panic!("seed {seed}: no trunk");
            };
            lifts.insert(base);
            // Dirt under the base turns to rooted dirt; air under it holds a root.
            let under = world.blocks[&[0, base - 1, 0]];
            assert_eq!(under, if base == 0 { rooted } else { roots }, "seed {seed}");
            // All four sides of the base hold a root (the logs of a 1x1 trunk are at x=z=0).
            for d in [[0, -1], [1, 0], [0, 1], [-1, 0]] {
                assert_eq!(
                    world.blocks[&[d[0], base, d[1]]],
                    roots,
                    "seed {seed} side {d:?}"
                );
            }
            // Every root is within the configured reach of the trunk.
            for (p, s) in &world.blocks {
                if *s == roots {
                    assert!(
                        p[0].abs() <= 16 && p[2].abs() <= 16 && p[1] >= base - 9,
                        "{p:?}"
                    );
                }
            }
        }
        assert!(
            lifts.len() > 1,
            "the base is lifted by a random amount: {lifts:?}"
        );
    }

    #[test]
    fn a_tree_refuses_stone_ground() {
        let (feature, mut world) = setup("minecraft:stone");
        let mut random = XoroshiroRandom::new(7);
        assert!(!feature.place(&mut world, &mut random, [0, 0, 0]).unwrap());
        assert_eq!(world.blocks.len(), 1);
    }

    #[test]
    fn forest_entries_rank_by_climate_distance() {
        let c = |min: f32, max: f32| Climate {
            min_temperature: min,
            max_temperature: max,
            min_groundwater: 0.0,
            max_groundwater: 500.0,
            min_rain_variance: -1.0,
            max_rain_variance: 1.0,
            absolute: false,
            min_elevation: -64,
            max_elevation: 320,
        };
        let entry = |climate| {
            let tree = Tree {
                kind: TreeKind::Random(vec![]),
                trunk: None,
                placement: Placement {
                    width: 1,
                    height: 1,
                    ground: Ground::Normal,
                },
                roots: None,
            };
            Entry {
                climate,
                bush_log: None,
                bush_leaves: None,
                fallen_log: None,
                fallen_leaves: None,
                groundcover: None,
                normal: Planted::Tree(tree.clone()),
                dead: Planted::Tree(tree),
                old_growth: None,
                krummholz: None,
                soil: None,
                old_growth_chance: 6,
                spoiler_chance: 200,
                fallen_chance: 14,
                dead_chance: 75,
                floating: false,
            }
        };
        let (cold, warm) = (entry(c(-10.0, 5.0)), entry(c(10.0, 30.0)));
        assert!(cold.valid(0.0, 200.0, 0.0, 70.0) && !warm.valid(0.0, 200.0, 0.0, 70.0));
        assert!(warm.valid(20.0, 200.0, 0.0, 70.0));
        // Distance: (temp - (max-min)/2) * 10 + (water - 250) + (var - 1) * 250 + (elev - 192) * 5.
        let d = cold.distance(0.0, 200.0, 0.0, 70.0);
        assert!(
            (d - ((0.0 - 7.5) * 10.0
                + (200.0 - 250.0)
                + (0.0 - 1.0) * 250.0
                + (70.0 - 192.0) * 5.0))
                .abs()
                < 1e-3
        );
    }
}
