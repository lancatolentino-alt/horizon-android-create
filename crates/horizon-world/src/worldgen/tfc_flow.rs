//! TerraFirmaCraft's `river.Flow` (EUPL 1.2): the 16 compass directions a river can flow, plus
//! `NONE`, with the original's angle conversion and interpolation.

use std::f32::consts::PI;

/// `Flow.NONE`; the other values are `0..16` ordered by unit-circle angle (0 = east, 4 = north).
pub const NONE: u8 = 16;
const MODULUS: i32 = 16;

/// Serialised names, in ordinal order (`Flow.getSerializedName`).
pub const NAMES: [&str; 17] = [
    "e", "nee", "ne", "nne", "n", "nnw", "nw", "nww", "w", "sww", "sw", "ssw", "s", "sse", "se",
    "see", "none",
];

/// `Flow.fromAngle`: the closest flow to a polar angle in `[-pi, pi]`.
pub fn from_angle(angle: f64) -> u8 {
    let mut ordinal = (8.0 * (angle / f64::from(PI)) + 0.5).floor() as i32;
    if ordinal < 0 {
        ordinal += MODULUS;
    }
    ordinal as u8
}

/// `Flow.getVector`: the horizontal unit vector `(x, z)`, `NONE` being zero.
pub fn vector(flow: u8) -> (f64, f64) {
    if flow >= NONE {
        return (0.0, 0.0);
    }
    let angle = f32::from(flow) * (1.0 / 8.0) * PI;
    (f64::from(angle.cos()), -f64::from(angle.sin()))
}

fn mth_lerp(delta: f32, a: f32, b: f32) -> f32 {
    a + delta * (b - a)
}

fn round_lerp(left: i32, right: i32, delta: f32) -> i32 {
    (mth_lerp(delta, left as f32, right as f32) + 0.5).floor() as i32
}

fn lerp_weight(left: u8, right: u8, delta: f32) -> f32 {
    match (left == NONE, right == NONE) {
        (true, true) => 0.0,
        (true, false) => delta,
        (false, true) => 1.0 - delta,
        _ => 1.0,
    }
}

fn lerp_non_empty(left: u8, right: u8, delta: f32) -> u8 {
    let (l, r) = (i32::from(left), i32::from(right));
    let distance = (l - r).abs();
    if distance == 8 {
        if delta == 0.5 {
            NONE
        } else if delta < 0.5 {
            left
        } else {
            right
        }
    } else if distance < 8 {
        round_lerp(l, r, delta) as u8
    } else {
        let (mut l, mut r) = (l, r);
        if l < r {
            l += MODULUS;
        } else {
            r += MODULUS;
        }
        (round_lerp(l, r, delta) % MODULUS) as u8
    }
}

fn lerp_weighted(left: u8, right: u8, delta: f32, weight_left: f32, weight_right: f32) -> u8 {
    if left == NONE && right == NONE {
        NONE
    } else if left == NONE {
        if mth_lerp(delta, weight_left, weight_right) < 0.5 {
            left
        } else {
            right
        }
    } else if right == NONE {
        if mth_lerp(delta, weight_left, weight_right) >= 0.5 {
            left
        } else {
            right
        }
    } else {
        lerp_non_empty(left, right, delta)
    }
}

/// `Flow.lerp(left, right, delta)`.
pub fn lerp2(left: u8, right: u8, delta: f32) -> u8 {
    lerp_weighted(
        left,
        right,
        delta,
        if left == NONE { 0.0 } else { 1.0 },
        if right == NONE { 0.0 } else { 1.0 },
    )
}

/// `Flow.lerp(flow00, flow01, flow10, flow11, delta0, delta1)`.
pub fn lerp4(f00: u8, f01: u8, f10: u8, f11: u8, delta0: f32, delta1: f32) -> u8 {
    let flow0 = lerp2(f00, f01, delta0);
    let flow1 = lerp2(f10, f11, delta0);
    lerp_weighted(
        flow0,
        flow1,
        delta1,
        lerp_weight(f00, f01, delta0),
        lerp_weight(f10, f11, delta0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_map_to_compass_points() {
        assert_eq!(from_angle(0.0), 0);
        assert_eq!(from_angle(std::f64::consts::FRAC_PI_2), 4);
        assert_eq!(from_angle(-std::f64::consts::FRAC_PI_2), 12);
        assert_eq!(from_angle(std::f64::consts::PI), 8);
        assert_eq!(from_angle(-std::f64::consts::PI), 8);
    }

    #[test]
    fn interpolation_wraps_the_long_way_round() {
        // see (15) and nee (1) average to east, not west.
        assert_eq!(lerp2(15, 1, 0.5), 0);
        assert_eq!(lerp2(0, 8, 0.5), NONE);
        assert_eq!(lerp2(NONE, NONE, 0.3), NONE);
        assert_eq!(lerp2(NONE, 4, 0.9), 4);
        assert_eq!(lerp4(4, 4, 4, 4, 0.3, 0.7), 4);
    }
}
