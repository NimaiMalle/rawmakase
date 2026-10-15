//! The learned Shadows and Highlights operator of the scene tone stage: a small network
//! per slider position, fitted to Camera Raw on training photos
//! (`scripts/corpus/scene-tone-net.py`, which documents the features and writes
//! `local_tone_net_data.rs`). Its inputs are the pixel's guided base level B
//! (local_tone.rs), four coarser levels of the measurement copy at the pixel, and global
//! statistics of the copy's blurred histograms; its output is the log2 luminance gain.
//!
//! Evaluating the network per pixel would be slow on the CPU, so it is evaluated on a
//! grid of cells of the measurement copy ([`CELL`] map pixels each), at [`POINTS`] base
//! levels around the cell's own level: a tone curve per cell. A pixel reads the curve of
//! its four nearest cells at its own base level, bilinearly, as the map's other grids
//! are read. Every curve is projected so that base + gain does not decrease (pooled
//! adjacent violators), the same guarantee the measured tables have. CPU, masks and the
//! GPU read the same grids.
use super::local_tone::blur;
use super::local_tone_net_data::{COARSE, GLOBAL_RADII, INPUTS, NETS, Net, PERCENTILES};
use rayon::prelude::*;

/// Map pixels per cell edge.
pub(crate) const CELL: usize = 4;
/// Curve points per cell: base offsets of −6 to +6 stops in steps of a stop.
pub(crate) const POINTS: usize = 13;
const OFFSET0: f32 = -6.;
/// The slider positions curves are built at when masks move a slider, with 0 the identity.
pub(crate) const POSITIONS: [f32; 7] = [-1., -0.5, -0.25, 0., 0.25, 0.5, 1.];

/// Three passes of a box blur: a near-Gaussian of sigma about `r`.
fn blur3(x: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let a = blur(x, w, h, r);
    let a = blur(&a, w, h, r);
    blur(&a, w, h, r)
}
/// The sorted value at index floor((n − 1) q), as the fitting script reads it.
fn percentile(v: &mut [f32], q: f32) -> f32 {
    let k = ((v.len() - 1) as f32 * q) as usize;
    v.select_nth_unstable_by(k, f32::total_cmp);
    v[k]
}
/// Global statistics of the map: for each blurred copy the percentiles, the maximum and
/// the minimum; the log2 mean luminance; the 99.9th percentile of the level.
pub(crate) fn globals(logs: &[f32], lum: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut g = Vec::with_capacity(2 * (PERCENTILES.len() + 2) + 2);
    for r in GLOBAL_RADII {
        let mut b = blur3(logs, w, h, r);
        for q in PERCENTILES {
            g.push(percentile(&mut b, q));
        }
        g.push(b.iter().copied().fold(f32::MIN, f32::max));
        g.push(b.iter().copied().fold(f32::MAX, f32::min));
    }
    let mean = lum.iter().map(|y| *y as f64).sum::<f64>() / lum.len() as f64;
    g.push((mean as f32).max(super::local_tone::FLOOR).log2());
    let mut l = logs.to_vec();
    g.push(percentile(&mut l, 0.999));
    g
}
/// The network's inputs for base level `b`, the coarse levels at the pixel and the globals.
fn inputs(b: f32, coarse: &[f32], glob: &[f32], out: &mut [f32]) {
    out[0] = b;
    for (o, c) in out[1..1 + coarse.len()].iter_mut().zip(coarse) {
        *o = c - b;
    }
    let n = 1 + coarse.len();
    out[n..n + glob.len()].copy_from_slice(glob);
    for (o, g) in out[n + glob.len()..].iter_mut().zip(glob) {
        *o = b - g;
    }
}
/// The network's log2 gain for `x`.
fn forward(net: &Net, x: &[f32]) -> f32 {
    let mut h: Vec<f32> = x.to_vec();
    let last = net.weights.len() - 1;
    for (i, (w, b)) in net.weights.iter().zip(net.biases).enumerate() {
        let n_in = h.len();
        let mut next: Vec<f32> = b
            .iter()
            .enumerate()
            .map(|(j, b)| {
                let row = &w[j * n_in..(j + 1) * n_in];
                b + row.iter().zip(&h).map(|(w, x)| w * x).sum::<f32>()
            })
            .collect();
        if i < last {
            for v in &mut next {
                *v /= 1. + (-*v).exp();
            }
        }
        h = next;
    }
    h[0]
}
/// The two measured networks around slider value `s` of `control` and the weight of
/// the second (`None` for the identity side).
fn bracket(control: char, s: f32) -> ((Option<&'static Net>, Option<&'static Net>), f32) {
    let s = s.clamp(-1., 1.);
    let mut points: Vec<(f32, Option<&Net>)> = NETS
        .iter()
        .filter(|n| n.control == control)
        .map(|n| (n.value, Some(n)))
        .collect();
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    points.insert(points.len() / 2, (0., None));
    let j = points
        .windows(2)
        .position(|w| s <= w[1].0)
        .unwrap_or(points.len() - 2);
    let ((s0, n0), (s1, n1)) = (points[j], points[j + 1]);
    ((n0, n1), (s - s0) / (s1 - s0))
}
/// What the operator reads of the measurement copy: the cell grid's levels and the globals.
pub(crate) struct Cells {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// The guided base level at each cell (the mean of the map's over the cell).
    pub(crate) center: Vec<f32>,
    /// The coarse levels at each cell, `COARSE.len()` per cell.
    coarse: Vec<f32>,
    glob: Vec<f32>,
}
impl Cells {
    /// From the map's log2 levels, luminance and guided-filter coefficients (`a`, `b`).
    pub(crate) fn new(logs: &[f32], lum: &[f32], a: &[f32], b: &[f32], w: usize, h: usize) -> Self {
        let (cw, ch) = (w.div_ceil(CELL), h.div_ceil(CELL));
        let coarse_maps: Vec<Vec<f32>> = COARSE.par_iter().map(|r| blur3(logs, w, h, *r)).collect();
        let base: Vec<f32> = logs
            .iter()
            .zip(a.iter().zip(b))
            .map(|(l, (a, b))| a * l + b)
            .collect();
        let cell_mean = |v: &[f32], cx: usize, cy: usize| {
            let (x0, y0) = (cx * CELL, cy * CELL);
            let (x1, y1) = ((x0 + CELL).min(w), (y0 + CELL).min(h));
            let mut s = 0.;
            for y in y0..y1 {
                for x in x0..x1 {
                    s += v[y * w + x];
                }
            }
            s / ((x1 - x0) * (y1 - y0)) as f32
        };
        let mut center = Vec::with_capacity(cw * ch);
        let mut coarse = Vec::with_capacity(cw * ch * COARSE.len());
        for cy in 0..ch {
            for cx in 0..cw {
                center.push(cell_mean(&base, cx, cy));
                for m in &coarse_maps {
                    coarse.push(cell_mean(m, cx, cy));
                }
            }
        }
        Self {
            width: cw,
            height: ch,
            center,
            coarse,
            glob: globals(logs, lum, w, h),
        }
    }
    /// The curves of `control` at slider value `s` on this grid: `POINTS` log2 gains per
    /// cell, at base levels `center + OFFSET0 + k`; `None` at 0.
    pub(crate) fn curves(&self, control: char, s: f32) -> Option<Vec<f32>> {
        if s == 0. {
            return None;
        }
        let ((n0, n1), t) = bracket(control, s);
        let nc = COARSE.len();
        let mut out: Vec<f32> = (0..self.width * self.height)
            .into_par_iter()
            .flat_map_iter(|i| {
                let mut x = vec![0.; INPUTS];
                let coarse = &self.coarse[i * nc..(i + 1) * nc];
                let mut curve = [0.; POINTS];
                for (k, g) in curve.iter_mut().enumerate() {
                    let b = self.center[i] + OFFSET0 + k as f32;
                    inputs(b, coarse, &self.glob, &mut x);
                    let g0 = n0.map_or(0., |n| forward(n, &x));
                    let g1 = n1.map_or(0., |n| forward(n, &x));
                    *g = g0 + (g1 - g0) * t;
                }
                monotone(&mut curve);
                curve
            })
            .collect();
        out.shrink_to_fit();
        Some(out)
    }
}
/// Projects base + gain onto nondecreasing values along a cell's curve (pooled adjacent
/// violators on the outputs; the inputs step by one stop).
fn monotone(gain: &mut [f32; POINTS]) {
    let mut y: Vec<f32> = gain.iter().enumerate().map(|(k, g)| k as f32 + g).collect();
    // Blocks of (sum, count).
    let mut blocks: Vec<(f32, usize)> = Vec::with_capacity(POINTS);
    for v in &y {
        blocks.push((*v, 1));
        while blocks.len() > 1 {
            let n = blocks.len();
            let (s1, c1) = blocks[n - 1];
            let (s0, c0) = blocks[n - 2];
            if s0 / c0 as f32 <= s1 / c1 as f32 {
                break;
            }
            blocks.truncate(n - 2);
            blocks.push((s0 + s1, c0 + c1));
        }
    }
    let mut k = 0;
    for (s, c) in blocks {
        for _ in 0..c {
            y[k] = s / c as f32;
            k += 1;
        }
    }
    for (k, g) in gain.iter_mut().enumerate() {
        *g = y[k] - k as f32;
    }
}
/// A cell's curve read at base level `base`, linearly between points and held at the ends.
pub(crate) fn read(curve: &[f32], center: f32, base: f32) -> f32 {
    let f = (base - center - OFFSET0).clamp(0., (POINTS - 1) as f32);
    let i = (f as usize).min(POINTS - 2);
    curve[i] + (curve[i + 1] - curve[i]) * (f - i as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotone_projection_keeps_ordered_curves_and_flattens_reversals() {
        let mut c = [0.5; POINTS];
        monotone(&mut c);
        assert!(c.iter().all(|v| (v - 0.5).abs() < 1e-6));
        // A gain that falls by two stops over one point reverses base + gain.
        let mut c = [0.; POINTS];
        c[6] = 1.5;
        c[7] = -0.5;
        monotone(&mut c);
        for k in 1..POINTS {
            assert!(k as f32 + c[k] >= (k - 1) as f32 + c[k - 1] - 1e-6, "{c:?}");
        }
    }

    #[test]
    fn curves_read_linearly_and_hold_at_the_ends() {
        let curve: Vec<f32> = (0..POINTS).map(|k| k as f32 * 0.1).collect();
        assert!((read(&curve, 0., -6.) - 0.).abs() < 1e-6);
        assert!((read(&curve, 0., -5.5) - 0.05).abs() < 1e-6);
        assert!((read(&curve, 0., 9.) - 1.2).abs() < 1e-6);
        assert!((read(&curve, 2., 2.) - 0.6).abs() < 1e-6);
    }

    /// The networks reproduce the fitting script's gains on its fixture scene.
    #[test]
    fn networks_match_the_fixture() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/corpus/camera-raw/local-tone-net.json"
        ))
        .unwrap();
        let f: serde_json::Value = serde_json::from_str(&text).unwrap();
        let gains = f["gains"].as_object().unwrap();
        for net in NETS.iter() {
            let name = format!("{}{:+}", net.control, (net.value * 100.).round() as i32);
            let expected = gains[&name].as_array().unwrap();
            for (p, e) in f["pixels"].as_array().unwrap().iter().zip(expected) {
                let x: Vec<f32> = p["inputs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap() as f32)
                    .collect();
                assert_eq!(x.len(), INPUTS);
                let g = forward(net, &x);
                assert!(
                    (g - e.as_f64().unwrap() as f32).abs() < 2e-3,
                    "{name}: {g} vs {e}"
                );
            }
        }
    }
}
