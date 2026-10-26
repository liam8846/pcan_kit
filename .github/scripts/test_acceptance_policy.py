"""驗收範圍規則的回歸測試。"""

import unittest

from acceptance_policy import affected_files, requires_acceptance


class AcceptancePolicyTests(unittest.TestCase):
    def test_runtime_and_dependency_changes_require_acceptance(self):
        for path in (
            "Cargo.lock", "Cargo.toml", "rust-toolchain.toml",
            "crates/pcan-basic/src/rx/linux.rs", "crates/pcan-basic/src/rx/windows.rs",
            "crates/pcan-basic-sys/src/api.rs", "crates/pcan-core/src/frame.rs",
            "crates/pcan-link/src/link.rs", "crates/pcan-kit/src/open.rs",
            "crates/pcan-kit/examples/monitor.rs", "crates/pcan-socketcan/Cargo.toml",
        ):
            with self.subTest(path=path):
                self.assertTrue(requires_acceptance(path))

    def test_docs_ci_tests_and_socketcan_only_do_not_require_peak(self):
        for path in (
            "README.md", "CONTRIBUTING.md", ".github/workflows/ci.yml",
            "crates/pcan-link/tests/router.rs", "crates/pcan-basic/tests/rx_lifecycle.rs",
            "crates/pcan-socketcan/src/socket.rs",
        ):
            with self.subTest(path=path):
                self.assertFalse(requires_acceptance(path))

    def test_renames_and_all_pages_are_checked(self):
        pages = [[{"filename": "README.md"}], [{
            "filename": "archive/old.rs",
            "previous_filename": "crates/pcan-basic/src/channel.rs",
        }]]
        self.assertEqual(affected_files(pages), ["crates/pcan-basic/src/channel.rs"])


if __name__ == "__main__":
    unittest.main()
