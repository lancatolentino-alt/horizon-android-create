//! TerraFirmaCraft's `tfc:cave_vegetation` (`CaveVegetationFeature`) and `tfc:rivulet`
//! (`RivuletFeature`), EUPL 1.2. Block tags and solidity are matched by block name.

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    optional,
    placement::PlacementContext,
    required,
    tfc_tree::{
        Weighted, air_or_empty_fluid, is_air, is_fluid_block, replaceable, single_block_replaceable,
    },
};
use crate::{heightmap::HeightmapType, registry::BlockStateId};
use horizon_core::random::RandomSource;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

const HORIZONTAL: [[i32; 2]; 4] = [[0, -1], [1, 0], [0, 1], [-1, 0]];

#[derive(Clone, Debug)]
enum Kind {
    Vegetation {
        states: HashMap<String, Weighted>,
        water: Option<BlockStateId>,
        cobweb: Option<BlockStateId>,
        roots: Option<BlockStateId>,
        guano: Option<BlockStateId>,
    },
    Rivulet {
        states: HashMap<String, BlockStateId>,
        air: BlockStateId,
    },
    /// `tfc:thin_spike`: `states[tip * 2 + waterlogged]`.
    ThinSpike {
        states: [BlockStateId; 4],
        name: String,
        radius: i32,
        tries: i32,
        min_height: i32,
        max_height: i32,
        underwater: bool,
    },
}

#[derive(Clone, Debug)]
pub struct TfcCaveFeature {
    kind: Kind,
    names: Arc<Vec<String>>,
}

/// `BASE_STONE_OVERWORLD` plus TFC's raw rock, by name.
fn base_stone(n: &str) -> bool {
    n.starts_with("tfc:raw_")
        || matches!(
            n,
            "minecraft:stone"
                | "minecraft:granite"
                | "minecraft:diorite"
                | "minecraft:andesite"
                | "minecraft:tuff"
                | "minecraft:deepslate"
        )
}

impl TfcCaveFeature {
    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(json)?;
        let config: Object = match optional(&object, "config") {
            Some(config) => serde_json::from_str(config.get())?,
            None => object.clone(),
        };
        let kind: String = serde_json::from_str(required(&object, "type")?.get())?;
        let registry = bindings.registry();
        let state = |n: &str| registry.default_state(n);
        let kind = if kind == "tfc:thin_spike" {
            let name: String = {
                let v: serde_json::Value = serde_json::from_str(required(&config, "state")?.get())?;
                match v {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Object(o) => o
                        .get("Name")
                        .and_then(|n| n.as_str())
                        .map(str::to_owned)
                        .ok_or(DecodeError::Field("spike state"))?,
                    _ => return Err(DecodeError::Field("spike state")),
                }
            };
            let int = |key: &'static str| -> Result<i32, DecodeError> {
                Ok(serde_json::from_str(required(&config, key)?.get())?)
            };
            let (radius, tries) = (int("radius")?, int("tries")?);
            let (min_height, max_height) = (int("min_height")?, int("max_height")?);
            if !(1..=16).contains(&radius) || tries < 1 || min_height < 1 || max_height < min_height
            {
                return Err(DecodeError::Field("spike size"));
            }
            let underwater: bool = match optional(&config, "allow_underwater") {
                Some(v) => serde_json::from_str(v.get())?,
                None => false,
            };
            let base = registry
                .default_state(&name)
                .ok_or(DecodeError::Field("spike block"))?;
            let make = |tip: bool, wet: bool| -> Result<BlockStateId, DecodeError> {
                let mut st = registry.state(base).unwrap().clone();
                let tip_value = if tip { "true" } else { "false" };
                if st.properties.contains_key("tip") {
                    st.properties.insert("tip".into(), tip_value.into());
                } else if st.properties.contains_key("thickness") {
                    st.properties.insert(
                        "thickness".into(),
                        // `PILLAR_SHAPE` is 12 pixels wide: vanilla's base.
                        if tip { "tip" } else { "base" }.into(),
                    );
                }
                if st.properties.contains_key("waterlogged") {
                    st.properties.insert(
                        "waterlogged".into(),
                        if wet { "true" } else { "false" }.into(),
                    );
                }
                registry.id(&st).ok_or(DecodeError::Field("spike state"))
            };
            Kind::ThinSpike {
                states: [
                    make(false, false)?,
                    make(false, true)?,
                    make(true, false)?,
                    make(true, true)?,
                ],
                name,
                radius,
                tries,
                min_height,
                max_height,
                underwater,
            }
        } else if kind == "tfc:cave_vegetation" {
            let entries: Vec<Object> = serde_json::from_str(required(&config, "blocks")?.get())?;
            let mut states = HashMap::new();
            for entry in entries {
                let replace: Vec<String> =
                    serde_json::from_str(required(&entry, "replace")?.get())?;
                let with: Vec<Object> = serde_json::from_str(required(&entry, "with")?.get())?;
                let weighted = Weighted::decode(&with, bindings)?;
                for name in replace {
                    states.insert(name, weighted.clone());
                }
            }
            Kind::Vegetation {
                states,
                water: state("minecraft:water"),
                cobweb: state("minecraft:cobweb"),
                roots: state("minecraft:hanging_roots"),
                guano: state("tfc:groundcover/guano"),
            }
        } else {
            let entries: Vec<Object> = serde_json::from_str(required(&config, "states")?.get())?;
            let mut states = HashMap::new();
            for entry in entries {
                let replace: String = serde_json::from_str(required(&entry, "replace")?.get())?;
                let with: String = serde_json::from_str(required(&entry, "with")?.get())?;
                states.insert(
                    replace,
                    state(&with).ok_or(DecodeError::Field("rivulet block"))?,
                );
            }
            Kind::Rivulet {
                states,
                air: bindings.air[0],
            }
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
            Kind::Vegetation {
                states,
                water,
                cobweb,
                roots,
                guano,
            } => p.vegetation(states, [*water, *cobweb, *roots, *guano], random, origin),
            Kind::Rivulet { states, air } => p.rivulet(states, *air, random, origin),
            Kind::ThinSpike {
                states,
                name,
                radius,
                tries,
                min_height,
                max_height,
                underwater,
            } => p.thin_spike(
                states,
                name,
                (*radius, *tries, *min_height, *max_height, *underwater),
                random,
                origin,
            ),
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
fn rnd<E>(
    r: Result<i32, horizon_core::random::InvalidBound>,
) -> Result<i32, FeatureExecutionError<E>> {
    r.map_err(FeatureExecutionError::Random)
}

impl<W: FeatureWorld + PlacementContext> Placer<'_, W> {
    fn at(&mut self, p: [i32; 3]) -> R<String, W> {
        let s = self.world.block(p).map_err(we)?;
        Ok(self.names[s.0 as usize].clone())
    }
    fn put(&mut self, p: [i32; 3], s: BlockStateId) -> R<(), W> {
        self.world.set_block(p, s, 2).map_err(we)?;
        Ok(())
    }
    fn height(&mut self, x: i32, z: i32) -> R<i32, W> {
        self.world
            .height(HeightmapType::OceanFloorWg, x, z)
            .map_err(FeatureExecutionError::Query)
    }

    /// `CaveVegetationFeature`.
    fn vegetation(
        &mut self,
        states: &HashMap<String, Weighted>,
        [water, cobweb, roots, guano]: [Option<BlockStateId>; 4],
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        for _ in 0..128 {
            if random.next_float() < 0.8 {
                let dx = rnd(random.next_int_bound(15))? - rnd(random.next_int_bound(15))?;
                let dy = -rnd(random.next_int_bound(2))? - 1;
                let dz = rnd(random.next_int_bound(15))? - rnd(random.next_int_bound(15))?;
                let mut p = [origin[0] + dx, origin[1] + dy, origin[2] + dz];
                if is_air(&self.at(p)?) {
                    for _ in 0..7 {
                        p[1] -= 1;
                        if !is_air(&self.at(p)?) {
                            break;
                        }
                    }
                    let here = self.at(p)?;
                    if let Some(weighted) = states.get(&here)
                        && let Some(state) = weighted.get(random)
                    {
                        self.put(p, state)?;
                    }
                    if random.next_float() < 0.06 && p[1] < 40 {
                        p[1] += 1;
                        let below = self.at([p[0], p[1] - 1, p[2]])?;
                        if let Some(guano) = guano
                            && !air_or_empty_fluid(&below)
                            && !single_block_replaceable(&below)
                        {
                            self.put(p, guano)?;
                        }
                    }
                }
            }
            if random.next_float() < 0.003 {
                let dx = rnd(random.next_int_bound(15))? - rnd(random.next_int_bound(15))?;
                let dy = 4 + rnd(random.next_int_bound(7))?;
                let dz = rnd(random.next_int_bound(15))? - rnd(random.next_int_bound(15))?;
                let mut p = [origin[0] + dx, origin[1] + dy, origin[2] + dz];
                if is_air(&self.at(p)?) {
                    p[1] += 1;
                    if base_stone(&self.at(p)?)
                        && let Some(water) = water
                    {
                        self.put(p, water)?;
                    }
                }
            }
            if random.next_float() < 0.04 {
                let dx = rnd(random.next_int_bound(15))? - rnd(random.next_int_bound(15))?;
                let dy = 4 + rnd(random.next_int_bound(7))?;
                let dz = rnd(random.next_int_bound(15))? - rnd(random.next_int_bound(15))?;
                let mut p = [origin[0] + dx, origin[1] + dy, origin[2] + dz];
                if base_stone(&self.at(p)?) {
                    p[1] -= 1;
                    if is_air(&self.at(p)?) {
                        let state = if random.next_boolean() { cobweb } else { roots };
                        if let Some(state) = state {
                            self.put(p, state)?;
                        }
                    }
                }
            }
        }
        Ok(true)
    }

    fn spike_empty(&self, n: &str, underwater: bool) -> bool {
        if underwater {
            air_or_empty_fluid(n)
        } else {
            is_air(n)
        }
    }

    /// `ThinSpikeFeature`.
    fn thin_spike(
        &mut self,
        states: &[BlockStateId; 4],
        name: &str,
        (radius, tries, min_height, max_height, underwater): (i32, i32, i32, i32, bool),
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let mut placed_any = false;
        for _ in 0..tries {
            let dx = rnd(random.next_int_bound(radius))? - rnd(random.next_int_bound(radius))?;
            let dy = rnd(random.next_int_bound(radius))? - rnd(random.next_int_bound(radius))?;
            let dz = rnd(random.next_int_bound(radius))? - rnd(random.next_int_bound(radius))?;
            let mut p = [origin[0] + dx, origin[1] + dy, origin[2] + dz];
            for _ in 0..7 {
                p[1] += 1;
                let n = self.at(p)?;
                if !self.spike_empty(&n, underwater) {
                    p[1] -= 1;
                    break;
                }
            }
            // Place the first spike block; never extend an existing spike.
            let above = self.at([p[0], p[1] + 1, p[2]])?;
            if above == name
                || (!underwater && (above == "minecraft:ice" || above.ends_with("_ice")))
            {
                continue;
            }
            let Some(mut wet) = self.spike_block(p, states, name, underwater)? else {
                continue;
            };
            let height = if min_height == max_height {
                min_height
            } else {
                min_height + rnd(random.next_int_bound(max_height - min_height))?
            };
            for _ in 0..height {
                p[1] -= 1;
                match self.spike_block(p, states, name, underwater)? {
                    Some(w) => wet = w,
                    None => {
                        p[1] += 1;
                        break;
                    }
                }
            }
            if self.at(p)? == name {
                self.put(p, states[2 + usize::from(wet)])?;
                placed_any = true;
            }
        }
        Ok(placed_any)
    }

    fn spike_block(
        &mut self,
        p: [i32; 3],
        states: &[BlockStateId; 4],
        name: &str,
        underwater: bool,
    ) -> R<Option<bool>, W> {
        let here = self.at(p)?;
        if !self.spike_empty(&here, underwater) {
            return Ok(None);
        }
        // `canSurvive`: hang from a solid block or another spike.
        let above = self.at([p[0], p[1] + 1, p[2]])?;
        if above != name && (air_or_empty_fluid(&above) || replaceable(&above)) {
            return Ok(None);
        }
        let wet = is_fluid_block(&here);
        self.put(p, states[usize::from(wet)])?;
        Ok(Some(wet))
    }

    fn solid(&mut self, p: [i32; 3]) -> R<bool, W> {
        let n = self.at(p)?;
        Ok(!air_or_empty_fluid(&n) && !replaceable(&n))
    }

    /// `getReplaceState`: the first horizontally adjacent fluid, else air.
    fn replace_state(&mut self, p: [i32; 3], air: BlockStateId) -> R<BlockStateId, W> {
        for [dx, dz] in HORIZONTAL {
            let q = [p[0] + dx, p[1], p[2] + dz];
            if is_fluid_block(&self.at(q)?) {
                return self.world.block(q).map_err(we);
            }
        }
        Ok(air)
    }

    /// `RivuletFeature`.
    fn rivulet(
        &mut self,
        states: &HashMap<String, BlockStateId>,
        air: BlockStateId,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let (cx, cz) = (origin[0] >> 4 << 4, origin[2] >> 4 << 4);
        let inside =
            |x: i32, z: i32| x >= cx - 14 && x <= cx + 15 + 14 && z >= cz - 14 && z <= cz + 15 + 14;
        let mut chosen: Vec<[i32; 3]> = Vec::new();
        let mut seen: HashSet<[i32; 3]> = HashSet::new();
        let start_y = self.height(origin[0], origin[2])?;
        let start = [origin[0], start_y, origin[2]];
        if is_fluid_block(&self.at([start[0], start[1] - 1, start[2]])?) {
            return Ok(false);
        }
        let mut branches = std::collections::VecDeque::from([start]);
        let mut main = true;
        while let Some(branch_start) = branches.pop_front() {
            let mut max_length = 5 + rnd(random.next_int_bound(8))?;
            if main {
                main = false;
                max_length += 12;
            }
            let mut last = branch_start;
            for _ in 0..max_length {
                add_once(&mut seen, &mut chosen, branch_start);
                let mut chosen_direction: Option<[i32; 2]> = None;
                let mut chosen_height = 0;
                let mut possible = 0;
                for d in HORIZONTAL {
                    let q = [last[0] + d[0], last[1], last[2] + d[1]];
                    if !inside(q[0], q[2]) || is_fluid_block(&self.at([q[0], q[1] - 1, q[2]])?) {
                        continue;
                    }
                    let height = self.height(q[0], q[2])?;
                    if height <= q[1] && self.solid([q[0], height - 1, q[2]])? {
                        possible += 1;
                        if chosen_direction.is_none() || rnd(random.next_int_bound(possible))? == 0
                        {
                            chosen_height = height;
                            chosen_direction = Some(d);
                        }
                    }
                }
                let Some(d) = chosen_direction else {
                    break;
                };
                if possible > 1 && rnd(random.next_int_bound(3))? == 0 {
                    branches.push_back(last);
                }
                let next = [last[0] + d[0], chosen_height, last[2] + d[1]];
                if !seen.contains(&next)
                    && !is_fluid_block(&self.at([next[0], next[1] - 1, next[2]])?)
                {
                    last = next;
                    add_once(&mut seen, &mut chosen, last);
                } else {
                    break;
                }
            }
        }
        if chosen.is_empty() {
            return Ok(false);
        }
        for pos in java_hash_set_order(&chosen) {
            let mut p = pos;
            while !air_or_empty_fluid(&self.at(p)?) {
                let s = self.replace_state(p, air)?;
                self.put(p, s)?;
                p[1] += 1;
            }
            p = [pos[0], pos[1] - 1, pos[2]];
            let s = self.replace_state(p, air)?;
            self.put(p, s)?;
            p[1] -= 1;
            let here = self.at(p)?;
            if let Some(state) = states.get(&here) {
                self.put(p, *state)?;
                p[1] -= 1;
                self.put(p, *state)?;
            }
        }
        Ok(true)
    }
}

fn add_once(seen: &mut HashSet<[i32; 3]>, chosen: &mut Vec<[i32; 3]>, p: [i32; 3]) {
    if seen.insert(p) {
        chosen.push(p);
    }
}

/// Iteration order of a Java `HashSet<BlockPos>` filled in this (insertion) order: buckets by the
/// spread hash, insertion order inside a bucket, table doubling from 16 at a 0.75 load factor.
fn java_hash_set_order(items: &[[i32; 3]]) -> Vec<[i32; 3]> {
    let mut capacity = 16usize;
    while items.len() > capacity * 3 / 4 {
        capacity *= 2;
    }
    let bucket = |p: &[i32; 3]| {
        // `Vec3i.hashCode`: `(y + z * 31) * 31 + x`, then `h ^ (h >>> 16)`.
        let h = p[1]
            .wrapping_add(p[2].wrapping_mul(31))
            .wrapping_mul(31)
            .wrapping_add(p[0]);
        let spread = h ^ ((h as u32) >> 16) as i32;
        (spread as u32 as usize) & (capacity - 1)
    };
    let mut indexed: Vec<(usize, usize, [i32; 3])> = items
        .iter()
        .enumerate()
        .map(|(i, p)| (bucket(p), i, *p))
        .collect();
    indexed.sort_by_key(|e| (e.0, e.1));
    indexed.into_iter().map(|e| e.2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        registry::BlockRegistry,
        worldgen::{anchor::WorldGenerationContext, placement::PlacementQueryError},
    };
    use horizon_core::random::XoroshiroRandom;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    fn registry() -> &'static BlockRegistry {
        let names = [
            "minecraft:air",
            "minecraft:cave_air",
            "minecraft:void_air",
            "minecraft:water",
            "minecraft:lava",
            "tfc:raw_granite",
            "tfc:sulfur_granite",
            "tfc:calcite",
        ];
        let mut rows = Vec::new();
        let mut defaults = BTreeMap::new();
        for n in names {
            defaults.insert(n, rows.len());
            if n.ends_with("water") || n.ends_with("lava") {
                for level in 0..16 {
                    rows.push(json!([n, false, true, "", {"id": n, "properties": {"level": level.to_string()}}]));
                }
            } else if n == "tfc:calcite" {
                for tip in ["false", "true"] {
                    rows.push(json!([n, false, false, "", {"id": n, "properties": {"tip": tip}}]));
                }
            } else {
                rows.push(json!([n, n.ends_with("air"), false, "", {"id": n, "properties": {}}]));
            }
        }
        let facts = json!({"states": rows, "defaults": defaults});
        Box::leak(Box::new(
            BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
        ))
    }

    /// A slope: the ground height is `70 - x`, solid granite below it.
    struct World {
        blocks: HashMap<[i32; 3], BlockStateId>,
        air: BlockStateId,
        granite: BlockStateId,
    }
    impl FeatureWorld for World {
        type Error = ();
        fn block(&mut self, p: [i32; 3]) -> Result<BlockStateId, ()> {
            if let Some(s) = self.blocks.get(&p) {
                return Ok(*s);
            }
            Ok(if p[1] < 70 - p[0] {
                self.granite
            } else {
                self.air
            })
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
        fn height(&mut self, _: HeightmapType, x: i32, _: i32) -> Result<i32, PlacementQueryError> {
            Ok(70 - x)
        }
    }

    #[test]
    fn rivulets_run_downhill_and_turn_rock_to_sulfur() {
        let r = registry();
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        let f = TfcCaveFeature::decode(
            &json!({"type": "tfc:rivulet", "states": [{"replace": "tfc:raw_granite", "with": "tfc:sulfur_granite"}]}).to_string(),
            &bindings,
        )
        .unwrap();
        let mut w = World {
            blocks: HashMap::new(),
            air: r.default_state("minecraft:air").unwrap(),
            granite: r.default_state("tfc:raw_granite").unwrap(),
        };
        assert!(
            f.place(&mut w, &mut XoroshiroRandom::new(4), [2, 68, 2])
                .unwrap()
        );
        let sulfur = r.default_state("tfc:sulfur_granite").unwrap();
        assert!(w.blocks.values().filter(|s| **s == sulfur).count() >= 4);
    }

    #[test]
    fn java_hash_order_groups_by_bucket() {
        let items = [[0, 0, 0], [1, 0, 0], [16, 0, 0], [2, 0, 0]];
        // Hashes are x for y = z = 0, so buckets are x & 15: 0, 1, 0, 2.
        assert_eq!(
            java_hash_set_order(&items),
            vec![[0, 0, 0], [16, 0, 0], [1, 0, 0], [2, 0, 0]]
        );
    }

    #[test]
    fn calcite_hangs_from_the_ceiling_with_a_tip() {
        let r = registry();
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        let f = TfcCaveFeature::decode(
            &json!({"type": "tfc:thin_spike", "state": "tfc:calcite", "radius": 5, "tries": 20,
                "min_height": 2, "max_height": 5, "allow_underwater": false})
            .to_string(),
            &bindings,
        )
        .unwrap();
        // Solid rock for y >= 70 (the slope world is solid below `70 - x`; start far east so
        // the ground is out of the way, i.e. use the area x <= 0 where the ground is above 70).
        let mut w = World {
            blocks: HashMap::new(),
            air: r.default_state("minecraft:air").unwrap(),
            granite: r.default_state("tfc:raw_granite").unwrap(),
        };
        for x in -8..=8 {
            for z in -8..=8 {
                for y in 50..70 {
                    w.blocks.insert([100 + x, y, z], w.air);
                }
            }
        }
        // Ceiling: the world is granite for y < 70 - x = -30 at x = 100, so make it all air
        // and cap the cavity with a stone roof.
        for x in -8..=8 {
            for z in -8..=8 {
                w.blocks.insert([100 + x, 70, z], w.granite);
            }
        }
        assert!(
            f.place(&mut w, &mut XoroshiroRandom::new(2), [100, 60, 0])
                .unwrap()
        );
        // Tip states differ from the plain spike state.
        let plain = r.default_state("tfc:calcite").unwrap();
        assert!(w.blocks.values().any(|s| s.0 == plain.0 + 1));
        let spikes = w
            .blocks
            .iter()
            .filter(|(p, _)| p[1] < 70 && p[1] >= 50 && w.blocks[*p] != w.air)
            .count();
        assert!(spikes > 3);
    }
}
