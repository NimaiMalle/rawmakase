#!/usr/bin/env python3
"""Half-size checks of the classified test dataset: RAWmakase's half-size
decode of each RAW against its own full decode reduced to that size, and its
time against LibRaw's half-size path (`rawmakase half-check --libraw`, which
holds the gates: block CIEDE2000 mean at most 0.3 and 95th percentile at most
1, Laplacian energy ratio within 0.8 to 1.25, time at most LibRaw's + 10%).

The develop recordings hold LibRaw's half-size decodes, which RAWmakase's are
not, so `dataset-develop.py check --half size` compares them by size only and
this is the check of their pixels.

  python3 scripts/corpus/dataset-half.py                  # 3 files per make and format
  python3 scripts/corpus/dataset-half.py --per-group 0    # every file (hours)
  python3 scripts/corpus/dataset-half.py --manifest m.json --report half.json

Files run one at a time, so the times are not disturbed; run it on a quiet
machine. Small RAWs, which develop at full size in both modes, are listed
but not checked. Exit status: 0 every file passes, 1 a file fails a gate or
errs, 2 the run could not be made.
"""
import argparse
import importlib.util
import json
import os
import random
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

spec = importlib.util.spec_from_file_location('fingerprints', Path(__file__).with_name('dataset-fingerprints.py'))
fingerprints = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fingerprints)
Setup = fingerprints.Setup
ROOT = fingerprints.ROOT
GATES = {'colour mean': ('de_mean', lambda v: v <= 0.3), 'colour p95': ('de_p95', lambda v: v <= 1.0),
         'detail': ('detail', lambda v: 0.8 <= v <= 1.25)}
MAX_TIME = 1.1
FILE_TIMEOUT = 900


def failures(check):
    """The gates a `half-check` line fails (as the binary judges them)."""
    out = []
    if check.get('error'):
        out.append('error')
    # Small RAWs (not halved) skip the image gates, not the time.
    if check.get('halved') is not False:
        for name, (key, ok) in GATES.items():
            if check.get(key) is not None and not ok(check[key]):
                out.append(name)
    own, libraw = check.get('seconds'), check.get('libraw_seconds')
    if own is not None and libraw and own > MAX_TIME * libraw:
        out.append('time')
    return out


def select(samples, per_group, seed):
    if per_group <= 0:
        return list(samples.values())
    groups = defaultdict(list)
    for s in sorted(samples.values(), key=lambda s: s['sha256']):
        groups[(s['make'] or '?', (s['ext'] or '').lower())].append(s)
    rng = random.Random(seed)
    return [s for g in sorted(groups) for s in rng.sample(groups[g], min(per_group, len(groups[g])))]


def summarize(results, public, samples):
    """The report: counts per gate, failing files by group, and the worst values."""
    checked = [r for r in results if r.get('halved')]
    fails = {r['sha256']: failures(r) for r in results}
    report = {
        'files': len(results),
        'checked': len(checked),
        'not halved': sum(1 for r in results if r.get('halved') is False),
        'errors': sum(1 for r in results if r.get('error')),
        'failing': Counter(f for v in fails.values() for f in v),
        'passing': sum(1 for v in fails.values() if not v),
    }
    for key in ('de_mean', 'de_p95', 'detail'):
        vals = sorted(r[key] for r in checked if r.get(key) is not None)
        if vals:
            report[key] = {'median': vals[len(vals) // 2], 'p90': vals[int(0.9 * (len(vals) - 1))],
                           'min': vals[0], 'max': vals[-1]}
    ratios = sorted(r['seconds'] / r['libraw_seconds'] for r in results if r.get('seconds') and r.get('libraw_seconds'))
    if ratios:
        report['time ratio'] = {'median': ratios[len(ratios) // 2], 'p90': ratios[int(0.9 * (len(ratios) - 1))],
                                'max': ratios[-1], 'own seconds': sum(r['seconds'] for r in results if r.get('seconds')),
                                'libraw seconds': sum(r['libraw_seconds'] for r in results if r.get('libraw_seconds'))}
    groups = defaultdict(list)
    for r in results:
        if fails[r['sha256']]:
            s = samples[r['sha256']]
            name = r['sha256'][:12] if public and not fingerprints.committed(s) else f"{r['sha256'][:12]} {s['model']}"
            groups[f"{s['make']} .{(s['ext'] or '').lower()}"].append((name, fails[r['sha256']],
                                                                        {k: r.get(k) for k in ('de_mean', 'de_p95', 'detail')}))
    report['failing files'] = dict(sorted(groups.items()))
    return report


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--dataset', type=Path, default=os.environ.get('RAWMAKASE_DATASET'))
    p.add_argument('--rawmakase', type=Path, default=ROOT / 'target/release/rawmakase')
    p.add_argument('--manifest', type=Path, help='a dataset.py manifest; default: every file')
    p.add_argument('--per-group', type=int, default=3, help='files per make and format (0: every file)')
    p.add_argument('--seed', type=int, default=1)
    p.add_argument('--no-libraw', action='store_true', help="leave LibRaw's time out")
    p.add_argument('--public', action='store_true', help='name only raw.pixls.us CC0 files')
    p.add_argument('--report', type=Path, help='also write the report as JSON')
    args = p.parse_args()
    try:
        if args.dataset is None:
            raise Setup('Set RAWMAKASE_DATASET or pass --dataset')
        samples = fingerprints.load_samples(args.dataset, args.manifest)
        chosen = select(samples, args.per_group, args.seed)
        results = []
        for i, s in enumerate(chosen):
            command = [str(args.rawmakase), 'half-check', *([] if args.no_libraw else ['--libraw']), str(s['file'])]
            try:
                out = subprocess.run(command, capture_output=True, text=True, timeout=FILE_TIMEOUT)
                line = json.loads(out.stdout) if out.returncode == 0 else {'error': f'exit status {out.returncode}'}
            except (subprocess.TimeoutExpired, json.JSONDecodeError) as e:
                line = {'error': type(e).__name__}
            line['sha256'] = s['sha256']
            line.pop('path', None)
            results.append(line)
            print(f'\r{i + 1}/{len(chosen)} files', end='', file=sys.stderr, flush=True)
        print(file=sys.stderr)
        report = summarize(results, args.public, samples)
        if args.report:
            args.report.write_text(json.dumps({'report': report, 'files': results}, indent=1, default=list) + '\n')
        print(json.dumps(report, indent=1, default=list))
        return 1 if report['passing'] < report['files'] else 0
    except Setup as e:
        print(f'error: {e}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
