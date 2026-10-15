# Scene tone stage

RAWmakase renders the Basic panel's tone controls where Camera Raw does: on scene
values, before the camera profile's look table and tone curve. This page is the
contract for that stage: how it is compared with Camera Raw, what the survey found
about Camera Raw's stage order, what the stage measures from each photo, and the
gates a change to it must pass. Tracking issue: #374 (from #354).

## Pipeline

| # | Stage | Domain |
|---|---|---|
| S0 | Decode, demosaic, highlight recovery of sensor-clipped channels | camera RGB |
| S1 | White balance, the profile's matrix and HueSatMap, Camera Calibration | linear, sRGB primaries |
| S2 | Exposure and the DNG exposure ramp (black point) | linear Rec.2020 |
| S3 | **Scene tone**: Highlights, Shadows, Clarity, Dehaze, the white point and Whites, Blacks | linear ProPhoto, unclamped |
| S4 | Profile LookTable, enhanced look's HSV table, ProfileToneCurve, look curve | as before |
| S5 | Contrast, Levels, parametric and point curves, colour mixer, Point Color, RGB-table looks, grading, gamut, output | as before |

Each stage boundary has a `PixelOutput` tap, so tests can read a render between
stages through the same sampling, masks and caches as the display.

## Comparison contract

All Camera Raw comparisons go through one boundary: 16-bit ProPhoto RGB (gamma 1.8)
TIFFs from Photoshop 2026's Camera Raw (`scripts/corpus/camera_raw.py`, which opens
copies of the source with ProPhoto/16-bit open options), decoded to linear ProPhoto.

- **Stage probes** are synthetic DNGs (`scripts/corpus/probe_dng.py`): RGGB mosaics
  whose camera RGB is linear ProPhoto, with an embedded profile that maps it to XYZ
  D50 unchanged, As Shot white balance neutral, and either a linear ProfileToneCurve
  or a curved one. With the linear curve and no look, Camera Raw's ProPhoto output is
  its scene tone stage's output.
- **Photos at the scene stage**: `scripts/corpus/linear_profile.py` converts a RAW with
  Adobe DNG Converter and rewrites its embedded profile to a linear tone curve without
  a LookTable (keeping the camera's matrices, HueSatMap and baseline exposure), so a
  real photo can be read at the same boundary.
- **Final renders** (photo gates) compare Camera Raw's ProPhoto export, converted to
  sRGB by clipping (relative colorimetric, Bradford D50→D65), with RAWmakase's sRGB
  render of the settings Camera Raw embedded in its export.

For fitting the scene stage, samples Camera Raw clipped, or within 1% of the encoding
limits, are excluded and counted; final-render scores include them. Camera Raw
references are never re-blessed; RAWmakase snapshots and accepted-distance baselines
are re-blessed only per case with a reason.

## Survey (Camera Raw 18.7, Process Version 2012)

### Execution table

Measured with the probes above: each control rendered with a linear and with a curved
profile tone curve; "before the curve" means applying the curve to the linear render
predicts the curved render to the noise floor (encoded MAE 0.0003–0.003), "after"
that the control's curve measured on the linear probe predicts it when applied after
the profile curve. Look order: the same with a DCP LookTable, an XMP HSV look and an
XMP RGB-table look.

| Control | Stage | vs LookTable / HSV look | vs RGB-table look | Colour handling |
|---|---|---|---|---|
| Exposure | S2 (scale), then S3's white point | before | before | RGBTone (white-point rolloff) |
| White point (default) | S3 | before | before | RGBTone |
| Whites | S3, with the white point | before | before | RGBTone |
| Blacks | S3, after Whites (exact composition) | before | before | per channel (negative) |
| Highlights, Shadows | S3 | before | before | luminance gain |
| Clarity | S3 | before | before | luminance gain |
| Dehaze | S3 | before | before | per channel (positive: from the airlight) |
| Texture | before the profile curve (full-resolution detail) | before | before | per channel |
| Contrast | S5, after the profile curve and looks | after | — | RGBTone |

### What the default render does in S3

Camera Raw's default render is not the identity in S3. It maps a white point W* to
output 1, with a shoulder above about 0.25 (scene-linear), and leaves lower values
alone (identity when W* = 1), with the toe of its black point below about 0.02:

- W* is the sensor's white in scene units after Exposure, limited to twice the photo's
  maximum but not below 1. When the sensor's white is below 1 (negative Exposure), W*
  is that white, but not below 0.5. The sensor's white is the level at which a
  neutral's *last* channel clips, times 2^(baseline exposure + Exposure): with the
  first channel's clip the synthetic chart, whose white balance is not neutral, was
  0.02–0.04 too bright at +2 and +3 EV; with the last channel's, its gray ramp is within
  0.001 at Exposure 0 and −1.
- The photo's maximum is that of its brightest channel on a reduced copy (128 pixels
  on the long edge): on a 920-pixel probe a bright spot counts fully from about 8 pixels
  across and not at 2, and twice as large on a probe twice the size. A red highlight
  counts like a white one.
- The black point's toe does not follow the camera's baseline exposure. Engine 4 scaled
  it with the baseline (0.0015 × 2^(baseline + Exposure)), which crushed the deep
  shadows of files with a large baseline.
- The curve family is measured on neutral ramps at quarter stops of W*; above W* = 4
  it repeats in two-stop steps. Predicted from the table and this rule, 60 probes with
  different sensor whites, image maxima, sizes of bright areas and Exposure settings
  are within 0.00026 mean (0.0015 worst) encoded.

Engine 4 clipped at 1 instead: a probe whose sensor white is at scene 4 rendered
scene 1.0 at 1.0, where Camera Raw renders 0.69.

### Whites and Blacks

- **Whites** is a curve family over scene values, applied RGBTone. Negative Whites
  depends on the white point only; positive Whites stretches toward the photo's own
  maximum: a photo 4 stops below its sensor's white gets most of those stops back at
  +100 (a probe whose maximum is 0.25 maps 0.0625 to white). Two probes with the same
  white point and maximum but different sensor whites differ too, so the table is
  measured over sensor white × the photo's maximum below it × Whites (`white3.bin`).
  On photos the level it stretches toward is not their maximum: a specular highlight
  at the sensor's white does not stop it. For each of 22 training photos the level
  that best reproduces Camera Raw's Whites +100 through the table is, within 0.1 stops
  on average (median 0.06), twice the 99th percentile of the photo's luminance, at most
  its sensor's white; its maximum misses by 1.0 stops on average, the 99.9th
  percentile by 0.6. RAWmakase stretches toward that level; the white point keeps the
  maximum.
- **Blacks** composes after the white point and Whites exactly (encoded error 0.0000:
  Blacks maps their output). Negative Blacks maps the photo's darkest level to black
  when it lies above a fixed black point (about 0.016 scene); even a 4-pixel dark spot
  counts, so the minimum is measured at higher resolution than the maximum. Positive
  Blacks does not depend on it. Negative Blacks works on each channel.
- **The default black** follows the photo's dark scene level, including camera
  baseline exposure and the user's Exposure. Its key is the 0.1th percentile of
  the measurement copy's luminance before red-eye correction and spot removal.
  Positive global Shadows moves this key by its median log-luminance gain on the
  measurement copy. This is a robust approximation to Camera Raw's moving black
  response; using the unchanged key overstated the gain at negative Exposure.
  A mask selects the response at its combined Shadows value locally, interpolating
  prepared curves at steps of 0.1; pixels outside it keep the global response.
  Full-frame masked Shadows matches the global adjustment in Camera Raw probes
  with both Auto and None default-black rendering. Blacks retains its separate
  minimum-based key.

  Camera Raw 18.7 neutral ramps with the same scene values and different camera
  baselines produce the same dark outputs; raising Exposure moves the response.
  The earlier model subtracted sensor white from the key and used incomplete
  ramps measured at one baseline. Extending each row from its first nonzero
  sample also produced different low-end slopes for rows whose measured portions
  were actually the same curve.

  `default_black.bin` now contains paired DefaultBlackRender None → Auto transfers
  at absolute dark keys −14 to −7, every half stop. Above −7 the response is held
  at the measured saturated response. Samples are monotone; below a ramp's first
  measured input the generator continues its ratio, rather than storing missing
  samples as zero. These remain unmeasured tails, especially relevant to negative
  local adjustments that push pixels below the original scene minimum.

  The white tables already contain the black response of their calibration ramp,
  whose darkest patch is 14 stops below its maximum. The engine composes the
  inverse of that response with the photo's response before uploading the shared
  CPU/GPU correction table. Matching calibration and scene keys gives the identity;
  it does not apply black rendering twice. DefaultBlackRender None retains its
  separate white-curve path.

  Regenerate this family independently with `scene-tone-tables.py
  render-default-black DIR` and `scene-tone-tables.py default-black-table DIR`.
  The regression suite compares complete scene-stage renders across camera
  baselines and Exposure, and checks that correction curves preserve brightness
  order and the calibration identity. This corrects black rendering; it does not
  establish parity for the separate spatial Shadows operator.

### Local operators

Shadows and Highlights are luminance gains applied before the white point and the
profile's curve. Since the learned operator (below) replaced the measured table
families, this section records what Camera Raw does and how the operator is built and
judged; the earlier tables, their keys and the two-scale lift are in the history of
`local_tone.rs` and in this page's history.

**What Camera Raw does (probes, Camera Raw 18.7, W\* = 1).** Uniform images never
change at any level: the response is driven by contrast and the histogram, never by a
plain tone curve. On two-level images Shadows +100 lifts only the darker level, by an
amount that depends on the two levels (about 0.1 EV at 1 stop of contrast, 0.3 at 2,
0.45 at 4, 0.7 at 6) and not on which level is the minority or on its share of the
frame between 1.6% and 98%. Below about 65 px at 1024 px a spatial regime takes over
(a single 32 px dark square is lifted 3.3 EV at 6 stops; a 32 px bright spot lifts the
whole dark frame 2.4 EV, uniformly to the far corner). Highlights −100 removes about
45% of the contrast to the dark key up to 4 stops and collapses bright regions onto it
beyond about 4.5 stops when the histogram has a gap (−5.8 EV at 6 stops); with
intermediate levels present the pull of the top stop is about 1 EV. Keys are read from
a blurred copy: the same 6.25% of dark area pulls the bright majority by 2.1 EV as one
256 px square and by 0.5 EV as 1,024 scattered 8 px squares. The lift of the darkest
level grows with the histogram mass between it and the bright key (0.67 EV for a
two-level image, 1.53 EV with a mid level added, 1.4–1.6 EV for a filled 6-stop
staircase, fading out 3 stops above the minimum). On photos the arrangement effects are
small; the histogram shape and the photo's range are what the measured tables missed:
their held-out error was 2.2 times their in-sample error, and on wide-range (stress)
photos Camera Raw lifts the darkest decile 2–4.5 EV where the tables lifted 1 EV less.

**The learned operator** (`local_tone_net.rs`, fitted by
`scripts/corpus/scene-tone-net.py`). One network per slider position (±25, ±50, ±100 of
each control; three hidden layers of 48 SiLU units) predicts the log2 luminance gain
from, for a pixel: its guided base level B (the self-guided box filter of the
measurement copy's log2 luminance, radius 3.2% of the long edge, ε 0.5, read per pixel
through the map's coefficients as before); four coarser levels of the copy at the pixel
(three box passes of radius 8, 16, 32 and 64 map pixels), less B; and global statistics
of the copy, the percentiles 0.1 to 99.9, maximum and minimum of its log2 luminance
blurred by three box passes of radius 2 and of 8, the log2 mean luminance and the
99.9th percentile, both as they are and less B. Between measured positions the two
networks' gains are interpolated linearly, as the tables were; 0 is the identity. The
network is evaluated per cell of the measurement copy (4 × 4 map pixels), at 13 base
levels from 6 stops below to 6 stops above the cell's own base level: a tone curve per
cell, projected so that base + gain does not decrease (pooled adjacent violators, the
tables' guarantee). A pixel reads the curves of its four nearest cells at its own base
level, bilinearly. Masks read curves at the seven slider positions (0 included) and
interpolate at their own value; CPU, masks and the GPU read the same grids. The
networks' weights are `local_tone_net_data.rs`; `tests/corpus/camera-raw/local-tone-net.json`
holds a synthetic scene's inputs and the script's gains, which the engine must reproduce.

**Training and judging.** Gains are measured as scene-tone-local.py measures them: Camera
Raw's render of a linear-profile DNG at a slider position and its default render, mapped
back through the photo's own default global curve (RAWmakase's scene output against its
scene input), the log2 difference being the gain. Pixels near black, near white or below
2^-10 are left out, and so are pixels whose render falls below the engine's default black
floor (the inverse is undefined there; two synthetic scenes otherwise contributed gains of
7 EV) and photos whose default render is black. The training set has three parts, sampled
at 120,000 pixels per photo and 30,000 per synthetic scene: the 24 earlier training photos
and the 59 photos of the stress families' training splits (dark, clipped and wide-range
captures), 78 with usable taps; 40 synthetic scenes of `scene-tone-local.py`; and about
250 synthetic probes rendered in Camera Raw, the two-level, staircase, patch-chart and
arrangement images the survey above was measured on plus three families added for the
stage probes in CI (clipped plateaus with a smooth edge on dark keys, tiny bright spots in
dark fields, and the same values sorted into a ramp), each also rendered at Exposure +2,
+1 and −2 with engine taps at that Exposure so that values far above the sensor white are
seen in training. Synthetic scenes keep their clipped pixels (the scene value is known
exactly; Highlights pulls them back below white). Photo-only networks matched Camera Raw
on photos but not on the stage probes (they lifted the dark-gap scene 0.15 EV where Camera
Raw lifts 0.79); the probes brought the stage probes into tolerance at a small cost on
photos. Held-out scoring on the general and stress validation photos, RMS / median gain
error in EV:

| Setting | General validation (21) | Stress validation (27) |
|---|---|---|
| Shadows +100 | 0.36 / 0.29 | 0.60 / 0.46 |
| Shadows +50 | 0.17 / 0.14 | 0.27 / 0.23 |
| Highlights −100 | 0.20 / 0.16 | 0.31 / 0.25 |
| Highlights −50 | 0.10 / 0.07 | 0.17 / 0.15 |

One stress photo is left out of the table: Camera Raw's Shadows render of it falls below
the engine's black floor almost everywhere, so its measured gain (7.6 EV RMS) is not a
target the operator can reach. The structure was selected on photo-only fits against the
measured tables (Shadows +100 0.32 against 0.40 on the general set, 0.53 against 0.57 on
stress; Highlights −100 0.16 against 0.24 and 0.29 against 0.44).
A per-pixel model on raw Gaussian bases scored the same but was mottled at fine scales;
without the global statistics the stress error doubled (0.92 EV at Shadows +100); the
pixel's own level adds nothing beyond its guided base. Refitting the table families on
the same 79 photos, with keys on a blurred copy or a histogram-mass dimension, gained
5–10% at most, which is why the structure changed rather than the fit.

### Dehaze

On synthetic scenes Dehaze works on each channel by its level relative to the photo's
bright end (the 99th percentile of luminance): no change at the top and about −0.5 EV a
few stops below at +40. A curve per amount explains ±40 to 0.10 (doing nothing:
0.36–0.64) and +100 to 0.46 (1.22) in cross-validation; the rest is spatial. On the 22
training photos the same curves fitted on the photos themselves do better at every
amount (cross-validated by photo, per-channel gain in EV): +100 0.54 against 0.69 for
the synthetic curves and 0.90 for doing nothing, +40 0.16 (0.18, 0.30), −40 0.14 (0.16,
0.57), −100 0.34 (0.39, 1.45). **Negative Dehaze** uses these photo-fitted curves.

**Positive Dehaze** is a dark-channel haze removal model (`scene_tone/haze.rs`), on the
scene tone stage's input (linear ProPhoto), measured on the photo's measurement copy (512
pixels on the long edge) at Exposure 0:

1. The dark channel: the minimum of the darkest channel over a (2r+1)² window, clamped
   at the borders, with r = max(1, round(0.005 × the copy's long edge)) (3 at 512). The
   airlight A, per channel, is the mean of the pixels at or above the dark channel's 99th
   percentile (sorted index ⌊(n−1)·0.99⌋).
2. The haze's density: the same window minimum of min_c(I_c / A_c), smoothed by a guided
   filter with itself as the guide (box radius 2r, ε = 0.01).
3. A pixel p at Dehaze s > 0 (the slider plus the masks' Dehaze there) becomes
   J = (p − g·A) / t + g·A, with t = max(1 − ω(s)·density, 0.3), the density read
   bilinearly at the pixel's position and g = 2^Exposure. ω is linear between (0, 0),
   (+20, 0.3), (+40, 0.45) and (+100, 0.9). No channel falls below half of p scaled by
   the luminance's change, so weak channels of bright saturated colours are not driven
   to black (on the training photos: +100 3.06 → 3.04; a floor of 0.7 costs 3.18, 1.0
   costs 4.53). The transmission floor 0.3 costs nothing there (3.06 at 0.1 to 0.3).

The model's few parameters (the window, ε, the transmission floor and ω) were chosen by
grid search on the 22 training photos against Camera Raw 18.7, by display ΔE00. Median
over the photos, against the curves above: +100 5.97 → 3.06, +40 2.73 → 1.68, +20 1.42.
A and the density read exactly what the photo's measures read (white balance, the
profile, calibration, the camera's exposure), so they are kept with them in the stage
cache, measured the first time a render has positive Dehaze somewhere; Exposure scales A
with the scene.

The model is fitted to photos; on synthetic inputs it does less well than the curves.
On the stage probes at +40 the error of the effect grows on all three scenes (0.048 →
0.110, 0.069 → 0.152 and 0.036 → 0.044 EV), and on the synthetic chart it darkens Dehaze
+100 more than Camera Raw (p95 ΔE00 11.3 → 17.1, while the mean improves 6.9 → 5.9):
its wide-gamut cyan patches lose their red, which Camera Raw only reduces, and its dark
gray field darkens strongly.

### Masks

A full-frame mask's Shadows, Highlights, Dehaze, Clarity and Contrast render like the
global sliders in Camera Raw (within 0.006–0.012 encoded, against effects of
0.03–0.12); its Whites and Blacks do not (Blacks +50 differs by 0.07 for an effect of
0.013). Lightroom stores a mask's Exposure normalised: `LocalExposure2012` 1 is +4 EV
(0.8 rendered as +3.2 EV).

## Testing

- **Clipped-highlight regressions (CI)**: `clipped_highlights_do_not_solarize`
  decodes a synthetic clipped plateau with a smooth radial edge on backgrounds
  five, seven and nine stops darker. Highlights 0, −50 and −100 run through both
  global XMP settings and a full-frame mask. Rim/centre luminance ratios must not
  exceed Camera Raw 18.7's measurements by more than 0.05. References are in
  `camera-raw/highlight-spots.json`, reproduced by
  `scripts/corpus/highlight-probes.py`; they are independent references, not
  blessable engine snapshots. The scalar curve test also sweeps every integer
  slider position, including interpolation and the clamped extensions, and checks
  global, per-pixel and GPU-uploaded tables. The hardware test
  `gpu_measured_tails_match_cpu_globally_and_in_masks` checks actual GPU evaluation.
- **Tier 1 (stage probes, CI)**: `scene_probes_do_not_regress` (`tests/color/scene_probes.rs`)
  renders synthetic probes to the S3 output tap (`develop::quality::render_stage`) and
  compares them with Camera Raw 18.7's renders of the same probes
  (`tests/corpus/camera-raw/scene-probes.json`, from `scripts/corpus/scene-probes.py`):
  white ramps at W* = 1, 4 and 16 (default, Whites ±50 and ±100), Exposure −2, −1 and +1
  on ramps at W* = 1 (clipped) and 4 (overrange), black ramps with darkest levels 2^-4
  and 2^-8 (Blacks ±50 and ±100), a full-frame mask's Whites +50 and Blacks −50, and
  three synthetic scenes (`probe_scenes.py` seeds 1, 3, 4, committed reduced to 384×256)
  at Shadows +100/−50, Highlights −100/+50, Dehaze ±40 and Clarity ±50. Ramps are scored
  by patch means, scenes by 24×16 block luminances; each case records its encoded (gamma
  1.8) mean and largest error and, for scenes, the mean error of the setting's effect in
  EV in `scene-probes-baseline.json`. A case fails when it gets further from Camera Raw
  than its baseline (5% plus 0.0005 mean, 0.003 largest, 0.005 EV effect); intended
  changes are blessed per case with `RAWMAKASE_BLESS=1`.
- **Tier 2 (structural, CI)**: stage placement (the full render equals the downstream
  stages replayed from the S3 tap), CPU/GPU equivalence per stage, cached against fresh
  renders, photo measures equal across preview sizes, regions and export.
- **Tier 3 (photo gates, private)**: below.

## Photo gates

`scripts/corpus/gate.py` holds the frozen metric and the acceptance rule. Photos,
references and renders stay outside the repository.

**Sets.** Train (fitting statistics), validation (accept or reject during
development) and holdout (scored once, at the switch decision), fixed before fitting,
from raw.pixls.us CC0 RAWs and the owner's RAWs. Each is stratified by scene
brightness: the 90th percentile of L* of Camera Raw's default render (Adobe
Standard), below 40, 40–75, above 75. One photo per camera model, so a scene never
appears in two sets.

**Settings.** Each percentage slider (Contrast, Highlights, Shadows, Whites, Blacks,
Texture, Clarity, Dehaze) at ±25, ±50, ±100; Exposure at −2, −1, +1, +2 EV; Exposure −1
with Whites +100, Shadows +50 with Highlights −50, Contrast +50 with Blacks −50,
Dehaze +40 with Clarity +40, a gradient mask with Exposure +2 EV and Shadows +50 (0.5 and 0.5 in Lightroom's normalised local units); and,
with Adobe Color, the default render, Shadows +100, Dehaze +40 and Whites +100 (#354).

**Metric.** Both renders reduced by area to 1024 pixels on the long edge. Per-photo
score: median ΔE00 over pixels (secondary: mean). A setting's score over a set: the
median of the per-photo scores (secondary: mean). Signed ΔL*: mean of RAWmakase −
Camera Raw L* over pixels whose Camera Raw L* falls in a 10-wide band, pooled over a
stratum's photos; bands with less than 1% of the pixels are skipped. Clipping
disagreement: the fraction of pixels where exactly one render is at white (any
channel ≥ 0.995) or at black (all ≤ 0.005). Valid pixels: the common frame less a
3-pixel border. Final-render scores include clipped pixels.

Metric policy 2 additionally retains mean ΔE00 for every overlapping spatial
window (edge 1/16 of the valid frame's long edge, half-window steps, including
the far edges). Corresponding windows are compared individually: a bad region
elsewhere cannot hide a new defect. Reference-defined highlights (L* ≥ 90) and
shadows (L* ≤ 10) are also scored separately whenever they contain at least 16
pixels, even below 1% of the frame. These use unsigned colour distance, so
light/dark or colour errors cannot cancel. Any non-finite pixel invalidates a
photo rather than being silently discarded.

All acceptance modes require three scored photos in **every** brightness
stratum for every declared setting. Missing strata, missing settings, unscored
references and inconclusive results fail, including in `dataset-gate.py`.
Every corresponding spatial or tonal region must stay within baseline +2 ΔE00;
every photo's clipping disagreement must stay within baseline +0.5 percentage
points, independently of the stratum average. These checks supplement, not
replace, the clipped-plateau and monotonicity regressions above.

`--rule regression` protects the preceding engine's measured behaviour.
`--rule parity` adds absolute requirements on **each photo**, including defaults:
median ΔE00 ≤ 2, every spatial/tonal region's mean ΔE00 ≤ 5, and clipping
disagreement ≤ 0.5%. These are initial engineering acceptance budgets, not
experimentally established perceptual-equivalence thresholds. They deliberately
do not become looser when the baseline is bad. Review regional crops and retain
full-resolution/detail tests before claiming Lightroom parity. Sharpening, noise
reduction, lens corrections and errors outside sRGB remain outside this gate.

Scores made before metric policy 2 cannot pass acceptance or resume a new metric
run: they do not contain regional errors. Rescore retained renders, or render
the same pinned baseline and candidate binaries into new run folders when the
renders were deleted. Keep Camera Raw references and old scores unchanged.

**Switch rule** (`--rule switch`, fixed before fitting). For every setting, on validation and then
holdout, in every brightness stratum:

1. **Material improvement**: the setting's score ≤ the default render's score + 1.0,
   or ≤ 60% of engine 4's score for that setting.
2. **Bands**: |signed ΔL*| ≤ 3 in every band, or ≤ 50% of engine 4's in that band.
3. **No photo regresses**: per-photo score worse than engine 4's by at most 1.0 for
   settings whose largest slider magnitude is ≤ 50 (Exposure ±1 EV counts as 50,
   ±2 EV as 100) and 2.0 above that.
4. **Endpoints**: clipping disagreement ≤ engine 4's + 0.5 percentage points.
5. **Default render** (all sliders 0): score no worse than engine 4's + 0.1.

Settings where engine 4 is already within the default render's score + 0.5 need only
rules 3–5. A stratum with fewer than 3 photos, or a photo with less than 50% valid
pixels, is inconclusive, which fails the gate until data is added. Any exception
needs the owner's explicit, documented approval per setting. A holdout is scored
once; if it fails and its errors are inspected, it becomes validation data and a
fresh holdout is frozen before further fitting. Adaptive statistics are identified
with controlled synthetic probes before fitting; no offsets are fitted to photos.
Engine 4's photo results are frozen first, as the comparison baseline.

### Default-black calibration validation

The paired black-response calibration corrects the baseline-exposure dependence
and is checked against fixed Camera Raw ramp outputs. Updating the engine
snapshots records this intentional rendering change; the Camera Raw reference
pixels remain unchanged. In scene 3, final-output mean absolute errors improve
for Contrast +50 (0.0074 → 0.0038), Dehaze +40 (0.0118 → 0.0090) and
Dehaze −40 (0.0107 → 0.0064). Their adjustment-relative errors increase from
0.064 to 0.074, 0.223 to 0.264 and 0.069 to 0.092 EV respectively because
the default rendering also changes. These are accepted calibration tradeoffs,
not evidence of complete Camera Raw parity. Shadows +100 on this scene changes
slightly in the other direction (output error 0.0023 → 0.0026). The weak
Shadows response in scenes with scattered bright gaps remains a separate issue.
