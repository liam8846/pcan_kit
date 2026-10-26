"""跨平台封裝、輸入檢查與可重現性回歸測試；替身不代表平台二進位驗收。"""

import os
from pathlib import Path
import tarfile
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch
from zipfile import ZipFile

from package_release import DOCUMENTS, EXAMPLES, TARGETS, package


class PackageReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'Cargo.toml').write_text('[workspace.package]\nversion = "0.1.0"\n', encoding='utf-8')
        for document in DOCUMENTS:
            (self.root / document).parent.mkdir(parents=True, exist_ok=True)
            (self.root / document).write_text('首版文件\n', encoding='utf-8')
        for target in TARGETS:
            folder = self.root / 'target' / target / 'release' / 'examples'
            folder.mkdir(parents=True)
            suffix = '.exe' if target.endswith('windows-msvc') else ''
            for example in EXAMPLES:
                (folder / f'{example}{suffix}').write_bytes(b'fixture binary')
            (folder / 'PCANBasic.dll').write_bytes(b'must not distribute vendor library')

    def test_both_archives_contain_only_five_examples_documents_and_licenses(self):
        for target in TARGETS:
            with self.subTest(target=target):
                path = package(self.root, self.root / 'dist', 'v0.1.0', target)
                suffix = '.exe' if target.endswith('windows-msvc') else ''
                names = {f'{example}{suffix}' for example in EXAMPLES} | set(DOCUMENTS)
                prefix = f'pcan-kit-v0.1.0-{target}/'
                if suffix:
                    with ZipFile(path) as archive:
                        self.assertEqual(set(archive.namelist()), {prefix + name for name in names})
                        for info in archive.infolist():
                            self.assertEqual(info.date_time, (1980, 1, 1, 0, 0, 0))
                            self.assertEqual(info.external_attr >> 16 & 0o777,
                                             0o755 if info.filename.endswith('.exe') else 0o644)
                else:
                    with tarfile.open(path) as archive:
                        self.assertEqual(set(archive.getnames()), {prefix + name for name in names})
                        for info in archive.getmembers():
                            self.assertEqual((info.mtime, info.uid, info.gid), (0, 0, 0))
                            self.assertEqual(info.mode, 0o755 if Path(info.name).name in EXAMPLES else 0o644)

    def test_file_timestamps_do_not_change_archives(self):
        for target in TARGETS:
            with self.subTest(target=target):
                first = package(self.root, self.root / 'first', 'v0.1.0', target).read_bytes()
                for source in self.root.rglob('*'):
                    if source.is_file():
                        os.utime(source, (1720000000, 1720000000))
                second = package(self.root, self.root / 'second', 'v0.1.0', target).read_bytes()
                self.assertEqual(first, second)

    def test_missing_example_or_invalid_version_produces_no_archive(self):
        output = self.root / 'dist'
        with self.assertRaises(ValueError):
            package(self.root, output, 'v0.1.1', TARGETS[0])
        self.assertFalse(output.exists())
        (self.root / 'target' / TARGETS[0] / 'release/examples/monitor').unlink()
        with self.assertRaises(FileNotFoundError):
            package(self.root, output, 'v0.1.0', TARGETS[0])
        self.assertFalse(output.exists())

    def test_existing_archive_is_not_overwritten(self):
        path = package(self.root, self.root / 'dist', 'v0.1.0', TARGETS[0])
        previous = path.read_bytes()
        with self.assertRaises(FileExistsError):
            package(self.root, self.root / 'dist', 'v0.1.0', TARGETS[0])
        self.assertEqual(path.read_bytes(), previous)

    def test_failed_packaging_removes_partial_output(self):
        output = self.root / 'dist'
        with patch('package_release.tarfile.open', side_effect=RuntimeError('fixture failure')):
            with self.assertRaises(RuntimeError):
                package(self.root, output, 'v0.1.0', TARGETS[0])
        self.assertEqual(list(output.iterdir()), [])


if __name__ == '__main__':
    unittest.main()
