//! TerraFirmaCraft's remaining plant features (EUPL 1.2): `tfc:fruit_trees`
//! (`FruitTreeFeature`), `tfc:spreading_bush` (`SpreadingBushFeature`), `tfc:bananas`
//! (`BananaFeature`), `tfc:bamboo` (`TFCBambooFeature`), `tfc:vines` (`TFCVinesFeature`) and
//! `tfc:kelp` (`TFCKelpFeature`). Draw order follows the Java; block tags and `canSurvive` tests
//! are matched by block name, and block properties a mod block lacks are skipped.

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    density::identifier,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    integer, optional,
    placement::PlacementContext,
    required,
    tfc_tree::{
        air_or_empty_fluid, is_air, is_bush_plantable_on, is_dirt, is_fluid_block, is_gravel,
        is_leaves, is_sand, replaceable, single_block_replaceable, sturdy,
    },
};
use crate::{
    heightmap::HeightmapType,
    registry::{BlockRegistry, BlockStateId},
};
use horizon_core::random::RandomSource;
use std::{collections::HashMap, sync::Arc};

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

/// `Direction.Plane.HORIZONTAL`: north, east, south, west.
const HORIZONTAL: [(&str, i32, i32); 4] = [
    ("north", 0, -1),
    ("east", 1, 0),
    ("south", 0, 1),
    ("west", -1, 0),
];

/// Face names in `Direction` order: down, up, north, south, west, east.
const SIDES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];
const SIDE_OFFSETS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

/// `#c:stones`, smooth stone, cobblestone and sandstone blocks by name.
fn is_stoneish(n: &str) -> bool {
    !n.contains("brick")
        && !n.contains("slab")
        && !n.contains("stairs")
        && !n.contains("wall")
        && (n.starts_with("tfc:raw_")
            || n.starts_with("tfc:cobble_")
            || n.starts_with("tfc:smooth_")
            || [
                "stone",
                "granite",
                "diorite",
                "andesite",
                "deepslate",
                "basalt",
                "tuff",
                "cobble",
                "sandstone",
            ]
            .iter()
            .any(|k| n.contains(k)))
}
/// `#tfc:anemone_plantable_on`: stones plus gravel and sand.
fn anemone_plantable_on(n: &str) -> bool {
    is_stoneish(n) || is_gravel(n) || is_sand(n)
}

#[derive(Clone, Debug)]
enum Kind {
    FruitTree {
        /// `saplings` 2, 3, 4 with `natural` set.
        branch: [BlockStateId; 3],
    },
    Banana {
        /// Stage 0..=2, healthy.
        stages: [BlockStateId; 3],
    },
    Bamboo {
        probability: f32,
        trunk: BlockStateId,
        final_large: BlockStateId,
        top_large: BlockStateId,
        top_small: BlockStateId,
        /// Soil states `tfc:dirt/*`/`tfc:grass/*` by name to the rooted dirt that replaces them.
        rooted: HashMap<String, BlockStateId>,
    },
    Vines {
        /// By horizontal direction, the state with only that face set (the Java accumulates, so
        /// the first accepted direction is the only one set).
        faces: [BlockStateId; 4],
    },
    Kelp {
        body: BlockStateId,
        heads: Vec<BlockStateId>,
        head_block: String,
        tries: i32,
        radius: i32,
        min_height: i32,
        max_height: i32,
    },
    /// `tfc:creeping_ocean_plant`: `states[mask * 2 + waterlogged]`, mask bits are
    /// down, up, north, south, west, east.
    Creeping {
        states: Vec<BlockStateId>,
        stone: bool,
        radius: i32,
        height: i32,
        tide_height: i32,
        integrity: f32,
    },
    /// `tfc:creeping_plant` (ivy): `states[mask]`, mask bits as above, no waterlogging.
    CreepingLand {
        states: Vec<BlockStateId>,
        radius: i32,
        height: i32,
        tide_height: i32,
        integrity: f32,
    },
    /// `tfc:rotatable_water_plant`: `states[direction * 2 + waterlogged]` for down, up, north,
    /// south, west, east.
    Rotatable { states: Vec<BlockStateId> },
    SpreadingBush {
        /// Stage 0..=2.
        bush: [BlockStateId; 3],
        /// The cane facing north, east, south, west.
        cane: [BlockStateId; 4],
        bush_name: String,
    },
}

#[derive(Clone, Debug)]
pub struct TfcPlantFeature {
    kind: Kind,
    names: Arc<Vec<String>>,
}

fn text(object: &Object, key: &'static str) -> Result<String, DecodeError> {
    Ok(serde_json::from_str(required(object, key)?.get())?)
}

/// A block id from a string or a `{ "Name": .. }` state.
fn block_name(object: &Object, key: &'static str) -> Result<String, DecodeError> {
    let value: serde_json::Value = serde_json::from_str(required(object, key)?.get())?;
    match value {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Object(o) => o
            .get("Name")
            .and_then(|n| n.as_str())
            .map(str::to_owned)
            .ok_or(DecodeError::Field("plant state name")),
        _ => Err(DecodeError::Field("plant state")),
    }
}

fn int(object: &Object, key: &'static str, default: i32) -> Result<i32, DecodeError> {
    Ok(optional(object, key)
        .map(integer)
        .transpose()?
        .unwrap_or(default))
}

/// The default state of `block` with each property that exists set.
fn with(
    registry: &BlockRegistry,
    block: &str,
    properties: &[(&str, &str)],
) -> Result<BlockStateId, DecodeError> {
    let default = registry
        .default_state(block)
        .ok_or(DecodeError::Field("plant block"))?;
    let mut state = registry.state(default).unwrap().clone();
    for (property, value) in properties {
        if state.properties.contains_key(*property) {
            state.properties.insert((*property).into(), (*value).into());
        }
    }
    registry.id(&state).ok_or(DecodeError::Field("plant state"))
}

/// `#tfc:creeping_plant_not_plantable_on`.
fn creeping_not_plantable_on(n: &str) -> bool {
    matches!(
        n,
        "minecraft:packed_ice"
            | "minecraft:snow_block"
            | "minecraft:blue_ice"
            | "tfc:sea_ice"
            | "minecraft:powder_snow"
    )
}

fn is_bamboo_plantable_on(n: &str) -> bool {
    is_bush_plantable_on(n) || is_sand(n) || is_gravel(n) || is_dirt(n)
}

/// `SPREADING_FRUIT_GROWS_ON` is `#tfc:bush_plantable_on`.
fn spreading_grows_on(n: &str) -> bool {
    is_bush_plantable_on(n)
}

/// `VineBlock.isAcceptableNeighbour`: a sturdy block to hang on.
fn vine_support(n: &str) -> bool {
    !air_or_empty_fluid(n) && !single_block_replaceable(n) && !is_leaves(n) && !n.contains("plant/")
}

impl TfcPlantFeature {
    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(json)?;
        let kind = identifier(&serde_json::from_str::<String>(
            required(&object, "type")?.get(),
        )?)?;
        let config: Object = match optional(&object, "config") {
            Some(config) => serde_json::from_str(config.get())?,
            None => object.clone(),
        };
        let registry = bindings.registry();
        let kind = match kind.as_str() {
            "tfc:fruit_trees" => {
                let b = block_name(&config, "state")?;
                let one = |n: &str| with(registry, &b, &[("natural", "true"), ("saplings", n)]);
                Kind::FruitTree {
                    branch: [one("2")?, one("3")?, one("4")?],
                }
            }
            "tfc:bananas" => {
                let b = block_name(&config, "state")?;
                let one = |n: &str| with(registry, &b, &[("stage", n), ("lifecycle", "healthy")]);
                Kind::Banana {
                    stages: [one("0")?, one("1")?, one("2")?],
                }
            }
            "tfc:bamboo" => {
                let b = block_name(&config, "state")?;
                let probability = optional(&config, "probability")
                    .map(|v| serde_json::from_str::<f32>(v.get()))
                    .transpose()?
                    .unwrap_or(0.0);
                let make = |leaves: &str, stage: &str| {
                    with(
                        registry,
                        &b,
                        &[("age", "1"), ("leaves", leaves), ("stage", stage)],
                    )
                };
                let mut rooted = HashMap::new();
                for soil in ["loam", "sandy_loam", "silt", "silty_loam"] {
                    if let Some(id) = registry.default_state(&format!("tfc:rooted_dirt/{soil}")) {
                        rooted.insert(format!("tfc:dirt/{soil}"), id);
                        rooted.insert(format!("tfc:grass/{soil}"), id);
                    }
                }
                // Horizon's soils are the vanilla stand-ins unless the mod defines TFC's.
                if let Some(id) = registry.default_state("minecraft:rooted_dirt") {
                    for soil in ["minecraft:grass_block", "minecraft:dirt"] {
                        rooted.insert(soil.to_owned(), id);
                    }
                }
                Kind::Bamboo {
                    probability,
                    trunk: make("none", "0")?,
                    final_large: make("large", "1")?,
                    top_large: make("large", "0")?,
                    top_small: make("small", "0")?,
                    rooted,
                }
            }
            "tfc:vines" => {
                let b = block_name(&config, "state")?;
                let one = |d: &str| with(registry, &b, &[(d, "true")]);
                Kind::Vines {
                    faces: [one("north")?, one("east")?, one("south")?, one("west")?],
                }
            }
            "tfc:kelp" => {
                let (body, head) = (text(&config, "body")?, text(&config, "head")?);
                let heads = if registry.property_values(&head, "age").is_empty() {
                    vec![
                        registry
                            .default_state(&head)
                            .ok_or(DecodeError::Field("kelp head"))?,
                    ]
                } else {
                    (22..=25)
                        .map(|age| with(registry, &head, &[("age", &age.to_string())]))
                        .collect::<Result<_, _>>()?
                };
                let (tries, radius) = (int(&config, "tries", 10)?, int(&config, "radius", 4)?);
                let (min_height, max_height) = (
                    int(&config, "min_height", 1)?,
                    int(&config, "max_height", 2)?,
                );
                if tries < 1 || radius < 1 || min_height < 1 || max_height < min_height {
                    return Err(DecodeError::Field("kelp size"));
                }
                Kind::Kelp {
                    body: registry
                        .default_state(&body)
                        .ok_or(DecodeError::Field("kelp body"))?,
                    heads,
                    head_block: head,
                    tries,
                    radius,
                    min_height,
                    max_height,
                }
            }
            "tfc:creeping_ocean_plant" => {
                let b = text(&config, "block")?;
                let mut states = Vec::with_capacity(128);
                for mask in 0..64 {
                    for wet in ["false", "true"] {
                        let mut props: Vec<(&str, &str)> = SIDES
                            .iter()
                            .enumerate()
                            .map(|(i, name)| {
                                (*name, if mask >> i & 1 == 1 { "true" } else { "false" })
                            })
                            .collect();
                        props.push(("waterlogged", wet));
                        states.push(with(registry, &b, &props)?);
                    }
                }
                Kind::Creeping {
                    states,
                    stone: b.contains("barnacles") || b.contains("mussels"),
                    radius: int(&config, "radius", 3)?,
                    height: int(&config, "height", 5)?,
                    tide_height: int(&config, "tide_height", 0)?,
                    integrity: optional(&config, "integrity")
                        .map(|v| serde_json::from_str::<f32>(v.get()))
                        .transpose()?
                        .unwrap_or(1.0),
                }
            }
            "tfc:creeping_plant" => {
                let b = text(&config, "block")?;
                let mut states = Vec::with_capacity(64);
                for mask in 0..64 {
                    let props: Vec<(&str, &str)> = SIDES
                        .iter()
                        .enumerate()
                        .map(|(i, name)| (*name, if mask >> i & 1 == 1 { "true" } else { "false" }))
                        .collect();
                    states.push(with(registry, &b, &props)?);
                }
                Kind::CreepingLand {
                    states,
                    radius: int(&config, "radius", 3)?,
                    height: int(&config, "height", 5)?,
                    tide_height: int(&config, "tide_height", 0)?,
                    integrity: optional(&config, "integrity")
                        .map(|v| serde_json::from_str::<f32>(v.get()))
                        .transpose()?
                        .unwrap_or(1.0),
                }
            }
            "tfc:rotatable_water_plant" => {
                let b = text(&config, "block")?;
                let mut states = Vec::with_capacity(12);
                for facing in SIDES {
                    for wet in ["false", "true"] {
                        states.push(with(
                            registry,
                            &b,
                            &[("facing", facing), ("waterlogged", wet)],
                        )?);
                    }
                }
                Kind::Rotatable { states }
            }
            "tfc:spreading_bush" => {
                let b = text(&config, "block")?;
                let stage = |n: &str| with(registry, &b, &[("stage", n), ("age", n)]);
                let cane_name = format!("{b}_cane");
                let cane = |d: &str| with(registry, &cane_name, &[("facing", d)]);
                Kind::SpreadingBush {
                    bush: [stage("0")?, stage("1")?, stage("2")?],
                    cane: [cane("north")?, cane("east")?, cane("south")?, cane("west")?],
                    bush_name: b,
                }
            }
            _ => return Err(DecodeError::Field("plant type")),
        };
        Ok(Self {
            kind,
            names: Arc::new(
                (0..registry.len())
                    .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
                    .collect(),
            ),
        })
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let mut p = Placer {
            names: &self.names,
            world,
        };
        match &self.kind {
            Kind::FruitTree { branch } => p.fruit_tree(branch, random, origin),
            Kind::Banana { stages } => p.banana(stages, random, origin),
            Kind::Bamboo {
                probability,
                trunk,
                final_large,
                top_large,
                top_small,
                rooted,
            } => p.bamboo(
                *probability,
                [*trunk, *final_large, *top_large, *top_small],
                rooted,
                random,
                origin,
            ),
            Kind::Vines { faces } => p.vines(faces, random, origin),
            Kind::Kelp {
                body,
                heads,
                head_block,
                tries,
                radius,
                min_height,
                max_height,
            } => p.kelp(
                *body,
                heads,
                head_block,
                (*tries, *radius, *min_height, *max_height),
                random,
                origin,
            ),
            Kind::Creeping {
                states,
                stone,
                radius,
                height,
                tide_height,
                integrity,
            } => p.creeping(
                states,
                *stone,
                (*radius, *height, *tide_height),
                *integrity,
                random,
                origin,
            ),
            Kind::CreepingLand {
                states,
                radius,
                height,
                tide_height,
                integrity,
            } => p.creeping_land(
                states,
                (*radius, *height, *tide_height),
                *integrity,
                random,
                origin,
            ),
            Kind::Rotatable { states } => p.rotatable(states, random, origin),
            Kind::SpreadingBush {
                bush,
                cane,
                bush_name,
            } => p.spreading_bush(bush, cane, bush_name, random, origin),
        }
    }
}

struct Placer<'a, W> {
    names: &'a [String],
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
/// `Mth.nextInt`.
fn between<E>(
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
    fn at(&mut self, p: [i32; 3]) -> R<String, W> {
        let s = self.world.block(p).map_err(we)?;
        Ok(self.names[s.0 as usize].clone())
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

    /// `FruitTreeFeature`.
    fn fruit_tree(
        &mut self,
        branch: &[BlockStateId; 3],
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let below = self.at([origin[0], origin[1] - 1, origin[2]])?;
        if !is_bush_plantable_on(&below) {
            return Ok(false);
        }
        for j in 0..10 {
            if !replaceable(&self.at([origin[0], origin[1] + j, origin[2]])?) {
                return Ok(false);
            }
        }
        let saplings = between(random, 2, 4)?;
        self.put(origin, branch[(saplings - 2) as usize], 2)?;
        Ok(true)
    }

    /// `BananaFeature`.
    fn banana(
        &mut self,
        stages: &[BlockStateId; 3],
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let below = self.at([origin[0], origin[1] - 1, origin[2]])?;
        if !is_bush_plantable_on(&below) {
            return Ok(false);
        }
        let mut y = origin[1];
        for (stage, state) in stages.iter().enumerate() {
            let height = between(random, 2, 3)?;
            for _ in 1..height {
                self.put([origin[0], y, origin[2]], *state, 2)?;
                y += 1;
                if stage == 2 {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// `TFCVinesFeature`.
    fn vines(
        &mut self,
        faces: &[BlockStateId; 4],
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        if !is_air(&self.at(origin)?) {
            return Ok(false);
        }
        for (i, (_, dx, dz)) in HORIZONTAL.iter().enumerate() {
            let neighbour = self.at([origin[0] + dx, origin[1], origin[2] + dz])?;
            if vine_support(&neighbour) {
                self.put(origin, faces[i], 2)?;
                let max_length = rnd(random.next_int_bound(14))?;
                let mut p = origin;
                for _ in 0..max_length {
                    p[1] -= 1;
                    if is_air(&self.at(p)?) {
                        self.put(p, faces[i], 2)?;
                    } else {
                        break;
                    }
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `TFCBambooFeature`.
    fn bamboo(
        &mut self,
        probability: f32,
        [trunk, final_large, top_large, top_small]: [BlockStateId; 4],
        rooted: &HashMap<String, BlockStateId>,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        if !is_air(&self.at(origin)?) {
            return Ok(false);
        }
        if is_bamboo_plantable_on(&self.at([origin[0], origin[1] - 1, origin[2]])?) {
            let trunk_size = rnd(random.next_int_bound(12))? + 5;
            if rnd(Ok::<f32, horizon_core::random::InvalidBound>(
                random.next_float(),
            ))? < probability
            {
                let radius = rnd(random.next_int_bound(4))? + 1;
                for x in origin[0] - radius..=origin[0] + radius {
                    for z in origin[2] - radius..=origin[2] + radius {
                        let (dx, dz) = (x - origin[0], z - origin[2]);
                        if dx * dx + dz * dz <= radius * radius {
                            let y = self.height(HeightmapType::WorldSurface, x, z)? - 1;
                            let name = self.at([x, y, z])?;
                            if let Some(id) = rooted.get(&name) {
                                self.put([x, y, z], *id, 2)?;
                            }
                        }
                    }
                }
            }
            let mut cursor = origin;
            let mut j = 0;
            while j < trunk_size && is_air(&self.at(cursor)?) {
                self.put(cursor, trunk, 2)?;
                cursor[1] += 1;
                j += 1;
            }
            if cursor[1] - origin[1] >= 3 {
                self.put(cursor, final_large, 2)?;
                cursor[1] -= 1;
                self.put(cursor, top_large, 2)?;
                cursor[1] -= 1;
                self.put(cursor, top_small, 2)?;
            }
        }
        Ok(true)
    }

    /// `TFCKelpFeature` (the fluid is always the single water of Horizon).
    fn kelp(
        &mut self,
        body: BlockStateId,
        heads: &[BlockStateId],
        head_block: &str,
        (tries, radius, min_height, max_height): (i32, i32, i32, i32),
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let mut placed_any = false;
        for _ in 0..tries {
            let dx = rnd(random.next_int_bound(radius))? - rnd(random.next_int_bound(radius))?;
            let dz = rnd(random.next_int_bound(radius))? - rnd(random.next_int_bound(radius))?;
            let (x, z) = (origin[0] + dx, origin[2] + dz);
            let y = self.height(HeightmapType::OceanFloor, x, z)?;
            let state = self.at([x, y, z])?;
            if !is_fluid_block(&state) {
                continue;
            }
            if self.kelp_can_place([x, y, z], &state)? {
                let height = between(random, min_height, max_height)?;
                let mut cursor = [x, y, z];
                for i in 1..=height {
                    let here = self.at(cursor)?;
                    if self.kelp_can_place(cursor, &here)? {
                        let above = self.at([cursor[0], cursor[1] + 1, cursor[2]])?;
                        if i == height || !is_fluid_block(&above) {
                            let below = self.at([cursor[0], cursor[1] - 1, cursor[2]])?;
                            if below != head_block {
                                let age = between(random, 22, 25)?;
                                let head = heads[(age - 22).min(heads.len() as i32 - 1) as usize];
                                self.put(cursor, head, 16)?;
                            }
                            break;
                        }
                        self.put(cursor, body, 16)?;
                    }
                    cursor[1] += 1;
                }
                placed_any = true;
            }
        }
        Ok(placed_any)
    }

    /// `state.canSurvive(level, pos) && FluidHelpers.isEmptyFluid(level.getBlockState(pos))` for kelp:
    /// water here, a solid block below.
    fn kelp_can_place(&mut self, pos: [i32; 3], here: &str) -> R<bool, W> {
        if !is_fluid_block(here) {
            return Ok(false);
        }
        let below = self.at([pos[0], pos[1] - 1, pos[2]])?;
        Ok(!air_or_empty_fluid(&below)
            && !single_block_replaceable(&below)
            && below != "minecraft:magma_block")
    }

    fn is_water_here(&mut self, p: [i32; 3]) -> R<bool, W> {
        Ok(is_fluid_block(&self.at(p)?) && self.at(p)? != "minecraft:lava")
    }

    /// `CreepingOceanPlantFeature`.
    fn creeping(
        &mut self,
        states: &[BlockStateId],
        stone: bool,
        (radius, height, tide_height): (i32, i32, i32),
        integrity: f32,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let seed = self
            .world
            .world_seed()
            .map_err(FeatureExecutionError::Query)?;
        let Some(terrain) = super::tfc_terrain::active_for(seed) else {
            return Ok(false);
        };
        let mut success = false;
        for x in -radius..=radius {
            for z in -radius..=radius {
                for y in 0..height {
                    if x * x + z * z < radius * radius && random.next_float() < integrity {
                        let c = [origin[0] + x, origin[1] + y, origin[2] + z];
                        if !replaceable(&self.at(c)?)
                            || f64::from(c[1])
                                > f64::from(tide_height) + terrain.tide_level(c[0], c[2])
                        {
                            continue;
                        }
                        let mut mask = 0usize;
                        for (i, d) in SIDE_OFFSETS.iter().enumerate() {
                            let n = self.at([c[0] + d[0], c[1] + d[1], c[2] + d[2]])?;
                            let ground = if stone {
                                is_stoneish(&n)
                            } else {
                                anemone_plantable_on(&n)
                            };
                            if ground {
                                mask |= 1 << i;
                            }
                        }
                        if mask != 0 {
                            let wet = usize::from(self.is_water_here(c)?);
                            self.put(c, states[mask * 2 + wet], 2)?;
                            success = true;
                        }
                    }
                }
            }
        }
        Ok(success)
    }

    /// `CreepingPlantFeature`: above the tide line, on any sturdy face except ice and snow.
    /// Always returns false, as the Java does.
    fn creeping_land(
        &mut self,
        states: &[BlockStateId],
        (radius, height, tide_height): (i32, i32, i32),
        integrity: f32,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let seed = self
            .world
            .world_seed()
            .map_err(FeatureExecutionError::Query)?;
        let Some(terrain) = super::tfc_terrain::active_for(seed) else {
            return Ok(false);
        };
        for x in -radius..=radius {
            for z in -radius..=radius {
                for y in 0..height {
                    if x * x + z * z < radius * radius && random.next_float() < integrity {
                        let c = [origin[0] + x, origin[1] + y, origin[2] + z];
                        if !replaceable(&self.at(c)?)
                            || f64::from(c[1])
                                <= f64::from(tide_height) + 0.5 + terrain.tide_level(c[0], c[2])
                        {
                            continue;
                        }
                        let mut mask = 0usize;
                        for (i, d) in SIDE_OFFSETS.iter().enumerate() {
                            let n = self.at([c[0] + d[0], c[1] + d[1], c[2] + d[2]])?;
                            if !creeping_not_plantable_on(&n) && sturdy(&n) {
                                mask |= 1 << i;
                            }
                        }
                        if mask != 0 {
                            self.put(c, states[mask], 2)?;
                        }
                    }
                }
            }
        }
        Ok(false)
    }

    /// `RotatableWaterPlantFeature`.
    fn rotatable(
        &mut self,
        states: &[BlockStateId],
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let pos = [
            origin[0],
            origin[1] + rnd(random.next_int_bound(12))?,
            origin[2],
        ];
        if !replaceable(&self.at(pos)?) {
            return Ok(false);
        }
        // `Direction.allShuffled`: `Util.shuffle` over down, up, north, south, west, east.
        let mut order = [0usize, 1, 2, 3, 4, 5];
        for j in (2..=6).rev() {
            let k = rnd(random.next_int_bound(j))? as usize;
            order.swap(j as usize - 1, k);
        }
        for d in order {
            // Attached to the block opposite the facing.
            let o = SIDE_OFFSETS[d ^ 1];
            let attached = self.at([pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]])?;
            if anemone_plantable_on(&attached) {
                let wet = usize::from(self.is_water_here(pos)?);
                self.put(pos, states[d * 2 + wet], 2)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `SpreadingBushFeature`.
    fn spreading_bush(
        &mut self,
        bush: &[BlockStateId; 3],
        cane: &[BlockStateId; 4],
        bush_name: &str,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let mut any = false;
        for _ in 0..5 {
            let dx = rnd(random.next_int_bound(3))? - rnd(random.next_int_bound(3))?;
            let dy = rnd(random.next_int_bound(2))? - rnd(random.next_int_bound(2))?;
            let dz = rnd(random.next_int_bound(3))? - rnd(random.next_int_bound(3))?;
            let mut cursor = [origin[0] + dx, origin[1] + dy, origin[2] + dz];
            let below = self.at([cursor[0], cursor[1] - 1, cursor[2]])?;
            if !(spreading_grows_on(&below) && below != bush_name) {
                continue;
            }
            let height = 1 + rnd(random.next_int_bound(3))?;
            for i in 0..height {
                if !replaceable(&self.at(cursor)?) {
                    break;
                }
                any = true;
                let stage = height - i - 1;
                self.put(cursor, bush[stage as usize], 2)?;
                let d = rnd(random.next_int_bound(4))? as usize;
                let (_, ox, oz) = HORIZONTAL[d];
                let side = [cursor[0] + ox, cursor[1], cursor[2] + oz];
                if stage > 0 && replaceable(&self.at(side)?) {
                    self.put(side, cane[d], 2)?;
                }
                cursor[1] += 1;
            }
        }
        Ok(any)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::{anchor::WorldGenerationContext, placement::PlacementQueryError};
    use horizon_core::random::XoroshiroRandom;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    fn registry() -> &'static BlockRegistry {
        const LEVELS: [&str; 16] = [
            "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15",
        ];
        let defs: [(&str, Vec<(&str, Vec<&str>)>); 14] = [
            ("minecraft:air", vec![]),
            ("minecraft:cave_air", vec![]),
            ("minecraft:void_air", vec![]),
            ("minecraft:lava", vec![("level", LEVELS.to_vec())]),
            ("minecraft:water", vec![("level", LEVELS.to_vec())]),
            ("minecraft:stone", vec![]),
            ("minecraft:dirt", vec![]),
            (
                "tfc:plant/banana_plant",
                vec![("stage", vec!["0", "1", "2"])],
            ),
            (
                "tfc:plant/raspberry_bush",
                vec![("stage", vec!["0", "1", "2"])],
            ),
            (
                "tfc:plant/raspberry_bush_cane",
                vec![("facing", vec!["north", "east", "south", "west"])],
            ),
            (
                "minecraft:kelp",
                vec![("age", vec!["22", "23", "24", "25"])],
            ),
            ("minecraft:kelp_plant", vec![]),
            (
                "tfc:plant/ivy",
                ["down", "up", "north", "south", "west", "east"]
                    .iter()
                    .map(|side| (*side, vec!["false", "true"]))
                    .collect(),
            ),
            (
                "tfc:plant/starfish",
                vec![
                    (
                        "facing",
                        vec!["down", "up", "north", "south", "west", "east"],
                    ),
                    ("waterlogged", vec!["false", "true"]),
                ],
            ),
        ];
        let mut rows = Vec::new();
        let mut defaults = BTreeMap::new();
        for (name, props) in &defs {
            defaults.insert(*name, rows.len());
            let mut combos: Vec<BTreeMap<&str, &str>> = vec![BTreeMap::new()];
            for (p, values) in props {
                combos = combos
                    .iter()
                    .flat_map(|c| {
                        values.iter().map(move |v| {
                            let mut c = c.clone();
                            c.insert(*p, *v);
                            c
                        })
                    })
                    .collect();
            }
            for c in combos {
                let liquid = name.ends_with("water") || name.ends_with("lava");
                rows.push(
                    json!([name, name.ends_with("air"), liquid, "", {"id": name, "properties": c}]),
                );
            }
        }
        let facts = json!({"states": rows, "defaults": defaults});
        Box::leak(Box::new(
            BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
        ))
    }

    struct World {
        blocks: HashMap<[i32; 3], BlockStateId>,
        air: BlockStateId,
        floor: i32,
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
            Ok(self.floor)
        }
        fn world_seed(&mut self) -> Result<i64, PlacementQueryError> {
            Ok(99)
        }
    }

    fn feature(config: serde_json::Value) -> TfcPlantFeature {
        let bindings = FeatureBlockBindings::new(registry(), BTreeSet::new()).unwrap();
        TfcPlantFeature::decode(&config.to_string(), &bindings).unwrap()
    }

    fn world(ground: &str, floor: i32) -> World {
        let registry = registry();
        let mut world = World {
            blocks: HashMap::new(),
            air: registry.default_state("minecraft:air").unwrap(),
            floor,
        };
        for x in -6..=6 {
            for z in -6..=6 {
                world
                    .blocks
                    .insert([x, -1, z], registry.default_state(ground).unwrap());
            }
        }
        world
    }

    #[test]
    fn bananas_stack_to_a_top_stage() {
        let f = feature(json!({"type": "tfc:bananas", "state": "tfc:plant/banana_plant"}));
        let mut w = world("minecraft:dirt", 0);
        assert!(
            f.place(&mut w, &mut XoroshiroRandom::new(3), [0, 0, 0])
                .unwrap()
        );
        let top = (0..8)
            .filter(|y| w.blocks.contains_key(&[0, *y, 0]))
            .max()
            .unwrap();
        assert!(top >= 2);
        let mut stone = world("minecraft:stone", 0);
        assert!(
            !f.place(&mut stone, &mut XoroshiroRandom::new(3), [0, 0, 0])
                .unwrap()
        );
    }

    #[test]
    fn spreading_bushes_grow_on_dirt_only() {
        let f = feature(json!({"type": "tfc:spreading_bush", "block": "tfc:plant/raspberry_bush"}));
        let mut w = world("minecraft:dirt", 0);
        assert!(
            f.place(&mut w, &mut XoroshiroRandom::new(5), [0, 0, 0])
                .unwrap()
        );
        assert!(w.blocks.len() > 100);
        let mut stone = world("minecraft:stone", 0);
        assert!(
            !f.place(&mut stone, &mut XoroshiroRandom::new(5), [0, 0, 0])
                .unwrap()
        );
    }

    #[test]
    fn kelp_grows_up_from_the_floor_with_a_head() {
        let f = feature(json!({
            "type": "tfc:kelp", "body": "minecraft:kelp_plant", "head": "minecraft:kelp",
            "tries": 20, "radius": 4, "min_height": 2, "max_height": 4
        }));
        let mut w = world("minecraft:stone", 0);
        let water = registry().default_state("minecraft:water").unwrap();
        for x in -6..=6 {
            for z in -6..=6 {
                for y in 0..8 {
                    w.blocks.insert([x, y, z], water);
                }
            }
        }
        assert!(
            f.place(&mut w, &mut XoroshiroRandom::new(9), [0, 0, 0])
                .unwrap()
        );
        let head = registry().default_state("minecraft:kelp").unwrap();
        assert!(w.blocks.values().any(|s| s.0 >= head.0 && s.0 < head.0 + 4));
    }

    #[test]
    fn starfish_attach_to_stone_and_face_away_from_it() {
        let f =
            feature(json!({"type": "tfc:rotatable_water_plant", "block": "tfc:plant/starfish"}));
        let mut w = world("minecraft:stone", 0);
        let water = registry().default_state("minecraft:water").unwrap();
        for y in 0..12 {
            w.blocks.insert([0, y, 0], water);
        }
        let mut placed = false;
        for seed in 0..20 {
            placed |= f
                .place(&mut w, &mut XoroshiroRandom::new(seed), [0, 0, 0])
                .unwrap();
        }
        assert!(placed);
    }

    /// The sides of the ivy at `p`, or `None` when it holds something else.
    fn ivy_sides(w: &World, p: [i32; 3]) -> Option<Vec<&'static str>> {
        let r = registry();
        let state = r.state(*w.blocks.get(&p)?).unwrap();
        if state.name != "tfc:plant/ivy" {
            return None;
        }
        Some(
            ["down", "up", "north", "south", "west", "east"]
                .into_iter()
                .filter(|side| state.properties.get(*side).map(String::as_str) == Some("true"))
                .collect(),
        )
    }

    #[test]
    fn ivy_clings_to_a_wall_above_the_tide_line_only() {
        crate::worldgen::tfc_tide_pool::tests::activate();
        let f = feature(json!({"type": "tfc:creeping_plant", "config": {
            "block": "tfc:plant/ivy", "radius": 4, "height": 7, "tide_height": 2, "integrity": 1.0
        }}));
        let stone = registry().default_state("minecraft:stone").unwrap();
        let wall = |w: &mut World, bottom: i32| {
            for y in bottom..bottom + 20 {
                for z in -6..=6 {
                    w.blocks.insert([1, y, z], stone);
                }
            }
        };
        // High above the shore: every cell beside the wall holds ivy with only the east face set.
        let mut w = world("minecraft:dirt", 0);
        wall(&mut w, 90);
        // The call reports no success, as in the Java.
        assert!(
            !f.place(&mut w, &mut XoroshiroRandom::new(3), [0, 90, 0])
                .unwrap()
        );
        assert_eq!(ivy_sides(&w, [0, 92, 0]), Some(vec!["east"]));
        assert_eq!(ivy_sides(&w, [0, 90, 3]), Some(vec!["east"]));
        // Away from any wall, and out of the circle, nothing grows.
        assert_eq!(ivy_sides(&w, [-2, 92, 0]), None);
        assert_eq!(ivy_sides(&w, [0, 92, 5]), None);
        // Below the tide line the same wall stays bare.
        let mut w = world("minecraft:dirt", 0);
        wall(&mut w, 40);
        f.place(&mut w, &mut XoroshiroRandom::new(3), [0, 40, 0])
            .unwrap();
        assert_eq!(ivy_sides(&w, [0, 42, 0]), None);
    }
}
