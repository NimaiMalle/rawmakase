# Systematic camera calibration audit

Measurement base: `e8335b45e871bc4bd19814b9c9c7b50785d542be`, containing merged
#478 and #481. GitHub reports #481 merged with base `main`; all applicable checks
succeeded (the legacy packaging job was intentionally skipped). The comparison
binary was rebuilt from this exact revision. This work changes X-T5 native white
balance calibration, not baseline exposure or the rendering engine.

## Inventory and evidence

The [generated inventory](camera-calibration-inventory.md) accounts for every
sample, grouped by camera and failure type, with measured/applied WB values,
recording modes, available ISO, hashes, source links and a separate baseline table.
The machine-readable equivalent includes matrices, signatures and AnalogBalance:

```sh
python3 scripts/cameras/inventory.py --report /tmp/calibration-inventory.json \
  --markdown docs/camera-calibration-inventory.md
```

The original 149 facts reproduce **79 matches, 61 mismatches and nine unsupported
samples**, across 145 make/model pairs. The 61 mismatches were 30 Fujifilm and 31
Canon samples, not 61 proven model-wide defects. Most models have one sample.

Temporary copies of all 142 original public RAWs were reconverted with Adobe DNG
Converter **18.7 (2743)**. Their recorded calibration tags agreed with the earlier
facts. Ten additional public files already available in the corpus were converted;
all public hashes were verified. Originals and sidecars were untouched. Numerical
recording metadata, baseline tags and native WB facts were added; no RAWs, profiles,
serial numbers or local paths are committed. The expanded facts contain **159
samples / 145 models**: 152 public and seven anonymous local samples.
Native EXIF `BitsPerSample`/`Compression` can describe a preview rather than
sensor storage; they are not used to infer RAW precision. Public recording-mode
labels and converted full-image storage tags are kept separate.

With the X-T5 correction, the expanded audit reports 81 `ok`, 63 `mismatch`, five
`missing` table rows, and ten `unsupported` samples. The actionable grouping is:

| Category | Samples | Interpretation |
| --- | ---: | --- |
| Supported match | 38 | Includes two identity-calibrated D810 samples with no camera table row; missing baseline rows are not WB errors. |
| Per-file correction matches | 45 | 43 Sony samples plus two X-T5 samples. |
| Supported DNG transform; native correction unresolved | 66 | 63 measured mismatches and three non-identity samples without table rows. |
| Unsupported transformation/metadata | 10 | The original nine plus another RX1R III recording mode. |

The 66 native gaps have insufficient evidence for installing a constant correction.
The Fujifilm D65 presets suggest additional per-file relationships, but no model
borrows the X-T5 reference. Canon's native daylight presets are recorded as evidence;
their relationship to converter calibration remains unresolved. Multiple files do
not establish multiple independent bodies; body count and firmware coverage remain
unknown. The `evidence_role` field identifies X-T5 calibration versus validation;
other new captures are diagnostic, not independent render-validation claims.

## Verified correction: Fujifilm X-T5

The calibration sample is public file **6123**, ISO 125, compressed 3:2. Its native
D65 preset `[658, 302, 473]` and converter gains `[0.9347, 1, 1.055]` imply the
integer model reference `[615, 302, 499]`. The independent validation file **7271**,
ISO 400, sports-finder/crop mode, has preset `[646, 302, 494]`. The same reference
predicts its different gains `[0.952, 1, 1.0101]` to Converter's four-decimal precision.
No rendered pixel or exposure offset was fitted to obtain that reference.

The native adapter reads the preset; the camera table owns the model reference;
profile WB conversion applies the correction only for matching Adobe calibration.
The rule normalizes both presets to green before dividing, as the existing Sony
rule does. It never replaces the camera's As Shot multipliers with the daylight
preset. DNG metadata takes precedence and cannot receive the native correction twice.

An actual-adapter test opens both hash-verified RAWs and their converted DNGs and
uses the same embedded profile. As Shot agrees within 0.1 K / 0.01 tint and custom
WB gains within 0.00001 at 3200/0, 5600/0, 7500/0 and 5600/+20. Unit tests cover
normalization, missing/invalid metadata, signature mismatch, unknown models and
DNG precedence. The numerical regression failed with identity calibration before
the implementation. Saved edits retain their stored settings and baseline; an
explicit camera-baseline action changes only that baseline.

```sh
RAWMAKASE_CORPUS=/path/to/corpus \
RAWMAKASE_CALIBRATION_DNGS=/path/to/hash-named-conversions \
  cargo test --locked --test camera_calibration -- --ignored
```

## Unsupported cases are not interchangeable

All original nine records were inspected beyond the audit's first error: each has
matching Adobe signatures, equal diagonal CameraCalibration1/2 and non-identity
AnalogBalance. Fresh conversion shows **linear RGB DNG storage** (Photometric-
Interpretation 34892, three samples per pixel) in every case. None supplies evidence
of an off-diagonal or illuminant-dependent correction.

- Sony **ILCE-1M2** and **DSC-RX1RM3**: reduced/linear recordings. Additional full
  mosaic RAWs convert with identity AnalogBalance and the same diagonal camera
  calibration as their linear counterparts. Another RX1R III aspect mode also
  produces a linear DNG. Native calibration references are candidates, not installed.
- Canon **EOS 5D Mark IV, EOS 5DS R, EOS 6D Mark II, EOS 7D Mark II, EOS 80D,
  EOS-1D X Mark II**: the original samples are sRAW2. An additional full-size 5D IV
  RAW has identity AnalogBalance and the same diagonal calibration.
- Nikon **D810**: the original small RAW has identity CameraCalibration but
  non-identity AnalogBalance. Additional 12-bit and 14-bit mosaic files have
  identity calibration and identity AnalogBalance.

The native adapter currently rejects converted linear DNGs because they are neither
mosaics nor LibRaw-native sRAWs. Thus these comparisons first need a deliberately
tested linear-DNG decode path, including where the stored analog gain is represented
in pixels and AsShotNeutral. Installing AnalogBalance as native model gains would
mix storage normalization with unit calibration and risks applying gain twice.
General matrices, illuminant interpolation and analog scaling remain unsupported;
no general matrix was flattened into diagonal gains. A focused decode/transformation
change needs synthetic order/signature tests and actual native/linear-DNG comparisons.

## Baseline exposure remains separate

There are now converter baseline tags for **155/159** samples. Four anonymous
local samples still lack them. Comparing raw tags with static rows gives 48 equal
values, 92 unequal values and 19 unavailable comparisons (missing tag or table row).
These are **not 92 exposure defects**. The inventory deliberately does not propose
replacement values: runtime Fujifilm shifts, Canon Highlight Tone Priority,
extended ISO, black/white sensor normalization and profile baseline offsets must
be resolved before a rendered-default comparison is meaningful.

For example, the two X-T5 converter tags differ by one stop (+0.02 / +1.02) while
the table is a fitted −0.10. That is separate from their independently measured
WB corrections. The fitted row is left unchanged; its remaining tone/normalization
cause is unresolved. No old edit is migrated to a new exposure value.

## Render validation and scope

Camera Raw references use **Adobe Standard**, with the embedded profile/settings
verified and RAW, TIFF and resolved-XMP hashes frozen. No Adobe Color label-only
comparison is used. No references or thresholds are replaced to pass this change.

The ISO 125 calibration photo passes all 17 distinct settings: default, four
custom WB settings, Shadows +50/+100, Dehaze +20/+40, Whites +50/+100, Exposure
±1, Highlights ±100, Shadows +100 with Highlights −100, and a warm-WB/contrast/
shadows/highlights combination. The combined tone case remains close to its fixed
extra-MAE limit (0.0198 versus 0.020); this is not an engine-parity claim.

| ISO 125 case | Before median ΔE00 | Corrected median ΔE00 |
| --- | ---: | ---: |
| Default / As Shot | 1.66 | 1.66 |
| 3200 K / 0 | 2.61 | 1.76 |
| 5600 K / 0 | 3.09 | 1.66 |
| 7500 K / 0 | 3.49 | 1.66 |
| 5600 K / +20 | 3.21 | 1.67 |

Three of these custom-WB cases failed the existing gate before the correction;
all four pass afterward. As Shot's displayed coordinates move from about
5645 K / +7.6 to 5161 K / +6.9, versus Camera Raw's embedded 5150 K / +7.
The small remaining coordinate difference is not explained as rounding.
As Shot retains the camera multipliers; its corrected temperature can still alter
profile interpolation. The default score rounds to 1.66 in both versions.

At the pinned calibration revision, the independent ISO 400 sample has a framing mismatch: the checker receives
171×256 reference and 170×256 candidate arrays. It refuses to score them. This
limits rendered validation despite native/DNG metadata agreement. The exact baseline
binary produces the same framing failure, confirming that it predates this change.

The separate RAF decode correction at
`87d37dae79567c90d929ce3a0af1b69ab5042722` resolves that framing failure.
LibRaw aligned the native crop's starting row from 21 to 24, removing three rows;
retaining the preceding CFA period and applying the exact crop restores
6000×4000, exporting 2048×1365 like the frozen reference. See
[decode ownership and bounds](demosaic.md) and the
[follow-up numerical results](../data/fuji-crop-validation.json). Camera calibration,
exposure and tone processing are unchanged by that correction.

With the same frozen Adobe Standard references and unchanged gates, the independent
photo passes default/As Shot (median ΔE00 1.66), all four custom-WB settings
(1.68–1.94), Exposure ±1, Highlights ±100, Shadows +50, Dehaze +20/+40,
Whites +50/+100 and the mixed warm-WB/contrast/shadows/highlights case. Two tone
comparisons still fail: Shadows +100 has extra RGB MAE **0.0241**, and Shadows
+100 with Highlights −100 has **0.0420**, against the **0.020** limit.
These remain separate rendering gaps; no camera offset is fitted to hide them.
That is **15/17 distinct settings** passing for independent validation and
**17/17** for the calibration photo, or **34/36** comparisons including the
repeated default in each suite. This supports the X-T5 WB correction without
claiming complete rendered parity or additional body/firmware coverage.

The prior Sony investigation remains separate: A7 II baseline +0.35 EV and per-file
WB were already in #481; the 5421 K versus Lightroom 5400 K difference remains
unexplained. The tree image was a Shadows calibration case (about 0.50 → 1.22 EV,
Camera Raw 1.34), not independent validation. The other Sony under-lift (~0.44 EV),
Nikon 1.56 versus 1.65 EV, and the slightly regressed mixed chart remain engine
limitations. This calibration change does not claim to resolve them.

## Verification record

The candidate source is pinned at `cdcad0c92f077b084ab9d6899e4f03f047de780f`.
[Numeric render results](../data/camera-calibration-validation.json) retain the
baseline/candidate binary hashes, source hashes, reference/XMP hashes, verified
settings, per-case metrics and all framing failures. References remain frozen.

Passed locally on macOS: Rust 1.99 stable formatting, workspace clippy with
warnings denied, workspace tests (including synthetic/frozen rendering tests),
model all-features tests, no-default-features clippy, Rust 1.98.1 all-target check,
seven explicit GPU tests, architecture and crate-closure checks, cargo-deny,
shadow-context data validation, 26 Python tests and the actual-file native/DNG
regression including saved settings and the explicit baseline action.

Linux/Windows packaging and installation, PostgreSQL statement preparation and
the complete private photographic corpus were not run locally. The two X-T5
photos do not establish all scene, body, firmware, ISO or recording-mode coverage.
The original framing failures remain in the historical numeric record. The
follow-up record fixes their decoding cause rather than waiving the gate; its
two remaining tone failures are retained explicitly.

The crop follow-up reran the same available local checks successfully, including
all seven GPU checks, the native/DNG calibration regression and a hash-verified
crop regression through RAWmakase, LibRaw full-size and half-size decode. The crop
regression failed at 6000×3997 before the fix. Synthetic tests cover aligned and
unaligned origins, contained crops, zero sizes, overflow and invalid bounds.

Switching baseline/candidate builds in a shared target directory exposed a stale
native archive: a pinned executable rejected the Rust/C++ metadata layout before
opening images. That run was discarded. The native package was cleaned and the
candidate rebuilt; accepted measurements use the rebuilt binary recorded above.
