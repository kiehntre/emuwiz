"""Offline subprocess regressions for the actual homebrew_discover example.

Build it with scripts/cargo-iso --low-debug build --offline -p archivefs-core
--example homebrew_discover, then pass its path as --example to this script.
Only synthetic metadata in private temporary directories is used.
"""

import argparse
import json
import pathlib
import subprocess
import tempfile
import unicodedata
import unittest


def catalogue(provider="local", project="p", asset="a", platform="Unknown Console"):
    identity = {"provider": provider, "provider_id": project}
    return {
        "schema_version": 1,
        "provider": provider,
        "records": [{
            "project": {"id": identity, "title": "Synthetic", "summary": "Safe summary",
                        "source_page_url": "https://example.org/project"},
            "listings": [{"project": identity, "release": None, "artifacts": [{
                "provider_asset_id": asset, "filename": "game.nes",
                "acquisition": {"source_page_url": "https://example.org/project",
                                "link_url": "https://example.org/game.nes",
                                "mode": "browser_required"},
                "platform_claims": [{"platform": platform,
                                     "evidence": {"release_metadata": "Synthetic claim"}}],
            }]}],
            "observations": [{"source_url": "https://example.org/project",
                              "observed_on": "2026-10-11", "note": "Synthetic metadata"}],
        }],
    }


class OutputTests(unittest.TestCase):
    def invoke(self, value, *args):
        with tempfile.TemporaryDirectory(prefix="emuwiz-homebrew-output-") as directory:
            path = pathlib.Path(directory) / "catalogue.json"
            path.write_text(json.dumps(value), encoding="utf-8")
            return subprocess.run([str(EXAMPLE), str(path), *args], capture_output=True, check=False)

    def assert_terminal_safe(self, stderr, lines=1):
        self.assertEqual(stderr.count(b"\n"), lines, repr(stderr))
        self.assertFalse(any(unicodedata.category(c) == "Cc" for c in stderr.decode("utf-8").replace("\n", "")), repr(stderr))

    def assert_identity_preserved(self, value, stdout):
        self.assertFalse(any(unicodedata.category(c) == "Cc" for c in stdout.decode("utf-8").replace("\n", "")), repr(stdout))
        record = json.loads(stdout)[0]
        self.assertEqual(record["project"]["id"], value["records"][0]["project"]["id"])
        artifact = record["listings"][0]["artifacts"][0]
        original = value["records"][0]["listings"][0]["artifacts"][0]
        self.assertEqual(artifact["provider_asset_id"], original["provider_asset_id"])
        self.assertEqual(artifact["platform_claims"], original["platform_claims"])

    def test_exact_review_terminal_reproductions(self):
        for value, prefix in [
            (catalogue(provider="local\x1b[2J\rFORGED\nPROVIDER"),
             b'note: "local\\u{1b}[2J\\rFORGED\\nPROVIDER"/"p" '),
            (catalogue(project="evil\x1b[2J\rFORGED\nID"),
             b'note: "local"/"evil\\u{1b}[2J\\rFORGED\\nID" '),
        ]:
            with self.subTest(prefix=prefix):
                result = self.invoke(value)
                self.assertEqual(result.returncode, 0, repr(result.stderr))
                self.assert_terminal_safe(result.stderr)
                self.assertTrue(result.stderr.startswith(prefix), repr(result.stderr))
                self.assert_identity_preserved(value, result.stdout)
                again = self.invoke(value)
                self.assertEqual((again.stdout, again.stderr), (result.stdout, result.stderr))

    def test_all_displayed_claim_fields_escape_terminal_controls(self):
        # NUL is forbidden by catalogue validation; every other C0/C1 and DEL
        # may occur in identifiers/evidence and must be escaped at output.
        for field in ["provider", "project", "asset", "platform"]:
            for codepoint in [*range(1, 32), *range(127, 160)]:
                with self.subTest(field=field, codepoint=codepoint):
                    value = catalogue(**{field: "prefix" + chr(codepoint) + "suffix"})
                    result = self.invoke(value)
                    self.assertEqual(result.returncode, 0, repr(result.stderr))
                    self.assert_terminal_safe(result.stderr)
                    self.assert_identity_preserved(value, result.stdout)

    def test_quotes_backslashes_and_unicode_preserve_identity(self):
        value = catalogue(**{key: 'prefix"\\ミ🌙suffix' for key in ["provider", "project", "asset", "platform"]})
        result = self.invoke(value)
        self.assertEqual(result.returncode, 0, repr(result.stderr))
        self.assert_terminal_safe(result.stderr)
        self.assertIn(b'\\"\\\\', result.stderr)
        self.assert_identity_preserved(value, result.stdout)

    def test_character_diagnostics_cover_seven_review_gaps(self):
        for field, text, codepoint, index in [
            ("title", "é👩\0Q", "U+0000", 2),
            ("summary", "é👩\0Q", "U+0000", 2),
            ("title", "\n", "U+000A", 0),
            ("title", "\r", "U+000D", 0),
            ("title", "\t", "U+0009", 0),
            ("title", "\n\t\r", "U+000A", 0),
            ("title", "A\x1bB\0", "U+001B", 1),
        ]:
            with self.subTest(field=field, text=text):
                value = catalogue()
                value["records"][0]["project"][field] = text
                result = self.invoke(value)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, b"")
                self.assert_terminal_safe(result.stderr)
                self.assertIn(f'records[0].project.{field}'.encode(), result.stderr)
                self.assertIn(codepoint.encode(), result.stderr)
                self.assertIn(f'at character {index};'.encode(), result.stderr)

    def test_other_errors_and_nul_remain_terminal_safe(self):
        for field in ["provider", "project", "asset", "platform"]:
            result = self.invoke(catalogue(**{field: "prefix\0suffix"}))
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, b"")
            self.assert_terminal_safe(result.stderr)
        value = catalogue(platform="NES")
        result = self.invoke(value, "", "unknown\x1b[2J\r\n\t")
        self.assertNotEqual(result.returncode, 0)
        self.assert_terminal_safe(result.stderr)
        with tempfile.TemporaryDirectory(prefix="emuwiz-homebrew-output-") as directory:
            missing = pathlib.Path(directory) / "missing\x1b[2J\r\n\t.json"
            result = subprocess.run([str(EXAMPLE), str(missing)], capture_output=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assert_terminal_safe(result.stderr)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--example", type=pathlib.Path, required=True)
    args, remaining = parser.parse_known_args()
    EXAMPLE = args.example.resolve(strict=True)
    unittest.main(argv=[__file__, *remaining])
