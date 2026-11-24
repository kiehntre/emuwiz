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
    # What a post-verification change in the extracted tree can and cannot reach. The hook (same
    # technique as test_installer_upgrade_preservation.py) runs after release_check has verified
    # SHA256SUMS and snapshotted the two binaries.
    def test_binaries_changed_after_verification_are_not_installed(self):
        # VERIFIED: the binaries are installed from the private snapshot taken at verification.
        self.hook(RECORDS_MARKER,
                  "printf '#!/bin/sh\\necho substituted\\n' > \"$script_dir/bin/emuwiz\"\n"
                  "printf '#!/bin/sh\\necho substituted\\n' > \"$script_dir/bin/emuwiz-cli\"")
        result = self.run_sh("--prefix", self.bin)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.bin / "emuwiz").read_bytes(), GUI)
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), CLI)

    @unittest.expectedFailure
    def test_known_limit_other_assets_changed_after_verification_are_installed(self):
        # KNOWN LIMITATION, not fixed by this suite's repairs: D5-03 enforces that SHA256SUMS
        # COVERS the launcher template, icons and starter config, and they are checked against it
        # at verification time, but they are then read again from the extracted tree, not from a
        # snapshot. A change made to them after verification is installed. This test states the
        # wanted behaviour and is expected to fail until those assets are snapshotted too; an
        # "unexpected success" means the limit is gone and the marker should be removed.
        # Both substitutions keep the files valid so the install itself succeeds (exit 0 is
        # asserted, so the test can never pass by not reaching the check).
        self.hook(RECORDS_MARKER,
                  "printf '# substituted\\n' >> \"$desktop_template\"\n"
                  "printf 'mount_root = \"/substituted\"\\n' > \"$script_dir/config.toml.example\"")
        result = self.run_sh("--prefix", self.bin)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("substituted", (self.home / ".config/emuwiz/config.toml").read_text(),
                         "unverified starter-config bytes were installed")
        self.assertNotIn("substituted", self.desktop.read_text(), "unverified desktop bytes were installed")

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

    def test_foreign_binary_with_release_bytes_survives_interrupted_install_retry_and_uninstall(self):
        # Review finding F1: a user's own emuwiz-cli, byte-identical to the release but never
        # recorded, was skipped as foreign by a run that was then interrupted (or failed) after
        # recording its intent; the retry adopted it and --uninstall deleted it.
        for interruption in ('    [ "$size" != 128 ] || kill -9 0',
                             '    [ "$size" != 128 ] || fail "injected failure after the intent"'):
            with self.subTest(interruption=interruption):
                shutil.rmtree(self.root / "real", ignore_errors=True)
                shutil.rmtree(self.data, ignore_errors=True)
                self.bin.mkdir(parents=True)
                users = self.installed_binary("emuwiz-cli")
                users.write_bytes(CLI)
                os.chmod(users, 0o755)
                self.hook(ICON_COPY_MARKER, interruption)
                first = self.run_sh("--prefix", self.bin, new_session=True)
                shutil.copyfile(ROOT / "install.sh", self.script)
                self.write_sums()
                self.assertNotEqual(first.returncode, 0, "fixture: the first run was interrupted")
                intent = self.data / "emuwiz-installer/install-intent"
                self.assertTrue(intent.exists(), "fixture: the interrupted run recorded its intent")
                self.assertNotIn(b"bin-emuwiz-cli", intent.read_bytes(),
                                 "the intent recorded a binary the run left in place as foreign")
                retry = self.run_sh("--prefix", self.bin)
                self.assertIn("foreign", retry.stderr, "the retry adopted the user's emuwiz-cli")
                self.assertFalse(b"bin-emuwiz-cli" in self.manifest.read_bytes(),
                                 "the manifest claims the user's emuwiz-cli")
                self.run_sh("--uninstall", "--prefix", self.bin)
                self.assertTrue(users.exists(), "--uninstall deleted the user's own emuwiz-cli")
                self.assertEqual(users.read_bytes(), CLI)
                self.assertFalse(self.installed_binary("emuwiz").exists(),
                                 "uninstall left the binary the installer did publish")

    def test_replace_foreign_interrupted_before_the_backup_never_claims_the_users_binary(self):
        # Review finding N1: with --replace-foreign the intent recorded the user's own emuwiz
        # (byte-identical to the release) although it is moved aside only later; a run killed or
        # failing in between let the retry adopt it without a backup and --uninstall delete it.
        gui_call = 'install_binary_slot bin-emuwiz "$src_gui" "$bin_dir/emuwiz"'
        for interruption in ("kill -9 0", 'fail "injected failure before the emuwiz backup"'):
            for retry_flags in ((), ("--replace-foreign",)):
                with self.subTest(interruption=interruption, retry=retry_flags):
                    shutil.rmtree(self.root / "real", ignore_errors=True)
                    shutil.rmtree(self.data, ignore_errors=True)
                    self.bin.mkdir(parents=True)
                    users = self.installed_binary("emuwiz")
                    users.write_bytes(GUI)
                    os.chmod(users, 0o755)
                    self.hook(gui_call, interruption)
                    first = self.run_sh("--prefix", self.bin, "--replace-foreign", new_session=True)
                    shutil.copyfile(ROOT / "install.sh", self.script)
                    self.write_sums()
                    self.assertNotEqual(first.returncode, 0, "fixture: the first run was interrupted")
                    self.assertEqual(users.read_bytes(), GUI, "fixture: interrupted before the backup")
                    intent = self.data / "emuwiz-installer/install-intent"
                    self.assertTrue(intent.exists(), "fixture: the interrupted run recorded its intent")
                    self.assertNotIn(b"bin-emuwiz ", intent.read_bytes(),
                                     "the intent recorded a foreign binary not yet moved aside")
                    retry = self.run_sh("--prefix", self.bin, *retry_flags)
                    if retry_flags:
                        self.assertEqual(retry.returncode, 0, retry.stderr)
                    else:
                        # Skipping a foreign destination is reported with a non-zero exit by design.
                        self.assertNotEqual(retry.returncode, 0, "the retry adopted the user's emuwiz")
                        self.assertIn("foreign", retry.stderr, "the retry adopted the user's emuwiz")
                        self.assertNotIn(b"bin-emuwiz ", self.manifest.read_bytes(),
                                         "the manifest claims the user's emuwiz")
                    self.run_sh("--uninstall", "--prefix", self.bin)
                    survivors = [p for p in self.bin.rglob("emuwiz") if p.is_file() and p.read_bytes() == GUI]
                    self.assertEqual(len(survivors), 1,
                                     "the user's own emuwiz was deleted (or the installed copy was left)")

    # Interrupted --replace-foreign runs recover on a plain retry once the user's file is aside.
    REPLACED_SLOT_POINTS = {
        # B: the foreign file is aside, the intent not yet rewritten.
        "B": ('    record_replaced_slot_in_intent "$slot" "$src"', '    [ "$slot" != {slot} ] || {stop}'),
        # C: inside the intent rewrite, between the complete staging write and the atomic rename.
        "C": ('    chmod 0600 "$intent_tmp"', '    [ "${{rrs_slot:-}}" != {slot} ] || {stop}'),
        # D: the intent names the slot and its backup; nothing published yet.
        "D": ('    binary_tmp=$(mktemp -- "/proc/self/fd/7/.emuwiz-binary.XXXXXX") || fail "could not stage binary"',
              '    [ "$slot" != {slot} ] || {stop}'),
        # E: both binaries published (and recorded), the manifest not yet written.
        "E": ('install_alias_slot alias-archivefs-cli emuwiz-cli "$bin_dir/archivefs-cli"', '{stop}'),
    }
    STOPS = ("kill -9 0", 'fail "injected failure"')

    def fresh_prefix(self, foreign):
        shutil.rmtree(self.root / "real", ignore_errors=True)
        shutil.rmtree(self.data, ignore_errors=True)
        self.bin.mkdir(parents=True)
        for name, data in foreign.items():
            self.installed_binary(name).write_bytes(data)
            os.chmod(self.installed_binary(name), 0o755)

    def interrupted_replace_run(self, marker, mutation):
        self.hook(marker, mutation)
        first = self.run_sh("--prefix", self.bin, "--replace-foreign", new_session=True)
        shutil.copyfile(ROOT / "install.sh", self.script)
        self.write_sums()
        self.assertNotEqual(first.returncode, 0, "fixture: the first run was interrupted")

    def backups(self):
        return sorted(p for p in self.bin.glob(".emuwiz-foreign-backup.*/*"))

    def test_interrupted_replace_foreign_recovers_on_a_plain_retry(self):
        release = {"emuwiz": GUI, "emuwiz-cli": CLI}
        slots = {"emuwiz": "bin-emuwiz", "emuwiz-cli": "bin-emuwiz-cli"}
        for name in ("emuwiz", "emuwiz-cli"):
            for user_bytes in (b"#!/bin/sh\necho users-own\n", release[name]):
                for point, (marker, mutation) in self.REPLACED_SLOT_POINTS.items():
                    for stop in self.STOPS:
                        with self.subTest(binary=name, identical=user_bytes == release[name],
                                          point=point, stop=stop):
                            self.fresh_prefix({name: user_bytes})
                            self.interrupted_replace_run(marker, mutation.format(slot=slots[name], stop=stop))
                            aside = self.backups()
                            self.assertEqual([p.name for p in aside], [name],
                                             "fixture: the user's file was moved aside exactly once")
                            self.assertEqual(aside[0].read_bytes(), user_bytes)
                            retry = self.run_sh("--prefix", self.bin)  # no --replace-foreign
                            self.assertEqual(retry.returncode, 0, retry.stderr)
                            self.assertNotIn("foreign", retry.stderr, "the retry called its own binary foreign")
                            self.assertIn(f"{slots[name]} ".encode(), self.manifest.read_bytes())
                            self.assertEqual(self.installed_binary(name).read_bytes(), release[name])
                            self.assertEqual(self.backups(), aside, "the retry made a second backup")
                            self.assertEqual(aside[0].read_bytes(), user_bytes, "the user's backup changed")
                            self.assertFalse(list(self.data.rglob(".install-intent.*")),
                                             "a staging file of the intent was left behind")
                            done = self.run_sh("--uninstall", "--prefix", self.bin)
                            self.assertEqual(done.returncode, 0, done.stderr)
                            self.assertFalse(self.installed_binary(name).exists(), "uninstall left the release copy")
                            self.assertEqual(self.backups(), aside, "uninstall touched the user's backup")
                            self.assertEqual(aside[0].read_bytes(), user_bytes)

    def test_user_moving_the_original_back_is_never_adopted_or_deleted(self):
        # Review finding (F-b widened): after an interrupted --replace-foreign run, the user moves
        # their byte-identical original back from the backup path the installer printed. A plain
        # retry must not adopt it, and --uninstall must not delete the only copy.
        release = {"emuwiz": GUI, "emuwiz-cli": CLI}
        slots = {"emuwiz": "bin-emuwiz", "emuwiz-cli": "bin-emuwiz-cli"}
        alias_line = 'install_alias_slot alias-archivefs-cli emuwiz-cli "$bin_dir/archivefs-cli"'
        for name in ("emuwiz", "emuwiz-cli"):
            for point in ("D", "E", "E twice"):
                for keep_empty_dir in (True, False):
                    for stop in self.STOPS:
                        with self.subTest(binary=name, point=point, keep_empty_dir=keep_empty_dir, stop=stop):
                            self.fresh_prefix({name: release[name]})
                            marker, mutation = self.REPLACED_SLOT_POINTS[point.split()[0]]
                            self.interrupted_replace_run(marker, mutation.format(slot=slots[name], stop=stop))
                            if point == "E twice":
                                # A plain retry interrupted again before its manifest: the backup entry
                                # must carry over into the retry's own intent.
                                self.hook(alias_line, stop)
                                again = self.run_sh("--prefix", self.bin, new_session=True)
                                shutil.copyfile(ROOT / "install.sh", self.script)
                                self.write_sums()
                                self.assertNotEqual(again.returncode, 0, "fixture: the retry was interrupted")
                            intent = (self.data / "emuwiz-installer/install-intent").read_text()
                            self.assertIn(f"replaced-{slots[name]} .emuwiz-foreign-backup.", intent)
                            (aside,) = self.backups()
                            os.replace(aside, self.installed_binary(name))  # the user takes it back
                            if not keep_empty_dir:
                                aside.parent.rmdir()
                            retry = self.run_sh("--prefix", self.bin)
                            self.assertNotEqual(retry.returncode, 0, "the retry adopted the user's restored file")
                            self.assertIn("is no longer in", retry.stderr)
                            self.assertIn("may be that original, so it is not treated as EmuWiz-owned", retry.stderr)
                            self.assertIn("leaving foreign path untouched", retry.stderr)
                            self.assertNotIn(f"{slots[name]} ".encode(), self.manifest.read_bytes(),
                                             "the manifest claims the user's restored file")
                            self.run_sh("--uninstall", "--prefix", self.bin)
                            self.assertEqual(self.installed_binary(name).read_bytes(), release[name],
                                             "--uninstall deleted the user's only copy")

    def test_backup_moved_elsewhere_leaves_no_stale_record_and_an_empty_slot_recovers(self):
        # Review finding (stale backup entry): killed at D (destination empty, backup holds the
        # original), the user moves the backup somewhere else. A plain retry must install the empty
        # slot without carrying the stale backup entry, so that even if this retry is interrupted
        # too, the next plain retry takes back its own binary.
        release = {"emuwiz": GUI, "emuwiz-cli": CLI}
        slots = {"emuwiz": "bin-emuwiz", "emuwiz-cli": "bin-emuwiz-cli"}
        alias_line = 'install_alias_slot alias-archivefs-cli emuwiz-cli "$bin_dir/archivefs-cli"'
        users = b"#!/bin/sh\necho users-own\n"
        for name in ("emuwiz", "emuwiz-cli"):
            for second_interruption in (None,) + self.STOPS:
                for stop in self.STOPS:
                    with self.subTest(binary=name, stop=stop, second_interruption=second_interruption):
                        self.fresh_prefix({name: users})
                        shutil.rmtree(self.root / "kept", ignore_errors=True)
                        marker, mutation = self.REPLACED_SLOT_POINTS["D"]
                        self.interrupted_replace_run(marker, mutation.format(slot=slots[name], stop=stop))
                        self.assertFalse(self.installed_binary(name).exists(), "fixture: destination empty at D")
                        (aside,) = self.backups()
                        kept = self.root / "kept" / name
                        kept.parent.mkdir()
                        os.replace(aside, kept)  # moved elsewhere, not back
                        aside.parent.rmdir()
                        if second_interruption:
                            self.hook(alias_line, second_interruption)
                            retry = self.run_sh("--prefix", self.bin, new_session=True)
                            shutil.copyfile(ROOT / "install.sh", self.script)
                            self.write_sums()
                            self.assertNotEqual(retry.returncode, 0, "fixture: the retry was interrupted")
                            intent = (self.data / "emuwiz-installer/install-intent").read_text()
                            self.assertNotIn(f"replaced-{slots[name]} ", intent, "stale backup entry carried over")
                            self.assertIn(f"{slots[name]} ", intent)
                            retry = self.run_sh("--prefix", self.bin)
                        else:
                            retry = self.run_sh("--prefix", self.bin)
                            self.assertIn("is empty and will be installed as new", retry.stderr)
                            self.assertNotIn("may be that original", retry.stderr)
                        self.assertEqual(retry.returncode, 0, retry.stderr)
                        self.assertNotIn("leaving foreign path untouched", retry.stderr)
                        self.assertIn(f"{slots[name]} ".encode(), self.manifest.read_bytes())
                        self.assertEqual(self.installed_binary(name).read_bytes(), release[name])
                        self.assertEqual(self.backups(), [], "a retry created a foreign backup")
                        done = self.run_sh("--uninstall", "--prefix", self.bin)
                        self.assertEqual(done.returncode, 0, done.stderr)
                        self.assertFalse(self.installed_binary(name).exists())
                        self.assertEqual(kept.read_bytes(), users, "the user's moved original changed")

    def test_uninstall_with_a_missing_backup_and_empty_slot_never_promises_an_install(self):
        # Review follow-up: during --uninstall the warning must not say the empty slot "will be
        # installed as new"; nothing is installed or removed there.
        slots = {"emuwiz": "bin-emuwiz", "emuwiz-cli": "bin-emuwiz-cli"}
        users = b"#!/bin/sh\necho users-own\n"
        for name in ("emuwiz", "emuwiz-cli"):
            with self.subTest(binary=name):
                self.fresh_prefix({name: users})
                shutil.rmtree(self.root / "kept", ignore_errors=True)
                marker, mutation = self.REPLACED_SLOT_POINTS["D"]
                self.interrupted_replace_run(marker, mutation.format(slot=slots[name], stop="kill -9 0"))
                (aside,) = self.backups()
                kept = self.root / "kept" / name
                kept.parent.mkdir()
                os.replace(aside, kept)
                done = self.run_sh("--uninstall", "--prefix", self.bin)
                self.assertEqual(done.returncode, 0, done.stderr)
                self.assertIn("is absent, so there is nothing to remove there", done.stderr)
                self.assertNotIn("will be installed as new", done.stderr)
                self.assertFalse(self.installed_binary(name).exists())
                self.assertEqual(kept.read_bytes(), users)

    def test_known_limit_original_copied_back_with_the_backup_left_in_place(self):
        # KNOWN LIMITATION (documented, not a loss of the only copy): if the user COPIES the original
        # back and leaves the backup where it was, the installer cannot tell the copy in place from
        # the release binary it published (same bytes), so the retry adopts it and --uninstall
        # removes it. The backup copy is never touched, so the user's bytes survive there.
        release = {"emuwiz": GUI, "emuwiz-cli": CLI}
        slots = {"emuwiz": "bin-emuwiz", "emuwiz-cli": "bin-emuwiz-cli"}
        for name in ("emuwiz", "emuwiz-cli"):
            with self.subTest(binary=name):
                self.fresh_prefix({name: release[name]})
                marker, mutation = self.REPLACED_SLOT_POINTS["E"]
                self.interrupted_replace_run(marker, mutation.format(slot=slots[name], stop="kill -9 0"))
                (aside,) = self.backups()
                shutil.copyfile(aside, self.installed_binary(name))
                retry = self.run_sh("--prefix", self.bin)
                self.assertEqual(retry.returncode, 0, retry.stderr)
                self.run_sh("--uninstall", "--prefix", self.bin)
                self.assertFalse(self.installed_binary(name).exists())
                self.assertEqual(self.backups(), [aside], "the backup was touched")
                self.assertEqual(aside.read_bytes(), release[name], "the user's bytes were lost")

    def test_both_binaries_foreign_interrupted_between_them(self):
        # emuwiz-cli is aside and published; emuwiz is still the user's own, not yet moved.
        users = {"emuwiz": GUI, "emuwiz-cli": b"#!/bin/sh\necho users-own-cli\n"}
        gui_call = 'install_binary_slot bin-emuwiz "$src_gui" "$bin_dir/emuwiz"'
        for stop in self.STOPS:
            with self.subTest(stop=stop):
                self.fresh_prefix(users)
                self.interrupted_replace_run(gui_call, stop)
                intent = (self.data / "emuwiz-installer/install-intent").read_bytes()
                self.assertIn(b"bin-emuwiz-cli ", intent)
                self.assertNotIn(b"bin-emuwiz ", intent, "the user's emuwiz was recorded before it was aside")
                # Plain retry: the CLI is taken back; the user's emuwiz is left alone (point A: an
                # earlier run's --replace-foreign is never inherited).
                plain = self.run_sh("--prefix", self.bin)
                self.assertNotEqual(plain.returncode, 0)
                self.assertIn("leaving foreign path untouched", plain.stderr)
                self.assertNotIn("emuwiz-cli", "".join(l for l in plain.stderr.splitlines(True) if "foreign" in l))
                self.assertEqual(self.installed_binary("emuwiz").read_bytes(), GUI)
                self.assertEqual(self.installed_binary("emuwiz-cli").read_bytes(), CLI)
                self.assertEqual([p.name for p in self.backups()], ["emuwiz-cli"])
                manifest = self.manifest.read_bytes()
                self.assertIn(b"bin-emuwiz-cli ", manifest)
                self.assertNotIn(b"bin-emuwiz ", manifest, "the manifest claims the user's emuwiz")
                # A run given the flag itself completes the replacement, with one backup per file.
                flagged = self.run_sh("--prefix", self.bin, "--replace-foreign")
                self.assertEqual(flagged.returncode, 0, flagged.stderr)
                aside = {p.name: p.read_bytes() for p in self.backups()}
                self.assertEqual(aside, users, "each user file must be aside exactly once, unchanged")
                done = self.run_sh("--uninstall", "--prefix", self.bin)
                self.assertEqual(done.returncode, 0, done.stderr)
                self.assertEqual({p.name: p.read_bytes() for p in self.backups()}, users)
                self.assertEqual(sorted(p.name for p in self.bin.iterdir() if not p.name.startswith(".")), [])

    def test_uninstall_for_another_prefix_refuses_an_intent_with_no_binary_lines(self):
        # Review finding N2: an interrupted install at another prefix whose binaries were both
        # foreign leaves an intent with no slot lines; --uninstall for this prefix must still
        # refuse rather than remove that installation's launcher, icons and intent.
        self.bin.mkdir(parents=True)
        for name in ("emuwiz", "emuwiz-cli"):
            self.installed_binary(name).write_bytes(b"#!/bin/sh\necho users-own\n")
        self.interrupt_first_install()
        intent = self.data / "emuwiz-installer/install-intent"
        self.assertTrue(intent.exists(), "fixture: the interrupted run recorded its intent")
        recorded = intent.read_bytes()
        self.assertNotIn(b"bin-emuwiz", recorded, "fixture: both binaries were foreign")
        self.assertTrue(self.desktop.exists(), "fixture: the interrupted run published its launcher")
        launcher, icons = self.desktop.read_bytes(), self.icons()
        result = self.run_sh("--uninstall", "--prefix", self.root / "other/bin")
        self.assertNotEqual(result.returncode, 0, "uninstall for another prefix was not refused")
        self.assertIn(str(self.bin), result.stderr)
        self.assertEqual(intent.read_bytes(), recorded, "the other installation's intent was changed")
        self.assertEqual(self.desktop.read_bytes(), launcher, "the other installation's launcher was removed")
        self.assertEqual(self.icons(), icons, "the other installation's icons were removed")

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

    def test_prefix_reached_through_a_symlink_with_a_trailing_line_break_is_refused(self):
        real = self.root / "real\n"
        real.mkdir()
        link = self.root / "link"
        link.symlink_to(real)
        result = self.run_sh("--prefix", link)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("line break", result.stderr)
        self.assertFalse((self.root / "real").exists(), "installed into the sibling the newline was stripped to")
        self.assertEqual(os.listdir(real), [])

    def test_data_directory_with_a_line_break_is_refused(self):
        self.env["XDG_DATA_HOME"] = str(self.root / "data\nhome")
        result = self.run_sh("--prefix", self.bin)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("line break", result.stderr)
        self.assertFalse(self.bin.exists() and os.listdir(self.bin))

    def test_uninstall_for_another_prefix_leaves_an_interrupted_installs_intent_alone(self):
        self.interrupt_first_install()
        intent = self.data / "emuwiz-installer/install-intent"
        before = intent.read_bytes()
        result = self.run_sh("--uninstall", "--prefix", self.root / "other/bin")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(str(self.bin), result.stderr)
        self.assertEqual(intent.read_bytes(), before)
        self.assertTrue(self.desktop.exists(), "the other install's launcher was removed")
        self.assertTrue(self.installed_binary().exists())
        # The right prefix still adopts its own binaries afterwards.
        self.assertEqual(self.run_sh("--prefix", self.bin).returncode, 0)

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
