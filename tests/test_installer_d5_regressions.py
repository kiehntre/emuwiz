#!/usr/bin/env python3
"""Installer safety regressions: D5-01, D5-02, D5-03, D5-04, D5-05 and S12-01.

Every case runs a private copy of install.sh against a synthetic bundle in a
disposable HOME/XDG/TMPDIR/prefix tree. Nothing touches a real installation and
nothing uses the network. Originated as the Night Watch P3/D5 acceptance suite;
the last group of cases adds the safety edges of the repairs.

  D5-01  an uninstall run for another prefix leaves the real install alone
  D5-02  a prefix with a line break is refused before anything is published
  D5-03  SHA256SUMS must cover every file the installer publishes
  S12-01 install.sh never imports a module from its working directory
  D5-04  staging files left by an interrupted run are swept
  D5-05  a re-run adopts exactly the binaries its interrupted predecessor published
"""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
CLI = b"#!/bin/sh\necho cli\n"
GUI = b"#!/bin/sh\necho gui\n"
DESKTOP = "io.github.kiehntre.emuwiz"
RECORDS_MARKER = 'manifest_records_tmp=$(mktemp -- "/proc/self/fd/8/.manifest-records.XXXXXX") ||'
ICON_COPY_MARKER = '    cp -- "$icon_source" "$icon_tmp"'


def sha(data):
    return hashlib.sha256(data).hexdigest()


class D5Fixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="emuwiz-d5-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "bundle"
        (self.bundle / "bin").mkdir(parents=True)
        self.script = self.bundle / "install.sh"
        shutil.copyfile(ROOT / "install.sh", self.script)
        for asset_dir in ("branding", "linux"):
            shutil.copytree(ROOT / "assets" / asset_dir, self.bundle / "assets" / asset_dir)
        shutil.copyfile(ROOT / "config.toml.example", self.bundle / "config.toml.example")
        for name, data in (("emuwiz-cli", CLI), ("emuwiz", GUI)):
            (self.bundle / "bin" / name).write_bytes(data)
            os.chmod(self.bundle / "bin" / name, 0o755)
        self.home = self.root / "home"
        self.home.mkdir()
        self.data = self.home / ".local/share"
        self.manifest = self.data / "emuwiz-installer/manifest"
        self.desktop = self.data / f"applications/{DESKTOP}.desktop"
        self.bin = self.root / "real/bin"
        (self.root / "tmp").mkdir()
        self.env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(self.home),
                    "XDG_DATA_HOME": str(self.data), "XDG_CONFIG_HOME": str(self.home / ".config"),
                    "TMPDIR": str(self.root / "tmp"), "LC_ALL": "C"}
        self.write_sums()

    def write_sums(self, only=None):
        records = []
        for path in sorted(self.bundle.rglob("*")):
            relative = path.relative_to(self.bundle).as_posix()
            if path.is_file() and not path.is_symlink() and relative != "SHA256SUMS":
                if only is None or relative in only:
                    records.append(f"{sha(path.read_bytes())}  {relative}")
        (self.bundle / "SHA256SUMS").write_text("\n".join(records) + "\n")

    def hook(self, marker, mutation):
        text = self.script.read_text()
        self.assertEqual(text.count(marker), 1, marker)
        self.script.write_text(text.replace(marker, mutation + "\n" + marker))
        self.write_sums()

    def run_sh(self, *args, cwd=None, new_session=False):
        return subprocess.run(["sh", str(self.script), *map(str, args)], env=self.env,
                              cwd=cwd or self.root, stdin=subprocess.DEVNULL, capture_output=True,
                              text=True, errors="surrogateescape", timeout=120,
                              start_new_session=new_session)

    def icons(self):
        return sorted((self.data / "icons").rglob(f"{DESKTOP}.png")) if (self.data / "icons").exists() else []


    def interrupt_first_install(self):
        self.hook(ICON_COPY_MARKER, '    [ "$size" != 128 ] || kill -9 0')
        self.run_sh("--prefix", self.bin, new_session=True)
        shutil.copyfile(ROOT / "install.sh", self.script)
        self.write_sums()


class D5InstallerRegressions(D5Fixture):
    # D5-01 ------------------------------------------------------------------
    def test_uninstall_with_another_prefix_leaves_the_real_install_alone(self):
        self.assertEqual(self.run_sh("--prefix", self.bin).returncode, 0)
        manifest = self.manifest.read_bytes()
        for other in (self.root / "typo/bin", None):  # missing directory, and no --prefix at all
            args = ["--uninstall"] + (["--prefix", other] if other else [])
            self.run_sh(*args)
            self.assertTrue(self.manifest.exists(), f"uninstall {args} deleted the ownership manifest of {self.bin}")
            self.assertEqual(self.manifest.read_bytes(), manifest,
                             f"uninstall {args} deleted the ownership manifest of {self.bin}")
            self.assertTrue(self.desktop.exists(), f"uninstall {args} removed the real install's launcher")
            self.assertEqual(len(self.icons()), 5, f"uninstall {args} removed the real install's icons")
        result = self.run_sh("--uninstall", "--prefix", self.bin)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sorted(os.listdir(self.bin)), [], result.stderr)

    # D5-02 ------------------------------------------------------------------
    def test_line_break_prefix_publishes_nothing(self):
        prefix = self.root / "pre\npost"
        result = self.run_sh("--prefix", prefix)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("line break", result.stderr)
        published = sorted(os.listdir(prefix)) if prefix.exists() else []
        self.assertEqual(published, [], "binaries/aliases were published before the line-break refusal")

    # D5-03 ------------------------------------------------------------------
    def test_installed_assets_are_the_verified_bytes(self):
        # Stand-in for a post-verification change in the extracted tree (same hook technique as
        # test_installer_upgrade_preservation.py): runs immediately after release_check.
        self.hook(RECORDS_MARKER,
                  "printf '[Desktop Action x]\\nName=x\\nExec=/bin/sh -c substituted\\n' >> \"$desktop_template\"\n"
                  "printf 'mount_root = \"/substituted\"\\n' > \"$script_dir/config.toml.example\"")
        result = self.run_sh("--prefix", self.bin)
        if result.returncode == 0:
            self.assertNotIn("substituted", self.desktop.read_text(), "unverified desktop bytes were installed")
            self.assertNotIn("substituted", (self.home / ".config/emuwiz/config.toml").read_text(),
                             "unverified starter-config bytes were installed")

    def test_sums_must_cover_every_installed_asset(self):
        self.write_sums(only={"install.sh", "bin/emuwiz", "bin/emuwiz-cli"})
        result = self.run_sh("--prefix", self.bin)
        self.assertNotEqual(result.returncode, 0, "a SHA256SUMS that omits the launcher/icons/config was accepted")
        self.assertFalse(self.bin.exists() and os.listdir(self.bin))

    # S12-01 extension -------------------------------------------------------
    def test_module_planted_in_the_bundle_is_not_imported(self):
        marker = self.root / "planted.txt"
        (self.bundle / "hashlib.py").write_text(f"open({str(marker)!r}, 'a').write('x')\nraise SystemExit(9)\n")
        self.write_sums()  # SHA256SUMS legitimately covers everything else; hashlib.py is an extra file
        (self.bundle / "SHA256SUMS").write_text("".join(
            l + "\n" for l in (self.bundle / "SHA256SUMS").read_text().splitlines() if not l.endswith("hashlib.py")))
        self.run_sh("--prefix", self.bin, cwd=self.bundle)  # the documented `./install.sh` from the bundle
        self.assertFalse(marker.exists(), "install.sh imported a module from its working directory")

    # D5-04 ------------------------------------------------------------------
    def test_interrupted_run_leaves_no_staging_files_behind(self):
        self.interrupt_first_install()
        self.run_sh("--prefix", self.bin, "--replace-foreign")
        self.run_sh("--uninstall", "--prefix", self.bin)
        leftovers = sorted(str(p.relative_to(self.root)) for p in self.root.rglob(".*")
                           if p.is_file() and (".manifest-records." in p.name or ".emuwiz-binary." in p.name
                                               or p.name.startswith(f".{DESKTOP}.")))
        self.assertEqual(leftovers, [], "staging files from the interrupted run were never cleaned up")

    # D5-05 ------------------------------------------------------------------
    def test_rerun_after_interrupted_first_install_adopts_its_own_binaries(self):
        self.interrupt_first_install()
        self.assertFalse(self.manifest.exists())
        result = self.run_sh("--prefix", self.bin)
        self.assertEqual(result.returncode, 0, "re-run calls the binaries the interrupted run published foreign:\n"
                         + result.stderr)
        self.assertEqual(self.run_sh("--uninstall", "--prefix", self.bin).returncode, 0)
        self.assertEqual(sorted(os.listdir(self.bin)), [])


class D5RepairSafetyEdges(D5Fixture):
    """Edges that keep the repairs from widening what the installer may touch.

    """

    def installed_binary(self, name="emuwiz"):
        return self.bin / name

    # D5-05 ------------------------------------------------------------------
    def test_modified_binary_from_an_interrupted_run_is_not_adopted(self):
        self.interrupt_first_install()
        victim = self.installed_binary()
        self.assertTrue(victim.exists(), "fixture: the interrupted run published the binary")
        victim.write_bytes(b"#!/bin/sh\necho not-ours\n")
        result = self.run_sh("--prefix", self.bin)
        self.assertEqual(victim.read_bytes(), b"#!/bin/sh\necho not-ours\n",
                         "a modified file was replaced without --replace-foreign")
        self.assertIn("foreign", result.stderr)

    def test_unrelated_binary_is_never_claimed_without_an_intent(self):
        self.bin.mkdir(parents=True)
        other = self.installed_binary()
        other.write_bytes(GUI)  # same bytes as the bundle, but no installer ever recorded it
        result = self.run_sh("--prefix", self.bin)
        self.assertIn("foreign", result.stderr)
        self.assertFalse(self.manifest.exists() and b"bin-emuwiz " in self.manifest.read_bytes())

    def test_intent_for_another_prefix_is_not_used(self):
        self.interrupt_first_install()
        other = self.root / "other/bin"
        result = self.run_sh("--prefix", other)
        self.assertEqual(result.returncode, 0, result.stderr)
        # The first prefix's binaries stay exactly as the interrupted run left them.
        self.assertTrue(self.installed_binary().exists())

    def test_garbled_intent_adopts_nothing(self):
        self.interrupt_first_install()
        intent = self.data / "emuwiz-installer/install-intent"
        self.assertTrue(intent.exists(), "fixture: the interrupted run recorded its intent")
        intent.write_text(intent.read_text().replace("end\n", "end\nextra\n"))
        result = self.run_sh("--prefix", self.bin)
        self.assertIn("foreign", result.stderr)

    # D5-01 ------------------------------------------------------------------
    def test_uninstall_refusal_names_the_recorded_install_and_exits_nonzero(self):
        self.assertEqual(self.run_sh("--prefix", self.bin).returncode, 0)
        result = self.run_sh("--uninstall", "--prefix", self.root / "typo/bin")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(str(self.bin), result.stderr)
        self.assertTrue(self.manifest.exists())

    def test_uninstall_with_a_different_data_directory_is_refused(self):
        self.assertEqual(self.run_sh("--prefix", self.bin).returncode, 0)
        manifest = self.manifest.read_bytes()
        self.env["XDG_DATA_HOME"] = str(self.root / "elsewhere")
        (self.root / "elsewhere").mkdir()
        self.run_sh("--uninstall", "--prefix", self.bin)
        self.assertEqual(self.manifest.read_bytes(), manifest)
        self.assertTrue(self.installed_binary().exists())

    def test_uninstall_after_an_interrupted_install_removes_only_its_own_files(self):
        self.interrupt_first_install()
        stranger = self.bin / "unrelated-tool"
        stranger.write_text("mine")
        result = self.run_sh("--uninstall", "--prefix", self.bin)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sorted(os.listdir(self.bin)), ["unrelated-tool"])
        self.assertFalse((self.data / "emuwiz-installer").exists(),
                         "bookkeeping left behind after everything it covered was removed")

    # D5-03 ------------------------------------------------------------------
    def test_an_uncovered_symlinked_icon_is_refused(self):
        icon = self.bundle / "assets/branding/emuwiz-logo-64.png"
        icon.unlink()
        icon.symlink_to("emuwiz-logo-32.png")
        self.write_sums(only={"install.sh", "bin/emuwiz", "bin/emuwiz-cli"} |
                        {f"assets/branding/emuwiz-logo-{n}.png" for n in (32, 128, 256, 512)} |
                        {"assets/linux/io.github.kiehntre.emuwiz.desktop.in", "config.toml.example"})
        result = self.run_sh("--prefix", self.bin)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.bin.exists() and os.listdir(self.bin))

    # D5-04 ------------------------------------------------------------------
    def test_sweep_leaves_other_files_alone(self):
        self.assertEqual(self.run_sh("--prefix", self.bin).returncode, 0)
        keep = [self.bin / ".emuwiz-binary.keep", self.bin / ".emuwiz-binary.tooLongName",
                self.data / "emuwiz-installer/.manifest-records-not-ours"]
        for path in keep:
            path.write_text("user file")
        (self.bin / ".emuwiz-binary.abc123").symlink_to(self.root / "target-file")
        (self.root / "target-file").write_text("target")
        self.assertEqual(self.run_sh("--prefix", self.bin).returncode, 0)
        for path in keep:
            self.assertTrue(path.exists(), f"{path} was removed by the sweep")
        self.assertEqual((self.root / "target-file").read_text(), "target")
        self.assertTrue((self.bin / ".emuwiz-binary.abc123").is_symlink())


if __name__ == "__main__":
    unittest.main()
