"""The desktop-entry templates place EmuWiz under Games/Emulator, not Accessories."""
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEMPLATES = [
    ROOT / "assets/linux/io.github.kiehntre.emuwiz.desktop.in",
    ROOT / "packaging/appimage/emuwiz.desktop",
]


def entry(path):
    pairs = {}
    for line in path.read_text().splitlines():
        if "=" in line and not line.startswith("["):
            key, value = line.split("=", 1)
            pairs[key] = value
    return pairs


class DesktopEntryCategories(unittest.TestCase):
    def test_categories_are_games_and_emulator(self):
        for path in TEMPLATES:
            with self.subTest(path=path.name):
                self.assertEqual(entry(path)["Categories"], "Game;Emulator;")

    def test_other_launcher_properties_are_preserved(self):
        for path in TEMPLATES:
            with self.subTest(path=path.name):
                e = entry(path)
                self.assertEqual(e["Name"], "EmuWiz")
                self.assertEqual(e["Icon"], "io.github.kiehntre.emuwiz")
                self.assertEqual(e["Terminal"], "false")
                self.assertEqual(e["StartupWMClass"], "io.github.kiehntre.emuwiz")
                self.assertIn("Exec", e)

    @unittest.skipUnless(shutil.which("desktop-file-validate"), "validator not installed")
    def test_entries_validate(self):
        for path in TEMPLATES:
            with self.subTest(path=path.name), tempfile.TemporaryDirectory() as tmp:
                out = Path(tmp) / "io.github.kiehntre.emuwiz.desktop"
                out.write_text(path.read_text().replace("@EMUWIZ_EXEC@", '"/usr/bin/emuwiz"'))
                result = subprocess.run(["desktop-file-validate", str(out)], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
