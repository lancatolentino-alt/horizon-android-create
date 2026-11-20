//! TerraFirmaCraft's river geometry: `world.river.River`, `MidpointFractal`,
//! `RiverHelpers` and `world.region.RiverEdge`, ported with Java's evaluation
//! order, `PriorityQueue` tie-breaking and float/double mixing preserved.

use super::tfc_region::units;
use horizon_core::geometry::trig;
use horizon_core::random::{RandomSource, XoroshiroRandom};
use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};

const MIN_BRANCH_ANGLE: f64 = 0.4f32 as f64;
const MIN_BRANCH_DISTANCE: i32 = 2;
const MIN_RIVER_EDGE_COUNT: usize = 6;

/// `RiverHelpers.norm2`.
pub fn norm2(x: f64, y: f64) -> f64 {
    x * x + y * y
}

/// `RiverHelpers.normInf`.
pub fn norm_inf(x: f64, y: f64) -> f64 {
    x.abs().max(y.abs())
}

/// `RiverHelpers.projectAlongLine`.
pub fn project_along_line(vx: f64, vy: f64, wx: f64, wy: f64, px: f64, py: f64) -> f64 {
    let l2 = norm2(vx - wx, vy - wy);
    if l2 == 0.0 {
        return l2;
    }
    let t = (((px - vx) * (wx - vx)) + ((py - vy) * (wy - vy))) / l2;
    // Mth.clamp(double, double, double)
    t.clamp(0.0, 1.0)
}

/// `RiverHelpers.distancePointToLineSq`.
pub fn distance_point_to_line_sq(vx: f64, vy: f64, wx: f64, wy: f64, px: f64, py: f64) -> f64 {
    let t = project_along_line(vx, vy, wx, wy, px, py);
    let x0 = vx + t * (wx - vx);
    let y0 = vy + t * (wy - vy);
    norm2(x0 - px, y0 - py)
}

/// `Math.round(double)` narrowed to `int`.
pub fn java_round(x: f64) -> i32 {
    let f = x.floor();
    (if x - f >= 0.5 { f + 1.0 } else { f }) as i64 as i32
}

/// `River.Vertex`. Edges share vertices by `Arc`, as the Java code shares references.
#[derive(Debug, PartialEq)]
pub struct Vertex {
    pub x: f64,
    pub y: f64,
    pub angle: f64,
    pub length: f64,
    pub distance: i32,
}

impl Vertex {
    /// Record equality (`Double.compare`), which is what Java's `HashMap` uses.
    pub fn key(&self) -> (u64, u64, u64, u64, i32) {
        (
            self.x.to_bits(),
            self.y.to_bits(),
            self.angle.to_bits(),
            self.length.to_bits(),
            self.distance,
        )
    }
}

/// `River.Edge`.
#[derive(Clone, Debug)]
pub struct Edge {
    pub source: Arc<Vertex>,
    pub drain: Arc<Vertex>,
}

fn distance(edge: &Edge, vertex: &Vertex) -> f64 {
    distance_point_to_line_sq(
        edge.source.x,
        edge.source.y,
        edge.drain.x,
        edge.drain.y,
        vertex.x,
        vertex.y,
    )
}

fn orientation(p: &Vertex, q: &Vertex, r: &Vertex) -> i32 {
    let value = (q.y - p.y) * (r.x - q.x) - (q.x - p.x) * (r.y - q.y);
    if value == 0.0 {
        0
    } else if value > 0.0 {
        1
    } else {
        2
    }
}

fn intersect_collinear(p: &Vertex, q: &Vertex, r: &Vertex) -> bool {
    q.x <= p.x.max(r.x) && q.x >= p.x.min(r.x) && q.y <= p.y.max(r.y) && q.y >= p.y.min(r.y)
}

fn intersect(p1: &Vertex, q1: &Vertex, p2: &Vertex, q2: &Vertex) -> bool {
    let o1 = orientation(p1, q1, p2);
    let o2 = orientation(p1, q1, q2);
    let o3 = orientation(p2, q2, p1);
    let o4 = orientation(p2, q2, q1);
    (o1 != o2 && o3 != o4)
        || (o1 == 0 && intersect_collinear(p1, p2, q1))
        || (o2 == 0 && intersect_collinear(p1, q2, q1))
        || (o3 == 0 && intersect_collinear(p2, p1, q2))
        || (o4 == 0 && intersect_collinear(p2, q1, q2))
}

/// `River.Builder`.
pub struct Builder {
    queue: VecDeque<Edge>,
    random: XoroshiroRandom,
    edges: Vec<Edge>,
    root: Arc<Vertex>,
    depth: i32,
    feather_sq: f64,
}

impl Builder {
    pub fn new(
        random: XoroshiroRandom,
        drain_x: f64,
        drain_y: f64,
        angle: f64,
        length: f64,
        depth: i32,
        feather: f64,
    ) -> Self {
        Self {
            queue: VecDeque::new(),
            random,
            edges: Vec::new(),
            root: Arc::new(Vertex {
                x: drain_x,
                y: drain_y,
                angle,
                length,
                distance: 0,
            }),
            depth,
            feather_sq: feather * feather,
        }
    }

    fn branch_extra(&mut self) -> i32 {
        self.random
            .next_int_bound(1 + (self.depth as f32 * 0.3f32) as i32)
            .unwrap()
    }

    fn prune_if_too_short(&mut self) {
        if self.edges.len() < MIN_RIVER_EDGE_COUNT {
            self.edges.clear();
            self.queue.clear();
        }
    }

    fn intersect_any(&self, edge: &Edge) -> bool {
        for e in &self.edges {
            if !Arc::ptr_eq(&e.source, &edge.drain)
                && !Arc::ptr_eq(&e.drain, &edge.drain)
                && (distance(e, &edge.source) < self.feather_sq
                    || intersect(&e.source, &e.drain, &edge.source, &edge.drain))
            {
                return true;
            }
        }
        false
    }

    fn compute_next(&mut self, prev: &Vertex, length: f64, distance: i32) -> Arc<Vertex> {
        let next_angle = if distance == 0 {
            prev.angle
        } else {
            let a = self.random.next_double() * f64::from(0.5f32) + f64::from(0.2f32);
            let sign = if self.random.next_boolean() {
                1.0
            } else {
                -1.0
            };
            prev.angle + a * sign
        };
        let next_length =
            length * (self.random.next_double() * f64::from(0.08f32) + f64::from(0.92f32));
        let dx = f64::from(trig::cos(f64::from(next_angle as f32))) * next_length;
        let dy = f64::from(trig::sin(f64::from(next_angle as f32))) * next_length;
        Arc::new(Vertex {
            x: prev.x + dx,
            y: prev.y + dy,
            angle: next_angle,
            length: next_length,
            distance: distance + 1,
        })
    }
}

type Legal<'a> = &'a dyn Fn(&Vertex, &Vertex) -> bool;

/// `MultiParallelBuilder.intersectAny`.
fn intersect_any_all(builders: &[Builder], legal: Legal<'_>, edge: &Edge) -> bool {
    if !legal(&edge.drain, &edge.source) {
        return true;
    }
    builders.iter().any(|b| b.intersect_any(edge))
}

/// `Builder.buildInitialBranch`; `true` means the branch was cut short by an intersection.
fn build_initial_branch(builders: &mut [Builder], i: usize, legal: Legal<'_>) -> bool {
    let mut prev = builders[i].root.clone();
    let depth = builders[i].depth;
    let length = depth + builders[i].branch_extra();
    for _ in 0..length {
        let next = builders[i].compute_next(&prev, prev.length, prev.distance);
        let edge = Edge {
            source: next.clone(),
            drain: prev.clone(),
        };
        if intersect_any_all(builders, legal, &edge) {
            builders[i].prune_if_too_short();
            return true;
        }
        builders[i].edges.push(edge.clone());
        prev = next;
        if prev.distance < depth && prev.distance >= MIN_BRANCH_DISTANCE {
            builders[i].queue.push_back(edge);
        }
    }
    false
}

/// `Builder.buildBranch`; `true` means the builder is finished.
fn build_branch(builders: &mut [Builder], i: usize, legal: Legal<'_>) -> bool {
    let Some(prev_edge) = builders[i].queue.pop_front() else {
        builders[i].prune_if_too_short();
        return true;
    };
    let depth = builders[i].depth;
    let mut prev = prev_edge.drain.clone();
    let mut prev_dist = prev.distance + builders[i].random.next_int_bound(3).unwrap();
    let branch_next = builders[i].compute_next(&prev, prev.length, prev_dist);
    let delta_angle = (prev_edge.source.angle - branch_next.angle).abs();
    if delta_angle < MIN_BRANCH_ANGLE
        || (2.0 * std::f64::consts::PI - delta_angle < MIN_BRANCH_ANGLE)
    {
        return false;
    }
    let mut branch: Vec<Edge> = Vec::new();
    let first = Edge {
        source: branch_next.clone(),
        drain: prev.clone(),
    };
    if intersect_any_all(builders, legal, &first) {
        return false;
    }
    branch.push(first);
    prev = branch_next;
    let mut k = 0;
    // The loop bound draws a fresh random number on every check, as in Java.
    while k < depth - prev.distance + builders[i].branch_extra() {
        let next = builders[i].compute_next(&prev, prev.length, prev_dist);
        let edge = Edge {
            source: next.clone(),
            drain: prev.clone(),
        };
        if intersect_any_all(builders, legal, &edge) {
            break;
        }
        branch.push(edge.clone());
        prev = next;
        prev_dist = prev.distance;
        if prev_dist < depth && prev_dist >= MIN_BRANCH_DISTANCE {
            builders[i].queue.push_back(edge);
        }
        k += 1;
    }
    builders[i].edges.extend(branch);
    false
}

/// `java.util.PriorityQueue` over `(key, builder)`, to reproduce its tie order.
#[derive(Default)]
struct JavaHeap {
    items: Vec<(i32, usize)>,
}

impl JavaHeap {
    fn offer(&mut self, item: (i32, usize)) {
        let mut k = self.items.len();
        self.items.push(item);
        while k > 0 {
            let parent = (k - 1) >> 1;
            let e = self.items[parent];
            if item.0 >= e.0 {
                break;
            }
            self.items[k] = e;
            k = parent;
        }
        self.items[k] = item;
    }

    fn poll(&mut self) -> Option<(i32, usize)> {
        if self.items.is_empty() {
            return None;
        }
        let result = self.items[0];
        let x = self.items.pop().unwrap();
        let n = self.items.len();
        if n > 0 {
            let mut k = 0;
            let half = n >> 1;
            while k < half {
                let mut child = 2 * k + 1;
                let mut c = self.items[child];
                let right = child + 1;
                if right < n && c.0 > self.items[right].0 {
                    child = right;
                    c = self.items[child];
                }
                if x.0 <= c.0 {
                    break;
                }
                self.items[k] = c;
                k = child;
            }
            self.items[k] = x;
        }
        Some(result)
    }
}

/// `MultiParallelBuilder.build`: grows every builder in parallel and returns all edges.
pub fn build_rivers(mut builders: Vec<Builder>, legal: Legal<'_>) -> Vec<Edge> {
    let key = |b: &Builder| -(b.edges.len() as i32) - 10 * b.queue.len() as i32;
    let mut working = JavaHeap::default();
    for i in 0..builders.len() {
        if build_initial_branch(&mut builders, i, legal) {
            working.offer((key(&builders[i]), i));
        }
    }
    while let Some((_, i)) = working.poll() {
        if !build_branch(&mut builders, i, legal) {
            working.offer((key(&builders[i]), i));
        }
    }
    builders.into_iter().flat_map(|b| b.edges).collect()
}

const MAX_BISECTIONS: usize = 10;
const JITTER_MAX: f64 = 0.2f32 as f64;
const JITTER_MIN: f64 = 0.05f32 as f64;

fn encompassing_ranges() -> &'static [f64; MAX_BISECTIONS] {
    static RANGES: OnceLock<[f64; MAX_BISECTIONS]> = OnceLock::new();
    RANGES.get_or_init(|| {
        let mut out = [0.0; MAX_BISECTIONS];
        let sqrt2 = 2.0f64.sqrt();
        let (mut delta, mut midpoint_delta, mut norm) = (0.0f64, 0.0f64, 1.0f64);
        for slot in &mut out {
            *slot = delta.max(midpoint_delta);
            let prev_midpoint = midpoint_delta;
            midpoint_delta = 0.5 * (delta + midpoint_delta) + sqrt2 * JITTER_MAX * norm;
            delta = prev_midpoint.max(delta);
            norm *= 0.5 + JITTER_MAX;
        }
        out
    })
}

fn random_jitter(random: &mut impl RandomSource) -> f64 {
    let value = 2.0 * random.next_double() - 1.0;
    (JITTER_MAX - JITTER_MIN) * value + if value < 0.0 { -JITTER_MIN } else { JITTER_MIN }
}

/// `MidpointFractal`: a river segment bisected into a jittered polyline.
#[derive(Clone, Debug)]
pub struct MidpointFractal {
    pub segments: Vec<f64>,
    norm: f64,
}

impl MidpointFractal {
    pub fn new(
        random: &mut impl RandomSource,
        bisections: usize,
        sx: f64,
        sy: f64,
        dx: f64,
        dy: f64,
    ) -> Self {
        assert!(
            bisections < MAX_BISECTIONS,
            "Bisections must be within [0, MAX_BISECTIONS)"
        );
        let mut segments = vec![sx, sy, dx, dy];
        for _ in 0..bisections {
            let mut split = vec![0.0; (segments.len() << 1) - 2];
            split[0] = segments[0];
            split[1] = segments[1];
            let mut split_index = 2;
            let mut index = 0;
            while index < segments.len() - 2 {
                let (source_x, source_y, drain_x, drain_y) = (
                    segments[index],
                    segments[index + 1],
                    segments[index + 2],
                    segments[index + 3],
                );
                let norm = norm_inf(source_x - drain_x, source_y - drain_y);
                let bisect_x = random_jitter(random) * norm + (source_x + drain_x) * 0.5;
                let bisect_y = random_jitter(random) * norm + (source_y + drain_y) * 0.5;
                split[split_index] = bisect_x;
                split[split_index + 1] = bisect_y;
                split[split_index + 2] = drain_x;
                split[split_index + 3] = drain_y;
                split_index += 4;
                index += 2;
            }
            segments = split;
        }
        let norm = encompassing_ranges()[bisections] * norm_inf(sx - dx, sy - dy);
        Self { segments, norm }
    }

    pub fn maybe_intersect(&self, x: f64, y: f64, distance: f64) -> bool {
        let n = self.segments.len();
        let d = distance_point_to_line_sq(
            self.segments[0],
            self.segments[1],
            self.segments[n - 2],
            self.segments[n - 1],
            x,
            y,
        );
        let t = distance + self.norm;
        d <= t * t
    }

    /// `intersect(x, y, distance)`: whether the point comes within `distance` of the fractal.
    pub fn intersect(&self, x: f64, y: f64, distance: f64) -> bool {
        if !self.maybe_intersect(x, y, distance) {
            return false;
        }
        let dist_sq = distance * distance;
        self.segments
            .windows(4)
            .step_by(2)
            .any(|w| distance_point_to_line_sq(w[0], w[1], w[2], w[3], x, y) < dist_sq)
    }

    /// `calculateFlow`: the flow of the segment nearest a point.
    pub fn calculate_flow(&self, x: f64, y: f64) -> u8 {
        let s = &self.segments;
        let (mut min, mut min_index) = (f64::MAX, 0usize);
        let mut i = 0;
        while i < s.len() - 2 {
            let d = distance_point_to_line_sq(s[i], s[i + 1], s[i + 2], s[i + 3], x, y);
            if d < min {
                min = d;
                min_index = i;
            }
            i += 2;
        }
        let angle = trig::atan2(
            -(s[min_index + 3] - s[min_index + 1]),
            s[min_index + 2] - s[min_index],
        );
        super::tfc_flow::from_angle(angle)
    }

    pub fn intersect_distance(&self, x: f64, y: f64) -> f64 {
        let mut min = f64::MAX;
        let mut i = 0;
        while i < self.segments.len() - 2 {
            let s = &self.segments;
            let d = distance_point_to_line_sq(s[i], s[i + 1], s[i + 2], s[i + 3], x, y);
            if d < min {
                min = d;
            }
            i += 2;
        }
        min
    }
}

pub const MIN_WIDTH: i32 = 8;
pub const MAX_WIDTH: i32 = 24;
pub const MIN_VALLEY_WIDTH: i32 = 20;
/// `1 + Mth.ceil(1.5f * RIVER_LENGTH)`.
const MAX_AFFECTING_GRID_DISTANCE: i32 = 6;

/// `world.region.RiverEdge`. `drain_edge` indexes the owning region's river list.
#[derive(Clone, Debug)]
pub struct RiverEdge {
    pub width: i32,
    pub source: Arc<Vertex>,
    pub drain: Arc<Vertex>,
    pub fractal: MidpointFractal,
    pub min_part_x: i32,
    pub min_part_z: i32,
    pub max_part_x: i32,
    pub max_part_z: i32,
    pub source_edge: bool,
    pub drain_edge: Option<usize>,
}

impl RiverEdge {
    pub fn new(edge: &Edge, random: &mut impl RandomSource) -> Self {
        let fractal = MidpointFractal::new(
            random,
            4,
            edge.source.x,
            edge.source.y,
            edge.drain.x,
            edge.drain.y,
        );
        let center_x = java_round(f64::from(0.5f32) * (edge.source.x + edge.drain.x));
        let center_z = java_round(f64::from(0.5f32) * (edge.source.y + edge.drain.y));
        Self {
            width: 0,
            source: edge.source.clone(),
            drain: edge.drain.clone(),
            fractal,
            min_part_x: units::grid_to_part(center_x - MAX_AFFECTING_GRID_DISTANCE),
            min_part_z: units::grid_to_part(center_z - MAX_AFFECTING_GRID_DISTANCE),
            max_part_x: units::grid_to_part(center_x + MAX_AFFECTING_GRID_DISTANCE),
            max_part_z: units::grid_to_part(center_z + MAX_AFFECTING_GRID_DISTANCE),
            source_edge: false,
            drain_edge: None,
        }
    }

    /// `widthSq(exactGridX, exactGridZ)`, including the original's `drain().y()` used twice.
    pub fn width_sq_at(&self, rivers: &[RiverEdge], x: f64, z: f64) -> f64 {
        let t = project_along_line(
            self.source.x,
            self.source.y,
            self.drain.y,
            self.drain.y,
            x,
            z,
        );
        let drain_width = self.drain_edge.map_or(self.width, |i| rivers[i].width);
        let w = f64::from(self.width) + (f64::from(drain_width) - f64::from(self.width)) * t;
        w * w
    }
}
