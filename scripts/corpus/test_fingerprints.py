"""Synthetic tests of the decode fingerprint comparison; no RAWs needed."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('fingerprints', Path(__file__).with_name('dataset-fingerprints.py'))
fingerprints = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fingerprints)


class Compare(unittest.TestCase):
    old = {'version': 2, 'decoder': 'a', 'header': {'metadata': {'wb': [2, 1, 1.5], 'make': 'X'}},
           'sensor': {'white': 4095}}

    def test_nested_fields_compare_by_dotted_name(self):
        new = {'version': 2, 'decoder': 'b', 'header': {'metadata': {'wb': [2, 1, 1.25], 'make': 'X'}},
               'sensor': {'white': 4095, 'cfa': 'RG/gB'}}
        self.assertEqual(fingerprints.compare(self.old, new), [
            ('header.metadata.wb', [2, 1, 1.5], [2, 1, 1.25]),
            ('sensor.cfa', None, 'RG/gB'),
        ])

    def test_an_approved_delta_is_no_change_but_another_value_is(self):
        new = {**self.old, 'sensor': {'white': 4000}}
        deltas = {('h', 'sensor.white'): (4095, 4000)}
        self.assertEqual(fingerprints.compare(self.old, new, 'h', deltas), [])
        self.assertEqual(len(fingerprints.compare(self.old, new, 'other', deltas)), 1)
        moved = {**self.old, 'sensor': {'white': 3900}}
        self.assertEqual(len(fingerprints.compare(self.old, moved, 'h', deltas)), 1)

    def test_deltas_need_evidence_and_approval(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / 'deltas.toml'
            path.write_text('[[delta]]\nsha256 = "h"\nfield = "sensor.white"\nbefore = 1\nafter = 2\n')
            with self.assertRaises(fingerprints.Setup):
                fingerprints.load_deltas(path)
            path.write_text(path.read_text() + 'evidence = "e"\napproval = "a"\n')
            self.assertEqual(fingerprints.load_deltas(path), {('h', 'sensor.white'): (1, 2)})
            path.write_text(path.read_text().replace('approval = "a"', 'approval = "pending"'))
            self.assertEqual(fingerprints.load_deltas(path), {})
            # An absent field (null in the fingerprint) is { absent = true }.
            path.write_text('[[delta]]\nsha256 = "h"\nfield = "sensor.black_pattern.rows"\n'
                            'before = { absent = true }\nafter = 1\nevidence = "e"\napproval = "a"\n')
            deltas = fingerprints.load_deltas(path)
            self.assertEqual(deltas, {('h', 'sensor.black_pattern.rows'): (None, 1)})
            old = {'sensor': {'black_pattern': None}}
            new = {'sensor': {'black_pattern': {'rows': 1}}}
            self.assertEqual(fingerprints.compare(old, new, 'h', deltas), [])
            path.write_text(path.read_text().replace('{ absent = true }', '{ missing = 1 }'))
            with self.assertRaises(fingerprints.Setup):
                fingerprints.load_deltas(path)
        # The repository's own deltas load: every entry complete, one per field.
        import tomllib
        entries = tomllib.loads(fingerprints.DELTAS.read_text()).get('delta', [])
        approved = [d for d in entries if d['approval'] != 'pending']
        self.assertEqual(len(fingerprints.load_deltas()), len(approved))


spec = importlib.util.spec_from_file_location('develop', Path(__file__).with_name('dataset-develop.py'))
develop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(develop)


class DevelopCheck(unittest.TestCase):
    def test_a_baseline_file_missing_from_the_dataset_is_a_problem_unless_a_manifest_selects(self):
        import types
        with tempfile.TemporaryDirectory() as d:
            dataset = Path(d)
            sample = {'sha256': 'a' * 64, 'source': 's', 'make': 'X', 'model': 'Y', 'ext': 'raw', 'path': 'p.raw',
                      'license': 'x'}
            fingerprints.write_baseline(dataset / 'baselines/develop/s.json', {},
                                        {h: {'sha256': h, 'full': {}} for h in ('a' * 64, 'b' * 64)})
            for manifest, status in ((None, 1), (Path('m.json'), 0)):
                args = types.SimpleNamespace(dataset=dataset, manifest=manifest, public=False)
                result = develop.check(args, {'a' * 64: sample}, {'a' * 64: {'sha256': 'a' * 64, 'full': {}}}, [])
                self.assertEqual(result, status)

    def test_half_size_compares_drafts_by_size_only(self):
        import types
        with tempfile.TemporaryDirectory() as d:
            dataset = Path(d)
            h = 'a' * 64
            sample = {'sha256': h, 'source': 's', 'make': 'X', 'model': 'Y', 'ext': 'raw', 'path': 'p.raw',
                      'license': 'x'}
            old = {'sha256': h, 'full': {'pixels_sha256': '1'}, 'half': {'size': [2, 1], 'pixels_sha256': '1'}}
            fingerprints.write_baseline(dataset / 'baselines/develop/s.json', {}, {h: old})
            other_pixels = {**old, 'half': {'size': [2, 1], 'pixels_sha256': '2'}}
            other_size = {**old, 'half': {'size': [1, 1], 'pixels_sha256': '1'}}
            other_full = {**old, 'full': {'pixels_sha256': '2'}}
            for half, new, status in (('all', other_pixels, 1), ('size', other_pixels, 0),
                                      ('size', other_size, 1), ('size', other_full, 1)):
                args = types.SimpleNamespace(dataset=dataset, manifest=Path('m.json'), public=False, half=half)
                self.assertEqual(develop.check(args, {h: sample}, {h: new}, []), status, (half, new))


spec = importlib.util.spec_from_file_location('half', Path(__file__).with_name('dataset-half.py'))
half = importlib.util.module_from_spec(spec)
spec.loader.exec_module(half)


class HalfCheck(unittest.TestCase):
    def test_each_gate_fails_on_its_own(self):
        ok = {'halved': True, 'de_mean': 0.2, 'de_p95': 0.9, 'detail': 1.1, 'seconds': 1.0, 'libraw_seconds': 1.0}
        self.assertEqual(half.failures(ok), [])
        for change, gate in (({'de_mean': 0.31}, 'colour mean'), ({'de_p95': 1.01}, 'colour p95'),
                             ({'detail': 1.26}, 'detail'), ({'detail': 0.79}, 'detail'),
                             ({'seconds': 1.11}, 'time'), ({'error': 'x'}, 'error')):
            self.assertEqual(half.failures({**ok, **change}), [gate], change)
        # Small RAWs, not halved, are not judged; without LibRaw's time, time is not either.
        self.assertEqual(half.failures({'halved': False, 'de_p95': 9}), [])
        # ... but their time is.
        self.assertEqual(half.failures({'halved': False, 'de_p95': 9, 'seconds': 2.0, 'libraw_seconds': 1.0}),
                         ['time'])
        self.assertEqual(half.failures({**ok, 'libraw_seconds': None, 'seconds': 9}), [])

    def test_the_report_counts_and_names_failing_files(self):
        samples = {h: {'sha256': h, 'make': 'X', 'model': 'Y', 'ext': 'raw', 'source': 'own', 'license': 'x'}
                   for h in ('a' * 64, 'b' * 64)}
        results = [{'sha256': 'a' * 64, 'halved': True, 'de_mean': 0.1, 'de_p95': 0.5, 'detail': 1.0},
                   {'sha256': 'b' * 64, 'halved': True, 'de_mean': 0.5, 'de_p95': 2.0, 'detail': 1.0}]
        report = half.summarize(results, True, samples)
        self.assertEqual((report['files'], report['passing']), (2, 1))
        self.assertEqual(report['failing'], {'colour mean': 1, 'colour p95': 1})
        # A private file is named by its hash alone with --public.
        self.assertEqual(report['failing files']['X .raw'][0][0], 'b' * 12)


if __name__ == '__main__':
    unittest.main()
