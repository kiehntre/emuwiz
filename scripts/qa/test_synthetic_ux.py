"""Small deterministic checks for the GUI recovery fixtures."""
import io
import tempfile
import unittest
import zipfile
from pathlib import Path

from synthetic_library import Builder, add_ux_recovery, build_lab, verify_lab


class RecoveryFixtures(unittest.TestCase):
    def test_repeat_builds_have_identical_bytes_and_absent_manual_stays_absent(self):
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            builders = [Builder(Path(root), "tiny", 1, "synthetic") for root in (first, second)]
            for builder in builders:
                add_ux_recovery(builder)
            self.assertEqual(builders[0].entries, builders[1].entries)
            self.assertEqual(len(builders[0].entries), 8)
            for item in builders[0].entries:
                a, b = (Path(root) / item["relative_path"] for root in (first, second))
                if item["kind"] == "virtual":
                    self.assertFalse(a.exists())
                    self.assertFalse(b.exists())
                else:
                    self.assertEqual(a.read_bytes(), b.read_bytes())

    def test_archive_signatures_are_deliberately_truncated_and_cbz_is_readable(self):
        with tempfile.TemporaryDirectory() as root:
            builder = Builder(Path(root), "tiny", 1, "synthetic")
            add_ux_recovery(builder)
            entries = {e["fixture_id"]: e for e in builder.entries}
            def data(key):
                return (Path(root) / entries[key]["relative_path"]).read_bytes()
            self.assertEqual(data("ux.broken.rar"), b"Rar!\x1a\x07\x01\x00")
            self.assertEqual(len(data("ux.broken.7z")), 6)
            with self.assertRaises(zipfile.BadZipFile):
                zipfile.ZipFile(io.BytesIO(data("ux.broken.cbz")))
            with zipfile.ZipFile(io.BytesIO(data("ux.manual.pages"))) as archive:
                self.assertEqual(archive.namelist(), ["page10.png", "page2.png", "page1.png"])
                self.assertIsNone(archive.testzip())

    def test_complete_tiny_lab_still_validates(self):
        with tempfile.TemporaryDirectory(prefix="emuwiz-ux-test-") as temp:
            root = Path(temp) / "lab"
            build_lab(root, "tiny", 1, 0, 0, False)
            _, failures, _, _ = verify_lab(root, write=False)
            self.assertEqual(failures, [])


if __name__ == "__main__":
    unittest.main()
