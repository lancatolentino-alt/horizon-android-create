//! `minecraft:random_patch` (gone from 26.3, so this is the 1.21 feature as TFC's own type) and
//! TFC's `DynamicDensityRandomPatchFeature` (EUPL 1.2): `tries` placements of a nested placed
//! feature, each offset from the origin by a triangle-distributed amount. The dynamic version
//! scales `tries` by altitude and by how dense the chunk's forest is.

use super::{
    DecodeError, Object,
    feature::FeatureWorld,
    feature_graph::{FeatureExecutionError, PlacedFeatureDefinition},
    integer, optional,
    placement::PlacementContext,
    required, tfc_terrain,
    tree::kind,
    vegetation::{PlacedDecoder, PlacedPlacer},
};
use horizon_core::random::RandomSource;

const SEA_LEVEL: i32 = 63;

#[derive(Clone, Debug)]
pub struct TfcPatchFeature {
    dynamic: bool,
    tries: i32,
    xz_spread: i32,
    y_spread: i32,
    feature: Box<PlacedFeatureDefinition>,
}

/// `Mth.clampedMap(float...)`.
fn clamped_map(value: f32, in_min: f32, in_max: f32, out_min: f32, out_max: f32) -> f32 {
    let t = ((value - in_min) / (in_max - in_min)).clamp(0.0, 1.0);
    out_min + t * (out_max - out_min)
}

/// `DynamicDensityRandomPatchFeature.getTries`, with the default `grass_density` of 0.5.
fn dynamic_tries(tries: i32, y: i32, forest_density: i32) -> i32 {
    let mut tries = tries;
    if y > SEA_LEVEL + 25 {
        let scale = 1.0f32
            - clamped_map(
                y as f32,
                (SEA_LEVEL + 25) as f32,
                (SEA_LEVEL + 100) as f32,
                0.0,
                0.8,
            );
        tries = (tries as f32 * scale) as i32;
    }
    match forest_density {
        4 => tries.min(8),
        3 => tries.min(14),
        2 => tries.min(40),
        _ => (tries as f32 * (0.5f32 * 2.0f32)) as i32,
    }
}

impl TfcPatchFeature {
    pub(crate) fn decode(
        json: &str,
        depth: usize,
        remaining: &mut usize,
        nested: &PlacedDecoder<'_>,
    ) -> Result<Self, DecodeError> {
        let object: Object = serde_json::from_str(json)?;
        let dynamic = match kind(&object)?.as_str() {
            "tfc:random_patch" => false,
            "tfc:dynamic_random_patch" => true,
            _ => return Err(DecodeError::Field("patch feature type")),
        };
        let int = |name: &'static str, default: i32| -> Result<i32, DecodeError> {
            Ok(optional(&object, name)
                .map(integer)
                .transpose()?
                .unwrap_or(default))
        };
        let tries = int("tries", 128)?;
        let (xz_spread, y_spread) = (int("xz_spread", 7)?, int("y_spread", 3)?);
        if tries < 1 || xz_spread < 0 || y_spread < 0 {
            return Err(DecodeError::Field("patch size"));
        }
        Ok(Self {
            dynamic,
            tries,
            xz_spread,
            y_spread,
            feature: Box::new(nested(
                required(&object, "feature")?.get(),
                depth + 1,
                remaining,
            )?),
        })
    }

    pub(crate) fn place<W: FeatureWorld + PlacementContext, R: RandomSource>(
        &self,
        world: &mut W,
        random: &mut R,
        origin: [i32; 3],
        nested: &mut PlacedPlacer<'_, W, R>,
    ) -> Result<bool, FeatureExecutionError<W::Error>> {
        let tries = if self.dynamic {
            let density = match world.world_seed() {
                Ok(seed) => tfc_terrain::active_for(seed)
                    .map_or(0, |terrain| terrain.forest_density(origin[0], origin[2])),
                Err(_) => 0,
            };
            dynamic_tries(self.tries, origin[1], density)
        } else {
            self.tries
        };
        let (width, height) = (self.xz_spread + 1, self.y_spread + 1);
        let mut placed = 0;
        for _ in 0..tries {
            let dx = random.next_int_bound(width)? - random.next_int_bound(width)?;
            let dy = random.next_int_bound(height)? - random.next_int_bound(height)?;
            let dz = random.next_int_bound(width)? - random.next_int_bound(width)?;
            if nested(
                &self.feature,
                world,
                random,
                [origin[0] + dx, origin[1] + dy, origin[2] + dz],
            )? {
                placed += 1;
            }
        }
        Ok(placed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_tries_follow_altitude_and_forest_density() {
        // Open ground below the altitude falloff keeps its tries.
        assert_eq!(dynamic_tries(10, 70, 0), 10);
        assert_eq!(dynamic_tries(10, 70, 1), 10);
        // Forests cap them.
        assert_eq!(dynamic_tries(100, 70, 4), 8);
        assert_eq!(dynamic_tries(100, 70, 3), 14);
        assert_eq!(dynamic_tries(100, 70, 2), 40);
        assert_eq!(dynamic_tries(10, 70, 4), 8);
        // 80% fewer by sea level + 100 (f32: 1 - 0.8 is just under 0.2, so 19, as in Java).
        assert_eq!(dynamic_tries(100, SEA_LEVEL + 100, 0), 19);
        assert_eq!(dynamic_tries(100, SEA_LEVEL + 200, 0), 19);
        // About halfway up the falloff: scale ~0.605.
        assert_eq!(dynamic_tries(100, SEA_LEVEL + 62, 0), 60);
    }
}
