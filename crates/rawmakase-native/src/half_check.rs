//! The half-size checks: a photo's half-size decode against its full decode
//! reduced to the same size, and its time against LibRaw's half-size path,
//! for `rawmakase half-check` and `scripts/corpus/dataset-half.py`.
//!
//! - Colour: the CIEDE2000 difference of blocks of [`BLOCK`] × [`BLOCK`]
//!   half-size pixels (each block's mean colour, through the photo's camera
//!   to sRGB matrix to CIELAB), mean at most [`MAX_MEAN`] and 95th
//!   percentile at most [`MAX_P95`].
//! - Detail: the energy of the luminance Laplacian, half-size over the full
//!   decode reduced, within [`DETAIL`].
//! - Time: opening and decoding at half size at most [`MAX_TIME`] times
//!   LibRaw's (when measured), the faster of two runs each.
//!
//! The full decode is reduced as the half-size decode is laid out: half-size
//! pixel (i, j) is the mean of the full decode's 2 × 2 block at (2i, 2j),
//! reaching one pixel back where an odd side leaves one row or column.
use crate::camera_data::{CameraImage, Decode, Demosaic};
use crate::raw::Decoder;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// Side of the colour check's blocks, in half-size pixels.
pub const BLOCK: usize = 16;
pub const MAX_MEAN: f64 = 0.3;
pub const MAX_P95: f64 = 1.;
pub const DETAIL: std::ops::RangeInclusive<f64> = 0.8..=1.25;
pub const MAX_TIME: f64 = 1.1;

/// One photo's half-size check.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct HalfCheck {
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Full and half-size width and height.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full: Option<[u32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub half: Option<[u32; 2]>,
    /// Whether the half-size decode is the full decode's halves (⌈W/2⌉ ×
    /// ⌈H/2⌉); small RAWs develop at full size in both modes and are judged
    /// on their time only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub halved: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub de_mean: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub de_p95: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<f64>,
    /// Seconds to open and decode at half size, RAWmakase's and LibRaw's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub libraw_seconds: Option<f64>,
}

impl HalfCheck {
    /// The gates this check fails, by name; empty when it passes.
    pub fn failures(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.error.is_some() {
            out.push("error");
        }
        // Every file is timed, small RAWs too.
        let slow = matches!((self.seconds, self.libraw_seconds),
            (Some(own), Some(libraw)) if own > MAX_TIME * libraw);
        if self.halved == Some(false) {
            if slow {
                out.push("time");
            }
            return out;
        }
        if self.de_mean.is_some_and(|v| v > MAX_MEAN) {
            out.push("colour mean");
        }
        if self.de_p95.is_some_and(|v| v > MAX_P95) {
            out.push("colour p95");
        }
        if self.detail.is_some_and(|v| !DETAIL.contains(&v)) {
            out.push("detail");
        }
        if slow {
            out.push("time");
        }
        out
    }
}

fn develop(path: &Path, decoder: Decoder, decode: Decode) -> anyhow::Result<(CameraImage, f64)> {
    let mut best = f64::INFINITY;
    let mut image = None;
    for _ in 0..2 {
        let t = Instant::now();
        let im =
            crate::photo::open_with(path, decoder)?.develop(decode, &AtomicBool::new(false))?;
        best = best.min(t.elapsed().as_secs_f64());
        image = Some(im);
    }
    Ok((image.expect("developed twice"), best))
}

/// Checks the photo at `path`; with `libraw`, also times LibRaw's half-size path.
pub fn check(path: &Path, libraw: bool) -> HalfCheck {
    let mut c = HalfCheck {
        path: path.to_path_buf(),
        ..Default::default()
    };
    let result = (|| -> anyhow::Result<()> {
        let (half, seconds) = develop(path, Decoder::Own, Decode::Half)?;
        c.half = Some([half.width, half.height]);
        c.seconds = Some(seconds);
        if libraw {
            c.libraw_seconds = develop(path, Decoder::Libraw, Decode::Half)
                .ok()
                .map(|(_, s)| s);
        }
        let full = crate::photo::open_with(path, Decoder::Own)?
            .develop(Decode::Full(Demosaic::Rawmakase), &AtomicBool::new(false))?;
        c.full = Some([full.width, full.height]);
        let halved = [half.width, half.height] == [full.width.div_ceil(2), full.height.div_ceil(2)];
        c.halved = Some(halved);
        if halved {
            let (mean, p95, detail) = compare(&full, &half);
            (c.de_mean, c.de_p95, c.detail) = (Some(mean), Some(p95), Some(detail));
        }
        Ok(())
    })();
    if let Err(e) = result {
        c.error = Some(format!("{e:#}"));
    }
    c
}

/// Block CIEDE2000 mean and 95th percentile, and the detail ratio, of `half`
/// against `full` reduced; `half` must be ⌈W/2⌉ × ⌈H/2⌉ of `full`.
pub(crate) fn compare(full: &CameraImage, half: &CameraImage) -> (f64, f64, f64) {
    let (fw, fh) = (full.width as usize, full.height as usize);
    let (hw, hh) = (half.width as usize, half.height as usize);
    let m = full.metadata.matrix;
    let srgb = |p: [f32; 3]| -> [f64; 3] {
        std::array::from_fn(|r| (0..3).map(|c| f64::from(m[r][c]) * f64::from(p[c])).sum())
    };
    let start = |i: usize, n: usize| (2 * i).min(n.saturating_sub(2));
    let mut reduced = vec![[0f64; 3]; hw * hh];
    for j in 0..hh {
        for i in 0..hw {
            let (x0, y0) = (start(i, fw), start(j, fh));
            let (x1, y1) = ((x0 + 2).min(fw), (y0 + 2).min(fh));
            let n = ((x1 - x0) * (y1 - y0)) as f64;
            let mut s = [0f64; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = srgb(full.pixels[y * fw + x]);
                    for c in 0..3 {
                        s[c] += p[c] / n;
                    }
                }
            }
            reduced[j * hw + i] = s;
        }
    }
    let ours: Vec<[f64; 3]> = half.pixels.iter().map(|&p| srgb(p)).collect();
    let mut de = Vec::new();
    for by in 0..hh / BLOCK {
        for bx in 0..hw / BLOCK {
            let (mut a, mut b) = ([0f64; 3], [0f64; 3]);
            for y in by * BLOCK..(by + 1) * BLOCK {
                for x in bx * BLOCK..(bx + 1) * BLOCK {
                    for c in 0..3 {
                        a[c] += ours[y * hw + x][c];
                        b[c] += reduced[y * hw + x][c];
                    }
                }
            }
            let n = (BLOCK * BLOCK) as f64;
            de.push(delta_e2000(lab(a.map(|v| v / n)), lab(b.map(|v| v / n))));
        }
    }
    de.sort_by(f64::total_cmp);
    let mean = de.iter().sum::<f64>() / de.len().max(1) as f64;
    let p95 = de
        .get(((de.len() as f64 * 0.95) as usize).min(de.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0.);
    let luma = |p: &[f64; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    let energy = |v: &[[f64; 3]]| {
        let (mut e, mut n) = (0., 0.);
        for j in 1..hh.saturating_sub(1) {
            for i in 1..hw.saturating_sub(1) {
                let k = j * hw + i;
                let l = 4. * luma(&v[k])
                    - luma(&v[k - 1])
                    - luma(&v[k + 1])
                    - luma(&v[k - hw])
                    - luma(&v[k + hw]);
                e += l * l;
                n += 1.;
            }
        }
        if n > 0. { e / n } else { 0. }
    };
    let reference = energy(&reduced);
    let detail = if reference > 0. {
        energy(&ours) / reference
    } else {
        1.
    };
    (mean, p95, detail)
}

/// CIELAB (D65) of linear sRGB.
fn lab([r, g, b]: [f64; 3]) -> [f64; 3] {
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f64| {
        if t > 216. / 24389. {
            t.cbrt()
        } else {
            (24389. / 27. * t + 16.) / 116.
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116. * fy - 16., 500. * (fx - fy), 200. * (fy - fz)]
}

/// CIEDE2000 colour difference (Sharma, Wu and Dalal 2005).
fn delta_e2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;
    let c_bar = (a1.hypot(b1) + a2.hypot(b2)) / 2.;
    let g = 0.5 * (1. - (c_bar.powi(7) / (c_bar.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1. + g) * a1, (1. + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| {
        if a == 0. && b == 0. {
            0.
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.)
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0. {
        0.
    } else if (h2p - h1p).abs() <= 180. {
        h2p - h1p
    } else if h2p - h1p > 180. {
        h2p - h1p - 360.
    } else {
        h2p - h1p + 360.
    };
    let dhh = 2. * (c1p * c2p).sqrt() * (dh / 2.).to_radians().sin();
    let l_bar = (l1 + l2) / 2.;
    let cp_bar = (c1p + c2p) / 2.;
    let hp_bar = if c1p * c2p == 0. {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180. {
        (h1p + h2p) / 2.
    } else if h1p + h2p < 360. {
        (h1p + h2p + 360.) / 2.
    } else {
        (h1p + h2p - 360.) / 2.
    };
    let t = 1. - 0.17 * (hp_bar - 30.).to_radians().cos()
        + 0.24 * (2. * hp_bar).to_radians().cos()
        + 0.32 * (3. * hp_bar + 6.).to_radians().cos()
        - 0.20 * (4. * hp_bar - 63.).to_radians().cos();
    let d_theta = 30. * (-((hp_bar - 275.) / 25.).powi(2)).exp();
    let rc = 2. * (cp_bar.powi(7) / (cp_bar.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1. + 0.015 * (l_bar - 50.).powi(2) / (20. + (l_bar - 50.).powi(2)).sqrt();
    let sc = 1. + 0.045 * cp_bar;
    let sh = 1. + 0.015 * cp_bar * t;
    let rt = -(2. * d_theta).to_radians().sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh))
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ciede2000_matches_published_pairs() {
        // Sharma, Wu and Dalal's test data, pairs 1, 7 and 17.
        for (a, b, want) in [
            ([50., 2.6772, -79.7751], [50., 0., -82.7485], 2.0425),
            ([50., 0., 0.], [50., -1., 2.], 2.3669),
            ([50., 2.5, 0.], [73., 25., -18.], 27.1492),
        ] {
            let got = delta_e2000(a, b);
            assert!((got - want).abs() < 1e-4, "{got} for {want}");
        }
    }

    fn image(width: u32, height: u32, pixel: impl Fn(usize, usize) -> [f32; 3]) -> CameraImage {
        let metadata = crate::camera_data::Metadata {
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        };
        CameraImage {
            width,
            height,
            recovered: Default::default(),
            pixels: (0..(width * height) as usize)
                .map(|k| pixel(k % width as usize, k / width as usize))
                .collect(),
            metadata,
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        }
    }

    /// The full decode reduced is its own perfect draft; a uniformly
    /// brighter draft fails on colour, a flattened one on detail.
    #[test]
    fn a_draft_is_judged_against_the_full_decode_reduced() {
        let full = image(64, 64, |x, y| {
            let v = 0.2 + 0.3 * ((x / 3 + y / 5) % 2) as f32;
            [v, v * 0.9, v * 0.8]
        });
        let reduce = |gain: f32| {
            image(32, 32, |i, j| {
                let mut s = [0.; 3];
                for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = full.pixels[(2 * j + y) * 64 + 2 * i + x];
                    for c in 0..3 {
                        s[c] += p[c] / 4.;
                    }
                }
                s.map(|v| v * gain)
            })
        };
        let (mean, p95, detail) = compare(&full, &reduce(1.));
        assert!(
            mean < 1e-4 && p95 < 1e-4 && (detail - 1.).abs() < 1e-5,
            "{mean} {p95} {detail}"
        );
        let brighter = HalfCheck {
            de_mean: Some(compare(&full, &reduce(1.2)).0),
            de_p95: Some(compare(&full, &reduce(1.2)).1),
            halved: Some(true),
            ..Default::default()
        };
        assert_eq!(brighter.failures(), ["colour mean", "colour p95"]);
        let flat = image(32, 32, |_, _| [0.35, 0.315, 0.28]);
        let (_, _, detail) = compare(&full, &flat);
        assert!(detail < 0.01, "{detail}");
        let slow = HalfCheck {
            seconds: Some(1.2),
            libraw_seconds: Some(1.),
            ..Default::default()
        };
        assert_eq!(slow.failures(), ["time"]);
        // A small RAW (not halved) skips the image gates but not the time.
        let small = HalfCheck {
            halved: Some(false),
            de_p95: Some(9.),
            seconds: Some(2.),
            libraw_seconds: Some(1.),
            ..Default::default()
        };
        assert_eq!(small.failures(), ["time"]);
    }

    #[test]
    fn the_chart_is_checked_at_half_size() {
        let chart = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/corpus/charts/synthetic-d65.dng"
        );
        let c = check(chart.as_ref(), false);
        assert!(c.error.is_none(), "{:?}", c.error);
        assert_eq!(
            (c.full, c.half, c.halved),
            (Some([970, 742]), Some([485, 371]), Some(true))
        );
        assert!(c.de_mean.is_some_and(f64::is_finite) && c.detail.is_some_and(f64::is_finite));
    }
}
