#!/usr/bin/env python3
"""Deterministic asset-destination races in private copies of the installer."""
import errno
import fcntl
import io
import os
import select
from pathlib import Path
import shlex
import subprocess
import tempfile
from contextlib import redirect_stdout
from unittest import mock
import unittest

import test_installer_ownership as ownership

FOREIGN = ownership.FOREIGN


class AssetRaceTests(unittest.TestCase):
    def setUp(self):
        self.case = ownership.OwnershipTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)

    def destination(self, asset):
        c = self.case
        return {
            "binary": c.bin / "emuwiz-cli",
            "alias": c.bin / "archivefs-cli",
            "desktop": c.data / "applications/io.github.kiehntre.emuwiz.desktop",
            "icon": c.data / "icons/hicolor/32x32/apps/io.github.kiehntre.emuwiz.png",
        }[asset]

    def collision(self, path, kind):
        p, outside = shlex.quote(str(path)), shlex.quote(str(self.case.outside))
        remove = f"rm -f -- {p}\n"  # fixtures contain only a file/symlink here
        if kind == "regular":
            return remove + f"cp -- {outside} {p}"
        if kind == "symlink":
            return remove + f"ln -s -- {outside} {p}"
        if kind == "directory":
            return remove + f"mkdir -- {p}\ncp -- {outside} {p}/canary"
        raise AssertionError(kind)

    def publication_hook(self, asset, mutation):
        markers = {
            "binary": '    publish_asset file "$slot" "$binary_tmp" "$gate"',
            "alias": '    publish_asset symlink "$slot" "$target" "$gate"',
            "desktop": '    publish_asset file desktop "$desktop_tmp" "$desktop_gate"',
            "icon": '    publish_asset file "icon-$size" "$icon_tmp" "$icon_gate"',
        }
        condition = '"$slot" = bin-emuwiz-cli' if asset == "binary" else (
            '"$slot" = alias-archivefs-cli' if asset == "alias" else
            '"$size" = 32' if asset == "icon" else "1 = 1")
        self.case.hook(markers[asset], f"if [ {condition} ]; then\n{mutation}\nfi")

    def assert_preserved(self, path, kind):
        if kind == "symlink":
            self.assertTrue(path.is_symlink())
            self.assertEqual(os.readlink(path), str(self.case.outside))
        elif kind == "directory":
            self.assertTrue(path.is_dir())
            self.assertEqual((path / "canary").read_bytes(), FOREIGN)
            self.assertEqual([p.name for p in path.iterdir()], ["canary"])
        else:
            self.assertFalse(path.is_symlink())
            self.assertEqual(path.read_bytes(), FOREIGN)

    def check_race(self, asset, kind):
        path = self.destination(asset)
        self.publication_hook(asset, self.collision(path, kind))
        result = self.case.run_install()
        self.assert_preserved(path, kind)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("destination changed during installation", result.stderr)
        self.assertFalse(self.case.manifest.exists())

    def test_binary_file_race(self):
        self.check_race("binary", "regular")

    def test_binary_symlink_race(self):
        self.check_race("binary", "symlink")

    def test_alias_file_race(self):
        self.check_race("alias", "regular")

    def test_desktop_file_race(self):
        self.check_race("desktop", "regular")

    def test_icon_file_race(self):
        self.check_race("icon", "regular")

    def test_directory_collision(self):
        self.check_race("desktop", "directory")

    def test_remaining_asset_kind_races(self):
        for asset, kind in (("binary", "directory"), ("alias", "symlink"),
                            ("alias", "directory"), ("desktop", "symlink"),
                            ("icon", "symlink"), ("icon", "directory")):
            with self.subTest(asset=asset, kind=kind):
                case = AssetRaceTests()
                case.setUp()
                try:
                    case.check_race(asset, kind)
                finally:
                    case.doCleanups()

    def test_every_managed_asset_slot(self):
        slots = [("bin-emuwiz-cli", "bin", "emuwiz-cli"), ("bin-emuwiz", "bin", "emuwiz")]
        slots += [("alias-" + name, "bin", name) for name in ("archivefs-cli", "emuwiz-gui", "archivefs-gui")]
        slots += [("desktop", "applications", "io.github.kiehntre.emuwiz.desktop")]
        slots += [(f"icon-{size}", f"icons/hicolor/{size}x{size}/apps", "io.github.kiehntre.emuwiz.png")
                  for size in (32, 64, 128, 256, 512)]
        for slot, parent, name in slots:
            with self.subTest(slot=slot):
                case = AssetRaceTests()
                case.setUp()
                c = case.case
                try:
                    path = (c.bin if parent == "bin" else c.data / parent) / name
                    c.hook('    asset_record=$(asset_io publish',
                           f'if [ "$2" = {shlex.quote(slot)} ]; then\n'
                           + case.collision(path, "regular") + "\nfi")
                    result = c.run_install()
                    case.assert_preserved(path, "regular")
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(c.manifest.exists())
                finally:
                    case.doCleanups()

    def test_managed_destinations_replaced_after_gate(self):
        for asset in ("binary", "alias", "desktop", "icon"):
            for kind in ("regular", "symlink", "directory"):
                with self.subTest(asset=asset, kind=kind):
                    case = AssetRaceTests()
                    case.setUp()
                    try:
                        case.case.installed()
                        path = case.destination(asset)
                        case.publication_hook(asset, case.collision(path, kind))
                        result = case.case.run_install()
                        case.assert_preserved(path, kind)
                        self.assertNotEqual(result.returncode, 0)
                        self.assertEqual(case.case.manifest.read_bytes(), case.case.original_manifest)
                    finally:
                        case.doCleanups()

    def test_fresh_publication(self):
        c = self.case
        result = c.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in ("emuwiz-cli", "emuwiz"):
            self.assertEqual((c.bin / name).read_bytes(), (c.bundle / "bin" / name).read_bytes())
            self.assertTrue(os.access(c.bin / name, os.X_OK))
        self.assertIn(b"record_count 11\n", c.manifest.read_bytes())

    def test_legitimate_upgrade(self):
        c = self.case
        c.installed()
        result = c.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((c.bin / "emuwiz-cli").read_bytes(), (c.bundle / "bin/emuwiz-cli").read_bytes())
        self.assertNotEqual(c.manifest.read_bytes(), c.original_manifest)

    def test_legitimate_reinstall(self):
        c = self.case
        self.assertEqual(c.run_install().returncode, 0)
        original = c.assets_snapshot()
        self.assertEqual(c.run_install().returncode, 0)
        self.assertEqual(c.assets_snapshot(), original)

    def test_upgrade_all_file_assets_and_shorter_binary(self):
        c = self.case
        c.installed()
        template = c.bundle / "assets/linux/io.github.kiehntre.emuwiz.desktop.in"
        template.write_text(template.read_text() + "# upgraded desktop\n")
        for size in (32, 64, 128, 256, 512):
            icon = c.bundle / f"assets/branding/emuwiz-logo-{size}.png"
            icon.write_bytes(icon.read_bytes() + b"new release bytes")
        for data in (b"#!/bin/sh\n# longer GUI release\nexit 0\n", b"#!/bin/sh\nexit 0\n"):
            (c.bundle / "bin/emuwiz").write_bytes(data)
            result = c.run_install()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((c.bin / "emuwiz").read_bytes(), data)
            self.assertEqual((c.bin / "emuwiz-cli").read_bytes(), (c.bundle / "bin/emuwiz-cli").read_bytes())
            self.assertIn(b"# upgraded desktop", self.destination("desktop").read_bytes())
            for size in (32, 64, 128, 256, 512):
                self.assertEqual((c.data / f"icons/hicolor/{size}x{size}/apps/io.github.kiehntre.emuwiz.png").read_bytes(),
                                 (c.bundle / f"assets/branding/emuwiz-logo-{size}.png").read_bytes())

    def test_running_native_binary_update_fails_closed(self):
        c = self.case
        ownership.shutil.copyfile(ownership.shutil.which("cat"), c.bundle / "bin/emuwiz-cli")
        c.installed()
        running = subprocess.Popen([str(c.bin / "emuwiz-cli")], stdin=subprocess.PIPE,
                                   stdout=subprocess.DEVNULL)
        try:
            result = c.run_install()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Text file busy", result.stderr)
            self.assertEqual((c.bin / "emuwiz-cli").read_bytes(), c.original_cli)
            self.assertEqual(c.manifest.read_bytes(), c.original_manifest)
            running.communicate(timeout=30)  # EOF, no timing-based race
            self.assertEqual(c.run_install().returncode, 0)
        finally:
            if running.poll() is None:
                running.kill()
            running.communicate()

    def test_owned_open_does_not_follow_a_last_window_symlink(self):
        c = self.case
        c.installed()
        path = self.destination("binary")
        c.hook('            fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=7)',
               f"            os.unlink({str(path)!r})\n"
               f"            os.symlink({str(c.outside)!r}, {str(path)!r})")
        result = c.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assert_preserved(path, "symlink")
        self.assertEqual(c.manifest.read_bytes(), c.original_manifest)

    def test_replace_foreign_backups(self):
        for asset, kind in (("binary", "regular"), ("alias", "symlink"),
                            ("desktop", "directory"), ("icon", "regular")):
            with self.subTest(asset=asset, kind=kind):
                case = AssetRaceTests()
                case.setUp()
                c = case.case
                try:
                    path = case.destination(asset)
                    path.parent.mkdir(parents=True, exist_ok=True)
                    if kind == "symlink":
                        path.symlink_to(c.outside)
                    elif kind == "directory":
                        path.mkdir()
                        (path / "canary").write_bytes(FOREIGN)
                    else:
                        path.write_bytes(FOREIGN)
                    result = c.run_install("--replace-foreign")
                    self.assertEqual(result.returncode, 0, result.stderr)
                    backups = list(path.parent.glob(".emuwiz-foreign-backup.*/" + path.name))
                    self.assertEqual(len(backups), 1)
                    case.assert_preserved(backups[0], kind)
                    self.assertIn(str(backups[0]), result.stderr)
                    self.assertNotIn("/proc/self/fd/7", result.stderr)
                finally:
                    case.doCleanups()

    def test_replace_foreign_does_not_authorize_new_collision(self):
        c = self.case
        path = self.destination("binary")
        path.write_bytes(b"original foreign file\n")
        self.publication_hook("binary", self.collision(path, "symlink"))
        result = c.run_install("--replace-foreign")
        self.assertNotEqual(result.returncode, 0)
        self.assert_preserved(path, "symlink")
        backups = list(c.bin.glob(".emuwiz-foreign-backup.*/emuwiz-cli"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), b"original foreign file\n")
        self.assertFalse(c.manifest.exists())

    def test_replace_foreign_unreadable_file(self):
        c = self.case
        path = self.destination("binary")
        path.write_bytes(FOREIGN)
        with path.open("rb") as held:
            path.chmod(0)
            result = c.run_install("--replace-foreign")
            self.assertEqual(result.returncode, 0, result.stderr)
            backups = list(c.bin.glob(".emuwiz-foreign-backup.*/emuwiz-cli"))
            self.assertEqual(len(backups), 1)
            self.assertEqual(backups[0].stat().st_mode & 0o777, 0)
            self.assertEqual(backups[0].stat().st_ino, os.fstat(held.fileno()).st_ino)
            self.assertEqual(held.read(), FOREIGN)

    def test_parent_directory_replacement(self):
        for asset in ("binary", "alias", "desktop", "icon"):
            with self.subTest(asset=asset):
                case = AssetRaceTests()
                case.setUp()
                c = case.case
                try:
                    path = case.destination(asset)
                    parent = shlex.quote(str(path.parent))
                    case.publication_hook(asset,
                        f"mv -- {parent} {parent}.old\nmkdir -- {parent}\n"
                        + case.collision(path, "regular"))
                    if asset in ("binary", "alias"):
                        c.unmanaged = Path(str(c.bin) + ".old") / "unmanaged"
                    result = c.run_install()
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("parent changed", result.stderr)
                    case.assert_preserved(path, "regular")
                    self.assertFalse(c.manifest.exists())
                finally:
                    case.doCleanups()

    def test_symlinked_parent_retarget(self):
        c = self.case
        apps = c.data / "applications"
        apps.parent.mkdir(parents=True)
        original = c.root / "original-apps"
        replacement = c.root / "replacement-apps"
        original.mkdir()
        replacement.mkdir()
        apps.symlink_to(original, target_is_directory=True)
        protected = replacement / self.destination("desktop").name
        protected.write_bytes(FOREIGN)
        self.publication_hook("desktop", f"rm -- {shlex.quote(str(apps))}\n"
                              f"ln -s -- {shlex.quote(str(replacement))} {shlex.quote(str(apps))}")
        result = c.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(protected.read_bytes(), FOREIGN)
        self.assertFalse(c.manifest.exists())

    def test_cooperating_installers_are_serialized(self):
        c = self.case
        c.hook('manifest_records_tmp=$(mktemp', "printf 'LOCKED\\n'\nIFS= read -r release_token")
        first = subprocess.Popen(["sh", str(c.script), "--prefix", str(c.bin)],
                                 env=c.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, text=True)
        try:
            self.assertTrue(select.select([first.stdout], [], [], 30)[0], "installer did not reach the held lock")
            self.assertEqual(first.stdout.readline(), "LOCKED\n")
            second = c.run_install()
            self.assertNotEqual(second.returncode, 0)
            self.assertIn("bookkeeping lock", second.stderr)
            out, err = first.communicate("continue\n", timeout=30)
            self.assertEqual(first.returncode, 0, out + err)
            self.assertIn(b"record_count 11\n", c.manifest.read_bytes())
        finally:
            if first.poll() is None:
                first.kill()
            first.communicate()

    def test_last_syscall_collision(self):
        for asset in ("binary", "alias", "desktop", "icon"):
            for kind in ("regular", "symlink", "directory"):
                with self.subTest(asset=asset, kind=kind):
                    case = AssetRaceTests()
                    case.setUp()
                    c = case.case
                    try:
                        path = case.destination(asset)
                        mutation = f"    if name == {path.name!r}:\n"
                        if kind == "regular":
                            mutation += f"        with open({str(path)!r}, 'wb') as collision: collision.write({FOREIGN!r})"
                        elif kind == "symlink":
                            mutation += f"        os.symlink({str(c.outside)!r}, {str(path)!r})"
                        else:
                            mutation += f"        os.mkdir({str(path)!r})\n        with open({str(path / 'canary')!r}, 'wb') as collision: collision.write({FOREIGN!r})"
                        c.hook('    os.link(f"/proc/self/fd/{fd}", name, dst_dir_fd=7, follow_symlinks=True)', mutation)
                        result = c.run_install()
                        self.assertNotEqual(result.returncode, 0)
                        case.assert_preserved(path, kind)
                        self.assertFalse(c.manifest.exists())
                    finally:
                        case.doCleanups()

    def test_held_update_preserves_last_write_window_replacement(self):
        for kind in ("regular", "symlink", "directory"):
            with self.subTest(kind=kind):
                case = AssetRaceTests()
                case.setUp()
                c = case.case
                try:
                    c.installed()
                    path = case.destination("binary")
                    mutation = f"                    os.unlink({str(path)!r})\n"
                    if kind == "regular":
                        mutation += f"                    with open({str(path)!r}, 'wb') as collision: collision.write({FOREIGN!r})"
                    elif kind == "symlink":
                        mutation += f"                    os.symlink({str(c.outside)!r}, {str(path)!r})"
                    else:
                        mutation += f"                    os.mkdir({str(path)!r})\n                    with open({str(path / 'canary')!r}, 'wb') as collision: collision.write({FOREIGN!r})"
                    c.hook('                    shutil.copyfileobj(input_stream, stream)', mutation)
                    result = c.run_install()
                    self.assertNotEqual(result.returncode, 0)
                    case.assert_preserved(path, kind)
                    self.assertEqual(c.manifest.read_bytes(), c.original_manifest)
                finally:
                    case.doCleanups()

    def test_last_syscall_parent_replacement(self):
        c = self.case
        path = self.destination("binary")
        c.hook('    os.link(f"/proc/self/fd/{fd}", name, dst_dir_fd=7, follow_symlinks=True)',
               f"    if name == 'emuwiz-cli':\n        os.rename({str(c.bin)!r}, {str(c.bin) + '.old'!r})\n"
               f"        os.mkdir({str(c.bin)!r})\n        with open({str(path)!r}, 'wb') as collision: collision.write({FOREIGN!r})")
        c.unmanaged = Path(str(c.bin) + ".old") / "unmanaged"
        result = c.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assert_preserved(path, "regular")
        self.assertFalse(c.manifest.exists())
        self.assertEqual((Path(str(c.bin) + ".old") / "emuwiz-cli").read_bytes(), (c.bundle / "bin/emuwiz-cli").read_bytes())

    def test_managed_in_place_change_after_gate(self):
        c = self.case
        c.installed()
        path = self.destination("binary")
        self.publication_hook("binary", f"cp -- {shlex.quote(str(c.outside))} {shlex.quote(str(path))}")
        result = c.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assert_preserved(path, "regular")
        self.assertEqual(c.manifest.read_bytes(), c.original_manifest)

    def test_hardlinked_owned_update_refuses_to_modify_neighbour(self):
        c = self.case
        c.installed()
        neighbour = c.bin / "hardlinked-neighbour"
        os.link(self.destination("binary"), neighbour)
        result = c.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("hardlinks", result.stderr)
        self.assertEqual(neighbour.read_bytes(), c.original_cli)
        self.assertEqual(c.manifest.read_bytes(), c.original_manifest)

    def test_missing_safe_syscall_has_no_fallback(self):
        c = self.case
        c.hook('    os.link(f"/proc/self/fd/{fd}", name, dst_dir_fd=7, follow_symlinks=True)',
               '    raise OSError(38, "linkat unavailable")')
        result = c.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("linkat unavailable", result.stderr)
        self.assertFalse(self.destination("binary").exists())
        self.assertFalse(c.manifest.exists())

    def test_unavailable_runtime_has_no_fallback(self):
        c = self.case
        c.hook('    if sys.platform != "linux"', '    sys.platform = "unsupported"')
        result = c.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("support is required", result.stderr)
        self.assertFalse(self.destination("binary").exists())
        self.assertFalse(c.manifest.exists())


class AssetHelperTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="emuwiz-asset-helper-")
        self.addCleanup(self.temp.cleanup)
        self.parent = Path(self.temp.name)
        self.staged = self.parent / "staged"
        self.staged.write_bytes(b"new bytes\n")
        try:
            self.saved = os.dup(7)
        except OSError:
            self.saved = None
        fd = os.open(self.parent, os.O_RDONLY | os.O_DIRECTORY)
        os.dup2(fd, 7)
        if fd != 7:
            os.close(fd)
        self.addCleanup(self.restore_fd)
        script = (ownership.ROOT / "install.sh").read_text()
        source = script.split("<<'EMUWIZ_ASSET_PY'\n", 1)[1].split("\nEMUWIZ_ASSET_PY", 1)[0]
        self.helper = {"__name__": "asset_test"}
        exec(compile(source, "embedded_asset_helper", "exec"), self.helper)

    def restore_fd(self):
        os.close(7)
        if self.saved is not None:
            os.dup2(self.saved, 7)
            os.close(self.saved)

    def publish(self, gate="absent", expected="absent", digest="-", kind="file", source=None):
        before = {number for number in os.listdir("/proc/self/fd")
                  if os.path.exists("/proc/self/fd/" + number)}
        output = io.StringIO()
        try:
            with redirect_stdout(output):
                self.helper["publish"](kind, "test", source or str(self.staged), "destination", expected, gate,
                                       digest, str(self.parent), str(self.parent))
            return output.getvalue()
        finally:
            for number in set(os.listdir("/proc/self/fd")) - before:
                try:
                    os.close(int(number))
                except OSError:
                    pass

    def test_direct_fresh_file_and_symlink(self):
        self.assertIn("test file", self.publish())
        self.assertEqual((self.parent / "destination").read_bytes(), self.staged.read_bytes())
        (self.parent / "destination").unlink()
        self.assertEqual(self.publish(kind="symlink", source="emuwiz"), "test symlink emuwiz\n")
        self.assertEqual(os.readlink(self.parent / "destination"), "emuwiz")

    def test_direct_owned_digest_is_required(self):
        path = self.parent / "destination"
        path.write_bytes(FOREIGN)
        expected = self.helper["snapshot"]("destination")
        with self.assertRaisesRegex(RuntimeError, "content"):
            self.publish("owned", expected, "0" * 64)
        self.assertEqual(path.read_bytes(), FOREIGN)

    def test_direct_foreign_gate_never_authorizes(self):
        with self.assertRaisesRegex(RuntimeError, "not authorized"):
            self.publish("foreign")
        self.assertFalse((self.parent / "destination").exists())

    def test_direct_unsupported_linkat_fails_closed(self):
        with mock.patch.object(self.helper["os"], "link", side_effect=OSError(errno.ENOSYS, "unavailable")):
            with self.assertRaises(OSError):
                self.publish()
        self.assertFalse((self.parent / "destination").exists())


if __name__ == "__main__":
    unittest.main()
