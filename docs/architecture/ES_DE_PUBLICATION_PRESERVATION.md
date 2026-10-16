# ES-DE publication preservation: core repair

Original core baseline: a1b5e2a972489346243ced99441ed6aa19e6460b (sections up to
"Validation isolation" describe that core-only candidate). The 2026-10-16 sections
at the end describe its port onto origin/main 6fd4d70b, which also changes two
small parts of the classic Playing Library GUI. Coordinated GUI/RetroDECK
link-plus-gamelist orchestration remains unported.

## Confirmed root causes

The prior apply path wrote the preview's replacement without checking its original
bytes or expected absence. Rollback restored the original snapshot unconditionally;
schema-1 restart recovery had no published after-image against which to check the
current file. Recovery-record existence was not exclusive publication ownership.
String matching of closing tags and encoded path fragments also accepted malformed
documents and confused comments with game entries. These are reachable core paths
used by production Playing Library and RetroDECK controllers.

## Core contract

The existing planner, apply, rollback and recovery entry points are retained.
Plans now capture shared hashed object identities plus device/inode bindings for
existing directory ancestors. Stale before-images, replaced files, unavailable
reviewed ancestors and symlinked ancestors refuse. Unknown XML is preserved as
bytes; a bounded structural walk identifies the root insertion span and decoded
game/path values. DTDs, malformed structure, invalid names/characters and unsupported
XML encodings refuse. Relative paths are not guessed into absolute identities.

Apply/recovery/rollback use a persistent sibling `.es-de-publish.lock` inode and
nonblocking advisory flock. Never delete that inode to clear a supposedly stale
lock: the kernel releases the lock when the holder exits. This is Unix support;
other hosts refuse rather than using weaker namespace/exclusion evidence.

The shared atomic text writer remains the mutation primitive. Fresh preconditions
are repeated after writing recovery evidence and before replacement. Reads are
bounded, no-follow at the leaf, and checked using the existing descriptor-bound
hashed object identity. A write/finalization conflict keeps recovery evidence.

Rollback requires exact after-image bytes, or returns AlreadyRestored without
writing when the before-image is already present. Detailed outcome helpers expose
this distinction; the original Result<()> wrappers remain compatible. A different
current image refuses, including deletion of a newly generated file that was
subsequently edited.

Schema 2 recovery records include before/after images, namespace evidence and an
integrity digest. The digest detects accidental modification; it is not a keyed
signature and does not authenticate a malicious local writer able to recompute it.
Schema 1 records have insufficient after-state evidence and remain review-required.
They are not silently upgraded, removed or used to restore over current files.
Successful publication still removes its recovery record; this is not a durable
combined link/metadata Undo receipt.

## Concurrency limitations

Advisory exclusion covers cooperating EmuWiz processes using the same destination
and unchanged lock inode. ES-DE/editors need not cooperate. Comparison and the
shared path-based atomic rename are separate operations: a non-cooperating writer
or ancestor swap after the final check can still race. Inverse remove/replace has
the same limitation. Atomic rename prevents partial file visibility, not lost
updates. A post-write conflict cannot recover unseen external bytes safely, so it
never triggers unconditional restoration. Frontend quiescence/cooperation or a
stronger reviewed replacement protocol remains necessary before claiming full
cross-application exclusion. Inverse operations bind namespace and exact content;
they cannot attribute an external replacement having identical after-image bytes
to its writer. A durable successful after-object receipt belongs to the combined
workflow follow-up. Directory syncing is best-effort on unsupported
filesystems; fault-injection tests model process interruption, not every power-loss
or filesystem ordering behaviour.

## Subsequent GUI/RetroDECK integration

* Invalidate stale confirmations on a core conflict; require fresh review.
* Consume structured ApplyOutcome and RollbackOutcome states, including inner
  failures returned inside Ok. Never publish the complete plan after partial links.
* Retain an immutable receipt binding the applied link transaction and exact
  gamelist publication. Later preview state must not replace that receipt.
* Journal combined phase progress, including metadata-written/finalization-failed,
  metadata reversal and incomplete link rollback. Preserve evidence on conflicts.
* Display legacy recovery as review-required, not a promise of safe restoration.
  Replace synthetic schema-1 success expectations with valid schema-2 fixtures;
  add explicit GUI refusal tests for old records.
* Keep History checks advisory and repeat full owning-workflow verification.

## Existing GUI ownership reconciliation

The original worktrees were read only. Dirty state alone cannot prove that a
session is active or abandoned; no ownership release was established.

* emuwiz-gui-organisation-playing-library: committed HEAD ac37f78e is an ancestor
  of baseline. Actual controller, presentation helper and equivalent-duplicates
  contents match main despite historical dirty/untracked status. Playing Library
  test differences retain older RomM fixture expectations. Organisation/tests
  also differ from current main and must be preserved.
* emuwiz-org-integration: committed HEAD fdc8acbd is an ancestor of baseline.
  Controller/presentation/equivalent-duplicates match main. Tests contain older
  RomM fixture expectations and a debugging print. Organisation/tests differ.
* emuwiz-mame-family-performance: HEAD 30c7542e is not an ancestor of baseline.
  Controller/tests differ substantially, including destination-change state
  invalidation absent from baseline; newer presentation and duplicate modules
  are absent. Do not integrate its whole old tree over main.

Coordination scope includes crates/archivefs-gui/src/playing_library_page.rs,
playing_library_page/tests.rs, playing_library_page/presentation.rs and related
gui_v2/organisation.rs and gui_v2/tests.rs. No dirty core publication or operation
files were found in other registered worktrees when establishing the core lane.

## Validation isolation

All new regressions use TempDir fixtures. Cargo runs through scripts/cargo-iso
with a fresh subprocess HOME and XDG data/config/cache roots, explicit existing
Rust toolchain/cache locations and a target unique to this repair worktree.
No real frontend, ROM, database or mount is a test fixture. No cache is deleted.

The first five added preservation regressions failed against the baseline before
the repair. The final repaired core runs passed:

| Selected core suite | Passed | Failed |
| --- | ---: | ---: |
| launch::es_de_publish::tests | 50 | 0 |
| playing_library (includes RetroDECK projection and multi-file/1G1R checks) | 130 | 0 |
| dat::rename_apply (includes shared identity, History and interrupted rollback) | 138 | 0 |
| operation::tests | 28 | 0 |

These are selected-suite execution totals, with some overlapping operation tests.
New fixtures cover stale bytes/absence, same-byte inode replacement, changed
parents and symlinks, missing ancestors, simultaneous attempts at barriers,
malformed XML, semantic paths and byte-preserved unknown metadata, guarded inverse
operations, legacy/tampered receipts, fresh idempotent replanning, and injected
before-write/after-write/finalization interruptions. No unrestricted suite was
run against the user's real HOME.

The three selected existing RetroDECK GUI tests passed (normal apply/Undo,
publication refusal with link cleanup, and unverified-preview blocking). The
existing ES-DE GUI module produced 13 passes and one failure:
`confirmed_recovery_restores_the_exact_prior_content_and_touches_nothing_else`
at `crates/archivefs-gui/src/playing_library_page/tests.rs:1438`. Its schema-1
fixture expects unconditional restoration over different current bytes. The
new core refuses with "manual review required", leaving the gamelist and receipt
intact. This is an intentionally changed contract; the failing assertion is
reported, not weakened or hidden. Updating this owned GUI fixture, and adding
new guarded-recovery controller coverage, belongs to the subsequent coordinated
GUI slice. This candidate is core-review-ready, not combined merge-ready.

The final `scripts/cargo-iso --low-debug check --workspace --all-targets` passed
with the same isolated subprocess environment, compiling production GUI callers
and all test targets. Targeted Rust 2024 formatting checks, `git diff --check`
and the allowed-file scope guard passed. Existing unrelated core/GUI warnings
remain unchanged. No release build, merge or push was performed.

## 2026-10-16 completion on current main (owner decision: option 2)

Recovered: the reviewed core above (`c64efe63`, identical tree to `68d25bde`), cherry-picked onto
origin/main `6fd4d70b` without conflicts, plus the publication-lock fork/exec retry and its two tests from
`308b42a1`. Not recovered, by owner decision: the branch's DAT rename-journal rewrite (`journal/namespace.rs`
and the rename_apply changes), the coordinated ES-DE/RetroDECK link+metadata operation (`operation.rs`, which
depends on that rewrite) and its GUI wiring. `4bfcbc95` (following trusted symlinked ancestors) needs the
rewrite's `resolve_trusted_dir`, so symlinked ancestors of a gamelist still **refuse** (fail closed). No R2,
rename-journal or recovery-authority code is changed. The gamelist recovery record stays separate from rename
transactions; there is no atomic coordination between the playing-library links and the gamelist write.

New, with regressions that failed before the fix:

* Matching uses lexically normalised absolute paths (`//`, `.`), so an equivalent path is already present.
* Two elections for one destination add one entry.
* Superseded the same day by "Path semantics and per-entry matching" below (the first version refused the whole
  file for any same-name relative entry, which would have blocked normal ES-DE gamelists).
* `EsDeGamelistPublication::preserved_summary()` reports preserved game, folder and other root-level entries and
  bytes. Publication is insertion-only, so changed entries are always zero.

Limitations: Undo (`rollback_es_de_gamelist_publication`) needs the in-memory publication and the exact
after-image; after a successful apply the recovery record is removed, so there is no durable post-restart Undo
receipt or retained backup copy. Interrupted publications keep their schema-2 recovery record. Exclusion covers
cooperating EmuWiz processes only (see Concurrency limitations).

GUI (classic Playing Library page, two minimal changes): wording for `AmbiguousExistingEntry`; and a refused
recovery no longer hides its still-unresolved record (`confirm_esde_recovery` used to `take()` the path before
trying). The schema-1 "restores over different bytes" GUI test is replaced by the branch's reviewed
`legacy_recovery_is_refused_and_preserves_current_content_and_evidence`, which asserts the refusal, the untouched
current bytes and the retained record.

Validation (2026-10-16, scripts/cargo-iso): core `launch::es_de_publish`, `playing_library`, `operation::tests`,
`atomic_text` filters 265/265; GUI `playing_library_page` 68/68 after the fix (first run 67/1, the predicted
schema-1 test, then the hidden-record defect it exposed); `check --workspace --all-targets` passed.

## Path semantics and per-entry matching (2026-10-16, second pass)

ES-DE resolves a `<path>` beginning `./` against the **system's ROM directory**, which is the system's
`es_systems.xml` `<path>` with `%ROMPATH%` replaced by the `ROMDirectory` setting. It is not the gamelist's
directory (`LegacyGamelistFileLocation` only moves the XML). Evidence used, read-only: EmuWiz's discovery model
(`EsDeSystemFinding::rom_path_raw`, `%ROMPATH%/<system>` in bundled systems); this host's real ES-DE and RetroDECK
installs, whose `es_settings.xml` set `ROMDirectory` to `/mnt/usbdrive/games` and `/home/davedap/retrodeck/roms`; and
their gamelists, where all 2,328 sampled `<path>` values use `./` (a few nested), alongside `<folder>` entries.
No real gamelist was modified; no EmuWiz code was run against them.

ROM root (`matching::system_rom_root`): the discovery-resolved system path, or `%ROMPATH%` plus a single absolute
`ROMDirectory` read (bounded, no-follow leaf) from that profile's `es_settings.xml`. Unverified when ROMDirectory is
absent, empty (ES-DE's `~/ROMs` default), `~`-based, relative or set twice; when the system has conflicting
definitions or another variable; when the path contains `..`; or when the directory is missing. The root is
canonicalised.

Existing entries: `./x` joins the root (`..` refused) and must canonicalise inside it (an escaping symlink is
unresolved); absolute paths are used as written (lexically normalised); `~/`, bare relative and whitespace-padded
paths are unresolved (whether ES-DE trims is not established, so padding is neither matched nor ignored).

A new entry is **already present** only on an exact normalised path or a file identity (device + inode) shared with
the destination or its source ROM. A file name alone is never a match. A new entry is **refused, per entry**, when an
existing entry with the same file name is unresolved, points at a missing file (the game may have moved) or cannot
be checked, or when this game's own files cannot be checked against a present same-name file. Refused entries are
listed in the preview (`EsDeGamelistPublication::refused`, shown by the GUI with reasons) and nothing is written for
them; the rest of the publication proceeds. A whole-system refusal is not needed: the gamelist write is still one
guarded insertion, a skipped entry only means no new metadata, and the playing-library links are a separate step
either way. `apply` also refuses any `new_content` that is not `previous_content` with an insertion at the root's
closing span, so a hand-built publication cannot replace entries.

Remaining limitations: `~`-based ROM directories (including ES-DE's default) are unverified because the profile does
not record the home `~` means; such systems still publish, but same-name `./` entries refuse per entry. A
`DestinationEvidence` lock file and any directories created for a new gamelist remain after Undo. A stale-preview
conflict is shown with generic I/O wording. A legacy schema-1 record stays a manual-review dead end in the GUI.

Validation: core `launch::es_de_publish` + `atomic_text::tests::parent_boundary` 76/76 (all new matching regressions
written before the code; the original padded-path test passes unchanged).
