# Demosaicing

By default RAWmakase demosaics full-size images itself (`crates/rawmakase-native/src/demosaic.rs`). LibRaw only unpacks the sensor data (`ora_cfa_open` / `ora_cfa_copy` in `crates/rawmakase-native/native/raw.cpp`), and still provides metadata, white balance, crops and previews. Black levels (including per-position patterns), white level and camera white balance are applied in RAWmakase. The colour pattern comes from LibRaw's `COLOR()` as a 48×48 tile, which covers Bayer and X-Trans periods.

The algorithm uses the same two passes for both sensor types:

1. **Green** at red and blue sites. On Bayer it blends Hamilton–Adams horizontal and vertical estimates by inverse gradient, clamped to the neighbouring greens. On X-Trans it weights the green neighbours by how well opposite neighbours agree.
2. **Red and blue** from the difference to green of the nearest same-colour samples (radius 1, else 2), weighted by inverse distance.

Borders are mirrored, which keeps the Bayer colour parity. Half-size drafts and files that aren't single-channel Bayer or X-Trans (e.g. Foveon, linear DNG) still use LibRaw.

Preferences > Performance, or `Decode::full(Demosaic::Libraw)` in code, switches to LibRaw's AHD (Bayer) and 1-pass Markesteijn (X-Trans); the environment variable `RAWMAKASE_LIBRAW_DEMOSAIC=1` forces the same for the whole run.

For native X-Trans RAFs, `raw` retains the header's exact recommended crop when
LibRaw rounds its origin inward to a six-pixel CFA boundary. If that rounding
excluded pixels, the adapter extends the decode area back by one complete CFA
period and applies the exact crop inside it. The right/bottom bounds and colour
phase remain unchanged. Invalid crops cannot extend the sensor buffer; contained
crops, DNGs and other sensors keep their existing decode area. LibRaw's saved
geometry is updated before unpacking so full-size and half-size development agree.

Public X-T5 sample 7271 (Sports Finder mode) requests a 6000×4000 crop starting
at raw row 21. LibRaw 0.22.2 exposes row 24 and 3997 rows; the adapter now decodes
from row 18 and crops three rows inside it, retaining all 4000 requested rows.
This fixes the 2048×1364 export to 2048×1365 without rescaling comparison inputs
or changing white balance, exposure or tone processing. `tests/fuji_crop.rs`
checks the hash-verified public sample through all three decode paths.

## Measurements — 2026-09-26 (M1 Pro, loaded machine)

| File | LibRaw develop | RAWmakase develop | 100% crop MAE vs Adobe (LibRaw → RAWmakase) | Fine-detail MAE |
|---|---:|---:|---:|---:|
| X100F DSCF7853 (X-Trans, 24 MP) | 2.33 s | 0.42 s | 0.0186 → 0.0187 | 0.0048 → 0.0050 |
| A7 II DSC00805 (Bayer, 24 MP) | 0.79 s | 0.37 s | 0.0105 → 0.0094 | 0.0072 → 0.0059 |
| A7CR (Bayer, 61 MP) | 1.62 s | 1.04 s | — | — |

The crop metrics compare three 800 px crops at 100% with Lightroom's X100F export and Camera Raw's A7 II render, allowing ±1 px alignment. Fine detail is the error of the image minus a 7 px box blur. Scorecards with the new default: X100F Lightroom references 0.0093 MAE, A7 II Camera Raw references 0.0063.
