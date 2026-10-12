//! Effects > Film Grain: a Boolean grain model after Newson, Delon & Galerne, "A
//! Stochastic Film Grain Model for Resolution-Independent Rendering" (2017).
//!
//! Dark grains (silver on the print) of lognormal radius lie at random positions in
//! full-resolution pixels, so a preview, a region and the export show the same grains.
//! Each of `CANDIDATES` staggered grids of square cells holds one grain a cell, of
//! hashed centre, radius and threshold; a grain is present where the tone's density passes its
//! threshold. Presence and the grains' soft edges are continuous, so neighbouring
//! pixels at smooth gradients see the same grains and the GPU port stays within float
//! rounding of this one. The density is chosen so the share left uncovered averages the
//! pixel's luminance: isolated grains in highlights, small gaps between overlapping ones
//! in shadows, strongest in the midtones and nothing at black or white.
//!
//! The density follows the Poisson approximation `E[uncovered] = exp(−N·p·a/cell²)`
//! for a finite pool of candidates, so the grain is mean-preserving only approximately
//! (within 0.02 of Amount on flat tones). Previews down to half size average exactly
//! the full-resolution pixels under each pixel; smaller ones take one sample a pixel
//! and scale what is left: a statistical approximation of the resized export, as the
//! measured Grain's.
//!
//! Color adds the colour of colour film's grain: each dye layer has a grain of its own,
//! over its own channel, of dye clouds about three times the silver grains' size (blue's,
//! the fastest layer's, the largest). Only the layers' differences from their
//! luminance are added, so the grain's brightness stays the mono grain's and its
//! colour varies more slowly: film's colour mottling rather than a sensor's colour
//! noise.
use crate::model::film_grain::FilmGrain;

/// The long edge, in pixels, at which grain sizes below are given.
const REFERENCE_EDGE: f32 = 6000.;
/// Mean grain radius (pixels at the reference edge) at Size 0 and per unit of Size, and
/// the smallest in pixels at any photo size: smaller grains would average away, and too
/// faint to cover the shadows.
const RADIUS_BASE: f32 = 0.35;
const RADIUS_PER_SIZE: f32 = 1.2;
const RADIUS_MIN: f32 = 0.35;
/// Lognormal log standard deviation at Roughness 0 and per unit of Roughness.
const SIGMA_BASE: f32 = 0.1;
const SIGMA_PER_ROUGHNESS: f32 = 0.5;
/// A grain's soft edge (an enlarger's or scanner's blur): its half-width as a share of
/// the radius, and at least `EDGE_MIN` pixels.
const EDGE_SOFTNESS: f32 = 0.5;
const EDGE_MIN: f32 = 0.6;
/// The smallest radius a grain is drawn at, about a pixel's footprint: a smaller grain
/// spreads over it, fainter, with its area kept, as a pixel averages it.
const BLUR: f32 = 0.8;
/// The largest radius, in mean radii: the lognormal's tail beyond is clamped.
const TAIL: f32 = 2.5;
/// What Amount 1 adds, as a share of the full grain render's deviation from the tone.
pub(crate) const STRENGTH: f32 = 0.4;
/// Candidate grains per cell.
pub(crate) const CANDIDATES: u32 = 16;
/// Half-width of a grain's presence window around its threshold.
const PRESENCE: f32 = 0.02;
/// The lightest and darkest tones the density is calibrated for: beyond them the grain
/// fades out. The darkest also keeps the presence below `MAX_PRESENCE`.
const TONE_LIMIT: f32 = 0.02;
const MAX_PRESENCE: f32 = 0.95;
/// Hash salts for a candidate's threshold, centre and radius.
const SALT_THRESHOLD: u32 = 0x68e3_1da4;
const SALT_X: u32 = 0xb5ad_4ece;
const SALT_Y: u32 = 0x1b87_3593;
const SALT_RADIUS: u32 = 0xcc9e_2d51;
const SALT_ANGLE: u32 = 0x27d4_eb2f;
const CANDIDATE_STEP: u32 = 0x632b_e5ab;
/// Colour grain: each dye layer's grain size relative to the mono grain, red, green,
/// blue, and its seed's salt. Dye clouds are larger and softer than silver grains, so
/// the colour varies more slowly than the grain's brightness; blue, the fastest layer,
/// is the coarsest.
const LAYER_SIZE: [f32; 3] = [3.2, 3., 3.8];
/// How much of the dye layers' colour Color 1 adds: film's colour grain is subtler than
/// its brightness grain.
pub(crate) const COLOR_STRENGTH: f32 = 0.5;
const LAYER_SALT: [u32; 3] = [0x3c6e_f372, 0x9b05_688c, 0xa54f_f53a];
/// The R2 low-discrepancy sequence's steps, offsetting each candidate's grid.
const R2: [f32; 2] = [0.754_877_7, 0.569_840_3];

/// Integer hash to `[0, 1)` in 24-bit steps, which `f32` holds exactly on the CPU and
/// the GPU.
pub(crate) fn unit_hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut v = (x as u32).wrapping_mul(0x9e37_79b9) ^ (y as u32).wrapping_mul(0x85eb_ca6b) ^ seed;
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb_352d);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846c_a68b);
    v ^= v >> 16;
    (v >> 8) as f32 / 16_777_216.
}
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}
/// How a grain of radius `r` is drawn: the radius, soft-edge half-width (in
/// full-resolution pixels) and opacity of its disc.
fn grain_shape(r: f32) -> (f32, f32, f32) {
    let drawn = r.max(BLUR);
    let w = (EDGE_SOFTNESS * drawn).max(EDGE_MIN);
    (drawn, w, (r / drawn).powi(2))
}
/// A grain's coverage at distance `d` from its centre.
fn coverage(r: f32, d: f32) -> f32 {
    let (drawn, w, opacity) = grain_shape(r);
    opacity * (1. - smoothstep(drawn - w, drawn + w, d))
}
/// How far a grain of radius `r` reaches.
fn reach(r: f32) -> f32 {
    let (drawn, w, _) = grain_shape(r);
    drawn + w
}
/// The area a grain of radius `r` covers, its soft edge counted by its coverage.
fn effective_area(r: f32) -> f32 {
    let outer = reach(r);
    let steps = 64;
    let step = outer / steps as f32;
    (0..steps)
        .map(|i| {
            let d = (i as f32 + 0.5) * step;
            coverage(r, d) * 2. * std::f32::consts::PI * d * step
        })
        .sum()
}

/// A recipe's Film Grain for an output whose full-resolution long edge is `edge`
/// pixels: the mono grain, and the red, green and blue dye layers' for Color. Shared
/// by the CPU and the GPU preview.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FilmGrainLayers {
    pub(crate) color: f32,
    pub(crate) mono: FilmGrainField,
    /// Red, green and blue.
    pub(crate) layers: [FilmGrainField; 3],
}
impl FilmGrainLayers {
    pub(crate) fn new(g: &FilmGrain, edge: f32) -> Self {
        Self {
            color: g.color,
            mono: FilmGrainField::layer(g, edge, 1., g.seed),
            layers: std::array::from_fn(|c| {
                FilmGrainField::layer(g, edge, LAYER_SIZE[c], g.seed ^ LAYER_SALT[c])
            }),
        }
    }
    /// What the grain adds to each channel of encoded `rgb` at full-resolution
    /// position `x`, `y`, for an output of `scale` pixels per full-resolution pixel:
    /// the mono grain over the luminance, and with Color each dye layer's grain over its
    /// own channel less what they add to the luminance, so Color changes the grain's
    /// colour, not its brightness.
    pub(crate) fn delta(&self, x: f32, y: f32, scale: f32, rgb: [f32; 3]) -> [f32; 3] {
        let mono = self.mono.delta(x, y, scale, crate::color::luminance(rgb));
        if self.color == 0. {
            return [mono; 3];
        }
        let dye: [f32; 3] = std::array::from_fn(|c| self.layers[c].delta(x, y, scale, rgb[c]));
        let neutral = crate::color::luminance(dye);
        dye.map(|d| mono + COLOR_STRENGTH * self.color * (d - neutral))
    }
}

/// One grain layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FilmGrainField {
    pub(crate) amount: f32,
    /// Mean radius and the radii's lognormal log standard deviation.
    pub(crate) radius: f32,
    pub(crate) sigma: f32,
    /// Cell side; every grain lies, soft edge and all, within a cell of its own.
    pub(crate) cell: f32,
    pub(crate) max_radius: f32,
    /// Mean effective area of a grain.
    pub(crate) area: f32,
    /// The darkest tone the density is calibrated for.
    pub(crate) darkest: f32,
    pub(crate) seed: u32,
}
impl FilmGrainField {
    /// The mono grain.
    #[cfg(test)]
    pub(crate) fn new(g: &FilmGrain, edge: f32) -> Self {
        Self::layer(g, edge, 1., g.seed)
    }
    /// A layer whose grains are `size` times the mono grain's, with `seed`.
    fn layer(g: &FilmGrain, edge: f32, size: f32, seed: u32) -> Self {
        let radius = ((RADIUS_BASE + RADIUS_PER_SIZE * g.size) * size * edge / REFERENCE_EDGE)
            .max(RADIUS_MIN);
        let sigma = SIGMA_BASE + SIGMA_PER_ROUGHNESS * g.roughness;
        // Every grain, soft edge and all, reaches at most a cell from its centre.
        let max_radius = TAIL * radius;
        let cell = reach(max_radius);
        let mut field = Self {
            amount: g.amount,
            radius,
            sigma,
            cell,
            max_radius,
            area: 0.,
            darkest: TONE_LIMIT,
            seed,
        };
        // The mean over the normal `z` of the radius each `z` gives, by the midpoint
        // rule over ±5 standard deviations.
        let nodes = 80;
        let (mut area, mut weights) = (0., 0.);
        for i in 0..nodes {
            let z = -5. + (i as f32 + 0.5) * 10. / nodes as f32;
            let weight = (-0.5 * z * z).exp();
            area += weight * effective_area(field.grain_radius(z));
            weights += weight;
        }
        field.area = area / weights;
        let full = (CANDIDATES as f32 * field.area / (cell * cell)).max(f32::MIN_POSITIVE);
        // At most mid gray, whatever the grains: the calibrated tones stay a range.
        field.darkest = TONE_LIMIT.max((-MAX_PRESENCE * full).exp()).min(0.5);
        field
    }
    fn grain_radius(&self, z: f32) -> f32 {
        (self.radius * (self.sigma * z - 0.5 * self.sigma * self.sigma).exp()).min(self.max_radius)
    }
    /// Each candidate's presence threshold for tone `l`, already limited to the
    /// calibrated tones.
    fn presence(&self, l: f32) -> f32 {
        (-l.ln() * self.cell * self.cell / (CANDIDATES as f32 * self.area)).min(MAX_PRESENCE)
    }
    /// The share of full-resolution position `x`, `y` left uncovered at presence `p`.
    pub(crate) fn uncovered(&self, x: f32, y: f32, p: f32) -> f32 {
        let mut uncovered = 1.;
        for k in 0..CANDIDATES {
            // Each candidate's grid is offset by the R2 sequence, so a position's phase
            // within its cell differs between candidates and no cell pattern shows.
            let ox = (0.5 + k as f32 * R2[0]).fract() * self.cell;
            let oy = (0.5 + k as f32 * R2[1]).fract() * self.cell;
            let (lx, ly) = (x - ox, y - oy);
            let cx = (lx / self.cell).floor() as i32;
            let cy = (ly / self.cell).floor() as i32;
            let key = self.seed ^ k.wrapping_mul(CANDIDATE_STEP);
            for j in -1..=1 {
                for i in -1..=1 {
                    let (gx, gy) = (cx + i, cy + j);
                    let threshold = unit_hash(gx, gy, key ^ SALT_THRESHOLD);
                    if threshold - PRESENCE >= p {
                        continue;
                    }
                    let px = (gx as f32 + unit_hash(gx, gy, key ^ SALT_X)) * self.cell;
                    let py = (gy as f32 + unit_hash(gx, gy, key ^ SALT_Y)) * self.cell;
                    // Grains reach at most `cell` from their centre.
                    let d = (lx - px).hypot(ly - py);
                    if d >= self.cell {
                        continue;
                    }
                    let presence = smoothstep(threshold - PRESENCE, threshold + PRESENCE, p);
                    // Box–Muller.
                    let u = 1. - unit_hash(gx, gy, key ^ SALT_RADIUS);
                    let angle = unit_hash(gx, gy, key ^ SALT_ANGLE);
                    let z = (-2. * u.ln()).sqrt() * (2. * std::f32::consts::PI * angle).cos();
                    uncovered *= 1. - presence * coverage(self.grain_radius(z), d);
                }
            }
        }
        uncovered
    }
    /// What the grain adds at full-resolution position `x`, `y` for an output of
    /// `scale` pixels per full-resolution pixel over encoded luminance `l`.
    pub(crate) fn delta(&self, x: f32, y: f32, scale: f32, l: f32) -> f32 {
        if self.amount == 0. {
            return 0.;
        }
        let fade = (l / self.darkest).min((1. - l) / TONE_LIMIT).clamp(0., 1.);
        if fade == 0. {
            return 0.;
        }
        let tone = l.clamp(self.darkest, 1. - TONE_LIMIT);
        let p = self.presence(tone);
        let (n, attenuation) = if scale >= 1. {
            (1, 1.)
        } else {
            // Down to half size, 2×2 samples: exactly the full-resolution pixels under
            // the preview pixel. Smaller, one sample: further apart than a grain, it
            // averages fewer grains than the pixel holds, so what is left is scaled
            // down. A grain, drawn at least `BLUR` wide with its soft edge, stays
            // correlated over about `1.5·max(radius, BLUR) + 0.4` pixels (fitted to the
            // resized full render at Size 0 to 1).
            let n = if scale >= 0.5 { 2 } else { 1 };
            let width = 1.5 * self.radius.max(BLUR) + 0.4;
            (n, (n as f32 * scale * width).min(1.))
        };
        let mut sum = 0.;
        for j in 0..n {
            for i in 0..n {
                let ox = ((i as f32 + 0.5) / n as f32 - 0.5) / scale.min(1.);
                let oy = ((j as f32 + 0.5) / n as f32 - 0.5) / scale.min(1.);
                sum += self.uncovered(x + ox, y + oy, p);
            }
        }
        let grain = sum / (n * n) as f32;
        STRENGTH * self.amount * fade * (grain - tone) * attenuation
    }
}

#[cfg(test)]
mod tests;
