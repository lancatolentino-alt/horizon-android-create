//! TerraFirmaCraft's cave rock features (EUPL 1.2): `tfc:cave_spike` (`CaveSpikesFeature`),
//! `tfc:large_cave_spike` (`LargeCaveSpikesFeature`) and `tfc:cave_column` (`CaveColumnFeature`).
//!
//! Rock blocks are stand-ins named by rock: the raw block is `tfc:raw_<rock>` (hardened rock is
//! the same block here), the spike is `tfc:spike_<rock>`, a pointed-dripstone look-alike whose
//! `thickness` is the spike part and whose `vertical_direction` is the way it points.

use super::{
    DecodeError, Object,
    block_feature::FeatureBlockBindings,
    feature::{FeatureError, FeatureWorld},
    feature_graph::FeatureExecutionError,
    optional,
    placement::PlacementContext,
    required,
};
use crate::registry::BlockStateId;
use horizon_core::{geometry::trig, random::RandomSource};
use std::{collections::HashMap, sync::Arc};

type R<T, W> = Result<T, FeatureExecutionError<<W as FeatureWorld>::Error>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Small,
    Large,
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Base,
    Middle,
    Tip,
}

#[derive(Clone, Debug)]
struct Rock {
    raw: BlockStateId,
    /// `[direction up?][part][waterlogged]`; absent when the mod has no spike for the rock.
    spikes: Option<[[[BlockStateId; 2]; 3]; 2]>,
}

#[derive(Clone, Debug)]
pub struct TfcSpikeFeature {
    kind: Kind,
    /// `tfc:raw_<rock>` block name -> rock.
    rocks: Arc<HashMap<String, Rock>>,
    /// Rock name -> raw state, for the column's rock lookup.
    by_rock: Arc<HashMap<String, BlockStateId>>,
    names: Arc<Vec<String>>,
}

impl TfcSpikeFeature {
    pub(crate) fn decode(
        json: &str,
        bindings: &FeatureBlockBindings<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(json)?;
        let kind: String = serde_json::from_str(required(&object, "type")?.get())?;
        let _ = optional(&object, "config");
        let kind = match kind.as_str() {
            "tfc:cave_spike" => Kind::Small,
            "tfc:large_cave_spike" => Kind::Large,
            "tfc:cave_column" => Kind::Column,
            _ => return Err(DecodeError::Field("spike type")),
        };
        let registry = bindings.registry();
        let names: Vec<String> = (0..registry.len())
            .map(|i| registry.state(BlockStateId(i as u32)).unwrap().name.clone())
            .collect();
        let mut rocks = HashMap::new();
        let mut by_rock = HashMap::new();
        for name in &names {
            let Some(rock) = name.strip_prefix("tfc:raw_") else {
                continue;
            };
            let Some(raw) = registry.default_state(name) else {
                continue;
            };
            let spikes = registry
                .default_state(&format!("tfc:spike_{rock}"))
                .and_then(|base| {
                    let mut table = [[[base; 2]; 3]; 2];
                    for (d, up) in [false, true].into_iter().enumerate() {
                        for (p, part) in [Part::Base, Part::Middle, Part::Tip]
                            .into_iter()
                            .enumerate()
                        {
                            for wet in 0..2 {
                                let mut st = registry.state(base)?.clone();
                                if st.properties.contains_key("thickness") {
                                    st.properties.insert(
                                        "thickness".into(),
                                        match part {
                                            Part::Base => "base",
                                            // TFC's middle is 8 pixels wide: vanilla's frustum, not its 10-pixel middle.
                                            Part::Middle => "frustum",
                                            Part::Tip => "tip",
                                        }
                                        .into(),
                                    );
                                }
                                if st.properties.contains_key("vertical_direction") {
                                    st.properties.insert(
                                        "vertical_direction".into(),
                                        if up { "up" } else { "down" }.into(),
                                    );
                                }
                                if st.properties.contains_key("waterlogged") {
                                    st.properties.insert(
                                        "waterlogged".into(),
                                        if wet == 1 { "true" } else { "false" }.into(),
                                    );
                                }
                                table[d][p][wet] = registry.id(&st)?;
                            }
                        }
                    }
                    Some(table)
                });
            by_rock.insert(rock.to_owned(), raw);
            rocks.insert(name.clone(), Rock { raw, spikes });
        }
        Ok(Self {
            kind,
            rocks: Arc::new(rocks),
            by_rock: Arc::new(by_rock),
            names: Arc::new(names),
        })
    }

    pub fn place<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        match self.kind {
            Kind::Column => self.column(world, random, origin),
            _ => self.spikes(world, random, origin),
        }
    }

    fn name_at<W: FeatureWorld>(&self, world: &mut W, p: [i32; 3]) -> R<&str, W> {
        let s = world.block(p).map_err(we)?;
        Ok(self.names[s.0 as usize].as_str())
    }

    /// `CaveSpikesFeature.place`.
    fn spikes<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        // `true` is UP.
        let mut up = random.next_boolean();
        for attempt in 0..2 {
            let wall = [origin[0], origin[1] + if up { -1 } else { 1 }, origin[2]];
            let name = self.name_at(world, wall)?.to_owned();
            if let Some(rock) = self.rocks.get(&name) {
                // `wallRock.spike().ifPresent`.
                if let Some(spikes) = rock.spikes {
                    let step = if up { 1 } else { -1 };
                    let mut placer = Spiker {
                        feature: self,
                        spikes: &spikes[usize::from(up)],
                        raw: rock.raw,
                        step,
                        world,
                    };
                    if self.kind == Kind::Large {
                        placer.large(random, origin)?;
                    } else {
                        let weight = random.next_float();
                        placer.small(origin, weight)?;
                    }
                }
                return Ok(true);
            }
            if attempt == 0 {
                up = !up;
            }
        }
        Ok(true)
    }

    /// `CaveColumnFeature.place`.
    fn column<W: FeatureWorld + PlacementContext>(
        &self,
        world: &mut W,
        random: &mut impl RandomSource,
        origin: [i32; 3],
    ) -> R<bool, W> {
        let amp = next_float(random, 0.25, 0.6);
        let offset = next_int(random, 1, 5)? as f32;
        let period = next_float(random, 0.4, 0.6);
        let below = self.name_at(world, [origin[0], origin[1] - 1, origin[2]])?;
        if !base_stone(below) {
            return Ok(false);
        }
        let seed = world.world_seed().map_err(FeatureExecutionError::Query)?;
        let terrain = super::tfc_terrain::active_for(seed);
        let max_y = world.max_y().map_err(FeatureExecutionError::Query)?;
        let (xo, zo) = (origin[0], origin[2]);
        let mut cache = super::tfc_terrain::TfcTerrain::rock_cache(xo >> 4, zo >> 4);
        let mut y = origin[1];
        loop {
            let state = match &terrain {
                Some(terrain) => {
                    let surface = terrain.surface_height_at(xo, zo);
                    let rock = terrain.rock(xo, y, zo, surface, &mut cache);
                    self.by_rock.get(rock).copied()
                }
                None => None,
            };
            let Some(state) = state else {
                return Ok(false);
            };
            let radius = (amp * trig::sin(f64::from(period * y as f32 + offset)) + 2.0) as i32;
            for x in -radius..=radius {
                for z in -radius..=radius {
                    if x * x + z * z < radius * radius {
                        world.set_block([xo + x, y, zo + z], state, 2).map_err(we)?;
                    }
                }
            }
            y += 1;
            let middle = self.name_at(world, [xo, y, zo])?;
            if middle != "minecraft:cave_air" && middle != "minecraft:lava" || y > max_y {
                return Ok(true);
            }
        }
    }
}

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

/// `Mth.nextFloat`.
fn next_float(random: &mut impl RandomSource, min: f32, max: f32) -> f32 {
    if min >= max {
        min
    } else {
        random.next_float() * (max - min) + min
    }
}

/// `Mth.nextInt`.
fn next_int<E>(
    random: &mut impl RandomSource,
    min: i32,
    max: i32,
) -> Result<i32, FeatureExecutionError<E>> {
    if min >= max {
        Ok(min)
    } else {
        Ok(random.next_int_bound(max - min + 1)? + min)
    }
}

struct Spiker<'a, W> {
    feature: &'a TfcSpikeFeature,
    /// `[part][waterlogged]` for the pointing direction.
    spikes: &'a [[BlockStateId; 2]; 3],
    raw: BlockStateId,
    /// `direction.getStepY()`.
    step: i32,
    world: &'a mut W,
}

impl<W: FeatureWorld + PlacementContext> Spiker<'_, W> {
    fn put(&mut self, p: [i32; 3], state: BlockStateId) -> R<(), W> {
        self.world.set_block(p, state, 2).map_err(we)?;
        Ok(())
    }

    fn name(&mut self, p: [i32; 3]) -> R<String, W> {
        Ok(self.feature.name_at(self.world, p)?.to_owned())
    }

    /// `replaceBlock`: cave air, water or lava becomes the spike part (waterlogged in water).
    fn replace(&mut self, p: [i32; 3], part: Part) -> R<(), W> {
        let index = part as usize;
        match self.name(p)?.as_str() {
            "minecraft:cave_air" | "minecraft:lava" => self.put(p, self.spikes[index][0]),
            "minecraft:water" => self.put(p, self.spikes[index][1]),
            _ => Ok(()),
        }
    }

    /// `replaceBlockWithoutFluid`.
    fn replace_plain(&mut self, p: [i32; 3], state: BlockStateId) -> R<(), W> {
        if matches!(
            self.name(p)?.as_str(),
            "minecraft:cave_air" | "minecraft:water" | "minecraft:lava"
        ) {
            self.put(p, state)?;
        }
        Ok(())
    }

    fn along(&self, p: [i32; 3], n: i32) -> [i32; 3] {
        [p[0], p[1] + self.step * n, p[2]]
    }

    /// `placeSmallSpike(level, pos, spike, raw, direction, sizeWeight)`.
    fn small(&mut self, pos: [i32; 3], weight: f32) -> R<(), W> {
        let wall = self.along(pos, -1);
        if base_stone(&self.name(wall)?) {
            self.put(wall, self.raw)?;
        }
        if weight < 0.2 {
            self.replace(pos, Part::Middle)?;
            self.replace(self.along(pos, 1), Part::Tip)?;
        } else if weight < 0.7 {
            self.replace(pos, Part::Base)?;
            self.replace(self.along(pos, 1), Part::Middle)?;
            self.replace(self.along(pos, 2), Part::Tip)?;
        } else {
            self.replace_plain(pos, self.raw)?;
            self.replace(self.along(pos, 1), Part::Base)?;
            self.replace(self.along(pos, 2), Part::Middle)?;
            self.replace(self.along(pos, 3), Part::Tip)?;
        }
        Ok(())
    }

    /// `LargeCaveSpikesFeature.place`.
    fn large(&mut self, random: &mut impl RandomSource, pos: [i32; 3]) -> R<(), W> {
        let height = 6 + random.next_int_bound(11)?;
        let radius = 2 + random.next_int_bound(1)?;
        let mut max_height_reached = 0;
        for y in -3..=height {
            let mut radius_squared =
                radius as f32 * (1.0 - 1.5f32 * y.abs() as f32 / height as f32);
            if radius_squared < 0.0 {
                continue;
            }
            radius_squared *= radius_squared;
            for x in -radius..=radius {
                for z in -radius..=radius {
                    let p = [pos[0] + x, pos[1] + y * self.step, pos[2] + z];
                    let actual = (x * x + z * z) as f32 / radius_squared;
                    if f64::from(actual) < 0.7 {
                        self.replace_plain(p, self.raw)?;
                        if x == 0 && z == 0 {
                            max_height_reached = y;
                        }
                    } else if f64::from(actual) < 0.85 && random.next_boolean() {
                        // Only fill in if continuing downwards.
                        let behind = self.world.block(self.along(p, -1)).map_err(we)?;
                        if behind == self.raw {
                            self.replace_plain(p, self.raw)?;
                        }
                    } else if f64::from(actual) < 1.0 && random.next_int_bound(3)? == 0 && y > 0 {
                        let weight = random.next_float();
                        self.small(p, weight)?;
                    }
                }
            }
        }
        self.small(self.along(pos, max_height_reached - 1), 1.0)
    }
}

fn we<E>(e: E) -> FeatureExecutionError<E> {
    FeatureExecutionError::Basic(FeatureError::World(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heightmap::HeightmapType;
    use crate::{
        registry::BlockRegistry,
        worldgen::{anchor::WorldGenerationContext, placement::PlacementQueryError},
    };
    use horizon_core::random::XoroshiroRandom;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    const NAMES: [&str; 8] = [
        "minecraft:air",
        "minecraft:cave_air",
        "minecraft:void_air",
        "minecraft:stone",
        "tfc:raw_granite",
        "tfc:spike_granite",
        "minecraft:water",
        "minecraft:lava",
    ];

    fn registry() -> &'static BlockRegistry {
        let mut rows = Vec::new();
        let mut defaults = BTreeMap::new();
        let rocks = [
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
            "limestone",
            "marble",
            "phyllite",
            "quartzite",
            "rhyolite",
            "schist",
            "shale",
            "slate",
            "tuff",
        ];
        let extra: Vec<String> = rocks.iter().map(|r| format!("tfc:raw_{r}")).collect();
        for n in NAMES
            .iter()
            .copied()
            .chain(extra.iter().map(String::as_str))
        {
            defaults.insert(n, rows.len());
            if n.ends_with("lava") || n.ends_with("water") {
                for level in 0..16 {
                    rows.push(json!([n, false, true, "", {"id": n, "properties": {"level": level.to_string()}}]));
                }
            } else if n == "tfc:spike_granite" {
                for t in ["base", "frustum", "tip"] {
                    for d in ["up", "down"] {
                        for w in ["false", "true"] {
                            rows.push(json!([n, false, false, "", {"id": n, "properties":
                                {"thickness": t, "vertical_direction": d, "waterlogged": w}}]));
                        }
                    }
                }
                // The default is the first row pushed for the name.
            } else {
                rows.push(json!([n, n.ends_with("air"), false, "", {"id": n, "properties": {}}]));
            }
        }
        let facts = json!({"states": rows, "defaults": defaults});
        Box::leak(Box::new(
            BlockRegistry::from_facts_json(facts.to_string().as_bytes()).unwrap(),
        ))
    }

    struct World {
        blocks: HashMap<[i32; 3], BlockStateId>,
        rock: BlockStateId,
        cave: BlockStateId,
    }
    impl FeatureWorld for World {
        type Error = ();
        fn block(&mut self, p: [i32; 3]) -> Result<BlockStateId, ()> {
            // A cave between y=60 and y=70 inside solid raw granite.
            Ok(*self.blocks.get(&p).unwrap_or(if (60..70).contains(&p[1]) {
                &self.cave
            } else {
                &self.rock
            }))
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
            Ok(100)
        }
        fn world_seed(&mut self) -> Result<i64, PlacementQueryError> {
            Ok(99)
        }
    }

    fn feature(kind: &str) -> (TfcSpikeFeature, World) {
        let r = registry();
        let bindings = FeatureBlockBindings::new(r, BTreeSet::new()).unwrap();
        let f =
            TfcSpikeFeature::decode(&json!({"type": kind, "config": {}}).to_string(), &bindings)
                .unwrap();
        let world = World {
            blocks: HashMap::new(),
            rock: r.default_state("tfc:raw_granite").unwrap(),
            cave: r.default_state("minecraft:cave_air").unwrap(),
        };
        (f, world)
    }

    fn spike_ids(f: &TfcSpikeFeature) -> Vec<BlockStateId> {
        let s = f.rocks["tfc:raw_granite"].spikes.unwrap();
        s.iter().flatten().flatten().copied().collect()
    }

    #[test]
    fn a_small_spike_hangs_or_rises_from_the_wall_it_stands_on() {
        let (f, _) = feature("tfc:cave_spike");
        let ids = spike_ids(&f);
        let (mut up, mut down) = (0, 0);
        for seed in 0..40 {
            let (_, mut world) = feature("tfc:cave_spike");
            // Ceiling at y=70, floor at y=59; stand mid-cave next to neither: pick both ends.
            for origin in [[0, 69, 0], [0, 60, 0]] {
                f.place(&mut world, &mut XoroshiroRandom::new(seed), origin)
                    .unwrap();
            }
            for (p, s) in &world.blocks {
                if ids.contains(s) {
                    // Spikes sit in the cave and extend away from a wall.
                    assert!((60..70).contains(&p[1]), "{p:?}");
                    if p[1] >= 66 {
                        down += 1;
                    } else {
                        up += 1;
                    }
                }
            }
        }
        assert!(up > 0 && down > 0, "up {up}, down {down}");
    }

    #[test]
    fn a_large_spike_fills_a_cone_of_raw_rock_and_is_deterministic() {
        let (f, mut a) = feature("tfc:large_cave_spike");
        let (_, mut b) = feature("tfc:large_cave_spike");
        f.place(&mut a, &mut XoroshiroRandom::new(5), [0, 69, 0])
            .unwrap();
        f.place(&mut b, &mut XoroshiroRandom::new(5), [0, 69, 0])
            .unwrap();
        assert_eq!(a.blocks, b.blocks);
        let raw = a.rock;
        let filled = a.blocks.values().filter(|s| **s == raw).count();
        assert!(filled > 10, "raw blocks placed: {filled}");
    }

    #[test]
    fn a_column_rises_through_the_cave_in_the_rock_of_its_layer() {
        crate::worldgen::tfc_terrain::set_active(
            99,
            Arc::new(crate::worldgen::tfc_terrain::TfcTerrain::new(99)),
        );
        let (f, mut world) = feature("tfc:cave_column");
        assert!(
            f.place(&mut world, &mut XoroshiroRandom::new(2), [0, 60, 0])
                .unwrap()
        );
        let ys: Vec<i32> = world.blocks.keys().map(|p| p[1]).collect();
        assert_eq!((ys.iter().min(), ys.iter().max()), (Some(&60), Some(&69)));
        for s in world.blocks.values() {
            assert!(f.names[s.0 as usize].starts_with("tfc:raw_"));
        }
        // The shaft always includes the centre column.
        assert!((60..70).all(|y| world.blocks.contains_key(&[0, y, 0])));
    }

    #[test]
    fn a_column_needs_stone_below() {
        let (f, mut world) = feature("tfc:cave_column");
        // Floating in the cave with air below: nothing is placed.
        assert!(
            !f.place(&mut world, &mut XoroshiroRandom::new(1), [0, 65, 0])
                .unwrap()
        );
        assert!(world.blocks.is_empty());
    }
}
