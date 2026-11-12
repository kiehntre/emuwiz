#!/usr/bin/env python3
"""Owner-signature gate for Cargo retirement (scripts/emuwiz-cargo-retire).

Test-only keys are generated in a private temporary directory per process; no real key
is read, created or installed.  The production allowed-signers file is never touched:
in-process tests point the executor's module constants at a test file.
"""
import hashlib
import json
import os
import secrets
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.dont_write_bytecode = True
if __name__ == "__main__":
    # one module instance, so the harness (which imports this module) and the tests here
    # share the same throwaway keys
    sys.modules.setdefault("test_emuwiz_cargo_signing", sys.modules["__main__"])

_KEYS = {}


def keys():
    """{'dir', 'owner', 'attacker', 'signers'}: two throwaway ed25519 keys; only 'owner'
    is listed in the allowed-signers file."""
    if not _KEYS:
        d = tempfile.mkdtemp(prefix="emuwiz-test-signing.")
        for who in ("owner", "attacker"):
            subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", f"test-{who}",
                            "-f", os.path.join(d, who)], check=True)
        anchor = os.path.join(d, "anchor")
        os.mkdir(anchor, 0o755)
        signers = os.path.join(anchor, "allowed_signers")
        with open(os.path.join(d, "owner.pub")) as handle:
            pub = handle.read().split()
        with open(signers, "w") as handle:
            handle.write(f'emuwiz-owner namespaces="emuwiz-cargo-retire@1,emuwiz-cargo-retire-selftest@1,emuwiz-hold-release@1" {pub[0]} {pub[1]}\n')
        os.chmod(signers, 0o644)
        _KEYS.update(dir=d, owner=os.path.join(d, "owner"), attacker=os.path.join(d, "attacker"),
                     signers=signers)
        import atexit
        atexit.register(shutil.rmtree, d, True)
    return _KEYS


def install_test_anchor(executor_module):
    """Point an in-process executor at the test allowed-signers file (never production)."""
    executor_module.OWNER_SIGNERS = keys()["signers"]
    executor_module.TRUST_ANCHOR_UID = os.geteuid()


def sign(path, key=None, namespace="emuwiz-cargo-retire@1"):
    sig = path + ".sig"
    if os.path.lexists(sig):
        os.unlink(sig)
    subprocess.run(["ssh-keygen", "-q", "-Y", "sign", "-f", key or keys()["owner"], "-n", namespace, path],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return sig


def write_record(X, manifest_path, digest, groups, now=None, hours=1.0, signer="owner", **override):
    """A scoped-run record for exactly `groups` (manifest group dicts), signed by `signer`
    ('owner', 'attacker' or None for unsigned).  Returns its path."""
    at = time.time() if now is None else now
    record = {"schema": X.SCOPED_SCHEMA, "kind": X.SCOPED_KIND, "operation": X.SCOPED_OPERATION,
              "host": socket.gethostname(), "target_root": os.path.realpath(X.F.target_root()),
              "state_dir": X.maintenance_dir(), "manifest_path": os.path.realpath(manifest_path),
              "manifest_sha256": digest,
              "targets": sorted(({"directory": g["directory"], "device": g["device"], "inode": g["inode"]}
                                 for g in groups), key=lambda t: t["directory"]),
              "created_at": at, "expires_at": at + hours * 3600, "nonce": secrets.token_hex(16),
              "recorded_by": "tester", "note": "test"}
    record.update(override)
    data = (json.dumps(record, indent=1, sort_keys=True) + "\n").encode()
    directory = X.scoped_runs_dir()
    os.makedirs(os.path.dirname(directory), mode=0o700, exist_ok=True)
    os.makedirs(directory, mode=0o700, exist_ok=True)
    path = os.path.join(directory, f"scoped-{hashlib.sha256(data).hexdigest()[:16]}.json")
    with open(path, "wb") as handle:
        handle.write(data)
    os.chmod(path, 0o600)
    if signer:
        sign(path, keys()[signer])
    return path


import test_emuwiz_cargo_retire as R  # noqa: E402


class SignedGate(R.Base):
    """Focused tests: what an agent running as this account can and cannot satisfy."""

    def fixture(self, name="wt-sign"):
        wt, target = self.managed_cache(name)
        g = self.group(wt, target)
        path, digest = self.manifest([g])
        return wt, target, g, path, digest

    def execute(self, path, digest, scoped):
        return self.run_retire(path, digest, execute=True, scoped=scoped)

    def assert_refused(self, result, code, target):
        rc, receipt = result
        self.assertNotEqual(rc, 0, receipt)
        self.assertEqual(receipt["refused"]["code"], code, receipt["refused"])
        self.assertTrue(os.path.isdir(os.path.join(target, "debug", "deps")), "nothing may be deleted")

    def test_owner_signed_record_retires_and_receipt_names_the_signer(self):
        _, target, g, path, digest = self.fixture()
        rc, receipt = self.execute(path, digest, write_record(self.mod, path, digest, [g]))
        self.assertEqual(rc, 0, receipt)
        self.assertEqual(receipt["scoped_run"]["owner_signature"], "VERIFIED")
        self.assertTrue(receipt["scoped_run"]["signer_fingerprint"].startswith("SHA256:"))
        self.assertIn("tamper-evident", receipt["scoped_run"]["limitation"])
        self.assertFalse(os.path.exists(os.path.join(target, "debug", "deps")))

    def test_same_user_self_authorisation_is_refused(self):
        """An agent with only this account's filesystem and command permissions can write a
        perfect record, but cannot produce the owner's signature: unsigned, signed with its
        own fresh key, or with a forged/copied .sig, nothing is deleted."""
        _, target, g, path, digest = self.fixture()
        self.assert_refused(self.execute(path, digest, write_record(self.mod, path, digest, [g], signer=None)),
                            "OWNER_SIGNATURE_REQUIRED", target)
        self.assert_refused(self.execute(path, digest, write_record(self.mod, path, digest, [g], signer="attacker")),
                            "OWNER_SIGNATURE_INVALID", target)
        rec = write_record(self.mod, path, digest, [g], signer=None)
        with open(rec + ".sig", "w") as handle:
            handle.write("-----BEGIN SSH SIGNATURE-----\nAAAA\n-----END SSH SIGNATURE-----\n")
        self.assert_refused(self.execute(path, digest, rec), "OWNER_SIGNATURE_INVALID", target)

    def test_agent_cannot_point_the_executor_at_its_own_signers_file(self):
        """The trust anchor has no CLI/env override for an unprivileged run, and a file the
        agent owns is not a trust anchor even when the module constant names it."""
        _, target, g, path, digest = self.fixture()
        rogue = os.path.join(self.tmp, "rogue_signers")
        with open(keys()["dir"] + "/attacker.pub") as handle:
            pub = handle.read().split()
        with open(rogue, "w") as handle:
            handle.write(f"emuwiz-owner {pub[0]} {pub[1]}\n")
        os.environ["EMUWIZ_OWNER_SIGNERS"] = rogue
        self.addCleanup(os.environ.pop, "EMUWIZ_OWNER_SIGNERS", None)
        self.assert_refused(self.execute(path, digest, write_record(self.mod, path, digest, [g], signer="attacker")),
                            "OWNER_SIGNATURE_INVALID", target)
        self.mod.TRUST_ANCHOR_UID = 0          # production rule: the anchor must be root-owned
        self.mod.OWNER_SIGNERS = rogue
        self.assert_refused(self.execute(path, digest, write_record(self.mod, path, digest, [g], signer="attacker")),
                            "OWNER_SIGNERS_UNAVAILABLE", target)

    def test_production_without_installed_signers_refuses_everything(self):
        _, target, g, path, digest = self.fixture()
        self.mod.TRUST_ANCHOR_UID = 0
        self.mod.OWNER_SIGNERS = "/nonexistent/emuwiz/cargo-retire-allowed-signers"
        self.assert_refused(self.execute(path, digest, write_record(self.mod, path, digest, [g])),
                            "OWNER_SIGNERS_UNAVAILABLE", target)

    def test_signature_over_other_bytes_or_namespace_is_refused(self):
        _, target, g, path, digest = self.fixture()
        rec = write_record(self.mod, path, digest, [g])
        other = write_record(self.mod, path, digest, [g])
        shutil.copy(other + ".sig", rec + ".sig")            # a real owner signature, for another record
        self.assert_refused(self.execute(path, digest, rec), "OWNER_SIGNATURE_INVALID", target)
        rec2 = write_record(self.mod, path, digest, [g], signer=None)
        sign(rec2, namespace="file")
        self.assert_refused(self.execute(path, digest, rec2), "OWNER_SIGNATURE_INVALID", target)

    def test_edited_record_breaks_name_hash_and_signature(self):
        _, target, g, path, digest = self.fixture()
        rec = write_record(self.mod, path, digest, [g])
        data = json.load(open(rec))
        data["expires_at"] += 86400 * 30
        with open(rec, "w") as handle:
            json.dump(data, handle, indent=1, sort_keys=True)
        self.assert_refused(self.execute(path, digest, rec), "SCOPED_RUN_INVALID", target)

    def test_wrong_root_and_wrong_digest_are_refused(self):
        _, target, g, path, digest = self.fixture()
        self.assert_refused(self.execute(path, digest, write_record(
            self.mod, path, digest, [g], target_root="/mnt/elsewhere")), "SCOPED_RUN_WRONG_ROOT", target)
        self.assert_refused(self.execute(path, digest, write_record(
            self.mod, path, "0" * 64, [g])), "SCOPED_RUN_MISMATCH", target)

    def test_altered_target_identity_is_refused(self):
        _, target, g, path, digest = self.fixture()
        forged = dict(g, inode=g["inode"] + 1)
        self.assert_refused(self.execute(path, digest, write_record(self.mod, path, digest, [forged])),
                            "SCOPED_RUN_MISMATCH", target)

    def test_extra_target_is_refused(self):
        wt2, t2 = self.managed_cache("wt-other")
        _, target, g, path, digest = self.fixture()
        g2 = self.group(wt2, t2)
        self.assert_refused(self.execute(path, digest, write_record(self.mod, path, digest, [g, g2])),
                            "SCOPED_RUN_MISMATCH", target)

    def test_expired_and_overlong_records_are_refused(self):
        _, target, g, path, digest = self.fixture()
        self.assert_refused(self.execute(path, digest, write_record(
            self.mod, path, digest, [g], now=time.time() - 7200)), "SCOPED_RUN_EXPIRED", target)
        self.assert_refused(self.execute(path, digest, write_record(
            self.mod, path, digest, [g], hours=48)), "SCOPED_RUN_EXPIRED", target)

    def test_replay_is_refused_even_after_the_consumed_marker_is_removed(self):
        wt, target, g, path, digest = self.fixture()
        rec = write_record(self.mod, path, digest, [g])
        rc, receipt = self.execute(path, digest, rec)
        self.assertEqual(rc, 0, receipt)
        rc, receipt = self.execute(path, digest, rec)
        self.assertEqual(receipt["refused"]["code"], "SCOPED_RUN_REPLAYED")
        os.unlink(self.mod.consumed_path(rec))
        rc, receipt = self.execute(path, digest, rec)
        self.assertEqual(receipt["refused"]["code"], "SCOPED_RUN_REPLAYED")
        os.unlink(os.path.join(self.mod.scoped_runs_dir(), "used-nonces.log"))   # and the nonce ledger
        rc, receipt = self.execute(path, digest, rec)
        self.assertEqual(receipt["refused"]["code"], "SCOPED_RUN_REPLAYED")       # journal still names the nonce

    def test_signed_execution_needs_the_canonical_receipts_directory(self):
        _, target, g, path, digest = self.fixture()
        rec = write_record(self.mod, path, digest, [g])
        rc, receipt = self.run_retire(path, digest, execute=True, scoped=rec,
                                      extra_args=["--receipt-dir", os.path.join(self.tmp, "elsewhere")])
        self.assertEqual(receipt["refused"]["code"], "SCOPED_RUN_INVALID")
        self.assertTrue(os.path.isdir(os.path.join(target, "debug", "deps")))

    def test_concurrent_executions_consume_once(self):
        _, target, g, path, digest = self.fixture()
        rec = write_record(self.mod, path, digest, [g])
        self.mod.check_scoped_run(rec, [{"group": g}], digest, path, time.time())
        results = []

        def consume(i):
            try:
                self.mod.consume_scoped_run(rec, f"exec-{i}")
                results.append("ok")
            except self.mod.Refusal as refusal:
                results.append(refusal.code)

        threads = [threading.Thread(target=consume, args=(i,)) for i in range(8)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        self.assertEqual(results.count("ok"), 1, results)
        self.assertEqual(set(results) - {"ok"}, {"SCOPED_RUN_REPLAYED"})

    def test_check_owner_signing_reports_a_usable_anchor_and_a_verified_selftest(self):
        ok, report = self.mod.owner_signing_status()
        self.assertTrue(ok, report)
        self.assertEqual(len(report["keys"]), 1)
        probe = os.path.join(self.tmp, "selftest.txt")
        with open(probe, "wb") as handle:
            handle.write(self.mod.SELFTEST_HEADER + b"harmless\n")
        sign(probe, namespace=self.mod.SELFTEST_NAMESPACE)
        ok, report = self.mod.owner_signing_status(probe)
        self.assertTrue(ok, report)
        self.assertTrue(report["selftest"]["verified"])
        sign(probe, keys()["attacker"], namespace=self.mod.SELFTEST_NAMESPACE)
        ok, report = self.mod.owner_signing_status(probe)
        self.assertFalse(ok)

    def test_check_owner_signing_refuses_missing_unrestricted_or_bad_anchor(self):
        self.mod.OWNER_SIGNERS = os.path.join(self.tmp, "missing")
        self.assertFalse(self.mod.owner_signing_status()[0])
        d = tempfile.mkdtemp(dir=self.tmp)
        os.chmod(d, 0o755)
        with open(keys()["owner"] + ".pub") as handle:
            pub = handle.read().split()
        good = os.path.join(d, "good")
        with open(good, "w") as handle:
            handle.write(f'# comment\nemuwiz-owner,other namespaces="emuwiz-cargo-retire@1",valid-after="20260101" '
                         f'{pub[0]} {pub[1]} owner key with spaces\n')
        os.chmod(good, 0o644)
        self.mod.OWNER_SIGNERS = good
        ok, report = self.mod.owner_signing_status()
        self.assertTrue(ok, report)
        for content in (f"emuwiz-owner {pub[0]} {pub[1]}\n",                       # no namespace restriction
                        f'someone-else namespaces="emuwiz-cargo-retire@1" {pub[0]} {pub[1]}\n',
                        'emuwiz-owner namespaces="emuwiz-cargo-retire@1" ssh-ed25519 AAAAnotakey\n', ""):
            path = os.path.join(d, "signers")
            with open(path, "w") as handle:
                handle.write(content)
            os.chmod(path, 0o644)
            self.mod.OWNER_SIGNERS = path
            ok, report = self.mod.owner_signing_status()
            self.assertFalse(ok, (content, report))
        os.chmod(path, 0o666)                                                     # writable by others
        self.assertFalse(self.mod.owner_signing_status()[0])

    # step 2 of the owner runbook (SIGNING-KEY-SETUP.md), verbatim except for the key path
    RUNBOOK_SIGNERS = ("printf 'emuwiz-owner namespaces=\"emuwiz-cargo-retire@1,emuwiz-cargo-retire-selftest@1,"
                       "emuwiz-hold-release@1\" %s\\n' \"$(cut -d' ' -f1,2 \"$1\")\"")

    def test_runbook_signers_line_verifies_a_hold_release(self):
        doc = os.path.expanduser("~/emuwiz-audit-ledger/2026-11-12-lifecycle-overhaul/SIGNING-KEY-SETUP.md")
        if os.path.exists(doc):
            with open(doc) as handle:
                self.assertIn("emuwiz-hold-release@1", handle.read())
        d = tempfile.mkdtemp(dir=self.tmp)
        os.chmod(d, 0o755)
        signers = os.path.join(d, "cargo-retire-allowed-signers")
        with open(signers, "w") as handle:
            subprocess.run(["bash", "-c", self.RUNBOOK_SIGNERS, "-", keys()["owner"] + ".pub"],
                           stdout=handle, check=True)
        os.chmod(signers, 0o644)
        self.mod.OWNER_SIGNERS = signers
        self.assertTrue(self.mod.owner_signing_status()[0])
        release = os.path.join(self.tmp, "hold-release-e1.json")
        with open(release, "wb") as handle:
            handle.write(b'{"kind": "hold-release", "entry": "e1"}\n')
        with open(release, "rb") as handle:
            raw = handle.read()
        sign(release, namespace=self.mod.F.HOLD_RELEASE_NAMESPACE)
        facts = self.mod.verify_hold_release(release, raw)
        self.assertTrue(facts and facts["signer_fingerprint"], facts)
        # the same key cannot release a HOLD by signing in another namespace, nor can another key
        sign(release, namespace=self.mod.SIGNATURE_NAMESPACE)
        self.assertIsNone(self.mod.verify_hold_release(release, raw))
        sign(release, keys()["attacker"], namespace=self.mod.F.HOLD_RELEASE_NAMESPACE)
        self.assertIsNone(self.mod.verify_hold_release(release, raw))
        # a signers line without emuwiz-hold-release@1 (the earlier runbook) does not verify it
        with open(signers, "w") as handle:
            subprocess.run(["bash", "-c", self.RUNBOOK_SIGNERS.replace(",emuwiz-hold-release@1", ""), "-",
                            keys()["owner"] + ".pub"], stdout=handle, check=True)
        sign(release, namespace=self.mod.F.HOLD_RELEASE_NAMESPACE)
        self.assertIsNone(self.mod.verify_hold_release(release, raw))

    def test_selftest_signature_never_authorises_retirement(self):
        _, target, g, path, digest = self.fixture()
        rec = write_record(self.mod, path, digest, [g], signer=None)
        sign(rec, namespace=self.mod.SELFTEST_NAMESPACE)
        self.assert_refused(self.execute(path, digest, rec), "OWNER_SIGNATURE_INVALID", target)

    def test_verify_scoped_run_is_read_only(self):
        import contextlib
        import io
        _, target, g, path, digest = self.fixture()
        rec = write_record(self.mod, path, digest, [g])
        for _ in range(2):                      # repeatable: nothing consumed
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                rc = self.mod.main(["--manifest", path, "--approved-sha256", digest, "--scoped-run", rec,
                                    "--receipt-dir", self.receipts, "--verify-scoped-run"])
            self.assertEqual(rc, 0, out.getvalue())
            self.assertTrue(json.loads(out.getvalue())["valid"])
        self.assertFalse(os.path.lexists(self.mod.consumed_path(rec)))
        self.assertTrue(os.path.isdir(os.path.join(target, "debug", "deps")))
        bad = write_record(self.mod, path, digest, [g], signer="attacker")
        with contextlib.redirect_stdout(io.StringIO()):
            rc = self.mod.main(["--manifest", path, "--approved-sha256", digest, "--scoped-run", bad,
                                "--verify-scoped-run"])
        self.assertEqual(rc, 4)

    @unittest.skipIf(os.path.exists("/etc/emuwiz/cargo-retire-allowed-signers"),
                     "a production signers file is installed; this test covers the not-installed state")
    def test_installer_refuses_without_trusted_keys_and_changes_nothing(self):
        install = os.path.join(self.tmp, "installed")
        os.makedirs(install)
        with open(os.path.join(install, "emuwiz-cargo-maintain"), "w") as handle:
            handle.write("old\n")
        probe = os.path.join(self.tmp, "selftest.txt")
        with open(probe, "wb") as handle:
            handle.write(self.mod.SELFTEST_HEADER)
        before = sorted(os.listdir(self.tmp))
        r = subprocess.run([os.path.join(HERE, "emuwiz-cargo-stage-install"), "--source", HERE, "--selftest", probe,
                            "--install-dir", install], capture_output=True, text=True)
        self.assertEqual(r.returncode, 3, r.stderr)
        self.assertIn("owner-signature verification is missing, invalid or unusable", r.stderr)
        self.assertEqual(open(os.path.join(install, "emuwiz-cargo-maintain")).read(), "old\n")
        self.assertEqual(sorted(os.listdir(self.tmp)), before)

    # ---- routine mode: no signature, only what every gate accepts
    def routine(self, path, digest, *extra, authority=lambda group: None):
        # emuwiz-cargo-maintain installs this in-process for its routine cycle; here a stub
        # stands in for its policy/source re-check (tested for real in test_emuwiz_cargo_evidence)
        self.mod.ROUTINE_AUTHORITY = authority
        try:
            return self.run_retire(path, digest, *extra, execute=True, scoped=None, extra_args=["--routine"])
        finally:
            self.mod.ROUTINE_AUTHORITY = None

    def test_routine_from_the_command_line_is_refused(self):
        _, target, g, path, digest = self.fixture()
        self.assert_refused(self.routine(path, digest, authority=None), "ROUTINE_REFUSED", target)
        p2, d2 = self.manifest([dict(g, source_head=None)], "nohead.json")
        self.assert_refused(self.routine(p2, d2), "ROUTINE_REFUSED", target)

    def test_routine_holds_when_the_final_recheck_fails(self):
        _, target, g, path, digest = self.fixture()
        rc, receipt = self.routine(path, digest, authority=lambda group: "the source HEAD changed since the plan")
        self.assertEqual(receipt["groups"][0]["outcome"], "HELD")
        self.assertEqual(receipt["groups"][0]["holds"][0]["code"], "ROUTINE_CONDITION_CHANGED")
        self.assertTrue(os.path.isdir(os.path.join(target, "debug", "deps")))

    def test_routine_retires_an_eligible_cache_without_a_signature(self):
        _, target, g, path, digest = self.fixture()
        rc, receipt = self.routine(path, digest)
        self.assertEqual(rc, 0, receipt)
        self.assertEqual(receipt["authorisation"], "ROUTINE_SAFETY_GATES")
        self.assertFalse(os.path.exists(os.path.join(target, "debug", "deps")))

    def test_routine_refuses_every_exceptional_option(self):
        _, target, g, path, digest = self.fixture()
        for extra in (["--maintenance-window-attested"],
                      ["--scoped-run", write_record(self.mod, path, digest, [g])]):
            self.assert_refused(self.routine(path, digest, *extra), "ROUTINE_REFUSED", target)
        orphan = dict(g, source_exists=False, associated_source_paths="[]")
        p2, d2 = self.manifest([orphan], "orphan.json")
        self.assert_refused(self.routine(p2, d2), "ROUTINE_REFUSED", target)
        many = [dict(g, directory=g["directory"] + f"-x{i}") for i in range(11)]
        p3, d3 = self.manifest(many, "many.json")
        self.assert_refused(self.routine(p3, d3), "ROUTINE_REFUSED", target)

    def test_routine_holds_protected_caches_and_stops_at_the_first_uncertain_target(self):
        wt1, t1 = self.managed_cache("wt-rt-pin")
        wt2, t2 = self.managed_cache("wt-rt-next")
        self.assertEqual(self.ret("pin", t1, "--reason", "running-gui").returncode, 0)
        path, digest = self.manifest([self.group(wt1, t1), self.group(wt2, t2)], "rt.json")
        rc, receipt = self.routine(path, digest)
        outcomes = [g["outcome"] for g in receipt["groups"]]
        self.assertEqual(outcomes, ["HELD", "NOT_STARTED_ROUTINE_STOPPED"], receipt["groups"])
        self.assertTrue(receipt.get("routine_stopped"))
        self.assertTrue(os.path.isdir(os.path.join(t1, "debug", "deps")))
        self.assertTrue(os.path.isdir(os.path.join(t2, "debug", "deps")))

    def test_routine_holds_a_changed_identity(self):
        wt, target, g, path, digest = self.fixture()
        p2, d2 = self.manifest([dict(g, inode=g["inode"] + 1)], "id.json")
        rc, receipt = self.routine(p2, d2)
        self.assertNotEqual([x["outcome"] for x in receipt["groups"]], ["RETIRED"])
        self.assertTrue(os.path.isdir(os.path.join(target, "debug", "deps")))

    def test_missing_record_is_refused(self):
        _, target, g, path, digest = self.fixture()
        self.assert_refused(self.execute(path, digest, None), "SCOPED_RUN_REQUIRED", target)


if __name__ == "__main__":
    unittest.main()
