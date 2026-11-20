//! TerraFirmaCraft's `tfc:coral_tree`, `tfc:coral_claw` and `tfc:coral_mushroom` (EUPL 1.2):
//! vanilla's coral shapes with TFC's `CoralHelpers.placeCoralBlock`, which only grows in (salt)
//! water and sprinkles fans, wall fans and sea pickles around each block.
//!
//! The coral blocks are vanilla's own: a random `#minecraft:coral_blocks` block makes the shape,
//! `#tfc:salt_water_corals` are the vanilla coral plants and fans, and the wall fans are vanilla's.

use super::{
    DecodeError,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    placement::PlacementContext,
    tree_blocks as tb,
    vegetation::counter_clockwise,
};
use crate::registry::BlockStateId;
use horizon_core::{geometry::Direction, random::RandomSource};
use std::sync::Arc;

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

const TYPES: [&str; 5] = ["tube", "brain", "bubble", "fire", "horn"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Tree,
    Claw,
    Mushroom,
}

#[derive(Clone, Debug)]
struct Corals {
    /// `#minecraft:coral_blocks`.
    blocks: Vec<BlockStateId>,
    /// `#tfc:salt_water_corals`: plants, then fans.
    plants: Vec<BlockStateId>,
    /// `#tfc:salt_water_wall_corals`, one list per horizontal direction (N, E, S, W).
    walls: Vec<[BlockStateId; 4]>,
    /// The sea pickle with 1 to 4 pickles.
    pickles: Option<[BlockStateId; 4]>,
    /// Names that count as `BlockTags.CORALS`, by state.
    corals: Arc<Vec<bool>>,
}

#[derive(Clone, Debug)]
pub struct TfcCoralFeature {
    shape: Shape,
    corals: Corals,
    /// State id -> is the salt water block.
    names: Arc<Vec<String>>,
    /// Whether the registry has a mod's salt water; without one, vanilla water is the sea.
    has_salt: bool,
}

fn facing_name(d: Direction) -> &'static str {
    match d {
        Direction::North => "north",
        Direction::East => "east",
        Direction::South => "south",
        _ => "west",
    }
}

impl TfcCoralFeature {
    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let kind: super::Object = serde_json::from_str(json)?;
        let shape =
            match serde_json::from_str::<String>(super::required(&kind, "type")?.get())?.as_str() {
                "tfc:coral_tree" => Shape::Tree,
                "tfc:coral_claw" => Shape::Claw,
                "tfc:coral_mushroom" => Shape::Mushroom,
                _ => return Err(DecodeError::Field("coral type")),
            };
        let registry = bindings.registry();
        let names: Vec<String> = (0..registry.len())
            .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
            .collect();
        let with = |name: &str, props: &[(&str, &str)]| -> Option<BlockStateId> {
            let mut state = registry.state(registry.default_state(name)?)?.clone();
            for (key, value) in props {
                if state.properties.contains_key(*key) {
                    state.properties.insert((*key).into(), (*value).into());
                }
            }
            registry.id(&state)
        };
        let get = |name: String| with(&name, &[("waterlogged", "true")]);
        let mut blocks = Vec::new();
        let mut plants = Vec::new();
        let mut fans = Vec::new();
        let mut walls = Vec::new();
        for t in TYPES {
            if let Some(s) = get(format!("minecraft:{t}_coral_block")) {
                blocks.push(s);
            }
            if let Some(s) = get(format!("minecraft:{t}_coral")) {
                plants.push(s);
            }
            if let Some(s) = get(format!("minecraft:{t}_coral_fan")) {
                fans.push(s);
            }
            let name = format!("minecraft:{t}_coral_wall_fan");
            let mut row = [BlockStateId(0); 4];
            let mut ok = true;
            for (i, d) in tb::HORIZONTAL.iter().enumerate() {
                match with(
                    &name,
                    &[("waterlogged", "true"), ("facing", facing_name(*d))],
                ) {
                    Some(s) => row[i] = s,
                    None => ok = false,
                }
            }
            if ok {
                walls.push(row);
            }
        }
        plants.extend(fans);
        let pickles = {
            let mut table = [BlockStateId(0); 4];
            let mut ok = true;
            for (i, n) in ["1", "2", "3", "4"].iter().enumerate() {
                match with(
                    "minecraft:sea_pickle",
                    &[("waterlogged", "true"), ("pickles", n)],
                ) {
                    Some(s) => table[i] = s,
                    None => ok = false,
                }
            }
            ok.then_some(table)
        };
        let corals = names
            .iter()
            .map(|n| {
                n.strip_prefix("minecraft:").is_some_and(|n| {
                    TYPES
                        .iter()
                        .any(|t| n == format!("{t}_coral") || n == format!("{t}_coral_fan"))
                })
            })
            .collect();
        Ok(Self {
            shape,
            corals: Corals {
                blocks,
                plants,
                walls,
                pickles,
                corals: Arc::new(corals),
            },
            has_salt: names.iter().any(|n| n == "tfc:salt_water"),
            names: Arc::new(names),
        })
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        // `CoralFeature.place`: a random coral block makes the whole shape.
        if self.corals.blocks.is_empty() {
            return Ok(false);
        }
        let state =
            self.corals.blocks[random.next_int_bound(self.corals.blocks.len() as i32)? as usize];
        let mut shaper = Shaper {
            feature: self,
            world,
            state,
        };
        match self.shape {
            Shape::Tree => shaper.tree(random, origin),
            Shape::Claw => shaper.claw(random, origin),
            Shape::Mushroom => shaper.mushroom(random, origin),
        }
    }
}

struct Shaper<'a, W> {
    feature: &'a TfcCoralFeature,
    world: &'a mut W,
    state: BlockStateId,
}

impl<W: FeatureWorld + PlacementContext> Shaper<'_, W> {
    fn is_salt_water(&mut self, p: [i32; 3]) -> R<bool, W> {
        let s = self.world.block(p).map_err(we)?;
        let name = &self.feature.names[s.0 as usize];
        // Corals need salt water: with a mod's salt water loaded, fresh water does not count.
        Ok(if self.feature.has_salt {
            name == "tfc:salt_water"
        } else {
            name == "minecraft:water"
        })
    }

    /// `CoralHelpers.placeCoralBlock`.
    fn place_block(&mut self, random: &mut impl RandomSource, pos: [i32; 3]) -> R<bool, W> {
        let above = tb::offset(pos, 0, 1, 0);
        let here = self.world.block(pos).map_err(we)?;
        let corals = &self.feature.corals;
        if !((matches!(
            self.feature.names[here.0 as usize].as_str(),
            "minecraft:water" | "tfc:salt_water"
        ) || corals.corals[here.0 as usize])
            && self.is_salt_water(above)?)
        {
            return Ok(false);
        }
        self.world.set_block(pos, self.state, 3).map_err(we)?;
        if random.next_float() < 0.25 {
            let pick = corals.plants[random.next_int_bound(corals.plants.len() as i32)? as usize];
            self.world.set_block(above, pick, 2).map_err(we)?;
        } else if random.next_float() < 0.05
            && let Some(pickles) = corals.pickles
        {
            let n = random.next_int_bound(4)? as usize;
            self.world.set_block(above, pickles[n], 2).map_err(we)?;
        }
        for (i, direction) in tb::HORIZONTAL.iter().enumerate() {
            if random.next_float() < 0.2 {
                let relative = tb::relative(pos, *direction, 1);
                if self.is_salt_water(relative)? {
                    let pick = random.next_int_bound(corals.walls.len() as i32)? as usize;
                    self.world
                        .set_block(relative, corals.walls[pick][i], 2)
                        .map_err(we)?;
                }
            }
        }
        Ok(true)
    }

    fn tree(&mut self, random: &mut impl RandomSource, origin: [i32; 3]) -> R<bool, W> {
        let mut pos = origin;
        let trunk = random.next_int_bound(3)? + 1;
        for _ in 0..trunk {
            if !self.place_block(random, pos)? {
                return Ok(true);
            }
            pos = tb::offset(pos, 0, 1, 0);
        }
        let top = pos;
        let branches = random.next_int_bound(3)? + 2;
        let mut directions = tb::HORIZONTAL;
        tb::shuffle(&mut directions, random)?;
        for &direction in directions.iter().take(branches as usize) {
            let mut pos = tb::relative(top, direction, 1);
            let height = random.next_int_bound(5)? + 2;
            let mut segment = 0;
            let mut j = 0;
            while j < height && self.place_block(random, pos)? {
                segment += 1;
                pos = tb::offset(pos, 0, 1, 0);
                if j == 0 || (segment >= 2 && random.next_float() < 0.25) {
                    pos = tb::relative(pos, direction, 1);
                    segment = 0;
                }
                j += 1;
            }
        }
        Ok(true)
    }

    fn claw(&mut self, random: &mut impl RandomSource, origin: [i32; 3]) -> R<bool, W> {
        if !self.place_block(random, origin)? {
            return Ok(false);
        }
        let claw = tb::HORIZONTAL[random.next_int_bound(4)? as usize];
        let branches = random.next_int_bound(2)? + 2;
        let mut possible = [claw, tb::clockwise(claw), counter_clockwise(claw)];
        tb::shuffle(&mut possible, random)?;
        for &branch in possible.iter().take(branches as usize) {
            let mut pos = origin;
            let sideways = random.next_int_bound(2)? + 1;
            pos = tb::relative(pos, branch, 1);
            let (segment, inway) = if branch == claw {
                (claw, random.next_int_bound(3)? + 2)
            } else {
                pos = tb::offset(pos, 0, 1, 0);
                let options = [branch, Direction::Up];
                let segment = options[random.next_int_bound(2)? as usize];
                (segment, random.next_int_bound(3)? + 3)
            };
            let mut i = 0;
            while i < sideways && self.place_block(random, pos)? {
                pos = tb::relative(pos, segment, 1);
                i += 1;
            }
            pos = tb::relative(pos, segment.opposite(), 1);
            pos = tb::offset(pos, 0, 1, 0);
            for _ in 0..inway {
                pos = tb::relative(pos, claw, 1);
                if !self.place_block(random, pos)? {
                    break;
                }
                if random.next_float() < 0.25 {
                    pos = tb::offset(pos, 0, 1, 0);
                }
            }
        }
        Ok(true)
    }

    fn mushroom(&mut self, random: &mut impl RandomSource, origin: [i32; 3]) -> R<bool, W> {
        let i = random.next_int_bound(3)? + 3;
        let j = random.next_int_bound(3)? + 3;
        let k = random.next_int_bound(3)? + 3;
        let l = random.next_int_bound(3)? + 1;
        for m in 0..=j {
            for n in 0..=i {
                for o in 0..=k {
                    let pos = [origin[0] + m, origin[1] + n - l, origin[2] + o];
                    if ((m != 0 && m != j) || (n != 0 && n != i))
                        && ((o != 0 && o != k) || (n != 0 && n != i))
                        && ((m != 0 && m != j) || (o != 0 && o != k))
                        && (m == 0 || m == j || n == 0 || n == i || o == 0 || o == k)
                        && random.next_float() >= 0.1
                    {
                        self.place_block(random, pos)?;
                    }
                }
            }
        }
        Ok(true)
    }
}

fn we<E>(e: E) -> FeatureExecutionError<E> {
    FeatureExecutionError::Basic(FeatureError::World(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{registry::BlockRegistry, worldgen::tfc_tide_pool::tests::Flat};
    use horizon_core::random::XoroshiroRandom;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    fn registry() -> &'static BlockRegistry {
        let mut rows = Vec::new();
        let mut defaults = BTreeMap::new();
        let mut add = |name: String, air: bool, fluid: bool, props: Vec<(&str, Vec<&str>)>| {
            defaults.insert(name.clone(), rows.len());
            let mut combos: Vec<BTreeMap<String, String>> = vec![BTreeMap::new()];
            for (key, values) in props {
                combos = combos
                    .into_iter()
                    .flat_map(|c| {
                        values.iter().map(move |v| {
                            let mut c = c.clone();
                            c.insert(key.into(), (*v).into());
                            c
                        })
                    })
                    .collect();
            }
            for c in combos {
                rows.push(json!([name, air, fluid, "", {"id": name, "properties": c}]));
            }
        };
        for n in ["air", "cave_air", "void_air"] {
            add(format!("minecraft:{n}"), true, false, vec![]);
        }
        add("minecraft:stone".into(), false, false, vec![]);
        let levels: Vec<String> = (0..16).map(|l| l.to_string()).collect();
        let levels: Vec<&str> = levels.iter().map(String::as_str).collect();
        add(
            "minecraft:water".into(),
            false,
            true,
            vec![("level", levels)],
        );
        let wl = vec!["false", "true"];
        for t in TYPES {
            add(format!("minecraft:{t}_coral_block"), false, false, vec![]);
            add(
                format!("minecraft:{t}_coral"),
                false,
                false,
                vec![("waterlogged", wl.clone())],
            );
            add(
                format!("minecraft:{t}_coral_fan"),
                false,
                false,
                vec![("waterlogged", wl.clone())],
            );
            add(
                format!("minecraft:{t}_coral_wall_fan"),
                false,
                false,
                vec![
                    ("facing", vec!["north", "south", "west", "east"]),
                    ("waterlogged", wl.clone()),
                ],
            );
        }
        add(
            "minecraft:sea_pickle".into(),
            false,
            false,
            vec![("pickles", vec!["1", "2", "3", "4"]), ("waterlogged", wl)],
        );
        let facts = json!({"states": rows, "defaults": defaults});
        Box::leak(Box::new(
            BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
        ))
    }

    fn sea(r: &BlockRegistry, water: bool) -> Flat {
        let w = r.default_state("minecraft:water").unwrap();
        let mut blocks = HashMap::new();
        if water {
            for x in -12..=12 {
                for z in -12..=12 {
                    for y in 0..=30 {
                        blocks.insert([x, y, z], w);
                    }
                }
            }
        }
        Flat {
            blocks,
            surface: 0,
            rock: r.default_state("minecraft:stone").unwrap(),
            air: r.default_state("minecraft:air").unwrap(),
        }
    }

    fn feature(kind: &str, r: &'static BlockRegistry) -> TfcCoralFeature {
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        TfcCoralFeature::decode(&json!({"type": kind}).to_string(), &bindings).unwrap()
    }

    #[test]
    fn each_shape_grows_one_coral_block_type_only_inside_water() {
        let r = registry();
        for kind in ["tfc:coral_tree", "tfc:coral_claw", "tfc:coral_mushroom"] {
            let f = feature(kind, r);
            let water = r.default_state("minecraft:water").unwrap();
            let mut grew = 0;
            for seed in 0..40 {
                let mut world = sea(r, true);
                f.place(&mut world, &mut XoroshiroRandom::new(seed), [0, 3, 0])
                    .unwrap();
                let solid: BTreeSet<_> = world
                    .blocks
                    .values()
                    .filter(|s| f.names[s.0 as usize].ends_with("_coral_block"))
                    .collect();
                assert!(solid.len() <= 1, "{kind}: one block type per shape");
                grew += solid.len();
                for (p, s) in &world.blocks {
                    // Nothing leaves the sea: every edited cell was water, so nothing sits on rock.
                    assert!(p[1] >= 0 && (*s != water || p[1] <= 30));
                }
            }
            assert!(grew > 30, "{kind} grew in {grew}/40 tries");
            // Dry land: nothing at all is placed.
            let mut dry = sea(r, false);
            f.place(&mut dry, &mut XoroshiroRandom::new(1), [0, 3, 0])
                .unwrap();
            assert!(dry.blocks.is_empty(), "{kind} must not grow in air");
        }
    }

    #[test]
    fn decorations_are_waterlogged_vanilla_corals() {
        let r = registry();
        let f = feature("tfc:coral_mushroom", r);
        let (mut fans, mut walls, mut pickles) = (0, 0, 0);
        for seed in 0..60 {
            let mut world = sea(r, true);
            f.place(&mut world, &mut XoroshiroRandom::new(seed), [0, 3, 0])
                .unwrap();
            for s in world.blocks.values() {
                let st = r.state(*s).unwrap();
                let name = &st.name;
                if name.ends_with("_coral_wall_fan") {
                    walls += 1;
                    assert_eq!(
                        st.properties.get("waterlogged").map(String::as_str),
                        Some("true")
                    );
                } else if name.ends_with("_coral") || name.ends_with("_coral_fan") {
                    fans += 1;
                    assert_eq!(
                        st.properties.get("waterlogged").map(String::as_str),
                        Some("true")
                    );
                } else if name == "minecraft:sea_pickle" {
                    pickles += 1;
                }
            }
        }
        assert!(
            fans > 50 && walls > 50 && pickles > 0,
            "{fans} {walls} {pickles}"
        );
    }
}
