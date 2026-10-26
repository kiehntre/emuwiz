# CodeQL security triage, 26 October 2026

Repository: `kiehntre/emuwiz`. Reviewed main commit
`a24c7e1fcfee031bcdea29799e44e7ea214eda02`, in one worktree on
`security/codeql-triage-20261026`. No existing worktree was edited.

## Outcome and priority

All 32 open alerts were retrieved with severity, rule, code location and SARIF
paths: four Critical and 28 High. One conditional disclosure was reproduced and
repaired (#29); 31 findings are demonstrable false positives on their reported
flows. No live credential or cryptographic secret was found in those flows.

The four Critical alerts were reviewed first. Their constants are persistent egui
widget identifiers. They are neither cryptographic salts nor encryption keys;
randomising them would disrupt GUI identity without improving security.

The confirmed defect is narrower than an unconditional credential leak. Normal
policy creation stores no credentials. However, policy validation accepts unknown
JSON fields, including nested scope/limits fields, and `policy show` previously
printed them verbatim. A private synthetic policy containing token metadata
reproduced the disclosure. An account able to place that metadata in an otherwise
valid policy could cause it to be included in captured terminal/CI diagnostics.
No secret-bearing real policy was read during testing.

## Source fix and preservation

`policy_for_display` constructs a separate allowlisted diagnostic view. It keeps
schema, approval metadata, verified enablement, public integrity digest, documented
scope and budget fields. Unknown keys and values are replaced by a generic
`redacted_fields: "[REDACTED]"` marker at each policy level. It does not expose
unknown key names or nested extension contents. Standard policies retain their
existing output. `status --json` already projects public fields and needs no fix.

The authority policy is never redacted in memory or on disk. Validation, digest
calculation, issuer/owner checks, mode/identity/symlink checks, transition locking,
code fingerprints and the deletion executor are unchanged. As with every change
to this maintenance script, an old code-bound enablement becomes disabled until
the owner deliberately enables the new code. That safeguard remains in force.
No CodeQL configuration, rule, test fixture, cryptographic algorithm, filesystem
operation or unrelated production code was changed. No alerts were dismissed.

## Per-alert evidence

Locations below refer to the reviewed main SHA, before the display helper shifted
Python line numbers. Each ID links to its GitHub security alert. Rule keys:

- C: `rust/hard-coded-cryptographic-value`, Critical.
- R: `rust/cleartext-logging`, High.
- P: `py/clear-text-logging-sensitive-data`, High.

The evidence archive `emuwiz-qa-artifacts/codeql-triage-20261026/` outside the
repository contains the original `alerts.json`, baseline Python/Rust SARIF,
and `alert-evidence.json`. The latter maps every ID to its severity, full rule,
exact location, affected source excerpt and all SARIF thread flows and related
locations (54 code-flow entries). Critical queries provide direct locations
rather than SARIF thread flows; their helper-to-widget call paths were inspected
in source. This table states the security meaning of those paths.

| Alert | Rule | Main location | Data flow and affected code | Assessment |
| --- | --- | --- | --- | --- |
| [#4](https://github.com/kiehntre/emuwiz/security/code-scanning/4) | R | `crates/archivefs-gui/src/romm_source.rs:429` | Synthetic catalogue test app -> RomM progress -> `notes.remove(0)` (vector eviction; no log). | False positive |
| [#5](https://github.com/kiehntre/emuwiz/security/code-scanning/5) | R | `crates/archivefs-core/src/patch_manager/cheat_sources.rs:3436` | Public Libretro catalogue definition -> test `trusted_retroarch_cheat_sources().remove(0)`; public URLs/config, vector removal, no log. | False positive |
| [#6](https://github.com/kiehntre/emuwiz/security/code-scanning/6) | R | `crates/archivefs-core/src/patch_manager/cheat_sources.rs:3463` | Public Libretro catalogue definition -> test `trusted_retroarch_cheat_sources().remove(0)`; public URLs/config, vector removal, no log. | False positive |
| [#7](https://github.com/kiehntre/emuwiz/security/code-scanning/7) | R | `crates/archivefs-core/src/patch_manager/cheat_sources.rs:3481` | Public Libretro catalogue definition -> test `trusted_retroarch_cheat_sources().remove(0)`; public URLs/config, vector removal, no log. | False positive |
| [#8](https://github.com/kiehntre/emuwiz/security/code-scanning/8) | R | `crates/archivefs-core/src/patch_manager/cheat_sources.rs:3540` | Public Libretro catalogue definition -> test `trusted_retroarch_cheat_sources().remove(0)`; public URLs/config, vector removal, no log. | False positive |
| [#10](https://github.com/kiehntre/emuwiz/security/code-scanning/10) | R | `crates/archivefs-core/src/patch_manager/cheat_sources.rs:3702` | Public Libretro catalogue definition -> test `trusted_retroarch_cheat_sources().remove(0)`; public URLs/config, vector removal, no log. | False positive |
| [#11](https://github.com/kiehntre/emuwiz/security/code-scanning/11) | R | `crates/archivefs-core/src/patch_manager/cheat_sources.rs:3880` | Public Libretro catalogue definition -> test `trusted_retroarch_cheat_sources().remove(0)`; public URLs/config, vector removal, no log. | False positive |
| [#12](https://github.com/kiehntre/emuwiz/security/code-scanning/12) | R | `crates/archivefs-core/src/repair/quarantine/tests.rs:250` | Synthetic `trusted_root` -> `quarantine_destination` path -> failing test assertion diagnostics. | False positive |
| [#15](https://github.com/kiehntre/emuwiz/security/code-scanning/15) | R | `crates/archivefs-core/src/patch_manager/gamehacking_browser_import/tests.rs:772` | Literal forbidden cookie/token keywords -> redaction regression assertion; no live secret. | False positive |
| [#16](https://github.com/kiehntre/emuwiz/security/code-scanning/16) | R | `crates/archivefs-gui/src/tests/emulator_profiles_and_setup.rs:1336` | Synthetic unfetched catalogue app -> fixed prerequisite error -> assertion diagnostics. | False positive |
| [#17](https://github.com/kiehntre/emuwiz/security/code-scanning/17) | R | `crates/archivefs-core/examples/real_canary.rs:76` | Synthetic canary file metadata -> numeric Unix UID -> fingerprint diagnostic. | False positive |
| [#18](https://github.com/kiehntre/emuwiz/security/code-scanning/18) | R | `crates/archivefs-gui/src/activity_history.rs:350` | Synthetic catalogue test app -> cheat controller -> `history.entries.push_front` (in-memory history). | False positive |
| [#19](https://github.com/kiehntre/emuwiz/security/code-scanning/19) | R | `crates/archivefs-gui/src/activity_history.rs:367` | Synthetic catalogue test app -> history UI -> `entries.remove` (vector eviction; no log). | False positive |
| [#20](https://github.com/kiehntre/emuwiz/security/code-scanning/20) | R | `crates/archivefs-gui/src/clipboard.rs:122` | Synthetic catalogue test app -> clipboard `init_error` -> `log_error_once` -> stderr; no clipboard content on the reported path. | False positive |
| [#22](https://github.com/kiehntre/emuwiz/security/code-scanning/22) | R | `crates/archivefs-core/src/patch_manager/cheat_sources.rs:3583` | Public Libretro catalogue definition -> test `trusted_retroarch_cheat_sources().remove(0)`; public URLs/config, vector removal, no log. | False positive |
| [#23](https://github.com/kiehntre/emuwiz/security/code-scanning/23) | R | `crates/archivefs-core/src/patch_manager/cheat_sources.rs:3913` | Public Libretro catalogue definition -> test `trusted_retroarch_cheat_sources().remove(0)`; public URLs/config, vector removal, no log. | False positive |
| [#24](https://github.com/kiehntre/emuwiz/security/code-scanning/24) | R | `crates/archivefs-core/src/patch_manager/cheat_sources.rs:3955` | Public Libretro catalogue definition -> test `trusted_retroarch_cheat_sources().remove(0)`; public URLs/config, vector removal, no log. | False positive |
| [#25](https://github.com/kiehntre/emuwiz/security/code-scanning/25) | R | `crates/archivefs-core/src/patch_manager/cheat_sources.rs:4011` | Public Libretro catalogue definition -> test `trusted_retroarch_cheat_sources().remove(0)`; public URLs/config, vector removal, no log. | False positive |
| [#26](https://github.com/kiehntre/emuwiz/security/code-scanning/26) | C | `crates/archivefs-gui/src/gui_v2/pages.rs:502` | Literal `v2_sidebar_scroll` -> `show_focus_reveal_scroll` -> `make_persistent_id` / `ScrollArea::id_salt`. | False positive |
| [#27](https://github.com/kiehntre/emuwiz/security/code-scanning/27) | C | `crates/archivefs-gui/src/gui_v2/simple_shell/focus_reveal_tests.rs:266` | Test literal `nested_shelf_regression` -> `show_focus_reveal_scroll` -> widget ID. | False positive |
| [#28](https://github.com/kiehntre/emuwiz/security/code-scanning/28) | R | `crates/archivefs-core/src/dat/rename_apply/fd_publication_probe_tests.rs:119` | `/proc/self/status` UID and capability mask -> unprivileged filesystem probe diagnostic. | False positive |
| [#29](https://github.com/kiehntre/emuwiz/security/code-scanning/29) | P | `scripts/emuwiz-cargo-maintain:583` | Trusted policy file -> JSON -> `load_policy` -> whole policy in `policy show` -> stdout. Unknown metadata survives validation. | Confirmed conditional disclosure; fixed |
| [#30](https://github.com/kiehntre/emuwiz/security/code-scanning/30) | P | `scripts/emuwiz-cargo-maintain:1010` | Trusted policy file -> `load_policy` -> `report`: public integrity digest and numeric reserve -> JSON stdout. | False positive |
| [#31](https://github.com/kiehntre/emuwiz/security/code-scanning/31) | C | `crates/archivefs-gui/src/gui_v2/pages.rs:1081` | Literal `v2_all_tools_scroll` -> `show_focus_reveal_scroll` -> widget ID. | False positive |
| [#32](https://github.com/kiehntre/emuwiz/security/code-scanning/32) | C | `crates/archivefs-gui/src/gui_v2/simple_shell.rs:1049` | Literal `v2_simple_destinations` -> `show_reveal_scroll` -> widget ID. | False positive |
| [#33](https://github.com/kiehntre/emuwiz/security/code-scanning/33) | R | `crates/archivefs-core/src/dat/rename_apply/compact_recovery/deployment_tests.rs:353` | Public verifier directory -> approval checks -> synthetic execution transaction -> failing assertion diagnostics; no signing key reaches this sink. | False positive |
| [#34](https://github.com/kiehntre/emuwiz/security/code-scanning/34) | R | `crates/archivefs-core/src/dat/rename_apply/compact_recovery/deployment_tests.rs:439` | Public verifier directory -> approval checks -> synthetic execution transaction -> failing assertion diagnostics; no signing key reaches this sink. | False positive |
| [#35](https://github.com/kiehntre/emuwiz/security/code-scanning/35) | R | `crates/archivefs-core/src/dat/rename_apply/compact_recovery/restart_status_tests.rs:153` | Public verifier directory -> grant inspection -> fixed verdict/reason -> synthetic failing assertion diagnostics; no signing key reaches this sink. | False positive |
| [#36](https://github.com/kiehntre/emuwiz/security/code-scanning/36) | R | `crates/archivefs-core/src/dat/rename_apply/compact_recovery/restart_status_tests.rs:190` | Public verifier directory -> grant inspection -> fixed verdict/reason -> synthetic failing assertion diagnostics; no signing key reaches this sink. | False positive |
| [#37](https://github.com/kiehntre/emuwiz/security/code-scanning/37) | R | `crates/archivefs-core/src/dat/rename_apply/compact_recovery/restart_status_tests.rs:472` | Public verifier directory -> grant inspection -> fixed verdict/reason -> synthetic failing assertion diagnostics; no signing key reaches this sink. | False positive |
| [#38](https://github.com/kiehntre/emuwiz/security/code-scanning/38) | R | `crates/archivefs-core/src/dat/rename_apply/compact_recovery/restart_status_tests.rs:727` | Public verifier directory -> grant inspection -> fixed verdict/reason -> synthetic failing assertion diagnostics; no signing key reaches this sink. | False positive |
| [#39](https://github.com/kiehntre/emuwiz/security/code-scanning/39) | R | `crates/archivefs-core/src/dat/rename_apply/compact_recovery/recovery_closure_tests.rs:314` | Public verifier directory -> approval checks -> synthetic execution transaction -> failing assertion diagnostics; no signing key reaches this sink. | False positive |

## Why the false-positive classifications are justified

The catalogue tests use fake transports and a public Libretro source definition,
not authentication tokens. `remove(0)` selects a catalogue source; it does not
write a log. GUI app constructors are synthetic fixtures; treating the entire
returned app as secret propagates to unrelated collection operations and OS
clipboard initialization errors. The reported clipboard path never reads or
prints clipboard text. The GameHacking assertion prints only literal synthetic
forbidden markers to diagnose a failed privacy test.

Quarantine/recovery assertions use private synthetic roots. Their `trusted`
values identify public verifier state or directories, not Ed25519 private keys.
The data paths were followed through the approval functions rather than assuming
all test code was safe. The canary/probe UID diagnostics document file ownership
and absence of privilege, and are not secrets. The real canary executable was
inspected, not executed against its hardcoded user filesystem root.

The Python #30 paths expose an unkeyed integrity hash and a validated byte count.
The digest is not a bearer token: approval relies on trusted ownership, file
identity/modes, structural state/history reconciliation and a code fingerprint.
The same-account trust limitation is already explicit in `verify_enablement`.
Removing the hash would discard useful diagnostics without fixing a secret leak.

## Validation and remaining limits

The two new regression tests pass. The disclosure regression was first run
against the unchanged production script and failed because the synthetic key and
value reached stdout. It now verifies top-level and nested redaction, unchanged
standard/absent output, preserved public status fields, enabled authority and
on-disk bytes, and rejection of a policy whose approved scope was tampered with.

All 97 tests from `python3 -I -B -m unittest discover -s scripts -p 'test_emuwiz_cargo_maintain*.py'` passed (322.111 seconds). These use private
HOME/registry/targets and fake Cargo. Existing fixture ResourceWarnings about
unclosed files remain; there were no test failures. The security-scan
classification tests, scope guard, GUI boundary guard and `git diff --check`
passed. No Rust runtime test was needed for the Python-only production change.

An independent read-only security reviewer separately reproduced #29, verified
the Critical false-positive paths, passed both new tests and additional private
fixtures (marker-name collisions, nested values, nonmutation), and found no
security regression. The exact final commit review is saved in the evidence
archive after the candidate is committed.

CodeQL CLI 2.27.3 was downloaded from GitHub's official release and checked against
its published SHA256. The complete Python code-scanning suite scanned all 51
Python files. It still reports #29's display sink and #30's public-field sink at
candidate lines 608 and 1035. The #29 behavior is fixed by runtime regression;
the query still marks the complete trusted-file result as secret, including
allowlisted public policy fields. No query suppression or naming workaround was
introduced to manufacture a clean scan.

The initial Rust scan completed with only the four widget-ID findings because
sandboxed extraction could not load either Cargo manifest and skipped macro
expansion. An offline retry confirmed the same limitation. Those missing 26
findings are a coverage failure, not resolved alerts. A further run with access to
the installed toolchain loads workspace semantics; its extraction diagnostics,
full-suite results and baseline comparison are retained in the evidence archive.
No Rust files have changed, so no Rust alert is claimed fixed by this candidate.
The extractor is explicitly assigned this worktree's isolated Cargo target.

There are **zero verified GitHub alert closures**. #29's conditional disclosure is
fixed by regression evidence, while its query finding persists. The other 31
alerts have individual false-positive evidence above; they remain undismissed.
GitHub-main verification requires an approved integration and a subsequent scan.
No merge or push is authorized by this local candidate.
