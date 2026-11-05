// Create kinetics: shafts, cogwheels, large cogwheels, power sources (creative motor, hand
// crank, water wheel), consumers (mechanical drill, mechanical press) and stress.
// Injected as `crate::game::create_kinetics`. Each kinetic block has a block entity holding
// its speed (rpm, signed). Speed spreads one block per tick from sources along a chain of
// "parents"; a block that loses its parent clears itself, so removing a source winds the
// network down without stale speeds circulating in loops. Every few ticks each source walks
// its whole network: if the consumers stress (impact x rpm) beats the sources capacity the
// sources stop until the load is cut.
use super::native_block_entity_draw::BlockEntityDraw;
use super::native_hooks::NativeScreen;
use super::{Game, GameLevel};
use horizon_client::containers::{ModBlockEntity, ModContainer};
use horizon_client::hud::HudBatch;
use horizon_core::position::BlockPos;
use horizon_world::level::{LevelAccess, PlacementContext};
use horizon_world::nbt::{Compound, JavaString, Tag};
use horizon_world::registry::BlockStateId;
use std::collections::HashSet;

pub const BE_ID: &str = "create:kinetic";
const NONE_HOPS: i32 = 255;
const MAX_HOPS: i32 = 128;
const MOTOR_DEFAULT_RPM: i32 = 16;
const MOTOR_MAX_RPM: i32 = 256;
const MOTOR_BLOCK: &str = "create:creative_motor";
const MOTOR_MENU: &str = "create:creative_motor";
const MOTOR_CAPACITY: i32 = 16384;
const CRANK_RPM: f32 = 32.0;
const CRANK_TICKS: i32 = 100;
const WHEEL_RPM: f32 = 8.0;
const NET_LIMIT: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Shaft,
    Cogwheel,
    LargeCogwheel,
    Motor,
    Crank,
    Wheel,
    Drill,
    Press,
}

pub fn kind_of(block: &str) -> Option<Kind> {
    match block {
        "create:shaft" => Some(Kind::Shaft),
        "create:cogwheel" => Some(Kind::Cogwheel),
        "create:large_cogwheel" => Some(Kind::LargeCogwheel),
        "create:creative_motor" => Some(Kind::Motor),
        "create:hand_crank" => Some(Kind::Crank),
        "create:water_wheel" => Some(Kind::Wheel),
        "create:mechanical_drill" => Some(Kind::Drill),
        "create:mechanical_press" => Some(Kind::Press),
        _ => None,
    }
}

impl Kind {
    fn uses_facing(self) -> bool {
        !matches!(self, Kind::Shaft | Kind::Cogwheel | Kind::LargeCogwheel)
    }
    fn is_source(self) -> bool {
        matches!(self, Kind::Motor | Kind::Crank | Kind::Wheel)
    }
    // Stress units per rpm a consumer asks for.
    fn impact(self) -> i32 {
        match self {
            Kind::Drill => 4,
            Kind::Press => 8,
            _ => 0,
        }
    }
    // Stress units a source gives while turning at `rpm`.
    fn capacity(self, rpm: f32) -> i32 {
        match self {
            Kind::Motor => MOTOR_CAPACITY,
            Kind::Crank => (8.0 * rpm.abs()) as i32,
            Kind::Wheel => (16.0 * rpm.abs()) as i32,
            _ => 0,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Kind::Shaft => "Shaft",
            Kind::Cogwheel => "Cogwheel",
            Kind::LargeCogwheel => "Large Cogwheel",
            Kind::Motor => "Creative Motor",
            Kind::Crank => "Hand Crank",
            Kind::Wheel => "Water Wheel",
            Kind::Drill => "Mechanical Drill",
            Kind::Press => "Mechanical Press",
        }
    }
}

// A kinetic block as the network sees it. `axis` is 0/1/2 for x/y/z; a block with a
// `facing` has it in `front` (and its `axis` is that directions axis).
#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub kind: Kind,
    pub axis: usize,
    pub front: [i32; 3],
}

impl Node {
    fn is_cog(&self) -> bool {
        matches!(self.kind, Kind::Cogwheel | Kind::LargeCogwheel)
    }
    // The presss shaft runs across its facing.
    fn shaft_axis(&self) -> usize {
        if self.axis == 0 {
            2
        } else {
            0
        }
    }
    // Whether a shaft end of this block faces the neighbour in direction `d`.
    fn has_end(&self, d: [i32; 3]) -> bool {
        let unit = d.iter().filter(|c| **c != 0).count() == 1;
        match self.kind {
            Kind::Motor => d == self.front,
            Kind::Crank | Kind::Drill => d == neg(self.front),
            Kind::Press => unit && d[self.shaft_axis()] != 0,
            _ => unit && d[self.axis] != 0,
        }
    }
}

fn neg(d: [i32; 3]) -> [i32; 3] {
    [-d[0], -d[1], -d[2]]
}

fn parse_axis(s: &str) -> Option<usize> {
    match s {
        "x" => Some(0),
        "y" => Some(1),
        "z" => Some(2),
        _ => None,
    }
}

fn parse_dir(s: &str) -> Option<[i32; 3]> {
    match s {
        "down" => Some([0, -1, 0]),
        "up" => Some([0, 1, 0]),
        "north" => Some([0, 0, -1]),
        "south" => Some([0, 0, 1]),
        "west" => Some([-1, 0, 0]),
        "east" => Some([1, 0, 0]),
        _ => None,
    }
}

fn axis_of(d: [i32; 3]) -> usize {
    d.iter().position(|c| *c != 0).unwrap_or(1)
}

fn axis_char(axis: usize) -> &'static str {
    ["x", "y", "z"][axis.min(2)]
}

fn dir_name(d: [i32; 3]) -> &'static str {
    match d {
        [0, -1, 0] => "down",
        [0, 1, 0] => "up",
        [0, 0, -1] => "north",
        [0, 0, 1] => "south",
        [-1, 0, 0] => "west",
        _ => "east",
    }
}

// The factor `f` with `my_speed = neighbour_speed * f` when `me` and the neighbour at
// offset `d` (neighbour minus me) are connected, else None. Rules as in Create:
//  - shaft ends facing each other: same speed;
//  - small cog and small cog: same axis, side by side in their plane: reversed;
//  - small cog and large cog: same axis, diagonal in their plane: 2:1, reversed;
//  - large cog and large cog: different axes, diagonal in the plane of both axes: reversed
//    or not by which side of each the neighbour is on.
pub fn link(me: &Node, d: [i32; 3], nb: &Node) -> Option<f32> {
    if me.has_end(d) && nb.has_end(neg(d)) {
        return Some(1.0);
    }
    if !me.is_cog() || !nb.is_cog() {
        return None;
    }
    let nonzero = d.iter().filter(|c| **c != 0).count();
    match (me.kind, nb.kind) {
        (Kind::Cogwheel, Kind::Cogwheel) => {
            if me.axis == nb.axis && d[me.axis] == 0 && nonzero == 1 {
                Some(-1.0)
            } else {
                None
            }
        }
        (Kind::LargeCogwheel, Kind::LargeCogwheel) => {
            if me.axis == nb.axis {
                return None;
            }
            let third = 3 - me.axis - nb.axis;
            if d[me.axis].abs() == 1 && d[nb.axis].abs() == 1 && d[third] == 0 {
                let mine = d[me.axis] > 0;
                let theirs = d[nb.axis] > 0;
                Some(if mine ^ theirs { -1.0 } else { 1.0 })
            } else {
                None
            }
        }
        _ => {
            // one small, one large
            if me.axis != nb.axis || d[me.axis] != 0 || nonzero != 2 {
                return None;
            }
            if me.kind == Kind::Cogwheel {
                Some(-2.0)
            } else {
                Some(-0.5)
            }
        }
    }
}

// Offsets worth looking at for `me`.
pub fn offsets(me: &Node) -> Vec<[i32; 3]> {
    let mut out = Vec::new();
    for a in 0..3 {
        for s in [-1, 1] {
            let mut d = [0; 3];
            d[a] = s;
            if me.has_end(d) {
                out.push(d);
            }
        }
    }
    if !me.is_cog() {
        return out;
    }
    let (p, q) = ((me.axis + 1) % 3, (me.axis + 2) % 3);
    if me.kind == Kind::Cogwheel {
        // small cogs side by side in the plane
        for a in [p, q] {
            for s in [-1, 1] {
                let mut d = [0; 3];
                d[a] = s;
                out.push(d);
            }
        }
    }
    // diagonal in the plane (small <-> large)
    for sp in [-1, 1] {
        for sq in [-1, 1] {
            let mut d = [0; 3];
            d[p] = sp;
            d[q] = sq;
            out.push(d);
        }
    }
    if me.kind == Kind::LargeCogwheel {
        // diagonal in the plane of this axis and each other axis (large <-> large)
        for other in [p, q] {
            for sa in [-1, 1] {
                for so in [-1, 1] {
                    let mut d = [0; 3];
                    d[me.axis] = sa;
                    d[other] = so;
                    out.push(d);
                }
            }
        }
    }
    out
}

// ---- block entity data ----

fn get_f32(n: &Compound, k: &str) -> f32 {
    match n.get(&JavaString::from(k)) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        _ => 0.0,
    }
}

fn get_int(n: &Compound, k: &str, default: i32) -> i32 {
    match n.get(&JavaString::from(k)) {
        Some(Tag::Int(v)) => *v,
        _ => default,
    }
}

fn get_hops(n: &Compound) -> i32 {
    get_int(n, "hops", NONE_HOPS)
}

fn get_parent(n: &Compound) -> Option<[i32; 3]> {
    match n.get(&JavaString::from("parent")) {
        Some(Tag::IntArray(v)) if v.len() == 3 => Some([v[0], v[1], v[2]]),
        _ => None,
    }
}

fn put_int(n: &mut Compound, k: &str, v: i32) {
    n.insert(JavaString::from(k), Tag::Int(v));
}

fn put_f32(n: &mut Compound, k: &str, v: f32) {
    n.insert(JavaString::from(k), Tag::Float(v.to_bits()));
}

fn set_state(n: &mut Compound, speed: f32, hops: i32, parent: Option<[i32; 3]>) {
    put_f32(n, "speed", speed);
    put_int(n, "hops", hops);
    match parent {
        Some(p) => {
            n.insert(JavaString::from("parent"), Tag::IntArray(p.to_vec()));
        }
        None => {
            n.remove(&JavaString::from("parent"));
        }
    }
}

// ---- world access ----

fn node_at(game: &Game<'_>, p: [i32; 3]) -> Option<Node> {
    let kind = kind_of(&game.native_block(p))?;
    if kind.uses_facing() {
        let front = parse_dir(&game.native_block_property(p, "facing")?)?;
        Some(Node { kind, axis: axis_of(front), front })
    } else {
        let axis = parse_axis(&game.native_block_property(p, "axis")?)?;
        Some(Node { kind, axis, front: [0; 3] })
    }
}

#[derive(Clone, Copy)]
struct Reading {
    speed: f32,
    hops: i32,
    load: i32,
    cap: i32,
    target: f32,
}

// What the kinetic block entity at `p` holds.
fn read_neighbour(game: &mut Game<'_>, p: [i32; 3]) -> Option<Reading> {
    game.native_with_mod_block_entity(p, |e| {
        if e.id == BE_ID {
            Some(Reading {
                speed: get_f32(&e.nbt, "speed"),
                hops: get_hops(&e.nbt),
                load: get_int(&e.nbt, "load", 0),
                cap: get_int(&e.nbt, "cap", 0),
                target: get_f32(&e.nbt, "target"),
            })
        } else {
            None
        }
    })
    .flatten()
}

struct Out {
    speed: f32,
    hops: i32,
    parent: Option<[i32; 3]>,
    load: i32,
    cap: i32,
}

fn idle() -> Out {
    Out { speed: 0.0, hops: NONE_HOPS, parent: None, load: 0, cap: 0 }
}

// One tick of a block that is not a source.
fn step(game: &mut Game<'_>, pos: [i32; 3], me: &Node, nbt: &Compound) -> Out {
    let speed = get_f32(nbt, "speed");
    let hops = get_hops(nbt);
    let at = |d: [i32; 3]| [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
    if let Some(d) = get_parent(nbt) {
        // Keep a parent that still feeds us from closer to a source; else wind down.
        let p = at(d);
        if let (Some(nb), Some(r)) = (node_at(game, p), read_neighbour(game, p)) {
            if let Some(f) = link(me, d, &nb) {
                if r.speed != 0.0 && r.hops < hops && r.hops + 1 <= MAX_HOPS {
                    return Out { speed: r.speed * f, hops: r.hops + 1, parent: Some(d), load: r.load, cap: r.cap };
                }
            }
        }
        return idle();
    }
    if speed != 0.0 {
        return idle();
    }
    let mut best: Option<(i32, f32, [i32; 3], i32, i32)> = None;
    for d in offsets(me) {
        let p = at(d);
        let Some(nb) = node_at(game, p) else { continue };
        let Some(f) = link(me, d, &nb) else { continue };
        let Some(r) = read_neighbour(game, p) else { continue };
        if r.speed == 0.0 || r.hops >= MAX_HOPS {
            continue;
        }
        let cand = (r.hops + 1, r.speed * f, d, r.load, r.cap);
        let better = match best {
            None => true,
            Some(b) => cand.0 < b.0 || (cand.0 == b.0 && cand.1.abs() < b.1.abs()),
        };
        if better {
            best = Some(cand);
        }
    }
    match best {
        Some((h, s, d, load, cap)) => Out { speed: s, hops: h, parent: Some(d), load, cap },
        None => idle(),
    }
}

// ---- stress ----

fn motor_rpm(nbt: &Compound) -> i32 {
    get_int(nbt, "rpm", MOTOR_DEFAULT_RPM)
}

fn water_beside(game: &Game<'_>, pos: [i32; 3], me: &Node) -> bool {
    for a in 0..3 {
        if a == me.axis {
            continue;
        }
        for s in [-1, 1] {
            let mut p = pos;
            p[a] += s;
            if game.native_block(p) == "minecraft:water" {
                return true;
            }
        }
    }
    false
}

// The rpm a source wants to turn at.
fn source_target(game: &Game<'_>, pos: [i32; 3], me: &Node, nbt: &Compound) -> f32 {
    match me.kind {
        Kind::Motor => motor_rpm(nbt) as f32,
        Kind::Crank => {
            let now = game.ticks as i32;
            if get_int(nbt, "until", now.wrapping_sub(1)).wrapping_sub(now) > 0 {
                CRANK_RPM
            } else {
                0.0
            }
        }
        Kind::Wheel => {
            if water_beside(game, pos, me) {
                WHEEL_RPM
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

// Walks the network around the source at `pos` (turning at `target` rpm) and returns
// (stress asked for, stress available).
fn analyse(game: &mut Game<'_>, pos: [i32; 3], me: &Node, target: f32) -> (i32, i32) {
    let mut seen: HashSet<[i32; 3]> = HashSet::new();
    seen.insert(pos);
    let mut queue: Vec<([i32; 3], Node, f32)> = vec![(pos, *me, target)];
    let mut load = 0i32;
    let mut cap = 0i32;
    let mut i = 0;
    while i < queue.len() && queue.len() < NET_LIMIT {
        let (p, node, speed) = queue[i];
        i += 1;
        if node.kind.is_source() {
            let tgt = if p == pos {
                target
            } else {
                read_neighbour(game, p).map(|r| r.target).unwrap_or(0.0)
            };
            cap = cap.saturating_add(node.kind.capacity(tgt));
        }
        load = load.saturating_add((node.kind.impact() as f32 * speed.abs()) as i32);
        for d in offsets(&node) {
            let q = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
            if seen.contains(&q) {
                continue;
            }
            let Some(nb) = node_at(game, q) else { continue };
            let Some(f) = link(&nb, neg(d), &node) else { continue };
            seen.insert(q);
            queue.push((q, nb, speed * f));
        }
    }
    (load, cap)
}

// Minecrafts colour-code prefix, built without a literal so the module has no escapes.
fn sect() -> String {
    char::from(0xA7u8).to_string()
}

// ---- the mechanical drill ----
// As in Create: a spinning drill works on the block in front of it. Each stage takes
// hardness / (rpm / 100) ticks (at least one), ten stages break the block, and the crack
// shows on it meanwhile. The block drops what its loot table gives, as without a tool.

fn drill_id(pos: [i32; 3]) -> i32 {
    let h = pos[0].wrapping_mul(73856093) ^ pos[1].wrapping_mul(19349663) ^ pos[2].wrapping_mul(83492791);
    0x4000_0000 | (h & 0x3fff_ffff)
}

fn drill_idle(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound) {
    put_int(nbt, "bp", 0);
    put_int(nbt, "bt", 0);
    game.destroying.remove(&drill_id(pos));
}

fn drill_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, me: &Node, speed: f32) {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return;
    }
    if speed.abs() < 1.0 {
        drill_idle(game, pos, nbt);
        return;
    }
    let target = [pos[0] + me.front[0], pos[1] + me.front[1], pos[2] + me.front[2]];
    let state = game.world.block(target[0], target[1], target[2]);
    let facts = game.res.facts().get(state).clone();
    let name = facts.name.to_string();
    let empty = name == "minecraft:air"
        || name == "minecraft:cave_air"
        || name == "minecraft:void_air"
        || name == "minecraft:water"
        || name == "minecraft:lava";
    if empty || facts.hardness < 0.0 {
        drill_idle(game, pos, nbt);
        return;
    }
    let was = [get_int(nbt, "bx", i32::MIN), get_int(nbt, "by", i32::MIN), get_int(nbt, "bz", i32::MIN)];
    if was != target {
        put_int(nbt, "bx", target[0]);
        put_int(nbt, "by", target[1]);
        put_int(nbt, "bz", target[2]);
        put_int(nbt, "bp", 0);
        put_int(nbt, "bt", 0);
    }
    let hardness = facts.hardness.max(0.05);
    let break_speed = (speed.abs() / 100.0).clamp(1.0 / 128.0, 16.0);
    let mut wait = get_int(nbt, "bt", 0) - 1;
    let mut progress = get_int(nbt, "bp", 0);
    if wait <= 0 {
        let step = ((break_speed / hardness) as i32).clamp(1, (10 - progress).max(1));
        progress += step;
        wait = ((hardness / break_speed) as i32).max(1);
    }
    put_int(nbt, "bt", wait);
    put_int(nbt, "bp", progress);
    let id = drill_id(pos);
    if progress >= 10 {
        put_int(nbt, "bp", 0);
        put_int(nbt, "bt", 0);
        game.destroying.remove(&id);
        let drops = game.block_loot_at(target, state, None);
        let dim = game.dim.id().to_string();
        if game.native_set_block(&dim, target, BlockStateId(0), 3) {
            game.destroy_particles(target, state);
            for d in drops {
                if &*d.item != "minecraft:air" {
                    game.pop_resource(target, d);
                }
            }
        }
    } else {
        let stage = (progress.clamp(1, 10) - 1) as u8;
        let at = game.world.game_time;
        game.destroying.insert(id, (target, stage, at));
    }
}

// ---- hooks ----

pub fn block_entity(block: &str, entity: &mut Option<(String, Compound)>) {
    if entity.is_none() && kind_of(block).is_some() {
        let mut c = Compound::new();
        set_state(&mut c, 0.0, NONE_HOPS, None);
        *entity = Some((BE_ID.to_string(), c));
    }
}

pub fn ticks(block: &str, ticks: &mut bool) {
    if kind_of(block).is_some() {
        *ticks = true;
    }
}

pub fn tick(game: &mut Game<'_>, pos: [i32; 3], entity: &mut ModBlockEntity) {
    if entity.id != BE_ID {
        return;
    }
    let Some(me) = node_at(game, pos) else { return };
    let old = get_f32(&entity.nbt, "speed");
    let out = if me.kind.is_source() {
        let target = source_target(game, pos, &me, &entity.nbt);
        put_f32(&mut entity.nbt, "target", target);
        let mut load = get_int(&entity.nbt, "load", 0);
        let mut cap = get_int(&entity.nbt, "cap", 0);
        let mut over = get_int(&entity.nbt, "over", 0) == 1;
        if target == 0.0 {
            over = false;
            load = 0;
            cap = 0;
        } else {
            let now = game.ticks as i64;
            let due = (now + i64::from(pos[0]) * 3 + i64::from(pos[2])).rem_euclid(10) == 0;
            if due || cap == 0 {
                let (l, c) = analyse(game, pos, &me, target);
                load = l;
                cap = c;
                let now_over = l > c;
                if now_over && !over {
                    let line = sect()
                        + "cCreate: the network at "
                        + &pos[0].to_string()
                        + " "
                        + &pos[1].to_string()
                        + " "
                        + &pos[2].to_string()
                        + " is overstressed ("
                        + &l.to_string()
                        + " / "
                        + &c.to_string()
                        + " SU)";
                    game.native_chat(&line);
                }
                over = now_over;
            }
        }
        put_int(&mut entity.nbt, "over", if over { 1 } else { 0 });
        Out {
            speed: if over { 0.0 } else { target },
            hops: 0,
            parent: None,
            load,
            cap,
        }
    } else {
        step(game, pos, &me, &entity.nbt)
    };
    set_state(&mut entity.nbt, out.speed, out.hops, out.parent);
    put_int(&mut entity.nbt, "load", out.load);
    put_int(&mut entity.nbt, "cap", out.cap);
    if me.kind == Kind::Drill {
        drill_tick(game, pos, &mut entity.nbt, &me, out.speed);
    }
    if me.kind == Kind::Motor {
        let rpm = motor_rpm(&entity.nbt);
        entity.data.clear();
        entity.data.push(rpm);
        entity.data.push(out.load.min(32767));
        entity.data.push(out.cap.min(32767));
    }
    if out.speed.to_bits() != old.to_bits() {
        game.native_block_entity_sync(pos);
    }
}

pub fn update_tag(entity: &ModBlockEntity, tag: &mut Option<Compound>) {
    if entity.id == BE_ID {
        let mut c = Compound::new();
        put_f32(&mut c, "speed", get_f32(&entity.nbt, "speed"));
        *tag = Some(c);
    }
}

// A click on a kinetic block tells the player its speed and the networks stress; a click on
// the hand crank also turns it for a few seconds.
pub fn right_click(game: &mut Game<'_>, pos: [i32; 3]) {
    let Some(me) = node_at(game, pos) else { return };
    if me.kind == Kind::Crank {
        let until = (game.ticks as i32).wrapping_add(CRANK_TICKS);
        let _ = game.native_with_mod_block_entity(pos, |e| {
            put_int(&mut e.nbt, "until", until);
        });
    }
    let info = game.native_with_mod_block_entity(pos, |e| {
        (get_f32(&e.nbt, "speed"), get_int(&e.nbt, "load", 0), get_int(&e.nbt, "cap", 0))
    });
    if let Some((speed, load, cap)) = info {
        let s = sect();
        let line = s.clone()
            + "7"
            + me.kind.label()
            + ": "
            + &s
            + "f"
            + &(speed.round() as i32).to_string()
            + " RPM "
            + &s
            + "7| stress "
            + &s
            + "f"
            + &load.to_string()
            + &s
            + "7 / "
            + &s
            + "f"
            + &cap.to_string()
            + " SU";
        game.native_chat(&line);
    }
}

// The block entity draws the moving part (`<block>_part`, never placed); the placed blocks
// own model is empty for shafts and cogs (the others keep their casing and only the moving
// part is drawn). A turning part is drawn about the blocks centre, with Creates own phase
// offset: it depends only on the coordinates across the axis, so blocks along one shaft line
// stay aligned and meshing neighbours sit half a tooth apart. The press head moves up and
// down instead.
pub fn submit(
    game: &Game<'_>,
    pos: [i32; 3],
    block: &str,
    state: BlockStateId,
    entity: &ModBlockEntity,
    draw: &mut BlockEntityDraw,
) {
    // (part, state property of the block, property of the part, 0 spin / 1 press / 2 wheel)
    let (part, prop, part_prop, anim) = match block {
        "create:shaft" => ("create:shaft_part", "axis", "axis", 0),
        "create:cogwheel" => ("create:cogwheel_part", "axis", "axis", 0),
        "create:large_cogwheel" => ("create:large_cogwheel_part", "axis", "axis", 0),
        "create:creative_motor" => ("create:motor_shaft_part", "facing", "facing", 0),
        "create:hand_crank" => ("create:hand_crank_handle_part", "facing", "facing", 0),
        "create:mechanical_drill" => ("create:drill_head_part", "facing", "facing", 0),
        "create:mechanical_press" => ("create:press_head_part", "facing", "facing", 1),
        "create:water_wheel" => ("create:water_wheel_wheel_part", "facing", "axis", 2),
        _ => return,
    };
    let Some(value) = game.native_block_state_property(state, prop) else { return };
    let value = value.to_string();
    let axis = if prop == "axis" {
        parse_axis(&value)
    } else {
        parse_dir(&value).map(axis_of)
    };
    let Some(axis) = axis else { return };
    let part_value = if anim == 2 { axis_char(axis).to_string() } else { value.clone() };
    let speed = f64::from(get_f32(&entity.nbt, "speed"));
    let t = draw.game_time as f64 + f64::from(draw.partial);
    let degrees = (speed * 0.3 * t) % 360.0;
    draw.push();
    if anim == 1 {
        let dy = -0.45 * (1.0 - degrees.to_radians().cos());
        draw.translate([0.0, dy as f32, 0.0]);
    } else {
        let mut about = [0.0f32; 3];
        about[axis] = 1.0;
        let across: i32 = (0..3).filter(|i| *i != axis).map(|i| pos[i]).sum();
        let mut offset = if block == "create:large_cogwheel" { 11.25 } else { 0.0 };
        if across % 2 == 0 {
            offset = 22.5;
        }
        draw.rotate_about(about, degrees as f32 + offset, [0.5, 0.5, 0.5]);
    }
    game.native_draw_block(draw, part, &[(part_prop, part_value.as_str())]);
    draw.pop();
    if speed == 0.0 {
        draw.keep();
    }
}

// The direction the placer is looking (`getNearestLookingDirection`).
pub fn nearest_looking(yaw: f32, pitch: f32) -> [i32; 3] {
    let (y, p) = (yaw.to_radians(), pitch.to_radians());
    let v = [-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos()];
    let mut best = 0;
    for i in 1..3 {
        if v[i].abs() > v[best].abs() {
            best = i;
        }
    }
    let mut d = [0; 3];
    d[best] = if v[best] >= 0.0 { 1 } else { -1 };
    d
}

fn node_from_state(level: &GameLevel<'_>, state: BlockStateId) -> Option<Node> {
    let name = level.facts.get(state).name.to_string();
    let kind = kind_of(&name)?;
    let props = &level.registry.state(state)?.properties;
    if kind.uses_facing() {
        let front = parse_dir(props.get("facing")?)?;
        Some(Node { kind, axis: axis_of(front), front })
    } else {
        let axis = parse_axis(props.get("axis")?)?;
        Some(Node { kind, axis, front: [0; 3] })
    }
}

// As Create does: a block lines up with neighbours whose shaft end faces it (a facing block
// turns so its shaft end points at one); with none, shafts and cogs take the axis you are
// looking along (a cog placed on a cog copies its axis), the motor and crank point their
// shaft at you, and the drill and press face away from you. Sneaking skips the neighbour rule.
pub fn placement(
    level: &GameLevel<'_>,
    state: BlockStateId,
    context: &PlacementContext,
    answer: &mut Option<Option<BlockStateId>>,
) {
    let name = level.facts.get(state).name.to_string();
    let Some(kind) = kind_of(&name) else { return };
    let at = context.pos;
    let look = nearest_looking(context.yaw, context.pitch);
    let mut feeding: Vec<[i32; 3]> = Vec::new();
    if !context.sneaking {
        for a in 0..3 {
            for s in [-1, 1] {
                let mut d = [0; 3];
                d[a] = s;
                let nb = BlockPos::new(at.x + d[0], at.y + d[1], at.z + d[2]);
                if let Some(n) = node_from_state(level, level.block_state(nb)) {
                    if n.has_end(neg(d)) {
                        feeding.push(d);
                    }
                }
            }
        }
    }
    let new = if kind.uses_facing() {
        // Directions where the neighbour can take a shaft end.
        let mut facing = match kind {
            Kind::Motor => neg(look),
            Kind::Crank => neg(look),
            Kind::Drill => look,
            Kind::Press => {
                let mut h = look;
                if h[1] != 0 {
                    h = [0, 0, 1];
                }
                h
            }
            _ => look,
        };
        if let Some(d) = feeding.first() {
            facing = match kind {
                Kind::Motor => *d,
                Kind::Crank | Kind::Drill => neg(*d),
                _ => facing,
            };
        }
        if kind == Kind::Wheel {
            facing = look;
        }
        level.native_state_with_property(state, "facing", dir_name(facing))
    } else {
        let mut axes: Vec<usize> = feeding.iter().map(|d| axis_of(*d)).collect();
        axes.dedup();
        let mut axis = if axes.len() == 1 { axes[0] } else { axis_of(look) };
        if axes.is_empty() && !context.sneaking {
            let clicked = BlockPos::new(context.hit_pos.x, context.hit_pos.y, context.hit_pos.z);
            if let Some(n) = node_from_state(level, level.block_state(clicked)) {
                if n.is_cog() && matches!(kind, Kind::Cogwheel | Kind::LargeCogwheel) {
                    axis = n.axis;
                }
            }
        }
        level.native_state_with_property(state, "axis", axis_char(axis))
    };
    if let Some(s) = new {
        *answer = Some(Some(s));
    }
}

// ---- the creative motors screen ----
// A one-slot menu (`data/create/horizon/menu/creative_motor.json`, its slot hidden) whose
// three data slots are the motors speed and its networks stress (asked for, available).
// The buttons send ids to the server, which sets the speed.

struct Btn {
    id: i32,
    label: &'static str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

const BUTTONS: [Btn; 9] = [
    Btn { id: 1, label: "-64", x: 11.0, y: 44.0, w: 24.0, h: 16.0 },
    Btn { id: 2, label: "-16", x: 37.0, y: 44.0, w: 24.0, h: 16.0 },
    Btn { id: 3, label: "-1", x: 63.0, y: 44.0, w: 24.0, h: 16.0 },
    Btn { id: 4, label: "+1", x: 89.0, y: 44.0, w: 24.0, h: 16.0 },
    Btn { id: 5, label: "+16", x: 115.0, y: 44.0, w: 24.0, h: 16.0 },
    Btn { id: 6, label: "+64", x: 141.0, y: 44.0, w: 24.0, h: 16.0 },
    Btn { id: 10, label: "Reverse", x: 11.0, y: 64.0, w: 50.0, h: 16.0 },
    Btn { id: 11, label: "Stop", x: 63.0, y: 64.0, w: 50.0, h: 16.0 },
    Btn { id: 12, label: "Max", x: 115.0, y: 64.0, w: 50.0, h: 16.0 },
];

// The button under `at` (GUI pixels from the screens top left).
fn hit(at: [f32; 2]) -> Option<i32> {
    BUTTONS
        .iter()
        .find(|b| at[0] >= b.x && at[0] < b.x + b.w && at[1] >= b.y && at[1] < b.y + b.h)
        .map(|b| b.id)
}

pub fn container(block: &str, container: &mut Option<ModContainer>) {
    if block == MOTOR_BLOCK {
        *container = Some(ModContainer {
            slots: 1,
            menu: Some(MOTOR_MENU.to_string()),
            ..Default::default()
        });
    }
}

pub fn screen(block: &str, screen: &mut Option<NativeScreen>) {
    if block == MOTOR_BLOCK {
        *screen = Some(NativeScreen {
            width: 176.0,
            height: 178.0,
            slots: vec![None],
            inventory_y: 96.0,
            inventory_x: 8.0,
            own_labels: false,
        });
    }
}

pub fn screen_draw(game: &Game<'_>, block: &str, data: &[i32], batch: &mut HudBatch, origin: [f32; 2]) {
    if block != MOTOR_BLOCK {
        return;
    }
    let rpm = data.first().copied().unwrap_or(MOTOR_DEFAULT_RPM);
    let load = data.get(1).copied().unwrap_or(0);
    let cap = data.get(2).copied().unwrap_or(0);
    game.native_text(batch, "Rotation Speed", [origin[0] + 8.0, origin[1] + 18.0], 0x404040, false);
    let value = rpm.to_string() + " RPM";
    let width = game.native_text_width(&value);
    game.native_text(batch, &value, [origin[0] + 168.0 - width, origin[1] + 18.0], 0x1F1F1F, false);
    let stress = "Stress: ".to_string() + &load.to_string() + " / " + &cap.to_string() + " SU";
    let color = if load > cap { 0xAA0000 } else { 0x404040 };
    game.native_text(batch, &stress, [origin[0] + 8.0, origin[1] + 30.0], color, false);
    let cursor = game.native_cursor();
    let over = hit([cursor[0] - origin[0], cursor[1] - origin[1]]);
    for b in BUTTONS.iter() {
        let face = if over == Some(b.id) { 0xFFA8A8A8 } else { 0xFF8B8B8B };
        game.native_fill(batch, [origin[0] + b.x, origin[1] + b.y, b.w, b.h], 0xFF373737);
        game.native_fill(batch, [origin[0] + b.x + 1.0, origin[1] + b.y + 1.0, b.w - 2.0, b.h - 2.0], face);
        let label_width = game.native_text_width(b.label);
        game.native_text(
            batch,
            b.label,
            [origin[0] + b.x + (b.w - label_width) / 2.0, origin[1] + b.y + 4.0],
            0xFFFFFF,
            true,
        );
    }
}

pub fn screen_click(block: &str, at: [f32; 2], button: &mut Option<i32>) {
    if block == MOTOR_BLOCK {
        if let Some(id) = hit(at) {
            *button = Some(id);
        }
    }
}

pub fn menu_button(game: &mut Game<'_>, pos: [i32; 3], entity: &mut ModBlockEntity, id: i32) {
    if entity.id != BE_ID || game.native_block(pos) != MOTOR_BLOCK {
        return;
    }
    let cur = motor_rpm(&entity.nbt);
    let next = match id {
        1 => cur - 64,
        2 => cur - 16,
        3 => cur - 1,
        4 => cur + 1,
        5 => cur + 16,
        6 => cur + 64,
        10 => -cur,
        11 => 0,
        12 => MOTOR_MAX_RPM,
        _ => return,
    };
    let next = next.clamp(-MOTOR_MAX_RPM, MOTOR_MAX_RPM);
    put_int(&mut entity.nbt, "rpm", next);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: Kind, axis: usize) -> Node {
        Node { kind, axis, front: [0; 3] }
    }
    fn cog(axis: usize) -> Node {
        node(Kind::Cogwheel, axis)
    }
    fn shaft(axis: usize) -> Node {
        node(Kind::Shaft, axis)
    }

    #[test]
    fn shafts_chain_along_their_axis_only() {
        assert_eq!(link(&shaft(0), [1, 0, 0], &shaft(0)), Some(1.0));
        assert_eq!(link(&shaft(0), [0, 1, 0], &shaft(0)), None);
        assert_eq!(link(&shaft(0), [1, 0, 0], &shaft(1)), None);
    }

    #[test]
    fn small_cogs_mesh_in_plane_and_reverse() {
        assert_eq!(link(&cog(1), [1, 0, 0], &cog(1)), Some(-1.0));
        assert_eq!(link(&cog(1), [0, 1, 0], &cog(1)), Some(1.0)); // stacked on the shaft
        assert_eq!(link(&cog(1), [1, 0, 0], &cog(0)), None);
        assert_eq!(link(&cog(1), [1, 0, 1], &cog(1)), None); // diagonal small cogs do not mesh
    }

    #[test]
    fn small_and_large_mesh_diagonally_two_to_one() {
        let large = node(Kind::LargeCogwheel, 1);
        assert_eq!(link(&cog(1), [1, 0, 1], &large), Some(-2.0));
        assert_eq!(link(&large, [-1, 0, -1], &cog(1)), Some(-0.5));
        assert_eq!(link(&cog(1), [1, 0, 0], &large), None); // side by side does not mesh
    }

    #[test]
    fn large_cogs_mesh_at_right_angles() {
        let up = node(Kind::LargeCogwheel, 1);
        let east = node(Kind::LargeCogwheel, 0);
        assert_eq!(link(&up, [1, 1, 0], &east), Some(1.0));
        assert_eq!(link(&up, [-1, 1, 0], &east), Some(-1.0));
        assert_eq!(link(&up, [1, 0, 1], &up), None); // same axis: no
    }

    #[test]
    fn motor_feeds_the_shaft_in_front_only() {
        let motor = Node { kind: Kind::Motor, axis: 0, front: [1, 0, 0] };
        assert_eq!(link(&shaft(0), [-1, 0, 0], &motor), Some(1.0));
        assert_eq!(link(&shaft(0), [1, 0, 0], &motor), None);
    }

    #[test]
    fn crank_and_drill_take_a_shaft_at_their_back() {
        let crank = Node { kind: Kind::Crank, axis: 1, front: [0, 1, 0] };
        // a crank facing up has its shaft end down: a shaft below it joins, one above does not
        assert_eq!(link(&shaft(1), [0, 1, 0], &crank), Some(1.0)); // crank above the shaft
        assert_eq!(link(&shaft(1), [0, -1, 0], &crank), None); // crank below the shaft
        assert_eq!(link(&crank, [0, -1, 0], &shaft(1)), Some(1.0));
        let drill = Node { kind: Kind::Drill, axis: 2, front: [0, 0, -1] };
        assert_eq!(link(&drill, [0, 0, 1], &shaft(2)), Some(1.0));
        assert_eq!(link(&drill, [0, 0, -1], &shaft(2)), None);
    }

    #[test]
    fn press_takes_a_shaft_across_its_facing() {
        let press = Node { kind: Kind::Press, axis: 2, front: [0, 0, -1] };
        assert_eq!(link(&press, [1, 0, 0], &shaft(0)), Some(1.0));
        assert_eq!(link(&press, [-1, 0, 0], &shaft(0)), Some(1.0));
        assert_eq!(link(&press, [0, 0, 1], &shaft(2)), None);
    }

    #[test]
    fn stress_units() {
        assert_eq!(Kind::Crank.capacity(32.0), 256);
        assert_eq!(Kind::Wheel.capacity(8.0), 128);
        assert_eq!(Kind::Motor.capacity(1.0), 16384);
        assert_eq!(Kind::Press.impact() * 32, 256);
    }

    #[test]
    fn buttons_are_found_where_they_are_drawn() {
        assert_eq!(hit([12.0, 46.0]), Some(1));
        assert_eq!(hit([150.0, 50.0]), Some(6));
        assert_eq!(hit([120.0, 70.0]), Some(12));
        assert_eq!(hit([0.0, 0.0]), None);
    }

    #[test]
    fn looking_direction_follows_yaw_and_pitch() {
        assert_eq!(nearest_looking(0.0, 0.0), [0, 0, 1]);
        assert_eq!(nearest_looking(-90.0, 0.0), [1, 0, 0]);
        assert_eq!(nearest_looking(0.0, 90.0), [0, -1, 0]);
    }
}
