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
use horizon_core::geometry::Direction;
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
// A flat item (a sheet, an ingot, a stick) lies down; an item with a solid model (a block) does not.
fn flat_item(game: &Game<'_>, stack: &ItemStack) -> bool {
    !game.resolve_item(stack, DisplayContext::Ground, None, 0).uses_block_light()
}

// Where the k-th of n stacks lies on a depot, from its middle.
fn depot_spot(n: usize, k: usize) -> [f32; 2] {
    match (n, k) {
        (1, _) => [0.0, 0.0],
        (2, 0) => [-0.22, 0.0],
        (2, _) => [0.22, 0.0],
        (3, 0) => [-0.22, -0.14],
        (3, 1) => [0.22, -0.14],
        (3, _) => [0.0, 0.22],
        (_, 0) => [-0.22, -0.22],
        (_, 1) => [0.22, -0.22],
        (_, 2) => [-0.22, 0.22],
        _ => [0.22, 0.22],
    }
}

// The light a part of a solid block is drawn with: the brightest of the cells around it, in the packing
// of BlockEntityDraw.light (16 times sky light in the low byte, 16 times block light in the next).
fn open_light(game: &Game<'_>, pos: [i32; 3]) -> u32 {
    let (mut block, mut sky) = (0u32, 0u32);
    for a in 0..3 {
        for sgn in [-1, 1] {
            let mut p = pos;
            p[a] += sgn;
            let (b, s) = game.native_light(p);
            block = block.max(u32::from(b));
            sky = sky.max(u32::from(s));
        }
    }
    (sky * 16) | ((block * 16) << 8)
}

// A small local particle (the window only).
fn puff(game: &mut Game<'_>, kind: horizon_client::particles::ParticleType, at: [f64; 3]) {
    use horizon_client::particles::ParticleOptions;
    if !game.native_window() {
        return;
    }
    game.particles.add(&ParticleOptions::simple(kind), at, [0.0, 0.05, 0.0]);
}

const FUNNEL_BE: &str = "create:funnel_be";
const BASIN_BLOCK: &str = "create:basin";
const BASIN_BE: &str = "create:basin_be";
const BASIN_SLOTS: usize = 9;
// A mixing run is 100 units; each tick adds rpm / 16 units, between 1 and 5.
const MIX_UNITS: f32 = 100.0;

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
    Mixer,
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
    Hose,
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
        "create:mechanical_mixer" => Some(Kind::Mixer),
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
        "create:hose_pulley" => Some(Kind::Hose),
        _ => None,
    }
}

impl Kind {
    fn uses_facing(self) -> bool {
        matches!(self, Kind::Motor | Kind::Crank | Kind::Wheel | Kind::Drill | Kind::Press | Kind::Mixer | Kind::Belt | Kind::Pump | Kind::Handle | Kind::Hose | Kind::Saw | Kind::Crafter)
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
            Kind::Mixer => 4,
            Kind::Hose => 4,
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
            Kind::Mixer => "Mechanical Mixer",
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
            Kind::Hose => "Hose Pulley",
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
            Kind::Cogwheel | Kind::LargeCogwheel | Kind::Pump | Kind::Crusher | Kind::Millstone | Kind::Crafter | Kind::Mixer
        )
    }
    // The axis the saw shaft runs along: across its facing, the lower axis when first is set.
    fn saw_axis(&self) -> usize {
        let f = axis_of(self.front);
        if self.front[1] == 0 {
            return f;
        }
        let mut across = (0..3).filter(|a| *a != f);
        let low = across.next().unwrap_or(0);
        let high = across.next().unwrap_or(0);
        if self.first {
            low
        } else {
            high
        }
    }
    // A small cog: the cogwheel, the mixer, pump, millstone and crafter, whose cogs mesh with their neighbours.
    fn small_cog(&self) -> bool {
        matches!(self.kind, Kind::Cogwheel | Kind::Mixer | Kind::Pump | Kind::Millstone | Kind::Crafter)
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
            Kind::Saw => {
                if self.front[1] == 0 {
                    d == neg(self.front)
                } else {
                    unit && d[self.saw_axis()] != 0
                }
            }
            Kind::Hose => d == cw(self.front),
            Kind::Mixer => false,
            Kind::Clutch if self.powered => false,
            Kind::Belt => self.shaft && unit && d[self.shaft_axis()] != 0,
            _ => unit && d[self.axis] != 0,
        }
    }
}

// The mechanical pump is a small cogwheel turning on its facing axis (it has no shaft ends).

// Clockwise of a horizontal direction seen from above (north to east).
fn cw(d: [i32; 3]) -> [i32; 3] {
    [-d[2], 0, d[0]]
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
    // Chain to chain: each drive has one chain line, across its shaft. Two drives next to each other on
    // that line link whichever way their shafts run, as long as the line of each points at the other. A
    // shaft end never pairs with a chain side (it fails the checks above and here).
    if me.is_chain() && nb.is_chain() && d.iter().filter(|c| **c != 0).count() == 1 {
        let line = axis_of(d);
        if me.chain_axis() == line && nb.chain_axis() == line {
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
    let (small_me, small_nb) = (me.small_cog(), nb.small_cog());
    if small_me && small_nb {
        return if me.axis == nb.axis && d[me.axis] == 0 && nonzero == 1 { Some(-1.0) } else { None };
    }
    if !small_me && !small_nb {
        if me.axis == nb.axis {
            return None;
        }
        let third = 3 - me.axis - nb.axis;
        if d[me.axis].abs() == 1 && d[nb.axis].abs() == 1 && d[third] == 0 {
            let mine = d[me.axis] > 0;
            let theirs = d[nb.axis] > 0;
            return Some(if mine ^ theirs { -1.0 } else { 1.0 });
        }
        return None;
    }
    // one small, one large
    if me.axis != nb.axis || d[me.axis] != 0 || nonzero != 2 {
        return None;
    }
    if small_me {
        Some(-2.0)
    } else {
        Some(-0.5)
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
    if me.small_cog() {
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
    if !me.small_cog() {
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
    if kind == Kind::Hose {
        let front = parse_dir(&game.native_block_property(p, "facing")?)?;
        return Some(Node::new(kind, axis_of(cw(front)), front));
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

const DEPOT_SLOTS: usize = 4;

// Adds as much of `count` as fits into stacks of at most 64 in a bag of at most `slots` stacks;
// returns how many went in.
fn stacks_add(bag: &mut Vec<(String, u32)>, item: &str, count: u32, slots: usize) -> u32 {
    let mut left = count;
    for entry in bag.iter_mut() {
        if entry.0 == item && entry.1 < 64 {
            let put = (64 - entry.1).min(left);
            entry.1 += put;
            left -= put;
        }
    }
    while left > 0 && bag.len() < slots {
        let put = left.min(64);
        bag.push((item.to_string(), put));
        left -= put;
    }
    count - left
}

// Everything on the depot: the first stack, then the others (results, by-products).
fn depot_list(n: &Compound) -> Vec<(String, u32)> {
    let mut v = Vec::new();
    let item = get_text(n, "item");
    let count = get_int(n, "count", 0);
    if !item.is_empty() && count > 0 {
        v.push((item, count as u32));
    }
    v.extend(bag_read(n, "ex"));
    v
}
fn depot_store(n: &mut Compound, list: &[(String, u32)]) {
    match list.first() {
        Some((item, count)) => {
            n.insert(JavaString::from("item"), Tag::String(JavaString::from(item.as_str())));
            put_int(n, "count", *count as i32);
        }
        None => {
            n.remove(&JavaString::from("item"));
            n.remove(&JavaString::from("count"));
        }
    }
    let rest: Vec<(String, u32)> = list.iter().skip(1).cloned().collect();
    bag_write(n, "ex", &rest);
}
fn depot_content(n: &Compound) -> Option<(String, u32)> {
    depot_list(n).into_iter().next()
}
fn depot_put(n: &mut Compound, content: Option<(String, u32)>) {
    let mut list = depot_list(n);
    match content {
        Some(c) => {
            if list.is_empty() {
                list.push(c);
            } else {
                list[0] = c;
            }
        }
        None => {
            if !list.is_empty() {
                list.remove(0);
            }
        }
    }
    depot_store(n, &list);
}
// Puts a whole stack on the depot when it fits; false leaves the depot as it was.
fn depot_push_all(game: &mut Game<'_>, p: [i32; 3], item: &str, count: u32) -> bool {
    let item = item.to_string();
    let ok = game
        .native_with_mod_block_entity(p, move |e| {
            let mut list = depot_list(&e.nbt);
            let mut trial = list.clone();
            if stacks_add(&mut trial, &item, count, DEPOT_SLOTS) != count {
                return false;
            }
            stacks_add(&mut list, &item, count, DEPOT_SLOTS);
            depot_store(&mut e.nbt, &list);
            true
        })
        .unwrap_or(false);
    if ok {
        game.native_block_entity_sync(p);
    }
    ok
}
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

// ---- the basin: item stacks kept in the block entity, put in and taken out by hand ----
// "in" holds what waits to be mixed, "out" what came out of the mixer, each as one text:
// item|count;item|count (up to nine stacks, 64 each).

fn bag_read(n: &Compound, key: &str) -> Vec<(String, u32)> {
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

fn bag_write(n: &mut Compound, key: &str, bag: &[(String, u32)]) {
    let mut text = String::new();
    for (i, (item, count)) in bag.iter().enumerate() {
        if i > 0 {
            text.push(';');
        }
        text.push_str(item);
        text.push('|');
        text.push_str(&count.to_string());
    }
    n.insert(JavaString::from(key), Tag::String(JavaString::from(text.as_str())));
}

// Adds as much of `count` as fits (first into stacks of the same item, then into new stacks); returns
// how many went in.
fn bag_add(bag: &mut Vec<(String, u32)>, item: &str, count: u32) -> u32 {
    let mut left = count;
    for entry in bag.iter_mut() {
        if entry.0 == item && entry.1 < 64 {
            let put = (64 - entry.1).min(left);
            entry.1 += put;
            left -= put;
        }
    }
    while left > 0 && bag.len() < BASIN_SLOTS {
        let put = left.min(64);
        bag.push((item.to_string(), put));
        left -= put;
    }
    count - left
}

fn bag_total(bag: &[(String, u32)], item: &str) -> u32 {
    bag.iter().filter(|e| e.0 == item).map(|e| e.1).sum()
}

fn bag_take(bag: &mut Vec<(String, u32)>, item: &str, count: u32) {
    let mut left = count;
    for entry in bag.iter_mut() {
        if entry.0 == item && left > 0 {
            let t = entry.1.min(left);
            entry.1 -= t;
            left -= t;
        }
    }
    bag.retain(|e| e.1 > 0);
}

// A click on the basin: a held stack goes in (as much as fits); with an empty hand a stack comes out,
// the finished ones first.
fn basin_click(game: &mut Game<'_>, pos: [i32; 3]) {
    let held = game.held().cloned();
    if let Some(h) = held {
        if h.count > 0 {
            let item = h.item.to_string();
            let count = h.count;
            let added = game
                .native_with_mod_block_entity(pos, |e| {
                    let mut inb = bag_read(&e.nbt, "in");
                    let n = bag_add(&mut inb, &item, count);
                    if n > 0 {
                        bag_write(&mut e.nbt, "in", &inb);
                    }
                    n
                })
                .unwrap_or(0);
            if added > 0 {
                sfx(game, "minecraft:entity.item.frame.add_item", pos, 0.6, 1.1);
                game.native_block_entity_sync(pos);
                if added >= count {
                    *game.held_mut() = None;
                } else {
                    let mut rest = h.clone();
                    rest.count = count - added;
                    *game.held_mut() = Some(rest);
                }
            }
            return;
        }
    }
    let got = game
        .native_with_mod_block_entity(pos, |e| {
            let mut out = bag_read(&e.nbt, "out");
            if !out.is_empty() {
                let first = out.remove(0);
                bag_write(&mut e.nbt, "out", &out);
                return Some(first);
            }
            let mut inb = bag_read(&e.nbt, "in");
            if !inb.is_empty() {
                let first = inb.remove(0);
                bag_write(&mut e.nbt, "in", &inb);
                return Some(first);
            }
            None
        })
        .flatten();
    if let Some((item, count)) = got {
        sfx(game, "minecraft:entity.item.frame.remove_item", pos, 0.6, 1.1);
        game.native_block_entity_sync(pos);
        let at = at_player(game, pos);
        game.spawn_item(stack_of(&item, count), at, [0.0, 0.0, 0.0], 0);
    }
}

// The basin is broken: everything in it drops.
fn basin_spill(game: &mut Game<'_>, pos: [i32; 3]) {
    let got = game.native_with_mod_block_entity(pos, |e| {
        let mut all = bag_read(&e.nbt, "in");
        all.extend(bag_read(&e.nbt, "out"));
        all
    });
    let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
    for (item, count) in got.unwrap_or_default() {
        game.spawn_item(stack_of(&item, count), at, [0.0, 0.1, 0.0], 10);
    }
}

fn basin_mix_flag(game: &mut Game<'_>, basin: [i32; 3], on: bool) {
    let v = if on { 1 } else { 0 };
    let changed = game.native_with_mod_block_entity(basin, |e| {
        let same = get_int(&e.nbt, "mix", 0) == v;
        put_int(&mut e.nbt, "mix", v);
        !same
    });
    if changed == Some(true) {
        game.native_block_entity_sync(basin);
    }
}

fn basin_target_kind(game: &mut Game<'_>, p: [i32; 3]) -> bool {
    let name = game.native_block(p);
    name == DEPOT_BLOCK || name == "create:belt"
}

// Hands a stack to the depot or belt piece at `p` if it has room. True when it took it.
fn basin_give(game: &mut Game<'_>, p: [i32; 3], item: &str, count: u32) -> bool {
    let name = game.native_block(p);
    if name == DEPOT_BLOCK {
        return depot_push_all(game, p, item, count);
    }
    if name == "create:belt" {
        let item = item.to_string();
        let ok = game
            .native_with_mod_block_entity(p, move |e| {
                let mut v = belt_items(&e.nbt);
                let Some(spot) = belt_free_spot(&v) else { return false };
                v.push(BeltItem { item, count, p: spot, moving: false });
                belt_store(&mut e.nbt, &v);
                true
            })
            .unwrap_or(false);
        if ok {
            game.native_block_entity_sync(p);
        }
        return ok;
    }
    false
}

// One tick of a basin: its outlet turns towards a belt or depot beside it (below first), and a finished
// stack goes out onto it when it has room. True when clients need a fresh picture.
fn basin_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound) -> bool {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return false;
    }
    let n = get_int(nbt, "bt", 0) + 1;
    put_int(nbt, "bt", n % 100000);
    if n % 4 != 0 {
        return false;
    }
    let dirs: [(&str, [i32; 3]); 5] =
        [("down", [0, -1, 0]), ("east", [1, 0, 0]), ("north", [0, 0, -1]), ("south", [0, 0, 1]), ("west", [-1, 0, 0])];
    let current = game.native_block_property(pos, "facing").map(|v| v.to_string()).unwrap_or_default();
    // where each outlet would hand over: straight down, or across the rim at the basin level or one lower
    let spots = |d: [i32; 3]| -> Vec<[i32; 3]> {
        if d[1] != 0 {
            vec![[pos[0], pos[1] - 1, pos[2]]]
        } else {
            vec![[pos[0] + d[0], pos[1], pos[2] + d[2]], [pos[0] + d[0], pos[1] - 1, pos[2] + d[2]]]
        }
    };
    let mut wanted = current.clone();
    for (name, d) in dirs.iter() {
        if spots(*d).into_iter().any(|p| basin_target_kind(game, p)) {
            wanted = name.to_string();
            break;
        }
    }
    if wanted != current && !wanted.is_empty() {
        game.native_set_block_property(pos, "facing", &wanted);
    }
    let out = bag_read(nbt, "out");
    let Some((item, count)) = out.first().cloned() else { return false };
    let Some(d) = dirs.iter().find(|(name, _)| *name == wanted.as_str()).map(|(_, d)| *d) else { return false };
    for p in spots(d) {
        if basin_give(game, p, &item, count) {
            let mut rest = out.clone();
            rest.remove(0);
            bag_write(nbt, "out", &rest);
            return true;
        }
    }
    false
}

// ---- funnels: they move items between the block behind them and the block (or air) in front ----
// A funnel faces away from the block it sits on. Pulling (extracting off) it takes items lying in
// its mouth, or off a belt, depot or basin there, and puts them into the block behind it; extracting
// takes them out of a basin, depot or belt behind it and puts them into the block in front of it, or
// drops them. The andesite one moves one item at a time, the brass one up to sixteen, twice as often.
// Vanilla containers (chests, barrels, furnaces) work both ways, through the engine container calls.

fn funnel_brass(name: &str) -> Option<bool> {
    match name {
        "create:andesite_funnel" => Some(false),
        "create:brass_funnel" => Some(true),
        _ => None,
    }
}

fn direction_of(d: [i32; 3]) -> Direction {
    match d {
        [0, -1, 0] => Direction::Down,
        [0, 1, 0] => Direction::Up,
        [0, 0, -1] => Direction::North,
        [0, 0, 1] => Direction::South,
        [-1, 0, 0] => Direction::West,
        _ => Direction::East,
    }
}

// 1 basin, 2 depot, 3 belt piece, 0 anything else.
fn funnel_storage(game: &mut Game<'_>, p: [i32; 3]) -> u8 {
    let name = game.native_block(p);
    if name == BASIN_BLOCK {
        1
    } else if name == DEPOT_BLOCK {
        2
    } else if name == "create:belt" {
        3
    } else {
        0
    }
}

// Takes up to `amount` items of the first stack the storage at `p` lets go of (a basin gives up only
// what it finished).
fn storage_take(game: &mut Game<'_>, p: [i32; 3], amount: u32, face: Direction, filter: &str) -> Option<(String, u32)> {
    let want = filter.to_string();
    match funnel_storage(game, p) {
        1 => {
            let got = game
                .native_with_mod_block_entity(p, move |e| {
                    let mut out = bag_read(&e.nbt, "out");
                    let i = out.iter().position(|st| want.is_empty() || st.0 == want)?;
                    let (item, count) = out[i].clone();
                    let t = count.min(amount);
                    if t == count {
                        out.remove(i);
                    } else {
                        out[i].1 -= t;
                    }
                    bag_write(&mut e.nbt, "out", &out);
                    Some((item, t))
                })
                .flatten();
            if got.is_some() {
                game.native_block_entity_sync(p);
            }
            got
        }
        2 => {
            let got = game
                .native_with_mod_block_entity(p, move |e| {
                    let mut list = depot_list(&e.nbt);
                    let i = list.iter().position(|st| want.is_empty() || st.0 == want)?;
                    let (item, count) = list[i].clone();
                    let t = count.min(amount);
                    if t == count {
                        list.remove(i);
                    } else {
                        list[i].1 -= t;
                    }
                    depot_store(&mut e.nbt, &list);
                    Some((item, t))
                })
                .flatten();
            if got.is_some() {
                game.native_block_entity_sync(p);
            }
            got
        }
        3 => {
            let got = game
                .native_with_mod_block_entity(p, move |e| {
                    let mut v = belt_items(&e.nbt);
                    let i = v.iter().position(|b| want.is_empty() || b.item == want)?;
                    let item = v[i].item.clone();
                    let t = v[i].count.min(amount);
                    if t == v[i].count {
                        v.remove(i);
                    } else {
                        v[i].count -= t;
                    }
                    belt_store(&mut e.nbt, &v);
                    Some((item, t))
                })
                .flatten();
            if got.is_some() {
                game.native_block_entity_sync(p);
            }
            got
        }
        _ => {
            // a chest, barrel, furnace and the like: the first slot its side `face` lets go of
            let dim = game.native_dimension().to_string();
            let Some(slots) = game.native_container_slots(&dim, p, face) else { return None };
            for slot in slots {
                if !slot.can_take {
                    continue;
                }
                let Some((have, name)) = slot.stack.as_ref().map(|st| (st.count, st.item.to_string())) else { continue };
                if !filter.is_empty() && name != filter {
                    continue;
                }
                if let Some(out) = game.native_extract_item(&dim, p, face, slot.slot, have.min(amount)) {
                    return Some((out.item.to_string(), out.count));
                }
            }
            None
        }
    }
}

// Puts what fits of a stack into the storage at `p` (a basin takes it as ingredients); returns how many
// went in.
fn storage_give(game: &mut Game<'_>, p: [i32; 3], item: &str, count: u32) -> u32 {
    match funnel_storage(game, p) {
        1 => {
            let item = item.to_string();
            let added = game
                .native_with_mod_block_entity(p, move |e| {
                    let mut inb = bag_read(&e.nbt, "in");
                    let n = bag_add(&mut inb, &item, count);
                    if n > 0 {
                        bag_write(&mut e.nbt, "in", &inb);
                    }
                    n
                })
                .unwrap_or(0);
            if added > 0 {
                game.native_block_entity_sync(p);
            }
            added
        }
        2 => {
            let item = item.to_string();
            let added = game
                .native_with_mod_block_entity(p, move |e| {
                    let mut list = depot_list(&e.nbt);
                    let n = stacks_add(&mut list, &item, count, DEPOT_SLOTS);
                    if n > 0 {
                        depot_store(&mut e.nbt, &list);
                    }
                    n
                })
                .unwrap_or(0);
            if added > 0 {
                game.native_block_entity_sync(p);
            }
            added
        }
        3 => {
            if basin_give(game, p, item, count) {
                count
            } else {
                0
            }
        }
        _ => 0,
    }
}

// Hands a stack to the block at `p` through its side `face`: our own storage, a vanilla container, or
// (when `drop_ok`) the air, where the rest drops. Returns how many were taken.
// A click on a funnel. Only a wrench turns it between pulling in and taking out. A brass funnel takes any
// item as its filter (the same item again clears it), and an empty hand steps its count through 1, 2, 4, 8,
// 16, 32 and 64.
fn funnel_click(game: &mut Game<'_>, pos: [i32; 3], brass: bool, held: Option<&str>) {
    if held == Some("create:wrench") {
        let now = game.native_block_property(pos, "unloading").map(|v| v == "true").unwrap_or(false);
        game.native_set_block_property(pos, "unloading", if now { "false" } else { "true" });
        sfx(game, "minecraft:block.iron_trapdoor.open", pos, 0.4, 1.4);
        say(game, if now { "Funnel: pulling items in" } else { "Funnel: taking items out" });
        return;
    }
    if !brass {
        return;
    }
    match held {
        Some(item) => {
            let item = item.to_string();
            let cleared = game
                .native_with_mod_block_entity(pos, move |e| {
                    if get_text(&e.nbt, "flt") == item {
                        e.nbt.insert(JavaString::from("flt"), Tag::String(JavaString::from("")));
                        true
                    } else {
                        e.nbt.insert(JavaString::from("flt"), Tag::String(JavaString::from(item.as_str())));
                        false
                    }
                })
                .unwrap_or(false);
            sfx(game, "minecraft:block.iron_trapdoor.close", pos, 0.4, 1.6);
            say(game, if cleared { "Brass funnel: filter cleared" } else { "Brass funnel: filter set" });
        }
        None => {
            let now = game.native_with_mod_block_entity(pos, |e| get_int(&e.nbt, "cnt", 16)).unwrap_or(16);
            let next = match now {
                1 => 2,
                2 => 4,
                4 => 8,
                8 => 16,
                16 => 32,
                32 => 64,
                _ => 1,
            };
            let _ = game.native_with_mod_block_entity(pos, move |e| put_int(&mut e.nbt, "cnt", next));
            say(game, &("Brass funnel: ".to_string() + &next.to_string() + " items at a time"));
        }
    }
}

fn funnel_deliver(game: &mut Game<'_>, p: [i32; 3], item: &str, count: u32, face: Direction, drop_ok: bool) -> u32 {
    if funnel_storage(game, p) != 0 {
        return storage_give(game, p, item, count);
    }
    let leftover = game.native_insert_item(p, face, stack_of(item, count));
    let rest = leftover.map(|st| st.count).unwrap_or(0).min(count);
    if rest > 0 && drop_ok && is_air(&game.native_block(p)) {
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.3, f64::from(p[2]) + 0.5];
        game.spawn_item(stack_of(item, rest), at, [0.0, 0.0, 0.0], 10);
        return count;
    }
    count - rest
}

// One tick of a funnel on the server. True when it changed something clients should see (never: the
// storages it works on send their own pictures).
fn funnel_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound) -> bool {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return false;
    }
    let name = game.native_block(pos);
    let Some(brass) = funnel_brass(&name) else { return false };
    let n = get_int(nbt, "ft", 0) + 1;
    put_int(nbt, "ft", n % 100000);
    if n % (if brass { 4 } else { 8 }) != 0 {
        return false;
    }
    let facing = game.native_block_property(pos, "facing").and_then(|v| parse_dir(&v)).unwrap_or([0, 1, 0]);
    let extracting = game.native_block_property(pos, "unloading").map(|v| v == "true").unwrap_or(false);
    let dim = game.dim.id().to_string();
    let powered = game.native_neighbor_signal(&dim, pos) > 0;
    let was = game.native_block_property(pos, "powered").map(|v| v == "true").unwrap_or(false);
    if powered != was {
        game.native_set_block_property(pos, "powered", if powered { "true" } else { "false" });
    }
    if powered {
        return false;
    }
    let attached = [pos[0] - facing[0], pos[1] - facing[1], pos[2] - facing[2]];
    let mouth = [pos[0] + facing[0], pos[1] + facing[1], pos[2] + facing[2]];
    // the brass funnel has a filter item and a count; the andesite one takes anything, one at a time
    let filter = if brass { get_text(nbt, "flt") } else { String::new() };
    let amount = if brass { get_int(nbt, "cnt", 16).clamp(1, 64) as u32 } else { 1 };
    if extracting {
        if let Some((item, count)) = storage_take(game, attached, amount, direction_of(facing), &filter) {
            let accepted = funnel_deliver(game, mouth, &item, count, direction_of(neg(facing)), true);
            if accepted < count {
                funnel_deliver(game, attached, &item, count - accepted, direction_of(facing), false);
            }
            if accepted > 0 {
                sfx(game, "minecraft:entity.item.pickup", pos, 0.2, 1.6);
            }
        }
        return false;
    }
    // pulling in: first anything lying in the mouth, then a belt, depot or basin there
    let (x, y, z) = (f64::from(mouth[0]), f64::from(mouth[1]), f64::from(mouth[2]));
    let found = game.native_item_entities(&dim, [x, y, z], [x + 1.0, y + 1.0, z + 1.0]);
    let across = [(axis_of(facing) + 1) % 3, (axis_of(facing) + 2) % 3];
    for e in found {
        if e.stack.count == 0 {
            continue;
        }
        // only what lies near the middle of the mouth is taken, as in Java
        if across.iter().any(|a| (e.position[*a] - (f64::from(mouth[*a]) + 0.5)).abs() > 0.3) {
            continue;
        }
        let item = e.stack.item.to_string();
        if !filter.is_empty() && item != filter {
            continue;
        }
        let t = e.stack.count.min(amount);
        let accepted = funnel_deliver(game, attached, &item, t, direction_of(facing), false);
        if accepted > 0 {
            let mut rest = e.stack.clone();
            rest.count -= accepted;
            let _ = game.native_set_item_entity(&dim, e.id, rest);
            sfx(game, "minecraft:entity.item.pickup", pos, 0.2, 1.6);
            return false;
        }
    }
    if let Some((item, count)) = storage_take(game, mouth, amount, direction_of(neg(facing)), &filter) {
        let accepted = funnel_deliver(game, attached, &item, count, direction_of(facing), false);
        if accepted < count {
            funnel_deliver(game, mouth, &item, count - accepted, direction_of(neg(facing)), false);
        }
        if accepted > 0 {
            sfx(game, "minecraft:entity.item.pickup", pos, 0.2, 1.6);
        }
    }
    false
}

// ---- the blaze burner: fed with fuel it heats a basin above it ----

const BURNER_BLOCK: &str = "create:blaze_burner";
const BURNER_BE: &str = "create:blaze_burner_be";

// How many ticks one item burns in a blaze burner (a blaze cake lasts and superheats).
fn burner_fuel_time(item: &str) -> i32 {
    match item {
        "minecraft:coal_block" => 16000,
        "minecraft:coal" | "minecraft:charcoal" => 1600,
        "minecraft:blaze_rod" => 2400,
        "minecraft:dried_kelp_block" => 4000,
        "minecraft:stick" => 100,
        "create:blaze_cake" => 6000,
        _ => {
            if item.ends_with("_planks") || item.ends_with("_log") || item.ends_with("_wood") {
                300
            } else {
                0
            }
        }
    }
}

// Feeds the burner one item of the held stack. True when it took it.
fn burner_click(game: &mut Game<'_>, pos: [i32; 3]) -> bool {
    let Some(h) = game.held().cloned() else { return false };
    let item = h.item.to_string();
    let time = burner_fuel_time(&item);
    if time == 0 || h.count == 0 {
        return false;
    }
    let cake = item == "create:blaze_cake";
    let _ = game.native_with_mod_block_entity(pos, move |e| {
        let fuel = (get_int(&e.nbt, "fuel", 0) + time).min(24000);
        put_int(&mut e.nbt, "fuel", fuel);
        if cake {
            put_int(&mut e.nbt, "sup", 1);
        }
    });
    if h.count > 1 {
        let mut rest = h.clone();
        rest.count -= 1;
        *game.held_mut() = Some(rest);
    } else {
        *game.held_mut() = None;
    }
    sfx(game, "minecraft:item.firecharge.use", pos, 0.5, 1.0);
    game.native_block_entity_sync(pos);
    true
}

// One tick of a burner on the server: the fuel burns down and flames rise while it burns.
fn burner_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound) -> bool {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return false;
    }
    let fuel = get_int(nbt, "fuel", 0);
    if fuel <= 0 {
        return false;
    }
    put_int(nbt, "fuel", fuel - 1);
    if fuel % 6 == 0 {
        let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.9, f64::from(pos[2]) + 0.5];
        game.native_flame_particle(at, false);
    }
    if fuel == 1 {
        put_int(nbt, "sup", 0);
        return true;
    }
    false
}

fn burner_submit(game: &Game<'_>, entity: &ModBlockEntity, draw: &mut BlockEntityDraw) {
    let fuel = get_int(&entity.nbt, "fuel", 0);
    let part = if fuel <= 0 {
        "create:blaze_head_inert"
    } else if get_int(&entity.nbt, "sup", 0) == 1 {
        "create:blaze_head_super"
    } else if fuel > 2000 {
        "create:blaze_head_active"
    } else {
        "create:blaze_head_idle"
    };
    let t = draw.game_time as f64 + f64::from(draw.partial);
    let bob = (0.02 * (t * 0.08).sin()) as f32;
    draw.push();
    draw.translate([0.0, bob, 0.0]);
    game.native_draw_block(draw, part, &[]);
    draw.pop();
}

// ---- the mixer: two blocks above a basin it mixes what lies in it ----

struct MixRecipe {
    inputs: Vec<(&'static str, u32)>,
    outputs: Vec<(&'static str, u32)>,
    heated: bool,
}

fn mixing_recipes() -> Vec<MixRecipe> {
    vec![MixRecipe {
        inputs: vec![("minecraft:copper_ingot", 1), ("create:zinc_ingot", 1)],
        outputs: vec![("create:brass_ingot", 2)],
        heated: true,
    }]
}

// Until the blaze burner exists a lit campfire, lava or fire under the basin counts as heat.
fn basin_heated(game: &mut Game<'_>, basin: [i32; 3]) -> bool {
    let below = [basin[0], basin[1] - 1, basin[2]];
    let name = game.native_block(below);
    if name == BURNER_BLOCK {
        let fuel = game.native_with_mod_block_entity(below, |e| get_int(&e.nbt, "fuel", 0)).unwrap_or(0);
        if fuel > 0 {
            return true;
        }
    }
    if name == "minecraft:lava" || name == "minecraft:fire" || name == "minecraft:soul_fire" {
        return true;
    }
    if name == "minecraft:campfire" || name == "minecraft:soul_campfire" {
        return game.native_block_property(below, "lit").map(|v| v == "true").unwrap_or(false);
    }
    false
}

fn find_mixing(inb: &[(String, u32)], outb: &[(String, u32)], heated: bool) -> Option<MixRecipe> {
    for r in mixing_recipes() {
        if r.heated && !heated {
            continue;
        }
        if !r.inputs.iter().all(|(item, n)| bag_total(inb, item) >= *n) {
            continue;
        }
        // the results have to fit
        let mut trial_in: Vec<(String, u32)> = inb.to_vec();
        for (item, n) in r.inputs.iter() {
            bag_take(&mut trial_in, item, *n);
        }
        let mut trial_out: Vec<(String, u32)> = outb.to_vec();
        let fits = r.outputs.iter().all(|(item, n)| bag_add(&mut trial_out, item, *n) == *n);
        if fits {
            return Some(r);
        }
    }
    None
}

// The mixer stops: the basin stops turning its items and the run starts over.
fn mixer_stop(game: &mut Game<'_>, nbt: &mut Compound, basin: [i32; 3], present: bool) -> bool {
    if present {
        basin_mix_flag(game, basin, false);
    }
    if get_f32(nbt, "mp") > 0.0 {
        put_f32(nbt, "mp", 0.0);
        return true;
    }
    false
}

// One tick of the mixer on the server. "mp" is how far the run has got (0 when idle). True when the
// clients need a fresh picture (always while it runs, and once when it stops).
fn mixer_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, speed: f32) -> bool {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return false;
    }
    let basin = [pos[0], pos[1] - 2, pos[2]];
    let mp = get_f32(nbt, "mp");
    let rpm = speed.abs();
    let present = game.native_block(basin) == BASIN_BLOCK;
    if !present || rpm < 1.0 {
        return mixer_stop(game, nbt, basin, present);
    }
    let Some((inb, outb)) = game.native_with_mod_block_entity(basin, |e| (bag_read(&e.nbt, "in"), bag_read(&e.nbt, "out"))) else {
        return mixer_stop(game, nbt, basin, present);
    };
    let heated = basin_heated(game, basin);
    let Some(recipe) = find_mixing(&inb, &outb, heated) else {
        return mixer_stop(game, nbt, basin, present);
    };
    if mp == 0.0 {
        basin_mix_flag(game, basin, true);
    }
    let next = mp + (rpm / 16.0).clamp(1.0, 5.0);
    if (mp as i32) / 10 != (next as i32) / 10 {
        sfx(game, "minecraft:block.grindstone.use", pos, 0.2, 1.0);
    }
    if next >= MIX_UNITS {
        sfx(game, "minecraft:block.brewing_stand.brew", pos, 0.5, 1.0);
        let _ = game.native_with_mod_block_entity(basin, |e| {
            let mut inb = bag_read(&e.nbt, "in");
            let mut outb = bag_read(&e.nbt, "out");
            for (item, n) in recipe.inputs.iter() {
                bag_take(&mut inb, item, *n);
            }
            for (item, n) in recipe.outputs.iter() {
                bag_add(&mut outb, item, *n);
            }
            bag_write(&mut e.nbt, "in", &inb);
            bag_write(&mut e.nbt, "out", &outb);
            put_int(&mut e.nbt, "mix", 0);
        });
        game.native_block_entity_sync(basin);
        put_f32(nbt, "mp", 0.0);
        return true;
    }
    put_f32(nbt, "mp", next);
    if (next as i32) % 3 == 0 {
        let at = [f64::from(basin[0]) + 0.5, f64::from(basin[1]) + 0.75, f64::from(basin[2]) + 0.5];
        puff(game, horizon_client::particles::ParticleType::Splash, at);
    }
    true
}

// The mixer on screen: its cog inside turns about the vertical axis whenever it is powered, the pole
// comes through the top and the whisk below spins; while it mixes they go down into the basin.
fn mixer_submit(
    game: &Game<'_>,
    pos: [i32; 3],
    _state: BlockStateId,
    entity: &ModBlockEntity,
    draw: &mut BlockEntityDraw,
) {
    let speed = f64::from(get_f32(&entity.nbt, "speed"));
    let t = draw.game_time as f64 + f64::from(draw.partial);
    let offset = if (pos[0] + pos[2]).rem_euclid(2) == 0 { 22.5 } else { 0.0 };
    draw.push();
    draw.rotate_about([0.0, 1.0, 0.0], ((speed * 0.3 * t) % 360.0 + offset) as f32, [0.5, 0.5, 0.5]);
    game.native_draw_block(draw, "create:mixer_cog_part", &[("axis", "y")]);
    draw.pop();
    let mp = f64::from(get_f32(&entity.nbt, "mp"));
    // down in the first fifth of the run, hold, up in the last fifth
    let lift = (mp.min(MIX_UNITS as f64 - mp) / 20.0).clamp(0.0, 1.0);
    let eased = lift * lift * (3.0 - 2.0 * lift);
    let drop = (eased * 1.3) as f32;
    draw.push();
    draw.translate([0.0, -drop, 0.0]);
    game.native_draw_block(draw, "create:mixer_pole_part", &[]);
    draw.pop();
    let spin = (t * speed * 0.6) % 360.0;
    draw.push();
    draw.translate([0.0, -drop, 0.0]);
    draw.rotate_about([0.0, 1.0, 0.0], spin as f32, [0.5, 0.5, 0.5]);
    game.native_draw_block(draw, "create:mixer_head_part", &[]);
    draw.pop();
}

// The basin on screen: what is in it floats in a ring and turns round while the mixer works.
fn basin_submit(game: &Game<'_>, pos: [i32; 3], entity: &ModBlockEntity, draw: &mut BlockEntityDraw) {
    let mut shown = bag_read(&entity.nbt, "in");
    shown.extend(bag_read(&entity.nbt, "out"));
    shown.truncate(6);
    if shown.is_empty() {
        return;
    }
    let mixing = get_int(&entity.nbt, "mix", 0) == 1;
    let t = draw.game_time as f64 + f64::from(draw.partial);
    let turn = if mixing { t * 0.35 } else { 0.0 };
    let n = shown.len();
    for (k, (item, count)) in shown.iter().enumerate() {
        let a = turn + (k as f64) * std::f64::consts::TAU / (n as f64);
        let r = if n > 1 { 0.2 } else { 0.0 };
        let bob = 0.015 * (t * 0.15 + k as f64).sin();
        let stack = ItemStack { item: Arc::from(item.as_str()), count: *count, damage: 0, extra: None };
        let is_block = game.pack.assets.registry.default_state(item).is_some();
        draw.push();
        draw.translate([(0.5 + r * a.cos()) as f32, (0.45 + bob) as f32, (0.5 + r * a.sin()) as f32]);
        if !is_block {
            draw.rotate_about([1.0, 0.0, 0.0], 90.0, [0.0, 0.0, 0.0]);
        }
        game.native_draw_item(draw, &stack, DisplayContext::Ground, None, pos[0] ^ pos[2] ^ (k as i32));
        draw.pop();
    }
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

// The value of axis_along_first that makes the chain of a drive with this axis run along `line`.
fn chain_first_towards(axis: usize, line: usize) -> bool {
    let low = (0..3).find(|a| *a != axis).unwrap_or(0);
    line == low
}

fn chain_part(game: &mut Game<'_>, pos: [i32; 3], me: &Node) {
    let line = me.chain_axis();
    let mut has = [false, false];
    for (i, sgn) in [-1, 1].iter().enumerate() {
        let mut p = pos;
        p[line] += *sgn;
        if let Some(n) = node_at(game, p) {
            // the neighbour may face another way; its own chain line has to point back at this drive
            if n.is_chain() && n.chain_axis() == line {
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
        // Like Java: a drive that is not linked turns its chain line towards a chain drive beside it that
        // already runs along that line, or towards one that is not linked either (that one turns back),
        // as long as the neighbour shaft does not run along the line.
        for a in 0..3 {
            if a == me.axis {
                continue;
            }
            for sgn in [-1, 1] {
                let mut p = pos;
                p[a] += sgn;
                let Some(n) = node_at(game, p) else { continue };
                if !n.is_chain() || n.axis == a {
                    continue;
                }
                let alone = game.native_block_property(p, "part").map(|v| v == "none").unwrap_or(false);
                if n.chain_axis() != a && !alone {
                    continue;
                }
                let first = chain_first_towards(me.axis, a);
                if first != me.first {
                    game.native_set_block_property(pos, "axis_along_first", if first { "true" } else { "false" });
                }
                return;
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
                // a source block has level 0; flowing water has a higher level
                let level = game.native_block_property(p, "level").map(|v| v.to_string());
                if level.map_or(false, |l| l != "0") {
                    return true;
                }
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
        if game.native_block(p) == DEPOT_BLOCK {
            let any = game
                .native_with_mod_block_entity(p, |e| depot_list(&e.nbt).iter().any(|st| pressing(&st.0).is_some()))
                .unwrap_or(false);
            if any {
                return true;
            }
        }
    }
    for below in [1, 2] {
        let p = [pos[0], pos[1] - below, pos[2]];
        if let Some(items) = belt_items_at(game, p) {
            if items.iter().any(|b| pressing(&b.item).is_some() && (b.p - 0.5).abs() < 0.12) {
                return true;
            }
        }
    }
    let dim = game.dim.id().to_string();
    let (x, y, z) = (f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
    let found = game.native_item_entities(&dim, [x, y - 2.0, z], [x + 1.0, y, z + 1.0]);
    found.iter().any(|e| pressing(&e.stack.item).is_some())
}

// Presses one item of the stack on the belt piece or depot at `p`. True when one was pressed.
fn press_one(game: &mut Game<'_>, p: [i32; 3]) -> bool {
    if game.native_block(p) == "create:belt" {
        let done = game
            .native_with_mod_block_entity(p, |e| {
                let mut v = belt_items(&e.nbt);
                let mut hit = false;
                let mut found: Option<usize> = None;
                for (i, b) in v.iter().enumerate() {
                    if (b.p - 0.5).abs() >= 0.12 {
                        continue;
                    }
                    if pressing(&b.item).is_some() {
                        found = Some(i);
                        break;
                    }
                }
                if let Some(i) = found {
                    if let Some(r) = pressing(&v[i].item) {
                        // one item at a time: the rest of the stack stays as it is
                        if v[i].count > 1 {
                            v[i].count -= 1;
                            let extra = BeltItem { item: r.to_string(), count: 1, p: (v[i].p + 0.05).min(1.0), moving: v[i].moving };
                            v.push(extra);
                        } else {
                            v[i].item = r.to_string();
                        }
                        hit = true;
                    }
                }
                if hit {
                    belt_store(&mut e.nbt, &v);
                }
                hit
            })
            .unwrap_or(false);
        if done {
            game.native_block_entity_sync(p);
            return true;
        }
    }
    if game.native_block(p) == DEPOT_BLOCK {
        let (hit, spill) = game
            .native_with_mod_block_entity(p, |e| {
                let mut list = depot_list(&e.nbt);
                let mut hit = false;
                let mut spill: Option<String> = None;
                for i in 0..list.len() {
                    if let Some(r) = pressing(&list[i].0) {
                        // one item at a time: the rest of the stack stays, the pressed item takes a slot
                        if list[i].1 > 1 {
                            list[i].1 -= 1;
                            if list.len() < DEPOT_SLOTS {
                                list.push((r.to_string(), 1));
                            } else {
                                spill = Some(r.to_string());
                            }
                        } else {
                            list[i].0 = r.to_string();
                        }
                        hit = true;
                        break;
                    }
                }
                if hit {
                    depot_store(&mut e.nbt, &list);
                }
                (hit, spill)
            })
            .unwrap_or((false, None));
        if let Some(r) = spill {
            let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 1.0, f64::from(p[2]) + 0.5];
            game.spawn_item(stack_of(&r, 1), at, [0.12, 0.15, 0.0], 10);
        }
        if hit {
            game.native_block_entity_sync(p);
            return true;
        }
    }
    false
}

// The head hits the bottom: press one item, the first it finds (belt or depot one block down, then two,
// then something lying on the ground), and nothing more.
fn press_now(game: &mut Game<'_>, pos: [i32; 3]) {
    sfx(game, "minecraft:block.anvil.place", pos, 0.35, 1.7);
    for below in [1, 2] {
        if press_one(game, [pos[0], pos[1] - below, pos[2]]) {
            return;
        }
    }
    let dim = game.dim.id().to_string();
    let (x, y, z) = (f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
    let found = game.native_item_entities(&dim, [x, y - 2.0, z], [x + 1.0, y, z + 1.0]);
    for e in found {
        let Some(result) = pressing(&e.stack.item) else { continue };
        let mut rest = e.stack.clone();
        rest.count = e.stack.count.saturating_sub(1);
        if game.native_set_item_entity(&dim, e.id, rest) {
            game.spawn_item(stack_of(result, 1), e.position, [0.0, 0.1, 0.0], 10);
            return;
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
    // The pace of Create: a cycle is 240 units, and each tick adds 1 to 60 units (rpm / 512 of the way),
    // so about 8 ticks per stroke at 256 rpm, 60 at 32 rpm and 240 at 8 rpm. 1.5 degrees per unit.
    let units = (1.0 + (rpm / 512.0).clamp(0.0, 1.0) * 59.0).floor();
    let mut next = phase + units * 1.5;
    if phase < 180.0 && next >= 180.0 {
        press_now(game, pos);
    }
    if next >= 360.0 {
        next = 0.0;
    }
    put_f32(nbt, "pp", next);
    true
}

// Create draws a stack lying on a belt, depot or saw as several copies: one for a single item, then one
// more for each doubling, up to five. Copy k sits a little apart from the first.
fn stack_copies(count: u32) -> u32 {
    if count <= 1 {
        1
    } else {
        (1 + (31 - count.leading_zeros())).min(5)
    }
}

fn copy_offset(k: u32, is_block: bool) -> [f32; 3] {
    if is_block {
        match k {
            0 => [0.0, 0.0, 0.0],
            1 => [0.11, 0.0, 0.08],
            2 => [-0.11, 0.0, 0.08],
            3 => [0.0, 0.0, -0.12],
            _ => [0.0, 0.12, 0.0],
        }
    } else {
        let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
        [sign * 0.035 * (k as f32), 0.03 * (k as f32), -sign * 0.025 * (k as f32)]
    }
}

// ---- belts ----
// A belt is a row of belt pieces (start, middle..., end) made by the belt connector item between two
// shafts. Turning, it carries the items lying on it along its facing (the other way when the speed
// is negative) and hands the last one over to a free depot straight ahead.

static PENDING: Mutex<Option<[i32; 3]>> = Mutex::new(None);

fn is_air(name: &str) -> bool {
    name == "minecraft:air" || name == "minecraft:cave_air" || name == "minecraft:void_air"
}

const BELT_GAP: f32 = 0.5;
const BELT_MAX: usize = 4;
const BELT_TOP: f32 = 0.83;

// An item stack riding a belt piece: where it is along the piece (0 at the back end, 1 at the front end).
#[derive(Clone)]
struct BeltItem {
    item: String,
    count: u32,
    p: f32,
    moving: bool,
}

// The piece keeps its items as one text: item|count|where|moving;item|count|where|moving...
fn belt_items(n: &Compound) -> Vec<BeltItem> {
    let text = get_text(n, "bi");
    let mut out = Vec::new();
    for part in text.split(';') {
        let mut f = part.splitn(4, '|');
        let (Some(item), Some(count), Some(p)) = (f.next(), f.next(), f.next()) else { continue };
        let (Ok(count), Ok(p)) = (count.parse::<u32>(), p.parse::<f32>()) else { continue };
        let moving = f.next() == Some("1");
        if !item.is_empty() && count > 0 {
            out.push(BeltItem { item: item.to_string(), count, p, moving });
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
        text.push('|');
        text.push_str(if b.moving { "1" } else { "0" });
    }
    n.insert(JavaString::from("bi"), Tag::String(JavaString::from(text.as_str())));
}

static BELT_VIEW: Mutex<Vec<([i32; 3], String, f64, Vec<BeltItem>)>> = Mutex::new(Vec::new());

// The drawing side: the picture of the piece as last sent and the time it was first seen, so the items
// can be drawn moving on from where that picture put them, smoothly, whatever the network does.
fn belt_view(pos: [i32; 3], nbt: &Compound, now: f64) -> (Vec<BeltItem>, f64) {
    let text = get_text(nbt, "bi");
    let Ok(mut g) = BELT_VIEW.lock() else { return (belt_items(nbt), now) };
    if let Some(i) = g.iter().position(|e| e.0 == pos) {
        if g[i].1 == text {
            return (g[i].3.clone(), g[i].2);
        }
        let v = belt_items(nbt);
        if v.is_empty() {
            g.remove(i);
            return (v, now);
        }
        g[i] = (pos, text, now, v.clone());
        return (v, now);
    }
    let v = belt_items(nbt);
    if !v.is_empty() {
        g.push((pos, text, now, v.clone()));
    }
    (v, now)
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
                    v.push(BeltItem { p: entry, moving: true, ..carried });
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
    if crusher_accept(game, next, item) {
        return true;
    }
    if let Some(n) = node_at(game, next) {
        if n.kind == Kind::Saw && n.front == [0, 1, 0] && saw_accept(game, next, item) {
            return true;
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
    // A basin ahead takes the stack as ingredients when all of it fits.
    if game.native_block(next) == BASIN_BLOCK {
        let name = item.item.clone();
        let count = item.count;
        let fits = game
            .native_with_mod_block_entity(next, move |e| {
                let mut inb = bag_read(&e.nbt, "in");
                bag_add(&mut inb, &name, count) == count
            })
            .unwrap_or(false);
        return fits && storage_give(game, next, &item.item, item.count) == item.count;
    }
    // Anything else solid ahead holds the stack at the end of the piece. Only the open air lets it
    // fall off, otherwise it would land back on the belt and be taken up again, a pop back.
    if !is_air(&game.native_block(next)) {
        return false;
    }
    let s = f64::from(sign);
    let edge = [
        f64::from(pos[0]) + 0.5 + f64::from(me.front[0]) * s * 0.6,
        f64::from(pos[1]) + 0.9,
        f64::from(pos[2]) + 0.5 + f64::from(me.front[2]) * s * 0.6,
    ];
    let push = [f64::from(me.front[0]) * s * 0.1, 0.05, f64::from(me.front[2]) * s * 0.1];
    if item.count == 1 && game.native_block(next) == DRAIN {
        if bucket_fluid(&item.item).is_some() && drain_accept(game, next, &item.item) {
            return true;
        }
    }
    game.spawn_item(stack_of(&item.item, item.count), edge, push, 20);
    true
}

// One tick of a belt piece: items lying on it are taken in, the ones it carries move on at the speed of
// the belt (a gap apart) and pass to the next piece, a free depot, or fall off the end. True when it changed.
// Which way a belt carries things for a positive speed, along its facing: the top of a roller that turns
// counterclockwise about its shaft (as the shafts of the belt turn) runs along shaft x up, which is
// +z, and along z x up, which is -x. 1.0 is with the facing, -1.0 against it.
fn belt_flip(front: [i32; 3]) -> f32 {
    if front[0] != 0 {
        -(front[0] as f32)
    } else {
        front[2] as f32
    }
}

// A running press stands one or two blocks above this belt piece.
fn press_above_running(game: &mut Game<'_>, pos: [i32; 3]) -> bool {
    for up in [1, 2] {
        let p = [pos[0], pos[1] + up, pos[2]];
        if game.native_block(p) == "create:mechanical_press" {
            let r = game.native_with_mod_block_entity(p, |e| get_f32(&e.nbt, "speed"));
            if r.map_or(false, |sp| sp.abs() >= 1.0) {
                return true;
            }
        }
    }
    false
}

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
                items.push(BeltItem { item: e.stack.item.to_string(), count: e.stack.count, p: spot, moving: false });
                changed = true;
            }
        }
    }
    if speed.abs() >= 1.0 && !items.is_empty() {
        let sign = (if speed > 0.0 { 1.0f32 } else { -1.0 }) * belt_flip(me.front);
        let step = speed.abs() / 480.0 * sign;
        items.sort_by(|a, b| (b.p * sign).partial_cmp(&(a.p * sign)).unwrap_or(std::cmp::Ordering::Equal));
        let mut ahead: Option<f32> = None;
        let mut kept: Vec<BeltItem> = Vec::new();
        // Under a running press a pressable stack waits in the middle of the piece until it is pressed.
        let hold = items.iter().any(|b| pressing(&b.item).is_some()) && press_above_running(game, pos);
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
            if hold && pressing(&b.item).is_some() {
                if sign > 0.0 && b.p <= 0.5 {
                    np = np.min(0.5);
                } else if sign < 0.0 && b.p >= 0.5 {
                    np = np.max(0.5);
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
            // Only starting or stopping needs a fresh picture on the clients; they carry the item on
            // themselves while it moves.
            let mv = (np - b.p).abs() > 1e-6;
            if mv != b.moving {
                b.moving = mv;
                changed = true;
            }
            b.p = np;
            ahead = Some(np);
            kept.push(b);
        }
        items = kept;
    } else {
        for b in items.iter_mut() {
            if b.moving {
                b.moving = false;
                changed = true;
            }
        }
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
const DRAIN: &str = "create:item_drain";
const GLASS: &str = "create:glass_fluid_pipe";
const HOSE: &str = "create:hose_pulley";
const HOSE_MAX: f32 = 24.0;
const ROPE: &str = "create:hose_rope_part";
const MAGNET: &str = "create:hose_magnet_part";

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
    matches!(name, PIPE | TANK | SPOUT | DRAIN | GLASS)
}

// Whether the block at `p` has a fluid port on its face towards `d` (d points from p to the asker).
fn fluid_port(game: &Game<'_>, p: [i32; 3], d: [i32; 3]) -> bool {
    let name = game.native_block(p).to_string();
    match name.as_str() {
        PIPE | TANK => true,
        SPOUT => d != [0, -1, 0],
        DRAIN => d != [0, 1, 0],
        GLASS => game.native_block_property(p, "axis").and_then(|v| parse_axis(&v.to_string())) == Some(axis_of(d)),
        HOSE => match game.native_block_property(p, "facing").and_then(|v| parse_dir(&v.to_string())) {
            Some(f) => d == neg(cw(f)),
            None => false,
        },
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
        let q = add3(pos, *d);
        if fluid_port(game, q, neg(*d)) || world_source(game, q).is_some() {
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

// The tanks of one layer that touch each other sideways.
fn tank_layer(game: &Game<'_>, pos: [i32; 3]) -> Vec<[i32; 3]> {
    let mut cells: Vec<[i32; 3]> = vec![pos];
    let mut i = 0;
    while i < cells.len() && cells.len() < 32 {
        let p = cells[i];
        i += 1;
        for d in [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]] {
            let q = add3(p, d);
            if !cells.contains(&q) && game.native_block(q) == TANK {
                cells.push(q);
            }
        }
    }
    let minx = cells.iter().map(|c| c[0]).min().unwrap_or(pos[0]);
    let maxx = cells.iter().map(|c| c[0]).max().unwrap_or(pos[0]);
    let minz = cells.iter().map(|c| c[2]).min().unwrap_or(pos[2]);
    let maxz = cells.iter().map(|c| c[2]).max().unwrap_or(pos[2]);
    let (w, d) = (maxx - minx + 1, maxz - minz + 1);
    if w == d && (w == 2 || w == 3) && cells.len() as i32 == w * d {
        cells
    } else {
        vec![pos]
    }
}

// The footprint of a tank layer. Layers stack into one tank only when their footprints are the same, as in the game.
fn tank_footprint(game: &Game<'_>, pos: [i32; 3]) -> Vec<(i32, i32)> {
    let mut v: Vec<(i32, i32)> = tank_layer(game, pos).iter().map(|c| (c[0], c[2])).collect();
    v.sort();
    v
}

fn stacked(game: &Game<'_>, a: [i32; 3], b: [i32; 3]) -> bool {
    game.native_block(b) == TANK && tank_footprint(game, a) == tank_footprint(game, b)
}

// A single tank set on top of a 2x2 or 3x3 tank grows into a whole layer: the missing tanks are placed and
// taken from the tanks you hold (all of them or nothing).
fn tank_extend(game: &mut Game<'_>, pos: [i32; 3]) {
    let below = [pos[0], pos[1] - 1, pos[2]];
    if game.native_block(below) != TANK || tank_layer(game, pos).len() != 1 {
        return;
    }
    let foot = tank_layer(game, below);
    let minx = foot.iter().map(|c| c[0]).min().unwrap_or(0);
    let maxx = foot.iter().map(|c| c[0]).max().unwrap_or(0);
    let minz = foot.iter().map(|c| c[2]).min().unwrap_or(0);
    let maxz = foot.iter().map(|c| c[2]).max().unwrap_or(0);
    let (w, d) = (maxx - minx + 1, maxz - minz + 1);
    if foot.len() < 2 || foot.len() as i32 != w * d || w > 3 || d > 3 {
        return;
    }
    if !foot.iter().any(|c| c[0] == pos[0] && c[2] == pos[2]) {
        return;
    }
    let mut missing: Vec<[i32; 3]> = Vec::new();
    for c in foot.iter() {
        let cell = [c[0], pos[1], c[2]];
        if cell == pos {
            continue;
        }
        if is_air(&game.native_block(cell).to_string()) {
            missing.push(cell);
        } else {
            return;
        }
    }
    let have = match game.held() {
        Some(h) if h.item.to_string() == "create:fluid_tank" => h.count as i32,
        _ => 0,
    };
    if !game.creative && have < missing.len() as i32 {
        return;
    }
    let Some(st) = game.pack.assets.registry.default_state(TANK) else { return };
    let dim = game.dim.id().to_string();
    for cell in missing {
        if game.native_set_block(&dim, cell, st, 3) && !game.creative {
            game.consume_held();
        }
    }
}

// A tank shows a cap where no tank lies above it and a base where none lies below. Tanks side by side
// form a block up to 3 by 3: corners get a window facing out, edges and the middle stay plain, a single
// column gets one window.
fn tank_sync(game: &mut Game<'_>, pos: [i32; 3]) {
    let below = stacked(game, pos, [pos[0], pos[1] - 1, pos[2]]);
    let above = stacked(game, pos, [pos[0], pos[1] + 1, pos[2]]);
    let cells = tank_layer(game, pos);
    let minx = cells.iter().map(|c| c[0]).min().unwrap_or(pos[0]);
    let maxx = cells.iter().map(|c| c[0]).max().unwrap_or(pos[0]);
    let minz = cells.iter().map(|c| c[2]).min().unwrap_or(pos[2]);
    let maxz = cells.iter().map(|c| c[2]).max().unwrap_or(pos[2]);
    let w = maxx - minx + 1;
    let d = maxz - minz + 1;
    let rect = cells.len() as i32 == w * d && w <= 3 && d <= 3;
    let mut shape = "window".to_string();
    if rect && w > 1 && d > 1 {
        let xs = if pos[0] == minx { "w" } else if pos[0] == maxx { "e" } else { "" };
        let zs = if pos[2] == minz { "n" } else if pos[2] == maxz { "s" } else { "" };
        shape = if xs.is_empty() || zs.is_empty() { "plain".to_string() } else { "window_".to_string() + zs + xs };
    }
    let wants = [("bottom", if below { "false" } else { "true" }), ("top", if above { "false" } else { "true" }), ("shape", shape.as_str())];
    for (prop, v) in wants {
        let same = game.native_block_property(pos, prop).map(|x| x == v).unwrap_or(true);
        if !same {
            game.native_set_block_property(pos, prop, v);
        }
    }
}

struct Ends {
    tanks: Vec<[i32; 3]>,
    worlds: Vec<[i32; 3]>,
    // cells where a hose pulley end hangs
    hoses: Vec<[i32; 3]>,
    // glass pipes passed, with the direction walked
    flows: Vec<([i32; 3], [i32; 3])>,
}

// Walks the pipes from `from` in direction `dir` and collects the ends: tanks and spouts, and world
// cells where an open pipe end (or an open pump end) touches the world.
fn walk(game: &mut Game<'_>, from: [i32; 3], dir: [i32; 3]) -> Ends {
    let mut ends = Ends { tanks: Vec::new(), worlds: Vec::new(), hoses: Vec::new(), flows: Vec::new() };
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
            DRAIN => {
                if d != [0, -1, 0] {
                    ends.tanks.push(q);
                }
            }
            GLASS => {
                let a = game.native_block_property(q, "axis").and_then(|v| parse_axis(&v.to_string()));
                if a == Some(axis_of(d)) {
                    ends.flows.push((q, d));
                    stack.push((q, d));
                }
            }
            HOSE => {
                let f = game.native_block_property(q, "facing").and_then(|v| parse_dir(&v.to_string()));
                if let Some(f) = f {
                    if d == cw(f) {
                        let l = game.native_with_mod_block_entity(q, |e| get_f32(&e.nbt, "hl")).unwrap_or(0.0);
                        ends.hoses.push([q[0], q[1] - 1 - (l.floor() as i32), q[2]]);
                    }
                }
            }
            SPOUT => {
                if d != [0, 1, 0] {
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
                if game.native_block(q) == TANK && (if d[1] == 0 { tank_layer(game, p).contains(&q) } else { tank_footprint(game, p) == tank_footprint(game, q) }) {
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

fn put_cell(game: &mut Game<'_>, c: [i32; 3], fl: &str, each: i32, ask: i32) -> i32 {
    let fl = fl.to_string();
    let took = game
        .native_with_mod_block_entity(c, move |e| {
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
        game.native_block_entity_sync(c);
    }
    took
}

fn take_cell(game: &mut Game<'_>, c: [i32; 3], ask: i32) -> i32 {
    let took = game
        .native_with_mod_block_entity(c, move |e| {
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
        game.native_block_entity_sync(c);
    }
    took
}

// The cells of a pool grouped by layer, lowest layer first. A layer fills and drains evenly.
fn pool_layers(pool: &Pool) -> Vec<Vec<[i32; 3]>> {
    let mut out: Vec<Vec<[i32; 3]>> = Vec::new();
    for c in pool.cells.iter() {
        if let Some(last) = out.last_mut() {
            if last[0][1] == c[1] {
                last.push(*c);
                continue;
            }
        }
        out.push(vec![*c]);
    }
    out
}

// Puts up to `want` of `fluid` into the pool, lowest layer first. Returns what went in.
fn pool_put(game: &mut Game<'_>, pool: &Pool, fluid: &str, want: i32) -> i32 {
    if want <= 0 || (pool.amount > 0 && pool.fluid != fluid) {
        return 0;
    }
    let mut left = want;
    for layer in pool_layers(pool).iter() {
        let n = layer.len() as i32;
        while left > 0 {
            let share = ((left + n - 1) / n).max(1);
            let mut progress = 0;
            for c in layer.iter() {
                if left <= 0 {
                    break;
                }
                let t = put_cell(game, *c, fluid, pool.each, share.min(left));
                left -= t;
                progress += t;
            }
            if progress == 0 {
                break;
            }
        }
        if left <= 0 {
            break;
        }
    }
    want - left
}

// Takes up to `want` out of the pool, highest layer first. Returns what came out.
fn pool_take(game: &mut Game<'_>, pool: &Pool, want: i32) -> i32 {
    if want <= 0 || pool.amount <= 0 {
        return 0;
    }
    let mut left = want;
    for layer in pool_layers(pool).iter().rev() {
        let n = layer.len() as i32;
        while left > 0 {
            let share = ((left + n - 1) / n).max(1);
            let mut progress = 0;
            for c in layer.iter() {
                if left <= 0 {
                    break;
                }
                let t = take_cell(game, *c, share.min(left));
                left -= t;
                progress += t;
            }
            if progress == 0 {
                break;
            }
        }
        if left <= 0 {
            break;
        }
    }
    want - left
}

// ---- the world: source blocks drawn in, source blocks placed ----

fn world_source(game: &Game<'_>, p: [i32; 3]) -> Option<&'static str> {
    let name = game.native_block(p).to_string();
    if name == "minecraft:lava_cauldron" {
        return Some("lava");
    }
    if name == "minecraft:water_cauldron" {
        return match game.native_block_property(p, "level") {
            Some(v) if v != "3" => None,
            _ => Some("water"),
        };
    }
    let fluid = match name.as_str() {
        "minecraft:water" => "water",
        "minecraft:lava" => "lava",
        _ => return None,
    };
    match game.native_block_property(p, "level") {
        Some(v) if v == "0" => Some(fluid),
        Some(_) => None,
        None => Some(fluid),
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

// What is left when a source block is drawn away: a cauldron stays, anything else becomes air.
fn world_empty(game: &mut Game<'_>, p: [i32; 3]) {
    let here = game.native_block(p).to_string();
    let dim = game.dim.id().to_string();
    if here == "minecraft:water_cauldron" || here == "minecraft:lava_cauldron" {
        if let Some(st) = game.pack.assets.registry.default_state("minecraft:cauldron") {
            game.native_set_block(&dim, p, st, 3);
            return;
        }
    }
    game.native_set_block(&dim, p, BlockStateId(0), 3);
}

fn place_fluid(game: &mut Game<'_>, p: [i32; 3], fluid: &str) -> bool {
    if game.native_block(p) == "minecraft:cauldron" {
        let cname = if fluid == "lava" { "minecraft:lava_cauldron" } else { "minecraft:water_cauldron" };
        let Some(st) = game.pack.assets.registry.default_state(cname) else { return false };
        let dim = game.dim.id().to_string();
        let ok = game.native_set_block(&dim, p, st, 3);
        if ok && fluid != "lava" {
            game.native_set_block_property(p, "level", "3");
        }
        return ok;
    }
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
// A block sound (source 4 is the block volume slider). A bucket sound for the fluid: lava has its own.
fn sfx(game: &mut Game<'_>, sound: &str, pos: [i32; 3], volume: f32, pitch: f32) {
    let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
    if game.native_window() {
        let _ = game.native_play_local_sound(sound, at, volume, pitch, 0);
    } else {
        let _ = game.native_play_sound(sound, at, 4, volume, pitch);
    }
}

fn bucket_sfx(game: &mut Game<'_>, fluid: &str, fill: bool, pos: [i32; 3]) {
    let lava = fluid.contains("lava");
    let ev = match (fill, lava) {
        (true, false) => "minecraft:item.bucket.fill",
        (true, true) => "minecraft:item.bucket.fill_lava",
        (false, false) => "minecraft:item.bucket.empty",
        (false, true) => "minecraft:item.bucket.empty_lava",
    };
    sfx(game, ev, pos, 0.8, 1.0);
}

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
    let flow = if speed > 0.0 { neg(me.front) } else { me.front };
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
    let mut moved = false;
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
            moved = true;
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
                world_empty(game, *c);
            }
            fluid = wf.to_string();
            amt += BUCKET;
            moved = true;
            bucket_sfx(game, wf, true, *c);
            break;
        }
    }
    // draw in from the water or lava body where a hose pulley hangs
    if PUMP_CAP - amt >= BUCKET && (game.ticks as i64 / 2).rem_euclid(pace) == 0 {
        let dim = game.dim.id().to_string();
        for c in ins.hoses.iter() {
            let Some((src, wf, infinite)) = hose_source(game, *c) else { continue };
            if amt > 0 && fluid != wf {
                continue;
            }
            if !infinite {
                game.native_set_block(&dim, src, BlockStateId(0), 3);
            }
            fluid = wf.to_string();
            amt += BUCKET;
            moved = true;
            bucket_sfx(game, wf, true, *c);
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
        if put > 0 {
            moved = true;
        }
    }
    // place a source block at an open end at the front once a bucket is ready
    if amt >= BUCKET && !fluid.is_empty() {
        for c in outs.worlds.iter() {
            if in_cells.contains(c) {
                continue;
            }
            let here = game.native_block(*c).to_string();
            if (is_air(&here) || here == "minecraft:cauldron") && place_fluid(game, *c, &fluid) {
                amt -= BUCKET;
                moved = true;
                bucket_sfx(game, &fluid, false, *c);
                break;
            }
        }
    }
    if amt >= BUCKET && !fluid.is_empty() {
        for c in outs.hoses.iter() {
            if hose_place(game, *c, &fluid) {
                amt -= BUCKET;
                moved = true;
                bucket_sfx(game, &fluid, false, *c);
                break;
            }
        }
    }
    if moved && (game.ticks as i64 + i64::from(pos[0])).rem_euclid(30) < 2 {
        sfx(game, "minecraft:block.water.ambient", pos, 0.4, 0.8);
    }
    if moved && !fluid.is_empty() {
        let mut flows: Vec<([i32; 3], [i32; 3])> = outs.flows.clone();
        for (c, d) in ins.flows.iter() {
            flows.push((*c, neg(*d)));
        }
        stamp_flow(game, &flows, &fluid);
    }
    fluid_put(nbt, &fluid, amt);
}

// ---- the hose pulley ----
// Turning one way lowers the hose (until something solid is in the way), the other way raises it. The
// hose end is an open end for the pump: it draws water or lava from the body it hangs in (the highest
// source first; a big body of water never runs out) and puts source blocks down where it is free.

fn hose_free(game: &Game<'_>, p: [i32; 3]) -> bool {
    let n = game.native_block(p).to_string();
    is_air(&n) || n == "minecraft:water" || n == "minecraft:lava" || n == ROPE || n == MAGNET
}

// The hose is also made of real blocks in the air cells under the pulley (a rope column and the magnet at
// its tip), so it stays visible when the pulley block itself is out of view. Water cells are drawn by the
// block entity instead.
fn hose_blocks(game: &mut Game<'_>, pos: [i32; 3], l: f32, moving: bool) {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return;
    }
    let count = l.floor() as i32;
    let dim = game.dim.id().to_string();
    for k in 1..=(HOSE_MAX as i32 + 2) {
        let cell = [pos[0], pos[1] - k, pos[2]];
        let here = game.native_block(cell).to_string();
        // while the hose moves the block entity draws the last rope piece and the magnet smoothly
        let rope_to = if moving { count - 1 } else { count };
        let want = if k <= rope_to { Some(ROPE) } else if k == count + 1 && !moving { Some(MAGNET) } else { None };
        match want {
            Some(w) => {
                if is_air(&here) || here == "minecraft:water" || here == "minecraft:lava" || ((here == ROPE || here == MAGNET) && here != w) {
                    let wet = here == "minecraft:water";
                    if let Some(st) = game.pack.assets.registry.default_state(w) {
                        if game.native_set_block(&dim, cell, st, 3) && wet {
                            game.native_set_block_property(cell, "waterlogged", "true");
                        }
                    }
                }
            }
            None => {
                if here == ROPE || here == MAGNET {
                    // the cell gets its water or lava back when the fluid lies beside it
                    let mut back = "";
                    if game.native_block_property(cell, "waterlogged").map(|v| v == "true").unwrap_or(false) {
                        back = "water";
                    }
                    for d in DIRS.iter() {
                        if back == "water" {
                            break;
                        }
                        let nb = game.native_block(add3(cell, *d)).to_string();
                        if nb == "minecraft:lava" {
                            back = "lava";
                            break;
                        }
                        if nb == "minecraft:water" {
                            back = "water";
                        }
                    }
                    if back.is_empty() || !place_fluid(game, cell, back) {
                        game.native_set_block(&dim, cell, BlockStateId(0), 3);
                    }
                }
            }
        }
    }
}

fn hose_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, speed: f32) -> bool {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return false;
    }
    if speed.abs() < 1.0 {
        return false;
    }
    let l = get_f32(nbt, "hl");
    let mut n = (l + speed * 0.002).clamp(0.0, HOSE_MAX);
    if n > l {
        let cell = [pos[0], pos[1] - 1 - (n.floor() as i32), pos[2]];
        if !hose_free(game, cell) {
            n = l;
        }
    }
    if n != l {
        put_f32(nbt, "hl", n);
        true
    } else {
        false
    }
}

fn hose_source(game: &Game<'_>, end: [i32; 3]) -> Option<([i32; 3], &'static str, bool)> {
    // the tip may sit in a cell already drained: then look just below it
    let mut end = end;
    if !matches!(game.native_block(end).to_string().as_str(), "minecraft:water" | "minecraft:lava") {
        let mut found = [end[0], end[1] - 1, end[2]];
        for d in [[0, -1, 0], [1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]] {
            let c = add3(end, d);
            if matches!(game.native_block(c).to_string().as_str(), "minecraft:water" | "minecraft:lava") {
                found = c;
                break;
            }
        }
        end = found;
    }
    let name = game.native_block(end).to_string();
    let fluid = match name.as_str() {
        "minecraft:water" => "water",
        "minecraft:lava" => "lava",
        _ => return None,
    };
    let mut seen: HashSet<[i32; 3]> = HashSet::new();
    seen.insert(end);
    let mut queue: Vec<[i32; 3]> = vec![end];
    let mut i = 0;
    let mut best: Option<[i32; 3]> = None;
    while i < queue.len() && queue.len() < 400 {
        let p = queue[i];
        i += 1;
        if game.native_block_property(p, "level").map(|v| v == "0").unwrap_or(false) {
            best = match best {
                Some(b) if b[1] > p[1] => Some(b),
                _ => Some(p),
            };
        }
        for d in DIRS.iter() {
            let q = add3(p, *d);
            if seen.contains(&q) {
                continue;
            }
            if game.native_block(q) == name {
                seen.insert(q);
                queue.push(q);
            }
        }
    }
    let big = queue.len() >= 400;
    best.map(|b| (b, fluid, big && fluid == "water"))
}

// Output as in the game: the fluid fills the space around the tip from the lowest layer up, spreading
// sideways, one source block per bucket. The space is searched through air and flowing fluid.
fn hose_place(game: &mut Game<'_>, end: [i32; 3], fluid: &str) -> bool {
    let wn = match fluid {
        "water" => "minecraft:water",
        "lava" => "minecraft:lava",
        _ => return false,
    };
    let start = end;
    let mut seen: HashSet<[i32; 3]> = HashSet::new();
    seen.insert(start);
    let mut queue: Vec<[i32; 3]> = vec![start];
    let mut best: Option<[i32; 3]> = None;
    let mut i = 0;
    while i < queue.len() && queue.len() < 300 {
        let p = queue[i];
        i += 1;
        let here = game.native_block(p).to_string();
        let flowing = here == wn && game.native_block_property(p, "level").map(|v| v != "0").unwrap_or(false);
        if is_air(&here) || flowing {
            best = match best {
                Some(b) if b[1] <= p[1] => Some(b),
                _ => Some(p),
            };
        }
        for d in DIRS.iter() {
            let q = add3(p, *d);
            if seen.contains(&q) || q[1] > start[1] + 1 || (q[0] - start[0]).abs() > 12 || (q[2] - start[2]).abs() > 12 {
                continue;
            }
            let nb = game.native_block(q).to_string();
            if is_air(&nb) || nb == wn || nb == MAGNET || nb == ROPE {
                seen.insert(q);
                queue.push(q);
            }
        }
    }
    match best {
        Some(b) => place_fluid(game, b, fluid),
        None => false,
    }
}

// ---- glass pipes show the fluid moving through them ----
// A pump stamps the glass pipes it moves fluid through (fluid, direction, time). The block entity draws
// a moving stripe pattern while the stamp is fresh; the pipe clears it a little after the pump stops.

fn stamp_flow(game: &mut Game<'_>, flows: &[([i32; 3], [i32; 3])], fluid: &str) {
    let now = game.ticks as i32;
    for (c, d) in flows.iter() {
        let idx = DIRS.iter().position(|x| x == d).unwrap_or(0) as i32;
        let fl = fluid.to_string();
        let changed = game
            .native_with_mod_block_entity(*c, move |e| {
                let old = get_text(&e.nbt, "ff");
                let oi = get_int(&e.nbt, "fd", -1);
                put_int(&mut e.nbt, "ft", now);
                if old != fl || oi != idx {
                    e.nbt.insert(JavaString::from("ff"), Tag::String(JavaString::from(fl.as_str())));
                    put_int(&mut e.nbt, "fd", idx);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if changed {
            game.native_block_entity_sync(*c);
        }
    }
}

fn flow_expire(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound) {
    if get_text(nbt, "ff").is_empty() {
        return;
    }
    let now = game.ticks as i32;
    if now.wrapping_sub(get_int(nbt, "ft", now)) > 12 {
        nbt.remove(&JavaString::from("ff"));
        nbt.remove(&JavaString::from("fd"));
        game.native_block_entity_sync(pos);
    }
}

// ---- the item drain: empties full buckets (put in by hand, thrown on top, or carried by a belt) ----

const DRAIN_TIME: i32 = 30;

// Loads a full bucket into the drain: it is held for a while, tipped and emptied, then given back.
fn drain_load(nbt: &mut Compound, item: &str, now: i32) -> bool {
    let Some(fl) = bucket_fluid(item) else { return false };
    if !get_text(nbt, "hi").is_empty() {
        return false;
    }
    let cur = get_int(nbt, "fa", 0);
    let cf = get_text(nbt, "fl");
    if cur + BUCKET > SPOUT_CAP || (cur > 0 && cf != fl) {
        return false;
    }
    nbt.insert(JavaString::from("hi"), Tag::String(JavaString::from(item)));
    put_int(nbt, "ht", now);
    true
}

fn drain_accept(game: &mut Game<'_>, pos: [i32; 3], item: &str) -> bool {
    let it = item.to_string();
    let now = game.ticks as i32;
    let ok = game.native_with_mod_block_entity(pos, move |e| drain_load(&mut e.nbt, &it, now)).unwrap_or(false);
    if ok {
        game.native_block_entity_sync(pos);
        let fl = bucket_fluid(item).unwrap_or("water");
        bucket_sfx(game, fl, false, pos);
    }
    ok
}

fn drain_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound) {
    let now = game.ticks as i32;
    let held = get_text(nbt, "hi");
    if !held.is_empty() {
        if now.wrapping_sub(get_int(nbt, "ht", now)) >= DRAIN_TIME {
            if let Some(fl) = bucket_fluid(&held) {
                let amt = get_int(nbt, "fa", 0);
                fluid_put(nbt, fl, amt + BUCKET);
            }
            nbt.remove(&JavaString::from("hi"));
            nbt.remove(&JavaString::from("ht"));
            let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 1.0, f64::from(pos[2]) + 0.5];
            game.spawn_item(stack_of("minecraft:bucket", 1), at, [0.0, 0.15, 0.0], 10);
            game.native_block_entity_sync(pos);
        }
        return;
    }
    let dim = game.dim.id().to_string();
    let (x, y, z) = (f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
    let found = game.native_item_entities(&dim, [x, y + 0.6, z], [x + 1.0, y + 1.9, z + 1.0]);
    for e in found {
        if e.stack.count == 0 || bucket_fluid(&e.stack.item).is_none() {
            continue;
        }
        let mut probe = nbt.clone();
        if !drain_load(&mut probe, &e.stack.item, now) {
            continue;
        }
        let mut rest = e.stack.clone();
        rest.count -= 1;
        if game.native_set_item_entity(&dim, e.id, rest) {
            drain_load(nbt, &e.stack.item, now);
            game.native_block_entity_sync(pos);
            return;
        }
    }
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

const SPOUT_TIME: i64 = 20;

// A bucket waiting under the spout: on a depot (the fill takes a moment, with a stream) or on a belt.
fn spout_bucket(game: &mut Game<'_>, below: [i32; 3]) -> u8 {
    if let Some(Some((item, _))) = depot_at(game, below) {
        if item == "minecraft:bucket" {
            return 1;
        }
    }
    if game.native_block(below) == "create:belt" {
        let has = game
            .native_with_mod_block_entity(below, |e| belt_items(&e.nbt).iter().any(|b| b.item == "minecraft:bucket" && b.count == 1))
            .unwrap_or(false);
        if has {
            return 2;
        }
    }
    0
}

fn spout_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound) {
    let fluid = get_text(nbt, "fl");
    let amt = get_int(nbt, "fa", 0);
    let working = get_int(nbt, "sw", 0) == 1;
    let below = [pos[0], pos[1] - 1, pos[2]];
    let full = fluid_bucket(&fluid);
    let kind = if amt >= BUCKET && full.is_some() { spout_bucket(game, below) } else { 0 };
    if kind == 0 {
        if working {
            put_int(nbt, "sw", 0);
            game.native_block_entity_sync(pos);
        }
        return;
    }
    let Some(full) = full else { return };
    if kind == 1 {
        if !working {
            put_int(nbt, "sw", 1);
            put_int(nbt, "st", game.ticks as i32);
            game.native_block_entity_sync(pos);
            bucket_sfx(game, &fluid, false, pos);
            return;
        }
        if (game.ticks as i64) - i64::from(get_int(nbt, "st", 0)) < SPOUT_TIME {
            return;
        }
    }
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
        bucket_sfx(game, &fluid, true, pos);
        fluid_put(nbt, &fluid, amt - BUCKET);
        put_int(nbt, "sw", 0);
        game.native_block_entity_sync(pos);
    }
}

// Pipes, tanks and spouts tick every fifth tick.
fn fluid_tick(game: &mut Game<'_>, pos: [i32; 3], entity: &mut ModBlockEntity) {
    if game.serving.is_none() && game.options.dedicated.is_none() {
        return;
    }
    let name = game.native_block(pos).to_string();
    if name == PIPE {
        pipe_sync(game, pos);
        return;
    }
    if (game.ticks as i64 + i64::from(pos[0]) + i64::from(pos[1]) + i64::from(pos[2])).rem_euclid(5) != 0 {
        return;
    }
    if name == TANK {
        if get_int(&entity.nbt, "ex", 0) == 0 {
            put_int(&mut entity.nbt, "ex", 1);
            tank_extend(game, pos);
        }
        tank_sync(game, pos);
    } else if name == SPOUT {
        spout_tick(game, pos, &mut entity.nbt);
    } else if name == DRAIN {
        drain_tick(game, pos, &mut entity.nbt);
    } else if name == GLASS {
        flow_expire(game, pos, &mut entity.nbt);
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
    if game.native_block(pos) == DRAIN && bucket_fluid(&item).is_some() {
        if drain_accept(game, pos, &item) {
            if !game.creative {
                game.consume_held();
            }
            return true;
        }
        return false;
    }
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

fn pump_info(game: &mut Game<'_>, pos: [i32; 3], me: &Node) {
    let speed = game.native_with_mod_block_entity(pos, |e| get_f32(&e.nbt, "speed")).unwrap_or(0.0);
    let amt = game.native_with_mod_block_entity(pos, |e| get_int(&e.nbt, "fa", 0)).unwrap_or(0);
    let flow = if speed >= 0.0 { neg(me.front) } else { me.front };
    let outs = walk(game, pos, flow);
    let ins = walk(game, pos, neg(flow));
    let s = sect();
    let line = s.clone()
        + "7Pump: "
        + &s
        + "f"
        + &(speed as i32).to_string()
        + " rpm, buffer "
        + &amt.to_string()
        + " mB"
        + &s
        + "7 | back: "
        + &s
        + "f"
        + &ins.tanks.len().to_string()
        + " tanks, "
        + &ins.worlds.len().to_string()
        + " open"
        + &s
        + "7 | front: "
        + &s
        + "f"
        + &outs.tanks.len().to_string()
        + " tanks, "
        + &outs.worlds.len().to_string()
        + " open";
    game.native_chat(&line);
}

fn fluid_info(game: &mut Game<'_>, pos: [i32; 3], name: &str) {
    let pool = pool_of(game, pos);
    let total = pool.each * pool.cells.len() as i32;
    let label = match name {
        TANK => "Fluid Tank",
        DRAIN => "Item Drain",
        _ => "Spout",
    };
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
    if name == GLASS {
        let look = nearest_looking(context.yaw, context.pitch);
        if let Some(s) = level.native_state_with_property(state, "axis", axis_char(axis_of(look))) {
            *answer = Some(Some(s));
        }
        return;
    }
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
            "minecraft:dandelion" | "minecraft:sunflower" => Some(rec_of(100, &[("minecraft:yellow_dye", 2, 1.0)])),
            "minecraft:poppy" | "minecraft:red_tulip" | "minecraft:rose_bush" => {
                Some(rec_of(100, &[("minecraft:red_dye", 2, 1.0)]))
            }
            "minecraft:blue_orchid" => Some(rec_of(100, &[("minecraft:light_blue_dye", 2, 1.0)])),
            "minecraft:allium" => Some(rec_of(100, &[("minecraft:magenta_dye", 2, 1.0)])),
            "minecraft:azure_bluet" | "minecraft:oxeye_daisy" | "minecraft:white_tulip" => {
                Some(rec_of(100, &[("minecraft:light_gray_dye", 2, 1.0)]))
            }
            "minecraft:orange_tulip" => Some(rec_of(100, &[("minecraft:orange_dye", 2, 1.0)])),
            "minecraft:pink_tulip" | "minecraft:peony" => Some(rec_of(100, &[("minecraft:pink_dye", 2, 1.0)])),
            "minecraft:cornflower" => Some(rec_of(100, &[("minecraft:blue_dye", 2, 1.0)])),
            "minecraft:lily_of_the_valley" => Some(rec_of(100, &[("minecraft:white_dye", 2, 1.0)])),
            "minecraft:sugar_cane" => Some(rec_of(100, &[("minecraft:sugar", 2, 1.0)])),
            "minecraft:coal_ore" | "minecraft:deepslate_coal_ore" => Some(rec_of(150, &[("minecraft:coal", 1, 1.0)])),
            "minecraft:cobblestone" => Some(rec_of(150, &[("minecraft:gravel", 1, 1.0)])),
            "minecraft:gravel" => Some(rec_of(150, &[("minecraft:sand", 1, 1.0)])),
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
                    if timer > 0 && (game.ticks as i64 + i64::from(pos[0])).rem_euclid(3) == 0 {
                        let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 1.0, f64::from(pos[2]) + 0.5];
                        spray(game, at, Some(&item), 3);
                    }
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
// A saw facing sideways saws the log in front of it (with a crack); the logs joined to it above and beside
// it fall with it. A saw facing up is a piece of a belt line and cuts what passes over it.

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
        if (game.ticks as i64 + i64::from(pos[0])).rem_euclid(2) == 0 {
            let at = [f64::from(target[0]) + 0.5, f64::from(target[1]) + 0.5, f64::from(target[2]) + 0.5];
            spray(game, at, Some(&name), 4);
        }
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

fn spray(game: &mut Game<'_>, at: [f64; 3], item: Option<&str>, count: u32) {
    let options = match item {
        Some(i) => game.item_particle(&stack_of(i, 1)),
        None => horizon_client::particles::ParticleOptions::simple(horizon_client::particles::ParticleType::Smoke),
    };
    let smoke = horizon_client::particles::ParticleOptions::simple(horizon_client::particles::ParticleType::Smoke);
    let cell = [at[0].floor() as i32, at[1].floor() as i32, at[2].floor() as i32];
    for k in 0..count as i32 {
        let a = f64::from(pseudo(game, cell, k * 7 + 1)) - 0.5;
        let b = f64::from(pseudo(game, cell, k * 7 + 2)) - 0.5;
        let c = f64::from(pseudo(game, cell, k * 7 + 3)) - 0.5;
        let d = f64::from(pseudo(game, cell, k * 7 + 4)) - 0.5;
        let e = f64::from(pseudo(game, cell, k * 7 + 5)) - 0.5;
        let pos = [at[0] + a * 0.5, at[1] + b * 0.2, at[2] + c * 0.5];
        let vel = [d * 0.12, 0.05 + e * 0.05, c * 0.12];
        // always shown: the particle setting and the distance limiter do not drop these
        game.particles.add_with(&options, pos, vel, true, true);
    }
    game.particles.add_with(&smoke, at, [0.0, 0.03, 0.0], true, true);
}

// Puts a stack on a belt piece (a free spot) or a free depot; false when there is neither.
fn put_on(game: &mut Game<'_>, target: [i32; 3], item: &str, count: u32) -> bool {
    if let Some(items) = belt_items_at(game, target) {
        let Some(spot) = belt_free_spot(&items) else { return false };
        let mut next = items;
        next.push(BeltItem { item: item.to_string(), count, p: spot, moving: false });
        let _ = game.native_with_mod_block_entity(target, move |e| belt_store(&mut e.nbt, &next));
        game.native_block_entity_sync(target);
        return true;
    }
    if let Some(None) = depot_at(game, target) {
        depot_set(game, target, Some((item.to_string(), count)));
        return true;
    }
    false
}

// Puts a stack on a belt piece (a free spot), on a free depot, or out into the air; false when it is full.
fn saw_eject(game: &mut Game<'_>, pos: [i32; 3], m: [i32; 3], item: &str, count: u32) -> bool {
    let target = add3(pos, m);
    if let Some(items) = belt_items_at(game, target) {
        let Some(spot) = belt_free_spot(&items) else { return false };
        let mut next = items;
        next.push(BeltItem { item: item.to_string(), count, p: spot, moving: false });
        let _ = game.native_with_mod_block_entity(target, move |e| belt_store(&mut e.nbt, &next));
        game.native_block_entity_sync(target);
        return true;
    }
    match depot_at(game, target) {
        Some(None) => {
            depot_set(game, target, Some((item.to_string(), count)));
            return true;
        }
        Some(Some(_)) => return false,
        None => {}
    }
    let at = [
        f64::from(pos[0]) + 0.5 + f64::from(m[0]) * 0.7,
        f64::from(pos[1]) + 1.0,
        f64::from(pos[2]) + 0.5 + f64::from(m[2]) * 0.7,
    ];
    game.spawn_item(stack_of(item, count), at, [f64::from(m[0]) * 0.12, 0.1, f64::from(m[2]) * 0.12], 10);
    true
}

// A belt hands an item to a saw that faces up and is empty.
fn saw_accept(game: &mut Game<'_>, p: [i32; 3], item: &BeltItem) -> bool {
    let name = item.item.clone();
    let count = item.count;
    let ok = game
        .native_with_mod_block_entity(p, move |e| {
            if !pair_list(&e.nbt, "si").is_empty() || !pair_list(&e.nbt, "so").is_empty() {
                return false;
            }
            pair_store(&mut e.nbt, "si", &[(name, count)]);
            put_int(&mut e.nbt, "st", -1);
            true
        })
        .unwrap_or(false);
    if ok {
        game.native_block_entity_sync(p);
    }
    ok
}

// A saw facing up works as in Create: it is a piece of a belt line. Items lying on it or handed over by a
// belt are cut while the saw turns and leave the other side (the way the saw turns decides which side).
fn saw_top(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, me: &Node, speed: f32) -> bool {
    let mut changed = false;
    let mut input = pair_list(nbt, "si");
    let mut outs = pair_list(nbt, "so");
    let phase = (game.ticks as i64 + i64::from(pos[0]) + i64::from(pos[2])).rem_euclid(4);
    if input.is_empty() && outs.is_empty() && phase == 0 {
        let dim = game.dim.id().to_string();
        let (x, y, z) = (f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
        let found = game.native_item_entities(&dim, [x, y + 0.9, z], [x + 1.0, y + 1.8, z + 1.0]);
        if let Some(e) = found.into_iter().next() {
            let mut gone = e.stack.clone();
            gone.count = 0;
            if game.native_set_item_entity(&dim, e.id, gone) {
                input = vec![(e.stack.item.to_string(), e.stack.count)];
                put_int(nbt, "st", -1);
                changed = true;
            }
        }
    }
    let off = if speed < 0.0 { -1 } else { 1 };
    let mdir = if me.first { [0, 0, -off] } else { [off, 0, 0] };
    if speed.abs() >= 1.0 {
        if let Some((item, count)) = outs.first().cloned() {
            let chunk = count.min(64);
            if saw_eject(game, pos, mdir, &item, chunk) {
                if count > chunk {
                    outs[0].1 = count - chunk;
                } else {
                    outs.remove(0);
                }
                changed = true;
            }
        }
    }
    let mut timer = get_int(nbt, "st", -1);
    if outs.is_empty() && speed.abs() >= 1.0 {
        if let Some((item, count)) = input.first().cloned() {
            let rec = recipe_for(game, "create:cutting", &item);
            if timer < 0 {
                timer = rec.as_ref().map(|r| r.time).unwrap_or(10);
                put_int(nbt, "sm", timer.max(1));
            }
            timer -= ((speed.abs() / 16.0) as i32).clamp(1, 512);
            if phase % 2 == 0 {
                let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 1.05, f64::from(pos[2]) + 0.5];
                spray(game, at, Some(&item), 3);
                let total = get_int(nbt, "sm", 1).max(1);
                put_int(nbt, "sp", (100 * (total - timer.max(0)) / total).clamp(0, 100));
                changed = true;
            }
            if timer <= 0 {
                outs = match &rec {
                    Some(r) => roll_outputs(game, pos, r, count),
                    None => vec![(item, count)],
                };
                input.clear();
                timer = -1;
                put_int(nbt, "sp", 0);
                changed = true;
            }
        } else {
            timer = -1;
        }
    }
    put_int(nbt, "st", timer);
    if changed {
        pair_store(nbt, "si", &input);
        pair_store(nbt, "so", &outs);
    }
    changed
}

fn saw_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, me: &Node, speed: f32) -> bool {
    if !server_side(game) {
        return false;
    }
    if me.front[1] > 0 {
        return saw_top(game, pos, nbt, me, speed);
    }
    if me.front[1] < 0 || speed.abs() < 1.0 {
        crack_clear(game, pos, nbt);
        return false;
    }
    let target = [pos[0] + me.front[0], pos[1] + me.front[1], pos[2] + me.front[2]];
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

// The wheel two blocks away across a gap (horizontal only): the gap block, the gap axis and the speed of the
// other wheel. Only the wheel with the smaller coordinate looks forward; info looks both ways.
fn crusher_gap(game: &mut Game<'_>, pos: [i32; 3], me: &Node) -> Option<([i32; 3], usize, f32)> {
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
        let mut g = pos;
        g[a] += 1;
        return Some((g, a, r.speed));
    }
    None
}

// What a click on a crushing wheel says about its pair.
fn crusher_info(game: &mut Game<'_>, pos: [i32; 3]) {
    let Some(me) = node_at(game, pos) else { return };
    let mine = game.native_with_mod_block_entity(pos, |e| get_f32(&e.nbt, "speed")).unwrap_or(0.0);
    let mut seen: Option<f32> = None;
    for a in [0usize, 2usize] {
        if a == me.axis {
            continue;
        }
        for sgn in [-2, 2] {
            let mut p = pos;
            p[a] += sgn;
            if game.native_block(p) != "create:crushing_wheel" {
                continue;
            }
            let Some(nb) = node_at(game, p) else { continue };
            if nb.axis != me.axis {
                continue;
            }
            if let Some(r) = read_neighbour(game, p) {
                seen = Some(r.speed);
            }
        }
    }
    let text = match seen {
        None => "no wheel two blocks away on the same axis",
        Some(other) => {
            if other * mine < 0.0 {
                "pair ready: drop items into the gap"
            } else {
                "the wheels turn the same way: reverse one of them"
            }
        }
    };
    let s = sect();
    let line = s.clone() + "7Crushing Wheel: " + &s + "f" + text;
    game.native_chat(&line);
}

// A belt ending at the gap between two wheels hands the item to the wheel pair.
fn crusher_accept(game: &mut Game<'_>, next: [i32; 3], item: &BeltItem) -> bool {
    for a in [0usize, 2usize] {
        let mut lo = next;
        lo[a] -= 1;
        let mut hi = next;
        hi[a] += 1;
        if game.native_block(lo) != "create:crushing_wheel" || game.native_block(hi) != "create:crushing_wheel" {
            continue;
        }
        let Some(rec) = recipe_for(game, "create:crushing", &item.item) else { return false };
        let name = item.item.clone();
        let count = item.count;
        let time = rec.time as f32;
        return game
            .native_with_mod_block_entity(lo, move |e| {
                if !triple_list(&e.nbt, "cq").is_empty() {
                    return false;
                }
                triple_store(&mut e.nbt, "cq", &[(name, count, time)]);
                true
            })
            .unwrap_or(false);
    }
    false
}

fn crush_tick(game: &mut Game<'_>, pos: [i32; 3], nbt: &mut Compound, me: &Node, speed: f32) -> bool {
    if !server_side(game) {
        return false;
    }
    let mut queue = triple_list(nbt, "cq");
    let mut changed = false;
    let found_gap = crusher_gap(game, pos, me);
    let mut gap: Option<([i32; 3], usize)> = None;
    if let Some((g, a, other)) = found_gap {
        if other * speed < 0.0 {
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
        if queue.is_empty() {
            let found = game.native_item_entities(&dim, lo, hi);
            for e in found {
                if !queue.is_empty() {
                    break;
                }
                let name = e.stack.item.to_string();
                let Some(rec) = recipe_for(game, "create:crushing", &name) else { continue };
                let mut gone = e.stack.clone();
                gone.count = 0;
                if game.native_set_item_entity(&dim, e.id, gone) {
                    queue.push((name.clone(), e.stack.count, rec.time as f32));
                    changed = true;
                    spray(game, [f64::from(g[0]) + 0.5, f64::from(g[1]) + 0.8, f64::from(g[2]) + 0.5], Some(&name), 6);
                }
            }
        }
        let rate = (speed.abs() / 50.0).clamp(1.0, 512.0);
        let mut keep: Vec<(String, u32, f32)> = Vec::new();
        for (item, count, left) in queue.drain(..) {
            let left = left - rate;
            if left > 0.0 {
                if (game.ticks as i64).rem_euclid(3) == 0 {
                    spray(game, [f64::from(g[0]) + 0.5, f64::from(g[1]) + 1.0, f64::from(g[2]) + 0.5], Some(&item), 3);
                }
                keep.push((item, count, left));
                continue;
            }
            if let Some(rec) = recipe_for(game, "create:crushing", &item) {
                let outs = roll_outputs(game, g, &rec, count);
                let at = [f64::from(g[0]) + 0.5, f64::from(g[1]) + 0.2, f64::from(g[2]) + 0.5];
                for o in outs {
                    spray(game, at, Some(&o.0), 6);
                    let n = o.1.min(99);
                    if !put_on(game, [g[0], g[1] - 1, g[2]], &o.0, n) {
                        game.spawn_item(stack_of(&o.0, n), at, [0.0, -0.1, 0.0], 10);
                    }
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
    let due = ((game.ticks as i64) + i64::from(pos[0]) + i64::from(pos[2])).rem_euclid(5) == 0;
    if let Some((item, count)) = pair_list(nbt, "co").first().cloned() {
        if due {
            let Some((_, d)) = crafter_at(game, pos) else { return false };
            if put_on(game, add3(pos, d), &item, count) {
                pair_store(nbt, "co", &[]);
                return true;
            }
        }
        return false;
    }
    if speed.abs() < 1.0 {
        put_int(nbt, "ct", 0);
        return false;
    }
    if !due {
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
    let mut levels: Vec<i32> = vec![0];
    let mut i = 0;
    while i < members.len() && members.len() < 81 {
        let c = members[i];
        i += 1;
        let level = levels[i - 1];
        for d in plane {
            let q = add3(c, d);
            if members.contains(&q) {
                continue;
            }
            if let Some((f2, d2)) = crafter_at(game, q) {
                if f2 == front && add3(q, d2) == c {
                    members.push(q);
                    levels.push(level + 1);
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
    let inc = ((speed.abs() / 4.0) as i32).clamp(1, 64);
    let progress = get_int(nbt, "ct", 0) + inc;
    // the items slide along the arrows to the last crafter while the work goes on; every crafter
    // keeps the progress, the hops to go and the time it was written, for the drawing
    let hops = levels.iter().copied().max().unwrap_or(0).max(1);
    let stamp = game.ticks as i32;
    let pct = (progress.min(60) * 100 / 60).min(100);
    let rate = inc * 100 / 60;
    for m in members.iter() {
        if *m == pos {
            put_int(nbt, "cp", pct);
            put_int(nbt, "cr", rate);
            put_int(nbt, "ch", hops);
            put_int(nbt, "cs", stamp);
        } else {
            let _ = game.native_with_mod_block_entity(*m, move |e| {
                put_int(&mut e.nbt, "cp", pct);
                put_int(&mut e.nbt, "cr", rate);
                put_int(&mut e.nbt, "ch", hops);
                put_int(&mut e.nbt, "cs", stamp);
            });
            game.native_block_entity_sync(*m);
        }
    }
    if progress < 60 {
        put_int(nbt, "ct", progress);
        return true;
    }
    put_int(nbt, "ct", 0);
    put_int(nbt, "cp", 0);
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
                put_int(&mut e.nbt, "cp", 0);
            });
            game.native_block_entity_sync(*m);
        }
    }
    pair_store(nbt, "co", &[(result.item.to_string(), result.count)]);
    if put_on(game, add3(pos, dirv), &result.item.to_string(), result.count) {
        pair_store(nbt, "co", &[]);
    }
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
    if held_item.is_none() {
        let out = game
            .native_with_mod_block_entity(pos, |e| {
                let v = pair_list(&e.nbt, "co");
                pair_store(&mut e.nbt, "co", &[]);
                v
            })
            .unwrap_or_default();
        if !out.is_empty() {
            game.native_block_entity_sync(pos);
            give_player(game, pos, &out);
            return true;
        }
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
                let take = room.min(count).min(1);
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
    if entity.is_none() && block == BASIN_BLOCK {
        *entity = Some((BASIN_BE.to_string(), Compound::new()));
        return;
    }
    if entity.is_none() && funnel_brass(block).is_some() {
        *entity = Some((FUNNEL_BE.to_string(), Compound::new()));
        return;
    }
    if entity.is_none() && block == BURNER_BLOCK {
        *entity = Some((BURNER_BE.to_string(), Compound::new()));
        return;
    }
    if entity.is_none() && kind_of(block).is_some() {
        let mut c = Compound::new();
        set_state(&mut c, 0.0, NONE_HOPS, None);
        *entity = Some((BE_ID.to_string(), c));
    }
}

pub fn ticks(block: &str, ticks: &mut bool) {
    if kind_of(block).is_some() || is_fluid_block(block) || block == BASIN_BLOCK || block == BURNER_BLOCK || funnel_brass(block).is_some() {
        *ticks = true;
    }
}

pub fn tick(game: &mut Game<'_>, pos: [i32; 3], entity: &mut ModBlockEntity) {
    if entity.id == FLUID_BE {
        fluid_tick(game, pos, entity);
        return;
    }
    if entity.id == BASIN_BE {
        if basin_tick(game, pos, &mut entity.nbt) {
            game.native_block_entity_sync(pos);
        }
        return;
    }
    if entity.id == FUNNEL_BE {
        let _ = funnel_tick(game, pos, &mut entity.nbt);
        return;
    }
    if entity.id == BURNER_BE {
        if burner_tick(game, pos, &mut entity.nbt) {
            game.native_block_entity_sync(pos);
        }
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
    if me.kind == Kind::Mixer && mixer_tick(game, pos, &mut entity.nbt, out.speed) {
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
    if me.kind == Kind::Hose {
        let moved = hose_tick(game, pos, &mut entity.nbt, out.speed);
        let was = get_int(&entity.nbt, "hm", 0) == 1;
        if moved {
            put_int(&mut entity.nbt, "hm", 1);
            game.native_block_entity_sync(pos);
        } else if was {
            // the hose stopped: it settles on a whole block, where the real rope and magnet blocks stand
            put_int(&mut entity.nbt, "hm", 0);
            let l = get_f32(&entity.nbt, "hl");
            let mut t = l.round().clamp(0.0, HOSE_MAX);
            if t > l && !hose_free(game, [pos[0], pos[1] - 1 - (t as i32), pos[2]]) {
                t = l.floor();
            }
            put_f32(&mut entity.nbt, "hl", t);
            game.native_block_entity_sync(pos);
        }
        if moved || was || (game.ticks as i64 + i64::from(pos[0]) + i64::from(pos[2])).rem_euclid(20) == 0 {
            let l = get_f32(&entity.nbt, "hl");
            hose_blocks(game, pos, l, moved);
        }
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
        match depot_content(&entity.nbt) {
            Some(content) => depot_put(&mut c, Some(content)),
            None => {
                // an explicit empty, so a client that merges the tag drops the old item
                c.insert(JavaString::from("item"), Tag::String(JavaString::from("")));
                put_int(&mut c, "count", 0);
            }
        }
        let ex = get_text(&entity.nbt, "ex");
        c.insert(JavaString::from("ex"), Tag::String(JavaString::from(ex.as_str())));
        *tag = Some(c);
        return;
    }
    if entity.id == BURNER_BE {
        let mut c = Compound::new();
        put_int(&mut c, "fuel", get_int(&entity.nbt, "fuel", 0));
        put_int(&mut c, "sup", get_int(&entity.nbt, "sup", 0));
        *tag = Some(c);
        return;
    }
    if entity.id == BASIN_BE {
        let mut c = Compound::new();
        for key in ["in", "out"] {
            let text = get_text(&entity.nbt, key);
            c.insert(JavaString::from(key), Tag::String(JavaString::from(text.as_str())));
        }
        put_int(&mut c, "mix", get_int(&entity.nbt, "mix", 0));
        *tag = Some(c);
        return;
    }
    if entity.id == FLUID_BE {
        let mut c = Compound::new();
        let fl = get_text(&entity.nbt, "fl");
        c.insert(JavaString::from("fl"), Tag::String(JavaString::from(fl.as_str())));
        put_int(&mut c, "fa", get_int(&entity.nbt, "fa", 0));
        let ff = get_text(&entity.nbt, "ff");
        c.insert(JavaString::from("ff"), Tag::String(JavaString::from(ff.as_str())));
        put_int(&mut c, "fd", get_int(&entity.nbt, "fd", 0));
        put_int(&mut c, "sw", get_int(&entity.nbt, "sw", 0));
        let hi = get_text(&entity.nbt, "hi");
        c.insert(JavaString::from("hi"), Tag::String(JavaString::from(hi.as_str())));
        put_int(&mut c, "ht", get_int(&entity.nbt, "ht", 0));
        *tag = Some(c);
        return;
    }
    if entity.id == BE_ID {
        let mut c = Compound::new();
        put_f32(&mut c, "speed", get_f32(&entity.nbt, "speed"));
        put_f32(&mut c, "pp", get_f32(&entity.nbt, "pp"));
        put_f32(&mut c, "mp", get_f32(&entity.nbt, "mp"));
        put_f32(&mut c, "hl", get_f32(&entity.nbt, "hl"));
        let bi = get_text(&entity.nbt, "bi");
        c.insert(JavaString::from("bi"), Tag::String(JavaString::from(bi.as_str())));
        put_int(&mut c, "sp", get_int(&entity.nbt, "sp", 0));
        for key in ["cp", "cr", "ch", "cs"] {
            put_int(&mut c, key, get_int(&entity.nbt, key, 0));
        }
        for key in ["ci", "si", "co"] {
            let text = get_text(&entity.nbt, key);
            c.insert(JavaString::from(key), Tag::String(JavaString::from(text.as_str())));
        }
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

// Which of the n stacks on the depot the crosshair is nearest to: the view ray is cut with the depot box
// and the stack whose place is closest to where it enters is the one.
fn depot_aim_slot(game: &Game<'_>, pos: [i32; 3], n: usize) -> usize {
    let Some((o, d)) = game.aim() else { return 0 };
    let lo = [f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2])];
    let hi = [lo[0] + 1.0, lo[1] + 0.8125, lo[2] + 1.0];
    let mut t_in = 0.0f64;
    for a in 0..3 {
        if d[a].abs() < 1e-9 {
            continue;
        }
        let (t0, t1) = ((lo[a] - o[a]) / d[a], (hi[a] - o[a]) / d[a]);
        t_in = t_in.max(t0.min(t1));
    }
    let hx = o[0] + d[0] * t_in - lo[0];
    let hz = o[2] + d[2] * t_in - lo[2];
    let mut best = 0usize;
    let mut best_d = f64::MAX;
    for k in 0..n.min(DEPOT_SLOTS) {
        let [dx, dz] = depot_spot(n.min(DEPOT_SLOTS), k);
        let dist = (hx - (0.5 + f64::from(dx))).powi(2) + (hz - (0.5 + f64::from(dz))).powi(2);
        if dist < best_d {
            best_d = dist;
            best = k;
        }
    }
    best
}

// A click on the depot. By hand it has one slot: a held stack goes on only when the depot is empty (or
// tops up the same item); the others, results and by-products, come from the press, a basin or a funnel.
// An empty hand takes the whole stack the crosshair is on.
fn depot_click(game: &mut Game<'_>, pos: [i32; 3]) {
    if depot_at(game, pos).is_none() {
        return;
    }
    let held = game.held().cloned();
    if let Some(h) = held {
        if h.count > 0 {
            let item = h.item.to_string();
            let count = h.count;
            let added = game
                .native_with_mod_block_entity(pos, move |e| {
                    let mut list = depot_list(&e.nbt);
                    let slots = if list.is_empty() { 1 } else { list.len() };
                    let n = stacks_add(&mut list, &item, count, slots);
                    if n > 0 {
                        depot_store(&mut e.nbt, &list);
                    }
                    n
                })
                .unwrap_or(0);
            if added > 0 {
                sfx(game, "minecraft:entity.item.frame.add_item", pos, 0.6, 1.0);
                game.native_block_entity_sync(pos);
                if added >= count {
                    *game.held_mut() = None;
                } else {
                    let mut rest = h.clone();
                    rest.count = count - added;
                    *game.held_mut() = Some(rest);
                }
            }
            return;
        }
    }
    let have = game.native_with_mod_block_entity(pos, |e| depot_list(&e.nbt).len()).unwrap_or(0);
    if have == 0 {
        return;
    }
    let idx = depot_aim_slot(game, pos, have);
    let taken = game
        .native_with_mod_block_entity(pos, move |e| {
            let mut list = depot_list(&e.nbt);
            if idx >= list.len() {
                return None;
            }
            let st = list.remove(idx);
            depot_store(&mut e.nbt, &list);
            Some(st)
        })
        .flatten();
    if let Some((item, count)) = taken {
        sfx(game, "minecraft:entity.item.frame.remove_item", pos, 0.6, 1.0);
        game.native_block_entity_sync(pos);
        let at = at_player(game, pos);
        game.spawn_item(stack_of(&item, count), at, [0.0, 0.0, 0.0], 0);
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
            v.push(BeltItem { item, count, p, moving: false });
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
        let all = game.native_with_mod_block_entity(pos, |e| depot_list(&e.nbt)).unwrap_or_default();
        let at = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
        for (item, count) in all {
            game.spawn_item(stack_of(&item, count), at, [0.0, 0.1, 0.0], 10);
        }
    } else if name == "create:millstone" {
        spill_lists(game, pos, &["mi", "mo"]);
    } else if name == "create:mechanical_crafter" {
        spill_lists(game, pos, &["ci", "co"]);
    } else if name == BASIN_BLOCK {
        basin_spill(game, pos);
    } else if name == "create:belt" {
        belt_spill(game, pos);
        belt_dismantle(game, pos, Some(pos));
    } else if name == HOSE {
        let dim = game.dim.id().to_string();
        for k in 1..=(HOSE_MAX as i32 + 2) {
            let cell = [pos[0], pos[1] - k, pos[2]];
            let here = game.native_block(cell).to_string();
            if here == ROPE || here == MAGNET {
                game.native_set_block(&dim, cell, BlockStateId(0), 3);
            }
        }
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
    if name == TANK || name == SPOUT || name == DRAIN {
        let bucket = held_item.as_deref().map(|h| h == "minecraft:bucket" || bucket_fluid(h).is_some()).unwrap_or(false);
        if bucket {
            // Every click event of a tap is cancelled, so the bucket never places water beside the block.
            *canceled = true;
        }
        if fresh_click(game, pos) {
            if bucket_click(game, pos) {
                *canceled = true;
                return;
            }
            if !bucket {
                fluid_info(game, pos, &name);
            }
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
    if name == BASIN_BLOCK {
        if fresh_click(game, pos) {
            basin_click(game, pos);
        }
        *canceled = true;
        return;
    }
    if let Some(brass) = funnel_brass(&name) {
        if fresh_click(game, pos) {
            funnel_click(game, pos, brass, held_item.as_deref());
        }
        *canceled = true;
        return;
    }
    if name == BURNER_BLOCK && held_item.is_some() {
        if fresh_click(game, pos) && burner_click(game, pos) {
            *canceled = true;
            return;
        }
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
    if me.kind == Kind::Pump && held_item.is_none() && fresh_click(game, pos) {
        pump_info(game, pos, &me);
    }
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
    if me.kind == Kind::Crusher {
        crusher_info(game, pos);
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
    if me.kind == Kind::Hose {
        let l = game.native_with_mod_block_entity(pos, |e| get_f32(&e.nbt, "hl")).unwrap_or(0.0);
        let s = sect();
        let line = s.clone() + "7Hose length: " + &s + "f" + &((l * 10.0).round() / 10.0).to_string() + &s + "7 blocks (turn one way to lower, the other to raise)";
        game.native_chat(&line);
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
    if block == "create:creative_motor" {
        draw.light = open_light(game, pos);
    }
    if block == BASIN_BLOCK {
        basin_submit(game, pos, entity, draw);
        return;
    }
    if block == BURNER_BLOCK {
        burner_submit(game, entity, draw);
        return;
    }
    if block == "create:mechanical_mixer" {
        mixer_submit(game, pos, state, entity, draw);
        return;
    }
    if block == DEPOT_BLOCK {
        let list = depot_list(&entity.nbt);
        let n = list.len().min(DEPOT_SLOTS);
        for (k, (item, count)) in list.iter().take(n).enumerate() {
            let stack = ItemStack { item: Arc::from(item.as_str()), count: *count, damage: 0, extra: None };
            let [dx, dz] = depot_spot(n, k);
            let flat = flat_item(game, &stack);
            // a flat item lies on the depot as in Create; a solid item sits on its top (13 of 16)
            let top = if flat {
                0.84
            } else {
                let (lo, _) = game.resolve_item(&stack, DisplayContext::Ground, None, 0).bounding_box();
                0.8125 - lo.y + 0.005
            };
            for c in 0..stack_copies(*count) {
                let o = copy_offset(c, !flat);
                draw.push();
                draw.translate([0.5 + dx + o[0], top + o[1], 0.5 + dz + o[2]]);
                if flat {
                    draw.rotate_about([1.0, 0.0, 0.0], 90.0, [0.0, 0.0, 0.0]);
                }
                game.native_draw_item(draw, &stack, DisplayContext::Ground, None, pos[0] ^ pos[2] ^ (k as i32) ^ (c as i32));
                draw.pop();
            }
        }
        return;
    }
    if block == SPOUT {
        let fl = get_text(&entity.nbt, "fl");
        if get_int(&entity.nbt, "sw", 0) == 1 && !fl.is_empty() {
            game.native_draw_block(draw, "create:spout_stream_part", &[("fluid", fl.as_str())]);
        }
        return;
    }
    if block == GLASS {
        let fl = get_text(&entity.nbt, "ff");
        let Some(a) = game.native_block_state_property(state, "axis") else { return };
        let Some(axis) = parse_axis(&a.to_string()) else { return };
        if fl.is_empty() {
            draw.keep();
            return;
        }
        let dir = DIRS[(get_int(&entity.nbt, "fd", 0).clamp(0, 5)) as usize];
        let t = draw.game_time as f64 + f64::from(draw.partial);
        let mut frame = ((t * 0.8) as i64).rem_euclid(8);
        if dir[axis] < 0 {
            frame = 7 - frame;
        }
        let frame = frame.to_string();
        game.native_draw_block(draw, "create:pipe_flow_part", &[("axis", axis_char(axis)), ("fluid", fl.as_str()), ("frame", frame.as_str())]);
        return;
    }
    if block == DRAIN {
        let mut fl = get_text(&entity.nbt, "fl");
        let mut amt = get_int(&entity.nbt, "fa", 0);
        let hi = get_text(&entity.nbt, "hi");
        if !hi.is_empty() {
            let el = draw.game_time as f32 + draw.partial - get_int(&entity.nbt, "ht", 0) as f32;
            let prog = (el / DRAIN_TIME as f32).clamp(0.0, 1.0);
            let tilt = (el / 10.0).clamp(0.0, 1.0) * 75.0;
            let stack = ItemStack { item: Arc::from(hi.as_str()), count: 1, damage: 0, extra: None };
            draw.push();
            draw.translate([0.5, 1.0, 0.5]);
            draw.rotate_about([0.0, 0.0, 1.0], -tilt, [0.0, 0.0, 0.0]);
            game.native_draw_item(draw, &stack, DisplayContext::Ground, None, pos[0] ^ pos[2]);
            draw.pop();
            if let Some(f) = bucket_fluid(&hi) {
                if el >= 8.0 {
                    game.native_draw_block(draw, "create:pour_part", &[("fluid", f)]);
                }
                if fl.is_empty() {
                    fl = f.to_string();
                }
                amt += (BUCKET as f32 * prog) as i32;
            }
        }
        if amt > 0 && !fl.is_empty() {
            let level = ((amt * 8 + SPOUT_CAP - 1) / SPOUT_CAP).clamp(1, 8).to_string();
            game.native_draw_block(draw, "create:drain_fluid_part", &[("fluid", fl.as_str()), ("level", level.as_str())]);
        }
        return;
    }
    if block == PIPE {
        for d in DIRS.iter() {
            if !pipe_open(game, pos, *d) {
                continue;
            }
            let nb = game.native_block(add3(pos, *d));
            if nb == PIPE || nb == GLASS {
                continue;
            }
            game.native_draw_block(draw, "create:pipe_rim_part", &[("facing", dir_name(*d))]);
        }
        return;
    }
    if block == HOSE {
        let Some(f) = game.native_block_state_property(state, "facing") else { return };
        let f = f.to_string();
        let Some(front) = parse_dir(&f) else { return };
        let speed = f64::from(get_f32(&entity.nbt, "speed"));
        let l = get_f32(&entity.nbt, "hl") + ((speed / 500.0) as f32) * draw.partial;
        let l = l.clamp(0.0, HOSE_MAX);
        let axis = axis_of(cw(front));
        let mut about = [0.0f32; 3];
        about[axis] = 1.0;
        draw.push();
        draw.rotate_about(about, (speed * 0.3 * (draw.game_time as f64 + f64::from(draw.partial))) as f32 % 360.0, [0.5, 0.5, 0.5]);
        game.native_draw_block(draw, "create:hose_coil_part", &[("facing", f.as_str())]);
        draw.pop();
        half_shaft(game, pos, cw(front), speed, draw);
        let whole = l.floor() as i32;
        let part = l - whole as f32;
        for k in 1..=whole {
            if game.native_block([pos[0], pos[1] - k, pos[2]]) == ROPE {
                continue;
            }
            draw.push();
            draw.translate([0.0, -(k as f32), 0.0]);
            game.native_draw_block(draw, "create:hose_rope_part", &[]);
            draw.pop();
        }
        if part > 0.01 {
            draw.push();
            draw.translate([0.0, -l, 0.0]);
            game.native_draw_block(draw, "create:hose_rope_part", &[]);
            draw.pop();
        }
        if part > 0.01 || game.native_block([pos[0], pos[1] - 1 - whole, pos[2]]) != MAGNET {
            draw.push();
            draw.translate([0.0, -(l + 1.0), 0.0]);
            game.native_draw_block(draw, "create:hose_magnet_part", &[]);
            draw.pop();
        }
        return;
    }
    if block == TANK {
        let fl = get_text(&entity.nbt, "fl");
        let amt = get_int(&entity.nbt, "fa", 0);
        if amt > 0 && !fl.is_empty() {
            let level = ((amt * 8 + TANK_CAP - 1) / TANK_CAP).clamp(1, 8).to_string();
            let mut ext = 0;
            let layer = tank_layer(game, pos);
            for (bit, d) in [[-1, 0, 0], [1, 0, 0], [0, 0, -1], [0, 0, 1]].iter().enumerate() {
                if layer.contains(&add3(pos, *d)) {
                    ext |= 1 << bit;
                }
            }
            let has = |k: &str| game.native_block_state_property(state, k).map(|v| v.to_string() == "true").unwrap_or(false);
            if has("bottom") {
                ext |= 16;
            }
            if has("top") {
                ext |= 32;
            }
            let ext = ext.to_string();
            game.native_draw_block(draw, "create:fluid_level_part", &[("ext", ext.as_str()), ("fluid", fl.as_str()), ("level", level.as_str())]);
        }
        // Tanks side by side show one big cap on top and bottom: each tank draws its share of it.
        let cells = tank_layer(game, pos);
        if cells.len() > 1 {
            let minx = cells.iter().map(|c| c[0]).min().unwrap_or(pos[0]);
            let maxx = cells.iter().map(|c| c[0]).max().unwrap_or(pos[0]);
            let minz = cells.iter().map(|c| c[2]).min().unwrap_or(pos[2]);
            let maxz = cells.iter().map(|c| c[2]).max().unwrap_or(pos[2]);
            let (w, d) = (maxx - minx + 1, maxz - minz + 1);
            if cells.len() as i32 == w * d && w <= 3 && d <= 3 {
                let ux = w.to_string() + "_" + &(pos[0] - minx).to_string();
                let uz = d.to_string() + "_" + &(pos[2] - minz).to_string();
                let top = game.native_block_state_property(state, "top").map(|v| v.to_string() == "true").unwrap_or(false);
                let bottom = game.native_block_state_property(state, "bottom").map(|v| v.to_string() == "true").unwrap_or(false);
                if top {
                    game.native_draw_block(draw, "create:tank_cap_part", &[("face", "up"), ("ux", ux.as_str()), ("uz", uz.as_str())]);
                }
                if bottom {
                    game.native_draw_block(draw, "create:tank_cap_part", &[("face", "down"), ("ux", ux.as_str()), ("uz", uz.as_str())]);
                }
            }
        }
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
        let now = draw.game_time as f64 + f64::from(draw.partial);
        let (shown, seen) = belt_view(pos, &entity.nbt, now);
        let elapsed = (now - seen).clamp(0.0, 30.0);
        for b in shown {
            let mut pf = f64::from(b.p);
            if b.moving {
                pf += speed / 480.0 * f64::from(belt_flip(front)) * elapsed;
            }
            let p = pf.clamp(0.0, 1.0) as f32;
            let line = axis_of(front);
            let mut at = [0.5, BELT_TOP, 0.5];
            at[line] = if front[line] > 0 { p } else { 1.0 - p };
            let stack = ItemStack { item: Arc::from(b.item.as_str()), count: b.count, damage: 0, extra: None };
            let is_block = game.pack.assets.registry.default_state(&b.item).is_some();
            for k in 0..stack_copies(b.count) {
                let o = copy_offset(k, is_block);
                draw.push();
                draw.translate([at[0] + o[0], at[1] + o[1], at[2] + o[2]]);
                if !is_block {
                    draw.rotate_about([1.0, 0.0, 0.0], 90.0, [0.0, 0.0, 0.0]);
                }
                game.native_draw_item(draw, &stack, DisplayContext::Ground, None, pos[0] ^ pos[2] ^ (b.p * 100.0) as i32 ^ (k as i32));
                draw.pop();
            }
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
        chain_shaft(game, axis, speed, draw);
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
        let facing_text = value.to_string();
        let pointing = game.native_block_state_property(state, "pointing").unwrap_or("up").to_string();
        game.native_draw_block(
            draw,
            "create:crafter_arrow_part",
            &[("facing", facing_text.as_str()), ("pointing", pointing.as_str())],
        );
        let shown = pair_list(&entity.nbt, "co").first().cloned().or_else(|| pair_list(&entity.nbt, "ci").first().cloned());
        if let Some((item, count)) = shown.as_ref() {
            let stack = ItemStack { item: Arc::from(item.as_str()), count: *count, damage: 0, extra: None };
            // sliding along the arrows to the last crafter, then shrinking away into it
            let now = game.ticks as f64 + f64::from(draw.partial);
            let stamp = f64::from(get_int(&entity.nbt, "cs", 0));
            let cp = get_int(&entity.nbt, "cp", 0);
            let mut slide = [0.0f32; 3];
            let mut grow = 1.15f32;
            if cp > 0 && now - stamp < 15.0 && pair_list(&entity.nbt, "co").is_empty() {
                let pct = ((f64::from(cp) + f64::from(get_int(&entity.nbt, "cr", 0)) / 5.0 * (now - stamp)).min(100.0)) / 100.0;
                let t = pct * f64::from(get_int(&entity.nbt, "ch", 1).max(1));
                let mut cur = pos;
                let mut done = 0.0f64;
                for _ in 0..9 {
                    let Some((f2, d)) = crafter_at(game, cur) else { break };
                    if f2 != front {
                        break;
                    }
                    let next = add3(cur, d);
                    match crafter_at(game, next) {
                        Some((f3, _)) if f3 == front => {}
                        _ => break,
                    }
                    let step = (t - done).clamp(0.0, 1.0) as f32;
                    for a in 0..3 {
                        slide[a] += d[a] as f32 * step;
                    }
                    done += 1.0;
                    if t <= done {
                        break;
                    }
                    cur = next;
                }
                grow = 1.15 * (1.0 - ((pct - 0.85) / 0.15).clamp(0.0, 1.0)).max(0.02) as f32;
            }
            draw.push();
            draw.translate([0.5 + front[0] as f32 * 0.6 + slide[0], 0.5 + slide[1], 0.5 + front[2] as f32 * 0.6 + slide[2]]);
            if front[0] != 0 {
                draw.rotate_about([0.0, 1.0, 0.0], 90.0, [0.0, 0.0, 0.0]);
            }
            draw.scale([grow, grow, grow]);
            game.native_draw_item(draw, &stack, DisplayContext::Ground, None, pos[0] ^ pos[2]);
            draw.pop();
        }
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
        if front[1] == 0 {
            half_shaft(game, pos, neg(front), speed, draw);
        } else {
            through_shaft(game, pos, node.saw_axis(), speed, draw);
        }
        if let Some((item, count)) = pair_list(&entity.nbt, "si").first() {
            let stack = ItemStack { item: Arc::from(item.as_str()), count: *count, damage: 0, extra: None };
            let is_block = game.pack.assets.registry.default_state(item).is_some();
            let along = (get_int(&entity.nbt, "sp", 0) as f32 / 100.0 - 0.5) * 0.9;
            let sign = if speed < 0.0 { -1.0f32 } else { 1.0f32 };
            let (dx, dz) = if first { (0.0f32, -sign) } else { (sign, 0.0f32) };
            for k in 0..stack_copies(*count) {
                let o = copy_offset(k, is_block);
                draw.push();
                draw.translate([0.5 + dx * along + o[0], BELT_TOP + o[1], 0.5 + dz * along + o[2]]);
                if !is_block {
                    draw.rotate_about([1.0, 0.0, 0.0], 90.0, [0.0, 0.0, 0.0]);
                }
                game.native_draw_item(draw, &stack, DisplayContext::Ground, None, pos[0] ^ pos[2] ^ (k as i32));
                draw.pop();
            }
        }
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
        let dy = -0.6 * (1.0 - phase.to_radians().cos());
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

// The shaft of a chain drive: it turns without the half step the cogs need, so the shafts of linked
// drives stay lined up with each other as in Create.
fn chain_shaft(game: &Game<'_>, axis: usize, speed: f64, draw: &mut BlockEntityDraw) {
    let mut about = [0.0f32; 3];
    about[axis] = 1.0;
    let t = draw.game_time as f64 + f64::from(draw.partial);
    draw.push();
    draw.rotate_about(about, ((speed * 0.3 * t) % 360.0) as f32, [0.5, 0.5, 0.5]);
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
    if kind == Kind::Mixer {
        // the mixer has no facing: its cog turns about the vertical axis
        return Some(Node::new(kind, 1, [0; 3]));
    }
    let props = &level.registry.state(state)?.properties;
    if kind == Kind::Millstone {
        return Some(Node::new(kind, 1, [0; 3]));
    }
    if kind == Kind::Hose {
        let front = parse_dir(props.get("facing")?)?;
        return Some(Node::new(kind, axis_of(cw(front)), front));
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
// Which way the chain line of a new chain drive runs: along a neighbouring chain drive that already runs
// that way, else along one that can be turned (its shaft not along the line), else the way the player
// looks if that is across the shaft. The neighbour may face any way.
fn chain_first_for(level: &GameLevel<'_>, at: BlockPos, axis: usize, look_axis: usize) -> bool {
    let mut lines: Vec<usize> = (0..3).filter(|a| *a != axis).collect();
    if lines.len() == 2 && lines[1] == look_axis {
        lines.reverse();
    }
    for pass in 0..2 {
        for a in lines.iter() {
            for sgn in [-1, 1] {
                let mut d = [0; 3];
                d[*a] = sgn;
                let nb = BlockPos::new(at.x + d[0], at.y + d[1], at.z + d[2]);
                if let Some(n) = node_from_state(level, level.block_state(nb)) {
                    let fits = if pass == 0 { n.is_chain() && n.chain_axis() == *a } else { n.is_chain() && n.axis != *a };
                    if fits {
                        return chain_first_towards(axis, *a);
                    }
                }
            }
        }
    }
    chain_first_towards(axis, lines.first().copied().unwrap_or(0))
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
    if name == PIPE || name == TANK || name == GLASS {
        fluid_placement(level, state, context, &name, answer);
        return;
    }
    if funnel_brass(&name).is_some() {
        // A funnel faces away from the block it is put on; on a basin, depot or belt it starts out
        // extracting (taking things out of it), anywhere else pulling in.
        let normal = context.face.normal();
        let facing = dir_name(normal);
        let back = BlockPos::new(context.pos.x - normal[0], context.pos.y - normal[1], context.pos.z - normal[2]);
        let back_name = level.facts.get(level.block_state(back)).name.to_string();
        let extracting = back_name == BASIN_BLOCK || back_name == DEPOT_BLOCK || back_name == "create:belt";
        if let Some(s1) = level.native_state_with_property(state, "facing", facing) {
            let s2 = if extracting { level.native_state_with_property(s1, "unloading", "true") } else { Some(s1) };
            if let Some(s) = s2 {
                *answer = Some(Some(s));
            }
        }
        return;
    }
    let Some(kind) = kind_of(&name) else { return };
    if kind == Kind::Mixer {
        return;
    }
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
        if feeding.is_empty() && !context.sneaking {
            for a in 0..3 {
                for s in [-1, 1] {
                    let nb = BlockPos::new(at.x + if a == 0 { s } else { 0 }, at.y + if a == 1 { s } else { 0 }, at.z + if a == 2 { s } else { 0 });
                    let nn = level.facts.get(level.block_state(nb)).name.to_string();
                    if matches!(nn.as_str(), PIPE | TANK | GLASS | SPOUT | DRAIN | PUMP | VALVE) && a == axis_of(look) {
                        pipe = a;
                    }
                }
            }
        }
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
            if f != 1 && d[1] == 0 {
                facing = neg(*d);
            } else {
                first = axis_of(*d) == low;
            }
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
    } else if kind == Kind::Hose {
        // Horizontal; its shaft side (clockwise of the facing) turns towards a shaft end beside it.
        let mut f = [look[0], 0, look[2]];
        if f == [0, 0, 0] {
            f = [0, 0, 1];
        }
        if let Some(d) = feeding.iter().find(|d| d[1] == 0) {
            f = [d[2], 0, -d[0]];
        }
        let placed = level.native_state_with_property(state, "facing", dir_name(f));
        let in_water = level.facts.get(level.block_state(at)).name.to_string() == "minecraft:water";
        placed.map(|st| if in_water { level.native_state_with_property(st, "waterlogged", "true").unwrap_or(st) } else { st })
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
                let first = chain_first_for(level, at, axis, axis_of(look));
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
const SLIDER_Y: f32 = 24.0;
const SLIDER_W: f32 = 196.0;
const SLIDER_H: f32 = 48.0;

const BUTTONS: [Btn; 2] = [
    Btn { id: 20, label: "CW", x: 12.0, y: 38.0, w: 24.0, h: 14.0 },
    Btn { id: 21, label: "CCW", x: 12.0, y: 54.0, w: 24.0, h: 14.0 },
];

static DRAG: AtomicBool = AtomicBool::new(false);
static MOTOR_AT: Mutex<Option<[i32; 3]>> = Mutex::new(None);
static COMMIT: Mutex<Option<([i32; 3], i32)>> = Mutex::new(None);

// The speed under a pointer (GUI pixels from the screen top left): the further right, the faster;
// the upper half of the pad turns the other way (negative), the lower half turns forward.
fn rpm_at(x: f32, y: f32) -> i32 {
    let mag = ((x - SLIDER_X) / SLIDER_W * 256.0).round().clamp(0.0, 256.0) as i32;
    if y < SLIDER_Y + SLIDER_H / 2.0 {
        -mag
    } else {
        mag
    }
}

fn on_track(at: [f32; 2]) -> bool {
    at[0] >= SLIDER_X - 6.0 && at[0] <= SLIDER_X + SLIDER_W + 6.0 && at[1] >= SLIDER_Y - 3.0 && at[1] < SLIDER_Y + SLIDER_H + 3.0
}

// What a press at `at` (GUI pixels from the screen top left) does: a menu button id.
fn hit(at: [f32; 2]) -> Option<i32> {
    if on_track(at) {
        let v = rpm_at(at[0], at[1]);
        return Some(1000 + v.abs() * 2 + if v < 0 { 1 } else { 0 });
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
        let l = local(screen);
        let value = rpm_at(l[0], l[1]);
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
            height: 96.0,
            slots: vec![None],
            inventory_y: 4000.0,
            inventory_x: 0.0,
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
    let rpm = if dragging { rpm_at(cursor[0] - origin[0], cursor[1] - origin[1]) } else { setting };
    let (ox, oy) = (origin[0], origin[1]);
    // the dark board the controls sit on
    game.native_fill(batch, [ox + 2.0, oy + 2.0, 252.0, 92.0], 0xFF1B100D);
    centred(game, batch, "Generated Speed in RPM", ox + 128.0, oy + 10.0, 0xE9D3A0);
    // the direction buttons
    game.native_fill(batch, [ox + 10.0, oy + 36.0, 28.0, 34.0], GOLD);
    for b in BUTTONS.iter() {
        let on = (b.id == 20 && rpm >= 0) || (b.id == 21 && rpm < 0);
        let over = hit([cursor[0] - ox, cursor[1] - oy]) == Some(b.id);
        let face = if on { 0xFFF3E3B8 } else if over { 0xFFD8BC83 } else { BEIGE };
        game.native_fill(batch, [ox + b.x, oy + b.y, b.w, b.h], GOLD_DARK);
        game.native_fill(batch, [ox + b.x + 1.0, oy + b.y + 1.0, b.w - 2.0, b.h - 2.0], face);
        let w = game.native_text_width(b.label);
        game.native_text(batch, b.label, [ox + b.x + (b.w - w) / 2.0, oy + b.y + 3.0], 0x3A2412, false);
    }
    // the pad: two lanes, the upper one negative (counter clockwise), the lower one positive;
    // ticks every 32 rpm
    game.native_fill(batch, [ox + SLIDER_X - 3.0, oy + SLIDER_Y - 3.0, SLIDER_W + 6.0, SLIDER_H + 6.0], GOLD);
    game.native_fill(batch, [ox + SLIDER_X - 1.0, oy + SLIDER_Y - 1.0, SLIDER_W + 2.0, SLIDER_H + 2.0], MAROON);
    game.native_fill(batch, [ox + SLIDER_X, oy + SLIDER_Y + SLIDER_H / 2.0 - 0.5, SLIDER_W, 1.0], 0xFF4A2A22);
    for i in 0..=8 {
        let x = ox + SLIDER_X + i as f32 * SLIDER_W / 8.0;
        let tick = if i == 0 { 0xFF8A5A3A } else { 0xFF4A2A22 };
        game.native_fill(batch, [x - 0.5, oy + SLIDER_Y + 1.0, 1.0, 4.0], tick);
        game.native_fill(batch, [x - 0.5, oy + SLIDER_Y + SLIDER_H - 5.0, 1.0, 4.0], tick);
    }
    game.native_text(batch, "- (up)", [ox + SLIDER_X + 3.0, oy + SLIDER_Y + 6.0], 0x8A6A52, false);
    game.native_text(batch, "+ (down)", [ox + SLIDER_X + 3.0, oy + SLIDER_Y + SLIDER_H - 14.0], 0x8A6A52, false);
    // the handle with the speed on it: right = faster, up = negative
    let value = rpm.clamp(-MOTOR_MAX_RPM, MOTOR_MAX_RPM);
    let thumb = ox + SLIDER_X + value.abs() as f32 / 256.0 * SLIDER_W;
    let lane = if value < 0 { oy + SLIDER_Y + 3.0 } else { oy + SLIDER_Y + SLIDER_H / 2.0 + 3.0 };
    game.native_fill(batch, [thumb - 19.0, lane, 38.0, 18.0], GOLD_DARK);
    game.native_fill(batch, [thumb - 18.0, lane + 1.0, 36.0, 16.0], BEIGE);
    let label = value.to_string();
    let w = game.native_text_width(&label);
    game.native_text(batch, &label, [thumb - w / 2.0, lane + 5.0], 0x3A2412, false);
    centred(game, batch, "Slide right = faster, up = reverse. Let go to confirm", ox + 128.0, oy + 76.0, 0xCDBA8C);
    let stress = "Stress: ".to_string() + &load.to_string() + " / " + &cap.to_string() + " SU";
    let color = if load > cap { 0xE05050 } else { 0xA89C78 };
    centred(game, batch, &stress, ox + 128.0, oy + 85.0, color);
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
        n if n >= 1000 => {
            let v = n - 1000;
            if v & 1 == 1 {
                -(v >> 1)
            } else {
                v >> 1
            }
        }
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
        // facing each other along the shaft: they link through the shaft ends
        b.first = true;
        assert!(link(&a, [0, 1, 0], &b).is_some());
        // a drive facing another one with its shaft end against a chain side does not link
        let mut c = node(Kind::Chain, 0);
        c.first = true;
        assert_eq!(link(&a, [1, 0, 0], &c), None);
    }

    #[test]
    fn chain_drives_with_different_shafts_link_on_one_line() {
        // axis y with its chain along x, axis z with its chain along x as well: they link along x
        let mut a = node(Kind::Chain, 1);
        a.first = true;
        let mut b = node(Kind::Chain, 2);
        b.first = true;
        assert_eq!(a.chain_axis(), 0);
        assert_eq!(b.chain_axis(), 0);
        assert_eq!(link(&a, [1, 0, 0], &b), Some(1.0));
        assert_eq!(link(&b, [-1, 0, 0], &a), Some(1.0));
        // the line of b turned away: no link
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
    fn funnels_are_told_apart_and_face_the_right_way() {
        assert_eq!(funnel_brass("create:andesite_funnel"), Some(false));
        assert_eq!(funnel_brass("create:brass_funnel"), Some(true));
        assert_eq!(funnel_brass("create:basin"), None);
        assert_eq!(direction_of([0, 1, 0]), Direction::Up);
        assert_eq!(direction_of([0, 0, -1]), Direction::North);
        assert_eq!(direction_of(neg([1, 0, 0])), Direction::West);
    }

    #[test]
    fn a_basin_bag_fills_in_stacks_of_64() {
        let mut bag: Vec<(String, u32)> = Vec::new();
        assert_eq!(bag_add(&mut bag, "minecraft:copper_ingot", 40), 40);
        assert_eq!(bag_add(&mut bag, "minecraft:copper_ingot", 40), 40);
        assert_eq!(bag.len(), 2);
        assert_eq!(bag_total(&bag, "minecraft:copper_ingot"), 80);
        bag_take(&mut bag, "minecraft:copper_ingot", 70);
        assert_eq!(bag_total(&bag, "minecraft:copper_ingot"), 10);
        assert_eq!(bag.len(), 1);
        // nine stacks at most
        let mut full: Vec<(String, u32)> = Vec::new();
        for i in 0..BASIN_SLOTS {
            full.push((i.to_string(), 64));
        }
        assert_eq!(bag_add(&mut full, "minecraft:stone", 5), 0);
    }

    #[test]
    fn brass_needs_heat_and_room() {
        let inb = vec![("minecraft:copper_ingot".to_string(), 3), ("create:zinc_ingot".to_string(), 1)];
        let empty: Vec<(String, u32)> = Vec::new();
        assert!(find_mixing(&inb, &empty, false).is_none());
        let r = find_mixing(&inb, &empty, true).expect("brass");
        assert_eq!(r.outputs, vec![("create:brass_ingot", 2)]);
        // nothing to mix without the zinc
        let only = vec![("minecraft:copper_ingot".to_string(), 3)];
        assert!(find_mixing(&only, &empty, true).is_none());
        // no room for the result
        let mut full: Vec<(String, u32)> = Vec::new();
        for i in 0..BASIN_SLOTS {
            full.push((i.to_string(), 64));
        }
        assert!(find_mixing(&inb, &full, true).is_none());
    }

    #[test]
    fn a_chain_turns_towards_a_neighbour() {
        // axis y: across it x is first, z is second
        assert!(chain_first_towards(1, 0));
        assert!(!chain_first_towards(1, 2));
        // axis x: y is first
        assert!(chain_first_towards(0, 1));
        assert!(!chain_first_towards(0, 2));
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
            BeltItem { item: "minecraft:iron_ingot".to_string(), count: 3, p: 0.25, moving: false },
            BeltItem { item: "create:pulp".to_string(), count: 1, p: 0.75, moving: false },
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
        let one = vec![BeltItem { item: "a".to_string(), count: 1, p: 0.5, moving: false }];
        assert_eq!(belt_free_spot(&one), Some(0.25));
        let full: Vec<BeltItem> = (0..BELT_MAX).map(|i| BeltItem { item: "a".to_string(), count: 1, p: i as f32, moving: false }).collect();
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
    fn hose_pulley_ports() {
        let hose = Node::new(Kind::Hose, axis_of(cw([0, 0, -1])), [0, 0, -1]);
        // facing north: shaft on the east side, rotation axis x
        assert_eq!(cw([0, 0, -1]), [1, 0, 0]);
        assert!(hose.has_end([1, 0, 0]));
        assert!(!hose.has_end([-1, 0, 0]));
        assert!(!hose.has_end([0, 0, -1]));
        assert_eq!(cw([1, 0, 0]), [0, 0, 1]);
        assert_eq!(cw([0, 0, 1]), [-1, 0, 0]);
        assert_eq!(cw([-1, 0, 0]), [0, 0, -1]);
        assert_eq!(link(&shaft(0), [-1, 0, 0], &hose), Some(1.0));
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
        assert_eq!(hit([SLIDER_X, 60.0]), Some(1000));
        assert_eq!(hit([SLIDER_X + SLIDER_W / 2.0, 60.0]), Some(1256));
        assert_eq!(hit([SLIDER_X + SLIDER_W, 60.0]), Some(1512));
        assert_eq!(hit([SLIDER_X + SLIDER_W, 30.0]), Some(1513));
        assert_eq!(hit([20.0, 45.0]), Some(20));
        assert_eq!(hit([20.0, 60.0]), Some(21));
        assert_eq!(hit([0.0, 0.0]), None);
        assert_eq!(rpm_at(SLIDER_X + SLIDER_W / 4.0, 30.0), -64);
        assert_eq!(rpm_at(SLIDER_X + SLIDER_W / 4.0, 60.0), 64);
    }

    #[test]
    fn looking_direction_follows_yaw_and_pitch() {
        assert_eq!(nearest_looking(0.0, 0.0), [0, 0, 1]);
        assert_eq!(nearest_looking(-90.0, 0.0), [1, 0, 0]);
        assert_eq!(nearest_looking(0.0, 90.0), [0, -1, 0]);
    }
}
