# Local cheat-pack preview and dry-run planning (Batch 6)

Base: `a5e604e7456c183c9835ac6a3081a981b91a2c03`, verified equal to
`origin/main` after fetch. Tracked main was clean; existing untracked research
files in its worktree were left alone. Branch `feature/cheat-pack-import-preview`,
worktree `/home/davedap/emuwiz-cheat-pack-preview`. Batches 1–4 were inspected
read-only for their seams; none were cherry-picked. No provider/network or GUI
workflow is added.

## Architecture audit

- `user_cheat_import` already scans local files/directories and recognizes
  RetroArch CHT, PCSX2 PNACH, Dolphin GameSettings INI and Xenia patch TOML.
  Its report is a file-level index, not an install plan. It includes a scan
  timestamp, file digests, title/platform hints, matches and duplicate-file
  observations. It does not retain individual code bodies. Its title/platform
  and platform-only matching semantics are too permissive for this dry run.
- `cht_document`, `pcsx2_pnach`, `gecko_document` and `xenia_patch_document`
  remain the native parser authorities. Current CHT parsing isolates missing
  and empty codes, emits typed warnings, retains source indices and blocks
  truncated/unsafe fields. Duplicate fields currently retain the first value;
  Batch 1/2 will supply richer malformed/duplicate evidence.
- `cheat_catalogue` is a metadata-only RetroArch source index with separate
  persistent catalogue construction. Pack preview does not build/write it.
- `cheat_ir` supplies known operation semantics, fingerprints and conservative
  cross-source reconciliation. Its current duplicate grouping can separate a
  duplicate subset from a same-name conflict. Preview's indexed connected
  buckets keep that larger group under review instead of choosing a winner.
- `cheat_reconciliation_plan` resolves explicit review choices later. Preview
  supplies no choices and persists none.
- `cheat_journey`, `shared_preview`, materialization/install plans and
  `shared_transaction` own discovery/selection/preview/staging, confirmation,
  apply, backup, journals and rollback. They remain unchanged and are not
  called by this pack analysis.
- GUI `user_cheat_import_page` exposes the existing local index; Cheats & Mods
  previews concern selected emulator destinations. Full pack review UI is
  deferred. No GUI root wiring is required for the independent domain API.

## Public API and evidence chain

`preview_cheat_pack(root, catalogue, associations, existing_keys, limits)` reads
one local plain file or a selected directory. Its only other inputs are
immutable snapshots: catalogue records with EmuWiz `IdentityEvidence`, optional
root-relative source associations, and known logical keys. No destination,
emulator profile, database, executor, review persistence or enable request is
accepted. `can_apply()` always returns false.

`plan_cheat_pack_preview(preview, existing_keys)` is the pure re-planning seam
for already inspected evidence. It validates references/bounds and rebuilds
ordering, groups, actions and totals. It performs no I/O. A caller-supplied
Ready state never grants install authority.

The typed chain is:

```text
CheatPackPreview
  files[]: format, source digest/path, parser metadata, diagnostics
    association, match strength and all candidate game matches
    observation_indices[]
  observations[]: native code, IR, source index, parser evidence,
    source defaults (informational), execution fields, applicability,
    logical key and prospective action
  logical_cheats[]: observation indices, relationships,
    source-content/copy/group/mirror counts, existing reconciliation evidence
  games[]: file and logical-cheat indices
  totals: counts derived from the above rows
```

RetroArch globals, leading comments, entry extras and warnings remain available.
Native unknown codes stay opaque; no source-code repair or universal pack
manifest format is invented. Associations are supplied in memory by callers;
no filesystem sidecar is created or downloaded.

## Identity and applicability

- Only existing `IdentityEvidence` with `status=Verified` can support an exact
  hash/CRC match or a strong serial/product match. Optional string fields in
  `UserCheatLibraryGame` do not imply verification. Conflicting verified values
  for a game-identity kind are not accepted as identity proof.
- Identity requirements are source expectations, never proof of the selected
  ROM. They must agree with catalogue facts; a conflicting requirement cannot
  fall back to a convenient title match.
- Existing PNACH filename identity parsing and Dolphin game-ID parsing are
  reused. No new filename heuristics are introduced. Title and ordinary
  filename associations remain Possible even when platform agrees.
- Platform alone does not match every catalogue game. Missing release evidence
  is not a wildcard. Tied best matches remain Ambiguous. Candidate overflow is
  explicit, bounded and also remains Ambiguous; it cannot select a winner from
  a retained prefix.
- Explicit region/revision constraints are compared only with verified release
  facts. Unsupported release evidence requires review. The in-memory
  applicability enum also accommodates UnsupportedFormat/UnsupportedTarget.
- Parser validity and native usability are separate from neutral IR decoding.
  Opaque native CHT codes can be prospective catalogue additions for a verified
  game without being executable/convertible neutral operations. Missing,
  truncated or unsafe codes are rejected; nonblocking parser issues require
  review. Valid entries beside malformed entries stay individually visible.
- Xenia module-hash applicability cannot be established from this narrow
  current-main catalogue input; otherwise valid Xenia observations require
  review instead of inventing that binding. Batch 4 can replace/feed the
  applicability assessment, including richer emulator/target constraints.

## Logical identity, duplicates and provenance

Logical keys are versioned SHA-256 digests of verified game identity (or a
conservative unresolved association), platform, known semantic fingerprint or
strict opaque code/format, execution fields, engine and explicit release
constraints. Main's semantic fingerprint is reused for proven operations.
PNACH execution mode and CPU are retained separately because the neutral
operation alone does not carry those constraints. Unknown code whitespace is
not normalized into equivalence.

Keys exclude absolute source paths, transient catalogue IDs, scan timestamps,
file insertion order and provenance. Provenance remains in every observation;
changing only its location preserves a proven logical key. Changing code or
applicability changes that key. Different verified games never share an index
bucket merely because their source bytes happen to match.

Indexed name/code/raw/index buckets form connected groups. For verified game
identity, exact and proven semantic-equivalent duplicates share one logical
group. Unverified associations
retain potential groups as ambiguous possible duplicates, not proven duplicate
cheats; code/name/index conflicts, explicit region/revision variants and
syntax/engine variants keep
all affected observations under review. Existing reconciliation results are
retained as evidence; their `auto_winner` remains absent. A duplicate subset
cannot bypass another conflicting implementation of the same cheat.

Source SHA-256 identifies byte content, not an independent author. Identical
files are known copies (`known_copies`); another path is not independent
corroboration. `distinct_source_contents` counts distinct input digests.
`source_group` and `mirror_of` are explicit optional provenance inputs for
Batch 3. `independent_source_groups` is zero unless such evidence is supplied;
there is no fake trust score. Exact-byte copies and declared mirrors produce
WouldRetainExisting, while distinct additional observations can produce
WouldCorroborate without claiming independent source authorship.

## Actions and reconcilable totals

Every observation has exactly one action:

- WouldAdd
- WouldCorroborate
- WouldRetainExisting
- WouldRequireReview
- WouldRejectMalformed
- WouldRejectUnsupported
- WouldRemainUnmatched
- WouldRemainAmbiguous

These are prospective catalogue/review outcomes, not transaction commands.
They neither install nor enable anything. Existing keys are an immutable
snapshot, never a persistent import cache.

`observations` equals the sum of the eight action buckets (the two rejection
kinds share `would_reject`). `actions_reconcile()` checks that equation.
Files partition into accepted, malformed and rejected rows. Malformed entry
counts refer to observation rows, while whole-file failures have file-level
counts and diagnostics. Duplicate/conflict totals count logical groups;
release mismatches and action totals count observations. `usable_cheats`
counts Ready logical groups with no unsafe sibling/conflict, not each source
observation. Games are grouped separately from source files.

## Bounds and filesystem policy

Defaults are hard ceilings; callers may only tighten them:

| Resource | Bound |
|---|---:|
| Source file | 512 KiB |
| Cumulative bytes read | 128 MiB |
| Tree entries enumerated (directories included) | 10,000 |
| Recursion depth below root | 16 |
| Retained observations per file | 1,024 |
| Retained observations / game matches per pack | 65,536 / 65,536 |
| Source path | existing 4 KiB bound |
| Line length / lines per file | 8 KiB / 16,384 |
| Code length / code lines | existing CHT 4 KiB bound / 1,024 |
| Retained diagnostics across the pack | 256 |
| Catalogue games / retained matches per file | 100,000 / 128 |

Catalogue facts are bounded to 64 per game; associations to 32 identity
requirements, with 4 KiB evidence strings. Native parsers retain their stricter
field/entry limits. Oversized code retains an explicitly labelled bounded
sample and full-code digest; operations are withheld and the record rejected.
Source bytes are never allocated from an untrusted declared cheat count.
Partial/refused reads count
against the cumulative budget too; the bounded reader may consume one extra
EOF/growth probe byte when rejecting overflow, after which further reads stop.
File, entry or diagnostic bounds are explicit in diagnostics/truncation flags;
`complete=false` means
retained totals are incomplete, not estimated full-pack counts.

The shared importer walk now bounds directory collection itself and the whole
tree, including empty directories. An overflowing directory is skipped as a
whole: retaining the first arbitrary `read_dir` prefix would be nondeterministic.
Collection charges the remaining tree budget immediately, including entries
in a subtree later refused for overflow. At most one extra enumeration probe
detects overflow; subsequent descendants are refused before opening them.
Already collected parent entries still receive their individual outcomes, so
an overflowing child does not hide an independent parent-level file.
All retained files sort by native path; maps/sets have deterministic ordering.

Symlink entries are skipped. On Linux directory enumeration is anchored to an
open directory descriptor, and source reads open every component using
`openat`/`O_NOFOLLOW`. A swapped parent cannot redirect content reads. The leaf
is opened nonblocking and checked as a regular file to avoid a FIFO swap
blocking the preview. Local pack reads fail closed on other OSes until an
equivalent confined reader is proven; pure planning remains portable.
Executables/scripts are never parsed. Parent traversal
is refused. No archive extraction or content execution occurs.

## Integration seams and deferred work

- Batch 1: `CheatPackDiagnostic` retains existing typed CHT warnings; adapt its
  richer malformed evidence through the same file/observation seam.
- Batch 2: typed relationships and preserved main reconciliation evidence
  provide a place for the richer duplicate classifier, without changing the
  preview action/accounting contract.
- Batch 3: observation provenance/source-group/mirror fields are separate from
  logical keys; replace/enrich them with `CheatRecordProvenance`.
- Batch 4: replace/feed association and applicability from its selected-game,
  release, parser and route evidence.
- Batch 5: no dependencies or integration are assumed.

Deferred: GUI pack review, applying/importing a plan, provider/download work,
network acquisition, arbitrary archive/universal pack formats, caching,
full emulator route compatibility, non-Linux confined source reading and
broader release/module-hash bindings.
No other cheat branch is integrated here.

Validation uses only focused preview/import/parser/reconciliation tests,
targeted core check, `cargo fmt --all -- --check` and `git diff --check`.
The large synthetic pack exercises 2,048 files without pairwise duplicate
materialization. Source/ROM/save/config/database byte snapshots, unchanged
input objects and no new filesystem output provide read-only evidence.

## Original focused validation results

Commands were run with `--offline` in the feature worktree; no full workspace,
full GUI, release or live GUI validation was run.

| Command (all tests use `-p archivefs-core --lib`) | Result |
|---|---:|
| `cargo test ... cheat_pack_preview --offline` | 60 passed |
| `cargo test ... user_cheat_import --offline` | 14 passed |
| `cargo test ... cht_document --offline` | 33 passed |
| `cargo test ... cheat_ir --offline` | 32 passed |
| `cargo test ... cheat_reconciliation_plan --offline` | 10 passed |
| `cargo check -p archivefs-core --lib --offline` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `git diff --check` | Passed |
| Exact seven-file scope guard / GUI boundary ratchet | Passed / zero GUI changes |

The final 2,048-file synthetic run completed in 1.064 seconds; all 60 preview
tests completed in 1.32 seconds after compilation. This measures the bounded
fixture on this machine, not a general performance guarantee. The malformed
fixture sweep, overflow cases, parent/leaf symlinks, unchanged filesystem
snapshots and repeated-output assertions passed.

## Continuation audit

The continuation request described uncommitted work on base
`a5e604e7456c183c9835ac6a3081a981b91a2c03`. The actual worktree was clean at
`dfc4ea09c4ad45b28629473b5aee975f5c655ea5`, which had already committed all
seven Batch 6 files above that base. That implementation was preserved. No
branch recreation, reset, restore, clean, rebase, cherry-pick, merge, push or
promotion was performed.

The audit read the complete existing preview, source adapters, tests and
documentation, and the tracked import/reconciliation/export changes. The
existing 60 preview tests passed before further edits. The model, matching,
grouping, stable identities, native parser adapters, mutation safety and
prospective action accounting were already present; they were not replaced.

One remaining bounds inconsistency was corrected in the shared importer
walker: it had capped each directory independently and charged only processed
entries. Repeated overflowing children could therefore enumerate more than
the advertised tree budget, and a child could consume the budget before an
already collected parent neighbour received its outcome. Collection now
consumes one global remaining budget before processing, including refused
subtrees, with one extra overflow probe and no further directory reads after
exhaustion. A single regression test covers the missing nested-budget case,
parent-neighbour isolation, deterministic repeated output, source bytes and
reconcilable action totals. Existing tests were retained without duplication.

The continuation changes only `user_cheat_import.rs`,
`cheat_pack_preview/tests.rs`, and this document. The original seven-file
Batch 6 implementation remains in its existing commit. Batches 1–5 remain
separate and explicitly deferred to the final integration lane.

## Continuation focused validation results

All test commands used `cargo test --offline -p archivefs-core --lib` with
the named filter. Only the focused modules below were run.

| Filter or check | Result |
|---|---:|
| `cheat_pack_preview` | 61 passed |
| `user_cheat_import` | 14 passed |
| `cht_document` | 33 passed |
| `cheat_ir` | 32 passed |
| `cheat_reconciliation_plan` | 10 passed |
| `cargo fmt --all -- --check` | passed |
| `git diff --check` | passed |
| Task scope / GUI root boundary | passed |

The 150 focused tests passed with no failures. Preview tests completed in
1.43 seconds after compilation; the 2,048-file fixture took 1.151 seconds on
this run. The changed `archivefs-core` library and its test binary compiled
successfully. A separate targeted `cargo check` was not needed or rerun in
this continuation; the earlier check above belongs to the original commit.
No full workspace/GUI suite, release build or live GUI smoke was run. The
follow-up is a local commit on the existing branch; no push or promotion is
part of this batch.
