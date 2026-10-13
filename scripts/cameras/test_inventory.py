"""Evidence classification must not promote a sample into a camera-wide fix."""
import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location('inventory', Path(__file__).with_name('inventory.py'))
i = importlib.util.module_from_spec(spec)
spec.loader.exec_module(i)


class InventoryTests(unittest.TestCase):
    def fact(self, **tags):
        return dict(sha256='a' * 64, source='local', tags=dict(
            Make='Canon', Model='Canon Test', DNGVersion='1 4 0 0',
            CameraCalibrationSig='com.adobe', ProfileCalibrationSig='com.adobe',
            CameraCalibration1='0.9 0 0 0 1 0 0 0 1.1',
            CameraCalibration2='0.9 0 0 0 1 0 0 0 1.1', **tags))

    def test_supported_mismatch_is_not_evidence_for_a_fixed_gain(self):
        report = i.inventory(dict(converter='test', samples=[self.fact()]),
                             [dict(make='Canon', model='Test', baseline_exposure=.3)], [])
        sample = report['cameras'][0]['samples'][0]
        self.assertEqual(sample['category'], 'supported-native-gap')
        self.assertEqual(sample['measured'], [.9, 1., 1.1])
        self.assertEqual(sample['applied'], [1., 1., 1.])
        self.assertEqual(sample['correction_scope'], 'unresolved')
        self.assertEqual(sample['baseline']['status'], 'missing-evidence')
        self.assertIsNone(sample['baseline']['converter_ev'])

    def test_collects_all_blockers_instead_of_only_first_error(self):
        tags = self.fact()['tags']
        tags.update(AnalogBalance='2 1 1', CameraCalibration1='1 .1 0 0 1 0 0 0 1',
                    ProfileCalibrationSig='other')
        blockers = i.transformation_issues(tags)
        self.assertIn('non-identity-analog-balance', blockers)
        self.assertIn('non-diagonal-matrix', blockers)
        self.assertIn('illuminant-dependent-matrices', blockers)
        self.assertIn('signature-mismatch', blockers)

    def test_multiple_photos_do_not_establish_different_bodies(self):
        a = self.fact(); b = self.fact(); b['sha256'] = 'b' * 64
        report = i.inventory(dict(converter='test', samples=[a, b]), [], [])
        camera = report['cameras'][0]
        self.assertEqual(camera['sample_count'], 2)
        self.assertEqual(camera['independent_body_count'], None)
        self.assertEqual(camera['samples'][0]['correction_scope'], 'unresolved')

    def test_baseline_difference_is_separate_and_not_an_offset_proposal(self):
        report = i.inventory(dict(converter='test', samples=[self.fact(BaselineExposure=.5)]),
                             [dict(make='Canon', model='Test', baseline_exposure=.3)], [])
        baseline = report['cameras'][0]['samples'][0]['baseline']
        self.assertAlmostEqual(baseline['raw_tag_minus_table_ev'], .2)
        self.assertEqual(baseline['status'], 'requires-normalization-and-render-validation')
        self.assertNotIn('proposed', baseline)

    def test_identity_without_camera_matrices_does_not_require_a_signature(self):
        self.assertEqual(i.transformation_issues(dict(ProfileCalibrationSig='com.adobe')), [])

    def test_committed_inventory_is_reproducible(self):
        import subprocess
        import sys
        import tempfile
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as directory:
            report, document = Path(directory) / 'report.json', Path(directory) / 'inventory.md'
            subprocess.run([sys.executable, str(root / 'scripts/cameras/inventory.py'),
                            '--report', str(report), '--markdown', str(document)],
                           check=True, capture_output=True)
            self.assertEqual(document.read_text(), (root / 'docs/camera-calibration-inventory.md').read_text())

    def test_private_fields_are_not_copied_to_report(self):
        fact = self.fact(); fact['file'] = '/private/photo.CR3'; fact['serial'] = 'secret'
        fact['tags']['SourceFile'] = '/private/photo.dng'
        report = i.inventory(dict(converter='test', samples=[fact]), [], [])
        self.assertNotIn('/private', str(report))
        self.assertNotIn('secret', str(report))


if __name__ == '__main__':
    unittest.main()
