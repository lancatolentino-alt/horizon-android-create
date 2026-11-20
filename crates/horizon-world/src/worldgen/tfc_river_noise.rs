//! TerraFirmaCraft's river terrain shapes (`RiverNoise`, `RiverBlendType`, EUPL 1.2): each blend
//! type turns the surrounding height into a river valley and, for the 3D pass, carves caves.

use super::tfc_biome_ext::RiverBlendType;
use super::tfc_biome_noise::SEA_LEVEL_Y;
use super::tfc_noise::{Noise2D, OpenSimplex2D, OpenSimplex3D, clamped_map};
use super::tfc_region::Seed;
use super::tfc_river::{MAX_WIDTH, MIN_WIDTH};

const SEA: f64 = SEA_LEVEL_Y as f64;

fn f(x: f32) -> f64 {
    f64::from(x)
}
fn n(o: OpenSimplex2D) -> Noise2D {
    Noise2D::from_simplex(o)
}
fn clamp(v: f64, min: f64, max: f64) -> f64 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}
fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}
fn clamped_lerp(start: f64, end: f64, delta: f64) -> f64 {
    if delta < 0.0 {
        start
    } else if delta > 1.0 {
        end
    } else {
        lerp(delta, start, end)
    }
}
fn mth_map(v: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    lerp((v - from_min) / (from_max - from_min), to_min, to_max)
}

/// `RiverInfo`: the closest river edge's distance and width at a column, in blocks squared.
#[derive(Clone, Copy, Debug)]
pub struct RiverInfo {
    pub dist_sq: f64,
    pub width_sq: f64,
    /// `Flow` ordinal of the nearest edge (`tfc_flow::NONE` when unknown).
    pub flow: u8,
}

impl RiverInfo {
    pub fn norm_dist_sq(&self) -> f64 {
        self.dist_sq / self.width_sq
    }
}

pub trait RiverNoiseSampler: Send {
    fn set_column_and_sample_height(
        &mut self,
        _info: &RiverInfo,
        _x: i32,
        _z: i32,
        height_in: f64,
        _this_weight: f64,
    ) -> f64 {
        height_in
    }
    fn noise(&mut self, _y: i32, noise_in: f64) -> f64 {
        noise_in
    }
}

struct NoRiver;
impl RiverNoiseSampler for NoRiver {}

/// The roof of a river tunnel that is always tall enough to boat through.
fn tunnel_roof(height: f64, y: i32) -> bool {
    let y = f64::from(y);
    height < SEA && y > height && y < SEA + 2.0 + (SEA - height)
}

/// The cave roof plus "air above the height" rule shared by most river types.
fn simple_noise(height: f64, y: i32, noise_in: f64) -> f64 {
    if tunnel_roof(height, y) {
        1.0
    } else if f64::from(y) > height {
        0.0
    } else {
        noise_in
    }
}

struct Banked {
    dist: Noise2D,
    bank_cut: Noise2D,
    height: f64,
}
impl RiverNoiseSampler for Banked {
    fn set_column_and_sample_height(
        &mut self,
        info: &RiverInfo,
        x: i32,
        z: i32,
        height_in: f64,
        _w: f64,
    ) -> f64 {
        let (fx, fz) = (f64::from(x), f64::from(z));
        let dist_fac = info.norm_dist_sq() * f(0.8f32) + self.dist.noise(fx, fz);
        let river_height = 57.0 + if dist_fac < 1.0 { dist_fac * 6.0 } else { 6.0 };
        let height_in_weight = clamp(dist_fac - 1.0, 0.0, 2.0);
        let river_weight = 2.0 - height_in_weight;
        self.height = ((height_in * height_in_weight + river_height * river_weight) / 2.0)
            .min(self.bank_cut.noise(fx, fz));
        self.height
    }
    fn noise(&mut self, y: i32, noise_in: f64) -> f64 {
        simple_noise(self.height, y, noise_in)
    }
}

struct TallBanked {
    dist: Noise2D,
    surface: Noise2D,
    height: f64,
}
impl RiverNoiseSampler for TallBanked {
    fn set_column_and_sample_height(
        &mut self,
        info: &RiverInfo,
        x: i32,
        z: i32,
        height_in: f64,
        _w: f64,
    ) -> f64 {
        let (fx, fz) = (f64::from(x), f64::from(z));
        let dist_fac = info.norm_dist_sq() * f(0.8f32) + self.dist.noise(fx, fz);
        let river_height =
            57.0 + if dist_fac < 1.0 { dist_fac * 9.0 } else { 9.0 } + self.surface.noise(fx, fz);
        let height_in_weight = clamp(dist_fac - 1.0, 0.0, 2.0);
        let river_weight = 2.0 - height_in_weight;
        self.height = (height_in * height_in_weight + river_height * river_weight) / 2.0;
        self.height
    }
    fn noise(&mut self, y: i32, noise_in: f64) -> f64 {
        simple_noise(self.height, y, noise_in)
    }
}

struct Floodplain {
    dist: Noise2D,
    height: f64,
}
impl RiverNoiseSampler for Floodplain {
    fn set_column_and_sample_height(
        &mut self,
        info: &RiverInfo,
        x: i32,
        z: i32,
        height_in: f64,
        _w: f64,
    ) -> f64 {
        let dist_fac =
            info.norm_dist_sq() * f(0.8f32) + self.dist.noise(f64::from(x), f64::from(z));
        let river_height = if dist_fac < 1.0 {
            let depth = clamped_map(
                info.width_sq,
                f64::from(MIN_WIDTH * MIN_WIDTH),
                f64::from(MAX_WIDTH * MAX_WIDTH),
                3.0,
                6.0,
            );
            SEA - 2.0 - depth + dist_fac * depth
        } else if dist_fac < 2.0 {
            SEA - 2.0
        } else {
            let height_in_weight = clamp(2.0 * dist_fac - 4.0, 0.0, 1.0);
            let river_weight = 1.0 - height_in_weight;
            (SEA - 1.5) * river_weight + height_in * height_in_weight
        };
        self.height = river_height.min(height_in);
        self.height
    }
    fn noise(&mut self, y: i32, noise_in: f64) -> f64 {
        let fy = f64::from(y);
        if self.height < SEA - 2.5 && fy > self.height && fy < SEA + 2.0 + (SEA - self.height) {
            1.0
        } else if fy > self.height {
            0.0
        } else {
            noise_in
        }
    }
}

struct Wide {
    base: Noise2D,
    dist: Noise2D,
    offset: f64,
    height: f64,
}
impl RiverNoiseSampler for Wide {
    fn set_column_and_sample_height(
        &mut self,
        info: &RiverInfo,
        x: i32,
        z: i32,
        height_in: f64,
        _w: f64,
    ) -> f64 {
        let (fx, fz) = (f64::from(x), f64::from(z));
        let dist_fac = info.norm_dist_sq() * f(0.8f32) + self.dist.noise(fx, fz);
        let river_height = self.offset + dist_fac * 7.0 + self.base.noise(fx, fz);
        self.height = river_height.min(height_in);
        self.height
    }
    fn noise(&mut self, y: i32, noise_in: f64) -> f64 {
        simple_noise(self.height, y, noise_in)
    }
}

struct Canyon {
    base: Noise2D,
    dist: Noise2D,
    low_freq_cliff: Noise2D,
    height: f64,
}
impl RiverNoiseSampler for Canyon {
    fn set_column_and_sample_height(
        &mut self,
        info: &RiverInfo,
        x: i32,
        z: i32,
        height_in: f64,
        _w: f64,
    ) -> f64 {
        let (fx, fz) = (f64::from(x), f64::from(z));
        let dist_fac = info.norm_dist_sq() * 1.3 + self.dist.noise(fx, fz);
        let adj = if dist_fac > 0.6 {
            dist_fac * 0.4 + 0.8
        } else {
            dist_fac
        };
        let river_height = 55.0
            + lerp(self.low_freq_cliff.noise(fx, fz), dist_fac, adj) * 16.0
            + self.base.noise(fx, fz);
        self.height = river_height.min(height_in);
        self.height
    }
    fn noise(&mut self, y: i32, noise_in: f64) -> f64 {
        simple_noise(self.height, y, noise_in)
    }
}

struct TallCanyon {
    base: Noise2D,
    dist: Noise2D,
    cliff: OpenSimplex3D,
    height: f64,
    dist_fac: f64,
    x: i32,
    z: i32,
}
impl TallCanyon {
    fn raw_noise(&self, y: i32) -> f64 {
        let (fx, fy, fz) = (f64::from(self.x), f64::from(y), f64::from(self.z));
        if y > SEA_LEVEL_Y + 35 {
            0.0
        } else if y > SEA_LEVEL_Y + 20 {
            let easing = f64::from(1.0f32 - (y - SEA_LEVEL_Y - 20) as f32 / 15.0f32);
            easing * self.cliff.noise(fx, fy, fz)
        } else if y > SEA_LEVEL_Y {
            self.cliff.noise(fx, fy, fz)
        } else if y > SEA_LEVEL_Y - 8 {
            let easing = f64::from(y - SEA_LEVEL_Y + 8) / 8.0;
            easing * self.cliff.noise(fx, fy, fz)
        } else {
            0.0
        }
    }
}
impl RiverNoiseSampler for TallCanyon {
    fn set_column_and_sample_height(
        &mut self,
        info: &RiverInfo,
        x: i32,
        z: i32,
        height_in: f64,
        _w: f64,
    ) -> f64 {
        let dist_fac = info.norm_dist_sq() * 1.3 + self.dist.noise(f64::from(x), f64::from(z));
        let adj = if dist_fac > 0.32 {
            dist_fac * 0.2 + 1.6
        } else {
            dist_fac
        };
        let river_height = 55.0 + adj * 16.0 + self.base.noise(f64::from(x), f64::from(z));
        let width_factor = clamped_map(info.width_sq, 144.0, 324.0, 0.7, 1.1);
        self.dist_fac = (dist_fac * width_factor).max(0.0);
        self.x = x;
        self.z = z;
        self.height = river_height.min(height_in);
        self.height
    }
    fn noise(&mut self, y: i32, noise_in: f64) -> f64 {
        if tunnel_roof(self.height, y) {
            return 1.0;
        }
        clamped_lerp(self.raw_noise(y), noise_in, self.dist_fac)
    }
}

struct Talus {
    base: Noise2D,
    cliff_height: Noise2D,
    dist: Noise2D,
    height: f64,
}
impl RiverNoiseSampler for Talus {
    fn set_column_and_sample_height(
        &mut self,
        info: &RiverInfo,
        x: i32,
        z: i32,
        height_in: f64,
        weight: f64,
    ) -> f64 {
        let (fx, fz) = (f64::from(x), f64::from(z));
        let dist_fac = info.norm_dist_sq().sqrt() + self.dist.noise(fx, fz);
        let talus = 55.0
            + dist_fac * 12.0
            + self.base.noise(fx, fz)
            + if dist_fac > 1.5 {
                self.cliff_height.noise(fx, fz)
            } else {
                0.0
            };
        let canyon = 55.0 + info.norm_dist_sq() * 1.3 * 16.0;
        let river_height = clamped_map(weight, 0.9, 1.0, canyon, talus);
        self.height = river_height.min(height_in);
        self.height
    }
    fn noise(&mut self, y: i32, noise_in: f64) -> f64 {
        simple_noise(self.height, y, noise_in)
    }
}

struct Terraces {
    base: Noise2D,
    cliff_height: Noise2D,
    cliff_base: Noise2D,
    dist: Noise2D,
    height: f64,
}
impl RiverNoiseSampler for Terraces {
    fn set_column_and_sample_height(
        &mut self,
        info: &RiverInfo,
        x: i32,
        z: i32,
        height_in: f64,
        weight: f64,
    ) -> f64 {
        let (fx, fz) = (f64::from(x), f64::from(z));
        let dist_fac = info.norm_dist_sq().sqrt() * 0.85 + self.dist.noise(fx, fz);
        let sloped = 54.0 + dist_fac * 12.0 + self.base.noise(fx, fz);
        let cliff_base = self.cliff_base.noise(fx, fz);
        let cliff_height = self.cliff_height.noise(fx, fz);
        let lower = if sloped > cliff_base {
            clamped_map(sloped, cliff_base, cliff_base + 1.1, 0.0, cliff_height)
        } else {
            0.0
        };
        let upper = if sloped > cliff_base + cliff_height {
            clamped_map(
                sloped,
                cliff_base + cliff_height,
                cliff_base + cliff_height + 1.4,
                0.0,
                cliff_height + 4.0,
            )
        } else {
            0.0
        };
        let terrace = sloped + lower + upper;
        let canyon = 55.0 + info.norm_dist_sq() * 1.3 * 16.0;
        let river_height = clamped_map(weight, 0.9, 1.0, canyon, terrace);
        self.height = river_height.min(height_in);
        self.height
    }
    fn noise(&mut self, y: i32, noise_in: f64) -> f64 {
        simple_noise(self.height, y, noise_in)
    }
}

struct Cave {
    carving_center: Noise2D,
    carving_height: Noise2D,
    dist_squared: f64,
    weight: f64,
    height: f64,
    height_now: f64,
    center: f64,
    canyon_height: f64,
}
impl RiverNoiseSampler for Cave {
    fn set_column_and_sample_height(
        &mut self,
        info: &RiverInfo,
        x: i32,
        z: i32,
        height_in: f64,
        this_weight: f64,
    ) -> f64 {
        let (fx, fz) = (f64::from(x), f64::from(z));
        self.dist_squared = clamp(info.norm_dist_sq() * 1.3 - 0.1, 0.0, 1.0);
        self.weight = this_weight;
        self.height_now = height_in;
        self.height = self.carving_height.noise(fx, fz);
        self.center = self.carving_center.noise(fx, fz);
        self.canyon_height = 55.0 + info.norm_dist_sq() * 1.3 * 16.0;
        let max_height = self.center + self.height;
        if this_weight > 0.75 {
            height_in
        } else {
            let canyon_max = self.canyon_height.min(height_in);
            if this_weight > 0.5 {
                let interior =
                    mth_map(this_weight, 0.5, 0.75, max_height.min(height_in), height_in);
                let exterior =
                    mth_map(this_weight, 0.5, 0.75, canyon_max.min(height_in), height_in);
                self.height_now = lerp(self.dist_squared, interior, exterior);
                self.height_now
            } else {
                canyon_max
            }
        }
    }
    fn noise(&mut self, y: i32, noise_in: f64) -> f64 {
        let fy = f64::from(y);
        if self.canyon_height < SEA
            && fy > self.canyon_height
            && fy < SEA + 2.0 + (SEA - self.canyon_height)
        {
            return 1.0;
        }
        let mut vert = (fy - self.center) / self.height;
        if vert > 0.0 {
            vert = vert * self.weight * self.weight;
        }
        let column = (1.0 - vert * vert).max(0.0);
        let noise = lerp(self.dist_squared, column, noise_in);
        noise * clamped_map(self.weight, 0.5, 0.25, 1.0, 0.0)
    }
}

/// `RiverBlendType.createNoiseSampler(seed)`: draws seeds from `seed` in the order Java does.
pub fn create_sampler(kind: RiverBlendType, seed: &mut Seed) -> Box<dyn RiverNoiseSampler> {
    let s = |seed: &mut Seed| OpenSimplex2D::new(seed.next());
    match kind {
        RiverBlendType::None => Box::new(NoRiver),
        RiverBlendType::Banked => {
            let dist = n(s(seed)
                .octaves(3)
                .spread(f(0.05f32))
                .scaled(f(-0.2f32), f(0.2f32)));
            let bank_cut =
                n(s(seed).octaves(3))
                    .abs()
                    .spread(0.025)
                    .scaled4(0.0, 1.0, SEA - 4.0, SEA + 30.0);
            Box::new(Banked {
                dist,
                bank_cut,
                height: 0.0,
            })
        }
        RiverBlendType::TallBanked => {
            let dist = n(s(seed)
                .octaves(3)
                .spread(f(0.05f32))
                .scaled(f(-0.2f32), f(0.2f32)));
            let surface = n(s(seed).octaves(4).spread(0.07).scaled(-2.0, 2.0));
            Box::new(TallBanked {
                dist,
                surface,
                height: 0.0,
            })
        }
        RiverBlendType::Floodplain => {
            let dist = n(s(seed)
                .octaves(4)
                .spread(f(0.05f32))
                .scaled(f(-0.2f32), f(0.2f32)));
            Box::new(Floodplain { dist, height: 0.0 })
        }
        RiverBlendType::Wide | RiverBlendType::WideDeep => {
            let base = n(s(seed)
                .octaves(4)
                .spread(f(0.05f32))
                .scaled(f(-2.5f32), f(1.5f32)));
            let dist = n(s(seed)
                .octaves(4)
                .spread(f(0.05f32))
                .scaled(f(-0.15f32), f(0.15f32)));
            let offset = if kind == RiverBlendType::Wide {
                58.0
            } else {
                55.0
            };
            Box::new(Wide {
                base,
                dist,
                offset,
                height: 0.0,
            })
        }
        RiverBlendType::Canyon => {
            let base = n(s(seed).octaves(4).spread(f(0.05f32)).scaled(-7.0, 3.0));
            let dist = n(s(seed)
                .octaves(4)
                .spread(f(0.05f32))
                .scaled(f(-0.3f32), f(0.2f32)));
            let low_freq_cliff = n(s(seed).spread(f(0.0007f32))).clamped(0.0, 1.0);
            Box::new(Canyon {
                base,
                dist,
                low_freq_cliff,
                height: 0.0,
            })
        }
        RiverBlendType::TallCanyon => {
            let base = n(s(seed).octaves(4).spread(f(0.05f32)).scaled(-7.0, 3.0));
            let dist = n(s(seed)
                .octaves(4)
                .spread(f(0.05f32))
                .scaled(f(-0.3f32), f(0.2f32)));
            let cliff = OpenSimplex3D::new(seed.next())
                .octaves(2)
                .spread(f(0.1f32))
                .scaled(0.0, 3.0);
            Box::new(TallCanyon {
                base,
                dist,
                cliff,
                height: 0.0,
                dist_fac: 0.0,
                x: 0,
                z: 0,
            })
        }
        RiverBlendType::Talus => {
            let base = n(s(seed)
                .octaves(4)
                .spread(f(0.05f32))
                .scaled(f(-2.5f32), f(1.5f32)));
            let cliff_height = n(s(seed)
                .octaves(2)
                .spread(f(0.1f32))
                .scaled(f(3f32), f(8f32)));
            let dist = n(s(seed)
                .octaves(4)
                .spread(f(0.05f32))
                .scaled(f(-0.15f32), f(0.15f32)));
            Box::new(Talus {
                base,
                cliff_height,
                dist,
                height: 0.0,
            })
        }
        RiverBlendType::Terraces => {
            let base = n(s(seed)
                .octaves(4)
                .spread(f(0.05f32))
                .scaled(f(-2.5f32), f(1.5f32)));
            let cliff_height = n(s(seed)
                .octaves(2)
                .spread(f(0.1f32))
                .scaled(f(4f32), f(8f32)));
            let cliff_base = n(s(seed)
                .octaves(2)
                .spread(f(0.06f32))
                .scaled(SEA - 2.0, SEA + 4.0));
            let dist = n(s(seed)
                .octaves(4)
                .spread(f(0.05f32))
                .scaled(f(-0.15f32), f(0.15f32)));
            Box::new(Terraces {
                base,
                cliff_height,
                cliff_base,
                dist,
                height: 0.0,
            })
        }
        RiverBlendType::Cave => {
            let carving_center = n(s(seed)
                .octaves(2)
                .spread(f(0.02f32))
                .scaled(SEA - 3.0, SEA + 3.0));
            let carving_height = n(s(seed).octaves(4).spread(f(0.15f32)).scaled(8.0, 14.0));
            Box::new(Cave {
                carving_center,
                carving_height,
                dist_squared: 0.0,
                weight: 0.0,
                height: 0.0,
                height_now: 0.0,
                center: 0.0,
                canyon_height: 0.0,
            })
        }
    }
}
