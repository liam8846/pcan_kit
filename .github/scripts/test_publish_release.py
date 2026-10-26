"""驗證既有正式發行檔不可覆寫，失敗上傳不得提前公開；不呼叫真實 GitHub。"""

import json
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from publish_release import publish


class PublishReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.sha = 'a' * 40
        for name in ('pcan-kit-v0.1.0-x86_64-unknown-linux-gnu.tar.gz',
                     'pcan-kit-v0.1.0-x86_64-pc-windows-msvc.zip', 'release-notes.md'):
            (self.root / name).write_bytes(b'fixture')

    def view(self, draft=True, sha=None):
        return subprocess.CompletedProcess([], 0, json.dumps({
            'isDraft': draft, 'targetCommitish': sha or self.sha,
        }))

    def test_published_release_or_different_commit_is_never_modified(self):
        for response in (self.view(draft=False), self.view(sha='b' * 40)):
            with self.subTest(response=response), patch('publish_release.subprocess.run', return_value=response) as run:
                with self.assertRaises(ValueError):
                    publish(self.root, 'v0.1.0', self.sha, '0.1.0')
                self.assertEqual(run.call_count, 1)

    def test_new_release_is_created_as_draft_and_only_then_uploaded_and_published(self):
        missing = subprocess.CompletedProcess([], 1, '', 'release not found')
        with patch('publish_release.subprocess.run', side_effect=[missing, None, None, None]) as run:
            publish(self.root, 'v0.1.0', self.sha, '0.1.0')
            commands = [call.args[0] for call in run.call_args_list]
            self.assertEqual([command[2] for command in commands], ['view', 'create', 'upload', 'edit'])
            self.assertIn('--draft', commands[1])
            self.assertIn('--verify-tag', commands[1])
            self.assertIn('--draft=false', commands[-1])

    def test_same_commit_draft_can_resume_without_creating_another_release(self):
        with patch('publish_release.subprocess.run', side_effect=[self.view(), None, None]) as run:
            publish(self.root, 'v0.1.0', self.sha, '0.1.0')
            self.assertEqual([call.args[0][2] for call in run.call_args_list], ['view', 'upload', 'edit'])
            self.assertEqual(len((self.root / 'SHA256SUMS').read_text().splitlines()), 2)

    def test_upload_failure_leaves_draft_unpublished(self):
        with patch('publish_release.subprocess.run', side_effect=[
            self.view(), subprocess.CalledProcessError(1, ['gh', 'release', 'upload']),
        ]) as run:
            with self.assertRaises(subprocess.CalledProcessError):
                publish(self.root, 'v0.1.0', self.sha, '0.1.0')
            self.assertEqual(run.call_count, 2)

    def test_missing_asset_or_invalid_tag_is_rejected_before_any_github_call(self):
        with patch('publish_release.subprocess.run') as run:
            with self.assertRaises(ValueError):
                publish(self.root, 'v0.1.1', self.sha, '0.1.0')
            with self.assertRaises(ValueError):
                publish(self.root, 'v0.1.0', 'short', '0.1.0')
            (self.root / 'release-notes.md').unlink()
            with self.assertRaises(FileNotFoundError):
                publish(self.root, 'v0.1.0', self.sha, '0.1.0')
            run.assert_not_called()
