# GameCube Action Replay production integration

Date: 2026-10-03. Starting authoritative `main` and `origin/main`:
`370f059f48a03469be4da2757d5028edc216e515`, with a clean tracked main tree.
Branch: `feature/old-backlog-gamecube-ar-integration`.
Worktree: `/home/davedap/emuwiz-old-backlog-gamecube-ar-integration`.

## Post-MAME selection and ownership

The first observation-only scan covered **441 worktrees**, 169 unique dirty
tracked paths, 23 staged paths and one unmerged path. Other writers were
checkpointing while the backlog was inspected. The final pre-edit selection
scan covered **442 worktrees**, 46 dirty worktrees, **143 unique dirty tracked
paths, 23 staged paths and one unmerged path**, with no inspection errors.
The snapshots count 24 nonblank index-status paths, including the `UU` path;
23 are ordinary staged paths. Staged and unmerged counts are subsets of the
dirty tracked path count; counts are unique repository-relative paths, not
summed owner occurrences.
No other worktree was modified, reset, cleaned, checkpointed or pruned.

The MAME target-replacement candidate has landed: starting main itself contains
the reviewed replacement/exact-original undo implementation. Its canonical
`dat/rename_apply` paths were clean. Clean historical candidates were not
treated as active ownership. The following review used current main, existing
research, and the dirty-file map rather than picking a feature from memory.

| Old item | Status at selection | Evidence / why free or blocked |
| --- | --- | --- |
| Save Vault generic multi-file restore | STILL BLOCKED | Existing single-file transactions and directory snapshots are useful foundations, but `save_snapshots.rs` has an active owner. See owner ledger below and `SAVE_RESTORE_GAP_AUDIT.md`. |
| Save migration and savestate compatibility/resume readiness | STILL BLOCKED | Existing readiness candidates need shared registration in dirty core `lib.rs`; generic restore also needs dirty `save_snapshots.rs`. `SAVE_MIGRATION_PLANNER.md` remains readiness-only. |
| Saturn >512 MiB streaming patching | STILL BLOCKED | Current component patching still has the 512 MiB bound; a correct implementation needs the shared, dirty/unmerged `standalone_patch.rs`, not a second patch engine. |
| Saturn SSP patches | NOT READY FOR SAFE IMPLEMENTATION | Existing SSP research does not establish a verified public container, base binding and deterministic application contract. No speculative decoder is justified. |
| GameCube Action Replay decoder production integration | UNBLOCKED NOW — SELECTED | Isolated verified decoder candidate `7a5295b29ddcaa456b155aa813e47f94578f1aff` explicitly deferred only the registry/bridge integration. Those exact paths are now clean. |
| CUE/CHD preservation-preview blocker wiring | STILL BLOCKED | The remaining work needs dirty `optical_preservation.rs`; related conversion code is also owned. Existing INDEX 00/PREGAP preservation is already implemented. |
| Large-DAT index memory reduction | UNBLOCKED NOW | `dat/index.rs`, `dat/set.rs` and `dat/sources/audit_run.rs` are now clean. The completed baseline identifies retained/cloned index records, not XML streaming, as the memory problem. This is a valid later implementation lane, not selected here. |
| RomM v2 native browser backend | SUPERSEDED | `ROMM_NATIVE_BROWSER_DECISION.md` establishes existing cache/linkage/view-model projections; the missing native route is GUI work, and another backend would duplicate them. |
| Fuse cheat runtime executor | STILL BLOCKED | Existing readiness/projection can be reused, but safe execution needs dirty launch registration and `launch/process_spawn.rs`. |
| VICE cheat runtime executor | NOT READY FOR SAFE IMPLEMENTATION | `VICE_C64_CHEAT_ADAPTER.md` proves monitor commands run before kernel reset and can be overwritten. The current preview does not prove a contained post-load runtime. |
| RetroArch cheat runtime executor | STILL BLOCKED | `launch/cheat_launch_plan.rs` deliberately remains a pure plan; its executor needs dirty `launch/process_spawn.rs`. Content/core overrides and state isolation must also be proved before execution. |
| ADF trainer runtime projection | NOT READY FOR SAFE IMPLEMENTATION | `AMIGA_ADF_TRAINER_FOUNDATION.md` has typed identity/provenance, but no proved emulator runtime contract. Scratch-copy launch cannot substitute for that proof. |
| MAME reviewed existing-target merged reconstruction | ALREADY DONE | Landed on starting main `370f059f`; do not repeat it. |
| MAME exploded wrong-ROM replacement | UNBLOCKED NOW | Existing repair validation can use the newly landed canonical replacement/undo primitive; required reconstruction/repair files and tests are clean. Lower priority than finishing the isolated AR decoder. |
| MAME existing packed-target rewrite | NOT READY FOR SAFE IMPLEMENTATION | Arbitrary-member/metadata preservation and replacement policy remain unproved; merged-family publication does not automatically establish them. |
| Emulator foundations waiting on registration | STILL BLOCKED | `launch/mod.rs` remains dirty in five worktrees. Foundations with unproved containment, including Electron, remain preview-only independently of registration. Already launchable adapters were not reimplemented. |
| RetroArch legacy cheat migration | ALREADY DONE | Main includes `920e30f8` and the reviewed shared apply/undo path described in `RETROARCH_CHEAT_AUTOLOAD_PATH.md`. |

### Exact blocking owners at selection

Paths below are relative to the repository. All worktree paths are absolute.
` M` means unstaged tracked modification; `UU` means an unmerged index/worktree
path. Each owner of a required blocking path is listed; old committed work
without dirt is not listed as a collision.

| Required file | Status | Worktree owner |
| --- | --- | --- |
| `crates/archivefs-core/src/save_snapshots.rs` | ` M` | `/home/davedap/emuwiz-old-backlog-save-directory-restore` |
| `crates/archivefs-core/src/standalone_patch.rs` | ` M` | `/home/davedap/emuwiz-custom-dat-lifecycle` |
| `crates/archivefs-core/src/standalone_patch.rs` | `UU` | `/home/davedap/emuwiz-xdelta-live-integration-2` |
| `crates/archivefs-core/src/optical_preservation.rs` | ` M` | `/home/davedap/emuwiz-cue-chd-current-main` |
| `crates/archivefs-core/src/repair/optical_conversion.rs` | ` M` | `/home/davedap/emuwiz-snes9x-stella-readiness` |
| `crates/archivefs-core/src/repair/optical_conversion.rs` | ` M` | `/home/davedap/emuwiz-vita3k-readiness` |
| `crates/archivefs-core/src/launch/mod.rs` | ` M` | `/home/davedap/emuwiz-082-batch3-launch` |
| `crates/archivefs-core/src/launch/mod.rs` | ` M` | `/home/davedap/emuwiz-082-batch3-launch-clean` |
| `crates/archivefs-core/src/launch/mod.rs` | ` M` | `/home/davedap/emuwiz-082-batch4-dat-dryrun` |
| `crates/archivefs-core/src/launch/mod.rs` | ` M` | `/home/davedap/emuwiz-snes9x-stella-readiness` |
| `crates/archivefs-core/src/launch/mod.rs` | ` M` | `/home/davedap/emuwiz-vita3k-readiness` |
| `crates/archivefs-core/src/launch/process_spawn.rs` | ` M` | `/home/davedap/emuwiz-082-batch3-launch` |
| `crates/archivefs-core/src/launch/process_spawn.rs` | ` M` | `/home/davedap/emuwiz-082-batch3-launch-clean` |
| `crates/archivefs-core/src/launch/process_spawn.rs` | ` M` | `/home/davedap/emuwiz-082-batch4-dat-dryrun` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-9f-tape-refactor` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-batch1-dryrun` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-batch1a-dryrun` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-batch1b-dryrun` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-batch3-launch` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-batch3-launch-clean` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-batch4-dat-dryrun` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-batch5-diagnostics-dryrun` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-batch5-v2` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-batch7-self-update-dryrun` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-batch7-v2` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-first-wave-reapply-check` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-082-integration` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-codex-mods` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-mount-root-reconciliation-final-20260902` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-snes9x-stella-readiness` |
| `crates/archivefs-core/src/lib.rs` | ` M` | `/home/davedap/emuwiz-vita3k-readiness` |

The one unmerged path is `standalone_patch.rs` in
`/home/davedap/emuwiz-xdelta-live-integration-2`. The other staged paths belong
to `/home/davedap/emuwiz-082-batch2b-media-gui`, not the selected lane.
The complete observation snapshots are retained under
`/tmp/emuwiz-post-mame-backlog-r7ctxhez/ownership*.json`.

A later validation scan covered 444 worktrees and found 146 unique dirty
tracked paths: 143 outside this candidate and the three edited existing files
inside it. There were still 23 ordinary staged paths and one unmerged path.
All seven selected paths remained free of other tracked or untracked owners.
During validation another lane advanced main and `origin/main` together to
`73970bfdd66c5f68f9ac4ff899abbd425b16ebcb` (GUI multi-disc completeness review).
The remote SHA was also verified. That delta changes none of this candidate's
paths or any core/CLI source. This branch retains the stated starting base;
this task did not modify main, promote or push.

## Exact pre-edit scope

These seven paths were predicted before creating the branch, then checked
against tracked/staged/unmerged owners and proposed untracked files in every
registered worktree. **All seven were free.** No GUI file is required.

- `crates/archivefs-core/src/patch_manager/mod.rs`
- `crates/archivefs-core/src/patch_manager/bsfree_gamecube.rs`
- `crates/archivefs-core/src/patch_manager/bsfree_gamecube/tests.rs`
- `crates/archivefs-core/src/patch_manager/gamecube_wii_ar_decrypt.rs`
- `crates/archivefs-core/src/patch_manager/gamecube_wii_ar_decrypt/tests.rs`
- `docs/research/GC_WII_AR_DASH_DECRYPTOR.md`
- `docs/research/GAMECUBE_AR_PRODUCTION_INTEGRATION.md`

The existing typed cheat record remains unchanged. A provenance field on that
record would require GUI test-constructor edits owned by other lanes; instead
decode evidence is carried by the existing backend search/confirmation outcome
and keyed by upstream record ID. No duplicate bridge, registry or transaction
subsystem is created to avoid an ownership boundary.

## Foundation to capability

The decoder and its 20 tests are taken from the existing isolated candidate
`7a5295b29ddcaa456b155aa813e47f94578f1aff`. The cipher algorithm is unchanged;
only module registration, integration documentation and serializable evidence
derives are added to it. `GC_WII_AR_DASH_DECRYPTOR.md` is retained verbatim as
the historical provenance/bounds/research report. Its statements about that
isolated branch and its standalone commands describe the original candidate;
this document describes the production integration and Cargo validation.

Previously, dash-format BSFree GameCube records could only be browsed. Now a
complete fixed-key set can be decoded, classified, explicitly selected,
staged as Dolphin GameSettings, previewed and applied through the existing
shared transaction engine, with its existing journal, backup and exact undo.
The existing CLI/GUI callers of the backend obtain this capability without
production UI edits or another installation path.

The gate order is:

1. Catalogue loaders require the existing explicit `GameCube` platform mapping.
   The public single-record classifier is an explicitly GameCube API; it does
   not infer a platform from a title, filename or encrypted verifier.
2. Encrypted records require the exact `Action Replay` device label and an
   untruncated `code` field. Raw classification retains its previous behavior.
3. The bounded decoder requires ASCII, exact dash syntax, canonical alphabet,
   every line's parity, the complete set checksum, and supported verifier bits.
   Mixed input, expanded seeds, reserved flags/regions, malformed or excessive
   input fail closed, without partially decoded output or fallback recovery.
4. A master verifier forces `Unsupported`, even if its decoded body would
   otherwise be installable. The existing master/zero/self-modifying opcode
   refusals and original Action Replay/Gecko routing still decide the result.
5. Selection starts empty. Existing title/platform review and selected-disc
   identity remain authoritative. The internal AR game number never becomes
   a Dolphin Game ID or automatic applicability claim.
6. The existing staging, stale-plan/context checks, approved replacement,
   publication, journals and guarded undo are reused without modification.

`BsFreeGameCubeSearchOutcome.ar_decryption` holds exact original encrypted
text, decoder version, checked verifier/CRC evidence and decoded body, or a
precise refusal. Provider name, author, section and notes remain on the existing
classified record. For oversized failures only the byte count and SHA-256
are retained; the diagnostic does not clone the oversized input. Empty evidence
maps are omitted from serialization, preserving ordinary raw result shapes.
The explicit `classify_bsfree_gamecube_cheat_with_provenance` API exposes the
same evidence for non-catalogue callers. The legacy Vec-only loader retains
its signature; callers needing provenance use the search/confirmation APIs.

Decoded records use the same canonical output digest as equivalent raw records.
The existing analyzer therefore detects raw/encrypted duplicates and cross-
provider collisions at the actual output level; a verifier is never emitted
as an executable line. No source archive, provider catalogue or emulator media
is edited by decoding or staging.

## Limits and remaining work

This is GameCube-only fixed-key interoperability. Native Wii dash encryption,
alternate seeds/expansions and ambiguous alphabet aliases remain unsupported.
The inherited limits remain 16,384 input bytes and 256 nonempty encrypted
lines, including the verifier; at most 255 executable pairs are emitted.
There are no new dependencies, migrations, unsafe code, process launches or
network calls. No emulator installation is needed for tests.

Parity and the four-bit folded checksum detect some input errors; they do not
authenticate a provider or prove code applicability. The original report
documents checksum collisions, and its collision test is retained. Successful
decoding never bypasses identity, review, opcode, selection or transaction
gates. A future UI may show the backend's decode evidence; this task changes
no GUI production or test file and makes no game-runtime efficacy claim.

## Validation

Validation uses synthetic catalogues, staged INIs and source media in
`tempfile::TempDir`. It uses the isolated target directory
`/tmp/emuwiz-post-mame-backlog-r7ctxhez/target`, offline/locked Cargo resolution,
two test threads and no installed emulator. Test debug symbols for the large
core test binary are disabled; debug assertions remain enabled.

The focused tests cover published/native/Gecko
vectors, exact provenance, platform/device/code-truncation refusals, master
verifiers, unsupported opcode families, corruption/mixed/Unicode/oversized
input, raw-output equivalence, duplicate staging, read-only catalogue queries,
explicit apply, stale staging/wrong identity refusal, source immutability,
exact-original undo and changed-output undo refusal.

| Check | Result |
| --- | --- |
| `archivefs-core --lib patch_manager::bsfree_` | 66 passed, including 16 new integrated tests and existing Wii refusals |
| `archivefs-core --lib patch_manager::gamecube_wii_ar_decrypt::` | 20 passed; original isolated decoder tests retained unchanged |
| `archivefs-core --lib patch_manager::` | 2,019 passed, including provider, Dolphin dedup, staging and shared apply/undo regressions |
| Production core library build | Passed, offline and locked |
| Before/after ordinary raw serialization | 14 synthetic records, byte-identical JSON |
| Full `archivefs-core --lib` | 10,869 passed, 3 ignored, 0 failed; 621.21 seconds |
| `cargo check --offline --locked --workspace` | Passed; four existing GUI warnings |
| `cargo fmt --all -- --check` | Passed |
| `git diff --check` and exact seven-file scope guard | Passed |

The raw serialization comparison uses the same temporary driver and provider
metadata before and after, covering write sizes, float/pointer/add/conditional
operations, refused opcode families, placeholders, empty input, case/CRLF and
Unicode. The cached pre-change library's classifier and helper sources were
verified identical to starting main (intervening commits were unrelated).
The candidate production library was rebuilt before the comparison. Both
outputs have SHA-256
`bede2ac4beb707067179a394b460e1e98a3c7d4f88ee6fd278fe75809c8f666b`.
The driver and both outputs are retained beside the observation snapshots in
the isolated temporary validation directory. Integrated tests additionally
assert decoded/raw typed records, serialized records, output digests and
GameHacking adapter inputs are identical for the same operational code.

Focused/regression/full tests use
`cargo --config 'profile.test.package.archivefs-core.debug=0' test --offline --locked -p archivefs-core --lib`
with the stated filter and `-- --test-threads=2`. Workspace checking uses the
exact requested command without that test-profile override. Regression suites
that require synthetic HTTP servers were allowed to bind localhost; they
did not use a real provider service or an installed emulator. The focused and
regression counts overlap the full-suite total, and are not added to it.
