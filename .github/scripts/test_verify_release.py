"""發版門檻回歸測試。"""

import unittest

from verify_release import has_successful_ci, release_notes, require_hardware_review, validate_tag


class ReleasePolicyTests(unittest.TestCase):
    def test_only_matching_stable_version_is_allowed(self):
        validate_tag("v0.1.0", "0.1.0")
        for tag in ("v0.1.1", "v0.1.0-rc.1", "vlatest", "0.1.0"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                validate_tag(tag, "0.1.0")

    def test_ci_must_match_exact_commit_branch_event_and_workflow(self):
        run = {"head_sha": "abc", "head_branch": "master", "event": "push",
               "path": ".github/workflows/ci.yml", "status": "completed",
               "conclusion": "success"}
        self.assertTrue(has_successful_ci([{"workflow_runs": []}, {"workflow_runs": [run]}], "abc"))
        for key, value in {"head_sha": "old", "head_branch": "fix/test",
                           "event": "pull_request", "path": ".github/workflows/docs.yml",
                           "status": "in_progress", "conclusion": "failure"}.items():
            with self.subTest(key=key):
                self.assertFalse(has_successful_ci([{"workflow_runs": [dict(run, **{key: value})]}], "abc"))
        self.assertFalse(has_successful_ci([{"workflow_runs": []}], "abc"))

    def test_latest_ci_attempt_must_succeed(self):
        success = {"id": 10, "run_attempt": 1, "head_sha": "abc", "head_branch": "master",
                   "event": "push", "path": ".github/workflows/ci.yml",
                   "status": "completed", "conclusion": "success"}
        for newer in (
            {**success, "id": 11, "conclusion": "failure"},
            {**success, "run_attempt": 2, "status": "in_progress", "conclusion": None},
            {**success, "run_attempt": 2, "conclusion": "cancelled"},
        ):
            with self.subTest(newer=newer):
                self.assertFalse(has_successful_ci([{"workflow_runs": [success, newer]}], "abc"))
        self.assertTrue(has_successful_ci([{"workflow_runs": [success,
            {**success, "id": 11, "run_attempt": 1}]}], "abc"))

    def test_unconfigured_environment_cannot_approve_itself(self):
        valid = {"name": "hardware-acceptance", "protection_rules": [
            {"type": "required_reviewers", "reviewers": [{"type": "User", "reviewer": {"id": 1}}]}]}
        require_hardware_review(valid)
        for invalid in ({}, {"name": "hardware-acceptance"},
                        {**valid, "name": "other"},
                        {**valid, "protection_rules": []},
                        {**valid, "protection_rules": [{"type": "required_reviewers", "reviewers": []}]}):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                require_hardware_review(invalid)

    def test_release_notes_select_exact_version(self):
        changelog = "# 變更紀錄\n\n## [0.1.1]\n新版本\n\n## [0.1.0] — 初始版本\n### 功能\n- 首版\n\n## [0.0.1]\n舊版\n"
        notes = release_notes(changelog, "0.1.0")
        self.assertIn("- 首版", notes)
        self.assertNotIn("新版本", notes)
        self.assertNotIn("舊版", notes)
        for text in ("## [0.1.1]\n其他版本", "## [0.1.0]\n\n## [0.0.1]\n其他版本"):
            with self.subTest(text=text), self.assertRaises(ValueError):
                release_notes(text, "0.1.0")


if __name__ == "__main__":
    unittest.main()
