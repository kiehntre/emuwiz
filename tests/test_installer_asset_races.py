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

    # --- preflight(): busy detection bound to the verified inode --------------

    def owned_binary(self, content=b"old bytes\n", mode=0o644):
        path = self.parent / "destination"
        path.write_bytes(content)
        path.chmod(mode)  # before the snapshot: chmod changes mode and ctime
        fd = os.open(path, os.O_RDONLY)
        try:
            digest = self.helper["fingerprint"](fd)
        finally:
            os.close(fd)
        return path, self.helper["snapshot"]("destination"), digest

    def preflight(self, expected, digest):
        self.helper["preflight"](str(self.staged), "destination", expected, digest,
                                 str(self.parent), str(self.parent))

    def test_direct_preflight_accepts_an_idle_owned_binary_without_touching_it(self):
        path, expected, digest = self.owned_binary()
        before = path.stat()
        self.preflight(expected, digest)
        after = path.stat()
        self.assertEqual(path.read_bytes(), b"old bytes\n")
        for field in ("st_ino", "st_mode", "st_size", "st_nlink", "st_mtime_ns", "st_ctime_ns"):
            self.assertEqual(getattr(before, field), getattr(after, field), field)

    def test_direct_preflight_reports_the_executing_inode_as_busy_and_leaves_it_alone(self):
        path, expected, digest = self.owned_binary(Path(ownership.shutil.which("cat")).read_bytes(), 0o755)
        running = subprocess.Popen([str(path)], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL)
        try:
            before = path.stat()
            with self.assertRaises(self.helper["Busy"]):
                self.preflight(expected, digest)
            after = path.stat()
            # Verifying the digest reads the file (atime may move, as with the
            # installer's own gate); nothing the write probe could change does.
            for field in ("st_ino", "st_mode", "st_size", "st_nlink", "st_mtime_ns", "st_ctime_ns"):
                self.assertEqual(getattr(before, field), getattr(after, field), field)
            self.assertEqual(path.read_bytes(), Path(ownership.shutil.which("cat")).read_bytes())
            self.assertEqual(self.helper["snapshot"]("destination"), expected)
            running.communicate(timeout=30)
            self.preflight(expected, digest)  # the same call succeeds once it exits
        finally:
            if running.poll() is None:
                running.kill()
            running.communicate()

    def test_direct_preflight_does_not_follow_or_accept_a_replaced_pathname(self):
        path, expected, digest = self.owned_binary()
        replacement = self.parent / "replacement"
        replacement.write_bytes(path.read_bytes())  # identical bytes, different inode
        os.replace(replacement, path)
        with self.assertRaisesRegex(RuntimeError, "changed"):
            self.preflight(expected, digest)
        path.unlink()
        path.symlink_to(self.staged)
        with self.assertRaises(OSError):  # O_NOFOLLOW: never opens the link target
            self.preflight(expected, digest)

    def test_direct_preflight_skips_the_write_probe_when_bytes_are_identical(self):
        path, expected, digest = self.owned_binary(self.staged.read_bytes())
        real_open = os.open
        flags = []

        def recording_open(name, flag, *args, **kwargs):
            flags.append(flag)
            return real_open(name, flag, *args, **kwargs)

        with mock.patch.object(self.helper["os"], "open", recording_open):
            self.preflight(expected, digest)
        self.assertTrue(flags)
        self.assertFalse([flag for flag in flags if flag & os.O_ACCMODE == os.O_WRONLY])


class RunningBinaryTests(unittest.TestCase):
    """Upgrading while EmuWiz is running: refuse first, publish nothing."""

    HUMAN = ("currently running", "Close EmuWiz and try again")

    def setUp(self):
        self.case = ownership.OwnershipTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)
        self.processes = []
        self.addCleanup(self.stop_everything)
        c = self.case
        self.elf = Path(ownership.shutil.which("cat")).read_bytes()
        for name in ("emuwiz", "emuwiz-cli"):
            (c.bundle / "bin" / name).write_bytes(self.elf)
            (c.bundle / "bin" / name).chmod(0o755)
        result = c.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.installed_state = self.tree_state()
        self.installed_manifest = c.manifest.read_bytes()

    def stop_everything(self):
        for process in self.processes:
            if process.poll() is None:
                process.stdin.close()
                try:
                    process.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()

    def start(self, name, directory=None):
        """A real executing copy of the installed ELF. Popen returns after exec."""
        path = (directory or self.case.bin) / name
        process = subprocess.Popen([str(path)], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL)
        self.processes.append(process)
        return process

    def close(self, process):
        process.stdin.close()
        process.wait(timeout=30)  # EOF, never a sleep

    def tree_state(self):
        """Every entry, staging dot-files and the bookkeeping directory included."""
        c = self.case
        state = {}
        for base in (c.bin, c.data / "applications", c.data / "icons", c.directory):
            for path in sorted([base, *base.rglob("*")]):
                st = path.lstat()
                content = os.readlink(path) if path.is_symlink() else (
                    path.read_bytes() if path.is_file() else None)
                state[str(path)] = (st.st_ino, st.st_mode, st.st_size, st.st_mtime_ns, content)
        return state

    def release(self, cli=True, gui=True, everything=True):
        """Stage a new release. By default every managed file changes."""
        c = self.case
        for name, changed in (("emuwiz-cli", cli), ("emuwiz", gui)):
            (c.bundle / "bin" / name).write_bytes(self.elf + (b"\0release-2" if changed else b""))
        if everything:
            template = c.bundle / "assets/linux/io.github.kiehntre.emuwiz.desktop.in"
            template.write_text(template.read_text() + "# release 2\n")
            for size in (32, 64, 128, 256, 512):
                icon = c.bundle / f"assets/branding/emuwiz-logo-{size}.png"
                icon.write_bytes(icon.read_bytes() + b"release 2")

    def assert_refused_untouched(self, result, named):
        c = self.case
        self.assertNotEqual(result.returncode, 0, result.stdout)
        for phrase in self.HUMAN:
            self.assertIn(phrase, result.stderr)
        self.assertIn(f"{c.bin}/{named} is being executed", result.stderr)
        for cryptic in ("Text file busy", "Errno", "/proc/self/fd", "destination changed"):
            self.assertNotIn(cryptic, result.stderr)
        self.assertEqual(c.manifest.read_bytes(), self.installed_manifest)  # byte-identical
        # Nothing published, no staged leftover, no new inode anywhere.
        self.assertEqual(self.tree_state(), self.installed_state)

    # 1-3: refuse before ANY publication -----------------------------------

    def test_gui_running_with_changed_bytes_publishes_nothing(self):
        self.release()
        self.start("emuwiz")
        self.assert_refused_untouched(self.case.run_install(), "emuwiz")

    def test_cli_running_with_changed_bytes_publishes_nothing(self):
        self.release()
        self.start("emuwiz-cli")
        self.assert_refused_untouched(self.case.run_install(), "emuwiz-cli")

    def test_both_running_with_changed_bytes_publishes_nothing(self):
        self.release()
        self.start("emuwiz")
        self.start("emuwiz-cli")
        self.assert_refused_untouched(self.case.run_install(), "emuwiz-cli")

    def test_running_gui_cannot_leave_an_idle_changed_cli_published(self):
        # The reported failure: CLI idle and changed, GUI running and changed.
        # The CLI is published first, so the preflight has to stop it.
        self.release()
        self.start("emuwiz")
        result = self.case.run_install()
        self.assert_refused_untouched(result, "emuwiz")
        self.assertEqual((self.case.bin / "emuwiz-cli").read_bytes(), self.elf)

    # 4-5: no unnecessary refusal -------------------------------------------

    def test_running_binary_with_identical_bytes_is_not_refused(self):
        c = self.case
        self.release(cli=False, gui=False, everything=False)
        self.start("emuwiz")
        self.start("emuwiz-cli")
        inodes = {name: (c.bin / name).stat().st_ino for name in ("emuwiz", "emuwiz-cli")}
        result = c.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(inodes, {name: (c.bin / name).stat().st_ino for name in inodes})
        self.assertEqual((c.bin / "emuwiz").read_bytes(), self.elf)

    def test_running_unchanged_gui_does_not_block_an_idle_changed_cli(self):
        c = self.case
        self.release(cli=True, gui=False, everything=False)
        self.start("emuwiz")
        result = c.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((c.bin / "emuwiz-cli").read_bytes(), self.elf + b"\0release-2")
        self.assertEqual((c.bin / "emuwiz").read_bytes(), self.elf)

    def test_idle_owned_binaries_with_changed_bytes_upgrade(self):
        c = self.case
        self.release()
        result = c.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in ("emuwiz", "emuwiz-cli"):
            self.assertEqual((c.bin / name).read_bytes(), self.elf + b"\0release-2")
        self.assertNotEqual(c.manifest.read_bytes(), self.installed_manifest)

    # 6-7: bound to the verified object ------------------------------------

    def test_an_unrelated_running_copy_does_not_make_an_idle_binary_busy(self):
        # Same bytes, same name, different inode: busy-ness is per inode, not
        # per name, content or process.
        c = self.case
        elsewhere = c.root / "elsewhere"
        elsewhere.mkdir()
        for name in ("emuwiz", "emuwiz-cli"):
            (elsewhere / name).write_bytes(self.elf)
            (elsewhere / name).chmod(0o755)
            self.start(name, elsewhere)
        self.release()
        result = c.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((c.bin / "emuwiz").read_bytes(), self.elf + b"\0release-2")

    def test_pathname_replacement_between_gate_and_preflight_is_refused_not_misread(self):
        c = self.case
        self.release()
        running = self.start("emuwiz")  # the verified inode is executing...
        path = c.bin / "emuwiz"
        # ...and the name is swapped for an idle foreign file just before the
        # preflight opens it: that file must neither be inspected as the owned
        # object nor written.
        script = c.script.read_text()
        anchor = "    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=7)\n    held = os.fstat(fd)\n    if not stat.S_ISREG(held.st_mode) or identity(held) != expected:\n        raise RuntimeError(\"destination changed during installation\")\n    if fingerprint(fd) != digest:"
        self.assertEqual(script.count(anchor), 1)
        mutation = (f"    if name == 'emuwiz':\n        os.unlink({str(path)!r})\n"
                    f"        open({str(path)!r}, 'wb').write({FOREIGN!r})\n")
        c.script.write_text(script.replace(anchor, mutation + anchor))
        result = c.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("destination changed during installation", result.stderr)
        self.assertNotIn("currently running", result.stderr)
        self.assertEqual(path.read_bytes(), FOREIGN)
        self.assertEqual(c.manifest.read_bytes(), self.installed_manifest)
        self.assertIsNone(running.poll())

    # 8-9: existing protections unchanged ----------------------------------

    def test_user_hardlink_still_refuses_changed_bytes(self):
        c = self.case
        neighbour = c.bin / "hardlinked-neighbour"
        os.link(c.bin / "emuwiz-cli", neighbour)
        self.release(gui=False, everything=False)
        result = c.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("hardlinks", result.stderr)
        self.assertEqual(neighbour.read_bytes(), self.elf)
        self.assertEqual(c.manifest.read_bytes(), self.installed_manifest)

    def test_foreign_destination_is_still_never_overwritten(self):
        c = self.case
        self.release()
        (c.bin / "emuwiz").unlink()
        (c.bin / "emuwiz").write_bytes(FOREIGN)
        result = c.run_install()
        self.assertEqual((c.bin / "emuwiz").read_bytes(), FOREIGN)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("currently running", result.stderr)

    # 12: retry after closing -----------------------------------------------

    def test_retry_after_closing_succeeds_without_replace_foreign(self):
        c = self.case
        self.release()
        gui = self.start("emuwiz")
        cli = self.start("emuwiz-cli")
        self.assert_refused_untouched(c.run_install(), "emuwiz-cli")
        self.close(cli)
        self.assert_refused_untouched(c.run_install(), "emuwiz")  # GUI still open
        self.close(gui)
        result = c.run_install()  # plain retry: no --replace-foreign
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in ("emuwiz", "emuwiz-cli"):
            self.assertEqual((c.bin / name).read_bytes(), self.elf + b"\0release-2")
        self.assertIn(b"record_count 11\n", c.manifest.read_bytes())
        self.assertEqual(list(c.bin.glob(".emuwiz-foreign-backup.*")), [])

    # A start after the preflight keeps a readable message ------------------

    def test_a_process_started_after_the_preflight_gets_the_same_plain_message(self):
        c = self.case
        self.release()
        pidfile, fifo = c.root / "late.pid", c.root / "late.fifo"
        os.mkfifo(fifo)
        c.hook("    asset_record=$(asset_io publish",
               'if [ "$2" = bin-emuwiz ]; then\n'
               f"exec 3<>{shlex.quote(str(fifo))}\n"
               '"$bin_dir/emuwiz" <&3 3<&- >/dev/null 2>&1 7<&- 8<&- 9<&- &\n'
               f"echo $! > {shlex.quote(str(pidfile))}\n"
               'until [ "$(readlink /proc/$!/exe 2>/dev/null)" = "$bin_dir/emuwiz" ]; do sleep 0.01; done\n'
               "fi")
        try:
            result = c.run_install()
        finally:
            if pidfile.exists():
                try:
                    os.kill(int(pidfile.read_text()), 9)
                except ProcessLookupError:
                    pass
        self.assertNotEqual(result.returncode, 0)
        for phrase in self.HUMAN:
            self.assertIn(phrase, result.stderr)
        self.assertIn("run the installer again", result.stderr)
        for cryptic in ("Text file busy", "Errno", "/proc/self/fd"):
            self.assertNotIn(cryptic, result.stderr)
        self.assertEqual(c.manifest.read_bytes(), self.installed_manifest)


if __name__ == "__main__":
    unittest.main()
