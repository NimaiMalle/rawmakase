#!/usr/bin/env python3
"""Fit the learned Shadows and Highlights operator of the scene tone stage and write its
weights as Rust data (`local_tone_net_data.rs`).

  train   PHOTOS [--out DIR] [--epochs N] [--settings S+100,...]
          PHOTOS is scene-tone-local.py's `photos` folder (dng/, renders/, taps/): Camera
          Raw renders of linear-profile DNGs at each slider position and RAWmakase's
          scene-input and scene-output taps at the renders' size. Synthetic scenes join
          it through `scenes` below (a `<name>.scene` marker in dng/, their scene values
          as taps and their renders as arrays). One network per slider
          position is trained on the luminance gain Camera Raw applies, measured through
          each photo's own default global curve as scene-tone-local.py measures it.
          Writes DIR/<setting>.npz (weights, standardisation, held-out scores).
  export  DIR [--rs crates/.../local_tone_net_data.rs] [--fixture tests/corpus/...json]
          Writes the Rust data file from DIR's networks (standardisation folded into the
          first layer) and a fixture: a synthetic scene's features and gains for the
          engine's equivalence test.
  heldout PHOTOS DIR
          Scores DIR's networks on another `photos` folder.
  scenes  SYNTH PHOTOS
          Adds scene-tone-local.py's synthetic scenes (its `synth` folder) to PHOTOS: the
          scene values clipped at the sensor's white as the scene input, Camera Raw's
          default render as the scene output, its renders at each slider position.

Features (local_tone_net.rs computes the same from the measurement copy, 512 pixels on
the long edge, L = log2 luminance floored at 2^-14):
- B: the guided base level (local_tone.rs: self-guided box filter, radius 0.032, eps 0.5),
  per pixel at the render's size as the engine reads it through the map's a and b.
- Coarse levels: L blurred by three passes of a box of radius 8, 16, 32 and 64 map pixels
  (near-Gaussian), read at the pixel; the network sees them less B.
- Globals: of L blurred by three box passes of radius 2 and of 8, the percentiles 0.1, 0.5,
  1, 2, 5, 10, 25, 50, 75, 90, 95, 98, 99, 99.5, 99.9, the maximum and the minimum; the
  log2 mean luminance; L's 99.9th percentile. The network sees them and B less them.
The network: 77 inputs, three hidden layers of 48 SiLU units, one output, the log2 gain.
"""
import argparse
import importlib.util
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True
HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import camera_raw  # noqa: E402

_spec = importlib.util.spec_from_file_location('scene_tone_local', HERE / 'scene-tone-local.py')
stl = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(stl)

ROOT = HERE.resolve().parents[1]
RS = ROOT / 'crates/rawmakase-engine/src/develop/local_tone_net_data.rs'
YW, FLOOR, MAP = stl.YW, stl.FLOOR, stl.MAP
COARSE = [8, 16, 32, 64]
GLOBAL_RADII = [2, 8]
PCTS = [0.1, 0.5, 1, 2, 5, 10, 25, 50, 75, 90, 95, 98, 99, 99.5, 99.9]
WIDTH, LAYERS = 48, 3
SETTINGS = [f'{c}{v:+d}' for c in 'SH' for v in [-100, -50, -25, 25, 50, 100]]


def box3(a, r):
    """Three passes of local_tone::blur: a near-Gaussian of sigma about r."""
    for _ in range(3):
        a = stl.box(a, r)
    return a


def percentile(a, q):
    """local_tone_net.rs's percentile: the sorted value at index floor((n - 1) q)."""
    v = np.sort(a.ravel())
    return v[int((v.size - 1) * q / 100)]


def globals_of(L512, lin512):
    g = []
    for r in GLOBAL_RADII:
        b = box3(L512, r)
        g += [percentile(b, q) for q in PCTS] + [b.max(), b.min()]
    g += [np.log2(max(lin512.mean(), FLOOR)), percentile(L512, 99.9)]
    return np.array(g, np.float64)


def features(xin):
    """Per-pixel features (H, W, 1 + len(COARSE)) at xin's size and the globals, from the
    512-pixel map of xin (linear ProPhoto, the scene stage's input)."""
    lin = xin @ YW
    L = np.log2(np.maximum(lin, FLOOR))
    h, w = L.shape
    shape = stl.size(xin.shape, MAP)
    small = stl.area(xin, *shape)
    lin512 = small @ YW
    L512 = np.log2(np.maximum(lin512, FLOOR))
    r = max(1, round(stl.RADIUS * max(L512.shape)))
    m, m2 = stl.box(L512, r), stl.box(L512 * L512, r)
    var = np.maximum(m2 - m * m, 0)
    a = var / (var + stl.EPS)
    a, b = stl.box(a, r), stl.box(m - a * m, r)
    B = stl.grid_sample(a, L.shape) * L + stl.grid_sample(b, L.shape)
    coarse = [stl.grid_sample(box3(L512, rc), L.shape) for rc in COARSE]
    return np.stack([B] + coarse, -1), globals_of(L512, lin512)


def design(px, glob):
    """Network inputs from per-pixel features (N, 1 + len(COARSE)) and the globals."""
    B = px[:, :1]
    return np.concatenate([B, px[:, 1:] - B, np.broadcast_to(glob, (len(px), len(glob))), B - glob[None, :]], 1).astype(np.float32)


EXPOSURE = re.compile(r'^E([+-]\d+)$')


class Photo:
    """A photo's scene input, its Camera Raw renders and the derived features. `exposure`
    selects an exposure-shifted view: renders named `E+2`, `E+2S+100`, ... pair with the
    scene input scaled by 2^2, as the scene stage sees it after Exposure."""

    def __init__(self, d, stem, exposure=''):
        self.stem = stem + (f'@{exposure}' if exposure else '')
        self.prefix = exposure
        self.synthetic = (d / 'dng' / f'{stem}.scene').exists()
        # Shifted views have their own engine taps (scene input and default output at
        # that Exposure), so the gain is still defined through the engine's own curve.
        z = np.load(d / 'taps' / f'{self.stem}.npz')
        self.xin, self.xout = z['xin'].astype(np.float64), z['xout'].astype(np.float64)
        self.renders = {}
        for p in list((d / 'renders').glob(f'{stem}-*.tif')) + list((d / 'renders').glob(f'{stem}-*.npy')):
            self.renders[p.stem[len(stem) + 1:]] = p
        lin = self.xin @ YW
        self.L = np.log2(np.maximum(lin, FLOOR))
        self.px, self.glob = features(self.xin)
        self.default = self.read(exposure or 'default')
        o = np.argsort(lin.ravel())
        self.xs, self.ys = lin.ravel()[o], np.maximum.accumulate((self.xout @ YW).ravel()[o])
        self.d = d

    def exposures(self):
        """The exposure-shifted views available for this photo (renders and engine taps)."""
        return sorted(k for k in self.renders if EXPOSURE.match(k) and (self.d / 'taps' / f'{self.stem}@{k}.npz').exists())

    def has(self, setting):
        return self.prefix + setting in self.renders

    def read(self, setting):
        """A render as linear ProPhoto at the taps' size: a TIFF from Camera Raw, or a
        linear array (synthetic scenes keep their renders as half-size float16 arrays)."""
        p = self.renders[setting]
        a = np.load(p).astype(np.float64) if p.suffix == '.npy' else camera_raw.read_linear(p)
        if a.shape[:2] != self.xin.shape[:2]:
            a = stl.area(a, self.xin.shape[1], self.xin.shape[0])
        return a

    def gain(self, setting):
        """Camera Raw's log2 gain at `setting` on scene values, and the valid pixels."""
        r = self.read(self.prefix + setting)
        d, rl = self.default @ YW, r @ YW
        inv = lambda y: np.interp(y, self.ys, self.xs)
        ok = (d > 2 ** -11) & (rl > 2 ** -11) & (self.L > -10)
        after = inv(rl)
        if self.synthetic:
            # Scene values are known exactly, so clipped pixels (which Highlights pulls
            # back below white) are valid samples too.
            g = stl.log_gain(after, self.xin @ YW)
        else:
            ok &= self.default.max(-1) < 0.98
            g = stl.log_gain(after, inv(d))
        # Where Camera Raw's render falls below the engine's default black floor, the
        # inverse lands at the bottom of the curve and the gain is undefined.
        ok &= after > 2 ** -10
        return g, ok


def black_frame(d, stem):
    """A photo whose default render is essentially black (median below 2^-14) has no
    measurable tone response; its renders only add noise to the targets."""
    p = d / 'renders' / f'{stem}-default.tif'
    a = camera_raw.read_linear(p) if p.exists() else np.load(d / 'renders' / f'{stem}-default.npy')
    return float(np.median(a.astype(np.float64) @ YW)) < 2 ** -14


def stems_of(d):
    """The usable photo stems in a photos folder (taps and a default render present)."""
    out = []
    for dng in sorted(list((d / 'dng').glob('*.dng')) + list((d / 'dng').glob('*.scene'))):
        if not (d / 'taps' / f'{dng.stem}.npz').exists():
            print(f'{dng.name}: no taps, left out', flush=True)
            continue
        if not ((d / 'renders' / f'{dng.stem}-default.tif').exists() or (d / 'renders' / f'{dng.stem}-default.npy').exists()):
            print(f'{dng.name}: default render missing, left out', flush=True)
            continue
        if black_frame(d, dng.stem):
            print(f'{dng.name}: default render is black, left out', flush=True)
            continue
        out.append(dng.stem)
    print(f'{len(out)} photos in {d}', flush=True)
    return out


class Samples:
    """Compact training samples for one setting: the five per-pixel features, the photo index
    and the target gain. The 77-wide network input is rebuilt per batch from these and the
    photos' globals, so a photo set of any size costs 24 bytes per sample instead of the
    full-resolution feature maps."""

    def __init__(self):
        self.px, self.pid, self.y = [], [], []

    def add(self, photo, pid, setting, n, rng):
        g, ok = photo.gain(setting)
        idx = np.flatnonzero(ok.ravel())
        if idx.size < 1000:
            return
        sub = rng.choice(idx, min(n, idx.size), replace=False)
        self.px.append(photo.px.reshape(-1, photo.px.shape[-1])[sub].astype(np.float32))
        self.pid.append(np.full(len(sub), pid, np.int32))
        self.y.append(g.ravel()[sub].astype(np.float32))

    def arrays(self):
        return np.concatenate(self.px), np.concatenate(self.pid), np.concatenate(self.y)


def collect(d, stems, settings, per_photo, per_scene, rng):
    """Stream the photos once, keeping only the sampled rows for each setting and the globals."""
    samples = {s: Samples() for s in settings}
    globs, labels = [], []
    for stem in stems:
        p = Photo(d, stem)
        for view in [p] + [Photo(d, stem, e) for e in p.exposures()]:
            pid = len(globs)
            globs.append(view.glob.astype(np.float32))
            labels.append(view.stem)
            n = per_scene if (view.synthetic and per_scene) else per_photo
            for s in settings:
                if view.has(s):
                    samples[s].add(view, pid, s, n, rng)
            del view
        del p
    return samples, np.stack(globs), labels


def design_torch(px, glob):
    """`design` on torch tensors: px (N, 5) and the rows' globals (N, 36)."""
    import torch
    B = px[:, :1]
    return torch.cat([B, px[:, 1:] - B, glob, B - glob], 1)


def train_one(px, pid, y, globs, epochs, seed=0):
    import torch
    torch.manual_seed(seed)
    px, pid, y, globs = (torch.tensor(a) for a in (px, pid, y, globs))
    rng = np.random.default_rng(seed)
    stat = torch.tensor(rng.choice(len(px), min(len(px), 1 << 20), replace=False))
    xs = design_torch(px[stat], globs[pid[stat]])
    mu, sd = xs.mean(0), xs.std(0) + 1e-6
    del xs, stat
    layers, n = [], mu.numel()
    for _ in range(LAYERS):
        layers += [torch.nn.Linear(n, WIDTH), torch.nn.SiLU()]
        n = WIDTH
    layers.append(torch.nn.Linear(n, 1))
    m = torch.nn.Sequential(*layers)
    opt = torch.optim.Adam(m.parameters(), 2e-3)
    bs = 4096
    for _ in range(epochs):
        perm = torch.randperm(len(px))
        for s in range(0, len(px), bs):
            b = perm[s:s + bs]
            x = (design_torch(px[b], globs[pid[b]]) - mu) / sd
            loss = torch.nn.functional.mse_loss(m(x).squeeze(-1), y[b])
            opt.zero_grad()
            loss.backward()
            opt.step()
        for g in opt.param_groups:
            g['lr'] *= 0.8
    weights = [(l.weight.detach().numpy().astype(np.float64), l.bias.detach().numpy().astype(np.float64))
               for l in m if isinstance(l, torch.nn.Linear)]
    # Fold the standardisation into the first layer.
    mu, sd = mu.numpy().astype(np.float64), sd.numpy().astype(np.float64)
    w0, b0 = weights[0]
    weights[0] = (w0 / sd[None, :], b0 - (w0 / sd[None, :]) @ mu)
    return weights


def evaluate(weights, x):
    """The network on inputs (N, 77) with numpy, as local_tone_net.rs computes it."""
    h = x.astype(np.float64)
    for i, (w, b) in enumerate(weights):
        h = h @ w.T + b
        if i < len(weights) - 1:
            h = h / (1 + np.exp(-h))
    return h[:, 0]


def score_photo(weights, photo, setting, rng, n=150000):
    """Per-photo RMS, bias and the no-op RMS of the gain at `setting`."""
    g, ok = photo.gain(setting)
    idx = np.flatnonzero(ok.ravel())
    if idx.size < 1000:
        return None
    sub = rng.choice(idx, min(n, idx.size), replace=False)
    pred = evaluate(weights, design(photo.px.reshape(-1, photo.px.shape[-1])[sub], photo.glob))
    e = pred - g.ravel()[sub]
    return dict(rms=float(np.sqrt(np.mean(e ** 2))), bias=float(e.mean()), noop=float(np.sqrt(np.mean(g.ravel()[sub] ** 2))))


def score_samples(weights, px, pid, y, globs, stems, n=50000):
    """In-sample scores per photo on (a subset of) the training rows."""
    out = {}
    for i in np.unique(pid):
        rows = np.flatnonzero(pid == i)[:n]
        e = evaluate(weights, design(px[rows].astype(np.float64), globs[i].astype(np.float64))) - y[rows]
        out[stems[i]] = dict(rms=float(np.sqrt(np.mean(e ** 2))), bias=float(e.mean()), noop=float(np.sqrt(np.mean(y[rows] ** 2))))
    return out


def save(path, weights, extra):
    np.savez(path, **{f'w{i}': w for i, (w, _) in enumerate(weights)}, **{f'b{i}': b for i, (_, b) in enumerate(weights)}, **extra)


def load(path):
    z = np.load(path, allow_pickle=True)
    return [(z[f'w{i}'], z[f'b{i}']) for i in range(LAYERS + 1)]


def train(args):
    rng = np.random.default_rng(0)
    settings = args.settings.split(',') if args.settings else SETTINGS
    d = args.photos.resolve()
    stems = stems_of(d)
    args.out.mkdir(parents=True, exist_ok=True)
    samples, globs, labels = collect(d, stems, settings, args.per_photo, args.per_scene, rng)
    for setting in settings:
        px, pid, y = samples.pop(setting).arrays()
        print(f'{setting}: {len(px)} samples from {len(np.unique(pid))} photos', flush=True)
        weights = train_one(px, pid, y, globs, args.epochs)
        s = score_samples(weights, px, pid, y, globs, labels)
        print(f'{setting}: in-sample RMS {np.sqrt(np.mean([v["rms"] ** 2 for v in s.values()])):.3f} EV', flush=True)
        save(args.out / f'{setting}.npz', weights, dict(setting=setting, train=json.dumps(s)))
        del px, pid, y


def heldout(args):
    rng = np.random.default_rng(1)
    settings = [p.stem for p in sorted(args.dir.glob('*.npz'))]
    weights = {s: load(args.dir / f'{s}.npz') for s in settings}
    d = args.photos.resolve()
    scores = {s: {} for s in settings}
    for stem in stems_of(d):
        p = Photo(d, stem)
        for s in settings:
            if p.has(s):
                r = score_photo(weights[s], p, s, rng)
                if r:
                    scores[s][stem] = r
        del p
    for setting in settings:
        s = scores[setting]
        if not s:
            continue
        rms = np.sqrt(np.mean([v['rms'] ** 2 for v in s.values()]))
        print(f'{setting}: held-out RMS {rms:.3f} median {np.median([v["rms"] for v in s.values()]):.3f} EV over {len(s)} photos', flush=True)
        for k, v in s.items():
            print(f'    {k[:40]:<40} {v["rms"]:.3f} bias {v["bias"]:+.3f} noop {v["noop"]:.3f}')


def scenes(args):
    synth, d = args.synth.resolve(), args.photos.resolve()
    for sub in ['dng', 'renders', 'taps']:
        (d / sub).mkdir(parents=True, exist_ok=True)
    for seed in stl.SEEDS:
        default = synth / f'scene{seed}-default.npy'
        if not default.exists():
            continue
        image, be = stl.synthetic(seed)
        x = np.minimum(image.astype(np.float64), 2.0 ** be)
        out = np.load(default).astype(np.float64)
        h, w = out.shape[:2]
        x = stl.area(x, w, h)
        np.savez(d / 'taps' / f'scene{seed}.npz', xin=x.astype(np.float32), xout=out.astype(np.float32))
        (d / 'dng' / f'scene{seed}.scene').write_text(f'{synth}\n')
        for p in synth.glob(f'scene{seed}-*.npy'):
            target = d / 'renders' / p.name
            if not target.exists():
                target.symlink_to(p)
        print(f'scene {seed}', flush=True)


def rust_array(name, a):
    flat = ', '.join(f'{v:.6g}' for v in np.asarray(a).ravel())
    return f'    {name}: &[{flat}],\n'


def export(args):
    nets = []
    for setting in SETTINGS:
        p = args.dir / f'{setting}.npz'
        if not p.exists():
            sys.exit(f'{p} missing')
        nets.append((setting, load(p)))
    n_in = nets[0][1][0][0].shape[1]
    text = ['//! The learned Shadows and Highlights operator: one network per slider position,',
            '//! fitted to Camera Raw 18.7 on the training photos by `scripts/corpus/scene-tone-net.py`',
            '//! (the feature definitions are documented there and in local_tone_net.rs). Weights are',
            '//! row-major (outputs x inputs); the input standardisation is folded into the first',
            '//! layer.',
            '#![allow(clippy::excessive_precision, clippy::approx_constant, clippy::unreadable_literal)]',
            'pub(crate) struct Net {', '    pub control: char,', '    pub value: f32,',
            '    pub weights: &\'static [&\'static [f32]],', '    pub biases: &\'static [&\'static [f32]],', '}',
            f'pub(crate) const INPUTS: usize = {n_in};',
            f'pub(crate) const COARSE: [usize; {len(COARSE)}] = {COARSE};',
            f'pub(crate) const GLOBAL_RADII: [usize; {len(GLOBAL_RADII)}] = {GLOBAL_RADII};',
            f'pub(crate) const PERCENTILES: [f32; {len(PCTS)}] = [{", ".join(f"{q / 100:.3f}" for q in PCTS)}];',
            f'pub(crate) const NETS: [Net; {len(nets)}] = [']
    for setting, weights in nets:
        ws = ', '.join('&[' + ', '.join(f'{v:.7g}' for v in w.ravel()) + ']' for w, _ in weights)
        bs = ', '.join('&[' + ', '.join(f'{v:.7g}' for v in b.ravel()) + ']' for _, b in weights)
        text.append(f"    Net {{ control: '{setting[0]}', value: {int(setting[1:]) / 100:.2f}, weights: &[{ws}], biases: &[{bs}] }},")
    text.append('];')
    args.rs.write_text('\n'.join(text) + '\n')
    rustfmt = shutil.which('rustfmt') or str(Path.home() / '.cargo/bin/rustfmt')
    subprocess.run([rustfmt, '--edition', '2024', str(args.rs)], check=True)
    print(f'{args.rs}: {len(nets)} networks', flush=True)
    if args.fixture:
        import probe_scenes
        import probe_dng  # noqa: F401
        image, _ = probe_scenes.scene(3, 384, 256)
        xin = image[:, :, :3]
        px, glob = features(xin)
        rng = np.random.default_rng(0)
        idx = rng.choice(px.shape[0] * px.shape[1], 64, replace=False)
        x = design(px.reshape(-1, px.shape[-1])[idx], glob)
        fixture = dict(scene=3, width=xin.shape[1], height=xin.shape[0], globals=glob.tolist(),
                       pixels=[dict(index=int(i), inputs=row.tolist()) for i, row in zip(idx, x)],
                       gains={s: evaluate(w, x).tolist() for s, w in nets})
        args.fixture.write_text(json.dumps(fixture))
        print(f'{args.fixture}: fixture', flush=True)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='step', required=True)
    s = sub.add_parser('train')
    s.add_argument('photos', type=Path)
    s.add_argument('--out', type=Path, required=True)
    s.add_argument('--epochs', type=int, default=10)
    s.add_argument('--per-photo', type=int, default=120000)
    s.add_argument('--per-scene', type=int, default=30000, help='samples per synthetic scene or probe')
    s.add_argument('--settings', default='')
    s = sub.add_parser('heldout')
    s.add_argument('photos', type=Path)
    s.add_argument('dir', type=Path)
    s = sub.add_parser('scenes')
    s.add_argument('synth', type=Path)
    s.add_argument('photos', type=Path)
    s = sub.add_parser('export')
    s.add_argument('dir', type=Path)
    s.add_argument('--rs', type=Path, default=RS)
    s.add_argument('--fixture', type=Path, default=None)
    args = p.parse_args()
    {'train': train, 'heldout': heldout, 'scenes': scenes, 'export': export}[args.step](args)


if __name__ == '__main__':
    main()
