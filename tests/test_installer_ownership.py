#!/usr/bin/env python3
"""Ownership regressions; instrument only private bundle copies, never production hooks."""
import fcntl
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
FOREIGN = b"FOREIGN ownership, never overwrite\n"


class OwnershipTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="emuwiz-ownership-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        self.script = self.bundle / "install.sh"
        shutil.copyfile(ROOT / "install.sh", self.script)
        (self.bundle / "assets").mkdir()
        for asset_dir in ("branding", "linux"):
            shutil.copytree(ROOT / "assets" / asset_dir, self.bundle / "assets" / asset_dir)
        shutil.copyfile(ROOT / "config.toml.example", self.bundle / "config.toml.example")
        # Exercise the current-main packaged layout throughout.
        (self.bundle / "bin").mkdir()
        for name in ("emuwiz", "emuwiz-cli"):
            (self.bundle / "bin" / name).write_text("#!/bin/sh\nexit 0\n")
        self.home = self.root / "home"
        self.home.mkdir()
        self.bin = self.root / "installed-bin"
        self.bin.mkdir()
        self.data = self.home / ".local/share"
        self.directory = self.data / "emuwiz-installer"
        self.manifest = self.directory / "manifest"
        self.outside = self.root / "outside"
        self.outside.write_bytes(FOREIGN)
        self.unmanaged = self.bin / "unmanaged"
        self.unmanaged.write_bytes(FOREIGN)
        self.env = dict(os.environ, HOME=str(self.home), XDG_DATA_HOME=str(self.data))

    def run_install(self, *flags):
        result = subprocess.run(
            ["sh", str(self.script), "--prefix", str(self.bin), *flags],
            env=self.env, stdin=subprocess.DEVNULL, capture_output=True, text=True,
            timeout=30,
        )
        self.assertEqual(self.outside.read_bytes(), FOREIGN)
        self.assertEqual(self.unmanaged.read_bytes(), FOREIGN)
        return result

    def installed(self):
        result = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.original_manifest = self.manifest.read_bytes()
        self.original_cli = (self.bin / "emuwiz-cli").read_bytes()
        self.original_assets = self.assets_snapshot()
        (self.bundle / "bin/emuwiz-cli").write_text("#!/bin/sh\n# new release\nexit 0\n")

    def hook(self, marker, mutation):
        """Insert a deterministic mutation in a disposable installer copy."""
        script = self.script.read_text()
        self.assertEqual(script.count(marker), 1)
        script = script.replace(marker, mutation + "\n" + marker)
        self.script.write_text(script)

    def mutation(self, kind):
        m, outside, directory = map(shlex.quote, (str(self.manifest), str(self.outside), str(self.directory)))
        if kind == "regular":
            return f"printf 'FOREIGN ownership, never overwrite\\n' > {m}.replacement\nmv -f -- {m}.replacement {m}"
        if kind == "symlink":
            return f"rm -f -- {m}\nln -s -- {outside} {m}"
        if kind == "directory":
            return f"rm -f -- {m}\nmkdir -- {m}\ncp -- {outside} {m}/canary"
        if kind == "bookkeeping":
            return f"mv -- {directory} {directory}.old\nmkdir -- {directory}\ncp -- {outside} {m}"
        if kind == "bookkeeping-empty":
            return f"mv -- {directory} {directory}.old\nmkdir -- {directory}"
        if kind == "in-place":
            return f"printf 'FOREIGN ownership, never overwrite\\n' > {m}"
        raise AssertionError(kind)

    def assert_foreign_preserved(self, kind):
        if kind == "symlink":
            self.assertTrue(self.manifest.is_symlink())
            self.assertEqual(os.readlink(self.manifest), str(self.outside))
        elif kind == "directory":
            self.assertEqual((self.manifest / "canary").read_bytes(), FOREIGN)
        else:
            self.assertEqual(self.manifest.read_bytes(), FOREIGN)

    def assert_no_publication(self, had_install):
        if had_install:
            self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), self.original_cli)
            self.assertEqual(self.assets_snapshot(), self.original_assets)
        else:
            self.assertFalse((self.bin / "emuwiz-cli").exists())
            self.assertFalse((self.bin / "emuwiz").exists())
            self.assertFalse((self.bin / "archivefs-cli").is_symlink())
            self.assertFalse((self.data / "applications/io.github.kiehntre.emuwiz.desktop").exists())
            self.assertFalse((self.data / "icons").exists())

    def assets_snapshot(self):
        result = {}
        for base in (self.bin, self.data / "applications", self.data / "icons"):
            if base.exists():
                for path in base.rglob("*"):
                    if path.is_symlink() or path.is_file():
                        # Staging files are not published assets.
                        if path.name.startswith("."):
                            continue
                        st = path.lstat()
                        content = os.readlink(path) if path.is_symlink() else path.read_bytes()
                        result[str(path)] = (st.st_ino, st.st_mtime_ns, content)
        return result

    def test_prepublication_replacements(self):
        # Separate fixtures make every interleaving independent and repeatable.
        for kind, had_install in (("regular", True), ("symlink", True),
                                  ("regular", False), ("directory", True),
                                  ("bookkeeping", True), ("in-place", True)):
            with self.subTest(kind=kind, had_install=had_install):
                case = OwnershipTests()
                case.setUp()
                try:
                    if had_install:
                        case.installed()
                    case.hook('install_binary_slot bin-emuwiz-cli "$src_cli"', case.mutation(kind))
                    result = case.run_install()
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertIn("refusing further publication", result.stderr)
                    case.assert_no_publication(had_install)
                    case.assert_foreign_preserved(kind)
                finally:
                    case.doCleanups()

    def test_replacement_inside_first_copy(self):
        # Reproduce the review's cp interposition, including the initially absent
        # manifest case. Copying to staging must not publish a managed destination.
        for kind, had_install in (("regular", True), ("symlink", True), ("regular", False)):
            with self.subTest(kind=kind, had_install=had_install):
                case = OwnershipTests()
                case.setUp()
                try:
                    if had_install:
                        case.installed()
                    spy = case.root / "spy"
                    spy.mkdir()
                    marker = shlex.quote(str(case.root / "mutated"))
                    (spy / "cp").write_text(
                        "#!/bin/sh\n"
                        f"if [ ! -e {marker} ]; then\n: > {marker}\n"
                        + case.mutation(kind) + "\nfi\nexec /usr/bin/cp \"$@\"\n"
                    )
                    (spy / "cp").chmod(0o755)
                    case.env["PATH"] = str(spy) + os.pathsep + os.environ["PATH"]
                    result = case.run_install()
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    case.assert_no_publication(had_install)
                    case.assert_foreign_preserved(kind)
                finally:
                    case.doCleanups()

    def test_final_validation_rejects_changed_object(self):
        self.installed()
        self.hook('# No pathname rename over an existing manifest:', self.mutation("regular"))
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assert_foreign_preserved("regular")
        # This is a late failure: already published binaries remain, no rollback.
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(),
                         (self.bundle / "bin/emuwiz-cli").read_bytes())

    def test_change_during_publication_stops_sibling_binary(self):
        self.installed()
        old_gui_stat = (self.bin / "emuwiz").stat()
        self.hook('install_binary_slot bin-emuwiz "$src_gui"', self.mutation("regular"))
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assert_foreign_preserved("regular")
        self.assertEqual((self.bin / "emuwiz-cli").read_bytes(),
                         (self.bundle / "bin/emuwiz-cli").read_bytes())
        self.assertEqual((self.bin / "emuwiz").stat().st_ino, old_gui_stat.st_ino)

    def test_identical_content_on_replaced_inode_is_rejected(self):
        self.installed()
        m = shlex.quote(str(self.manifest))
        self.hook('install_binary_slot bin-emuwiz-cli "$src_cli"',
                  f"cp -- {m} {m}.replacement\nmv -- {m}.replacement {m}")
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assert_no_publication(True)
        self.assertEqual(self.manifest.read_bytes(), self.original_manifest)

    def test_descriptor_write_never_clobbers_replacement_in_last_window(self):
        for kind in ("regular", "symlink", "directory", "bookkeeping"):
            with self.subTest(kind=kind):
                case = OwnershipTests()
                case.setUp()
                try:
                    case.installed()
                    case.hook('    cat -- "$manifest_tmp" > /proc/self/fd/9', case.mutation(kind))
                    result = case.run_install()
                    self.assertNotEqual(result.returncode, 0)
                    case.assert_foreign_preserved(kind)
                finally:
                    case.doCleanups()

    def test_changed_parent_with_same_bookkeeping_inode_is_rejected(self):
        data = shlex.quote(str(self.data))
        directory = shlex.quote(str(self.directory))
        self.hook('install_binary_slot bin-emuwiz-cli "$src_cli"',
                  f"mv -- {data} {data}.old\nmkdir -- {data}\n"
                  f"mv -- {data}.old/emuwiz-installer {directory}")
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("bookkeeping location changed", result.stderr)
        self.assert_no_publication(False)

    def test_fresh_manifest_creation_never_clobbers_last_window_collision(self):
        for kind in ("regular", "symlink", "directory", "bookkeeping", "bookkeeping-empty"):
            with self.subTest(kind=kind):
                case = OwnershipTests()
                case.setUp()
                try:
                    case.hook('    ln -T -- "$manifest_tmp" /proc/self/fd/8/manifest', case.mutation(kind))
                    result = case.run_install()
                    self.assertNotEqual(result.returncode, 0)
                    if kind.startswith("bookkeeping"):
                        self.assertIn("bookkeeping location changed", result.stderr)
                    else:
                        self.assertIn("creation collided", result.stderr)
                    if kind == "bookkeeping-empty":
                        self.assertEqual(list(case.directory.iterdir()), [])
                    else:
                        case.assert_foreign_preserved(kind)
                finally:
                    case.doCleanups()

    def test_legitimate_fresh_reinstall_and_uninstall(self):
        self.installed()
        self.assertEqual(self.run_install().returncode, 0)
        self.assertEqual(self.run_install().returncode, 0)
        self.assertEqual(self.run_install("--uninstall").returncode, 0)
        self.assertFalse((self.bin / "emuwiz-cli").exists())

    def test_replace_foreign_backs_up_and_does_not_authorize_later_replacement(self):
        self.directory.mkdir(parents=True)
        self.manifest.write_bytes(FOREIGN)
        result = self.run_install("--replace-foreign")
        self.assertEqual(result.returncode, 0, result.stderr)
        backups = list(self.directory.glob(".emuwiz-foreign-backup.*/manifest"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), FOREIGN)
        self.original_cli = (self.bin / "emuwiz-cli").read_bytes()
        self.original_assets = self.assets_snapshot()
        self.hook('install_binary_slot bin-emuwiz-cli "$src_cli"', self.mutation("regular"))
        result = self.run_install("--replace-foreign")
        self.assertNotEqual(result.returncode, 0)
        self.assert_foreign_preserved("regular")
        self.assert_no_publication(True)

    def test_directory_lock_serializes_install_and_uninstall(self):
        self.installed()
        fd = os.open(self.directory, os.O_RDONLY | os.O_DIRECTORY)
        self.addCleanup(os.close, fd)
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        for flags in ((), ("--uninstall",)):
            result = self.run_install(*flags)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("bookkeeping lock", result.stderr)
            self.assert_no_publication(True)
            self.assertEqual(self.manifest.read_bytes(), self.original_manifest)

    def test_strict_parser(self):
        self.installed()
        valid = self.original_manifest
        cases = {
            "end payload": valid.replace(b"end\n", b"end unexpected-payload\n"),
            "unterminated unknown trailer": valid + b"unknown-field payload",
            "unterminated whitespace trailer": valid + b"   ",
            "NUL at end": valid.removesuffix(b"\n") + b"\x00\n",
            "unknown before end": valid.replace(b"end\n", b"unknown-field payload\nend\n"),
            "content after end": valid + b"# forbidden trailer\n",
            "truncated records": valid[:valid.index(b"bin-emuwiz-cli ") + 30],
            "duplicate schema": b"schema_version 2\n" + valid,
            "duplicate root": b"bin_dir duplicate\n" + valid,
            "duplicate data home": b"data_home duplicate\n" + valid,
            "duplicate count": b"record_count 11\n" + valid,
            "missing required field": b"\n".join(line for line in valid.split(b"\n")
                                                  if not line.startswith(b"data_home ")),
        }
        for description, malformed in cases.items():
            with self.subTest(description=description):
                self.manifest.write_bytes(malformed)
                result = self.run_install()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("refusing to install managed files", result.stderr)
                self.assertEqual(self.manifest.read_bytes(), malformed)
                self.assert_no_publication(True)
        self.manifest.write_bytes(valid)
        for remove_newline in (False, True):
            if remove_newline:
                self.manifest.write_bytes(self.manifest.read_bytes().removesuffix(b"\n"))
            result = self.run_install()
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_strict_parser_is_shared_with_uninstall(self):
        self.installed()
        for malformed in (self.original_manifest.replace(b"end\n", b"end payload\n"),
                          self.original_manifest + b"unknown unterminated"):
            self.manifest.write_bytes(malformed)
            result = self.run_install("--uninstall")
            self.assertIn("leaving foreign path untouched", result.stderr)
            self.assertEqual((self.bin / "emuwiz-cli").read_bytes(), self.original_cli)


if __name__ == "__main__":
    unittest.main()
