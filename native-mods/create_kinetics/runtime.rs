// Create kinetics and fluids: shafts, cogwheels, large cogwheels, power sources (creative motor, hand
// crank, water wheel), consumers (mechanical drill, mechanical press) and stress.
// Injected as `crate::game::create_kinetics`. Each kinetic block has a block entity holding
// its speed (rpm, signed). Speed spreads one block per tick from sources along a chain of
// "parents"; a block that loses its parent clears itself, so removing a source winds the
// network down without stale speeds circulating in loops. Every few ticks each source walks
// its whole network: if the consumers stress (impact x rpm) beats the sources capacity the
// sources stop until the load is cut.
use super::native_block_entity_draw::BlockEntityDraw;
use super::native_hooks::{NativeScreen, NativeScreenView};
use super::{Game, GameLevel};
use horizon_client::containers::{ModBlockEntity, ModContainer};
use horizon_client::hud::HudBatch;
use horizon_assets::item_model::DisplayContext;
use horizon_client::items::ItemStack;
use horizon_core::position::BlockPos;
use horizon_world::level::{LevelAccess, PlacementContext};
use horizon_world::nbt::{Compound, JavaString, Tag};
use horizon_world::registry::BlockStateId;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const BE_ID: &str = "create:kinetic";
const NONE_HOPS: i32 = 255;
const MAX_HOPS: i32 = 128;
const MOTOR_DEFAULT_RPM: i32 = 16;
const MOTOR_MAX_RPM: i32 = 256;
const MOTOR_BLOCK: &str = "create:creative_motor";
const MOTOR_MENU: &str = "create:creative_motor";
const MOTOR_CAPACITY: i32 = 16384;
const CRANK_RPM: f32 = 32.0;
const CRANK_TICKS: i32 = 12;
const WHEEL_RPM: f32 = 8.0;
const NET_LIMIT: usize = 2048;
const DEPOT_BLOCK: &str = "create:depot";
const DEPOT_BE: &str = "create:depot_be";
const HANDLE_RPM: f32 = 16.0;

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
    Gearbox,
    Clutch,
    Gearshift,
    Chain,
    AdjChain,
    Belt,
    Pump,
    Valve,
    Handle,
    Millstone,
    Saw,
    Crusher,
    Crafter,
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
        "create:gearbox" | "create:vertical_gearbox" => Some(Kind::Gearbox),
        "create:clutch" => Some(Kind::Clutch),
        "create:gearshift" => Some(Kind::Gearshift),
        "create:encased_chain_drive" => Some(Kind::Chain),
        "create:adjustable_chain_gearshift" => Some(Kind::AdjChain),
        "create:belt" => Some(Kind::Belt),
        "create:mechanical_pump" => Some(Kind::Pump),
        "create:fluid_valve" => Some(Kind::Valve),
        "create:copper_valve_handle" => Some(Kind::Handle),
        "create:millstone" => Some(Kind::Millstone),
        "create:mechanical_saw" => Some(Kind::Saw),
        "create:crushing_wheel" => Some(Kind::Crusher),
        "create:mechanical_crafter" => Some(Kind::Crafter),
        _ => None,
    }
}

impl Kind {
    fn uses_facing(self) -> bool {
        matches!(self, Kind::Motor | Kind::Crank | Kind::Wheel | Kind::Drill | Kind::Press | Kind::Belt | Kind::Pump | Kind::Handle | Kind::Saw | Kind::Crafter)
    }
    fn is_source(self) -> bool {
        matches!(self, Kind::Motor | Kind::Crank | Kind::Wheel | Kind::Handle)
    }
    // Stress units per rpm a consumer asks for.
    fn impact(self) -> i32 {
        match self {
            Kind::Drill => 4,
            Kind::Press => 8,
            Kind::Pump => 4,
            Kind::Millstone => 4,
            Kind::Saw => 4,
            Kind::Crusher => 8,
            Kind::Crafter => 2,
            _ => 0,
        }
    }
    // Stress units a source gives while turning at `rpm`.
    fn capacity(self, rpm: f32) -> i32 {
        match self {
            Kind::Motor => MOTOR_CAPACITY,
            Kind::Crank | Kind::Handle => (8.0 * rpm.abs()) as i32,
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
            Kind::Gearbox => "Gearbox",
            Kind::Clutch => "Clutch",
            Kind::Gearshift => "Gearshift",
            Kind::Chain => "Encased Chain Drive",
            Kind::AdjChain => "Adjustable Chain Gearshift",
            Kind::Belt => "Belt",
            Kind::Pump => "Mechanical Pump",
            Kind::Valve => "Fluid Valve",
            Kind::Handle => "Valve Handle",
            Kind::Millstone => "Millstone",
            Kind::Saw => "Mechanical Saw",
            Kind::Crusher => "Crushing Wheel",
            Kind::Crafter => "Mechanical Crafter",
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
    // Direction to the block feeding a gearbox (its source facing), zero if none.
    pub src: [i32; 3],
    // Chain drives: whether the chain runs along the first coordinate across the axis.
    pub first: bool,
    // Belts: whether this piece (a start, end or pulley) has a shaft across the belt.
    pub shaft: bool,
    // Clutches and gearshifts: whether a redstone signal reaches the block.
    pub powered: bool,
}

impl Node {
    fn new(kind: Kind, axis: usize, front: [i32; 3]) -> Node {
        Node { kind, axis, front, src: [0; 3], first: false, shaft: false, powered: false }
    }
    fn is_chain(&self) -> bool {
        matches!(self.kind, Kind::Chain | Kind::AdjChain)
    }
    // The axis a chain drive links along (across its own axis).
    fn chain_axis(&self) -> usize {
        let mut across = (0..3).filter(|a| *a != self.axis);
        let low = across.next().unwrap_or(0);
        let high = across.next().unwrap_or(0);
        if self.first {
            low
        } else {
            high
        }
    }
    fn is_cog(&self) -> bool {
        matches!(
            self.kind,
            Kind::Cogwheel | Kind::LargeCogwheel | Kind::Pump | Kind::Crusher | Kind::Millstone | Kind::Crafter
        )
    }
    // The axis the saw shaft runs along: across its facing, the lower axis when first is set.
    fn saw_axis(&self) -> usize {
        let f = axis_of(self.front);
        let mut across = (0..3).filter(|a| *a != f);
        let low = across.next().unwrap_or(0);
        let high = across.next().unwrap_or(0);
        if self.first {
            low
        } else {
            high
        }
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
            Kind::Crank | Kind::Drill | Kind::Handle => d == neg(self.front),
            Kind::Gearbox => unit && d[self.axis] == 0,
            Kind::Pump => false,
            Kind::Crafter => false,
            Kind::Millstone => d == [0, -1, 0],
            Kind::Saw => unit && d[self.saw_axis()] != 0,
            Kind::Clutch if self.powered => false,
            Kind::Belt => self.shaft && unit && d[self.shaft_axis()] != 0,
            _ => unit && d[self.axis] != 0,
        }
    }
}

// The mechanical pump is a small cogwheel turning on its facing axis (it has no shaft ends).
fn cog_kind(k: Kind) -> Kind {
    if k == Kind::Pump || k == Kind::Millstone || k == Kind::Crafter {
        Kind::Cogwheel
    } else if k == Kind::Crusher {
        Kind::LargeCogwheel
    } else {
        k
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

// How a gearbox turns the output facing `dir` compared with its input facing `src`
// (Create: straight through reverses, a side output reverses when it shares the input sign).
fn gearbox_mod(src: [i32; 3], dir: [i32; 3]) -> f32 {
    if src == [0; 3] {
        return 1.0;
    }
    if axis_of(dir) == axis_of(src) {
        if dir == src {
            1.0
        } else {
            -1.0
        }
    } else if dir.iter().sum::<i32>() == src.iter().sum::<i32>() {
        -1.0
    } else {
        1.0
    }
}

// A powered gearshift turns what it hands on (away from its input) the other way.
fn shift_mod(node: &Node, dir: [i32; 3]) -> f32 {
    if node.kind == Kind::Gearshift && node.powered && node.src != [0; 3] && dir != node.src {
        -1.0
    } else {
        1.0
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
        let f = if nb.kind == Kind::Gearbox {
            gearbox_mod(nb.src, neg(d))
        } else {
            shift_mod(nb, neg(d))
        };
        return Some(f);
    }
    if me.is_chain() && nb.is_chain() && me.axis == nb.axis && me.first == nb.first {
        let line = me.chain_axis();
        if d[line].abs() == 1 && d.iter().filter(|c| **c != 0).count() == 1 {
            return Some(1.0);
        }
    }
    if me.kind == Kind::Belt && nb.kind == Kind::Belt && me.front == nb.front {
        let a = axis_of(me.front);
        if d[a].abs() == 1 && d.iter().filter(|c| **c != 0).count() == 1 {
            return Some(1.0);
        }
    }
    if !me.is_cog() || !nb.is_cog() {
        return None;
    }
    let nonzero = d.iter().filter(|c| **c != 0).count();
    match (cog_kind(me.kind), cog_kind(nb.kind)) {
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
            if cog_kind(me.kind) == Kind::Cogwheel {
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
    if me.is_chain() {
        for sgn in [-1, 1] {
            let mut d = [0; 3];
            d[me.chain_axis()] = sgn;
            out.push(d);
        }
    }
    if me.kind == Kind::Belt {
        for sgn in [-1, 1] {
            let mut d = [0; 3];
            d[axis_of(me.front)] = sgn;
            out.push(d);
        }
    }
    if !me.is_cog() {
        return out;
    }
    let (p, q) = ((me.axis + 1) % 3, (me.axis + 2) % 3);
    if cog_kind(me.kind) == Kind::Cogwheel {
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
    if cog_kind(me.kind) == Kind::LargeCogwheel {
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
    if kind == Kind::Millstone {
        return Some(Node::new(kind, 1, [0; 3]));
    }
    if kind == Kind::Valve {
        let front = parse_dir(&game.native_block_property(p, "facing")?)?;
        let first = game.native_block_property(p, "axis_along_first").map(|v| v == "true").unwrap_or(false);
        return Some(Node::new(kind, valve_axis(axis_of(front), first), front));
    }
    if kind.uses_facing() {
        let front = parse_dir(&game.native_block_property(p, "facing")?)?;
        let mut node = Node::new(kind, axis_of(front), front);
        if kind == Kind::Belt {
            node.shaft = game.native_block_property(p, "part").map(|v| v != "middle").unwrap_or(false);
        }
        if kind == Kind::Saw {
            node.first = game.native_block_property(p, "axis_along_first").map(|v| v == "true").unwrap_or(false);
        }
        Some(node)
    } else {
        let axis = parse_axis(&game.native_block_property(p, "axis")?)?;
        let mut node = Node::new(kind, axis, [0; 3]);
        if node.is_chain() {
            node.first = game.native_block_property(p, "axis_along_first").map(|v| v == "true").unwrap_or(false);
        }
        if matches!(kind, Kind::Clutch | Kind::Gearshift) {
            node.powered = game.native_block_property(p, "powered").map(|v| v == "true").unwrap_or(false);
        }
        Some(node)
    }
}

#[derive(Clone, Copy)]
struct Reading {
    speed: f32,
    hops: i32,
    load: i32,
    cap: i32,
    target: f32,
    parent: Option<[i32; 3]>,
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
                parent: get_parent(&e.nbt),
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

// Follows the parent links from the block at `pos` (whose parent is at offset `d`) up to a
// source that is turning. A broken link, a loop or a stopped source means the block should stop.
fn chain_ok(game: &mut Game<'_>, pos: [i32; 3], d: [i32; 3]) -> bool {
    let mut at = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
    for _ in 0..=MAX_HOPS {
        let Some(r) = read_neighbour(game, at) else { return false };
        if r.speed == 0.0 {
            return false;
        }
        match r.parent {
            None => return r.hops == 0,
            Some(p) => at = [at[0] + p[0], at[1] + p[1], at[2] + p[2]],
        }
    }
    false
}

// One tick of a block that is not a source.
fn step(game: &mut Game<'_>, pos: [i32; 3], me: &Node, nbt: &Compound) -> Out {
    let speed = get_f32(nbt, "speed");
    let hops = get_hops(nbt);
    let at = |d: [i32; 3]| [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
    if let Some(d) = get_parent(nbt) {
        // Keep a parent that still feeds us from closer to a source; else wind down.
        let p = at(d);
        if let (Some(mut nb), Some(r)) = (node_at(game, p), read_neighbour(game, p)) {
            nb.src = r.parent.unwrap_or([0; 3]);
            if let Some(f) = link(me, d, &nb) {
                let due = (game.ticks as i64 + i64::from(pos[0]) + i64::from(pos[1]) * 2 + i64::from(pos[2]) * 3).rem_euclid(3) == 0;
                if r.speed != 0.0 && r.hops < hops && r.hops + 1 <= MAX_HOPS && (!due || chain_ok(game, pos, d)) {
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
        let Some(mut nb) = node_at(game, p) else { continue };
        let Some(r) = read_neighbour(game, p) else { continue };
        nb.src = r.parent.unwrap_or([0; 3]);
        let Some(f) = link(me, d, &nb) else { continue };
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

// ---- the depot: one stack lying on top, kept in the block entity ----

fn get_text(n: &Compound, k: &str) -> String {
    match n.get(&JavaString::from(k)) {
        Some(Tag::String(v)) => String::from_utf16_lossy(&v.0),
        _ => String::new(),
    }
}

fn depot_content(n: &Compound) -> Option<(String, u32)> {
    let item = get_text(n, "item");
    let count = get_int(n, "count", 0);
    if item.is_empty() || count <= 0 {
        None
    } else {
        Some((item, count as u32))
    }
}

fn depot_put(n: &mut Compound, content: Option<(String, u32)>) {
    match content {
        Some((item, count)) => {
            n.insert(JavaString::from("item"), Tag::String(JavaString::from(item.as_str())));
            put_int(n, "count", count as i32);
        }
        None => {
            n.remove(&JavaString::from("item"));
            n.remove(&JavaString::from("count"));
        }
    }
}

// What the depot at `pos` holds, if there is a depot there.
fn depot_at(game: &mut Game<'_>, pos: [i32; 3]) -> Option<Option<(String, u32)>> {
    if game.native_block(pos) != DEPOT_BLOCK {
        return None;
    }
    game.native_with_mod_block_entity(pos, |e| depot_content(&e.nbt))
}

fn depot_set(game: &mut Game<'_>, pos: [i32; 3], content: Option<(String, u32)>) {
    let _ = game.native_with_mod_block_entity(pos, |e| depot_put(&mut e.nbt, content));
    game.native_block_entity_sync(pos);
}

// ---- redstone: a clutch lets go while powered, a gearshift turns the other way, a press presses without items ----

fn redstone_sync(game: &mut Game<'_>, pos: [i32; 3]) {
    let dim = game.dim.id().to_string();
    let on = game.native_neighbor_signal(&dim, pos) > 0;
    let want = if on { "true" } else { "false" };
    let same = game.native_block_property(pos, "powered").map(|v| v == want).unwrap_or(true);
    if !same {
        game.native_set_block_property(pos, "powered", want);
    }
}

// ---- chain drives: the piece shown follows the neighbours in the chain ----
// A chain runs along one axis across the shaft. The end piece has its neighbour on the negative
// side, the start piece on the positive side, the middle on both.

fn chain_part(game: &mut Game<'_>, pos: [i32; 3], me: &Node) {
    let line = me.chain_axis();
    let mut has = [false, false];
    for (i, sgn) in [-1, 1].iter().enumerate() {
        let mut p = pos;
        p[line] += *sgn;
        if let Some(n) = node_at(game, p) {
            if n.is_chain() && n.axis == me.axis && n.first == me.first {
                has[i] = true;
            }
        }
    }
    let want = match (has[0], has[1]) {
        (true, true) => "middle",
        (true, false) => "end",
        (false, true) => "start",
        _ => "none",
    };
    if want == "none" {
        for a in 0..3 {
            if a == me.axis {
                continue;
            }
            for sgn in [-1, 1] {
                let mut p = pos;
                p[a] += sgn;
                if let Some(n) = node_at(game, p) {
                    if n.is_chain() && n.axis == me.axis && n.chain_axis() == a && n.first != me.first {
                        game.native_set_block_property(pos, "axis_along_first", if n.first { "true" } else { "false" });
                        return;
                    }
                }
            }
        }
    }
    let same = game.native_block_property(pos, "part").map(|v| v == want).unwrap_or(true);
    if !same {
        game.native_set_block_property(pos, "part", want);
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
        Kind::Handle => {
            let now = game.ticks as i32;
            if get_int(nbt, "until", now.wrapping_sub(1)).wrapping_sub(now) > 0 {
                if get_int(nbt, "hdir", 1) >= 0 {
                    HANDLE_RPM
                } else {
                    -HANDLE_RPM
                }
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
            let Some(mut nb) = node_at(game, q) else { continue };
            let Some(f) = link(&nb, neg(d), &node) else { continue };
            nb.src = neg(d);
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

// ---- the mechanical press ----
// A turning press lowers its head once per turn of its shaft. When the head is at the bottom it
// presses every item lying in the two blocks under the press (put a block two below it and drop
// the items on it). The recipes are Create pressing recipes.

fn pressing(item: &str) -> Option<&'static str> {
    match item {
        "minecraft:iron_ingot" => Some("create:iron_sheet"),
        "minecraft:copper_ingot" => Some("create:copper_sheet"),
        "minecraft:gold_ingot" => Some("create:golden_sheet"),
        "create:brass_ingot" => Some("create:brass_sheet"),
        "create:pulp" => Some("create:cardboard"),
        "minecraft:sugar_cane" => Some("minecraft:paper"),
        _ => None,
    }
}

// Whether something pressable lies under the press: on a depot one or two blocks down, or on the ground.
fn pressable_below(game: &mut Game<'_>, pos: [i32; 3]) -> bool {
    for below in [1, 2] {
        let p = [pos[0], pos[1] - below, pos[2]];
        if let Some(Some((item, _))) = depot_at(game, p) {
            if pressing(&item).is_some() {
                return true;
            }
        }
    }
    for below in [1, 2] {
        let p = [pos[0], pos[1] - below, pos[2]];
        if let Some(items) = belt_items_at(game, p) {
            if items.iter().any(|b| pressing(&b.item).is_some()) {
                return true;
            }
        }
    }
    let dim = game.dim.id().to_string();
    let (x, y, z) = (f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
    let found = game.native_item_entities(&dim, [x, y - 2.0, z], [x + 1.0, y, z + 1.0]);
    found.iter().any(|e| pressing(&e.stack.item).is_some())
}

// The head hits the bottom: press what lies under it. Whatever sat on a depot stays on it.
fn press_now(game: &mut Game<'_>, pos: [i32; 3]) {
    for below in [1, 2] {
        let p = [pos[0], pos[1] - below, pos[2]];
        if game.native_block(p) == "create:belt" {
            let changed = game
                .native_with_mod_block_entity(p, |e| {
                    let mut v = belt_items(&e.nbt);
                    let mut any = false;
                    for b in v.iter_mut() {
                        if let Some(r) = pressing(&b.item) {
                            b.item = r.to_string();
                            any = true;
                        }
                    }
                    if any {
                        belt_store(&mut e.nbt, &v);
                    }
                    any
                })
                .unwrap_or(false);
            if changed {
                game.native_block_entity_sync(p);
            }
        }
        if let Some(Some((item, count))) = depot_at(game, p) {
            if let Some(result) = pressing(&item) {
                depot_set(game, p, Some((result.to_string(), count)));
            }
        }
    }
    let dim = game.dim.id().to_string();
    let (x, y, z) = (f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
    let found = game.native_item_entities(&dim, [x, y - 2.0, z], [x + 1.0, y, z + 1.0]);
    for e in found {
        let Some(result) = pressing(&e.stack.item) else { continue };
        let count = e.stack.count;
        let mut gone = e.stack.clone();
        gone.count = 0;
        if game.native_set_item_entity(&dim, e.id, gone) {
            let made = ItemStack { item: Arc::from(result), count, damage: 0, extra: None };
            game.spawn_item(made, e.position, [0.0, 0.1, 0.0], 10);
        }
    }
}

// One tick of the press: a cycle (head down, head up) runs only while there is something to press,
// at the pace of the shaft. The phase (degrees, 0 = head up, 180 = at the bottom) is kept in the
// block entity and sent to the clients that draw the head. True when it changed.
fn press_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, speed: f32) -> bool {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return false;
    }
    let rpm = speed.abs();
    let phase = get_f32(nbt, "pp");
    if rpm < 1.0 {
        return false;
    }
    if phase == 0.0 {
        let dim = game.dim.id().to_string();
        let signal = game.native_neighbor_signal(&dim, pos) > 0;
        if !signal && !pressable_below(game, pos) {
            return false;
        }
    }
    let mut next = phase + rpm * 0.3;
    if phase < 180.0 && next >= 180.0 {
        press_now(game, pos);
    }
    if next >= 360.0 {
        next = 0.0;
    }
    put_f32(nbt, "pp", next);
    true
}

// ---- belts ----
// A belt is a row of belt pieces (start, middle..., end) made by the belt connector item between two
// shafts. Turning, it carries the items lying on it along its facing (the other way when the speed
// is negative) and hands the last one over to a free depot straight ahead.

static PENDING: Mutex<Option<[i32; 3]>> = Mutex::new(None);

fn is_air(name: &str) -> bool {
    name == "minecraft:air" || name == "minecraft:cave_air" || name == "minecraft:void_air"
}

const BELT_GAP: f32 = 0.25;
const BELT_MAX: usize = 4;
const BELT_TOP: f32 = 0.83;

// An item stack riding a belt piece: where it is along the piece (0 at the back end, 1 at the front end).
#[derive(Clone)]
struct BeltItem {
    item: String,
    count: u32,
    p: f32,
}

// The piece keeps its items as one text: item|count|where;item|count|where...
fn belt_items(n: &Compound) -> Vec<BeltItem> {
    let text = get_text(n, "bi");
    let mut out = Vec::new();
    for part in text.split(';') {
        let mut f = part.splitn(3, '|');
        let (Some(item), Some(count), Some(p)) = (f.next(), f.next(), f.next()) else { continue };
        let (Ok(count), Ok(p)) = (count.parse::<u32>(), p.parse::<f32>()) else { continue };
        if !item.is_empty() && count > 0 {
            out.push(BeltItem { item: item.to_string(), count, p });
        }
    }
    out
}

fn belt_store(n: &mut Compound, items: &[BeltItem]) {
    let mut text = String::new();
    for (i, b) in items.iter().enumerate() {
        if i > 0 {
            text.push(';');
        }
        text.push_str(&b.item);
        text.push('|');
        text.push_str(&b.count.to_string());
        text.push('|');
        text.push_str(&b.p.to_string());
    }
    n.insert(JavaString::from("bi"), Tag::String(JavaString::from(text.as_str())));
}

fn belt_items_at(game: &mut Game<'_>, p: [i32; 3]) -> Option<Vec<BeltItem>> {
    if game.native_block(p) != "create:belt" {
        return None;
    }
    game.native_with_mod_block_entity(p, |e| belt_items(&e.nbt))
}

// Where on the piece a free spot is for a new item, nearest the middle first.
fn belt_free_spot(items: &[BeltItem]) -> Option<f32> {
    if items.len() >= BELT_MAX {
        return None;
    }
    [0.5, 0.25, 0.75, 0.05, 0.95]
        .iter()
        .copied()
        .find(|c| items.iter().all(|b| (b.p - *c).abs() >= BELT_GAP))
}

// Hands an item that reached the end of the piece to what lies ahead: the next piece, a free depot, or the air.
fn belt_hand_over(game: &mut Game<'_>, pos: [i32; 3], me: &Node, sign: f32, item: &BeltItem) -> bool {
    let line = axis_of(me.front);
    let mut next = pos;
    next[line] += if sign > 0.0 { me.front[line] } else { -me.front[line] };
    if let Some(n) = node_at(game, next) {
        if n.kind == Kind::Belt && n.front == me.front {
            let entry = if sign > 0.0 { 0.0 } else { 1.0 };
            let carried = item.clone();
            let ok = game
                .native_with_mod_block_entity(next, move |e| {
                    let mut v = belt_items(&e.nbt);
                    if v.len() >= BELT_MAX || v.iter().any(|b| (b.p - entry).abs() < BELT_GAP) {
                        return false;
                    }
                    v.push(BeltItem { p: entry, ..carried });
                    belt_store(&mut e.nbt, &v);
                    true
                })
                .unwrap_or(false);
            if ok {
                game.native_block_entity_sync(next);
            }
            return ok;
        }
    }
    match depot_at(game, next) {
        Some(None) => {
            depot_set(game, next, Some((item.item.clone(), item.count)));
            return true;
        }
        Some(Some(_)) => return false,
        None => {}
    }
    let s = f64::from(sign);
    let edge = [
        f64::from(pos[0]) + 0.5 + f64::from(me.front[0]) * s * 0.6,
        f64::from(pos[1]) + 0.9,
        f64::from(pos[2]) + 0.5 + f64::from(me.front[2]) * s * 0.6,
    ];
    let push = [f64::from(me.front[0]) * s * 0.1, 0.05, f64::from(me.front[2]) * s * 0.1];
    game.spawn_item(stack_of(&item.item, item.count), edge, push, 20);
    true
}

// One tick of a belt piece: items lying on it are taken in, the ones it carries move on at the speed of
// the belt (a gap apart) and pass to the next piece, a free depot, or fall off the end. True when it changed.
fn belt_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, me: &Node, speed: f32) -> bool {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return false;
    }
    let mut items = belt_items(nbt);
    let before = items.len();
    let mut changed = false;
    let line = axis_of(me.front);
    let dim = game.dim.id().to_string();
    let here = [f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2])];
    if items.len() < BELT_MAX {
        let found = game.native_item_entities(&dim, [here[0], here[1] + 0.5, here[2]], [here[0] + 1.0, here[1] + 1.9, here[2] + 1.0]);
        for e in found {
            if items.len() >= BELT_MAX {
                break;
            }
            if e.stack.count == 0 {
                continue;
            }
            let along = (e.position[line] - here[line]) as f32;
            let p = if me.front[line] > 0 { along } else { 1.0 - along };
            let Some(spot) = belt_free_spot(&items) else { break };
            let spot = if items.iter().all(|b| (b.p - p).abs() >= BELT_GAP) { p.clamp(0.0, 1.0) } else { spot };
            let mut gone = e.stack.clone();
            gone.count = 0;
            if game.native_set_item_entity(&dim, e.id, gone) {
                items.push(BeltItem { item: e.stack.item.to_string(), count: e.stack.count, p: spot });
                changed = true;
            }
        }
    }
    if speed.abs() >= 1.0 && !items.is_empty() {
        let sign = if speed > 0.0 { 1.0f32 } else { -1.0 };
        let step = speed.abs() / 480.0 * sign;
        items.sort_by(|a, b| (b.p * sign).partial_cmp(&(a.p * sign)).unwrap_or(std::cmp::Ordering::Equal));
        let mut ahead: Option<f32> = None;
        let mut kept: Vec<BeltItem> = Vec::new();
        for mut b in items {
            let mut np = b.p + step;
            if let Some(a) = ahead {
                let limit = a - BELT_GAP * sign;
                if sign > 0.0 && np > limit {
                    np = limit.max(b.p);
                }
                if sign < 0.0 && np < limit {
                    np = limit.min(b.p);
                }
            }
            let exited = if sign > 0.0 { np >= 1.0 } else { np <= 0.0 };
            if exited {
                if belt_hand_over(game, pos, me, sign, &b) {
                    changed = true;
                    continue;
                }
                np = if sign > 0.0 { 0.999 } else { 0.001 };
            }
            if (np - b.p).abs() > 1e-6 {
                changed = true;
            }
            b.p = np;
            ahead = Some(np);
            kept.push(b);
        }
        items = kept;
    }
    belt_store(nbt, &items);
    changed || items.len() != before
}

// A broken or taken-apart piece drops what it carried.
fn belt_spill(game: &mut Game<'_>, p: [i32; 3]) {
    let Some(items) = belt_items_at(game, p) else { return };
    let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.9, f64::from(p[2]) + 0.5];
    for b in items {
        game.spawn_item(stack_of(&b.item, b.count), at, [0.0, 0.1, 0.0], 10);
    }
    let _ = game.native_with_mod_block_entity(p, |e| {
        e.nbt.remove(&JavaString::from("bi"));
    });
}

fn say(game: &mut Game<'_>, text: &str) {
    game.native_chat(text);
}

// Takes the belt through `at` apart: the end pieces turn back into shafts, the middle ones vanish, and a
// belt connector drops. `skip` is a piece that is being broken anyway.
fn belt_dismantle(game: &mut Game<'_>, at: [i32; 3], skip: Option<[i32; 3]>) {
    let Some(me) = node_at(game, at) else { return };
    if me.kind != Kind::Belt {
        return;
    }
    let line = axis_of(me.front);
    let mut pieces: Vec<[i32; 3]> = vec![at];
    for sgn in [-1, 1] {
        let mut p = at;
        for _ in 0..40 {
            p[line] += sgn;
            match node_at(game, p) {
                Some(n) if n.kind == Kind::Belt && n.front == me.front => pieces.push(p),
                _ => break,
            }
        }
    }
    let shaft_axis = if line == 0 { 2 } else { 0 };
    let dim = game.dim.id().to_string();
    let shaft = game.pack.assets.registry.default_state("create:shaft");
    for p in pieces {
        if Some(p) == skip {
            continue;
        }
        belt_spill(game, p);
        let part = game.native_block_property(p, "part").map(|v| v.to_string()).unwrap_or_default();
        if part != "middle" {
            if let Some(base) = shaft {
                game.native_set_block(&dim, p, base, 3);
                game.native_set_block_property(p, "axis", axis_char(shaft_axis));
            }
        } else {
            game.native_set_block(&dim, p, BlockStateId(0), 3);
        }
    }
    let drop = [f64::from(at[0]) + 0.5, f64::from(at[1]) + 1.0, f64::from(at[2]) + 0.5];
    game.spawn_item(stack_of("create:belt_connector", 1), drop, [0.0, 0.1, 0.0], 10);
}

// A belt piece checks its neighbours now and then: a start needs a piece ahead, an end one behind, a middle
// both. If one is gone the belt comes apart.
fn belt_validate(game: &mut Game<'_>, pos: [i32; 3], me: &Node) {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return;
    }
    let part = game.native_block_property(pos, "part").map(|v| v.to_string()).unwrap_or_default();
    let line = axis_of(me.front);
    let need_ahead = part != "end";
    let need_behind = part != "start";
    let mut broken = false;
    for (need, sgn) in [(need_ahead, 1), (need_behind, -1)] {
        if !need {
            continue;
        }
        let mut p = pos;
        p[line] += sgn * me.front[line];
        let ok = matches!(node_at(game, p), Some(n) if n.kind == Kind::Belt && n.front == me.front);
        if !ok {
            broken = true;
        }
    }
    if broken {
        belt_dismantle(game, pos, None);
    }
}

// The belt connector: click one shaft, then another shaft on the same axis in a straight horizontal
// line across it (up to 20 apart). The shafts become the pulleys and belt pieces fill the gap.
fn belt_connector(game: &mut Game<'_>, pos: [i32; 3]) {
    let name = game.native_block(pos);
    let axis = if name == "create:shaft" {
        game.native_block_property(pos, "axis").and_then(|v| parse_axis(&v))
    } else {
        None
    };
    let Some(axis) = axis.filter(|a| *a != 1) else {
        say(game, "Belt connector: click a horizontal shaft");
        return;
    };
    let first = match PENDING.lock() {
        Ok(mut p) => p.take(),
        Err(_) => None,
    };
    let Some(a) = first else {
        if let Ok(mut p) = PENDING.lock() {
            *p = Some(pos);
        }
        say(game, "Belt connector: now click the other shaft");
        return;
    };
    if a == pos {
        say(game, "Belt connector: cancelled");
        return;
    }
    let same_axis = game.native_block(a) == "create:shaft"
        && game.native_block_property(a, "axis").and_then(|v| parse_axis(&v)) == Some(axis);
    let h = if axis == 0 { 2 } else { 0 };
    let delta = [pos[0] - a[0], pos[1] - a[1], pos[2] - a[2]];
    let len = delta[h].abs();
    if !same_axis || delta[1] != 0 || delta[axis] != 0 || len < 1 || len > 19 {
        say(game, "Belt connector: the shafts must share an axis and line up across it, up to 20 apart");
        return;
    }
    let mut d = [0; 3];
    d[h] = delta[h].signum();
    for i in 1..len {
        let p = [a[0] + d[0] * i, a[1], a[2] + d[2] * i];
        if !is_air(&game.native_block(p)) {
            say(game, "Belt connector: something is in the way");
            return;
        }
    }
    let Some(base) = game.pack.assets.registry.default_state("create:belt") else { return };
    let dim = game.dim.id().to_string();
    let facing = dir_name(d);
    for i in 0..=len {
        let p = [a[0] + d[0] * i, a[1], a[2] + d[2] * i];
        let part = if i == 0 {
            "start"
        } else if i == len {
            "end"
        } else {
            "middle"
        };
        game.native_set_block(&dim, p, base, 3);
        game.native_set_block_property(p, "facing", facing);
        game.native_set_block_property(p, "part", part);
        game.native_set_block_property(p, "slope", "horizontal");
        game.native_set_block_property(p, "casing", "false");
    }
    if !game.creative {
        game.consume_held();
    }
    say(game, "Belt connector: belt made");
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
    game.destroy_block_progress(drill_id(pos), pos, -1);
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
        say(game, &("Drill: working on ".to_string() + &name));
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
        wait = ((hardness / break_speed) as i32).max(2);
    }
    put_int(nbt, "bt", wait);
    put_int(nbt, "bp", progress);
    let id = drill_id(pos);
    if progress >= 10 {
        put_int(nbt, "bp", 0);
        put_int(nbt, "bt", 0);
        game.destroy_block_progress(id, target, -1);
        say(game, &("Drill: broke ".to_string() + &name));
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
        game.destroy_block_progress(id, target, progress.clamp(1, 10) - 1);
    }
}

// ---- fluids: pipes, mechanical pump, valve, tanks and the spout ----
// Pipes hold nothing: they only join things. A running pump walks the pipes on its back and on its
// front (valves that are open let it through) and finds the ends: tanks and spouts (which hold
// fluid) and open pipe ends that touch the world (a water or lava source block there can be drawn
// in, or a source block is placed there). Each step the pump moves a little fluid from the ends at
// its back into itself and on to the ends at its front. Tanks that touch share one pool: the lowest
// tank fills first and the highest empties first.

const FLUID_BE: &str = "create:fluid";
const PIPE: &str = "create:fluid_pipe";
const TANK: &str = "create:fluid_tank";
const SPOUT: &str = "create:spout";
const PUMP: &str = "create:mechanical_pump";
const VALVE: &str = "create:fluid_valve";
const TANK_CAP: i32 = 8000;
const SPOUT_CAP: i32 = 1000;
const PUMP_CAP: i32 = 2000;
const BUCKET: i32 = 1000;

// down, up, north, south, west, east: the opposite of index i is i ^ 1.
const DIRS: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

fn add3(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

// The axis a valve turns on, from the axis of its pipe and its axis_along_first flag.
fn valve_axis(pipe: usize, first: bool) -> usize {
    match pipe {
        0 => {
            if first {
                1
            } else {
                2
            }
        }
        1 => {
            if first {
                0
            } else {
                2
            }
        }
        _ => {
            if first {
                0
            } else {
                1
            }
        }
    }
}

fn fluid_put(n: &mut Compound, fluid: &str, amount: i32) {
    if amount <= 0 || fluid.is_empty() {
        n.remove(&JavaString::from("fl"));
        n.remove(&JavaString::from("fa"));
    } else {
        n.insert(JavaString::from("fl"), Tag::String(JavaString::from(fluid)));
        put_int(n, "fa", amount);
    }
}

fn fluid_bucket(fluid: &str) -> Option<&'static str> {
    match fluid {
        "water" => Some("minecraft:water_bucket"),
        "lava" => Some("minecraft:lava_bucket"),
        "milk" => Some("minecraft:milk_bucket"),
        _ => None,
    }
}

fn bucket_fluid(item: &str) -> Option<&'static str> {
    match item {
        "minecraft:water_bucket" => Some("water"),
        "minecraft:lava_bucket" => Some("lava"),
        "minecraft:milk_bucket" => Some("milk"),
        _ => None,
    }
}

fn fluid_title(fluid: &str) -> &'static str {
    match fluid {
        "water" => "Water",
        "lava" => "Lava",
        "milk" => "Milk",
        _ => "Fluid",
    }
}

fn is_fluid_block(name: &str) -> bool {
    matches!(name, PIPE | TANK | SPOUT)
}

// Whether the block at `p` has a fluid port on its face towards `d` (d points from p to the asker).
fn fluid_port(game: &Game<'_>, p: [i32; 3], d: [i32; 3]) -> bool {
    let name = game.native_block(p).to_string();
    match name.as_str() {
        PIPE | TANK => true,
        SPOUT => d == [0, 1, 0],
        PUMP | VALVE => match game.native_block_property(p, "facing").and_then(|v| parse_dir(&v.to_string())) {
            Some(f) => axis_of(f) == axis_of(d),
            None => false,
        },
        _ => false,
    }
}

fn pipe_open(game: &Game<'_>, p: [i32; 3], d: [i32; 3]) -> bool {
    game.native_block_property(p, dir_name(d)).map(|v| v == "true").unwrap_or(false)
}

// A pipe opens towards every neighbour with a port, and a pipe with a single neighbour also opens on the
// far side (its open end). With no neighbour it keeps the shape it was placed with.
fn pipe_sync(game: &mut Game<'_>, pos: [i32; 3]) {
    let mut open = [false; 6];
    let mut n = 0;
    let mut last = 0;
    for (i, d) in DIRS.iter().enumerate() {
        if fluid_port(game, add3(pos, *d), neg(*d)) {
            open[i] = true;
            n += 1;
            last = i;
        }
    }
    if n == 0 {
        return;
    }
    if n == 1 {
        open[last ^ 1] = true;
    }
    for (i, d) in DIRS.iter().enumerate() {
        let name = dir_name(*d);
        let want = if open[i] { "true" } else { "false" };
        let same = game.native_block_property(pos, name).map(|v| v == want).unwrap_or(true);
        if !same {
            game.native_set_block_property(pos, name, want);
        }
    }
}

// A tank shows a cap where no tank lies above it and a base where none lies below.
fn tank_sync(game: &mut Game<'_>, pos: [i32; 3]) {
    let below = game.native_block([pos[0], pos[1] - 1, pos[2]]) == TANK;
    let above = game.native_block([pos[0], pos[1] + 1, pos[2]]) == TANK;
    for (prop, want) in [("bottom", !below), ("top", !above)] {
        let v = if want { "true" } else { "false" };
        let same = game.native_block_property(pos, prop).map(|x| x == v).unwrap_or(true);
        if !same {
            game.native_set_block_property(pos, prop, v);
        }
    }
}

struct Ends {
    tanks: Vec<[i32; 3]>,
    worlds: Vec<[i32; 3]>,
}

// Walks the pipes from `from` in direction `dir` and collects the ends: tanks and spouts, and world
// cells where an open pipe end (or an open pump end) touches the world.
fn walk(game: &Game<'_>, from: [i32; 3], dir: [i32; 3]) -> Ends {
    let mut ends = Ends { tanks: Vec::new(), worlds: Vec::new() };
    let mut seen: HashSet<[i32; 3]> = HashSet::new();
    seen.insert(from);
    let mut stack: Vec<([i32; 3], [i32; 3])> = vec![(from, dir)];
    while let Some((p, d)) = stack.pop() {
        if seen.len() > 600 {
            break;
        }
        let q = add3(p, d);
        if !seen.insert(q) {
            continue;
        }
        let name = game.native_block(q).to_string();
        match name.as_str() {
            PIPE => {
                if !pipe_open(game, q, neg(d)) {
                    continue;
                }
                for e in DIRS.iter() {
                    if *e != neg(d) && pipe_open(game, q, *e) {
                        stack.push((q, *e));
                    }
                }
            }
            TANK => ends.tanks.push(q),
            SPOUT => {
                if d == [0, -1, 0] {
                    ends.tanks.push(q);
                }
            }
            PUMP => {
                let f = game.native_block_property(q, "facing").and_then(|v| parse_dir(&v.to_string()));
                if let Some(f) = f {
                    if axis_of(f) == axis_of(d) {
                        stack.push((q, d));
                    }
                }
            }
            VALVE => {
                let f = game.native_block_property(q, "facing").and_then(|v| parse_dir(&v.to_string()));
                let on = game.native_block_property(q, "enabled").map(|v| v == "true").unwrap_or(false);
                if let Some(f) = f {
                    if on && axis_of(f) == axis_of(d) {
                        stack.push((q, d));
                    }
                }
            }
            _ => ends.worlds.push(q),
        }
    }
    ends
}

// ---- pools: a tank with the tanks touching it, or a spout on its own ----

struct Pool {
    cells: Vec<[i32; 3]>,
    fluid: String,
    amount: i32,
    each: i32,
}

fn pool_of(game: &mut Game<'_>, at: [i32; 3]) -> Pool {
    let is_tank = game.native_block(at) == TANK;
    let each = if is_tank { TANK_CAP } else { SPOUT_CAP };
    let mut cells: Vec<[i32; 3]> = vec![at];
    if is_tank {
        let mut seen: HashSet<[i32; 3]> = HashSet::new();
        seen.insert(at);
        let mut i = 0;
        while i < cells.len() && cells.len() < 64 {
            let p = cells[i];
            i += 1;
            for d in DIRS.iter() {
                let q = add3(p, *d);
                if seen.contains(&q) {
                    continue;
                }
                if game.native_block(q) == TANK {
                    seen.insert(q);
                    cells.push(q);
                }
            }
        }
        cells.sort_by_key(|c| (c[1], c[0], c[2]));
    }
    let mut fluid = String::new();
    let mut amount = 0;
    for c in cells.iter() {
        if let Some((f, a)) = game.native_with_mod_block_entity(*c, |e| (get_text(&e.nbt, "fl"), get_int(&e.nbt, "fa", 0))) {
            if a > 0 {
                amount += a;
                if fluid.is_empty() {
                    fluid = f;
                }
            }
        }
    }
    Pool { cells, fluid, amount, each }
}

// Puts up to `want` of `fluid` into the pool, lowest tank first. Returns what went in.
fn pool_put(game: &mut Game<'_>, pool: &Pool, fluid: &str, want: i32) -> i32 {
    if want <= 0 || (pool.amount > 0 && pool.fluid != fluid) {
        return 0;
    }
    let mut left = want;
    for c in pool.cells.iter() {
        if left <= 0 {
            break;
        }
        let fl = fluid.to_string();
        let each = pool.each;
        let ask = left;
        let took = game
            .native_with_mod_block_entity(*c, move |e| {
                let cur = get_int(&e.nbt, "fa", 0);
                let curf = get_text(&e.nbt, "fl");
                if cur > 0 && curf != fl {
                    return 0;
                }
                let add = (each - cur).max(0).min(ask);
                if add > 0 {
                    fluid_put(&mut e.nbt, &fl, cur + add);
                }
                add
            })
            .unwrap_or(0);
        if took > 0 {
            game.native_block_entity_sync(*c);
            left -= took;
        }
    }
    want - left
}

// Takes up to `want` out of the pool, highest tank first. Returns what came out.
fn pool_take(game: &mut Game<'_>, pool: &Pool, want: i32) -> i32 {
    if want <= 0 || pool.amount <= 0 {
        return 0;
    }
    let mut left = want;
    for c in pool.cells.iter().rev() {
        if left <= 0 {
            break;
        }
        let ask = left;
        let took = game
            .native_with_mod_block_entity(*c, move |e| {
                let cur = get_int(&e.nbt, "fa", 0);
                if cur <= 0 {
                    return 0;
                }
                let curf = get_text(&e.nbt, "fl");
                let t = cur.min(ask);
                fluid_put(&mut e.nbt, &curf, cur - t);
                t
            })
            .unwrap_or(0);
        if took > 0 {
            game.native_block_entity_sync(*c);
            left -= took;
        }
    }
    want - left
}

// ---- the world: source blocks drawn in, source blocks placed ----

fn world_source(game: &Game<'_>, p: [i32; 3]) -> Option<&'static str> {
    let name = game.native_block(p).to_string();
    let fluid = match name.as_str() {
        "minecraft:water" => "water",
        "minecraft:lava" => "lava",
        _ => return None,
    };
    match game.native_block_property(p, "level") {
        Some(v) if v == "0" => Some(fluid),
        _ => None,
    }
}

// Water with two water sources beside it renews itself, as in the game.
fn infinite_water(game: &Game<'_>, p: [i32; 3]) -> bool {
    let mut n = 0;
    for d in DIRS.iter() {
        if d[1] == 0 && world_source(game, add3(p, *d)) == Some("water") {
            n += 1;
        }
    }
    n >= 2
}

fn place_fluid(game: &mut Game<'_>, p: [i32; 3], fluid: &str) -> bool {
    let name = match fluid {
        "water" => "minecraft:water",
        "lava" => "minecraft:lava",
        _ => return false,
    };
    let Some(st) = game.pack.assets.registry.default_state(name) else { return false };
    let dim = game.dim.id().to_string();
    game.native_set_block(&dim, p, st, 3)
}

// ---- the pump ----

// One step of a turning pump (every second tick, server side): draws fluid in from the ends at its
// back, hands it on to the ends at its front. The pump keeps a small buffer in its block entity.
fn pump_step(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, me: &Node, speed: f32) {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return;
    }
    let rpm = speed.abs();
    if rpm < 1.0 {
        return;
    }
    if (game.ticks as i64 + i64::from(pos[0]) + i64::from(pos[2])).rem_euclid(2) != 0 {
        return;
    }
    let rate = ((rpm * 4.0) as i32).clamp(4, BUCKET);
    let flow = if speed > 0.0 { me.front } else { neg(me.front) };
    let outs = walk(game, pos, flow);
    let ins = walk(game, pos, neg(flow));
    let mut out_cells: HashSet<[i32; 3]> = HashSet::new();
    for c in outs.tanks.iter().chain(outs.worlds.iter()) {
        out_cells.insert(*c);
    }
    let mut in_cells: HashSet<[i32; 3]> = HashSet::new();
    for c in ins.tanks.iter().chain(ins.worlds.iter()) {
        in_cells.insert(*c);
    }
    let mut fluid = get_text(nbt, "fl");
    let mut amt = get_int(nbt, "fa", 0);
    // draw in from tanks and spouts at the back
    let mut want = rate.min(PUMP_CAP - amt);
    for c in ins.tanks.iter() {
        if want <= 0 {
            break;
        }
        if out_cells.contains(c) {
            continue;
        }
        let pool = pool_of(game, *c);
        if pool.cells.iter().any(|x| out_cells.contains(x)) {
            continue;
        }
        if pool.amount <= 0 || (amt > 0 && pool.fluid != fluid) {
            continue;
        }
        let got = pool_take(game, &pool, want);
        if got > 0 {
            fluid = pool.fluid.clone();
            amt += got;
            want -= got;
        }
    }
    // draw in a source block from an open end at the back, paced by the speed
    let pace = ((BUCKET / rate).max(1)) as i64;
    if PUMP_CAP - amt >= BUCKET && (game.ticks as i64 / 2).rem_euclid(pace) == 0 {
        let dim = game.dim.id().to_string();
        for c in ins.worlds.iter() {
            if out_cells.contains(c) {
                continue;
            }
            let Some(wf) = world_source(game, *c) else { continue };
            if amt > 0 && fluid != wf {
                continue;
            }
            if !(wf == "water" && infinite_water(game, *c)) {
                game.native_set_block(&dim, *c, BlockStateId(0), 3);
            }
            fluid = wf.to_string();
            amt += BUCKET;
            break;
        }
    }
    // hand on to tanks and spouts at the front
    let mut give = rate.min(amt);
    for c in outs.tanks.iter() {
        if give <= 0 {
            break;
        }
        if in_cells.contains(c) {
            continue;
        }
        let pool = pool_of(game, *c);
        if pool.cells.iter().any(|x| in_cells.contains(x)) {
            continue;
        }
        let put = pool_put(game, &pool, &fluid, give);
        amt -= put;
        give -= put;
    }
    // place a source block at an open end at the front once a bucket is ready
    if amt >= BUCKET && !fluid.is_empty() {
        for c in outs.worlds.iter() {
            if in_cells.contains(c) {
                continue;
            }
            if is_air(&game.native_block(*c)) && place_fluid(game, *c, &fluid) {
                amt -= BUCKET;
                break;
            }
        }
    }
    fluid_put(nbt, &fluid, amt);
}

// ---- the valve ----
// Turning one way opens it, the other way closes it. A new valve starts open. The enabled state shows
// it and lets the pump through.

fn valve_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, speed: f32) {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return;
    }
    let mut open = if nbt.get(&JavaString::from("vo")).is_some() { get_f32(nbt, "vo") } else { 1.0 };
    if speed.abs() >= 1.0 {
        open = (open + speed * 0.004).clamp(0.0, 1.0);
    }
    put_f32(nbt, "vo", open);
    let want = if open >= 0.5 { "true" } else { "false" };
    let same = game.native_block_property(pos, "enabled").map(|v| v == want).unwrap_or(true);
    if !same {
        game.native_set_block_property(pos, "enabled", want);
    }
}

// ---- the spout: fills empty buckets that lie on a depot or ride a belt under it ----

fn spout_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound) {
    let fluid = get_text(nbt, "fl");
    let amt = get_int(nbt, "fa", 0);
    if amt < BUCKET {
        return;
    }
    let Some(full) = fluid_bucket(&fluid) else { return };
    let below = [pos[0], pos[1] - 1, pos[2]];
    let mut done = false;
    if let Some(Some((item, count))) = depot_at(game, below) {
        if item == "minecraft:bucket" {
            if count <= 1 {
                depot_set(game, below, Some((full.to_string(), 1)));
            } else {
                depot_set(game, below, Some((item.clone(), count - 1)));
                let at = [f64::from(below[0]) + 0.5, f64::from(below[1]) + 1.0, f64::from(below[2]) + 0.5];
                game.spawn_item(stack_of(full, 1), at, [0.0, 0.1, 0.0], 10);
            }
            done = true;
        }
    }
    if !done && game.native_block(below) == "create:belt" {
        let name = full.to_string();
        done = game
            .native_with_mod_block_entity(below, move |e| {
                let mut v = belt_items(&e.nbt);
                let idx = v.iter().position(|b| b.item == "minecraft:bucket" && b.count == 1);
                match idx {
                    Some(i) => {
                        v[i].item = name;
                        belt_store(&mut e.nbt, &v);
                        true
                    }
                    None => false,
                }
            })
            .unwrap_or(false);
        if done {
            game.native_block_entity_sync(below);
        }
    }
    if done {
        fluid_put(nbt, &fluid, amt - BUCKET);
        game.native_block_entity_sync(pos);
    }
}

// Pipes, tanks and spouts tick every fifth tick.
fn fluid_tick(game: &mut Game<'_>, pos: [i32; 3], entity: &mut ModBlockEntity) {
    if (game.ticks as i64 + i64::from(pos[0]) + i64::from(pos[1]) + i64::from(pos[2])).rem_euclid(5) != 0 {
        return;
    }
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return;
    }
    let name = game.native_block(pos).to_string();
    if name == PIPE {
        pipe_sync(game, pos);
    } else if name == TANK {
        tank_sync(game, pos);
    } else if name == SPOUT {
        spout_tick(game, pos, &mut entity.nbt);
    }
}

// ---- buckets and the info line ----

fn give_item(game: &mut Game<'_>, pos: [i32; 3], item: &str) {
    let at = at_player(game, pos);
    game.spawn_item(stack_of(item, 1), at, [0.0, 0.0, 0.0], 0);
}

// A bucket on a tank or spout: a full one pours in, an empty one takes a bucket out.
fn bucket_click(game: &mut Game<'_>, pos: [i32; 3]) -> bool {
    let Some(item) = game.held().map(|h| h.item.to_string()) else { return false };
    let pool = pool_of(game, pos);
    let total = pool.each * pool.cells.len() as i32;
    if let Some(fl) = bucket_fluid(&item) {
        let fits = total - pool.amount >= BUCKET && (pool.amount == 0 || pool.fluid == fl);
        if !fits {
            return false;
        }
        pool_put(game, &pool, fl, BUCKET);
        if !game.creative {
            game.consume_held();
            give_item(game, pos, "minecraft:bucket");
        }
        return true;
    }
    if item == "minecraft:bucket" {
        let Some(full) = fluid_bucket(&pool.fluid) else { return false };
        if pool.amount < BUCKET {
            return false;
        }
        pool_take(game, &pool, BUCKET);
        if !game.creative {
            game.consume_held();
        }
        give_item(game, pos, full);
        return true;
    }
    false
}

fn fluid_info(game: &mut Game<'_>, pos: [i32; 3], name: &str) {
    let pool = pool_of(game, pos);
    let total = pool.each * pool.cells.len() as i32;
    let label = if name == TANK { "Fluid Tank" } else { "Spout" };
    let what = if pool.amount > 0 { fluid_title(&pool.fluid) } else { "empty" };
    let s = sect();
    let line = s.clone()
        + "7"
        + label
        + ": "
        + &s
        + "f"
        + what
        + " "
        + &pool.amount.to_string()
        + &s
        + "7 / "
        + &s
        + "f"
        + &total.to_string()
        + &s
        + "7 mB";
    game.native_chat(&line);
}

// A new pipe lies along the axis you look along; a new tank gets its window.
fn fluid_placement(
    level: &GameLevel<'_>,
    state: BlockStateId,
    context: &PlacementContext,
    name: &str,
    answer: &mut Option<Option<BlockStateId>>,
) {
    if name == TANK {
        if let Some(s) = level.native_state_with_property(state, "shape", "window") {
            *answer = Some(Some(s));
        }
        return;
    }
    let look = nearest_looking(context.yaw, context.pitch);
    let mut st = Some(state);
    for d in [look, neg(look)] {
        st = st.and_then(|s| level.native_state_with_property(s, dir_name(d), "true"));
    }
    if let Some(s) = st {
        *answer = Some(Some(s));
    }
}

// ---- round 3: millstone, mechanical saw, crushing wheel, mechanical crafter ----
// Recipes are read from Creates own recipe files when the pack carries them (types create:milling,
// create:crushing, create:cutting and create:mechanical_crafting, through the games table of custom
// recipes); the small tables below are the fallback. All four machines only run on the server side.

struct Rec {
    time: i32,
    outs: Vec<(String, u32, f32)>,
}

// A cheap repeatable random number between 0 and 1.
fn pseudo(game: &Game<'_>, pos: [i32; 3], salt: i32) -> f32 {
    let mut h = (game.ticks as u64) ^ (pos[0] as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= (pos[1] as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= (pos[2] as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= (salt as i64 as u64).wrapping_mul(0x27D4_EB2F_1656_67C5);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    ((h >> 40) as f32) / 16_777_216.0
}

fn json_ingredient(v: &serde_json::Value) -> Option<serde_json::Value> {
    if let Some(s) = v.as_str() {
        return Some(serde_json::Value::String(s.to_string()));
    }
    if let Some(s) = v.get("item").and_then(|x| x.as_str()) {
        return Some(serde_json::Value::String(s.to_string()));
    }
    if let Some(s) = v.get("tag").and_then(|x| x.as_str()) {
        return Some(serde_json::Value::String(String::from("#") + s));
    }
    None
}

fn ingredient_has(game: &Game<'_>, v: &serde_json::Value, item: &str) -> bool {
    let Some(j) = json_ingredient(v) else { return false };
    match game.native_recipe_ingredient(&j) {
        Some(ing) => ing.0.contains(item),
        None => false,
    }
}

// A recipe of type `kind` with one ingredient that fits `item`, from the packs recipe files.
fn custom_rec(game: &Game<'_>, kind: &str, item: &str) -> Option<Rec> {
    for r in game.native_recipes_of_type(kind) {
        let j: &serde_json::Value = &r.json;
        let Some(ings) = j.get("ingredients").and_then(|v| v.as_array()) else { continue };
        if ings.len() != 1 || !ingredient_has(game, &ings[0], item) {
            continue;
        }
        let time = j.get("processing_time").and_then(|v| v.as_i64()).unwrap_or(100) as i32;
        let mut outs: Vec<(String, u32, f32)> = Vec::new();
        if let Some(rs) = j.get("results").and_then(|v| v.as_array()) {
            for o in rs {
                let id = o
                    .get("id")
                    .and_then(|v| v.as_str())
                    .or_else(|| o.get("item").and_then(|v| v.as_str()));
                let Some(id) = id else { continue };
                let count = o.get("count").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
                let chance = o.get("chance").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                outs.push((id.to_string(), count.max(1), chance));
            }
        }
        if !outs.is_empty() {
            return Some(Rec { time: time.max(1), outs });
        }
    }
    None
}

const WOODS: [&str; 12] = [
    "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "pale_oak", "crimson", "warped",
    "bamboo",
];

// Sawing: a log becomes its stripped log, a stripped log becomes six planks.
fn builtin_cut(item: &str) -> Option<Rec> {
    let name = item.strip_prefix("minecraft:")?;
    let (stripped, base) = match name.strip_prefix("stripped_") {
        Some(b) => (true, b),
        None => (false, name),
    };
    let mut species = "";
    for sfx in ["_log", "_wood", "_stem", "_hyphae", "_block"] {
        if let Some(s) = base.strip_suffix(sfx) {
            if WOODS.contains(&s) {
                species = s;
                break;
            }
        }
    }
    if species.is_empty() {
        return None;
    }
    if stripped {
        Some(Rec { time: 50, outs: vec![(String::from("minecraft:") + species + "_planks", 6, 1.0)] })
    } else {
        Some(Rec { time: 50, outs: vec![(String::from("minecraft:stripped_") + base, 1, 1.0)] })
    }
}

fn rec_of(time: i32, outs: &[(&str, u32, f32)]) -> Rec {
    Rec { time, outs: outs.iter().map(|o| (o.0.to_string(), o.1, o.2)).collect() }
}

fn builtin(kind: &str, item: &str) -> Option<Rec> {
    match kind {
        "create:milling" => match item {
            "minecraft:wheat" => Some(rec_of(100, &[("create:wheat_flour", 1, 1.0)])),
            "minecraft:bone" => Some(rec_of(100, &[("minecraft:bone_meal", 3, 1.0)])),
            "minecraft:iron_ore" | "minecraft:deepslate_iron_ore" | "minecraft:raw_iron" => {
                Some(rec_of(200, &[("create:crushed_raw_iron", 1, 1.0)]))
            }
            "minecraft:copper_ore" | "minecraft:deepslate_copper_ore" | "minecraft:raw_copper" => {
                Some(rec_of(200, &[("create:crushed_raw_copper", 1, 1.0)]))
            }
            "minecraft:gold_ore" | "minecraft:deepslate_gold_ore" | "minecraft:raw_gold" => {
                Some(rec_of(200, &[("create:crushed_raw_gold", 1, 1.0)]))
            }
            "create:zinc_ore" | "create:deepslate_zinc_ore" | "create:raw_zinc" => {
                Some(rec_of(200, &[("create:crushed_raw_zinc", 1, 1.0)]))
            }
            _ => None,
        },
        "create:crushing" => match item {
            "minecraft:iron_ore" | "minecraft:deepslate_iron_ore" | "minecraft:raw_iron" => {
                Some(rec_of(400, &[("create:crushed_raw_iron", 1, 1.0), ("create:crushed_raw_iron", 1, 0.3)]))
            }
            "minecraft:copper_ore" | "minecraft:deepslate_copper_ore" | "minecraft:raw_copper" => {
                Some(rec_of(400, &[("create:crushed_raw_copper", 1, 1.0), ("create:crushed_raw_copper", 1, 0.3)]))
            }
            "minecraft:gold_ore" | "minecraft:deepslate_gold_ore" | "minecraft:raw_gold" => {
                Some(rec_of(400, &[("create:crushed_raw_gold", 1, 1.0), ("create:crushed_raw_gold", 1, 0.3)]))
            }
            "create:zinc_ore" | "create:deepslate_zinc_ore" | "create:raw_zinc" => {
                Some(rec_of(400, &[("create:crushed_raw_zinc", 1, 1.0), ("create:crushed_raw_zinc", 1, 0.3)]))
            }
            "minecraft:cobblestone" => Some(rec_of(250, &[("minecraft:gravel", 1, 1.0)])),
            _ => None,
        },
        "create:cutting" => builtin_cut(item),
        _ => None,
    }
}

fn recipe_for(game: &Game<'_>, kind: &str, item: &str) -> Option<Rec> {
    custom_rec(game, kind, item).or_else(|| builtin(kind, item))
}

// What a run of the recipe gives for `mult` items put in (each output rolled on its chance).
fn roll_outputs(game: &Game<'_>, pos: [i32; 3], rec: &Rec, mult: u32) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    for (i, o) in rec.outs.iter().enumerate() {
        let mut n = 0u32;
        for k in 0..mult.min(256) {
            if o.2 >= 1.0 || pseudo(game, pos, (i as i32) * 131 + k as i32) < o.2 {
                n += o.1;
            }
        }
        if n == 0 {
            continue;
        }
        let mut found = false;
        for e in out.iter_mut() {
            if e.0 == o.0 {
                e.1 += n;
                found = true;
            }
        }
        if !found {
            out.push((o.0.clone(), n));
        }
    }
    out
}

// ---- item lists kept in a block entity as text: item|count;item|count ----

fn pair_list(n: &Compound, key: &str) -> Vec<(String, u32)> {
    let text = get_text(n, key);
    let mut out = Vec::new();
    for part in text.split(';') {
        let mut f = part.splitn(2, '|');
        let (Some(item), Some(count)) = (f.next(), f.next()) else { continue };
        let Ok(count) = count.parse::<u32>() else { continue };
        if !item.is_empty() && count > 0 {
            out.push((item.to_string(), count));
        }
    }
    out
}

fn pair_store(n: &mut Compound, key: &str, list: &[(String, u32)]) {
    if list.is_empty() {
        n.remove(&JavaString::from(key));
        return;
    }
    let mut text = String::new();
    for (i, e) in list.iter().enumerate() {
        if i > 0 {
            text.push(';');
        }
        text.push_str(&e.0);
        text.push('|');
        text.push_str(&e.1.to_string());
    }
    n.insert(JavaString::from(key), Tag::String(JavaString::from(text.as_str())));
}

// Puts stacks into a list of at most `slots` stacks of 64; false (and no change) when they do not fit.
fn add_all(list: &mut Vec<(String, u32)>, items: &[(String, u32)], slots: usize) -> bool {
    let mut work = list.clone();
    for (item, count) in items {
        let mut left = *count;
        for e in work.iter_mut() {
            if left == 0 {
                break;
            }
            if e.0 == *item && e.1 < 64 {
                let add = (64 - e.1).min(left);
                e.1 += add;
                left -= add;
            }
        }
        while left > 0 {
            if work.len() >= slots {
                return false;
            }
            let add = left.min(64);
            work.push((item.clone(), add));
            left -= add;
        }
    }
    *list = work;
    true
}

fn take_held(game: &mut Game<'_>, n: u32) {
    let left = match game.held() {
        Some(h) => h.count.saturating_sub(n),
        None => return,
    };
    if left == 0 {
        *game.held_mut() = None;
    } else if let Some(h) = game.held_mut() {
        h.count = left;
    }
}

fn give_player(game: &mut Game<'_>, pos: [i32; 3], list: &[(String, u32)]) {
    let at = at_player(game, pos);
    for e in list {
        game.spawn_item(stack_of(&e.0, e.1), at, [0.0, 0.0, 0.0], 0);
    }
}

fn server_side(game: &Game<'_>) -> bool {
    game.serving.is_some() || game.options.dedicated.is_some()
}

// ---- the millstone ----
// A shaft below turns the stone. Items lying on top fall in (one kind at a time, up to 64), one item is
// milled per cycle, and the results wait inside (nine stacks) until a click with an empty hand takes them.

const MILL_SLOTS: usize = 9;

fn mill_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, speed: f32) -> bool {
    if !server_side(game) {
        return false;
    }
    let mut changed = false;
    let mut input = pair_list(nbt, "mi");
    if ((game.ticks as i64) + i64::from(pos[0]) + i64::from(pos[2])).rem_euclid(4) == 0 {
        let dim = game.dim.id().to_string();
        let (x, y, z) = (f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        let found = game.native_item_entities(&dim, [x, y + 0.9, z], [x + 1.0, y + 1.8, z + 1.0]);
        for e in found {
            let name = e.stack.item.to_string();
            if recipe_for(game, "create:milling", &name).is_none() {
                continue;
            }
            let have = input.first().cloned();
            let room = match &have {
                Some((it, c)) => {
                    if *it == name {
                        64u32.saturating_sub(*c)
                    } else {
                        0
                    }
                }
                None => 64,
            };
            if room == 0 {
                continue;
            }
            let take = room.min(e.stack.count);
            let mut rest = e.stack.clone();
            rest.count = e.stack.count - take;
            if game.native_set_item_entity(&dim, e.id, rest) {
                let total = have.map(|v| v.1).unwrap_or(0) + take;
                input = vec![(name, total)];
                changed = true;
            }
        }
    }
    let mut timer = get_int(nbt, "mt", -1);
    if speed.abs() >= 1.0 {
        if let Some((item, count)) = input.first().cloned() {
            match recipe_for(game, "create:milling", &item) {
                Some(rec) => {
                    if timer < 0 {
                        timer = rec.time;
                    }
                    let rate = ((speed.abs() / 16.0) as i32).clamp(1, 512);
                    timer -= rate;
                    if timer <= 0 {
                        let rolled = roll_outputs(game, pos, &rec, 1);
                        let mut outs = pair_list(nbt, "mo");
                        if add_all(&mut outs, &rolled, MILL_SLOTS) {
                            pair_store(nbt, "mo", &outs);
                            if count > 1 {
                                input = vec![(item, count - 1)];
                            } else {
                                input.clear();
                            }
                            timer = -1;
                            changed = true;
                        } else {
                            timer = 1;
                        }
                    }
                }
                None => {
                    timer = -1;
                }
            }
        } else {
            timer = -1;
        }
    }
    put_int(nbt, "mt", timer);
    if changed {
        pair_store(nbt, "mi", &input);
    }
    changed
}

// A click on the millstone: a held stack that can be milled goes in; an empty hand takes the results,
// or, when there are none, the stack waiting to be milled.
fn millstone_click(game: &mut Game<'_>, pos: [i32; 3]) -> bool {
    let held = game.held().cloned();
    if let Some(h) = &held {
        let name = h.item.to_string();
        if h.count > 0 && recipe_for(game, "create:milling", &name).is_some() {
            let count = h.count;
            let took = game
                .native_with_mod_block_entity(pos, move |e| {
                    let have = pair_list(&e.nbt, "mi").first().cloned();
                    let room = match &have {
                        Some((it, c)) => {
                            if *it == name {
                                64u32.saturating_sub(*c)
                            } else {
                                0
                            }
                        }
                        None => 64,
                    };
                    let take = room.min(count);
                    if take == 0 {
                        return 0;
                    }
                    let total = have.map(|v| v.1).unwrap_or(0) + take;
                    pair_store(&mut e.nbt, "mi", &[(name, total)]);
                    take
                })
                .unwrap_or(0);
            if took > 0 {
                game.native_block_entity_sync(pos);
                take_held(game, took);
                return true;
            }
        }
    }
    let outs = game
        .native_with_mod_block_entity(pos, |e| {
            let v = pair_list(&e.nbt, "mo");
            pair_store(&mut e.nbt, "mo", &[]);
            v
        })
        .unwrap_or_default();
    if !outs.is_empty() {
        game.native_block_entity_sync(pos);
        give_player(game, pos, &outs);
        return true;
    }
    if held.is_none() {
        let back = game
            .native_with_mod_block_entity(pos, |e| {
                let v = pair_list(&e.nbt, "mi");
                pair_store(&mut e.nbt, "mi", &[]);
                v
            })
            .unwrap_or_default();
        if !back.is_empty() {
            game.native_block_entity_sync(pos);
            give_player(game, pos, &back);
            return true;
        }
    }
    false
}

// ---- the mechanical saw ----
// The blade points along the saw facing. It cuts the items lying on a belt piece or a depot in front of it
// (or on the ground there), and a log in front of it: the log is sawn through with a crack, and the
// logs joined to it above and beside it fall with it.

fn is_log(name: &str) -> bool {
    name.ends_with("_log")
        || name.ends_with("_wood")
        || name.ends_with("_stem")
        || name.ends_with("_hyphae")
        || name == "minecraft:bamboo_block"
}

fn crack_clear(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound) {
    let was = [get_int(nbt, "bx", i32::MIN), get_int(nbt, "by", i32::MIN), get_int(nbt, "bz", i32::MIN)];
    if get_int(nbt, "bp", 0) != 0 && was[0] != i32::MIN {
        game.destroy_block_progress(drill_id(pos), was, -1);
    }
    put_int(nbt, "bp", 0);
    put_int(nbt, "bt", 0);
}

fn saw_belt(game: &mut Game<'_>, target: [i32; 3]) {
    let Some(items) = belt_items_at(game, target) else { return };
    let mut next = items.clone();
    let mut changed = false;
    for b in next.iter_mut() {
        if let Some(rec) = recipe_for(game, "create:cutting", &b.item) {
            if let Some(o) = rec.outs.first() {
                b.item = o.0.clone();
                b.count = b.count.saturating_mul(o.1);
                changed = true;
            }
        }
    }
    if changed {
        let _ = game.native_with_mod_block_entity(target, move |e| belt_store(&mut e.nbt, &next));
        game.native_block_entity_sync(target);
    }
}

fn saw_ground(game: &mut Game<'_>, target: [i32; 3]) {
    let dim = game.dim.id().to_string();
    let (x, y, z) = (f64::from(target[0]), f64::from(target[1]), f64::from(target[2]));
    let found = game.native_item_entities(&dim, [x, y, z], [x + 1.0, y + 1.0, z + 1.0]);
    for e in found {
        let name = e.stack.item.to_string();
        let Some(rec) = recipe_for(game, "create:cutting", &name) else { continue };
        let Some(o) = rec.outs.first() else { continue };
        let count = e.stack.count.saturating_mul(o.1).min(99);
        let mut gone = e.stack.clone();
        gone.count = 0;
        if game.native_set_item_entity(&dim, e.id, gone) {
            game.spawn_item(stack_of(&o.0, count), e.position, [0.0, 0.1, 0.0], 10);
        }
    }
}

fn saw_tree(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, target: [i32; 3], speed: f32) {
    let state = game.world.block(target[0], target[1], target[2]);
    let facts = game.res.facts().get(state).clone();
    let name = facts.name.to_string();
    if !is_log(&name) {
        crack_clear(game, pos, nbt);
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
        wait = ((hardness / break_speed) as i32).max(2);
    }
    put_int(nbt, "bt", wait);
    put_int(nbt, "bp", progress);
    let id = drill_id(pos);
    if progress < 10 {
        game.destroy_block_progress(id, target, progress.clamp(1, 10) - 1);
        return;
    }
    put_int(nbt, "bp", 0);
    put_int(nbt, "bt", 0);
    game.destroy_block_progress(id, target, -1);
    let mut seen: HashSet<[i32; 3]> = HashSet::new();
    seen.insert(target);
    let mut stack = vec![target];
    let mut order: Vec<[i32; 3]> = Vec::new();
    while let Some(p) = stack.pop() {
        order.push(p);
        if order.len() >= 96 {
            break;
        }
        for dx in -1..=1 {
            for dy in 0..=1 {
                for dz in -1..=1 {
                    let q = [p[0] + dx, p[1] + dy, p[2] + dz];
                    if seen.contains(&q) {
                        continue;
                    }
                    let st = game.world.block(q[0], q[1], q[2]);
                    let nm = game.res.facts().get(st).name.to_string();
                    if is_log(&nm) {
                        seen.insert(q);
                        stack.push(q);
                    }
                }
            }
        }
    }
    let dim = game.dim.id().to_string();
    for p in order {
        let st = game.world.block(p[0], p[1], p[2]);
        let drops = game.block_loot_at(p, st, None);
        if game.native_set_block(&dim, p, BlockStateId(0), 3) {
            game.destroy_particles(p, st);
            for d in drops {
                if &*d.item != "minecraft:air" {
                    game.pop_resource(p, d);
                }
            }
        }
    }
}

fn saw_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, me: &Node, speed: f32) -> bool {
    if !server_side(game) {
        return false;
    }
    if speed.abs() < 1.0 {
        crack_clear(game, pos, nbt);
        return false;
    }
    let target = [pos[0] + me.front[0], pos[1] + me.front[1], pos[2] + me.front[2]];
    let blk = game.native_block(target);
    let due = ((game.ticks as i64) + i64::from(pos[0]) + i64::from(pos[2])).rem_euclid(5) == 0;
    if blk == "create:belt" {
        if due {
            saw_belt(game, target);
        }
        crack_clear(game, pos, nbt);
        return false;
    }
    if blk == DEPOT_BLOCK {
        if due {
            if let Some(Some((item, count))) = depot_at(game, target) {
                if let Some(rec) = recipe_for(game, "create:cutting", &item) {
                    if let Some(o) = rec.outs.first() {
                        depot_set(game, target, Some((o.0.clone(), count.saturating_mul(o.1).min(99))));
                    }
                }
            }
        }
        crack_clear(game, pos, nbt);
        return false;
    }
    if due {
        saw_ground(game, target);
    }
    saw_tree(game, pos, nbt, target, speed);
    false
}

// ---- the crushing wheels ----
// Two wheels with one block between them, turning against each other, crush the items that fall into the
// gap (the wheel with the smaller coordinate runs the pair). Crushed items wait in the wheel for the
// recipe time, then the results drop out below the gap. The queue is text: item|count|time left.

fn triple_list(n: &Compound, key: &str) -> Vec<(String, u32, f32)> {
    let text = get_text(n, key);
    let mut out = Vec::new();
    for part in text.split(';') {
        let mut f = part.splitn(3, '|');
        let (Some(item), Some(count), Some(left)) = (f.next(), f.next(), f.next()) else { continue };
        let (Ok(count), Ok(left)) = (count.parse::<u32>(), left.parse::<f32>()) else { continue };
        if !item.is_empty() && count > 0 {
            out.push((item.to_string(), count, left));
        }
    }
    out
}

fn triple_store(n: &mut Compound, key: &str, list: &[(String, u32, f32)]) {
    if list.is_empty() {
        n.remove(&JavaString::from(key));
        return;
    }
    let mut text = String::new();
    for (i, e) in list.iter().enumerate() {
        if i > 0 {
            text.push(';');
        }
        text.push_str(&e.0);
        text.push('|');
        text.push_str(&e.1.to_string());
        text.push('|');
        text.push_str(&e.2.to_string());
    }
    n.insert(JavaString::from(key), Tag::String(JavaString::from(text.as_str())));
}

fn crush_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, me: &Node, speed: f32) -> bool {
    if !server_side(game) {
        return false;
    }
    let mut queue = triple_list(nbt, "cq");
    let mut changed = false;
    // the partner two blocks away across the gap (horizontal only)
    let mut gap: Option<([i32; 3], usize)> = None;
    for a in [0usize, 2usize] {
        if a == me.axis {
            continue;
        }
        let mut p = pos;
        p[a] += 2;
        if game.native_block(p) != "create:crushing_wheel" {
            continue;
        }
        let Some(nb) = node_at(game, p) else { continue };
        if nb.axis != me.axis {
            continue;
        }
        let Some(r) = read_neighbour(game, p) else { continue };
        if r.speed * speed < 0.0 {
            let mut g = pos;
            g[a] += 1;
            gap = Some((g, a));
        }
    }
    let running = speed.abs() >= 1.0 && gap.is_some();
    if let (true, Some((g, a))) = (running, gap) {
        let dim = game.dim.id().to_string();
        let mut lo = [f64::from(g[0]), f64::from(g[1]), f64::from(g[2])];
        let mut hi = [lo[0] + 1.0, lo[1] + 2.0, lo[2] + 1.0];
        lo[a] -= 0.25;
        hi[a] += 0.25;
        if queue.len() < 4 {
            let found = game.native_item_entities(&dim, lo, hi);
            for e in found {
                if queue.len() >= 4 {
                    break;
                }
                let name = e.stack.item.to_string();
                let Some(rec) = recipe_for(game, "create:crushing", &name) else { continue };
                let mut gone = e.stack.clone();
                gone.count = 0;
                if game.native_set_item_entity(&dim, e.id, gone) {
                    queue.push((name, e.stack.count, rec.time as f32));
                    changed = true;
                }
            }
        }
        let rate = (speed.abs() / 50.0).clamp(1.0, 512.0);
        let mut keep: Vec<(String, u32, f32)> = Vec::new();
        for (item, count, left) in queue.drain(..) {
            let left = left - rate;
            if left > 0.0 {
                keep.push((item, count, left));
                continue;
            }
            if let Some(rec) = recipe_for(game, "create:crushing", &item) {
                let outs = roll_outputs(game, g, &rec, count);
                let at = [f64::from(g[0]) + 0.5, f64::from(g[1]) + 0.2, f64::from(g[2]) + 0.5];
                for o in outs {
                    game.spawn_item(stack_of(&o.0, o.1.min(99)), at, [0.0, -0.1, 0.0], 10);
                }
            }
            changed = true;
        }
        queue = keep;
    }
    triple_store(nbt, "cq", &queue);
    changed
}

// ---- the mechanical crafter ----
// Crafters side by side on one wall, all facing the same way, form a grid. Each holds one stack and points
// at the next one; a crafter that points at nothing is the last one and puts the result out in its
// direction. Directions are as seen from the front: up, down, left, right. When every crafter of a group
// holds something, the group is turning and the items make a recipe, one of each is used up and the result
// comes out. Click with the wrench to turn the pointing; click with an item to fill, with an empty hand to
// empty.

const POINTS: [&str; 4] = ["up", "right", "down", "left"];

fn right_vec(front: [i32; 3]) -> [i32; 3] {
    [front[2], 0, -front[0]]
}

fn point_vec(front: [i32; 3], pointing: &str) -> [i32; 3] {
    match pointing {
        "down" => [0, -1, 0],
        "right" => right_vec(front),
        "left" => neg(right_vec(front)),
        _ => [0, 1, 0],
    }
}

// The facing and the pointing direction of the crafter at `p`.
fn crafter_at(game: &Game<'_>, p: [i32; 3]) -> Option<([i32; 3], [i32; 3])> {
    if game.native_block(p) != "create:mechanical_crafter" {
        return None;
    }
    let front = parse_dir(&game.native_block_property(p, "facing")?)?;
    let pointing = game.native_block_property(p, "pointing")?;
    Some((front, point_vec(front, &pointing)))
}

fn crafter_result(game: &Game<'_>, cells: &[Option<String>], w: usize, h: usize) -> Option<ItemStack> {
    for r in game.native_recipes_of_type("create:mechanical_crafting") {
        let j: &serde_json::Value = &r.json;
        let Some(rows) = j.get("pattern").and_then(|v| v.as_array()) else { continue };
        let Some(key) = j.get("key").and_then(|v| v.as_object()) else { continue };
        if rows.len() != h {
            continue;
        }
        for mirror in [false, true] {
            let mut ok = true;
            for (ri, row) in rows.iter().enumerate() {
                let Some(row) = row.as_str() else {
                    ok = false;
                    break;
                };
                let chars: Vec<char> = row.chars().collect();
                if chars.len() != w {
                    ok = false;
                    break;
                }
                for ci in 0..w {
                    let ch = if mirror { chars[w - 1 - ci] } else { chars[ci] };
                    let cell = &cells[ri * w + ci];
                    if ch == ' ' {
                        if cell.is_some() {
                            ok = false;
                        }
                    } else {
                        let fits = match (cell, key.get(&ch.to_string())) {
                            (Some(item), Some(ing)) => ingredient_has(game, ing, item),
                            _ => false,
                        };
                        if !fits {
                            ok = false;
                        }
                    }
                }
                if !ok {
                    break;
                }
            }
            if ok {
                if let Some(res) = j.get("result").and_then(|v| game.native_recipe_result(v)) {
                    return Some(res);
                }
            }
        }
    }
    if w <= 3 && h <= 3 {
        let mut grid: Vec<Option<ItemStack>> = vec![None; 9];
        for r in 0..h {
            for c in 0..w {
                if let Some(item) = &cells[r * w + c] {
                    grid[r * 3 + c] = Some(stack_of(item, 1));
                }
            }
        }
        return game.recipes().craft(&grid, 3);
    }
    None
}

fn crafter_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, speed: f32) -> bool {
    if !server_side(game) {
        return false;
    }
    if speed.abs() < 1.0 {
        put_int(nbt, "ct", 0);
        return false;
    }
    if ((game.ticks as i64) + i64::from(pos[0]) + i64::from(pos[2])).rem_euclid(5) != 0 {
        return false;
    }
    let Some((front, dirv)) = crafter_at(game, pos) else { return false };
    // only the last crafter of a group works the group
    if let Some((f2, _)) = crafter_at(game, add3(pos, dirv)) {
        if f2 == front {
            put_int(nbt, "ct", 0);
            return false;
        }
    }
    let right = right_vec(front);
    let plane = [[0, 1, 0], [0, -1, 0], right, neg(right)];
    let mut members: Vec<[i32; 3]> = vec![pos];
    let mut i = 0;
    while i < members.len() && members.len() < 81 {
        let c = members[i];
        i += 1;
        for d in plane {
            let q = add3(c, d);
            if members.contains(&q) {
                continue;
            }
            if let Some((f2, d2)) = crafter_at(game, q) {
                if f2 == front && add3(q, d2) == c {
                    members.push(q);
                }
            }
        }
    }
    // every crafter holds something and turns
    let mut cells_at: Vec<(i32, i32, String)> = Vec::new();
    for m in members.iter() {
        let item = if *m == pos {
            pair_list(nbt, "ci").first().map(|v| v.0.clone())
        } else {
            if read_neighbour(game, *m).map(|r| r.speed.abs() < 1.0).unwrap_or(true) {
                put_int(nbt, "ct", 0);
                return false;
            }
            game.native_with_mod_block_entity(*m, |e| pair_list(&e.nbt, "ci").first().map(|v| v.0.clone())).flatten()
        };
        let Some(item) = item else {
            put_int(nbt, "ct", 0);
            return false;
        };
        cells_at.push((m[0] * right[0] + m[2] * right[2], m[1], item));
    }
    let umin = cells_at.iter().map(|c| c.0).min().unwrap_or(0);
    let umax = cells_at.iter().map(|c| c.0).max().unwrap_or(0);
    let vmin = cells_at.iter().map(|c| c.1).min().unwrap_or(0);
    let vmax = cells_at.iter().map(|c| c.1).max().unwrap_or(0);
    let (w, h) = ((umax - umin + 1) as usize, (vmax - vmin + 1) as usize);
    if w > 9 || h > 9 {
        return false;
    }
    let mut cells: Vec<Option<String>> = vec![None; w * h];
    for c in cells_at.iter() {
        let col = (c.0 - umin) as usize;
        let row = (vmax - c.1) as usize;
        cells[row * w + col] = Some(c.2.clone());
    }
    let Some(result) = crafter_result(game, &cells, w, h) else {
        put_int(nbt, "ct", 0);
        return false;
    };
    let progress = get_int(nbt, "ct", 0) + ((speed.abs() / 4.0) as i32).clamp(1, 64);
    if progress < 60 {
        put_int(nbt, "ct", progress);
        return false;
    }
    put_int(nbt, "ct", 0);
    for m in members.iter() {
        if *m == pos {
            let mut v = pair_list(nbt, "ci");
            if let Some(first) = v.first_mut() {
                first.1 -= 1;
            }
            v.retain(|e| e.1 > 0);
            pair_store(nbt, "ci", &v);
        } else {
            let _ = game.native_with_mod_block_entity(*m, |e| {
                let mut v = pair_list(&e.nbt, "ci");
                if let Some(first) = v.first_mut() {
                    first.1 -= 1;
                }
                v.retain(|e| e.1 > 0);
                pair_store(&mut e.nbt, "ci", &v);
            });
            game.native_block_entity_sync(*m);
        }
    }
    let at = [
        f64::from(pos[0]) + 0.5 + f64::from(dirv[0]) * 0.75,
        f64::from(pos[1]) + 0.5 + f64::from(dirv[1]) * 0.75,
        f64::from(pos[2]) + 0.5 + f64::from(dirv[2]) * 0.75,
    ];
    let speed_out = [f64::from(dirv[0]) * 0.1, f64::from(dirv[1]) * 0.1 + 0.05, f64::from(dirv[2]) * 0.1];
    game.spawn_item(result, at, speed_out, 10);
    true
}

fn crafter_click(game: &mut Game<'_>, pos: [i32; 3], held_item: Option<String>) -> bool {
    if held_item.as_deref() == Some("create:wrench") {
        let now = game.native_block_property(pos, "pointing").unwrap_or_default();
        let at = POINTS.iter().position(|p| *p == now.as_str()).unwrap_or(0);
        let next = POINTS[(at + 1) % 4];
        game.native_set_block_property(pos, "pointing", next);
        say(game, &(String::from("Mechanical Crafter points ") + next));
        return true;
    }
    if let Some(item) = held_item {
        let count = game.held().map(|h| h.count).unwrap_or(0);
        let took = game
            .native_with_mod_block_entity(pos, move |e| {
                let have = pair_list(&e.nbt, "ci").first().cloned();
                let room = match &have {
                    Some((it, c)) => {
                        if *it == item {
                            64u32.saturating_sub(*c)
                        } else {
                            0
                        }
                    }
                    None => 64,
                };
                let take = room.min(count);
                if take == 0 {
                    return 0;
                }
                let total = have.map(|v| v.1).unwrap_or(0) + take;
                pair_store(&mut e.nbt, "ci", &[(item, total)]);
                take
            })
            .unwrap_or(0);
        if took > 0 {
            game.native_block_entity_sync(pos);
            take_held(game, took);
            return true;
        }
        return false;
    }
    let back = game
        .native_with_mod_block_entity(pos, |e| {
            let v = pair_list(&e.nbt, "ci");
            pair_store(&mut e.nbt, "ci", &[]);
            v
        })
        .unwrap_or_default();
    if back.is_empty() {
        return false;
    }
    game.native_block_entity_sync(pos);
    give_player(game, pos, &back);
    true
}

// Breaking a machine gives back what it holds.
fn spill_lists(game: &mut Game<'_>, pos: [i32; 3], keys: &[&str]) {
    let mut all: Vec<(String, u32)> = Vec::new();
    for k in keys {
        let key = String::from(*k);
        let got = game.native_with_mod_block_entity(pos, move |e| pair_list(&e.nbt, &key)).unwrap_or_default();
        all.extend(got);
    }
    let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
    for e in all {
        game.spawn_item(stack_of(&e.0, e.1), at, [0.0, 0.1, 0.0], 10);
    }
}

// ---- hooks ----

pub fn block_entity(block: &str, entity: &mut Option<(String, Compound)>) {
    if entity.is_none() && block == DEPOT_BLOCK {
        *entity = Some((DEPOT_BE.to_string(), Compound::new()));
        return;
    }
    if entity.is_none() && is_fluid_block(block) {
        *entity = Some((FLUID_BE.to_string(), Compound::new()));
        return;
    }
    if entity.is_none() && kind_of(block).is_some() {
        let mut c = Compound::new();
        set_state(&mut c, 0.0, NONE_HOPS, None);
        *entity = Some((BE_ID.to_string(), c));
    }
}

pub fn ticks(block: &str, ticks: &mut bool) {
    if kind_of(block).is_some() || is_fluid_block(block) {
        *ticks = true;
    }
}

pub fn tick(game: &mut Game<'_>, pos: [i32; 3], entity: &mut ModBlockEntity) {
    if entity.id == FLUID_BE {
        fluid_tick(game, pos, entity);
        return;
    }
    if entity.id != BE_ID {
        return;
    }
    let Some(me) = node_at(game, pos) else { return };
    let old = get_f32(&entity.nbt, "speed");
    if me.kind == Kind::Motor {
        if let Ok(mut slot) = COMMIT.lock() {
            if let Some((key, value)) = *slot {
                if key == pos {
                    put_int(&mut entity.nbt, "rpm", value.clamp(-MOTOR_MAX_RPM, MOTOR_MAX_RPM));
                    *slot = None;
                }
            }
        }
    }
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
    if me.kind == Kind::Press && press_tick(game, pos, &mut entity.nbt, out.speed) {
        game.native_block_entity_sync(pos);
    }
    if me.kind == Kind::Millstone && mill_tick(game, pos, &mut entity.nbt, out.speed) {
        game.native_block_entity_sync(pos);
    }
    if me.kind == Kind::Saw && saw_tick(game, pos, &mut entity.nbt, &me, out.speed) {
        game.native_block_entity_sync(pos);
    }
    if me.kind == Kind::Crusher && crush_tick(game, pos, &mut entity.nbt, &me, out.speed) {
        game.native_block_entity_sync(pos);
    }
    if me.kind == Kind::Crafter && crafter_tick(game, pos, &mut entity.nbt, out.speed) {
        game.native_block_entity_sync(pos);
    }
    if me.kind == Kind::Pump {
        pump_step(game, pos, &mut entity.nbt, &me, out.speed);
    }
    if me.kind == Kind::Valve {
        valve_tick(game, pos, &mut entity.nbt, out.speed);
    }
    if me.kind == Kind::Belt {
        if belt_tick(game, pos, &mut entity.nbt, &me, out.speed) {
            game.native_block_entity_sync(pos);
        }
        if (game.ticks as i64 + i64::from(pos[0]) + i64::from(pos[2])).rem_euclid(5) == 0 {
            belt_validate(game, pos, &me);
        }
    }
    if matches!(me.kind, Kind::Clutch | Kind::Gearshift)
        && (game.ticks as i64 + i64::from(pos[0]) + i64::from(pos[2])).rem_euclid(2) == 0
    {
        redstone_sync(game, pos);
    }
    if me.is_chain() && (game.ticks as i64 + i64::from(pos[0]) + i64::from(pos[2])).rem_euclid(5) == 0 {
        chain_part(game, pos, &me);
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
    if entity.id == DEPOT_BE {
        let mut c = Compound::new();
        depot_put(&mut c, depot_content(&entity.nbt));
        *tag = Some(c);
        return;
    }
    if entity.id == FLUID_BE {
        let mut c = Compound::new();
        let fl = get_text(&entity.nbt, "fl");
        c.insert(JavaString::from("fl"), Tag::String(JavaString::from(fl.as_str())));
        put_int(&mut c, "fa", get_int(&entity.nbt, "fa", 0));
        *tag = Some(c);
        return;
    }
    if entity.id == BE_ID {
        let mut c = Compound::new();
        put_f32(&mut c, "speed", get_f32(&entity.nbt, "speed"));
        put_f32(&mut c, "pp", get_f32(&entity.nbt, "pp"));
        let bi = get_text(&entity.nbt, "bi");
        c.insert(JavaString::from("bi"), Tag::String(JavaString::from(bi.as_str())));
        *tag = Some(c);
    }
}

// Clicks. Each use reaches this hook once per hand, so the depot, belt and connector ignore a repeat of
// the same click. The creative motor is left to the game, which opens its screen the normal way.
static LAST_CLICK: Mutex<Option<(i64, [i32; 3])>> = Mutex::new(None);

fn fresh_click(game: &Game<'_>, pos: [i32; 3]) -> bool {
    let now = game.ticks as i64;
    match LAST_CLICK.lock() {
        Ok(mut last) => {
            if let Some((t, p)) = *last {
                if p == pos && now - t <= 3 {
                    return false;
                }
            }
            *last = Some((now, pos));
            true
        }
        Err(_) => true,
    }
}

fn stack_of(item: &str, count: u32) -> ItemStack {
    ItemStack { item: Arc::from(item), count, damage: 0, extra: None }
}

// Where taken items appear: at the player eyes, so they are picked up at once.
fn at_player(game: &Game<'_>, pos: [i32; 3]) -> [f64; 3] {
    match game.eye_position() {
        Some(p) => p,
        None => [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 1.0, f64::from(pos[2]) + 0.5],
    }
}

// A click on the depot: a held stack goes on it (more of the same item adds to what lies there); with
// an empty hand, one item at a time comes back.
fn depot_click(game: &mut Game<'_>, pos: [i32; 3]) {
    let Some(content) = depot_at(game, pos) else { return };
    let held = game.held().cloned();
    match (content, held) {
        (Some((item, count)), Some(h)) if h.count > 0 && &*h.item == item.as_str() => {
            let total = (count + h.count).min(64);
            depot_set(game, pos, Some((item, total)));
            *game.held_mut() = None;
        }
        (Some((item, count)), _) => {
            let rest = if count > 1 { Some((item.clone(), count - 1)) } else { None };
            depot_set(game, pos, rest);
            let at = at_player(game, pos);
            game.spawn_item(stack_of(&item, 1), at, [0.0, 0.0, 0.0], 0);
        }
        (None, Some(h)) if h.count > 0 => {
            depot_set(game, pos, Some((h.item.to_string(), h.count)));
            *game.held_mut() = None;
        }
        _ => {}
    }
}

// The held stack is put on the belt piece, where there is room.
fn belt_put(game: &mut Game<'_>, pos: [i32; 3]) {
    let held = game.held().cloned();
    let Some(h) = held else { return };
    if h.count == 0 {
        return;
    }
    let item = h.item.to_string();
    let count = h.count;
    let ok = game
        .native_with_mod_block_entity(pos, move |e| {
            let mut v = belt_items(&e.nbt);
            let Some(p) = belt_free_spot(&v) else { return false };
            v.push(BeltItem { item, count, p });
            belt_store(&mut e.nbt, &v);
            true
        })
        .unwrap_or(false);
    if ok {
        game.native_block_entity_sync(pos);
        *game.held_mut() = None;
    }
}

// An empty hand takes one item off the belt piece, the one nearest the middle.
fn belt_take(game: &mut Game<'_>, pos: [i32; 3]) -> bool {
    let taken = game
        .native_with_mod_block_entity(pos, |e| {
            let mut v = belt_items(&e.nbt);
            if v.is_empty() {
                return None;
            }
            let mut best = 0;
            for i in 0..v.len() {
                if (v[i].p - 0.5).abs() < (v[best].p - 0.5).abs() {
                    best = i;
                }
            }
            let item = v[best].item.clone();
            if v[best].count > 1 {
                v[best].count -= 1;
            } else {
                v.remove(best);
            }
            belt_store(&mut e.nbt, &v);
            Some(item)
        })
        .flatten();
    let Some(item) = taken else { return false };
    game.native_block_entity_sync(pos);
    let at = at_player(game, pos);
    game.spawn_item(stack_of(&item, 1), at, [0.0, 0.0, 0.0], 0);
    true
}

// The depot is broken: what lies on it drops. A belt piece is broken: the whole belt comes apart.
pub fn destroyed(game: &mut Game<'_>, pos: [i32; 3], state: BlockStateId) {
    let name = game.res.facts().get(state).name.to_string();
    if name == DEPOT_BLOCK {
        if let Some(Some((item, count))) = depot_at(game, pos) {
            let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
            game.spawn_item(stack_of(&item, count), at, [0.0, 0.1, 0.0], 10);
        }
    } else if name == "create:millstone" {
        spill_lists(game, pos, &["mi", "mo"]);
    } else if name == "create:mechanical_crafter" {
        spill_lists(game, pos, &["ci"]);
    } else if name == "create:belt" {
        belt_spill(game, pos);
        belt_dismantle(game, pos, Some(pos));
    }
}

// A click on the depot puts the held stack on it or takes it off; on a belt it puts the held stack on the
// belt; on the hand crank it turns the crank (hold the use button to keep it turning); on any other kinetic
// block it tells the player its speed and the network stress.
pub fn right_click(game: &mut Game<'_>, pos: [i32; 3], canceled: &mut bool) {
    let held_item = game.held().map(|h| h.item.to_string());
    if held_item.as_deref() == Some("create:belt_connector") {
        if fresh_click(game, pos) {
            belt_connector(game, pos);
        }
        *canceled = true;
        return;
    }
    let name = game.native_block(pos);
    if name == MOTOR_BLOCK {
        if let Ok(mut at) = MOTOR_AT.lock() {
            *at = Some(pos);
        }
        return;
    }
    if name == TANK || name == SPOUT {
        if fresh_click(game, pos) {
            if bucket_click(game, pos) {
                *canceled = true;
                return;
            }
            fluid_info(game, pos, &name);
        }
        return;
    }
    if name == "create:millstone" || name == "create:mechanical_crafter" {
        if fresh_click(game, pos) {
            let done = if name == "create:millstone" {
                millstone_click(game, pos)
            } else {
                crafter_click(game, pos, held_item.clone())
            };
            if done {
                *canceled = true;
                return;
            }
        } else {
            *canceled = true;
            return;
        }
    }
    if name == DEPOT_BLOCK {
        if fresh_click(game, pos) {
            depot_click(game, pos);
        }
        *canceled = true;
        return;
    }
    if name == "create:belt" && held_item.is_some() {
        if fresh_click(game, pos) {
            belt_put(game, pos);
        }
        *canceled = true;
        return;
    }
    if name == "create:belt" && held_item.is_none() {
        if fresh_click(game, pos) && belt_take(game, pos) {
            *canceled = true;
            return;
        }
    }
    let Some(me) = node_at(game, pos) else { return };
    if me.kind == Kind::Handle {
        let now = game.ticks as i32;
        let until = now.wrapping_add(CRANK_TICKS);
        let _ = game.native_with_mod_block_entity(pos, move |e| {
            let last = get_int(&e.nbt, "until", now.wrapping_sub(100));
            if now.wrapping_sub(last) > 10 {
                let d = get_int(&e.nbt, "hdir", 1);
                put_int(&mut e.nbt, "hdir", -d);
            }
            put_int(&mut e.nbt, "until", until);
        });
    }
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
    if me.kind == Kind::Pump {
        let held = game.native_with_mod_block_entity(pos, |e| (get_text(&e.nbt, "fl"), get_int(&e.nbt, "fa", 0)));
        if let Some((fl, amt)) = held {
            let s = sect();
            let what = if amt > 0 { fluid_title(&fl) } else { "empty" };
            let line = s.clone() + "7Pump holds: " + &s + "f" + what + " " + &amt.to_string() + &s + "7 mB";
            game.native_chat(&line);
        }
    }
    if me.kind == Kind::Valve {
        let open = game.native_block_property(pos, "enabled").map(|v| v == "true").unwrap_or(false);
        let s = sect();
        let line = s.clone() + "7Fluid Valve: " + &s + "f" + if open { "open" } else { "closed" } + &s + "7 (turn it one way to open, the other to close)";
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
    if block == DEPOT_BLOCK {
        if let Some((item, count)) = depot_content(&entity.nbt) {
            let stack = ItemStack { item: Arc::from(item.as_str()), count, damage: 0, extra: None };
            let is_block = game.pack.assets.registry.default_state(&item).is_some();
            draw.push();
            if is_block {
                draw.translate([0.5, 0.9, 0.5]);
            } else {
                // A flat item lies on the depot as in Create.
                draw.translate([0.5, 0.77, 0.5]);
                draw.rotate_about([1.0, 0.0, 0.0], 90.0, [0.0, 0.0, 0.0]);
            }
            game.native_draw_item(draw, &stack, DisplayContext::Ground, None, pos[0] ^ pos[2]);
            draw.pop();
        }
        draw.keep();
        return;
    }
    if block == TANK {
        let fl = get_text(&entity.nbt, "fl");
        let amt = get_int(&entity.nbt, "fa", 0);
        if amt > 0 && !fl.is_empty() {
            let level = ((amt * 8 + TANK_CAP - 1) / TANK_CAP).clamp(1, 8).to_string();
            game.native_draw_block(draw, "create:fluid_level_part", &[("fluid", fl.as_str()), ("level", level.as_str())]);
        }
        draw.keep();
        return;
    }
    if block == VALVE {
        let Some(f) = game.native_block_state_property(state, "facing") else { return };
        let Some(front) = parse_dir(&f.to_string()) else { return };
        let first = game.native_block_state_property(state, "axis_along_first").map(|v| v.to_string() == "true").unwrap_or(false);
        let speed = f64::from(get_f32(&entity.nbt, "speed"));
        through_shaft(game, pos, valve_axis(axis_of(front), first), speed, draw);
        if speed == 0.0 {
            draw.keep();
        }
        return;
    }
    if block == "create:belt" {
        let Some(f) = game.native_block_state_property(state, "facing") else { return };
        let f = f.to_string();
        let Some(front) = parse_dir(&f) else { return };
        let part = game.native_block_state_property(state, "part").map(|v| v.to_string()).unwrap_or_default();
        let speed = f64::from(get_f32(&entity.nbt, "speed"));
        if part == "start" || part == "middle" || part == "end" {
            // The belt surface is one of eight pictures, each shifted a little further along the belt.
            let t = draw.game_time as f64 + f64::from(draw.partial);
            let frame = ((-t * speed / 60.0).floor() as i64).rem_euclid(8).to_string();
            game.native_draw_block(
                draw,
                "create:belt_part",
                &[("facing", f.as_str()), ("part", part.as_str()), ("frame", frame.as_str())],
            );
        }
        if part != "middle" {
            let a = axis_of(front);
            through_shaft(game, pos, if a == 0 { 2 } else { 0 }, speed, draw);
        }
        // the items riding this piece, moving on between ticks
        let step = ((speed / 480.0) as f32) * draw.partial;
        for b in belt_items(&entity.nbt) {
            let p = (b.p + step).clamp(0.0, 1.0);
            let line = axis_of(front);
            let mut at = [0.5, BELT_TOP, 0.5];
            at[line] = if front[line] > 0 { p } else { 1.0 - p };
            let stack = ItemStack { item: Arc::from(b.item.as_str()), count: b.count, damage: 0, extra: None };
            let is_block = game.pack.assets.registry.default_state(&b.item).is_some();
            draw.push();
            draw.translate(at);
            if !is_block {
                draw.rotate_about([1.0, 0.0, 0.0], 90.0, [0.0, 0.0, 0.0]);
            }
            game.native_draw_item(draw, &stack, DisplayContext::Ground, None, pos[0] ^ pos[2] ^ (b.p * 100.0) as i32);
            draw.pop();
        }
        if speed == 0.0 {
            draw.keep();
        }
        return;
    }
    if matches!(block, "create:encased_chain_drive" | "create:adjustable_chain_gearshift") {
        let Some(value) = game.native_block_state_property(state, "axis") else { return };
        let Some(axis) = parse_axis(&value.to_string()) else { return };
        let speed = f64::from(get_f32(&entity.nbt, "speed"));
        through_shaft(game, pos, axis, speed, draw);
        if speed == 0.0 {
            draw.keep();
        }
        return;
    }
    if block == "create:millstone" {
        let speed = f64::from(get_f32(&entity.nbt, "speed"));
        let t = draw.game_time as f64 + f64::from(draw.partial);
        draw.push();
        draw.rotate_about([0.0, 1.0, 0.0], ((speed * 0.3 * t) % 360.0) as f32, [0.5, 0.5, 0.5]);
        game.native_draw_block(draw, "create:millstone_inner_part", &[]);
        draw.pop();
        if speed == 0.0 {
            draw.keep();
        }
        return;
    }
    if block == "create:crushing_wheel" {
        let Some(value) = game.native_block_state_property(state, "axis") else { return };
        let value = value.to_string();
        let Some(axis) = parse_axis(&value) else { return };
        let speed = f64::from(get_f32(&entity.nbt, "speed"));
        let t = draw.game_time as f64 + f64::from(draw.partial);
        let mut about = [0.0f32; 3];
        about[axis] = 1.0;
        draw.push();
        draw.rotate_about(about, ((speed * 0.3 * t) % 360.0) as f32, [0.5, 0.5, 0.5]);
        game.native_draw_block(draw, "create:crushing_wheel_part", &[("axis", value.as_str())]);
        draw.pop();
        if speed == 0.0 {
            draw.keep();
        }
        return;
    }
    if block == "create:mechanical_crafter" {
        let Some(value) = game.native_block_state_property(state, "facing") else { return };
        let Some(front) = parse_dir(&value.to_string()) else { return };
        let speed = f64::from(get_f32(&entity.nbt, "speed"));
        let axis = axis_of(front);
        let mut about = [0.0f32; 3];
        about[axis] = 1.0;
        draw.push();
        draw.rotate_about(about, spin_degrees(pos, axis, speed, draw), [0.5, 0.5, 0.5]);
        game.native_draw_block(draw, "create:cogwheel_shaftless_part", &[("axis", axis_char(axis))]);
        draw.pop();
        if speed == 0.0 {
            draw.keep();
        }
        return;
    }
    if block == "create:mechanical_saw" {
        let Some(value) = game.native_block_state_property(state, "facing") else { return };
        let value = value.to_string();
        let Some(front) = parse_dir(&value) else { return };
        let first = game.native_block_state_property(state, "axis_along_first").map(|v| v == "true").unwrap_or(false);
        let flipped = game.native_block_state_property(state, "flipped").unwrap_or("false").to_string();
        let mut node = Node::new(Kind::Saw, axis_of(front), front);
        node.first = first;
        let speed = f64::from(get_f32(&entity.nbt, "speed"));
        through_shaft(game, pos, node.saw_axis(), speed, draw);
        let mode = if speed > 0.5 {
            "active"
        } else if speed < -0.5 {
            "reversed"
        } else {
            "inactive"
        };
        game.native_draw_block(
            draw,
            "create:saw_blade_part",
            &[
                ("axis_along_first", if first { "true" } else { "false" }),
                ("facing", value.as_str()),
                ("flipped", flipped.as_str()),
                ("mode", mode),
            ],
        );
        if speed == 0.0 {
            draw.keep();
        }
        return;
    }
    if matches!(block, "create:mechanical_drill" | "create:mechanical_press") {
        extra_shafts(game, pos, block, state, entity, draw);
    }
    if matches!(block, "create:gearbox" | "create:vertical_gearbox" | "create:clutch" | "create:gearshift") {
        submit_halves(game, pos, block, state, entity, draw);
        return;
    }
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
        "create:mechanical_pump" => ("create:pump_cog_part", "facing", "axis", 3),
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
    let part_value = if anim >= 2 { axis_char(axis).to_string() } else { value.clone() };
    let speed = f64::from(get_f32(&entity.nbt, "speed"));
    let t = draw.game_time as f64 + f64::from(draw.partial);
    let degrees = (speed * 0.3 * t) % 360.0;
    draw.push();
    if anim == 1 {
        let phase = f64::from(get_f32(&entity.nbt, "pp"));
        let dy = -0.45 * (1.0 - phase.to_radians().cos());
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
    if anim == 2 {
        for sgn in [-1, 1] {
            let mut d = [0; 3];
            d[axis] = sgn;
            let mut about = [0.0f32; 3];
            about[axis] = 1.0;
            draw.push();
            draw.rotate_about(about, degrees as f32, [0.5, 0.5, 0.5]);
            game.native_draw_block(draw, "create:motor_shaft_part", &[("facing", dir_name(d))]);
            draw.pop();
        }
    }
    if speed == 0.0 {
        draw.keep();
    }
}

// Degrees a part turns by along its shaft at `speed` rpm: Create phase offset, set by the position across the axis.
fn spin_degrees(pos: [i32; 3], axis: usize, speed: f64, draw: &BlockEntityDraw) -> f32 {
    let t = draw.game_time as f64 + f64::from(draw.partial);
    let across: i32 = (0..3).filter(|i| *i != axis).map(|i| pos[i]).sum();
    let offset = if across % 2 == 0 { 22.5 } else { 0.0 };
    ((speed * 0.3 * t) % 360.0) as f32 + offset
}

// A whole shaft through the block along `axis` (chain drives, the press).
fn through_shaft(game: &Game<'_>, pos: [i32; 3], axis: usize, speed: f64, draw: &mut BlockEntityDraw) {
    let mut about = [0.0f32; 3];
    about[axis] = 1.0;
    draw.push();
    draw.rotate_about(about, spin_degrees(pos, axis, speed, draw), [0.5, 0.5, 0.5]);
    game.native_draw_block(draw, "create:shaft_part", &[("axis", axis_char(axis))]);
    draw.pop();
}

// A half shaft sticking out of the block towards `d`.
fn half_shaft(game: &Game<'_>, pos: [i32; 3], d: [i32; 3], speed: f64, draw: &mut BlockEntityDraw) {
    let axis = axis_of(d);
    let mut about = [0.0f32; 3];
    about[axis] = 1.0;
    draw.push();
    draw.rotate_about(about, spin_degrees(pos, axis, speed, draw), [0.5, 0.5, 0.5]);
    game.native_draw_block(draw, "create:motor_shaft_part", &[("facing", dir_name(d))]);
    draw.pop();
}

// The drill has its shaft at the back, the press one across it.
fn extra_shafts(
    game: &Game<'_>,
    pos: [i32; 3],
    block: &str,
    state: BlockStateId,
    entity: &ModBlockEntity,
    draw: &mut BlockEntityDraw,
) {
    let Some(value) = game.native_block_state_property(state, "facing") else { return };
    let Some(front) = parse_dir(&value.to_string()) else { return };
    let speed = f64::from(get_f32(&entity.nbt, "speed"));
    if block == "create:mechanical_drill" {
        half_shaft(game, pos, neg(front), speed, draw);
    } else {
        through_shaft(game, pos, axis_of(front), speed, draw);
    }
}

// The shaft halves of a gearbox (one on each side across its axis) or of a clutch or
// gearshift (one at each end), each turning as the block does at its own port.
fn submit_halves(
    game: &Game<'_>,
    pos: [i32; 3],
    block: &str,
    state: BlockStateId,
    entity: &ModBlockEntity,
    draw: &mut BlockEntityDraw,
) {
    let Some(value) = game.native_block_state_property(state, "axis") else { return };
    let Some(axis) = parse_axis(&value.to_string()) else { return };
    let src = get_parent(&entity.nbt).unwrap_or([0; 3]);
    let speed = f64::from(get_f32(&entity.nbt, "speed"));
    let t = draw.game_time as f64 + f64::from(draw.partial);
    let gearbox = block == "create:gearbox" || block == "create:vertical_gearbox";
    for a in 0..3 {
        for sgn in [-1, 1] {
            let mut d = [0; 3];
            d[a] = sgn;
            if gearbox == (a == axis) {
                continue;
            }
            let m = if gearbox { f64::from(gearbox_mod(src, d)) } else { 1.0 };
            let across: i32 = (0..3).filter(|i| *i != a).map(|i| pos[i]).sum();
            let offset = if across % 2 == 0 { 22.5 } else { 0.0 };
            let degrees = ((speed * m * 0.3 * t) % 360.0) as f32 + offset;
            let mut about = [0.0f32; 3];
            about[a] = 1.0;
            draw.push();
            draw.rotate_about(about, degrees, [0.5, 0.5, 0.5]);
            game.native_draw_block(draw, "create:motor_shaft_part", &[("facing", dir_name(d))]);
            draw.pop();
        }
    }
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
    if kind == Kind::Millstone {
        return Some(Node::new(kind, 1, [0; 3]));
    }
    if kind == Kind::Valve {
        let front = parse_dir(props.get("facing")?)?;
        let first = props.get("axis_along_first").map(|v| v == "true").unwrap_or(false);
        return Some(Node::new(kind, valve_axis(axis_of(front), first), front));
    }
    if kind.uses_facing() {
        let front = parse_dir(props.get("facing")?)?;
        let mut node = Node::new(kind, axis_of(front), front);
        if kind == Kind::Belt {
            node.shaft = props.get("part").map(|v| v != "middle").unwrap_or(false);
        }
        if kind == Kind::Saw {
            node.first = props.get("axis_along_first").map(|v| v == "true").unwrap_or(false);
        }
        Some(node)
    } else {
        let axis = parse_axis(props.get("axis")?)?;
        let mut node = Node::new(kind, axis, [0; 3]);
        if node.is_chain() {
            node.first = props.get("axis_along_first").map(|v| v == "true").unwrap_or(false);
        }
        Some(node)
    }
}

// A new chain drive turns its chain towards a chain drive beside it that has the same axis.
fn chain_first_for(level: &GameLevel<'_>, at: BlockPos, axis: usize) -> bool {
    for a in 0..3 {
        if a == axis {
            continue;
        }
        for sgn in [-1, 1] {
            let mut d = [0; 3];
            d[a] = sgn;
            let nb = BlockPos::new(at.x + d[0], at.y + d[1], at.z + d[2]);
            if let Some(n) = node_from_state(level, level.block_state(nb)) {
                if n.is_chain() && n.axis == axis && n.chain_axis() == a {
                    return n.first;
                }
            }
        }
    }
    false
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
    if name == PIPE || name == TANK {
        fluid_placement(level, state, context, &name, answer);
        return;
    }
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
    let new = if kind == Kind::Gearbox {
        // A gearbox has ports on the four sides across its axis. The plain one lies flat (axis y);
        // the vertical one (ports above and below) takes the horizontal axis you look along.
        let vertical = name == "create:vertical_gearbox";
        let mut axis = if vertical {
            let a = axis_of(look);
            if a == 1 { 0 } else { a }
        } else {
            1
        };
        if let Some(d) = feeding.first() {
            let fa = axis_of(*d);
            if vertical && fa == axis {
                axis = if fa == 0 { 2 } else { 0 };
            }
        }
        level.native_state_with_property(state, "axis", axis_char(axis))
    } else if kind == Kind::Valve {
        // The pipe runs along the axis you look along; the shaft goes across it, towards a shaft beside the valve.
        let mut pipe = axis_of(look);
        let turn: Option<usize> = feeding.first().map(|d| axis_of(*d));
        if let Some(r) = turn {
            if pipe == r {
                pipe = if r == 0 { 2 } else { 0 };
            }
        }
        let first = match turn {
            Some(r) => valve_axis(pipe, true) == r,
            None => true,
        };
        let mut dirv = [0; 3];
        dirv[pipe] = if look[pipe] != 0 { look[pipe] } else { 1 };
        level
            .native_state_with_property(state, "facing", dir_name(dirv))
            .and_then(|s| level.native_state_with_property(s, "axis_along_first", if first { "true" } else { "false" }))
            .and_then(|s| level.native_state_with_property(s, "enabled", "true"))
    } else if kind == Kind::Saw {
        let mut facing = neg(look);
        if context.sneaking {
            facing = look;
        }
        let f = axis_of(facing);
        let mut across = (0..3).filter(|a| *a != f);
        let low = across.next().unwrap_or(0);
        let mut first = false;
        if let Some(d) = feeding.first() {
            first = axis_of(*d) == low;
        }
        let with_facing = level.native_state_with_property(state, "facing", dir_name(facing));
        with_facing.and_then(|s| level.native_state_with_property(s, "axis_along_first", if first { "true" } else { "false" }))
    } else if kind == Kind::Crafter {
        let mut facing = neg(nearest_looking(context.yaw, 0.0));
        if let Some(d) = feeding.first() {
            if d[1] == 0 {
                facing = neg(*d);
            }
        }
        let right = right_vec(facing);
        let mut pointing = "up";
        for d in [[0, 1, 0], [0, -1, 0], right, neg(right)] {
            let nb = BlockPos::new(at.x + d[0], at.y + d[1], at.z + d[2]);
            let st = level.block_state(nb);
            if level.facts.get(st).name.to_string() != "create:mechanical_crafter" {
                continue;
            }
            let Some(props) = level.registry.state(st).map(|s| &s.properties) else { continue };
            if props.get("facing").and_then(|v| parse_dir(v)) == Some(facing) {
                if let Some(p) = props.get("pointing") {
                    pointing = POINTS.iter().copied().find(|x| *x == p.as_str()).unwrap_or("up");
                    break;
                }
            }
        }
        let with_facing = level.native_state_with_property(state, "facing", dir_name(facing));
        with_facing.and_then(|s| level.native_state_with_property(s, "pointing", pointing))
    } else if kind.uses_facing() {
        // Directions where the neighbour can take a shaft end.
        let mut facing = match kind {
            Kind::Motor => neg(look),
            Kind::Crank | Kind::Handle => neg(look),
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
                Kind::Crank | Kind::Drill | Kind::Handle => neg(*d),
                _ => facing,
            };
        }
        if kind == Kind::Wheel {
            facing = look;
        }
        if kind == Kind::Pump && !context.sneaking {
            // turn on the axis of a cogwheel beside it, as the pump meshes like a small cog
            'seek: for a in 0..3 {
                for s in [-1, 1] {
                    let mut d = [0; 3];
                    d[a] = s;
                    let nb = BlockPos::new(at.x + d[0], at.y + d[1], at.z + d[2]);
                    if let Some(n) = node_from_state(level, level.block_state(nb)) {
                        if n.is_cog() && n.axis != a {
                            let mut f = [0; 3];
                            f[n.axis] = if look[n.axis] != 0 { look[n.axis] } else { 1 };
                            facing = f;
                            break 'seek;
                        }
                    }
                }
            }
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
        let mut st = level.native_state_with_property(state, "axis", axis_char(axis));
        if matches!(kind, Kind::Chain | Kind::AdjChain) {
            if let Some(s1) = st {
                let first = chain_first_for(level, at, axis);
                if let Some(s2) = level.native_state_with_property(s1, "axis_along_first", if first { "true" } else { "false" }) {
                    st = Some(s2);
                }
            }
        }
        st
    };
    if let Some(s) = new {
        *answer = Some(Some(s));
    }
}

// ---- the creative motors screen ----
// As on Java: "Generated Speed in RPM" over a long track with a handle that shows the speed. Press on
// the track and slide, let go to confirm; two buttons give the direction. The menu is a one-slot menu
// (data/create/horizon/menu/creative_motor.json, its slot hidden) whose three data slots are the
// speed and the stress of its network (asked for, available). A press on the track sets the speed at
// once (through the menu button, so it works for guests); sliding is followed by the screen and
// handed to the motor when the press ends (in a world of your own).

struct Btn {
    id: i32,
    label: &'static str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

const SLIDER_X: f32 = 46.0;
const SLIDER_Y: f32 = 30.0;
const SLIDER_W: f32 = 196.0;
const SLIDER_H: f32 = 30.0;

const BUTTONS: [Btn; 2] = [
    Btn { id: 20, label: "CW", x: 12.0, y: 30.0, w: 24.0, h: 14.0 },
    Btn { id: 21, label: "CCW", x: 12.0, y: 46.0, w: 24.0, h: 14.0 },
];

static DRAG: AtomicBool = AtomicBool::new(false);
static MOTOR_AT: Mutex<Option<[i32; 3]>> = Mutex::new(None);
static COMMIT: Mutex<Option<([i32; 3], i32)>> = Mutex::new(None);

// The speed under a pointer at x (GUI pixels from the screen left).
fn rpm_at(x: f32) -> i32 {
    ((x - SLIDER_X) / SLIDER_W * 512.0 - 256.0).round().clamp(-256.0, 256.0) as i32
}

fn on_track(at: [f32; 2]) -> bool {
    at[0] >= SLIDER_X - 6.0 && at[0] <= SLIDER_X + SLIDER_W + 6.0 && at[1] >= SLIDER_Y - 3.0 && at[1] < SLIDER_Y + SLIDER_H + 3.0
}

// What a press at `at` (GUI pixels from the screen top left) does: a menu button id.
fn hit(at: [f32; 2]) -> Option<i32> {
    if on_track(at) {
        return Some(1000 + rpm_at(at[0]) + MOTOR_MAX_RPM);
    }
    BUTTONS
        .iter()
        .find(|b| at[0] >= b.x && at[0] < b.x + b.w && at[1] >= b.y && at[1] < b.y + b.h)
        .map(|b| b.id)
}

fn local(screen: &NativeScreenView<'_>) -> [f32; 2] {
    [screen.mouse[0] - screen.rect[0], screen.mouse[1] - screen.rect[1]]
}

pub fn mouse_pressed(screen: &NativeScreenView<'_>, button: i32) {
    if button != 0 || screen.menu != Some(MOTOR_MENU) {
        return;
    }
    if on_track(local(screen)) {
        DRAG.store(true, Ordering::Relaxed);
    }
}

pub fn mouse_released(screen: &NativeScreenView<'_>, button: i32) {
    if button != 0 || !DRAG.swap(false, Ordering::Relaxed) {
        return;
    }
    let at = MOTOR_AT.lock().ok().and_then(|g| *g);
    if let Some(key) = at {
        let value = rpm_at(local(screen)[0]);
        if let Ok(mut slot) = COMMIT.lock() {
            *slot = Some((key, value));
        }
    }
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
            width: 256.0,
            height: 182.0,
            slots: vec![None],
            inventory_y: 98.0,
            inventory_x: 48.0,
            own_labels: true,
        });
    }
}

const GOLD: u32 = 0xFFC9984F;
const GOLD_DARK: u32 = 0xFF7A5427;
const BEIGE: u32 = 0xFFE9D3A0;
const MAROON: u32 = 0xFF2A1210;

fn centred(game: &Game<'_>, batch: &mut HudBatch, text: &str, centre: f32, y: f32, color: u32) {
    let width = game.native_text_width(text);
    game.native_text(batch, text, [centre - width / 2.0, y], color, false);
}

pub fn screen_draw(game: &Game<'_>, block: &str, data: &[i32], batch: &mut HudBatch, origin: [f32; 2]) {
    if block != MOTOR_BLOCK {
        return;
    }
    let setting = data.first().copied().unwrap_or(MOTOR_DEFAULT_RPM);
    let load = data.get(1).copied().unwrap_or(0);
    let cap = data.get(2).copied().unwrap_or(0);
    let cursor = game.native_cursor();
    let dragging = DRAG.load(Ordering::Relaxed);
    let rpm = if dragging { rpm_at(cursor[0] - origin[0]) } else { setting };
    let (ox, oy) = (origin[0], origin[1]);
    // the dark board the controls sit on
    game.native_fill(batch, [ox + 4.0, oy + 4.0, 248.0, 82.0], 0xFF1B100D);
    centred(game, batch, "Generated Speed in RPM", ox + 128.0, oy + 12.0, 0xE9D3A0);
    // the direction buttons
    game.native_fill(batch, [ox + 10.0, oy + 28.0, 28.0, 34.0], GOLD);
    for b in BUTTONS.iter() {
        let on = (b.id == 20 && rpm >= 0) || (b.id == 21 && rpm < 0);
        let over = hit([cursor[0] - ox, cursor[1] - oy]) == Some(b.id);
        let face = if on { 0xFFF3E3B8 } else if over { 0xFFD8BC83 } else { BEIGE };
        game.native_fill(batch, [ox + b.x, oy + b.y, b.w, b.h], GOLD_DARK);
        game.native_fill(batch, [ox + b.x + 1.0, oy + b.y + 1.0, b.w - 2.0, b.h - 2.0], face);
        let w = game.native_text_width(b.label);
        game.native_text(batch, b.label, [ox + b.x + (b.w - w) / 2.0, oy + b.y + 3.0], 0x3A2412, false);
    }
    // the track: ticks every 32 rpm, a brighter one at zero
    game.native_fill(batch, [ox + SLIDER_X - 3.0, oy + SLIDER_Y - 3.0, SLIDER_W + 6.0, SLIDER_H + 6.0], GOLD);
    game.native_fill(batch, [ox + SLIDER_X - 1.0, oy + SLIDER_Y - 1.0, SLIDER_W + 2.0, SLIDER_H + 2.0], MAROON);
    for i in 0..=16 {
        let x = ox + SLIDER_X + i as f32 * SLIDER_W / 16.0;
        let tick = if i == 8 { 0xFF8A5A3A } else { 0xFF4A2A22 };
        game.native_fill(batch, [x - 0.5, oy + SLIDER_Y + 2.0, 1.0, 7.0], tick);
        game.native_fill(batch, [x - 0.5, oy + SLIDER_Y + SLIDER_H - 9.0, 1.0, 7.0], tick);
    }
    // the handle with the speed on it
    let value = rpm.clamp(-MOTOR_MAX_RPM, MOTOR_MAX_RPM);
    let thumb = ox + SLIDER_X + (value + MOTOR_MAX_RPM) as f32 / 512.0 * SLIDER_W;
    game.native_fill(batch, [thumb - 19.0, oy + SLIDER_Y + 5.0, 38.0, 20.0], GOLD_DARK);
    game.native_fill(batch, [thumb - 18.0, oy + SLIDER_Y + 6.0, 36.0, 18.0], BEIGE);
    let label = value.to_string();
    let w = game.native_text_width(&label);
    game.native_text(batch, &label, [thumb - w / 2.0, oy + SLIDER_Y + 11.0], 0x3A2412, false);
    centred(game, batch, "Press, slide and let go to confirm", ox + 128.0, oy + 68.0, 0xCDBA8C);
    let stress = "Stress: ".to_string() + &load.to_string() + " / " + &cap.to_string() + " SU";
    let color = if load > cap { 0xE05050 } else { 0xA89C78 };
    centred(game, batch, &stress, ox + 128.0, oy + 77.0, color);
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
        n if n >= 1000 => n - 1000 - MOTOR_MAX_RPM,
        20 => cur.abs(),
        21 => -cur.abs(),
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
        Node::new(kind, axis, [0; 3])
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
        let motor = Node::new(Kind::Motor, 0, [1, 0, 0]);
        assert_eq!(link(&shaft(0), [-1, 0, 0], &motor), Some(1.0));
        assert_eq!(link(&shaft(0), [1, 0, 0], &motor), None);
    }

    #[test]
    fn crank_and_drill_take_a_shaft_at_their_back() {
        let crank = Node::new(Kind::Crank, 1, [0, 1, 0]);
        // a crank facing up has its shaft end down: a shaft below it joins, one above does not
        assert_eq!(link(&shaft(1), [0, 1, 0], &crank), Some(1.0)); // crank above the shaft
        assert_eq!(link(&shaft(1), [0, -1, 0], &crank), None); // crank below the shaft
        assert_eq!(link(&crank, [0, -1, 0], &shaft(1)), Some(1.0));
        let drill = Node::new(Kind::Drill, 2, [0, 0, -1]);
        assert_eq!(link(&drill, [0, 0, 1], &shaft(2)), Some(1.0));
        assert_eq!(link(&drill, [0, 0, -1], &shaft(2)), None);
    }

    #[test]
    fn press_takes_a_shaft_along_its_facing() {
        let press = Node::new(Kind::Press, 2, [0, 0, -1]);
        assert_eq!(link(&press, [0, 0, 1], &shaft(2)), Some(1.0));
        assert_eq!(link(&press, [0, 0, -1], &shaft(2)), Some(1.0));
        assert_eq!(link(&press, [1, 0, 0], &shaft(0)), None);
    }

    #[test]
    fn gearbox_outputs() {
        // input from the west (src = west): east is straight through (reversed), north/south are side outputs
        let west = [-1, 0, 0];
        assert_eq!(gearbox_mod(west, [-1, 0, 0]), 1.0);
        assert_eq!(gearbox_mod(west, [1, 0, 0]), -1.0);
        assert_eq!(gearbox_mod(west, [0, 0, 1]), 1.0);
        assert_eq!(gearbox_mod(west, [0, 0, -1]), -1.0);
        assert_eq!(gearbox_mod([0; 3], [1, 0, 0]), 1.0);
    }

    #[test]
    fn gearbox_has_ports_across_its_axis() {
        let gb = node(Kind::Gearbox, 1);
        assert!(gb.has_end([1, 0, 0]));
        assert!(gb.has_end([0, 0, -1]));
        assert!(!gb.has_end([0, 1, 0]));
    }

    #[test]
    fn chain_drives_link_along_their_chain_only() {
        let mut a = node(Kind::Chain, 1);
        let mut b = node(Kind::Chain, 1);
        a.first = true;
        b.first = true;
        // axis y, first coordinate across it is x (the lower of x and z)
        assert_eq!(link(&a, [1, 0, 0], &b), Some(1.0));
        assert_eq!(link(&a, [0, 0, 1], &b), None);
        b.first = false;
        assert_eq!(link(&a, [1, 0, 0], &b), None);
    }

    #[test]
    fn pressing_recipes() {
        assert_eq!(pressing("minecraft:iron_ingot"), Some("create:iron_sheet"));
        assert_eq!(pressing("minecraft:sugar_cane"), Some("minecraft:paper"));
        assert_eq!(pressing("minecraft:dirt"), None);
    }

    #[test]
    fn chain_axis_follows_the_lower_axis_first() {
        let mut c = node(Kind::Chain, 1);
        c.first = true;
        assert_eq!(c.chain_axis(), 0);
        c.first = false;
        assert_eq!(c.chain_axis(), 2);
        let mut d = node(Kind::Chain, 0);
        d.first = true;
        assert_eq!(d.chain_axis(), 1);
    }

    #[test]
    fn belt_pieces_pass_rotation_along_the_belt() {
        let mut a = Node::new(Kind::Belt, 0, [1, 0, 0]);
        let mut b = Node::new(Kind::Belt, 0, [1, 0, 0]);
        a.shaft = true;
        b.shaft = false;
        assert_eq!(link(&a, [1, 0, 0], &b), Some(1.0));
        assert_eq!(link(&b, [-1, 0, 0], &a), Some(1.0));
        assert_eq!(link(&a, [0, 0, 1], &b), None);
        // a pulley piece takes a shaft across the belt, a middle piece does not
        assert!(a.has_end([0, 0, 1]));
        assert!(!b.has_end([0, 0, 1]));
        let s = node(Kind::Shaft, 2);
        assert_eq!(link(&a, [0, 0, 1], &s), Some(1.0));
    }

    #[test]
    fn clutch_lets_go_and_gearshift_reverses_while_powered() {
        let mut clutch = node(Kind::Clutch, 0);
        assert!(clutch.has_end([1, 0, 0]));
        clutch.powered = true;
        assert!(!clutch.has_end([1, 0, 0]));
        let mut shift = node(Kind::Gearshift, 0);
        shift.src = [-1, 0, 0];
        assert_eq!(shift_mod(&shift, [1, 0, 0]), 1.0);
        shift.powered = true;
        assert_eq!(shift_mod(&shift, [1, 0, 0]), -1.0);
        assert_eq!(shift_mod(&shift, [-1, 0, 0]), 1.0);
        assert_eq!(link(&shaft(0), [-1, 0, 0], &shift), Some(-1.0));
    }

    #[test]
    fn belt_items_survive_a_round_trip() {
        let mut c = Compound::new();
        let items = vec![
            BeltItem { item: "minecraft:iron_ingot".to_string(), count: 3, p: 0.25 },
            BeltItem { item: "create:pulp".to_string(), count: 1, p: 0.75 },
        ];
        belt_store(&mut c, &items);
        let back = belt_items(&c);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].item, "minecraft:iron_ingot");
        assert_eq!(back[0].count, 3);
        assert!((back[1].p - 0.75).abs() < 1e-6);
        belt_store(&mut c, &[]);
        assert!(belt_items(&c).is_empty());
    }

    #[test]
    fn belt_free_spots_keep_a_gap() {
        assert_eq!(belt_free_spot(&[]), Some(0.5));
        let one = vec![BeltItem { item: "a".to_string(), count: 1, p: 0.5 }];
        assert_eq!(belt_free_spot(&one), Some(0.25));
        let full: Vec<BeltItem> = (0..BELT_MAX).map(|i| BeltItem { item: "a".to_string(), count: 1, p: i as f32 }).collect();
        assert_eq!(belt_free_spot(&full), None);
    }

    #[test]
    fn valve_turns_across_its_pipe() {
        assert_eq!(valve_axis(0, true), 1);
        assert_eq!(valve_axis(0, false), 2);
        assert_eq!(valve_axis(1, true), 0);
        assert_eq!(valve_axis(2, false), 1);
        for pipe in 0..3 {
            assert_ne!(valve_axis(pipe, true), pipe);
            assert_ne!(valve_axis(pipe, false), pipe);
            assert_ne!(valve_axis(pipe, true), valve_axis(pipe, false));
        }
    }

    #[test]
    fn pump_and_valve_shafts() {
        let pump = Node::new(Kind::Pump, 2, [0, 0, -1]);
        assert!(!pump.has_end([0, 0, 1]));
        assert!(!pump.has_end([0, 0, -1]));
        assert!(!pump.has_end([1, 0, 0]));
        // a pump meshes like a small cogwheel on the same axis, side by side
        assert_eq!(link(&pump, [1, 0, 0], &cog(2)), Some(-1.0));
        assert_eq!(link(&cog(2), [-1, 0, 0], &pump), Some(-1.0));
        assert_eq!(link(&pump, [0, 0, 1], &cog(2)), None);
        assert_eq!(link(&pump, [1, 1, 0], &node(Kind::LargeCogwheel, 2)), Some(-2.0));
        let valve = Node::new(Kind::Valve, valve_axis(0, true), [1, 0, 0]);
        assert!(valve.has_end([0, 1, 0]));
        assert!(!valve.has_end([1, 0, 0]));
        let handle = Node::new(Kind::Handle, 1, [0, 1, 0]);
        assert_eq!(link(&shaft(1), [0, 1, 0], &handle), Some(1.0));
        assert_eq!(Kind::Pump.impact() * 16, 64);
    }

    #[test]
    fn buckets_and_fluids_match() {
        assert_eq!(bucket_fluid("minecraft:water_bucket"), Some("water"));
        assert_eq!(fluid_bucket("lava"), Some("minecraft:lava_bucket"));
        assert_eq!(fluid_bucket("steam"), None);
        assert_eq!(DIRS[3], [0, 0, 1]);
        for i in 0..6 {
            assert_eq!(DIRS[i ^ 1], neg(DIRS[i]));
        }
    }

    #[test]
    fn fluid_survives_a_round_trip() {
        let mut c = Compound::new();
        fluid_put(&mut c, "water", 1500);
        assert_eq!(get_text(&c, "fl"), "water");
        assert_eq!(get_int(&c, "fa", 0), 1500);
        fluid_put(&mut c, "water", 0);
        assert_eq!(get_text(&c, "fl"), "");
        assert_eq!(get_int(&c, "fa", 0), 0);
    }

    #[test]
    fn stress_units() {
        assert_eq!(Kind::Crank.capacity(32.0), 256);
        assert_eq!(Kind::Wheel.capacity(8.0), 128);
        assert_eq!(Kind::Motor.capacity(1.0), 16384);
        assert_eq!(Kind::Press.impact() * 32, 256);
    }

    #[test]
    fn the_slider_and_buttons_are_found_where_they_are_drawn() {
        assert_eq!(hit([SLIDER_X, 45.0]), Some(1000));
        assert_eq!(hit([SLIDER_X + SLIDER_W / 2.0, 45.0]), Some(1256));
        assert_eq!(hit([SLIDER_X + SLIDER_W, 45.0]), Some(1512));
        assert_eq!(hit([20.0, 36.0]), Some(20));
        assert_eq!(hit([20.0, 52.0]), Some(21));
        assert_eq!(hit([0.0, 0.0]), None);
        assert_eq!(rpm_at(SLIDER_X + SLIDER_W / 4.0), -128);
    }

    #[test]
    fn looking_direction_follows_yaw_and_pitch() {
        assert_eq!(nearest_looking(0.0, 0.0), [0, 0, 1]);
        assert_eq!(nearest_looking(-90.0, 0.0), [1, 0, 0]);
        assert_eq!(nearest_looking(0.0, 90.0), [0, -1, 0]);
    }
}
