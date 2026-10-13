#!/usr/bin/env python3
"""Classify recorded calibration evidence without installing camera corrections.

The JSON inventory is reproducible from committed facts, the camera table and the
public manifest. Baseline tag/table differences are diagnostics, never proposed
exposure offsets: sensor normalization, recording settings and renders still need
checking. Unknown metadata stays unknown, including the number of camera units.
"""
import argparse
from collections import Counter
import hashlib
import importlib.util
import json
from pathlib import Path
import tomllib

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('calibration', Path(__file__).with_name('check-calibration.py'))
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)


def transformation_issues(tags):
    """Inspect every condition; the audit's first error is not a complete diagnosis."""
    issues = []
    try:
        analog = c.numbers(tags, 'AnalogBalance', [1.] * 3)
        if any(v <= 0 for v in analog):
            issues.append('invalid-analog-balance')
        elif analog != [1.] * 3:
            issues.append('non-identity-analog-balance')
        a = c.numbers(tags, 'CameraCalibration1', c.IDENTITY)
        b = c.numbers(tags, 'CameraCalibration2', c.IDENTITY)
        if a != b:
            issues.append('illuminant-dependent-matrices')
        if any(m[j] != 0 for m in (a, b) for j in (1, 2, 3, 5, 6, 7)):
            issues.append('non-diagonal-matrix')
        if any(not 0 < m[j] < 4 for m in (a, b) for j in (0, 4, 8)):
            issues.append('unsupported-diagonal-range')
    except (ValueError, TypeError):
        issues.append('invalid-numerical-metadata')
    has_matrix = 'CameraCalibration1' in tags or 'CameraCalibration2' in tags
    if has_matrix and tags.get('CameraCalibrationSig', '') != tags.get('ProfileCalibrationSig', ''):
        issues.append('signature-mismatch')
    if has_matrix and tags.get('CameraCalibrationSig', '') != 'com.adobe':
        issues.append('non-adobe-signature')
    return issues


def inventory(facts, rows, public):
    sources = {r['sha256']: r for r in public}
    cameras = {}
    statuses = Counter()
    categories = Counter()
    for fact in facts['samples']:
        tags = fact['tags']
        result = c.audit(tags, rows)
        row = c.table_row(rows, result['make'], result['model'])
        make, model = (row['make'], row['model']) if row else (result['make'], result['model'])
        key = (make.casefold(), model.casefold())
        camera = cameras.setdefault(key, dict(make=make, model=model, samples=[], independent_body_count=None))
        status = result['status']
        if status == 'unsupported':
            category = 'unsupported-transformation-or-metadata'
        elif status == 'ok' or result.get('measured') == result.get('applied'):
            category = 'per-file-calibration' if row and ('sony_daylight_reference' in row or 'fujifilm_daylight_reference' in row) else 'supported-match'
        else:
            category = 'supported-native-gap'
        baseline = tags.get('BaselineExposure')
        table_ev = row.get('baseline_exposure') if row else None
        public_source = sources.get(fact['sha256'])
        if 'url' in fact and public_source is None:
            raise ValueError('public sample is missing from manifest')
        sample = dict(
            sha256=fact['sha256'], source='public' if public_source else fact.get('source', 'unknown'),
            status=status, category=category,
            correction_scope='existing-per-file-rule' if category == 'per-file-calibration' else
                             'existing-supported-rule' if category == 'supported-match' else 'unresolved',
            transformation_issues=transformation_issues(tags),
            measured=result.get('measured'), applied=result.get('applied'),
            recording=dict(mode=public_source.get('mode') if public_source else None,
                           iso=tags.get('ISO'), firmware=fact.get('native_metadata', {}).get('FirmwareVersion')),
            baseline=dict(converter_ev=baseline, table_ev=table_ev,
                          table_source=row.get('source') if row else None,
                          raw_tag_minus_table_ev=round(float(baseline) - table_ev, 6)
                          if baseline is not None and table_ev is not None else None,
                          status='missing-evidence' if baseline is None else
                                 'requires-normalization-and-render-validation'),
        )
        sample['transformation'] = {k: tags.get(k) for k in (
            'CameraCalibration1', 'CameraCalibration2', 'CameraCalibrationSig',
            'ProfileCalibrationSig', 'AnalogBalance', 'NativeDaylightWB',
            'CalibrationIlluminant1', 'CalibrationIlluminant2',
            'PhotometricInterpretation', 'SamplesPerPixel')}
        sample['evidence_role'] = fact.get('evidence_role', 'not-recorded')
        sample['capture_revision'] = fact.get('capture_revision')
        if public_source:
            if fact.get('url') != public_source['url']:
                raise ValueError('public source URL disagrees with manifest')
            sample['url'] = public_source['url']
        for field in ('reason', 'proposed_reference'):
            if field in result:
                sample[field] = result[field]
        camera['samples'].append(sample)
        statuses[status] += 1
        categories[category] += 1
    ordered = []
    for key in sorted(cameras):
        camera = cameras[key]
        camera['samples'].sort(key=lambda s: s['sha256'])
        camera['sample_count'] = len(camera['samples'])
        camera['categories'] = dict(sorted(Counter(s['category'] for s in camera['samples']).items()))
        camera['needs_more_evidence'] = any(s['correction_scope'] == 'unresolved' for s in camera['samples'])
        ordered.append(camera)
    return dict(schema=1, converter=facts.get('converter'), measured_date=facts.get('checked'),
                sample_count=sum(statuses.values()), camera_count=len(ordered),
                audit_status_counts=dict(sorted(statuses.items())), category_counts=dict(sorted(categories.items())),
                cameras=ordered)


def markdown(report):
    """One model group per row; hashes identify individual measurements in facts."""
    lines = ['# Camera calibration inventory', '',
             'Generated by `scripts/cameras/inventory.py`; do not edit the tables by hand.', '',
             f"Converter: {report['converter']}. Recorded date: {report['measured_date']}.",
             'The applied column uses the current camera table. Capture revisions are recorded per sample in the facts.', '',
             f"{report['sample_count']} samples, {report['camera_count']} make/model pairs. "
             'These are sample results, not counts of visibly defective cameras.', '',
             'See [assessment and limitations](camera-calibration-audit.md). Full matrices, signatures, '
             'native WB metadata and full hashes are in [the facts](../data/camera-calibration-facts.json). '
             'Public hash links point to the original CC0 RAWs. Anonymous local hashes have no image links.', '',
             'A match validates only the sampled metadata. Body count and firmware coverage are unknown. '
             'Every supported native gap remains unresolved; no fixed gain is inferred from a sample. '
             'Recording modes are the public source labels, not a claim about decoded storage.', '']
    labels = {
        'supported-match': 'Supported calibration matches',
        'per-file-calibration': 'Supported per-file calibration',
        'supported-native-gap': 'Supported DNG transformation; native correction unresolved',
        'unsupported-transformation-or-metadata': 'Unsupported transformation or metadata',
    }
    def fmt(value):
        if value is None:
            return 'unknown'
        if isinstance(value, list):
            return '[' + ', '.join(f'{v:g}' for v in value) + ']'
        return str(value).replace('|', r'\|')
    def source(sample):
        label = sample['sha256'][:12]
        return f"[{label}](<{sample['url']}>)" if 'url' in sample else f'`{label}` (local)'
    for category, title in labels.items():
        lines += [f'## {title}', '',
                  '| Camera (total samples) | Sample / role | ISO / mode | WB measured → applied |',
                  '| --- | --- | --- | --- |']
        for camera in report['cameras']:
            for sample in camera['samples']:
                if sample['category'] != category:
                    continue
                recording = sample['recording']
                if sample['measured'] is None:
                    transform = sample['transformation']
                    values = f"CC1 {fmt(transform['CameraCalibration1'])}; CC2 {fmt(transform['CameraCalibration2'])}; AB {fmt(transform['AnalogBalance'])}; " + ', '.join(sample['transformation_issues'])
                else:
                    values = f"{fmt(sample['measured'])} → {fmt(sample['applied'])}"
                lines.append(f"| {camera['make']} {camera['model']} ({camera['sample_count']}) | "
                             f"{source(sample)} / {sample['evidence_role']} | "
                             f"{fmt(recording['iso'])} / {fmt(recording['mode'])} | {values} |")
        lines.append('')
    lines += ['## Baseline exposure: separate, unnormalized audit', '',
              'These are raw converter tags and static table rows, **not rendered exposure errors or proposed offsets**. '
              'Canon Highlight Tone Priority, Fujifilm exposure shifts, extended ISO, sensor black/white normalization '
              'and profile offsets must be resolved first. A missing table row uses runtime fallback for native RAWs; '
              'DNGs use their own tag. No runtime fallback is inferred here. Existing edits retain saved baselines.', '',
              '| Camera | Sample | Converter EV | Table EV / source | Tag − table EV |',
              '| --- | --- | --- | --- | --- |']
    for camera in report['cameras']:
        for sample in camera['samples']:
            b = sample['baseline']
            lines.append(f"| {camera['make']} {camera['model']} | {source(sample)} | "
                         f"{fmt(b['converter_ev'])} | {fmt(b['table_ev'])} / {fmt(b['table_source'])} | "
                         f"{fmt(b['raw_tag_minus_table_ev'])} |")
    lines += ['', '## Reproduction inputs', '']
    for name, digest in report['input_sha256'].items():
        lines.append(f'- {name}: `{digest}`')
    return '\n'.join(lines) + '\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--markdown', type=Path)
    args = parser.parse_args()
    paths = {'facts': ROOT / 'data/camera-calibration-facts.json',
             'table': ROOT / 'data/cameras.toml', 'public_manifest': ROOT / 'tests/corpus/pixls.json'}
    report = inventory(json.loads(paths['facts'].read_text()),
                       tomllib.loads(paths['table'].read_text())['camera'],
                       json.loads(paths['public_manifest'].read_text())['files'])
    report['input_sha256'] = {k: hashlib.sha256(p.read_bytes()).hexdigest() for k, p in paths.items()}
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    if args.markdown:
        args.markdown.write_text(markdown(report))
    print(json.dumps({k: report[k] for k in ('sample_count', 'camera_count', 'audit_status_counts', 'category_counts')}))


if __name__ == '__main__':
    main()
