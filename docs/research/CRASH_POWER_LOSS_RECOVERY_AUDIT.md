# EmuWiz power-loss / crash-recovery audit

Date: 2026-09-20
Authoritative base: `41c9a645464a3fec9fa68cad7de78cfadd19cf12`
Worktree: `/home/davedap/emuwiz-crash-recovery-audit`
Branch: `research/crash-recovery-transactions`

This is an audit only. No GUI, production Rust, ROM, or media files were changed. No machine reboot or power-off test was attempted. The worktree started clean and the only committed change is this document.

## Executive result

EmuWiz has two materially different transaction families.

* `rename_apply`, Playing Library link creation, and exact-duplicate quarantine create a durable intent/checkpoint journal before each filesystem mutation. They reconcile the filesystem on restart, fail closed on ambiguity, and have explicit rollback/resume tests. They are **process-crash-safe at the operation/checkpoint level**, but not a single all-filesystem-object power-loss transaction: directory-entry durability is only best-effort and several objects can be created by one logical operation.
* The shared mod installer used by archive/local packages and the PCSX2, PPSSPP, Cemu, and RPCS3 adapters performs each file replacement atomically and verifies it, but writes its durable journal only after all destination mutations finish. A kill or power loss during apply can therefore leave destination files, backup files, and temporary files with no durable operation record. Rollback has the same end-of-rollback marker problem. These flows are **not restart-discoverable when interrupted** and require manual review.

The important distinction is: `fsync` plus `rename` makes one replacement atomic and substantially more durable; it does not make a multi-file workflow, its backup, its receipt, and its history one durable transaction.

No small isolated fix was made. The shared-mod transaction gap requires a durable intent/checkpoint design, not a safe one-line correction.

## Starting state and existing work

The actual main SHA was `41c9a645464a3fec9fa68cad7de78cfadd19cf12`. Main reported `## main...origin/main [ahead 293]` and had four pre-existing untracked research documents: `DAT_ACQUISITION_NOINTRO_TOSEC.md`, `DAT_BACKLOG_AUDIT_CURRENT_MAIN.md`, `DAT_COMPLETION_AUDIT_CURRENT_MAIN.md`, and `INTEGRATION_DEBT_CURRENT_MAIN.md`. None were touched.

Existing relevant work included rename journals, recovery/reconciliation and rollback tests; shared transaction and rollback code; Playing Library transaction tests; exact-duplicate quarantine tests; mod transaction tests; database recovery diagnostics; and a synthetic transaction probe. The nearest research document, `MAME_COMPLETENESS_ENGINE_RECOVERY_AUDIT.md`, is not equivalent: it concerns MAME completeness-engine recovery, not filesystem power-loss durability. No complete equivalent audit or fault-injection report was found in reachable project sources.

## Evidence and method

Read-only source inspection covered `dat/rename_apply`, `rom_organisation/transaction.rs`, `repair/quarantine.rs`, `platform_evidence_fusion/plan_transaction.rs`, `patch_manager/shared_transaction.rs`, `standalone_patch.rs`, `mod_history`, and `database.rs`. Existing deterministic tests were run; no user media was hashed. Development-state inspection was limited to existing journals, backups, temporary artifacts, and the SQLite catalogue.

The existing shared-transaction fault injector injects explicit errors at backup write, temporary write, flush, rename, verification, journal write, parent creation, source/destination mutation, and rollback points. It does not kill a subprocess between a real mutation and the final journal write, and it cannot prove actual hardware power-loss ordering.

## Transaction matrix

| workflow | journaled? | backup? | atomic? | durable? | interrupt detection | rollback | resume | risk |
|---|---|---|---|---|---|---|---|---|
| `rename_apply` | Yes, before intent and before each syscall/checkpoint | No copy; source/destination identity is the recovery evidence | Rename/link operation is no-clobber and verified | Journal payload syncs; parent-directory sync is best-effort; filesystem mutation is not part of one durable multi-object commit | Yes: Applying plus source/destination reconciliation | Explicit reverse transaction; ambiguous states fail closed | Explicit exact-envelope resume only; normal recovery never auto-resumes | Medium: directory power-loss window and multi-object ambiguity |
| Playing Library link creation | Yes, shared rename journal; created directories are recorded | No content backup for links | Symlink/hardlink creation is checked; cross-filesystem moves are refused | Same journal/directory durability limits | Yes when journal survives | Explicit rollback removes only proven links/directories | No automatic resume | Medium |
| Exact-duplicate quarantine | Yes, journal before quarantine moves and per-entry checkpoints | Quarantine is the destination, not a byte backup; companions/hardlinks are fail-closed | No-clobber move with identity re-proof | Journal sync is strong but parent sync best-effort | Yes | Explicit shared rollback | No automatic resume | Medium/high for power-loss ambiguity involving directory entries |
| Archive-package/local mod install | Only final shared journal | Replacement files are copied to a managed backup root | Temp-in-destination-directory, flush/sync, digest verification, atomic rename | Individual file replacement is substantially durable; intent and final receipt are not durable during apply | No if killed before final journal | Available from a completed journal; not safely discoverable after an unjournaled kill | No | High |
| PCSX2 mod transaction | Same shared installer | Yes for replaced target; new targets are removed on rollback | Atomic target replacement | Same gap | No during in-flight apply | Safe only when the completed journal exists and target identity still matches | No | High |
| PPSSPP mod transaction | Same shared installer | Yes for replaced target | Atomic target replacement | Same gap | No during in-flight apply | Completed-journal rollback only | No | High |
| Cemu graphic-pack transaction | Same shared installer | Yes for replacements | Atomic per file, package paths validated | Same gap | No during in-flight apply | Completed-journal rollback only | No | High |
| RPCS3 ordinary-mod transaction | Same shared installer | Yes for replacements | Atomic per file, nested layout preserved | Same gap | No during in-flight apply | Completed-journal rollback only | No | High |
| Mod rollback | Final rollback marker only, after all reverse mutations | Uses original managed backup | Each restore/remove is atomic and verified | Reverse mutations can be durable before rollback marker | No stepwise durable rollback discovery | In-process errors are truthful; kill can leave partial rollback | No | High |
| Patch-package derived output | Shared package installs use the shared gap; standalone patch produces a derived file | Shared package replacement backup; standalone output has no rollback claim | Standalone output is staged and atomically installed; source remains unchanged | Output replacement is strong per file, but no general operation journal | Shared package: no; standalone: process errors clean staging but no crash receipt | Standalone history explicitly makes no rollback claim | No | Medium/high |
| Standalone patch output | No filesystem transaction journal for the derived output | None | IPS/BPS/UPS/PPF are built in memory; xdelta uses a same-directory staging file and cleanup | Staging/output replacement is not a power-loss transaction | No startup interrupted-output projection | No automatic rollback | No | Medium |
| Transaction journals | Yes for rename family; final-only for shared family | None | `atomic_write_text` uses same-directory temp and rename | Temp is flushed/synced; parent sync errors are intentionally best-effort | Rename family can discover; shared final-only cannot | N/A | N/A | A durable journal can still describe an external filesystem state that was not durably committed |
| History/receipts | Rename journal is the operation record; shared history is final receipt | N/A | Atomic file write | Atomic/synced file payload, but not coupled to destination mutation | Only where an earlier intent exists | Receipt does not itself undo files | No | Receipt may be absent after successful mutation or present after a later external durability failure |
| Backup creation | Shared backup is recorded only in in-memory results until final journal | Yes, managed backup files | Backup write is temp/sync/rename and digest checked | Individual backup is strong; backup set has no durable pre-intent | No if kill precedes final journal | Restore from completed journal only | No | Backup without a discoverable transaction can be orphaned or mistaken for authoritative |
| Atomic replacement helpers | Same-directory temp, flush/sync, verify, rename, parent sync | Helper does not create semantic backup | Yes per destination path | Good per file, parent sync best-effort | No generic operation discovery | Caller-owned | Caller-owned | Atomicity is not durability of the surrounding workflow |

## Detailed filesystem ordering

### Rename, links, organization, and quarantine

The rename family writes a JSON journal before the first mutation, sets the transaction to `Applying`, then writes an entry checkpoint immediately before each syscall. It performs a no-clobber rename or link creation, re-proves source/destination identity, marks the entry `Applied`, and rewrites the journal. Directory creation is also recorded. A normal restart sees `Planned`, `Applying`, `ApplyFailed`, `RollingBack`, or `RollbackFailed` as needing recovery; it reconciles the actual source and destination rather than assuming the last checkpoint is true.

If the syscall never happened, source-only is classified as not applied. If the syscall happened and destination identity is correct, destination-only is classified as applied. Both-present, both-absent, or wrong-identity states fail closed. This is why a process kill after the rename syscall but before the journal rewrite is recoverable at the logical level. It is not proof that the directory entry survived a sudden loss of power: the journal and directory entry are separate durable objects.

Rollback writes `RollingBack` before each reverse syscall, performs no-clobber reverse work, verifies it, and writes the result. A kill leaves an unresolved state; it never claims a complete rollback. Exact resume is opt-in and requires the exact operation envelope, source/destination identity, and other preconditions. Recovery never silently resumes.

### Shared mod apply

`execute_shared_apply` builds a journal in memory, validates the plan, creates roots and locks, then applies every file. Per entry it may create parent directories, write a replacement backup under the managed backup root, write a same-directory destination temp, flush/sync it, verify its digest, rename it into place, sync the destination parent, and verify the final destination. Only after the complete loop does it release the lock, populate created-root information, and write the durable shared journal.

Therefore a kill or power loss after any destination or backup mutation and before the final journal write can leave real state with no durable receipt. If the final journal write fails after successful writes, the code reports `JournalFailedAfterSuccessfulWrite` in memory, but that warning is itself not available after a killed process unless the journal write succeeded. This is the clearest case where filesystem mutation can succeed while journal/history is not durable.

Rollback reads the completed journal, restores/removes entries with atomic writes and verification, cleans created directories, and writes a rollback marker at the end. A kill during rollback can leave a partially restored tree while the original success journal remains and no rollback marker exists. Automatic retry or rollback would need per-entry durable reverse checkpoints; current code does not have them.

### Patch-derived outputs

The standalone patch path reads and validates the base and patch, computes a derived output, and leaves the source untouched. IPS, BPS, UPS, and PPF are interpreted in memory. xdelta/VCDIFF is supervised with bounded resources and writes to a same-directory staging path before verification and replacement. Error cleanup is tested, but there is no generic durable output journal or startup discovery for a power-loss-created staging file. Standalone history intentionally does not claim rollback. Archive/local patch packages enter the shared transaction path and inherit its final-only journal gap.

## Atomicity versus durability

The core `atomic_write_text` helper creates a uniquely named temp file in the target directory, writes and flushes it, calls `sync_all` on the temp file, checks that the target is not a symlink/non-file, renames the temp over the target, and attempts to sync the parent directory. The patch-manager managed-write helper follows the same pattern. This protects readers from torn file contents and gives a strong per-file ordering point.

The parent-directory sync is best-effort and its error is discarded. Rename/link helpers also do not turn all directory-entry changes in a multi-object operation into one durable commit. Same-filesystem assumptions are explicit for rename-style moves; cross-filesystem organization moves are refused rather than copied silently. Hardlink/symlink creation is not a content fsync operation. Consequently:

* mutation can have succeeded while the following journal checkpoint is absent or not durable;
* a journal can be durable while the subsequent filesystem mutation is absent after power loss;
* a completed shared journal can be durable while a later directory-entry or backup durability failure leaves the physical tree different;
* receipts/history are not a commit protocol with external filesystem objects.

No blanket `fsync` change is recommended. The required architectural fix is a durable intent journal before the first shared-mod mutation, durable per-entry apply/rollback checkpoints, and startup reconciliation that proves identities before offering recovery.

## SQLite audit

Writable `open_connection` enables foreign keys and a five-second busy timeout. Catalogue refreshes use `BEGIN IMMEDIATE`, normal `transaction()` boundaries, and explicit commit/rollback. Migrations apply SQL, migration metadata, and `user_version` in a transaction. No application code sets `journal_mode` or `synchronous`; the live database was inspected read-only and reported:

```text
journal_mode = delete
synchronous  = 2 (FULL)
foreign_keys = 0 on the read-only diagnostic connection
user_version = 19 in the inspected live file
quick_check  = ok
```

The writer enables foreign keys per connection; the diagnostic read-only connection does not, which does not alter the database. No live `-journal`, `-wal`, or `-shm` sidecar existed at inspection time. SQLite transaction recovery is therefore handled by SQLite’s default DELETE rollback journal and FULL synchronous setting, but SQLite does not cover external filesystem mutations. A committed catalogue row cannot be treated as proof that a linked file, backup, mod destination, or receipt was durably committed.

The full database-filter test run also exposed three pre-existing expectation failures in the current main source: expected schema version 19 versus actual registered migration/schema version 20, and an expected table list missing `media_topology_evidence`. This audit made no migration or schema change; the failures are recorded, not fixed.

## Current recovery state and real outage evidence

Read-only inspection of the existing development state found:

* `rename-transactions`: 135 JSON journals; 44 top-level `applied` and 91 `rolled_back`; no current top-level `Planned`, `Applying`, `ApplyFailed`, `RollingBack`, or `RollbackFailed` journals.
* Some historical top-level `rolled_back` journals contain entry-level `rollback_failed` states (60 with one, 25 with two, and 3 with more). That is persisted evidence needing diagnostic/manual review, not proof that it came from the recent outage.
* `shared-cheat-history`: 19 success journals and 2 rollback markers. There is no incomplete shared apply marker.
* The shared backup area contains existing backup directories, including a zero-byte backup artifact. Its provenance cannot be tied to the outage from available metadata; it must not be auto-deleted.
* 20 `.archivefs-*`, `.tmp`, `.partial`, or `.staging` artifacts exist under the application data root, totalling 153,116,047 bytes. Most are named cache/download partials (including catalogue ZIP and metadata temporary names), not identifiable transaction temps. They were not deleted.
* The live SQLite file had no rollback/WAL/SHM sidecar and passed `quick_check`.

No incomplete journal, interrupted marker, or timestamped outage receipt was found that can be attributed to the recent machine power loss. The correct conclusion is “no direct outage evidence recoverable,” not “the outage left no state.”

## State model and restart discovery

The rename core can truthfully represent `Complete` (`Applied`), `RolledBack`, `Interrupted` (Applying/RollingBack or equivalent exact-resume interruption), `RecoveryAvailable`, `NeedsReview` (ambiguous identity/state), and `UnsafeToResume` (envelope or identity mismatch). The shared mod core can represent successful apply, rollback, and in-process partial failure, but cannot truthfully represent an in-flight interrupted apply after the process is gone because no intent journal exists yet. It also cannot distinguish a partially completed rollback from an unattempted rollback using a final marker alone.

There are APIs for rename recovery discovery and database interrupted-run projection. There is no single generic startup projection covering shared mod transactions and patch-derived outputs. The smallest future core addition is a durable operation registry with:

1. an intent record before the first external mutation;
2. per-entry mutation and reverse-mutation checkpoints;
3. source/destination/backup digests and filesystem identities;
4. a terminal `Complete`, `RolledBack`, `Interrupted`, `NeedsReview`, or `UnsafeToResume` record;
5. a read-only `discover_pending_operations()` API used by a future startup message.

Until that exists, a startup message can truthfully mention interrupted rename/database work, but not all possible shared-mod or standalone-patch work.

## Fault-injection results

Passed deterministic rename recovery fixtures covered crash after journal-before-first rename, crash after the first of N renames, before/after the syscall reconciliation, wrong destination identity, rollback interruption, and the “never auto-resume” policy. Shared-transaction tests passed injected apply failures, journal failures, atomic install, backup restore, rollback, and truthful in-process partial failure behavior. Adapter and patch tests passed for Cemu, PPSSPP, RPCS3, archive/local package, standalone patch, PCSX2 PNACH, and Xenia flows.

These are controlled error injections, not abrupt process termination. No test claims to reproduce a power cut. A future test-only subprocess harness should terminate at the exact stages “after destination mutation before final journal,” “after backup before destination,” “midway through N files,” “after journal completion,” and “during rollback,” then inspect only disposable temp trees and disposable SQLite files.

## SemaTor compatibility note (parallel research relevant to future adapters)

This audit also recorded the current SemaTor evidence needed for a future enhanced Atari ST launch adapter. The authoritative public project is [SirBaron/SemaTor](https://github.com/SirBaron/SemaTor), with public documentation at [sirbaron.github.io/SemaTor](https://sirbaron.github.io/SemaTor/). At the inspection date, the latest public preview was 1.1.230, released 2026-09-20.

The upstream README says SemaTor is a translation layer that runs 68000 game code and provides the hardware/OS interfaces needed by supported titles; it does not require Atari TOS. The source is private. The release is Linux x86-64 and Windows x64. Linux is the tested desktop target, distributed as a ZIP with `install.sh`, installed by default under `~/Games/SemaTor`, requiring SDL2 and Python 3 for installation. No AppImage, Flatpak, or source distribution was found. The public repository’s [license](https://github.com/SirBaron/SemaTor/blob/main/LICENSE) permits use of released binaries but prohibits redistribution of modified binaries and reverse engineering; it is not an open-source integration surface.

The public input formats are ST, IMG, MSA, full-sector DIM, STX, and supported disk images inside ZIP archives. The README describes companion-disk matching and edition-dependent compatibility. Original image bytes are not advertised as modified; guide text says a source disk remains unchanged for at least some save/options paths. Multi-disk handling is profile/game-specific. IPF was not listed in the current public README. The public guide says no TOS ROM or commercial disk images are bundled.

The public CLI surface is not documented. The documented Linux entry point is the installed desktop application; the README describes library UI controls (F11/F12, library settings, game selection, per-game options, controller mappings, and an update button), not a stable executable/profile/option argument contract. No supported headless profile-selection, fullscreen, controller, save-path, log-path, or exit-status CLI was found. Therefore an EmuWiz adapter cannot safely synthesize “executable + profile + enhancements” without a future upstream CLI/API contract or a documented launcher manifest. GUI automation is not acceptable.

The public catalogue currently lists 11 game guides across 14 edition profiles: Arkanoid II, Black Lamp, Mega lo Mania, Return to Genesis, SWIV, Time Bandit, Turrican II, Xenon, Xenon 2, Zak McKracken, and Zynaps. Enhancements are profile/game-specific and include widescreen/21:9 views, native campaign/content variants, independent music/effects/speech, timing/framerate options, control remaps, difficulty/cheat/practice settings, pointer/click fixes, extra HUD/panels, translation/audio variants, and optional frame generation. General CRT/picture controls and frame generation are user-controlled; many other items are profile-controlled and sometimes edition-controlled. Unsupported or untested editions are not equivalent to verified supported editions; the guide explicitly says filename alone does not verify an edition.

The public guide is static HTML, not a machine-readable profile manifest. It exposes no CRC, SHA, TOSEC/No-Intro identity, internal-title contract, or custom profile ID that EmuWiz can consume. It explicitly says a matching filename alone does not verify an edition. Consequently, the only safe future rule is: exact identity evidence supplied by SemaTor or an independently verified EmuWiz mapping is required; title/filename/fuzzy matching is `Needs Review` and must never offer enhanced launch automatically. A catalogue cache can safely cache versioned upstream metadata only after a machine-readable signed/hashed manifest exists; the current HTML should be treated as human documentation, not authorization data.

The public updater is an in-app signed Linux/Windows update flow, with GitHub downloads remaining available. The README says install 1.1.219 or newer once to enable future in-app installation. The release cadence is currently preview-driven and very rapid: public tags 1.1.217, 1.1.222, 1.1.225, 1.1.226, 1.1.227, and 1.1.230 were published across 2026-09-19/20. This is not a stable adapter ABI.

The safe EmuWiz design is therefore: keep normal Hatari/Steem launch always available; make SemaTor explicit; offer it only for an exact verified disk/profile identity; display the selected profile and enhancement switches before launch; fail closed on mismatch or unsupported edition; never modify source media; and label normal/emulated and enhanced sessions distinctly in history/activity. SemaTor should remain a specialist optional runner, never a replacement for Hatari/Steem.

## Local Atari ST opportunity

The existing SQLite catalogue was inspected read-only without hashing the library. It contains 1,388 current `AtariST` catalogue rows. A bounded name-only query found 24 candidate rows across eight of the eleven public SemaTor guide titles (Arkanoid, Return to Genesis, SWIV, Time Bandit, Turrican, Xenon, Xenon 2 overlap, and Zynaps). This is not a strong match: no SemaTor hash/profile identity is public and no image bytes were hashed. Strong matches: **0 measured**. Needs Review: **24 name-only candidates**, subject to de-duplication and exact edition verification once an upstream manifest exists. This estimate is catalogue evidence, not a claim that 24 distinct supported games are present.

## Validation record

With `CARGO_TARGET_DIR=/home/davedap/.cache/emuwiz-cargo-target` and `CARGO_INCREMENTAL=0`:

* rename_apply unit filter: 123 passed;
* shared_transaction unit filter: 29 passed;
* Playing Library filter: 127 passed;
* quarantine filter: 52 passed;
* exact_duplicate filter: 33 passed;
* PCSX2 local journey: 6 passed;
* PCSX2 end-to-end: 9 passed;
* Xenia local journey: 3 passed;
* Xenia end-to-end: 9 passed;
* Cemu graphic-pack: 7 passed;
* PPSSPP: 88 passed;
* RPCS3 ordinary mod: 7 passed;
* mod package: 21 passed;
* standalone patch: 21 passed;
* database filter: 299 passed, 3 pre-existing expectation failures described above.

`git diff --check` and `scripts/task-postcheck.sh` are required final checks for this research branch. No production-code or GUI validation is applicable because no production code was changed.

## Recommendations and blockers

1. Do not advertise shared-mod operations as power-loss recoverable until intent and per-entry checkpoint journals exist.
2. Add a single core startup discovery API only after all mutating families write the same durable operation envelope.
3. Keep automatic rollback and resume disabled for ambiguous states. Offer manual review with verified identities and explicit user confirmation.
4. Keep the source disk/media read-only by policy for patch/mod/enhanced-launch paths.
5. Request from SemaTor upstream a stable CLI, signed versioned profile manifest, exact media identity fields, documented save/config/log paths, and a machine-readable enhancement schema before implementing an adapter.
6. Do not route Atari ST titles to SemaTor based on filenames, titles, or the public HTML guide alone.

## Commit

The final local commit SHA is reported with delivery after the document-only commit. Nothing was pushed.
