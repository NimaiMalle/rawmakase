//! X-Trans demosaicing by directional candidates.
//!
//! X-Trans has no regular two-pixel greens, so one isotropic estimate turns fine
//! periodic detail (louvres, grilles) into colour moiré. Instead every direction
//! (horizontal, vertical and both diagonals) gets its own full RGB candidate, an
//! isotropic one joins them for soft detail where no direction is better, and
//! one-sided horizontal and vertical ones (left, right, up, down) cover a pixel next
//! to a thin line across fine detail, such as a mullion across louvres, where every
//! line through the pixel crosses one or the other:
//!
//! 1. Green at red/blue sites by interpolating along the direction between the
//!    nearest greens, corrected by the same-colour curvature along that line (a
//!    one-sided candidate takes the nearest green on its side).
//! 2. Red and blue from the difference to that direction's green, as on Bayer (the
//!    isotropic candidate uses its gradient-weighted greens from all 8 neighbours).
//! 3. Per pixel, the candidates are blended by how smooth their colour difference
//!    (R − G, B − G) is around it, along the smoothest line through each pixel. A
//!    correct direction leaves neutral detail neutral and a real colour edge smooth
//!    along itself; a wrong one leaves a colour pattern, which scores badly. A
//!    one-sided candidate must be clearly smoother than the others to win, as one
//!    side alone also flattens a real colour fringe along an edge.
//! 4. Where a thin line crosses fine detail, the smoothness alone often picks a
//!    candidate whose colour is wrong in a streak along the line. So the blend is made
//!    twice: the second time, each candidate is also scored by how far its colour
//!    difference is from the median of the first blend's around the pixel, beyond a
//!    tolerance that noise stays within. A streak or dot of false colour is not the
//!    median, while a real colour area or edge is, so real colour is kept.
//!
//! The image is processed in row bands with a margin so every window fits. The
//! margin repeats the image by whole periods of the colour pattern, so the pattern
//! continues across it and where the nearest sample of each colour lies is the same
//! for every pixel at the same place in the pattern: it is looked up, not searched.
use super::{Cfa, PATTERN};
use rayon::prelude::*;

/// Rows of output per band.
const BAND: usize = 64;
/// Margin around a band: green line search 4, colour difference line search 4,
/// the variation measure and its 3×3 window, one each, then the 5×5 median, 2.
const MARGIN: usize = 12;
/// Distance from the band edge at which a candidate's RGB is valid.
const CANDIDATE_EDGE: usize = 2 * REACH as usize;
/// How far along a line to look for a green or same-colour sample.
const REACH: isize = 4;
/// The eight ways along the four lines as (dx, dy), each next to its opposite.
const WAYS: [(isize, isize); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (-1, -1),
    (1, -1),
    (-1, 1),
];
/// Horizontal, vertical and the two diagonals, as one of their ways in `WAYS`.
const DIRECTIONS: [usize; 4] = [0, 2, 4, 6];
/// Left, right, down and up, which get one-sided candidates too.
const SIDES: [usize; 4] = [0, 1, 2, 3];
/// The directional candidates, the isotropic one and the one-sided ones.
const CANDIDATES: usize = DIRECTIONS.len() + 1 + SIDES.len();
/// How much smoother a one-sided candidate must be to weigh as much as the others:
/// one side alone also flattens a real colour fringe along an edge (X-H2), so it
/// should win only where every line through the pixel crosses detail.
const SIDE_PENALTY: f32 = 2.;
/// Radius of the window whose median colour difference the second blend prefers.
const MEDIAN_RADIUS: usize = 2;
/// Weight of the distance from that median against the roughness: large, so that it
/// decides wherever the roughness leaves a choice between candidates.
const MEDIAN_WEIGHT: f32 = 64.;
/// Distance from the median colour difference, in white-normalised units, that
/// costs nothing: noise stays averaged over the candidates as in the first blend
/// (without it a high-ISO photo's noise rose by a tenth), while false colour along
/// a thin line, several times larger, still decides.
const MEDIAN_TOLERANCE: f32 = 0.03;
/// Floor of the variation measure, in white-normalised units, so that noise on a
/// flat area does not pick a direction.
const VARIATION_FLOOR: f32 = 1e-3;
/// Sharpness of the blend: a candidate twice as rough as the smoothest weighs 1/16.
const BLEND_POWER: i32 = 4;
/// Weight of the Hamilton–Adams-style curvature correction of the directional green.
const CURVATURE: f32 = 0.2;

pub(crate) fn demosaic(cfa: &Cfa) -> Vec<[f32; 3]> {
    let w = cfa.width;
    let period = period(cfa.pattern);
    if w < period || cfa.height < period {
        return too_small(cfa);
    }
    let layout = Layout::new(cfa, period, w + 2 * MARGIN);
    let mut out = vec![[0f32; 3]; w * cfa.height];
    out.par_chunks_mut(w * BAND)
        .enumerate()
        .for_each(|(band, rows)| band_candidates(cfa, &layout, band * BAND, rows));
    out
}

/// The smallest period, dividing the pattern's side, the colour pattern repeats with
/// in both directions (6 on X-Trans).
fn period(pattern: &[u8; PATTERN * PATTERN]) -> usize {
    (1..=PATTERN)
        .filter(|p| PATTERN.is_multiple_of(*p))
        .find(|&p| {
            (0..PATTERN).all(|y| {
                (0..PATTERN).all(|x| pattern[y * PATTERN + x] == pattern[(y % p) * PATTERN + x % p])
            })
        })
        .unwrap_or(PATTERN)
}

/// An image smaller than a period of the pattern, which has no neighbourhood to
/// interpolate from: each colour is its mean over the image (0 where it is missing).
fn too_small(cfa: &Cfa) -> Vec<[f32; 3]> {
    let (mut sum, mut count) = ([0f32; 3], [0usize; 3]);
    for y in 0..cfa.height {
        for x in 0..cfa.width {
            let c = usize::from(cfa.color(x, y));
            sum[c] += cfa.data[y * cfa.width + x];
            count[c] += 1;
        }
    }
    let mean: [f32; 3] = std::array::from_fn(|c| {
        if count[c] > 0 {
            sum[c] / count[c] as f32
        } else {
            0.
        }
    });
    vec![mean; cfa.width * cfa.height]
}

/// Where the nearest samples of each colour lie from each place in the pattern, in a
/// band of a given width.
struct Layout {
    period: usize,
    /// Band offsets of `WAYS`.
    steps: [isize; 8],
    /// Per place, way and colour: how many steps away the nearest sample of the
    /// colour is, up to `REACH` (0 for none).
    near: Vec<[[u8; 3]; 8]>,
    /// Per place and colour: band offsets and inverse distances of the samples of the
    /// colour at distance 1, else 2, for when a line has none.
    around: Vec<[Vec<(isize, f32)>; 3]>,
}

impl Layout {
    fn new(cfa: &Cfa, period: usize, width: usize) -> Self {
        let steps = WAYS.map(|(dx, dy)| dy * width as isize + dx);
        let colour = |x: isize, y: isize| {
            let p = period as isize;
            cfa.color(x.rem_euclid(p) as usize, y.rem_euclid(p) as usize)
        };
        let places = (0..period * period).map(|k| ((k % period) as isize, (k / period) as isize));
        let near = places
            .clone()
            .map(|(x, y)| {
                WAYS.map(|(dx, dy)| {
                    std::array::from_fn(|c| {
                        (1..=REACH)
                            .find(|k| usize::from(colour(x + k * dx, y + k * dy)) == c)
                            .unwrap_or(0) as u8
                    })
                })
            })
            .collect();
        let around = places
            .map(|(x, y)| {
                std::array::from_fn(|c| {
                    [&RING_1[..], &RING_2[..]]
                        .into_iter()
                        .map(|ring| {
                            ring.iter()
                                .filter(|(dx, dy, _)| usize::from(colour(x + dx, y + dy)) == c)
                                .map(|&(dx, dy, weight)| (dy * width as isize + dx, weight))
                                .collect::<Vec<_>>()
                        })
                        .find(|samples| !samples.is_empty())
                        .unwrap_or_default()
                })
            })
            .collect();
        Self {
            period,
            steps,
            near,
            around,
        }
    }
}

/// The pixels at Chebyshev distance `R` as (dx, dy, inverse distance).
const fn ring<const R: isize, const N: usize>() -> [(isize, isize, f32); N] {
    let mut out = [(0, 0, 0.); N];
    let mut n = 0;
    let mut dy = -R;
    while dy <= R {
        let mut dx = -R;
        while dx <= R {
            if dx.abs() == R || dy.abs() == R {
                // 1 / √(dx² + dy²); `sqrt` is not const.
                let weight = match dx * dx + dy * dy {
                    1 => 1.,
                    2 => std::f32::consts::FRAC_1_SQRT_2,
                    4 => 0.5,
                    5 => 0.447_213_6,
                    _ => 0.353_553_38,
                };
                out[n] = (dx, dy, weight);
                n += 1;
            }
            dx += 1;
        }
        dy += 1;
    }
    out
}
const RING_1: [(isize, isize, f32); 8] = ring::<1, 8>();
const RING_2: [(isize, isize, f32); 16] = ring::<2, 16>();

/// A band of sensor samples with its margin.
struct Band<'a> {
    width: usize,
    height: usize,
    raw: Vec<f32>,
    colour: Vec<u8>,
    /// Each pixel's place in the pattern, as an index into the layout's tables.
    place: Vec<u16>,
    layout: &'a Layout,
}

impl<'a> Band<'a> {
    fn new(cfa: &Cfa, layout: &'a Layout, y0: usize, rows: usize) -> Self {
        let width = cfa.width + 2 * MARGIN;
        let height = rows + 2 * MARGIN;
        let p = layout.period as isize;
        // Outside the image, the pixel a whole number of periods inside it, which has
        // the colour the pattern has there.
        let inside = |v: isize, n: usize| {
            let n = n as isize;
            let v = if v < 0 {
                v + (-v + p - 1) / p * p
            } else if v >= n {
                v - (v - n + p) / p * p
            } else {
                v
            };
            v as usize
        };
        let mut raw = Vec::with_capacity(width * height);
        let mut colour = Vec::with_capacity(width * height);
        let mut place = Vec::with_capacity(width * height);
        for py in 0..height {
            let vy = y0 as isize + py as isize - MARGIN as isize;
            let gy = inside(vy, cfa.height);
            for px in 0..width {
                let vx = px as isize - MARGIN as isize;
                let gx = inside(vx, cfa.width);
                raw.push(cfa.data[gy * cfa.width + gx]);
                colour.push(cfa.color(gx, gy));
                place.push((vy.rem_euclid(p) * p + vx.rem_euclid(p)) as u16);
            }
        }
        Self {
            width,
            height,
            raw,
            colour,
            place,
            layout,
        }
    }

    /// Nearest sample of `colour` the `way` from `i`, as (distance, band index).
    #[inline]
    fn along(&self, i: usize, way: usize, colour: u8) -> Option<(f32, usize)> {
        let k = self.layout.near[usize::from(self.place[i])][way][usize::from(colour)];
        (k > 0).then(|| {
            let j = i as isize + isize::from(k) * self.layout.steps[way];
            (f32::from(k), j as usize)
        })
    }

    /// Green at a red or blue site from the 8 neighbours, each pair weighted by how
    /// well the greens on its two sides agree (no preferred direction).
    fn green_isotropic(&self, i: usize) -> f32 {
        let pw = self.width as isize;
        let contrast = {
            let (mut lo, mut hi) = (f32::INFINITY, 0f32);
            for o in [-pw - 1, -pw, -pw + 1, -1, 1, pw - 1, pw, pw + 1] {
                let j = (i as isize + o) as usize;
                if self.colour[j] == 1 {
                    lo = lo.min(self.raw[j]);
                    hi = hi.max(self.raw[j]);
                }
            }
            if hi >= lo { hi - lo } else { 0. }
        };
        let (mut sum, mut total) = (0., 0.);
        for (step, diag) in [(1, 1.), (pw, 1.), (pw + 1, 0.7), (pw - 1, 0.7)] {
            let a = (i as isize + step) as usize;
            let b = (i as isize - step) as usize;
            let (ga, gb) = (self.colour[a] == 1, self.colour[b] == 1);
            match (ga, gb) {
                (true, true) => {
                    let weight = diag / ((self.raw[a] - self.raw[b]).abs() + 1e-4);
                    sum += (self.raw[a] + self.raw[b]) * 0.5 * weight;
                    total += weight;
                }
                (true, false) | (false, true) => {
                    let v = if ga { self.raw[a] } else { self.raw[b] };
                    let weight = diag * 0.5 / (contrast + 1e-4);
                    sum += v * weight;
                    total += weight;
                }
                _ => {}
            }
        }
        if total > 0. { sum / total } else { self.raw[i] }
    }

    /// Green at `i` along `line`.
    fn green(&self, i: usize, line: Line) -> f32 {
        let own = self.colour[i];
        if own == 1 {
            return self.raw[i];
        }
        let way = match line {
            Line::Isotropic => return self.green_isotropic(i),
            Line::Side(way) => {
                return match self.along(i, way, 1) {
                    Some((_, j)) => self.raw[j],
                    None => self.green_isotropic(i),
                };
            }
            Line::Both(way) => way,
        };
        let c = self.raw[i];
        let (fwd, back) = (self.along(i, way, 1), self.along(i, way ^ 1, 1));
        let (mut est, lo, hi) = match (fwd, back) {
            (Some((a, ja)), Some((b, jb))) => {
                let (ga, gb) = (self.raw[ja], self.raw[jb]);
                ((ga * b + gb * a) / (a + b), ga.min(gb), ga.max(gb))
            }
            (Some((_, j)), None) | (None, Some((_, j))) => {
                let g = self.raw[j];
                (g, g, g)
            }
            (None, None) => return c,
        };
        // Same-colour curvature along the line, as the Hamilton–Adams correction.
        if let (Some((a, ja)), Some((b, jb))) =
            (self.along(i, way, own), self.along(i, way ^ 1, own))
        {
            est += CURVATURE * (c - (self.raw[ja] * b + self.raw[jb] * a) / (a + b));
        }
        est.clamp(lo, hi).max(0.)
    }
}

/// The pixels a candidate interpolates from.
#[derive(Clone, Copy)]
enum Line {
    /// Both ways along a line, starting with one of `WAYS`.
    Both(usize),
    /// One of `WAYS`.
    Side(usize),
    /// All eight neighbours.
    Isotropic,
}

/// A candidate's RGB and its roughness.
type Candidate = (Vec<[f32; 3]>, Vec<f32>);

/// Demosaics one band into `out` (whole rows starting at image row `y0`).
fn band_candidates(cfa: &Cfa, layout: &Layout, y0: usize, out: &mut [[f32; 3]]) {
    let w = cfa.width;
    let rows = out.len() / w;
    let band = Band::new(cfa, layout, y0, rows);
    let (pw, ph) = (band.width, band.height);
    let lines = DIRECTIONS
        .map(Line::Both)
        .into_iter()
        .chain([Line::Isotropic])
        .chain(SIDES.map(Line::Side));
    let candidates: Vec<Candidate> = lines
        .map(|line| {
            let rgb = candidate(&band, line);
            let roughness = roughness(&rgb, pw, ph);
            (rgb, roughness)
        })
        .collect();
    let roughness = |i: usize| -> [f32; CANDIDATES] {
        std::array::from_fn(|k| {
            let side = k > DIRECTIONS.len();
            candidates[k].1[i] * if side { SIDE_PENALTY } else { 1. }
        })
    };
    let chroma = |p: [f32; 3]| [p[0] - p[1], p[2] - p[1]];
    // The first blend's colour difference, as far out as the median reaches.
    let first_edge = MARGIN - MEDIAN_RADIUS;
    let mut first = vec![[0f32; 2]; pw * ph];
    for y in first_edge..ph - first_edge {
        for x in first_edge..pw - first_edge {
            let i = y * pw + x;
            first[i] = chroma(blend(&candidates, i, roughness(i)));
        }
    }
    // Their medians along the rows, then down the columns: a separable median, much
    // cheaper than the 5×5 one and as robust to a streak or a dot.
    let mut along_rows = vec![[0f32; 2]; pw * ph];
    for y in first_edge..ph - first_edge {
        for x in MARGIN..pw - MARGIN {
            let i = y * pw + x;
            along_rows[i] = std::array::from_fn(|c| {
                median5(std::array::from_fn(|k| first[i + k - MEDIAN_RADIUS][c]))
            });
        }
    }
    for (r, row) in out.chunks_mut(w).enumerate() {
        for (x, px) in row.iter_mut().enumerate() {
            let i = (r + MARGIN) * pw + x + MARGIN;
            let median: [f32; 2] = std::array::from_fn(|c| {
                median5(std::array::from_fn(|k| {
                    along_rows[i + k * pw - MEDIAN_RADIUS * pw][c]
                }))
            });
            let mut scores = roughness(i);
            for (score, (rgb, _)) in scores.iter_mut().zip(&candidates) {
                let c = chroma(rgb[i]);
                let distance = (c[0] - median[0]).abs() + (c[1] - median[1]).abs();
                *score += MEDIAN_WEIGHT * (distance - MEDIAN_TOLERANCE).max(0.);
            }
            *px = blend(&candidates, i, scores);
        }
    }
}

/// The candidates at `i` weighted by their scores (lower is better).
#[inline]
fn blend(candidates: &[Candidate], i: usize, scores: [f32; CANDIDATES]) -> [f32; 3] {
    // Relative to the best candidate, so it always weighs 1.
    let best = scores.iter().fold(f32::INFINITY, |a, b| a.min(*b));
    let mut sum = [0f32; 3];
    let mut total = 0.;
    for ((rgb, _), score) in candidates.iter().zip(scores) {
        let weight = (best / score).powi(BLEND_POWER);
        for c in 0..3 {
            sum[c] += rgb[i][c] * weight;
        }
        total += weight;
    }
    sum.map(|v| v / total)
}

/// The median of five values.
#[inline]
fn median5(mut v: [f32; 5]) -> f32 {
    let mut order = |a: usize, b: usize| {
        if v[a] > v[b] {
            v.swap(a, b);
        }
    };
    // The lowest and highest of the first four can't be the median; of what is left,
    // the median of three.
    order(0, 1);
    order(2, 3);
    order(0, 2);
    order(1, 3);
    // v[0] is the lowest of four and v[3] the highest; the median is the middle of
    // v[1], v[2] and v[4].
    order(1, 2);
    order(2, 4);
    order(1, 2);
    v[2]
}

/// Full RGB along one line, valid at distance `CANDIDATE_EDGE` from the band edge.
fn candidate(band: &Band, line: Line) -> Vec<[f32; 3]> {
    let (pw, ph) = (band.width, band.height);
    let mut green = vec![0f32; pw * ph];
    for y in REACH as usize..ph - REACH as usize {
        for x in REACH as usize..pw - REACH as usize {
            let i = y * pw + x;
            green[i] = band.green(i, line);
        }
    }
    let mut rgb = vec![[0f32; 3]; pw * ph];
    for y in CANDIDATE_EDGE..ph - CANDIDATE_EDGE {
        for x in CANDIDATE_EDGE..pw - CANDIDATE_EDGE {
            let i = y * pw + x;
            let own = band.colour[i];
            rgb[i][1] = green[i];
            for c in [0u8, 2] {
                if own == c {
                    rgb[i][c as usize] = band.raw[i];
                    continue;
                }
                // Along the line, between the nearest samples of the colour on both
                // sides (or from the nearest on its side), interpolating their
                // difference to green.
                let diff = |j: usize| band.raw[j] - green[j];
                match line {
                    Line::Both(way) => {
                        if let (Some((a, ja)), Some((b, jb))) =
                            (band.along(i, way, c), band.along(i, way ^ 1, c))
                        {
                            rgb[i][c as usize] =
                                (green[i] + (diff(ja) * b + diff(jb) * a) / (a + b)).max(0.);
                            continue;
                        }
                    }
                    Line::Side(way) => {
                        if let Some((_, j)) = band.along(i, way, c) {
                            rgb[i][c as usize] = (green[i] + diff(j)).max(0.);
                            continue;
                        }
                    }
                    Line::Isotropic => {}
                }
                // Otherwise the nearest samples within radius 1, else 2, weighted by
                // inverse distance.
                let around = &band.layout.around[usize::from(band.place[i])][usize::from(c)];
                let (mut sum, mut total) = (0., 0.);
                for &(o, weight) in around {
                    sum += diff((i as isize + o) as usize) * weight;
                    total += weight;
                }
                rgb[i][c as usize] = (green[i] + if total > 0. { sum / total } else { 0. }).max(0.);
            }
        }
    }
    rgb
}

/// How much R − G and B − G change through each pixel along its smoothest line,
/// summed over the 3×3 around it (plus the floor), valid at distance `CANDIDATE_EDGE + 2` from the band edge.
fn roughness(rgb: &[[f32; 3]], pw: usize, ph: usize) -> Vec<f32> {
    let chroma = |i: usize| [rgb[i][0] - rgb[i][1], rgb[i][2] - rgb[i][1]];
    let mut variation = vec![0f32; pw * ph];
    for y in CANDIDATE_EDGE + 1..ph - (CANDIDATE_EDGE + 1) {
        for x in CANDIDATE_EDGE + 1..pw - (CANDIDATE_EDGE + 1) {
            let i = y * pw + x;
            let across = |o: usize| {
                let (a, b) = (chroma(i - o), chroma(i + o));
                (a[0] - b[0]).abs() + (a[1] - b[1]).abs()
            };
            // Along the smoothest of the four lines: a real colour edge or fringe is
            // smooth along itself, the false colour of a wrong candidate is not
            // smooth along any line. (Summing horizontal and vertical changes made
            // flattening a real fringe the best score.)
            variation[i] = 2.
                * [across(1), across(pw), across(pw + 1), across(pw - 1)]
                    .into_iter()
                    .fold(f32::INFINITY, f32::min);
        }
    }
    // The 3×3 sum, along the rows and then down the columns.
    let edge = CANDIDATE_EDGE + 2;
    let mut rows = vec![0f32; pw * ph];
    for y in edge - 1..ph - (edge - 1) {
        for x in edge..pw - edge {
            let i = y * pw + x;
            rows[i] = variation[i - 1] + variation[i] + variation[i + 1];
        }
    }
    let mut out = vec![0f32; pw * ph];
    for y in edge..ph - edge {
        for x in edge..pw - edge {
            let i = y * pw + x;
            out[i] = rows[i - pw] + rows[i] + rows[i + pw] + VARIATION_FLOOR;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median5_is_the_median() {
        // Every order of five distinct values, and some ties.
        let mut v = [0f32, 1., 2., 3., 4.];
        let mut orders = vec![v];
        // Heap's algorithm.
        let mut c = [0usize; 5];
        let mut i = 0;
        while i < 5 {
            if c[i] < i {
                v.swap(if i % 2 == 0 { 0 } else { c[i] }, i);
                orders.push(v);
                c[i] += 1;
                i = 0;
            } else {
                c[i] = 0;
                i += 1;
            }
        }
        assert_eq!(orders.len(), 120);
        for o in orders {
            assert_eq!(median5(o), 2., "{o:?}");
        }
        assert_eq!(median5([1., 1., 0., 5., 1.]), 1.);
        assert_eq!(median5([3., 0., 0., 3., 3.]), 3.);
    }
}
