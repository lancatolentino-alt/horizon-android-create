//! TerraFirmaCraft's `BadlandsSurfaceBuilder` (EUPL 1.2): banded sand and sandstone strata, with
//! soil on top for the plain variant, in four flavours (normal, mesas, hoodoos, warped).

use super::tfc_noise::{OpenSimplex2D, clamped_map};
use super::tfc_surface::{
    Cell, Column, Layers, RockBlock, SEA_LEVEL, SoilType, State, SurfaceCtx, altitude_slope_depth,
    build_with, fixed, mid_type, surface_type, under_type,
};
use super::tfc_volcano_strato::{put_state, state_in};
use horizon_core::random::{RandomSource, XoroshiroRandom};

const PRIMARY_SIZE: usize = 8;
const SECONDARY_SIZE: usize = 5;
const LAYER_SIZE: usize = 16;

/// A small deterministic generator standing in for TFC's `seed.fork()` random source, whose
/// position in the seed stream depends on how many noises earlier biomes made.
type Layer = [&'static str; LAYER_SIZE];

/// `fill`: 8 primary, 5 secondary and 3 uncommon blocks, shuffled.
fn fill(
    random: &mut XoroshiroRandom,
    primary: &'static str,
    secondary: &'static str,
    uncommon: &'static str,
) -> Layer {
    let mut layers = [uncommon; LAYER_SIZE];
    for (i, slot) in layers.iter_mut().enumerate() {
        *slot = if i < PRIMARY_SIZE {
            primary
        } else if i < PRIMARY_SIZE + SECONDARY_SIZE {
            secondary
        } else {
            uncommon
        };
    }
    // `Helpers.shuffleArray`
    for i in (2..=LAYER_SIZE).rev() {
        let j = random.next_int_bound(i as i32).unwrap() as usize;
        layers.swap(i - 1, j);
    }
    layers
}

#[derive(Clone, Copy)]
enum Colour {
    Red,
    Brown,
    Yellow,
    White,
}

fn sand(colour: Colour) -> &'static str {
    match colour {
        Colour::Red => "tfc:sand/red",
        Colour::Brown => "tfc:sand/brown",
        Colour::Yellow => "tfc:sand/yellow",
        Colour::White => "tfc:sand/white",
    }
}

fn sandstone(colour: Colour) -> &'static str {
    match colour {
        Colour::Red => "tfc:raw_sandstone/red",
        Colour::Brown => "tfc:raw_sandstone/brown",
        Colour::Yellow => "tfc:raw_sandstone/yellow",
        Colour::White => "tfc:raw_sandstone/white",
    }
}

struct Strata {
    sand0: Layer,
    sand1: Layer,
    sand_karst: Layer,
    sand_volcanic: Layer,
    stone0: Layer,
    stone1: Layer,
    stone_karst: Layer,
    stone_volcanic: Layer,
}

pub struct Badlands {
    inverted: bool,
    dipping: bool,
    soil_min_depth: i32,
    strata: Strata,
    thresholds: [f32; LAYER_SIZE],
    grass_height: OpenSimplex2D,
    sand_height_offset: OpenSimplex2D,
    sand_style: OpenSimplex2D,
}

/// The four variants, in the order of `BadlandsSurfaceBuilder`'s factories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Normal,
    Mesas,
    Hoodoos,
    Warped,
}

impl Badlands {
    /// `new BadlandsSurfaceBuilder(.., seed)`: `random` is the stream's `seed.fork()`.
    pub fn new(variant: Variant, mut random: XoroshiroRandom) -> Self {
        let (inverted, dipping, soil_min_depth) = match variant {
            Variant::Normal => (false, false, -1),
            Variant::Mesas => (true, false, -3),
            Variant::Hoodoos => (true, false, -7),
            Variant::Warped => (true, true, -20),
        };
        let sand_height_offset = if dipping {
            OpenSimplex2D::new(random.next_long())
                .octaves(2)
                .scaled(-250.0, 250.0)
                .spread(0.005)
        } else {
            OpenSimplex2D::new(random.next_long())
                .octaves(2)
                .scaled(0.0, 6.0)
                .spread(f64::from(0.0014f32))
        };
        use Colour::*;
        let mut pair = |a: Colour, b: Colour, c: Colour| {
            (
                fill(&mut random, sand(a), sand(b), sand(c)),
                fill(&mut random, sandstone(a), sandstone(b), sandstone(c)),
            )
        };
        let (sand0, stone0) = pair(Red, Brown, Yellow);
        let (sand1, stone1) = pair(Brown, Yellow, White);
        let (sand_karst, stone_karst) = pair(Red, Yellow, White);
        let sand_volcanic = fill(
            &mut random,
            "tfc:sand/black",
            "tfc:gravel_tuff",
            "tfc:sand/red",
        );
        let stone_volcanic = fill(
            &mut random,
            "tfc:raw_sandstone/black",
            "tfc:raw_tuff",
            "tfc:raw_sandstone/red",
        );
        let mut thresholds = [0.0; LAYER_SIZE];
        for t in &mut thresholds {
            *t = random.next_float();
        }
        let sea = f64::from(SEA_LEVEL);
        Self {
            inverted,
            dipping,
            soil_min_depth,
            strata: Strata {
                sand0,
                sand1,
                sand_karst,
                sand_volcanic,
                stone0,
                stone1,
                stone_karst,
                stone_volcanic,
            },
            thresholds,
            grass_height: OpenSimplex2D::new(random.next_long())
                .octaves(2)
                .scaled(sea + 14.0, sea + 18.0)
                .spread(f64::from(0.5f32)),
            sand_height_offset,
            sand_style: OpenSimplex2D::new(random.next_long())
                .octaves(2)
                .scaled(f64::from(-0.3f32), f64::from(1.3f32))
                .spread(f64::from(0.0003f32)),
            // (the plain variant never reads the offset's range; kept for all four)
        }
    }

    fn sample_layer(
        &self,
        layers0: &Layer,
        layers1: &Layer,
        y: i32,
        threshold: f32,
    ) -> &'static str {
        // Thicker layers when the strata dip, so they look continuous.
        let height = if self.dipping { y / 4 } else { y };
        let index = height.rem_euclid(LAYER_SIZE as i32) as usize;
        if self.thresholds[index] < threshold {
            layers0[index]
        } else {
            layers1[index]
        }
    }

    fn sand_layer(&self, y: i32, style: f32, karst: bool, volcanic: bool) -> &'static str {
        let s = &self.strata;
        let (a, b) = if karst {
            (&s.sand_karst, &s.sand_karst)
        } else if volcanic {
            (&s.sand_volcanic, &s.sand_volcanic)
        } else {
            (&s.sand0, &s.sand1)
        };
        self.sample_layer(a, b, y, style)
    }

    fn stone_layer(&self, y: i32, style: f32, karst: bool, volcanic: bool) -> &'static str {
        let s = &self.strata;
        let (a, b) = if karst {
            (&s.stone_karst, &s.stone_karst)
        } else if volcanic {
            (&s.stone_volcanic, &s.stone_volcanic)
        } else {
            (&s.stone0, &s.stone1)
        };
        self.sample_layer(a, b, y, style)
    }

    pub fn build(&self, column: &mut Column, ctx: &mut SurfaceCtx, start_y: i32, end_y: i32) {
        let (x, z) = (f64::from(ctx.x), f64::from(ctx.z));
        let height_variation = self.grass_height.noise(x, z);
        let weight_variation = (1.0 - ctx.weight) * 23.0;
        let rainfall_variation =
            clamped_map(f64::from(ctx.ground_water()), 100.0, 500.0, 0.0, 22.0);
        // `RockSettings.karst()` and `mafic()` of `context.getRock()`. TFC's cursor is still at
        // y = 0 when this runs (`SurfaceBuilderContext.buildSurface`), so it is the rock at y = 0.
        let rock = (ctx.rock)(0);
        let karst = matches!(rock, "chalk" | "dolomite" | "limestone" | "marble");
        let volcanic = matches!(rock, "basalt" | "gabbro");
        if self.inverted {
            let shift = if self.dipping { -10.0 } else { -16.0 };
            let base = (shift + height_variation + weight_variation + rainfall_variation) as i32;
            self.build_sandstone(column, ctx, start_y, end_y, karst, volcanic, base);
        } else if f64::from(start_y - 5) > height_variation - weight_variation - rainfall_variation
        {
            let layers = Layers::three(
                surface_type(SoilType::Grass, RockBlock::Sand),
                mid_type(SoilType::Dirt, RockBlock::Sand),
                under_type(),
            );
            build_with(column, ctx, start_y, end_y, -1, &layers);
        } else {
            self.build_sandy(column, ctx, start_y, end_y, karst, volcanic);
        }
    }

    fn style_and_height(&self, ctx: &SurfaceCtx) -> (f32, i32) {
        let (x, z) = (f64::from(ctx.x), f64::from(ctx.z));
        (
            self.sand_style.noise(x, z) as f32,
            self.sand_height_offset.noise(x, z) as i32,
        )
    }

    fn build_sandy(
        &self,
        column: &mut Column,
        ctx: &mut SurfaceCtx,
        start_y: i32,
        end_y: i32,
        karst: bool,
        volcanic: bool,
    ) {
        let (style, height) = self.style_and_height(ctx);
        let mut surface_depth = -1;
        let mut y = start_y;
        while y >= end_y {
            match state_in(column, y) {
                Cell::Air => surface_depth = -1,
                Cell::Rock => {
                    if surface_depth == -1 {
                        if y < SEA_LEVEL - 1 {
                            put_state(column, ctx, y, &State::Rock(RockBlock::Sand));
                        } else {
                            let name = self.sand_layer(y + height, style, karst, volcanic);
                            put_state(column, ctx, y, &fixed(name));
                            surface_depth = 3;
                        }
                    } else if surface_depth > 0 {
                        surface_depth -= 1;
                        let name = self.stone_layer(y + height, style, karst, volcanic);
                        put_state(column, ctx, y, &fixed(name));
                    }
                }
                _ => {}
            }
            y -= 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_sandstone(
        &self,
        column: &mut Column,
        ctx: &mut SurfaceCtx,
        start_y: i32,
        end_y: i32,
        karst: bool,
        volcanic: bool,
        sandstone_base_height: i32,
    ) {
        let (style, height) = self.style_and_height(ctx);
        let mut surface_depth = -1;
        let mut sandstone_depth = start_y - sandstone_base_height;
        let mut y = start_y;
        while y >= end_y {
            match state_in(column, y) {
                Cell::Air => surface_depth = -1,
                Cell::Rock => {
                    if surface_depth == -1 {
                        if y < SEA_LEVEL - 1 {
                            put_state(column, ctx, y, &State::Rock(RockBlock::Sand));
                        } else {
                            surface_depth =
                                altitude_slope_depth(y, ctx.slope, self.soil_min_depth, 5);
                            if surface_depth < 0 {
                                if sandstone_depth > 0 {
                                    let name = self.stone_layer(y + height, style, karst, volcanic);
                                    put_state(column, ctx, y, &fixed(name));
                                }
                                surface_depth = 0;
                            } else {
                                put_state(
                                    column,
                                    ctx,
                                    y,
                                    &surface_type(SoilType::Grass, RockBlock::Sand),
                                );
                                sandstone_depth -= surface_depth;
                            }
                        }
                    } else if surface_depth > 0 {
                        surface_depth -= 1;
                        put_state(column, ctx, y, &mid_type(SoilType::Dirt, RockBlock::Sand));
                    } else if sandstone_depth > 0 {
                        sandstone_depth -= 1;
                        let name = self.stone_layer(y + height, style, karst, volcanic);
                        put_state(column, ctx, y, &fixed(name));
                    }
                }
                _ => {}
            }
            y -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_layer_has_eight_five_and_three_blocks() {
        let b = Badlands::new(Variant::Mesas, XoroshiroRandom::new(99));
        let count = |layer: &Layer, name: &str| layer.iter().filter(|n| **n == name).count();
        assert_eq!(count(&b.strata.sand_volcanic, "tfc:sand/black"), 8);
        assert_eq!(count(&b.strata.sand_volcanic, "tfc:gravel_tuff"), 5);
        assert_eq!(count(&b.strata.sand_volcanic, "tfc:sand/red"), 3);
    }

    #[test]
    fn layers_repeat_every_sixteen_blocks() {
        let b = Badlands::new(Variant::Mesas, XoroshiroRandom::new(5));
        for y in -20..20 {
            assert_eq!(
                b.sample_layer(&b.strata.sand0, &b.strata.sand1, y, 0.5),
                b.sample_layer(&b.strata.sand0, &b.strata.sand1, y + 16, 0.5)
            );
        }
    }
}
