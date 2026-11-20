//! Fluid states carried by the pinned vanilla block-state registry.
//! Flow updates, spreading and entity interaction are separate simulation work.
use crate::{
    chunk::BlockState,
    compression::invalid,
    registry::{BlockRegistry, BlockStateId},
};
use std::io;
use std::sync::RwLock;

/// Blocks a loaded content mod declares as water-family fluids (`"fluid": "water"`): salt water,
/// say. Empty unless such a mod is on, so a game without one never sees a variant. A variant
/// flows, swims and renders as water; only its block, and so its tint and its texture, differ.
static WATER_VARIANTS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Registers `name` as a water-family block and returns its variant number (1 and up; 0 is
/// vanilla water). Registering a name twice returns the same number.
pub fn register_water_variant(name: &str) -> u8 {
    let mut names = WATER_VARIANTS.write().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = names.iter().position(|n| n == name) {
        return i as u8 + 1;
    }
    assert!(names.len() < 254, "too many water variants");
    names.push(name.to_string());
    names.len() as u8
}
/// The filled bucket of each water variant, by variant number.
static VARIANT_BUCKETS: RwLock<Vec<(u8, &'static str)>> = RwLock::new(Vec::new());

/// Declares `item` as the bucket that holds water variant `variant`.
pub fn register_variant_bucket(variant: u8, item: &str) {
    let mut buckets = VARIANT_BUCKETS.write().unwrap_or_else(|e| e.into_inner());
    buckets.retain(|(v, _)| *v != variant);
    // A handful of names for the whole run: leaking them gives the `'static` the bucket code returns.
    buckets.push((variant, Box::leak(item.to_string().into_boxed_str())));
}
/// The bucket item holding variant `variant`.
pub fn variant_bucket(variant: u8) -> Option<&'static str> {
    let buckets = VARIANT_BUCKETS.read().unwrap_or_else(|e| e.into_inner());
    buckets.iter().find(|(v, _)| *v == variant).map(|(_, i)| *i)
}
/// The water variant a bucket item holds.
pub fn bucket_variant(item: &str) -> Option<u8> {
    let buckets = VARIANT_BUCKETS.read().unwrap_or_else(|e| e.into_inner());
    buckets.iter().find(|(_, i)| *i == item).map(|(v, _)| *v)
}
/// Forgets every registered variant: a mod set changes only at start, and tests reset it.
pub fn clear_water_variants() {
    WATER_VARIANTS
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    VARIANT_BUCKETS
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}
/// The variant number of a registered water-family block name.
pub fn water_variant_of(name: &str) -> Option<u8> {
    let names = WATER_VARIANTS.read().unwrap_or_else(|e| e.into_inner());
    if names.is_empty() {
        return None;
    }
    names.iter().position(|n| n == name).map(|i| i as u8 + 1)
}
/// The block name of variant `variant` (0 is `minecraft:water`).
pub fn water_variant_name(variant: u8) -> Option<String> {
    if variant == 0 {
        return Some("minecraft:water".into());
    }
    WATER_VARIANTS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(variant as usize - 1)
        .cloned()
}
/// How many variants are registered.
pub fn water_variant_count() -> usize {
    WATER_VARIANTS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .len()
}
/// True for a liquid block that is water or a registered water-family variant.
pub fn is_water_block_name(name: &str) -> bool {
    name == "minecraft:water" || water_variant_of(name).is_some()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FluidKind {
    Empty,
    FlowingWater,
    Water,
    FlowingLava,
    Lava,
}
impl FluidKind {
    pub const ALL: [Self; 5] = [
        Self::Empty,
        Self::FlowingWater,
        Self::Water,
        Self::FlowingLava,
        Self::Lava,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Empty => "minecraft:empty",
            Self::FlowingWater => "minecraft:flowing_water",
            Self::Water => "minecraft:water",
            Self::FlowingLava => "minecraft:flowing_lava",
            Self::Lava => "minecraft:lava",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
    pub const fn same_family(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Empty, Self::Empty)
                | (
                    Self::Water | Self::FlowingWater,
                    Self::Water | Self::FlowingWater
                )
                | (
                    Self::Lava | Self::FlowingLava,
                    Self::Lava | Self::FlowingLava
                )
        )
    }
    pub const fn mask(self) -> u8 {
        1 << self as u8
    }
}
#[derive(Clone, Copy, Debug)]
pub struct FluidState {
    kind: FluidKind,
    amount: u8,
    falling: bool,
    /// 0 is vanilla; a mod's water-family fluid has its registered number.
    variant: u8,
    /// `tfc_flow` ordinal of a river water's current; `NO_FLOW` for every other fluid.
    flow: u8,
}
/// No river current.
pub const NO_FLOW: u8 = 255;
/// Two fluid states are equal when they hold the same amount of the same kind: a variant is the
/// same fluid under another block, so every water check in the game still holds for it.
impl PartialEq for FluidState {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.amount == other.amount && self.falling == other.falling
    }
}
impl Eq for FluidState {}
impl FluidState {
    /// Native fluid states require an object; properties decode leniently.
    pub fn from_json(json: &str) -> io::Result<Self> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|e| invalid(e.to_string()))?;
        let name = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid("missing fluid identifier"))?;
        let (namespace, path) = name.split_once(':').unwrap_or(("minecraft", name));
        let name = format!(
            "{}:{path}",
            if namespace.is_empty() {
                "minecraft"
            } else {
                namespace
            }
        );
        let kind =
            FluidKind::from_name(&name).ok_or_else(|| invalid("unknown fluid identifier"))?;
        if kind == FluidKind::Empty {
            return Ok(Self::EMPTY);
        }
        let flowing = matches!(kind, FluidKind::FlowingWater | FluidKind::FlowingLava);
        let mut state = Self {
            kind,
            amount: if flowing { 1 } else { 8 },
            falling: true,
            variant: 0,
            flow: NO_FLOW,
        };
        if let Some(properties) = value
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            if let Some(value) = properties
                .get("falling")
                .and_then(serde_json::Value::as_str)
            {
                match value {
                    "true" => state.falling = true,
                    "false" => state.falling = false,
                    _ => {}
                }
            }
            if flowing {
                if let Some(value) = properties
                    .get("level")
                    .and_then(serde_json::Value::as_str)
                    .and_then(crate::registry::java_integer)
                    .filter(|v| (1..=8).contains(v))
                {
                    state.amount = value as u8;
                }
            }
        }
        Ok(state)
    }
    pub fn to_json(self) -> io::Result<String> {
        let mut value = serde_json::json!({"id": self.kind.name()});
        if self.kind != FluidKind::Empty {
            let mut properties = serde_json::json!({"falling": self.falling.to_string()});
            if !self.is_source() {
                properties["level"] = self.amount.to_string().into();
            }
            value["properties"] = properties;
        }
        serde_json::to_string(&value).map_err(|e| invalid(e.to_string()))
    }
    /// A fluid state from its parts; the empty fluid ignores amount and falling.
    pub const fn new(kind: FluidKind, amount: u8, falling: bool) -> Self {
        if matches!(kind, FluidKind::Empty) {
            return Self::EMPTY;
        }
        Self {
            kind,
            amount,
            falling,
            variant: 0,
            flow: NO_FLOW,
        }
    }
    pub const EMPTY: Self = Self {
        kind: FluidKind::Empty,
        amount: 0,
        falling: false,
        variant: 0,
        flow: NO_FLOW,
    };
    pub const WATER: Self = Self {
        kind: FluidKind::Water,
        amount: 8,
        falling: false,
        variant: 0,
        flow: NO_FLOW,
    };
    /// The same fluid as a water-family variant (a no-op for lava and the empty fluid).
    pub const fn with_variant(self, variant: u8) -> Self {
        if matches!(
            self.kind,
            FluidKind::Empty | FluidKind::Lava | FluidKind::FlowingLava
        ) {
            return self;
        }
        Self { variant, ..self }
    }
    /// The same fluid with a river current (`tfc_flow` ordinal).
    pub const fn with_flow(self, flow: u8) -> Self {
        Self { flow, ..self }
    }
    /// The river current's `tfc_flow` ordinal, if this is a river water.
    pub const fn river_flow(self) -> Option<u8> {
        if self.flow == NO_FLOW {
            None
        } else {
            Some(self.flow)
        }
    }
    pub const fn variant(self) -> u8 {
        self.variant
    }
    pub const fn kind(self) -> FluidKind {
        self.kind
    }
    pub const fn amount(self) -> u8 {
        self.amount
    }
    pub const fn falling(self) -> bool {
        self.falling
    }
    pub const fn is_source(self) -> bool {
        matches!(self.kind, FluidKind::Water | FluidKind::Lava)
    }
    pub const fn is_full(self) -> bool {
        self.amount == 8
    }
    pub fn height_at<W: FluidHeightWorld>(
        self,
        world: &mut W,
        position: [i32; 3],
    ) -> Result<f32, W::Error> {
        if self.kind == FluidKind::Empty {
            return Ok(0.0);
        }
        let above = [position[0], position[1].wrapping_add(1), position[2]];
        Ok(if self.kind.same_family(world.fluid(above)?.kind) {
            1.0
        } else {
            self.own_height()
        })
    }
    pub fn camera_height_at<W: FluidHeightWorld>(
        self,
        world: &mut W,
        position: [i32; 3],
    ) -> Result<f32, W::Error> {
        if self.is_source() {
            let above = [position[0], position[1].wrapping_add(1), position[2]];
            if world.sturdy_down(above)? {
                return Ok(1.0);
            }
        }
        self.height_at(world, position)
    }
    pub fn own_height(self) -> f32 {
        f32::from(self.amount) / 9.0
    }
    pub fn from_block(state: &BlockState) -> io::Result<Self> {
        let variant = if state.name == "minecraft:water" {
            Some(0)
        } else {
            water_variant_of(&state.name)
        };
        let water = variant.is_some();
        if water || state.name == "minecraft:lava" {
            let level = state
                .properties
                .get("level")
                .and_then(|v| v.parse::<u8>().ok())
                .filter(|v| *v <= 15)
                .ok_or_else(|| invalid("invalid liquid block level"))?;
            let kind = match (water, level == 0) {
                (true, true) => FluidKind::Water,
                (true, false) => FluidKind::FlowingWater,
                (false, true) => FluidKind::Lava,
                (false, false) => FluidKind::FlowingLava,
            };
            return Ok(Self {
                kind,
                amount: if level == 0 || level >= 8 {
                    8
                } else {
                    8 - level
                },
                falling: level >= 8,
                variant: variant.unwrap_or(0),
                flow: state
                    .properties
                    .get("flow")
                    .and_then(|v| crate::worldgen::tfc_flow::NAMES.iter().position(|n| n == v))
                    .filter(|i| *i < 16 && variant.is_some_and(|v| v > 0))
                    .map_or(NO_FLOW, |i| i as u8),
            });
        }
        if matches!(
            state.name.as_str(),
            "minecraft:kelp"
                | "minecraft:kelp_plant"
                | "minecraft:seagrass"
                | "minecraft:tall_seagrass"
                | "minecraft:bubble_column"
        ) {
            return Ok(Self::WATER);
        }
        match state.properties.get("waterlogged").map(String::as_str) {
            Some("true") => {
                // The pinned copper-grate blocks use falling source water.
                let falling = matches!(
                    state.name.as_str(),
                    "minecraft:copper_grate"
                        | "minecraft:exposed_copper_grate"
                        | "minecraft:weathered_copper_grate"
                        | "minecraft:oxidized_copper_grate"
                        | "minecraft:waxed_copper_grate"
                        | "minecraft:waxed_exposed_copper_grate"
                        | "minecraft:waxed_weathered_copper_grate"
                        | "minecraft:waxed_oxidized_copper_grate"
                );
                Ok(Self {
                    falling,
                    ..Self::WATER
                })
            }
            None | Some("false") => Ok(Self::EMPTY),
            _ => Err(invalid("invalid waterlogged block property")),
        }
    }
}
/// Queries required by fluid surface height and camera immersion. The caller
/// owns block shape/support behavior and world-query failures.
pub trait FluidHeightWorld {
    type Error;
    fn fluid(&mut self, position: [i32; 3]) -> Result<FluidState, Self::Error>;
    fn sturdy_down(&mut self, position: [i32; 3]) -> Result<bool, Self::Error>;
}
#[derive(Debug)]
pub struct FluidStateTable(Vec<FluidState>);
impl FluidStateTable {
    pub fn new(registry: &BlockRegistry) -> io::Result<Self> {
        (0..registry.len())
            .map(|i| FluidState::from_block(registry.state(BlockStateId(i as u32)).unwrap()))
            .collect::<io::Result<Vec<_>>>()
            .map(Self)
    }
    pub fn get(&self, state: BlockStateId) -> Option<FluidState> {
        self.0.get(state.0 as usize).copied()
    }
}

/// Bound identities for converting fluid states into their legacy liquid blocks.
pub struct FluidBlockBindings {
    air: BlockStateId,
    water: [BlockStateId; 16],
    /// Each registered variant's blocks by level (variant n is index n - 1); a variant whose block
    /// is missing from the registry falls back to vanilla water.
    variants: Vec<[BlockStateId; 16]>,
    lava: [BlockStateId; 16],
}
impl FluidBlockBindings {
    pub fn new(registry: &BlockRegistry) -> io::Result<Self> {
        let levels = |name: &str| -> io::Result<[BlockStateId; 16]> {
            let default = registry
                .default_state(name)
                .ok_or_else(|| invalid("missing liquid block"))?;
            let mut state = registry.state(default).unwrap().clone();
            let mut result = [default; 16];
            for (level, id) in result.iter_mut().enumerate() {
                state.properties.insert("level".into(), level.to_string());
                *id = registry
                    .id(&state)
                    .ok_or_else(|| invalid("missing liquid block level"))?;
            }
            Ok(result)
        };
        let water = levels("minecraft:water")?;
        let variants = (1..=water_variant_count() as u8)
            .map(|v| {
                water_variant_name(v)
                    .and_then(|name| levels(&name).ok())
                    .unwrap_or(water)
            })
            .collect();
        Ok(Self {
            air: registry
                .default_state("minecraft:air")
                .ok_or_else(|| invalid("missing air block"))?,
            water,
            variants,
            lava: levels("minecraft:lava")?,
        })
    }
    pub fn block(&self, state: FluidState) -> BlockStateId {
        if state.kind == FluidKind::Empty {
            return self.air;
        }
        let level = if state.is_source() {
            0
        } else {
            8 - state.amount.min(8) + if state.falling { 8 } else { 0 }
        } as usize;
        if state.kind.same_family(FluidKind::Water) {
            if state.variant > 0
                && let Some(blocks) = self.variants.get(state.variant as usize - 1)
            {
                return blocks[level];
            }
            self.water[level]
        } else {
            self.lava[level]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DATA_VERSION, nbt::Tag};

    /// A registry of air, vanilla water and lava, and the mod liquid `name`, each with its 16 levels.
    fn registry_with(name: &str) -> BlockRegistry {
        let mut states = vec![Tag::String("minecraft:air".into())];
        let mut defaults = vec![("minecraft:air".to_string(), 0)];
        for block in ["minecraft:water", "minecraft:lava", name] {
            defaults.push((block.to_string(), states.len() as i32));
            for level in 0..16 {
                states.push(Tag::compound([
                    ("id", Tag::String(block.into())),
                    (
                        "properties",
                        Tag::compound([("level", Tag::String(level.to_string().as_str().into()))]),
                    ),
                ]));
            }
        }
        BlockRegistry::from_nbt(&Tag::compound([
            ("DataVersion", Tag::Int(DATA_VERSION)),
            ("states", Tag::List(states)),
            (
                "defaults",
                Tag::compound(defaults.iter().map(|(n, i)| (n.as_str(), Tag::Int(*i)))),
            ),
        ]))
        .unwrap()
    }

    #[test]
    fn a_registered_water_block_reads_as_water_and_writes_itself_back() {
        let name = "fluidtest:salt_water";
        assert_eq!(water_variant_of(name), None);
        let registry = registry_with(name);
        // Unregistered, the mod block is no liquid at all.
        let source = registry.default_state(name).unwrap();
        assert!(!is_water_block_name(name));
        assert_eq!(
            FluidState::from_block(registry.state(source).unwrap()).unwrap_or(FluidState::EMPTY),
            FluidState::EMPTY
        );

        let variant = register_water_variant(name);
        assert!(variant > 0);
        assert_eq!(
            register_water_variant(name),
            variant,
            "registering twice is idempotent"
        );
        assert!(is_water_block_name(name));
        let table = FluidStateTable::new(&registry).unwrap();
        let salt = table.get(source).unwrap();
        // The same fluid as vanilla water, under its variant.
        assert_eq!(salt, FluidState::WATER);
        assert_eq!(salt.variant(), variant);
        let flowing = table
            .get(BlockStateId(source.0 + 3))
            .expect("a flowing level");
        assert_eq!(flowing.kind(), FluidKind::FlowingWater);
        assert_eq!(flowing.variant(), variant);

        let bindings = FluidBlockBindings::new(&registry).unwrap();
        // A variant converts back to its own block, level for level; vanilla water is unchanged.
        assert_eq!(bindings.block(salt), source);
        assert_eq!(bindings.block(flowing), BlockStateId(source.0 + 3));
        assert_eq!(
            bindings.block(FluidState::WATER),
            registry.default_state("minecraft:water").unwrap()
        );
        // Lava has no variants.
        assert_eq!(
            FluidState::new(FluidKind::Lava, 8, false)
                .with_variant(variant)
                .variant(),
            0
        );
        // A bucket of the variant round-trips between item and variant.
        register_variant_bucket(variant, "fluidtest:salt_bucket");
        assert_eq!(variant_bucket(variant), Some("fluidtest:salt_bucket"));
        assert_eq!(bucket_variant("fluidtest:salt_bucket"), Some(variant));
        assert_eq!(bucket_variant("minecraft:water_bucket"), None);
        // No other test registers a variant: leave the registry as it was found.
        clear_water_variants();
    }
}
