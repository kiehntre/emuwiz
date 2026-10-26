#!/usr/bin/env python3
"""Policy diagnostics must not disclose unrecognised metadata. Synthetic fixtures only."""
import copy
import json
import os
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from test_emuwiz_cargo_maintain import MaintBase  # noqa: E402


class PolicyDiagnostics(MaintBase):
    def test_policy_show_redacts_extensions_without_changing_authority(self):
        self.approve(enable=False)
        with open(self.m.policy_path()) as handle:
            policy = json.load(handle)
        # Unknown keys and values can both contain sensitive material. Nested
        # extensions are accepted too, and are covered by the integrity digest.
        for section in (policy, policy["scope"], policy["limits"]):
            section["synthetic-private-key-name"] = {"token": "synthetic-private-value"}
        policy["digest"] = self.m.policy_digest(policy)
        self.m.write_policy(policy)
        self.m.record_transition("policy_approved", self.m.next_generation(), policy["digest"], "DISABLED")
        rc, _ = self.call("policy", "enable")
        self.assertEqual(rc, 0)
        before = self.m.load_policy()[1]
        self.assertTrue(before["enabled"])
        with open(self.m.policy_path(), "rb") as handle:
            authority_bytes = handle.read()

        rc, output = self.call("policy", "show")
        self.assertEqual(rc, 0)
        self.assertNotIn("synthetic-private", output)
        shown = json.loads(output)
        self.assertEqual(shown["state"], "ok")
        self.assertEqual(shown["path"], self.m.policy_path())
        expected = copy.deepcopy(before)
        for section in (expected, expected["scope"], expected["limits"]):
            del section["synthetic-private-key-name"]
            section["redacted_fields"] = "[REDACTED]"
        self.assertEqual(shown["policy"], expected)

        rc, output = self.call("status", "--json")
        self.assertEqual(rc, 0)
        self.assertNotIn("synthetic-private", output)
        status = json.loads(output)
        self.assertTrue(status["policy"]["enabled"])
        self.assertEqual(status["policy"]["digest"], before["digest"])
        self.assertEqual(status["budgets"]["reserve_free_bytes"], 0)
        self.assertEqual(self.m.load_policy()[1], before)
        with open(self.m.policy_path(), "rb") as handle:
            self.assertEqual(handle.read(), authority_bytes)

        # Display redaction must not turn a tampered policy into valid authority.
        policy["scope"]["owners"].append("unapproved-owner")
        self.m.write_policy(policy)
        self.assertEqual(self.m.load_policy()[0], "invalid")
        rc, output = self.call("policy", "show")
        self.assertEqual(rc, 1)
        self.assertIsNone(json.loads(output)["policy"])
        self.assertNotIn("synthetic-private", output)

    def test_standard_and_absent_policy_diagnostics_are_preserved(self):
        rc, output = self.call("policy", "show")
        self.assertEqual(rc, 0)
        self.assertIsNone(json.loads(output)["policy"])
        self.approve(enable=False)
        before = self.m.load_policy()[1]
        rc, output = self.call("policy", "show")
        self.assertEqual(rc, 0)
        self.assertEqual(json.loads(output)["policy"], before)


if __name__ == "__main__":
    unittest.main()
