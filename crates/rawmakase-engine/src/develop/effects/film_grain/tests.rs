use super::*;
use crate::develop::effects::GrainField;
use crate::model::operators::GrainModel;

fn field(size: f32, roughness: f32, edge: f32) -> FilmGrainField {
    FilmGrainField::new(
        &FilmGrain {
            amount: 1.,
            size,
            roughness,
            ..FilmGrain::default()
        },
        edge,
    )
}
fn mean_and_deviation(values: &[f32]) -> (f32, f32) {
    let n = values.len() as f32;
    let mean = values.iter().sum::<f32>() / n;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n;
    (mean, variance.sqrt())
}
/// Positions far enough apart to sample the grain almost independently.
fn spread(count: usize) -> impl Iterator<Item = (f32, f32)> {
    (0..count).map(|i| ((i % 400) as f32 * 31.3, (i / 400) as f32 * 29.7))
}

/// On flat tones the grain adds about nothing on average, at every Size, Roughness and
/// photo size; it is exactly nothing at black and white, and fades below the darkest
/// tone it is calibrated for.
#[test]
fn film_grain_is_about_mean_preserving_and_fades_into_black_and_white() {
    let mut worst: f32 = 0.;
    for edge in [1500., 6000.] {
        for size in [0., 0.25, 1.] {
            for roughness in [0., 0.5, 1.] {
                let f = field(size, roughness, edge);
                assert!(f.presence(f.darkest) <= MAX_PRESENCE);
                assert!(f.darkest < 0.05, "edge {edge} size {size}: {}", f.darkest);
                for l in [f.darkest, 0.2, 0.5, 0.8, 0.95] {
                    let values: Vec<f32> =
                        spread(20_000).map(|(x, y)| f.delta(x, y, 1., l)).collect();
                    let (mean, _) = mean_and_deviation(&values);
                    worst = worst.max(mean.abs());
                    assert!(
                        mean.abs() < 0.02,
                        "edge {edge} size {size} roughness {roughness} l {l}: mean {mean}"
                    );
                }
                for (x, y) in spread(200) {
                    assert_eq!(f.delta(x, y, 1., 0.), 0.);
                    assert_eq!(f.delta(x, y, 1., 1.), 0.);
                }
                let deviation = |l: f32| {
                    let values: Vec<f32> =
                        spread(20_000).map(|(x, y)| f.delta(x, y, 1., l)).collect();
                    mean_and_deviation(&values).1
                };
                let ratio = deviation(f.darkest / 2.) / deviation(f.darkest);
                assert!(
                    (ratio - 0.5).abs() < 0.1,
                    "edge {edge} size {size} roughness {roughness}: faded to {ratio}"
                );
            }
        }
    }
    eprintln!("largest mean: {worst}");
}

#[test]
fn film_grain_is_strongest_in_the_midtones() {
    let f = field(0.25, 0.5, 6000.);
    let deviation = |l: f32| {
        let values: Vec<f32> = spread(20_000).map(|(x, y)| f.delta(x, y, 1., l)).collect();
        mean_and_deviation(&values).1
    };
    let mid = deviation(0.5);
    assert!(mid > 0.2 * STRENGTH, "{mid}");
    assert!(deviation(0.06) < mid && deviation(0.97) < mid * 0.5);
}

/// The variance of `value` at each of 4×4 phases within cells of `cell` pixels:
/// sampled at the same phase in many cells.
fn variance_by_phase(cell: f32, value: impl Fn(f32, f32) -> f32) -> Vec<f32> {
    let mut out = Vec::new();
    for by in 0..4 {
        for bx in 0..4 {
            let values: Vec<f32> = (0..4096)
                .map(|i| {
                    let x = ((i % 64) as f32 + (bx as f32 + 0.5) / 4.) * cell;
                    let y = ((i / 64) as f32 + (by as f32 + 0.5) / 4.) * cell;
                    value(x, y)
                })
                .collect();
            out.push(mean_and_deviation(&values).1.powi(2));
        }
    }
    out
}
fn spread_of(values: &[f32]) -> f32 {
    let max = values.iter().cloned().fold(f32::MIN, f32::max);
    let min = values.iter().cloned().fold(f32::MAX, f32::min);
    max / min
}

/// Grain on a lattice is strongest at its knots; Film Grain has no such structure,
/// which the same measure shows for a value-noise grain.
#[test]
fn film_grain_has_no_lattice() {
    let f = field(0.25, 0.5, 6000.);
    let film = spread_of(&variance_by_phase(f.cell, |x, y| f.delta(x, y, 1., 0.5)));
    assert!(film < 1.2, "film grain: {film}");
    let lattice = GrainField {
        model: GrainModel::Measured,
        amount: 1.,
        cell: 8.,
        coarse: 1.,
        fine: 0.,
        seed: 42,
    };
    let value_noise = spread_of(&variance_by_phase(lattice.cell, |x, y| {
        lattice.noise(x, y, 1., 0.62)
    }));
    assert!(value_noise > 1.5, "value noise: {value_noise}");
}

/// Each cell holds a fixed number of candidates; staggering their grids keeps that from
/// showing as a pattern of the cell's period in the mean. Sampled at 4×4 phases in the
/// same cells, each cell's own mean taken out. With 262 144 cells the noise in the
/// range of the 16 phase means is about 0.003, so a pattern of 0.008 or more fails.
#[test]
fn film_grain_mean_does_not_follow_the_cells() {
    use rayon::prelude::*;
    let mut worst: f32 = 0.;
    for size in [0., 0.25, 1.] {
        let f = field(size, 0.5, 6000.);
        for l in [0.2, 0.5] {
            let cells = 262_144;
            let residuals: Vec<[f32; 16]> = (0..cells)
                .into_par_iter()
                .map(|c| {
                    let (cx, cy) = ((c % 512) as f32, (c / 512) as f32);
                    let values: [f32; 16] = std::array::from_fn(|b| {
                        let x = (cx + ((b % 4) as f32 + 0.5) / 4.) * f.cell;
                        let y = (cy + ((b / 4) as f32 + 0.5) / 4.) * f.cell;
                        f.delta(x, y, 1., l)
                    });
                    let mean = values.iter().sum::<f32>() / 16.;
                    values.map(|v| v - mean)
                })
                .collect();
            let at: Vec<(f32, f32)> = (0..16)
                .map(|b| {
                    let r: Vec<f32> = residuals.iter().map(|v| v[b]).collect();
                    let (mean, deviation) = mean_and_deviation(&r);
                    (mean, deviation / (r.len() as f32).sqrt())
                })
                .collect();
            let error = at.iter().map(|m| m.1).fold(0., f32::max);
            let means: Vec<f32> = at.iter().map(|m| m.0).collect();
            let range = means.iter().cloned().fold(f32::MIN, f32::max)
                - means.iter().cloned().fold(f32::MAX, f32::min);
            worst = worst.max(range);
            // The test must resolve the limit: noise alone is well below it.
            assert!(
                5. * error < 0.004,
                "size {size} l {l}: standard error {error}"
            );
            assert!(range < 0.006, "size {size} l {l}: {means:?} ± {error}");
        }
    }
    eprintln!("largest phase range: {worst}");
}

/// Grain is the same share of the frame at any photo size: larger in pixels on a
/// larger photo, and larger with Size. Measured by how far the grain stays correlated:
/// the sum of its autocorrelation over lags of 1 to 4 pixels.
#[test]
fn film_grain_grows_with_size_and_the_photo() {
    let extent = |f: &FilmGrainField| {
        let at = |x: f32, y: f32| f.delta(x, y, 1., 0.5);
        let base: Vec<f32> = spread(20_000).map(|(x, y)| at(x, y)).collect();
        let (a, sa) = mean_and_deviation(&base);
        (1..=4)
            .map(|lag| {
                let shifted: Vec<f32> =
                    spread(20_000).map(|(x, y)| at(x + lag as f32, y)).collect();
                let (b, sb) = mean_and_deviation(&shifted);
                base.iter()
                    .zip(&shifted)
                    .map(|(x, y)| (x - a) * (y - b))
                    .sum::<f32>()
                    / base.len() as f32
                    / (sa * sb)
            })
            .sum::<f32>()
    };
    let small = extent(&field(0., 0.5, 6000.));
    let large = extent(&field(1., 0.5, 6000.));
    eprintln!("correlation extent: Size 0 {small}, Size 1 {large}");
    assert!(large > small * 1.5 + 0.2, "{small} {large}");
    let photo_small = extent(&field(0.5, 0.5, 1500.));
    let photo_large = extent(&field(0.5, 0.5, 6000.));
    eprintln!("correlation extent: 1500 px {photo_small}, 6000 px {photo_large}");
    assert!(
        photo_large > photo_small * 1.5 + 0.1,
        "{photo_small} {photo_large}"
    );
}

/// A brighter tone shows a subset of a darker tone's grains.
#[test]
fn brighter_tones_leave_more_uncovered_everywhere() {
    let f = field(0.25, 0.5, 6000.);
    for (x, y) in spread(5000) {
        let mut last = 0.;
        for l in [0.05, 0.2, 0.4, 0.6, 0.8, 0.95] {
            let u = f.uncovered(x, y, f.presence(l));
            assert!(u >= last, "{x} {y} {l}");
            last = u;
        }
    }
}

/// A preview at a half or a quarter of full size averages exactly the pixels the full
/// render has there; smaller ones approximate the averaged grain's strength.
#[test]
fn previews_average_the_full_resolution_grain() {
    let l = 0.4;
    let f = field(0.25, 0.5, 6000.);
    for n in [2u32, 4] {
        let scale = 1. / n as f32;
        for origin in [0i32, 7, -3] {
            for py in 0..12 {
                for px in 0..12 {
                    let (px, py) = (px + origin, py + origin);
                    let x = (px as f32 + 0.5) / scale - 0.5;
                    let y = (py as f32 + 0.5) / scale - 0.5;
                    let preview = f.delta(x, y, scale, l);
                    let mut full = 0.;
                    for j in 0..n as i32 {
                        for i in 0..n as i32 {
                            let fx = (px * n as i32 + i) as f32;
                            let fy = (py * n as i32 + j) as f32;
                            full += f.delta(fx, fy, 1., l);
                        }
                    }
                    full /= (n * n) as f32;
                    assert!(
                        (preview - full).abs() < 1e-4,
                        "{n} {px} {py}: {preview} {full}"
                    );
                }
            }
        }
    }
    for (size, scale) in [
        (0., 1. / 8.),
        (0.25, 1. / 8.),
        (1., 1. / 8.),
        (0.25, 1. / 16.),
    ] {
        let f = field(size, 0.5, 6000.);
        let k = (1. / scale) as usize;
        let (mut preview, mut full) = (Vec::new(), Vec::new());
        for i in 0..3000 {
            let (px, py) = ((i % 60) as f32 * 3., (i / 60) as f32 * 3.);
            preview.push(f.delta((px + 0.5) / scale - 0.5, (py + 0.5) / scale - 0.5, scale, l));
            let mut sum = 0.;
            for j in 0..k {
                for m in 0..k {
                    sum += f.delta(px * k as f32 + m as f32, py * k as f32 + j as f32, 1., l);
                }
            }
            full.push(sum / (k * k) as f32);
        }
        let ratio = mean_and_deviation(&preview).1 / mean_and_deviation(&full).1;
        eprintln!("size {size} scale {scale}: preview/full deviation {ratio}");
        assert!(
            (ratio - 1.).abs() < 0.35,
            "size {size} scale {scale}: {ratio}"
        );
    }
}

fn layers(color: f32) -> FilmGrainLayers {
    FilmGrainLayers::new(
        &FilmGrain {
            amount: 1.,
            color,
            ..FilmGrain::default()
        },
        6000.,
    )
}
fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let ((ma, sa), (mb, sb)) = (mean_and_deviation(a), mean_and_deviation(b));
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - ma) * (y - mb))
        .sum::<f32>()
        / a.len() as f32
        / (sa * sb)
}

/// At Color 0 every channel takes the mono grain over the luminance, exactly as before
/// Color existed; the mono grain is the green layer's.
#[test]
fn color_zero_is_the_mono_grain_on_every_channel() {
    let film = layers(0.);
    let mono = FilmGrainField::new(
        &FilmGrain {
            amount: 1.,
            ..FilmGrain::default()
        },
        6000.,
    );
    assert_eq!(*film.mono(), mono);
    for (x, y) in spread(2000) {
        let rgb = [0.6, 0.4, 0.2];
        let l = crate::color::luminance(rgb);
        assert_eq!(film.delta(x, y, 1., rgb), [mono.delta(x, y, 1., l); 3]);
    }
}

/// At Color 1 each channel has a grain of its own, about as strong as the mono grain
/// and still about mean-preserving; a little Color shades the mono grain rather than
/// replacing it.
#[test]
fn full_color_gives_each_channel_its_own_grain() {
    let gray = [0.5; 3];
    let channels = |film: &FilmGrainLayers| -> [Vec<f32>; 3] {
        let deltas: Vec<[f32; 3]> = spread(20_000)
            .map(|(x, y)| film.delta(x, y, 1., gray))
            .collect();
        std::array::from_fn(|c| deltas.iter().map(|d| d[c]).collect())
    };
    let [mono, ..] = channels(&layers(0.));
    let color = channels(&layers(1.));
    let (_, mono_deviation) = mean_and_deviation(&mono);
    for (c, channel) in color.iter().enumerate() {
        let (mean, deviation) = mean_and_deviation(channel);
        assert!(mean.abs() < 0.01, "channel {c}: mean {mean}");
        assert!(
            (deviation / mono_deviation - 1.).abs() < 0.3,
            "channel {c}: {deviation} against {mono_deviation}"
        );
    }
    for (a, b) in [(0, 1), (1, 2), (0, 2)] {
        let r = correlation(&color[a], &color[b]);
        assert!(r.abs() < 0.15, "channels {a} and {b}: correlation {r}");
    }
    // Green follows the mono grain's grains, so a little Color keeps its pattern.
    let [_, green, _] = channels(&layers(0.1));
    assert!(correlation(&mono, &green) > 0.9);
}

/// Each layer's grain follows its own channel: none in a channel at black or white.
#[test]
fn each_layer_follows_its_own_channel() {
    let film = layers(1.);
    let mut green = Vec::new();
    for (x, y) in spread(2000) {
        let [r, g, b] = film.delta(x, y, 1., [0., 0.5, 1.]);
        assert_eq!((r, b), (0., 0.));
        green.push(g);
    }
    assert!(mean_and_deviation(&green).1 > 0.05 * STRENGTH);
}

/// Blue, the fastest layer, has the coarsest grain, red a little coarser than green.
#[test]
fn blue_layer_grain_is_coarsest() {
    let film = layers(1.);
    let extent = |f: &FilmGrainField| {
        let base: Vec<f32> = spread(20_000)
            .map(|(x, y)| f.delta(x, y, 1., 0.5))
            .collect();
        (1..=4)
            .map(|lag| {
                let shifted: Vec<f32> = spread(20_000)
                    .map(|(x, y)| f.delta(x + lag as f32, y, 1., 0.5))
                    .collect();
                correlation(&base, &shifted)
            })
            .sum::<f32>()
    };
    let [red, green, blue] = film.layers.each_ref().map(extent);
    eprintln!("correlation extent: red {red}, green {green}, blue {blue}");
    assert!(blue > red && red > green, "{red} {green} {blue}");
}

/// Off, Film Grain leaves the finished photo exactly as it was; on, it changes it.
#[test]
fn spatial_finish_adds_film_grain_only_when_on() {
    use crate::model::recipe::Recipe;
    use crate::rendered::Rendered;
    let image = || Rendered {
        width: 32,
        height: 32,
        pixels: (0..1024).map(|i| [i as f32 / 1024.; 3]).collect(),
    };
    let mut r = Recipe::default();
    r.effects.vignette = -0.3;
    let mut off = image();
    crate::develop::effects::spatial_finish(&mut off, &r, [0, 0], [32, 32]);
    r.film_grain.size = 0.9;
    r.film_grain.seed = 3;
    let mut still_off = image();
    crate::develop::effects::spatial_finish(&mut still_off, &r, [0, 0], [32, 32]);
    assert_eq!(off.pixels, still_off.pixels);
    r.film_grain.amount = 0.3;
    let mut on = image();
    crate::develop::effects::spatial_finish(&mut on, &r, [0, 0], [32, 32]);
    assert_ne!(off.pixels, on.pixels);
}

/// Export-sized Film Grain on the CPU, for the benchmark gate:
/// `cargo test --release -p rawmakase-engine film_grain_export_time -- --ignored --nocapture`.
#[test]
#[ignore = "timing"]
fn film_grain_export_time() {
    use crate::model::recipe::Recipe;
    use crate::rendered::Rendered;
    let (w, h) = (6000u32, 4000u32);
    let image = Rendered {
        width: w,
        height: h,
        pixels: vec![[0.46; 3]; (w * h) as usize],
    };
    for color in [0., 0.5, 1.] {
        let mut image = image.clone();
        let mut r = Recipe::default();
        r.film_grain.amount = 0.3;
        r.film_grain.color = color;
        let start = std::time::Instant::now();
        crate::develop::effects::spatial_finish(&mut image, &r, [0, 0], [w, h]);
        eprintln!("24 MP Film Grain, Color {color}: {:?}", start.elapsed());
    }
}
