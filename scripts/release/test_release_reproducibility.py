#!/usr/bin/env python3
"""Synthetic current-packager and real orchestration/path-remapping regressions."""
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

import test_package_release as fixtures

REPO = Path(__file__).resolve().parents[2]
COMPARE = REPO / "scripts/compare-release-builds.sh"


def run(*args, **kwargs):
    return subprocess.run(args, text=True, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, **kwargs)


def checksum(path):
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    Path(str(path) + ".sha256").write_text(f"{digest}  {path.name}\n")


class ReproducibilityTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.ReleasePackagerTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.tearDown)
        self.root = self.fixture.root

    def pair(self):
        archives = []
        for name in ("first", "second"):
            release, result = self.fixture._package(name, archive=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            archives.append(release.parent / (release.name + ".tar.xz"))
        return archives

    def compare(self, first, second):
        return run("bash", str(COMPARE), "--archives", str(first), str(second))

    def mutate(self, path, mutate):
        with tarfile.open(path, "r:xz") as source:
            members = [(copy.copy(m), source.extractfile(m).read() if m.isfile() else None)
                       for m in source]
        mutate(members)
        with tarfile.open(path, "w:xz", format=tarfile.PAX_FORMAT, preset=9) as output:
            for member, data in members:
                output.addfile(member, io.BytesIO(data) if data is not None else None)
        checksum(path)

    def test_independent_packages_and_metadata_equal(self):
        first, second = self.pair()
        self.assertEqual(first.read_bytes(), second.read_bytes())
        result = self.compare(first, second)
        self.assertEqual(result.returncode, 0, result.stdout)
        with tarfile.open(first, "r:xz") as archive:
            members = archive.getmembers()
            self.assertEqual([m.name for m in members], sorted(m.name for m in members))
            for member in members:
                self.assertEqual(member.mtime, int(self.fixture.epoch))
                self.assertEqual((member.uid, member.gid, member.uname, member.gname), (0, 0, "", ""))
                self.assertIn(member.mode, (0o644, 0o755))
                self.assertFalse(member.issym() or member.islnk())
                self.assertFalse(set(member.pax_headers) & {"atime", "ctime", "mtime"})

    def test_input_order_mtime_permissions_and_long_pax_names_normalized(self):
        outputs = []
        for number, order in enumerate((("z", "a", "x" * 120), ("x" * 120, "a", "z"))):
            root = self.root / str(number) / "bundle"
            root.mkdir(parents=True)
            for name in order:
                path = root / name
                path.write_bytes(name.encode())
                path.chmod(0o600 if number else 0o644)
                os.utime(path, (number + 100, number + 100))
            output = self.root / f"{number}.tar.xz"
            fixtures.packager.create_archive(root, output, 1700000000)
            outputs.append(output)
        self.assertEqual(outputs[0].read_bytes(), outputs[1].read_bytes())
        with tarfile.open(outputs[0]) as archive:
            self.assertEqual(archive.getmembers()[-2].pax_headers.get("path"), "bundle/" + "x" * 120)

    def test_changed_binary_detected_with_member_hash(self):
        first, second = self.pair()
        def change(members):
            index = next(i for i, (m, _) in enumerate(members) if m.name.endswith("/bin/emuwiz"))
            member, data = members[index]
            members[index] = (member, data[:-1] + b"x")
        self.mutate(second, change)
        result = self.compare(first, second)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("/bin/emuwiz", result.stdout)
        self.assertIn('"sha256"', result.stdout)

    def test_changed_metadata_detected(self):
        first, second = self.pair()
        for field, value in (("mtime", 42), ("uid", 42), ("gname", "other"),
                             ("mode", 0o700), ("pax_headers", {"comment": "different"})):
            with self.subTest(field=field):
                shutil.copyfile(first, second)
                self.mutate(second, lambda members: setattr(members[0][0], field, value))
                result = self.compare(first, second)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(field, result.stdout)

    def test_changed_member_order_detected(self):
        first, second = self.pair()
        self.mutate(second, lambda members: members.reverse())
        self.assertNotEqual(self.compare(first, second).returncode, 0)

    def test_different_xz_encoding_is_not_silently_accepted(self):
        first, second = self.pair()
        import lzma
        second.write_bytes(lzma.compress(lzma.decompress(first.read_bytes()), preset=0))
        checksum(second)
        result = self.compare(first, second)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("encoding differs", result.stdout)

    def test_identically_wrong_checksum_sidecars_refused(self):
        first, second = self.pair()
        for path in (first, second):
            Path(str(path) + ".sha256").write_text(f"{'0' * 64}  {path.name}\n")
        self.assertNotEqual(self.compare(first, second).returncode, 0)

    def test_current_archive_verifier_negative_fixtures(self):
        first, _ = self.pair()
        result = run("bash", str(REPO / "scripts/test-release-artifact-verifier.sh"), str(first))
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("negative tests passed", result.stdout)

    def test_active_workflows_use_current_format(self):
        for name in ("ci", "release"):
            text = (REPO / f".github/workflows/{name}.yml").read_text()
            self.assertNotIn(".tar.gz", text)
            self.assertIn(".tar.xz", text)
        self.assertIn("release_bundle_name", (REPO / ".github/workflows/release.yml").read_text())

    def make_repo(self, name):
        root = self.root / name
        (root / "scripts").mkdir(parents=True)
        for script in ("compare-release-builds.sh", "release-common.sh", "build-release.sh"):
            shutil.copyfile(REPO / "scripts" / script, root / "scripts" / script)
            (root / "scripts" / script).chmod(0o755)
        return root

    def commit(self, root):
        for args in (("init", "-q"), ("add", "."),
                     ("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                      "-c", "core.hooksPath=/dev/null", "commit", "-qm", "synthetic fixture")):
            result = run("git", "-C", str(root), *args)
            self.assertEqual(result.returncode, 0, result.stdout)

    def test_builds_use_distinct_detached_source_target_and_output_roots(self):
        root = self.make_repo("orchestration")
        # Stub only the expensive compiler/package boundary; run the actual
        # comparison driver, local clones, artifact naming and archive checks.
        (root / "scripts/build-release.sh").write_text('''#!/usr/bin/env python3
import hashlib, io, json, os, pathlib, subprocess, sys, tarfile
args=dict(zip(sys.argv[1::2],sys.argv[2::2]))
out=pathlib.Path(args['--output-dir']); out.mkdir(parents=True)
source=pathlib.Path(__file__).resolve().parents[1]
record=dict(source=str(source), target=args['--target-dir'], output=str(out),
            head=subprocess.check_output(['git','-C',str(source),'rev-parse','HEAD'],text=True).strip(),
            branch=subprocess.check_output(['git','-C',str(source),'branch','--show-current'],text=True).strip())
(out/'roots.json').write_text(json.dumps(record))
arch={'x86_64':'x86_64','aarch64':'aarch64'}[os.uname().machine]
path=out/f'emuwiz-0.9.0-linux-{arch}.tar.xz'
with tarfile.open(path,'w:xz') as tar:
    info=tarfile.TarInfo('bundle/fixture'); info.size=1; tar.addfile(info,io.BytesIO(b'x'))
path.with_name(path.name+'.sha256').write_text(hashlib.sha256(path.read_bytes()).hexdigest()+'  '+path.name+'\\n')
''')
        tools = self.root / "tools"
        tools.mkdir()
        cargo = tools / "cargo"
        cargo.write_text('#!/bin/sh\nprintf \'%s\\n\' \'{"packages":[{"name":"archivefs-cli","version":"0.9.0"}]}\'\n')
        cargo.chmod(0o755)
        self.commit(root)
        output = self.root / "comparison-output"
        result = run("bash", str(root / "scripts/compare-release-builds.sh"),
                     "--output-dir", str(output), env=dict(os.environ, PATH=f"{tools}:{os.environ['PATH']}"))
        self.assertEqual(result.returncode, 0, result.stdout)
        a, b = [json.loads((output / f"run{i}/roots.json").read_text()) for i in (1, 2)]
        for key in ("source", "target", "output"):
            self.assertNotEqual(a[key], b[key])
        self.assertNotEqual(a["source"], str(root))
        self.assertEqual(a["head"], b["head"])
        self.assertEqual((a["branch"], b["branch"]), ("", ""))
        self.assertFalse(Path(a["source"]).exists())  # disposable clone cleaned

    def test_real_rust_paths_differ_without_remapping_and_match_with_build_flags(self):
        tools = self.root / "capture-tools"
        tools.mkdir()
        cargo = tools / "cargo"
        cargo.write_text('#!/usr/bin/env python3\nimport os,json\nprint(json.dumps(os.environ["CARGO_ENCODED_RUSTFLAGS"].split("\\x1f")))\nraise SystemExit(42)\n')
        cargo.chmod(0o755)
        outputs = {False: [], True: []}
        for index in (1, 2):
            root = self.make_repo(f"source {index}")
            target = self.root / f"target {index}"
            target.mkdir()
            generated = target / "generated.rs"
            generated.write_text('fn main() { println!("{}", file!()); }\n')
            source = root / "main.rs"
            source.write_text(f'include!({json.dumps(str(generated))});\n')
            self.commit(root)
            env = dict(os.environ, PATH=f"{tools}:{os.environ['PATH']}",
                       CARGO_ENCODED_RUSTFLAGS="-C\x1fopt-level=1")
            if index == 2:
                env.pop("CARGO_ENCODED_RUSTFLAGS")
                env["RUSTFLAGS"] = "-C opt-level=1"
            result = run("bash", str(root / "scripts/build-release.sh"), "--output-dir", str(self.root / f"out{index}"),
                         "--target-dir", str(target), env=env)
            self.assertEqual(result.returncode, 42, result.stdout)
            flags = json.loads(result.stdout)
            self.assertEqual(flags[:2], ["-C", "opt-level=1"])
            for remapped in (False, True):
                binary = self.root / f"binary-{index}-{remapped}"
                result = run("rustc", "--crate-name", "fixture", str(source), "-o", str(binary),
                             *(flags if remapped else []))
                self.assertEqual(result.returncode, 0, result.stdout)
                outputs[remapped].append(binary.read_bytes())
                if remapped:
                    self.assertNotIn(str(root).encode(), binary.read_bytes())
                    self.assertNotIn(str(target).encode(), binary.read_bytes())
        self.assertNotEqual(*outputs[False])
        self.assertEqual(*outputs[True])


if __name__ == "__main__":
    unittest.main()
