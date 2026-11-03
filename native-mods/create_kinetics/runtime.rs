// Create kinetics, stage 1: shafts, cogwheels, large cogwheels and a creative motor.
// Injected as `crate::game::create_kinetics`. Each kinetic block has a block entity
// holding its speed (rpm, signed). Speed spreads one block per tick from sources along
// a chain of "parents"; a block that loses its parent clears itself, so removing a
// source winds the network down without stale speeds circulating in loops.
use super::native_block_entity_draw::BlockEntityDraw;
use super::{Game, GameLevel};
use horizon_client::containers::ModBlockEntity;
use horizon_core::position::BlockPos;
use horizon_world::level::{LevelAccess, PlacementContext};
use horizon_world::nbt::{Compound, JavaString, Tag};
use horizon_world::registry::BlockStateId;

pub const BE_ID: &str = "create:kinetic";
const NONE_HOPS: i32 = 255;
const MAX_HOPS: i32 = 128;
const MOTOR_RPM: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Shaft,
    Cogwheel,
    LargeCogwheel,
    Motor,
}

pub fn kind_of(block: &str) -> Option<Kind> {
    match block {
        "create:shaft" => Some(Kind::Shaft),
        "create:cogwheel" => Some(Kind::Cogwheel),
        "create:large_cogwheel" => Some(Kind::LargeCogwheel),
        "create:creative_motor" => Some(Kind::Motor),
        _ => None,
    }
}

// A kinetic block as the network sees it. `axis` is 0/1/2 for x/y/z; a motor's `front`
// is the direction its shaft points (its `axis` is that direction's axis).
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
    // Whether a shaft end of this block faces the neighbour in direction `d`.
    fn has_end(&self, d: [i32; 3]) -> bool {
        match self.kind {
            Kind::Motor => d == self.front,
            _ => d[self.axis] != 0 && d.iter().filter(|c| **c != 0).count() == 1,
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

fn opposite(face: &str) -> &'static str {
    match face {
        "down" => "up",
        "up" => "down",
        "north" => "south",
        "south" => "north",
        "west" => "east",
        _ => "west",
    }
}

fn axis_name(face: &str) -> &'static str {
    match face {
        "west" | "east" => "x",
        "north" | "south" => "z",
        _ => "y",
    }
}

// The factor `f` with `my_speed = neighbour_speed * f` when `me` and the neighbour at
// offset `d` (neighbour minus me) are connected, else None.
pub fn link(me: &Node, d: [i32; 3], nb: &Node) -> Option<f32> {
    if me.has_end(d) && nb.has_end(neg(d)) {
        return Some(1.0);
    }
    if me.is_cog() && nb.is_cog() && me.axis == nb.axis && d[me.axis] == 0 {
        let nonzero = d.iter().filter(|c| **c != 0).count();
        let adjacent = nonzero == 1;
        let diagonal = nonzero == 2 && d.iter().all(|c| c.abs() <= 1);
        return match (me.kind, nb.kind) {
            (Kind::Cogwheel, Kind::Cogwheel) if adjacent => Some(-1.0),
            (Kind::Cogwheel, Kind::LargeCogwheel) if adjacent => Some(-2.0),
            (Kind::LargeCogwheel, Kind::Cogwheel) if adjacent => Some(-0.5),
            (Kind::LargeCogwheel, Kind::LargeCogwheel) if diagonal => Some(-1.0),
            _ => None,
        };
    }
    None
}

// Offsets worth looking at for `me`.
pub fn offsets(me: &Node) -> Vec<[i32; 3]> {
    let mut out = Vec::new();
    if me.kind == Kind::Motor {
        out.push(me.front);
        return out;
    }
    for a in 0..3 {
        for s in [-1, 1] {
            let mut d = [0; 3];
            d[a] = s;
            if a == me.axis || me.is_cog() {
                out.push(d);
            }
        }
    }
    if me.kind == Kind::LargeCogwheel {
        let (p, q) = ((me.axis + 1) % 3, (me.axis + 2) % 3);
        for sp in [-1, 1] {
            for sq in [-1, 1] {
                let mut d = [0; 3];
                d[p] = sp;
                d[q] = sq;
                out.push(d);
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

fn get_hops(n: &Compound) -> i32 {
    match n.get(&JavaString::from("hops")) {
        Some(Tag::Int(v)) => *v,
        _ => NONE_HOPS,
    }
}

fn get_parent(n: &Compound) -> Option<[i32; 3]> {
    match n.get(&JavaString::from("parent")) {
        Some(Tag::IntArray(v)) if v.len() == 3 => Some([v[0], v[1], v[2]]),
        _ => None,
    }
}

fn set_state(n: &mut Compound, speed: f32, hops: i32, parent: Option<[i32; 3]>) {
    n.insert(JavaString::from("speed"), Tag::Float(speed.to_bits()));
    n.insert(JavaString::from("hops"), Tag::Int(hops));
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
    if kind == Kind::Motor {
        let front = parse_dir(&game.native_block_property(p, "facing")?)?;
        Some(Node { kind, axis: axis_of(front), front })
    } else {
        let axis = parse_axis(&game.native_block_property(p, "axis")?)?;
        Some(Node { kind, axis, front: [0; 3] })
    }
}

// (speed, hops) of the kinetic block entity at `p`.
fn read_neighbour(game: &mut Game<'_>, p: [i32; 3]) -> Option<(f32, i32)> {
    game.native_with_mod_block_entity(p, |e| {
        if e.id == BE_ID {
            Some((get_f32(&e.nbt, "speed"), get_hops(&e.nbt)))
        } else {
            None
        }
    })
    .flatten()
}

// One tick of a non-source block: (speed, hops, parent offset).
fn step(game: &mut Game<'_>, pos: [i32; 3], me: &Node, nbt: &Compound) -> (f32, i32, Option<[i32; 3]>) {
    let speed = get_f32(nbt, "speed");
    let hops = get_hops(nbt);
    let at = |d: [i32; 3]| [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
    if let Some(d) = get_parent(nbt) {
        // Keep a parent that still feeds us from closer to a source; else wind down.
        let p = at(d);
        if let (Some(nb), Some((ns, nh))) = (node_at(game, p), read_neighbour(game, p)) {
            if let Some(f) = link(me, d, &nb) {
                if ns != 0.0 && nh < hops && nh + 1 <= MAX_HOPS {
                    return (ns * f, nh + 1, Some(d));
                }
            }
        }
        return (0.0, NONE_HOPS, None);
    }
    if speed != 0.0 {
        return (0.0, NONE_HOPS, None);
    }
    let mut best: Option<(i32, f32, [i32; 3])> = None;
    for d in offsets(me) {
        let p = at(d);
        let Some(nb) = node_at(game, p) else { continue };
        let Some(f) = link(me, d, &nb) else { continue };
        let Some((ns, nh)) = read_neighbour(game, p) else { continue };
        if ns == 0.0 || nh >= MAX_HOPS {
            continue;
        }
        let cand = (nh + 1, ns * f, d);
        let better = match best {
            None => true,
            Some(b) => cand.0 < b.0 || (cand.0 == b.0 && cand.1.abs() < b.1.abs()),
        };
        if better {
            best = Some(cand);
        }
    }
    match best {
        Some((h, s, d)) => (s, h, Some(d)),
        None => (0.0, NONE_HOPS, None),
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
    let (speed, hops, parent) = if me.kind == Kind::Motor {
        (MOTOR_RPM, 0, None)
    } else {
        step(game, pos, &me, &entity.nbt)
    };
    set_state(&mut entity.nbt, speed, hops, parent);
    if speed.to_bits() != old.to_bits() {
        game.native_block_entity_sync(pos);
    }
}

pub fn update_tag(entity: &ModBlockEntity, tag: &mut Option<Compound>) {
    if entity.id == BE_ID {
        let mut c = Compound::new();
        c.insert(JavaString::from("speed"), Tag::Float(get_f32(&entity.nbt, "speed").to_bits()));
        *tag = Some(c);
    }
}

// The block entity draws the moving part (`<block>_part`, never placed); the placed
// block's own model is empty (the motor's casing stays, only its shaft stub is drawn).
// A turning part is drawn about the block's centre, with Create's own phase offset:
// it depends only on the coordinates across the axis, so blocks along one shaft line
// stay aligned and meshing neighbours sit half a tooth apart.
pub fn submit(
    game: &Game<'_>,
    pos: [i32; 3],
    block: &str,
    state: BlockStateId,
    entity: &ModBlockEntity,
    draw: &mut BlockEntityDraw,
) {
    let (part, prop) = match block {
        "create:shaft" => ("create:shaft_part", "axis"),
        "create:cogwheel" => ("create:cogwheel_part", "axis"),
        "create:large_cogwheel" => ("create:large_cogwheel_part", "axis"),
        "create:creative_motor" => ("create:motor_shaft_part", "facing"),
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
    let speed = f64::from(get_f32(&entity.nbt, "speed"));
    let mut about = [0.0f32; 3];
    about[axis] = 1.0;
    let across: i32 = (0..3).filter(|i| *i != axis).map(|i| pos[i]).sum();
    let mut offset = if block == "create:large_cogwheel" { 11.25 } else { 0.0 };
    if across % 2 == 0 {
        offset = 22.5;
    }
    let t = draw.game_time as f64 + f64::from(draw.partial);
    let degrees = ((speed * 0.3 * t) % 360.0) as f32 + offset;
    draw.push();
    draw.rotate_about(about, degrees, [0.5, 0.5, 0.5]);
    game.native_draw_block(draw, part, &[(prop, value.as_str())]);
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

fn axis_char(axis: usize) -> &'static str {
    ["x", "y", "z"][axis.min(2)]
}

fn node_from_state(level: &GameLevel<'_>, state: BlockStateId) -> Option<Node> {
    let name = level.facts.get(state).name.to_string();
    let kind = kind_of(&name)?;
    let props = &level.registry.state(state)?.properties;
    if kind == Kind::Motor {
        let front = parse_dir(props.get("facing")?)?;
        Some(Node { kind, axis: axis_of(front), front })
    } else {
        let axis = parse_axis(props.get("axis")?)?;
        Some(Node { kind, axis, front: [0; 3] })
    }
}

// As Create does: a block lines up with neighbours whose shaft end faces it (the motor's
// shaft points at one); with none, shafts and cogs take the axis you are looking along
// (a cog placed on a cog copies its axis) and the motor's shaft faces you. Sneaking skips
// the neighbour rule.
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
    let new = if kind == Kind::Motor {
        let facing = feeding.first().copied().unwrap_or_else(|| neg(look));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cog(axis: usize) -> Node {
        Node { kind: Kind::Cogwheel, axis, front: [0; 3] }
    }
    fn shaft(axis: usize) -> Node {
        Node { kind: Kind::Shaft, axis, front: [0; 3] }
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
    }

    #[test]
    fn small_and_large_ratio() {
        let large = Node { kind: Kind::LargeCogwheel, axis: 1, front: [0; 3] };
        assert_eq!(link(&cog(1), [1, 0, 0], &large), Some(-2.0));
        assert_eq!(link(&large, [-1, 0, 0], &cog(1)), Some(-0.5));
        assert_eq!(link(&large, [1, 0, 1], &large), Some(-1.0));
    }

    #[test]
    fn looking_direction_follows_yaw_and_pitch() {
        assert_eq!(nearest_looking(0.0, 0.0), [0, 0, 1]);
        assert_eq!(nearest_looking(-90.0, 0.0), [1, 0, 0]);
        assert_eq!(nearest_looking(0.0, 90.0), [0, -1, 0]);
    }

    #[test]
    fn motor_feeds_the_shaft_in_front_only() {
        let motor = Node { kind: Kind::Motor, axis: 0, front: [1, 0, 0] };
        assert_eq!(link(&shaft(0), [-1, 0, 0], &motor), Some(1.0));
        assert_eq!(link(&shaft(0), [1, 0, 0], &motor), None);
    }
        }
        
