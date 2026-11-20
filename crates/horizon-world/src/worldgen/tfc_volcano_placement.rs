//! TFC's centred-feature placement modifiers: `tfc:volcano` (cinder cone), `tfc:tuff_ring`,
//! `tfc:tuya`, `tfc:atoll`, `tfc:stratovolcano` and `tfc:ocean_ridge`.

use super::tfc_region::hash_double;
use super::tfc_terrain::TfcTerrain;
use super::tfc_volcano::{CenterKind, Volcanoes};
use super::tfc_volcano_strato::Kind;

#[derive(Clone, Debug)]
pub struct CenterPlacement {
    pub kind: CenterKind,
    pub center: bool,
    pub use_offset_center: bool,
    /// `None` is `"all"`.
    pub variant: Option<String>,
    pub min_easing: f32,
    pub max_easing: f32,
    pub hash_min: f32,
    pub hash_max: f32,
}

fn chunk(v: i32) -> i32 {
    v >> 4
}

fn clamped_map(v: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
    c + t * (d - c)
}

impl CenterPlacement {
    fn biome_at(terrain: &TfcTerrain, x: i32, z: i32) -> i32 {
        terrain.biome_at_quart(x >> 2, z >> 2)
    }

    /// The centre branch of `getPositions`, shared by every variant.
    fn centre_checked(
        &self,
        terrain: &TfcTerrain,
        pos: [i32; 3],
        centre: [i32; 3],
    ) -> Option<[i32; 3]> {
        (chunk(centre[0]) == chunk(pos[0])
            && chunk(centre[2]) == chunk(pos[2])
            && Volcanoes::center_valid_biome(
                self.kind,
                Self::biome_at(terrain, centre[0], centre[2]),
            ))
        .then_some(centre)
    }

    /// `getPositions`: at most one position.
    pub fn position(&self, terrain: &TfcTerrain, pos: [i32; 3]) -> Option<[i32; 3]> {
        let [x, y, z] = pos;
        let volcanoes = terrain.volcanoes();
        let biome = Self::biome_at(terrain, x, z);
        if !Volcanoes::center_valid_biome(self.kind, biome) {
            return None;
        }
        let frequency = Volcanoes::center_frequency(biome);
        if self.kind != CenterKind::Stratovolcano {
            if self.center {
                let c = volcanoes.center_of(self.kind, x, y, z, frequency)?;
                return self.centre_checked(terrain, pos, c);
            }
            let easing = volcanoes.center_easing(self.kind, x, z, frequency);
            return (easing > self.min_easing && easing < self.max_easing).then_some(pos);
        }

        let strato = volcanoes.stratovolcanoes();
        let cell = strato.cell(f64::from(x), f64::from(z));
        let kind = strato.variant_kind(&cell);
        let name = super::tfc_volcano_strato::Stratovolcanoes::variant_name(kind);
        if let Some(v) = &self.variant
            && v != name
        {
            return None;
        }
        let (hash_min, hash_max) = (f64::from(self.hash_min), f64::from(self.hash_max));
        let in_hash = |h: f64| h >= hash_min && h <= hash_max;
        if self.use_offset_center {
            match kind {
                Kind::CraterLake => {
                    return self.crater_lake(terrain, pos, cell.noise, cell.x, cell.y, in_hash);
                }
                Kind::Kelimutu => {
                    return self.kelimutu(terrain, pos, cell.noise, cell.x, cell.y, in_hash);
                }
                _ => {}
            }
        }
        if !in_hash(hash_double(cell.noise, 3199)) {
            return None;
        }
        if self.center {
            let c = volcanoes.center_of(self.kind, x, y, z, frequency)?;
            return self.centre_checked(terrain, pos, c);
        }
        let easing = volcanoes.center_easing(self.kind, x, z, frequency);
        (easing > self.min_easing && easing < self.max_easing).then_some(pos)
    }

    fn crater_lake(
        &self,
        terrain: &TfcTerrain,
        pos: [i32; 3],
        noise: f64,
        cx: f64,
        cz: f64,
        in_hash: impl Fn(f64) -> bool,
    ) -> Option<[i32; 3]> {
        let center_x = cx + (-45.0 + 90.0 * hash_double(noise, 6));
        let center_z = cz + (-45.0 + 90.0 * hash_double(noise, 7));
        if !in_hash(hash_double(noise, 3200)) {
            return None;
        }
        if self.center {
            return self.centre_checked(terrain, pos, [center_x as i32, 0, center_z as i32]);
        }
        let (dx, dz) = (f64::from(pos[0]) - center_x, f64::from(pos[2]) - center_z);
        let easing = clamped_map(dx * dx + dz * dz, 0.0, 900.0, 1.0, 0.0);
        (easing > f64::from(self.min_easing)).then_some(pos)
    }

    fn kelimutu(
        &self,
        terrain: &TfcTerrain,
        pos: [i32; 3],
        noise: f64,
        c0x: f64,
        c0z: f64,
        in_hash: impl Fn(f64) -> bool,
    ) -> Option<[i32; 3]> {
        let (px, pz) = (f64::from(pos[0]), f64::from(pos[2]));
        let dist = |x: f64, z: f64| (px - x) * (px - x) + (pz - z) * (pz - z);
        let offset = |r: f64, k: f64| {
            if r > 0.0 {
                20.0 + r * 70.0
            } else {
                -20.0 + r * k
            }
        };
        let mut distance = dist(c0x, c0z);
        let (mut center_x, mut center_z, mut hash) = (c0x, c0z, hash_double(noise, 3199));

        let r1x = 2.0 * (0.5 - hash_double(noise, 68));
        let r1z = 2.0 * (0.5 - hash_double(noise, 69));
        let (c1x, c1z) = (c0x + offset(r1x, 70.0), c0z + offset(r1z, 70.0));
        let d1 = dist(c1x, c1z);
        if d1 <= distance {
            distance = d1;
            center_x = c1x;
            center_z = c1z;
            hash = hash_double(noise, 3201);
        }
        if hash_double(noise, 1066) > 0.3 {
            let r2x = 2.0 * (0.5 - hash_double(noise, 71));
            let r2z = 2.0 * (0.5 - hash_double(noise, 72));
            let (c2x, c2z) = (c0x + offset(r2x, 73.0), c0z + offset(r2z, 74.0));
            let d2 = dist(c2x, c2z);
            if d2 < distance {
                distance = d2;
                center_x = c2x;
                center_z = c2z;
                hash = hash_double(noise, 3202);
            }
        }
        if !in_hash(hash) {
            return None;
        }
        if self.center {
            return self.centre_checked(terrain, pos, [center_x as i32, 0, center_z as i32]);
        }
        let easing = clamped_map(distance, 0.0, 2500.0, 1.0, 0.0);
        (easing > f64::from(self.min_easing)).then_some(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::placement::PlacementModifier;

    #[test]
    fn decodes_the_modifiers() {
        for name in ["tfc:volcano", "tfc:tuff_ring", "tfc:tuya", "tfc:atoll"] {
            let json = format!(r#"{{"type":"{name}","center":true,"min_easing":0.25}}"#);
            assert!(matches!(
                PlacementModifier::from_json(&json),
                Ok(PlacementModifier::TfcCentered(_))
            ));
        }
        let json = r#"{"type":"tfc:stratovolcano","variant":"kelimutu","use_offset_center":true,"hash_min":0.5}"#;
        let Ok(PlacementModifier::TfcCentered(p)) = PlacementModifier::from_json(json) else {
            panic!("not decoded");
        };
        assert_eq!(p.variant.as_deref(), Some("kelimutu"));
        assert!(p.use_offset_center && p.hash_min == 0.5 && p.hash_max == 1.0);
        assert!(matches!(
            PlacementModifier::from_json(
                r#"{"type":"tfc:ocean_ridge","min_distance":2,"max_distance":9}"#
            ),
            Ok(PlacementModifier::TfcOceanRidge { min: 2, max: 9 })
        ));
    }

    #[test]
    fn centres_sit_at_full_easing_and_survive_the_chunk_check() {
        let terrain = TfcTerrain::new(1234);
        let volcanoes = terrain.volcanoes();
        let mut found = 0;
        for kind in [
            CenterKind::Tuya,
            CenterKind::TuffRing,
            CenterKind::Cinder,
            CenterKind::Atoll,
        ] {
            let placement = CenterPlacement {
                kind,
                center: true,
                use_offset_center: false,
                variant: None,
                min_easing: 0.0,
                max_easing: 1.0,
                hash_min: 0.0,
                hash_max: 1.0,
            };
            for gx in -40..40 {
                for gz in -40..40 {
                    let (x, z) = (gx * 160, gz * 160);
                    let biome = CenterPlacement::biome_at(&terrain, x, z);
                    if !Volcanoes::center_valid_biome(kind, biome) {
                        continue;
                    }
                    let freq = Volcanoes::center_frequency(biome);
                    let Some(c) = volcanoes.center_of(kind, x, 70, z, freq) else {
                        continue;
                    };
                    // Asking from the chunk of the centre returns it (when its own biome is valid).
                    if let Some(got) = placement.position(&terrain, c) {
                        assert_eq!(got, c);
                        found += 1;
                        if kind != CenterKind::Atoll {
                            assert!(volcanoes.center_easing(kind, c[0], c[2], freq) > 0.9);
                        }
                    }
                }
            }
        }
        assert!(found > 0, "no volcano centres found");
    }

    #[test]
    fn ocean_ridge_distance_is_cached_per_seed() {
        let a = crate::worldgen::tfc_biome_noise::ocean_ridge_distance_at(5, 100, 200);
        let b = crate::worldgen::tfc_biome_noise::ocean_ridge_edge_distance(5).noise(100.0, 200.0);
        assert_eq!(a, b);
        let c = crate::worldgen::tfc_biome_noise::ocean_ridge_distance_at(6, 100, 200);
        assert_ne!(a, c);
    }
}
