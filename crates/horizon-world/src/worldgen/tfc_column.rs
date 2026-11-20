//! TerraFirmaCraft's column plants (EUPL 1.2): `tfc:tall_plant` (`TallPlantFeature`, two stacked
//! halves), `tfc:weeping_vines` (`TFCWeepingVinesFeature`, vines hanging from leaves, logs and
//! stone) and `tfc:twisting_vines` (`TFCTwistingVinesFeature`, columns growing up from the
//! ground). The tag tests TFC makes (leaves, logs, base stone, grass-plantable) are matched by
//! block name here.

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    density::identifier,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    integer, optional, required,
};
use crate::registry::{BlockRegistry, BlockStateId};
use horizon_core::random::RandomSource;

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

#[derive(Clone, Debug)]
enum Kind {
    Tall {
        lower: BlockStateId,
        upper: BlockStateId,
    },
    Weeping(Column),
    Twisting(Column),
}

#[derive(Clone, Debug)]
struct Column {
    body: BlockStateId,
    /// The head block with `age` 0, and the id of each age 17..=25 (or just the one state if the
    /// head has no `age`).
    heads: Vec<BlockStateId>,
    tries: i32,
    radius: i32,
    min_height: i32,
    max_height: i32,
}

#[derive(Clone, Debug)]
pub struct TfcColumnFeature {
    kind: Kind,
    air: BlockStateId,
    names: std::sync::Arc<Vec<String>>,
}

fn with(
    registry: &BlockRegistry,
    block: &str,
    property: &str,
    value: &str,
) -> Result<BlockStateId, DecodeError> {
    let default = registry
        .default_state(block)
        .ok_or(DecodeError::Field("column plant block"))?;
    let mut state = registry.state(default).unwrap().clone();
    if state.properties.contains_key(property) {
        state.properties.insert(property.into(), value.into());
    }
    registry
        .id(&state)
        .ok_or(DecodeError::Field("column plant state"))
}

fn text(object: &Object, key: &'static str) -> Result<String, DecodeError> {
    Ok(serde_json::from_str(required(object, key)?.get())?)
}

fn int(object: &Object, key: &'static str, default: i32) -> Result<i32, DecodeError> {
    Ok(optional(object, key)
        .map(integer)
        .transpose()?
        .unwrap_or(default))
}

impl TfcColumnFeature {
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
        let air = registry
            .default_state("minecraft:air")
            .ok_or(DecodeError::Field("air"))?;
        let kind = match kind.as_str() {
            "tfc:tall_plant" => {
                let block = text(&config, "block")?;
                Kind::Tall {
                    lower: with(registry, &block, "half", "lower")?,
                    upper: with(registry, &block, "half", "upper")?,
                }
            }
            "tfc:weeping_vines" | "tfc:twisting_vines" => {
                let body = text(&config, "body")?;
                let head = text(&config, "head")?;
                let (tries, radius) = (int(&config, "tries", 10)?, int(&config, "radius", 4)?);
                let (min_height, max_height) = (
                    int(&config, "min_height", 1)?,
                    int(&config, "max_height", 2)?,
                );
                if tries < 1 || radius < 1 || min_height < 1 || max_height <= min_height {
                    return Err(DecodeError::Field("column plant size"));
                }
                let heads = if registry.property_values(&head, "age").is_empty() {
                    vec![
                        registry
                            .default_state(&head)
                            .ok_or(DecodeError::Field("column plant head"))?,
                    ]
                } else {
                    (17..=25)
                        .map(|age| with(registry, &head, "age", &age.to_string()))
                        .collect::<Result<_, _>>()?
                };
                let column = Column {
                    body: registry
                        .default_state(&body)
                        .ok_or(DecodeError::Field("column plant body"))?,
                    heads,
                    tries,
                    radius,
                    min_height,
                    max_height,
                };
                if kind == "tfc:weeping_vines" {
                    Kind::Weeping(column)
                } else {
                    Kind::Twisting(column)
                }
            }
            _ => return Err(DecodeError::Field("column plant type")),
        };
        Ok(Self {
            kind,
            air,
            names: std::sync::Arc::new(
                (0..registry.len())
                    .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
                    .collect(),
            ),
        })
    }

    fn name(&self, state: BlockStateId) -> &str {
        &self.names[state.0 as usize]
    }

    pub fn place<W: FeatureWorld>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let we = |e| FeatureExecutionError::Basic(FeatureError::World(e));
        let rnd = |r: Result<i32, _>| r.map_err(FeatureExecutionError::Random);
        match &self.kind {
            Kind::Tall { lower, upper } => {
                world.set_block(origin, *lower, 2).map_err(we)?;
                world
                    .set_block([origin[0], origin[1] + 1, origin[2]], *upper, 2)
                    .map_err(we)?;
                Ok(true)
            }
            Kind::Weeping(c) => {
                let mut placed = false;
                for _ in 0..c.tries {
                    let dx = rnd(random.next_int_bound(c.radius))?
                        - rnd(random.next_int_bound(c.radius))?;
                    let dy = rnd(random.next_int_bound(4))? - rnd(random.next_int_bound(4))?;
                    let dz = rnd(random.next_int_bound(c.radius))?
                        - rnd(random.next_int_bound(c.radius))?;
                    let mut p = [origin[0] + dx, origin[1] + dy + 10, origin[2] + dz];
                    let above = world.block([p[0], p[1] + 1, p[2]]).map_err(we)?;
                    let anchor = {
                        let n = self.name(above);
                        n.ends_with("_leaves")
                            || n.ends_with("_log")
                            || n.ends_with("_wood")
                            || n.starts_with("tfc:raw_")
                            || matches!(
                                n,
                                "minecraft:stone"
                                    | "minecraft:andesite"
                                    | "minecraft:diorite"
                                    | "minecraft:granite"
                                    | "minecraft:deepslate"
                                    | "minecraft:tuff"
                            )
                    };
                    if !anchor || world.block(p).map_err(we)? != self.air {
                        continue;
                    }
                    let height =
                        rnd(random.next_int_bound(c.max_height - c.min_height))? + c.min_height;
                    for i in 0..=height {
                        if world.block(p).map_err(we)? == self.air {
                            let below = world.block([p[0], p[1] - 1, p[2]]).map_err(we)?;
                            if i == height || below != self.air {
                                let head = self.head(c, random)?;
                                world.set_block(p, head, 2).map_err(we)?;
                                break;
                            }
                            world.set_block(p, c.body, 2).map_err(we)?;
                        }
                        p[1] -= 1;
                    }
                    placed = true;
                }
                Ok(placed)
            }
            Kind::Twisting(c) => {
                let mut placed = false;
                for _ in 0..c.tries {
                    let dx = rnd(random.next_int_bound(c.radius))?
                        - rnd(random.next_int_bound(c.radius))?;
                    let dz = rnd(random.next_int_bound(c.radius))?
                        - rnd(random.next_int_bound(c.radius))?;
                    let mut p = [origin[0] + dx, origin[1], origin[2] + dz];
                    let ground = world.block([p[0], p[1] - 1, p[2]]).map_err(we)?;
                    let n = self.name(ground);
                    let plantable = ground != self.air
                        && [
                            "dirt", "grass", "sand", "mud", "podzol", "peat", "clay", "gravel",
                        ]
                        .iter()
                        .any(|k| n.contains(k));
                    if !plantable || world.block(p).map_err(we)? != self.air {
                        continue;
                    }
                    let height =
                        rnd(random.next_int_bound(c.max_height - c.min_height))? + c.min_height;
                    for i in 1..=height {
                        if world.block(p).map_err(we)? == self.air {
                            let next = world.block([p[0], p[1] + 1, p[2]]).map_err(we)?;
                            if i == height || next != self.air {
                                let head = self.head(c, random)?;
                                world.set_block(p, head, 2).map_err(we)?;
                                break;
                            }
                            world.set_block(p, c.body, 2).map_err(we)?;
                        }
                        p[1] += 1;
                    }
                    placed = true;
                }
                Ok(placed)
            }
        }
    }

    /// `Mth.nextInt(rand, 17, 25)` for the head's age.
    fn head<E>(
        &self,
        c: &Column,
        random: &mut impl RandomSource,
    ) -> Result<BlockStateId, FeatureExecutionError<E>> {
        if c.heads.len() == 1 {
            return Ok(c.heads[0]);
        }
        let i = random
            .next_int_bound(9)
            .map_err(FeatureExecutionError::Random)?;
        Ok(c.heads[i as usize])
    }
}
