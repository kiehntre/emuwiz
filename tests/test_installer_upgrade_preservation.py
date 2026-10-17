#!/usr/bin/env python3
"""Upgrade preservation and release integrity regressions for install.sh.

Every case runs a private copy of install.sh against a synthetic bundle and a
disposable HOME, data and prefix tree. Interruptions are deterministic: a
private installer copy is edited to SIGKILL its own process group at an exact
point. Nothing here touches a real installation.
"""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
OLD_CLI = b"#!/bin/sh\necho old-cli\n"
NEW_CLI = b"#!/bin/sh\necho new-cli, a longer release than before\n"
OLD_GUI = b"#!/bin/sh\necho old-gui\n"
NEW_GUI = b"#!/bin/sh\necho new-gui\n"
USER_DATA = {
    ".config/emuwiz/config.toml": b"mount_root = '/srv/user-owned'\n",
    ".local/share/emuwiz/library.sqlite3": b"SQLite format 3\x00 synthetic user database\n",
    "roms/Game (Europe).zip": b"PK synthetic game file\n",
    "saves/game.srm": b"synthetic save\n",
}
KILL_GROUP = "os.killpg(os.getpgid(0), 9)"


def sha(data):
    return hashlib.sha256(data).hexdigest()


class UpgradePreservationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="emuwiz-upgrade-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "bundle"
        (self.bundle / "bin").mkdir(parents=True)
        self.script = self.bundle / "install.sh"
        shutil.copyfile(ROOT / "install.sh", self.script)
        (self.bundle / "assets").mkdir()
        for asset_dir in ("branding", "linux"):
            shutil.copytree(ROOT / "assets" / asset_dir, self.bundle / "assets" / asset_dir)
        shutil.copyfile(ROOT / "config.toml.example", self.bundle / "config.toml.example")
        self.home = self.root / "home"
        self.home.mkdir()
        for relative, data in USER_DATA.items():
            path = self.home / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        self.bin = self.root / "installed-bin"
        self.data = self.home / ".local/share"
        self.directory = self.data / "emuwiz-installer"
        self.previous = self.directory / "previous"
        self.journal = self.directory / "upgrade-pending"
        self.tmp = self.root / "tmp"
        self.tmp.mkdir()
        self.env = dict(os.environ, HOME=str(self.home), XDG_DATA_HOME=str(self.data),
                        TMPDIR=str(self.tmp))
        self.env.pop("EMUWIZ_CONFIG_HOME", None)
        self.env.pop("XDG_CONFIG_HOME", None)

    # -- fixtures -----------------------------------------------------------

    def release(self, cli, gui, sums=True):
        (self.bundle / "bin/emuwiz-cli").write_bytes(cli)
        (self.bundle / "bin/emuwiz").write_bytes(gui)
        for name in ("emuwiz-cli", "emuwiz"):
            os.chmod(self.bundle / "bin" / name, 0o755)
        if sums:
            self.write_sums()
        else:
            (self.bundle / "SHA256SUMS").unlink(missing_ok=True)

    def write_sums(self, extra=()):
        records = []
        for path in sorted(self.bundle.rglob("*")):
            relative = path.relative_to(self.bundle).as_posix()
            if path.is_file() and not path.is_symlink() and relative != "SHA256SUMS":
                records.append(f"{sha(path.read_bytes())}  {relative}")
        records.extend(extra)
        (self.bundle / "SHA256SUMS").write_text("\n".join(records) + "\n")

    def run_install(self, *flags, new_session=False):
        result = subprocess.run(
            ["sh", str(self.script), "--prefix", str(self.bin), *flags],
            env=self.env, stdin=subprocess.DEVNULL, capture_output=True, text=True,
            timeout=60, start_new_session=new_session,
        )
        self.assert_user_data_untouched()
        return result

    def hook(self, marker, mutation):
        script = self.script.read_text()
        self.assertEqual(script.count(marker), 1, marker)
        self.script.write_text(script.replace(marker, mutation + "\n" + marker))
        self.write_sums()

    def unhook(self):
        shutil.copyfile(ROOT / "install.sh", self.script)
        self.write_sums()

    def installed_old(self):
        self.release(OLD_CLI, OLD_GUI)
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.old_manifest = (self.directory / "manifest").read_bytes()

    def assert_user_data_untouched(self):
        for relative, data in USER_DATA.items():
            self.assertEqual((self.home / relative).read_bytes(), data, relative)

    def assert_old_install_intact(self):
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), OLD_CLI)
        self.assertEqual((self.bin / "emuwiz").read_bytes(), OLD_GUI)
        self.assertEqual((self.directory / "manifest").read_bytes(), self.old_manifest)

    def kept(self, slot, data):
        return self.previous / f"{slot}.{sha(data)}"

    def restore_command(self, stderr):
        commands = [line.strip() for line in stderr.splitlines() if line.strip().startswith("cp -- ")]
        self.assertEqual(len(commands), 1, stderr)
        return commands[0]

    # -- clean install, upgrade, reinstall -----------------------------------

    def test_clean_install_keeps_nothing_and_leaves_no_journal(self):
        self.release(NEW_CLI, NEW_GUI)
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Verified", result.stdout)
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), NEW_CLI)
        self.assertFalse(self.previous.exists())
        self.assertFalse(self.journal.exists())

    def test_upgrade_keeps_the_exact_previous_binary(self):
        self.installed_old()
        self.release(NEW_CLI, OLD_GUI)
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), NEW_CLI)
        self.assertEqual(self.kept("bin-emuwiz-cli", OLD_CLI).read_bytes(), OLD_CLI)
        # The unchanged GUI needs no copy.
        self.assertEqual(sorted(p.name for p in self.previous.iterdir()),
                         [self.kept("bin-emuwiz-cli", OLD_CLI).name])
        self.assertFalse(self.journal.exists())
        self.assertIn(str(self.kept("bin-emuwiz-cli", OLD_CLI)), result.stdout)
        self.assertIn(f"bin-emuwiz-cli file {sha(NEW_CLI)}", (self.directory / "manifest").read_text())
        self.assertEqual(oct(self.previous.stat().st_mode & 0o777), "0o700")

    def test_identical_reinstall_keeps_nothing(self):
        self.installed_old()
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.previous.exists())

    def test_later_upgrades_never_delete_older_kept_versions(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        self.assertEqual(self.run_install().returncode, 0)
        third = b"#!/bin/sh\necho third\n"
        self.release(third, NEW_GUI)
        self.assertEqual(self.run_install().returncode, 0)
        self.assertEqual(self.kept("bin-emuwiz-cli", OLD_CLI).read_bytes(), OLD_CLI)
        self.assertEqual(self.kept("bin-emuwiz-cli", NEW_CLI).read_bytes(), NEW_CLI)
        self.assertEqual(self.kept("bin-emuwiz", OLD_GUI).read_bytes(), OLD_GUI)

    def test_uninstall_leaves_kept_versions_and_says_so(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        self.assertEqual(self.run_install().returncode, 0)
        result = self.run_install("--uninstall")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.bin / "emuwiz-cli").exists())
        self.assertEqual(self.kept("bin-emuwiz-cli", OLD_CLI).read_bytes(), OLD_CLI)
        self.assertIn(str(self.previous), result.stdout)

    # -- release integrity ----------------------------------------------------

    def assert_refused_before_anything(self, result, text):
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(text, result.stderr)
        self.assertIn("nothing was installed or changed", result.stderr)

    def test_modified_package_is_refused_before_anything_is_created(self):
        self.release(NEW_CLI, NEW_GUI)
        (self.bundle / "bin/emuwiz-cli").write_bytes(NEW_CLI + b"# tampered\n")
        result = self.run_install()
        self.assert_refused_before_anything(result, "does not match SHA256SUMS")
        self.assertFalse(self.bin.exists())
        self.assertFalse(self.directory.exists())

    def test_modified_upgrade_package_leaves_the_installed_version_alone(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        (self.bundle / "bin/emuwiz").write_bytes(b"corrupt")
        result = self.run_install()
        self.assert_refused_before_anything(result, "bin/emuwiz does not match")
        self.assert_old_install_intact()
        self.assertFalse(self.previous.exists())

    def test_symlinked_binary_is_refused_even_with_matching_bytes(self):
        self.release(NEW_CLI, NEW_GUI)
        elsewhere = self.root / "elsewhere-cli"
        elsewhere.write_bytes(NEW_CLI)
        (self.bundle / "bin/emuwiz-cli").unlink()
        (self.bundle / "bin/emuwiz-cli").symlink_to(elsewhere)
        result = self.run_install()
        self.assert_refused_before_anything(result, "without following a link")
        self.assertFalse(self.bin.exists())

    def test_symlinked_directory_component_is_refused(self):
        self.release(NEW_CLI, NEW_GUI)
        real = self.root / "real-branding"
        shutil.move(str(self.bundle / "assets/branding"), real)
        (self.bundle / "assets/branding").symlink_to(real)
        result = self.run_install()
        self.assert_refused_before_anything(result, "without following a link")

    def test_traversal_record_is_refused(self):
        self.release(NEW_CLI, NEW_GUI)
        outside = self.root / "outside"
        outside.write_bytes(b"outside")
        self.write_sums(extra=[f"{sha(b'outside')}  ../outside"])
        result = self.run_install()
        self.assert_refused_before_anything(result, "unsafe path")
        self.assertFalse(self.bin.exists())

    def test_absolute_and_malformed_records_are_refused(self):
        for extra, text in (([f"{sha(b'x')}  /etc/passwd"], "unsafe path"),
                            (["not a checksum record"], "invalid or duplicate"),
                            ([f"{sha(NEW_CLI)}  bin/emuwiz-cli"], "invalid or duplicate")):
            with self.subTest(extra=extra):
                self.release(NEW_CLI, NEW_GUI)
                self.write_sums(extra=extra)
                self.assert_refused_before_anything(self.run_install(), text)

    def test_checksum_list_must_cover_both_binaries(self):
        self.release(NEW_CLI, NEW_GUI)
        lines = [l for l in (self.bundle / "SHA256SUMS").read_text().splitlines()
                 if not l.endswith("  bin/emuwiz")]
        (self.bundle / "SHA256SUMS").write_text("\n".join(lines) + "\n")
        self.assert_refused_before_anything(self.run_install(), "does not cover bin/emuwiz")

    def test_package_with_manifest_but_no_checksums_is_refused(self):
        self.release(NEW_CLI, NEW_GUI, sums=False)
        (self.bundle / "manifest.json").write_text("{}\n")
        self.assert_refused_before_anything(self.run_install(), "no SHA256SUMS")
        self.assertFalse(self.bin.exists())

    def test_binary_swapped_after_verification_is_not_installed(self):
        self.release(NEW_CLI, NEW_GUI)
        self.hook('mkdir -p -- "$bin_dir"',
                  f"printf 'swapped\\n' > {self.bundle}/bin/emuwiz-cli")
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), NEW_CLI)

    def test_snapshot_is_private_and_removed(self):
        self.release(NEW_CLI, NEW_GUI)
        self.assertEqual(self.run_install().returncode, 0)
        self.assertEqual(list(self.tmp.iterdir()), [])

    def test_unverified_layout_installs_with_an_explicit_note(self):
        self.release(NEW_CLI, NEW_GUI, sums=False)
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("ships no SHA256SUMS", result.stdout)

    # -- interruption and recovery ------------------------------------------

    def interrupt_publication(self):
        """Kill the whole installer while the new CLI is half written."""
        self.hook("                    shutil.copyfileobj(input_stream, stream)",
                  "                    stream.write(b'PARTIAL'); stream.flush(); " + KILL_GROUP)
        result = self.run_install(new_session=True)
        self.assertEqual(result.returncode, -9, result.stderr)
        self.unhook()

    def test_interrupted_write_leaves_a_verified_restorable_previous_version(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        self.interrupt_publication()
        self.assertTrue((self.bin / "emuwiz-cli").read_bytes().startswith(b"PARTIAL"))
        self.assertEqual(self.kept("bin-emuwiz-cli", OLD_CLI).read_bytes(), OLD_CLI)
        self.assertTrue(self.journal.exists())
        self.assertEqual((self.directory / "manifest").read_bytes(), self.old_manifest)

        before = (self.bin / "emuwiz-cli").read_bytes()
        result = self.run_install()
        self.assertEqual(result.returncode, 1)
        self.assertIn("did not finish. This run changed nothing", result.stderr)
        self.assertIn("holds neither the previous nor the new version", result.stderr)
        self.assertIn(sha(OLD_CLI), result.stderr)
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), before)
        self.assertTrue(self.journal.exists())

        subprocess.run(["sh", "-c", self.restore_command(result.stderr)], check=True)
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), OLD_CLI)
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), NEW_CLI)
        self.assertEqual((self.bin / "emuwiz").read_bytes(), NEW_GUI)
        self.assertFalse(self.journal.exists())

    def test_interrupted_before_the_manifest_reports_the_unrecorded_new_version(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        self.hook("# Compose complete metadata in the pinned directory", "kill -KILL $$")
        self.assertEqual(self.run_install(new_session=True).returncode, -9)
        self.unhook()
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), NEW_CLI)
        result = self.run_install()
        self.assertEqual(result.returncode, 1)
        self.assertIn("already holds the new version, which was never recorded", result.stderr)
        self.assertEqual(result.stderr.count("cp -- "), 2)
        for line in result.stderr.splitlines():
            if line.strip().startswith("cp -- "):
                subprocess.run(["sh", "-c", line.strip()], check=True)
        self.assert_old_install_intact()
        self.assertEqual(self.run_install().returncode, 0)
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), NEW_CLI)

    def test_interrupted_after_preserving_resolves_by_itself(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        self.hook('manifest_records_tmp=$(mktemp -- "/proc/self/fd/8/.manifest-records.XXXXXX")',
                  "kill -KILL $$")
        self.assertEqual(self.run_install(new_session=True).returncode, -9)
        self.unhook()
        self.assertTrue(self.journal.exists())
        self.assert_old_install_intact()
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), NEW_CLI)
        self.assertFalse(self.journal.exists())

    def test_missing_preserved_copy_is_reported_not_guessed(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        self.interrupt_publication()
        self.kept("bin-emuwiz-cli", OLD_CLI).unlink()
        result = self.run_install()
        self.assertEqual(result.returncode, 1)
        self.assertIn("missing or changed, so it cannot be restored from here", result.stderr)
        self.assertNotIn("cp -- ", result.stderr)

    def test_unreadable_journal_refuses(self):
        self.installed_old()
        self.journal.write_bytes(b"bin-emuwiz-cli truncated")
        result = self.run_install()
        self.assertEqual(result.returncode, 1)
        self.assertIn("cannot be read", result.stderr)
        self.assert_old_install_intact()

    def test_uninstall_after_an_interrupted_upgrade_keeps_the_evidence(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        self.interrupt_publication()
        result = self.run_install("--uninstall")
        self.assertIn("did not finish", result.stderr)
        self.assertTrue(self.journal.exists())
        self.assertTrue((self.directory / "manifest").exists())
        self.assertEqual(self.kept("bin-emuwiz-cli", OLD_CLI).read_bytes(), OLD_CLI)

    # -- refusal when recovery cannot be established --------------------------

    def test_insufficient_space_refuses_with_nothing_changed(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        self.hook("JOURNAL = \"upgrade-pending\"", "SPACE_MARGIN = 1 << 62")
        result = self.run_install()
        self.assertEqual(result.returncode, 1)
        self.assertIn("not enough free space", result.stderr)
        self.assertIn("nothing was installed", result.stderr)
        self.assert_old_install_intact()

    def test_unexpected_file_at_the_preserved_name_refuses(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        self.previous.mkdir(mode=0o700)
        self.kept("bin-emuwiz-cli", OLD_CLI).write_bytes(b"someone else's file")
        result = self.run_install()
        self.assertEqual(result.returncode, 1)
        self.assertIn("is not the installed version", result.stderr)
        self.assertEqual(self.kept("bin-emuwiz-cli", OLD_CLI).read_bytes(), b"someone else's file")
        self.assert_old_install_intact()

    def test_shared_preserved_directory_refuses(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        self.previous.mkdir()
        os.chmod(self.previous, 0o777)
        result = self.run_install()
        self.assertEqual(result.returncode, 1)
        self.assertIn("not private to you", result.stderr)
        self.assert_old_install_intact()

    def test_preserved_copy_removed_before_publication_blocks_the_change(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        self.hook('manifest_records_tmp=$(mktemp -- "/proc/self/fd/8/.manifest-records.XXXXXX")',
                  f"rm -f -- {self.kept('bin-emuwiz-cli', OLD_CLI)}")
        result = self.run_install()
        self.assertEqual(result.returncode, 1)
        self.assertIn("preserved previous version for bin-emuwiz-cli is missing", result.stderr)
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), OLD_CLI)

    def test_symlinked_preserved_directory_is_not_followed(self):
        self.installed_old()
        self.release(NEW_CLI, NEW_GUI)
        target = self.root / "redirected"
        target.mkdir(mode=0o700)
        self.previous.symlink_to(target)
        result = self.run_install()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(list(target.iterdir()), [])
        self.assert_old_install_intact()


if __name__ == "__main__":
    unittest.main()
