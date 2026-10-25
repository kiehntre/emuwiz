# EmuWiz save snapshot / restore gap audit

Audit basis: authoritative main `33ecab378c54cfdfa4dd0303e1008b114b920878`.
This is a read-only audit. No production code, save data, emulator profile, or
transaction implementation was changed.

## CURRENT SAVE ARCHITECTURE

There are three distinct layers:

1. `persistent_state_inventory` is a provider-neutral, read-only inventory. It
   receives effective paths from emulator/profile discovery, refuses symlink
   roots and members, bounds recursion and entry count, hashes regular files
   up to 64 MiB, records emulator/profile/install context, and assigns a
   portability class.
2. `save_snapshots` creates local immutable snapshots of one file or a bounded
   directory. It copies in chunks into a `.partial-*` staging directory,
   hashes while copying, rechecks source metadata, writes a manifest, syncs
   files, and publishes with a directory rename. It has no apply executor.
3. `memory_card_inventory` has a separate PS2 Save Vault. It can inspect PS1
   and PS2 cards, export PS2 files/PSU packages, and restore a PSU into a
   healthy PS2 card with a verified card backup and specialized undo.

The GUI-v2 Saves & States page is read-only inventory. Game Details exposes
Saves & Backups, but Compare and Restore are disabled for the generic layer.
The PS1/PS2 Save Vault is the existing specialized restore surface.

| Capability | Status | Evidence |
|---|---|---|
| discovery and bounded inventory | COMPLETE | `persistent_state_inventory` |
| immutable local snapshot | COMPLETE | `create_snapshot` and manifest |
| snapshot verification | COMPLETE | `verify_snapshot` hashes every artifact |
| generic restore plan | PARTIAL | `build_restore_plan` only projects changes/conflicts |
| generic restore apply | MISSING / PREVIEW-ONLY | `apply_supported` is always false |
| shared restore journal | MISSING | no generic save restore receipt is emitted |
| specialized PS2 PSU restore | COMPLETE within its narrow format | card-specific plan/apply/undo |

## SUPPORTED SAVE FAMILIES

The enum and path classifier cover SRAM, EEPROM, flash, NVRAM, VMU, PSP
savedata, save directories, memory cards, PS2 cards, save states, and opaque
system storage. This is classification/inventory evidence, not proof that each
format is safely restorable.

| Family | Current handling | Restore classification |
|---|---|---|
| raw SRAM / EEPROM / flash / NVRAM | bounded file inventory and generic snapshot | PREVIEW-ONLY; no format-specific binding or writer |
| per-game save directory | bounded recursive inventory and generic snapshot | PREVIEW-ONLY; multi-file apply is missing |
| PSP savedata | path/profile inventory classification | PREVIEW-ONLY; no PSP savedata identity/apply adapter here |
| VMU | type exists and paths can be inventoried | PREVIEW-ONLY; no VMU container/member restore proof |
| PS1 memory card | format inspection and entry evidence | PREVIEW-ONLY; no per-game card mutation |
| PS2 memory card | full structural inspection plus PSU restore | SUPPORTED only for the specialized healthy-card PSU path |
| save states | inventory and generic snapshot, explicitly emulator-bound | PREVIEW-ONLY / version-bound |
| config-adjacent saves | MAME `.cfg` can be classified; system containers are opaque | PREVIEW-ONLY or DO_NOT_TOUCH |
| RPCS3/xemu/Cemu virtual/system storage | root is retained as one opaque system container | DO_NOT_TOUCH for generic per-game restore |
| multi-file/container/database saves | inventory preserves the container boundary | PREVIEW-ONLY unless a format-specific adapter proves safe semantics |

Configured profile orchestration currently composes concrete roots for
DuckStation, PPSSPP, PCSX2, RPCS3, and xemu. The GUI remembered-profile seam
also labels RetroArch, Dolphin, Flycast, MAME, Hatari, FS-UAE, Xenia, Cemu,
and Vita3K roots, but the generic inventory must not be mistaken for a
format-specific restore adapter for those emulators.

## SNAPSHOT STATUS

Snapshots are the strongest completed part of the feature:

- manifest format version, snapshot ID, source path, artifact type, platform,
  emulator/profile, provenance, timestamp, total size, completeness, and
  per-file relative path/size/mtime/SHA-256 are retained;
- storage is under the EmuWiz data directory, separated by a sanitized game
  identity or `unknown-game`;
- absolute source paths are required; symlink sources and symlink members are
  refused; traversal components are refused;
- file count, total bytes, depth, and available-space checks are bounded;
- a failed copy removes the staging directory and cannot become a complete
  snapshot;
- verification detects changed or unreadable snapshot artifacts.

Snapshot creation requires the caller to pass `NotDetected` emulator use.
`Running` and `Unknown` are refused. This is safe, but the module does not
itself discover process ownership or prove that a process has flushed a save.

## RESTORE STATUS

`build_restore_plan` is an evidence projection, not authorization to mutate.
It compares added, replaced, and unchanged members; detects current-save
changes/newer mtimes; checks original path, emulator, profile, completeness,
and caller-supplied emulator-use status; requires a future pre-restore
snapshot; and sets `apply_supported: false`.

The plan does not retain a complete destination precondition suitable for a
later executor: it computes observations while planning but has no generic
receipt containing the destination tree fingerprint, snapshot fingerprint,
identity binding, or journal ID. There is no generic atomic publication,
rollback, or history entry.

The PS2 PSU path is a deliberate exception, not a generic implementation. It
requires a healthy supported PS2 card, parses a bounded PSU, checks source PSU
SHA-256 and the inspected card SHA-256, creates a caller-selected backup with
`create_new`, syncs and rereads that backup, writes the whole card through a
temporary file plus sync/rename, re-inspects the card, and undoes only when the
post-restore card hash is unchanged. Its receipt is an in-memory result and
backup file, not the shared transaction/history journal.

## IDENTITY / BINDING

### Strong evidence currently retained

- exact snapshot artifact SHA-256 and size;
- exact source path and relative member paths;
- selected emulator/profile strings when supplied;
- emulator installation/version where discovered by persistent-state
  orchestration;
- PS1/PS2 card structure and card hash during the specialized PS2 operation;
- PS2 save directory raw name and parsed file content for PSU verification;
- candidate product/serial evidence extracted from card entries.

### Weak or incomplete evidence

`SaveSnapshotManifest.game_identity` is optional and is copied from existing
evidence; the snapshot layer does not derive or strengthen it. Generic
restore checks path, emulator, and profile, but does not require a verified
game identity, platform match, artifact-type match, region/revision match, or
media hash match. A filename/path can therefore be present as provenance
without proving ownership of the target game. That is acceptable for a
preview, not for destructive restore.

Memory-card entry identities are candidate evidence. Some are structured, but
heuristic names are explicitly marked filename-only. A shared card hash proves
the card, not that a selected game owns the whole card. The PS2 restore path
matches the PSU save name and card structure; it does not establish an
authoritative EmuWiz game identity before changing the shared card.

Title-only and filename-only evidence must never silently authorize generic
destructive restore.

## RUNNING-EMULATOR SAFETY

The generic snapshot API models `NotDetected`, `Running`, and `Unknown` and
refuses the latter two. This is a useful fail-closed contract, but there is no
save-specific process/quiescence detector in this layer. The caller supplies
the status. Therefore:

- generic snapshot/restore: `MUST_BE_CLOSED` unless an authoritative lifecycle
  observation proves quiescence;
- PS2 PSU restore: `UNKNOWN` from this audit perspective. The card plan does
  not accept an emulator-use status and does not itself prove that PCSX2 is
  closed. The GUI must require explicit closure/readiness evidence before
  presenting this operation as generally safe;
- save states: `MUST_BE_CLOSED` and version/profile-bound;
- opaque system containers: `UNKNOWN`, hence blocked.

EmuWiz must not kill an emulator automatically. The novice message should be:
“Close the emulator before restoring this save.”

## BACKUP-BEFORE-RESTORE

Generic restore has the correct declared requirement—an automatic
pre-restore snapshot is required—but no executor implements it.

The PS2 exception creates a separate backup file before card mutation, uses
exclusive creation, syncs it, rereads it, checks length and SHA-256 against the
original card, and removes the backup if verification fails. This meets the
byte-backup requirement for that operation, but the backup is user-selected,
not a shared journal artifact, and its lifetime/retention is not managed by a
generic receipt.

## ATOMICITY

- Single-file and directory snapshot creation: staged and published
  atomically at the snapshot-directory level.
- Generic single-file restore: not implemented. The future executor should
  stage beside the destination, fsync the bytes, verify, and rename only
  after all preconditions pass.
- Generic multi-file restore: not implemented. Do not copy members one by one
  into a live save directory. Use a complete staged directory/set and an
  atomic swap where the filesystem and emulator format permit it; otherwise
  refuse.
- PS2 card restore: whole-card temporary file, sync, rename, and post-write
  structural/content verification. Failure attempts to restore original card
  bytes. This is specialized raw-card publication, not a general directory
  transaction.

## ROLLBACK

Generic rollback is MISSING because generic apply is missing. The planned
model must retain:

- exact pre-restore snapshot and hash;
- exact post-restore destination fingerprint;
- destination path/set and target binding;
- transaction/history receipt and operation timestamp;
- whether the destination was created or replaced.

Undo is safe only if the destination still matches the recorded post-restore
fingerprint. Any external edit, emulator write, missing backup, path change,
or identity change must block destructive undo and request review.

PS2 undo already checks the current whole-card SHA-256 against the recorded
post-restore hash and verifies the backup bytes before atomically restoring
them. It should eventually emit the shared history receipt, but its current
result object is not that receipt.

## SAVE STATES

Save states are distinct from ordinary in-game saves in both the snapshot enum
and the inventory UI. Inventory warns that they are emulator/core/version
bound; portability is `EmulatorBound`. The generic snapshot can preserve bytes
but cannot prove compatibility with a changed emulator version, core, plugin,
architecture, renderer, firmware, BIOS, or runtime state.

A future restore plan must bind at least emulator/profile, emulator version,
core/version where applicable, platform/architecture, media identity, slot
number, and state format. Without those facts it should remain preview-only.
Never describe a battery-save restore as equivalent to a save-state restore.

## MEMORY CARDS

- PS1 cards are inspected as shared card images with entry evidence; there is
  no generic per-game mutation path. Treat the card as the preservation unit.
- PS2 cards are structurally parsed, including FAT/directory chains and health.
  PSU restore can add or replace one save only on a healthy supported card,
  with capacity checks, backup, atomic card publication, verification, and
  stale-safe undo. It still needs stronger game binding and lifecycle proof
  before being generalized.
- GameCube/Dolphin, Dreamcast VMU, and other card-like formats currently have
  no equivalent format-specific restore executor in this save layer. Their
  inventory records are not permission to edit shared cards.

Plain language: “This memory card is shared by several games. EmuWiz will not
change it unless the card format, target save, backup, and rollback are all
proven.”

## MULTI-FILE SAVES

Snapshot creation supports bounded directories and records every relative
member. Restore planning can list per-member additions/replacements, but no
generic apply can guarantee that a multi-file set will not be partially
updated. Per-file copying must remain refused. Directory swap semantics,
cross-device handling, open-handle behavior, and emulator quiescence need an
adapter-specific proof.

## EXTERNAL / SYNC OWNERSHIP

No cloud-save feature or upload path was found in the audited save layer.
Cloud-managed state is classified as `CloudManaged`/`DoNotTouch` where known.
Emulator-native sync directories, databases, virtual disks, and profile roots
should be treated as externally owned unless an adapter explicitly proves
ownership and safe publication. No future restore should silently mutate a
sync-managed directory or global emulator configuration.

## GUI CONTRACT

The eventual Saves & States surface should show:

- current path and path provenance;
- latest snapshot, count, timestamp, and verification state;
- emulator/profile/version and game identity evidence;
- artifact family and whether it is shared or multi-file;
- restore readiness and exact blocking reason;
- whether the emulator must be closed;
- pre-restore backup destination and rollback availability.

Actions should be:

1. Create snapshot
2. Preview restore
3. Restore, only when backend preflight is fully safe
4. Undo restore, only while the post-restore fingerprint is unchanged

The current disabled Restore controls are correct for the generic path. The
PS2 Save Vault may retain its specialized controls, but should surface its
card-level backup, identity, and lifecycle limitations rather than implying
generic save support.

## NOVICE LANGUAGE

Use:

- “Close the emulator before restoring this save.”
- “This memory card is shared by several games.”
- “EmuWiz found the exact snapshot bytes, but it has not proved this save
  belongs to the selected game.”
- “This save state is tied to an emulator/version and may not load elsewhere.”
- “Restore is blocked until EmuWiz can create and verify a backup.”
- “The destination changed after the preview, so restore was refused.”

Keep hashes, paths, fingerprints, and format diagnostics under Advanced
details.

## P0 GAPS

P0 means a missing guarantee prevents safe generic restore/apply:

1. **No generic mutation executor.** `SaveRestorePlan.apply_supported` is
   false; there is no safe publication path for ordinary files or directories.
2. **No complete target binding.** Generic plans do not require verified game
   identity, media identity, artifact type, or format compatibility.
3. **No shared restore transaction/history receipt.** Pre-restore backup,
   destination precondition, postcondition, journal ID, and undo eligibility
   are not represented together.
4. **No generic quiescence proof.** The API accepts caller-supplied status, but
   there is no authoritative running-emulator/process-lock check; PS2 restore
   does not take the status at all.
5. **No generic multi-file atomic publication.** Applying a directory member by
   member would leave a half-restored save set.
6. **Shared-card mutation is not generically safe.** Card-level identity and
   unrelated-save preservation are not proven for formats beyond the narrow
   PS2 PSU path.

These are code-backed gaps. Save retention, deduplication, cloud sync, and
automatic cleanup are not P0 safety blockers because they are out of scope.

## P1 GAPS

- adapter-specific identity resolvers for each emulator/save family;
- process/lifecycle probes and explicit user-confirmed closure fallback;
- format compatibility and emulator/core/version checks for save states;
- shared-card member ownership and non-target preservation proofs;
- cross-device filesystem and directory-swap behavior;
- durable history retention and user-visible receipt browsing;
- free-space checks that include backup plus staging plus publication headroom;
- symlink/race-resistant destination revalidation immediately before publish;
- clear external-sync ownership detection;
- snapshot retention policy and repair of abandoned `.partial-*` directories.

## QUICK WINS

1. Add a `SaveTargetBinding` requirement to generic plans and refuse when game
   identity, emulator, profile, or artifact type is absent/mismatched.
2. Add a destination tree fingerprint to `SaveRestorePlan`, including file
   list, sizes, hashes, and type/symlink state.
3. Reuse `create_snapshot` for a mandatory pre-restore backup, but record its
   manifest ID and hash in a restore receipt.
4. Keep generic Restore disabled while adding deterministic refusal reasons and
   tests for the existing preview.
5. Extend the PS2 receipt into the shared history projection without changing
   its narrow card mutation semantics.
6. Add an explicit “Close the emulator” preflight state to the GUI rather than
   inferring it from path discovery.

## IMPLEMENTATION SEAMS

Do not add these in this audit, but the smallest reusable seams are:

```text
SaveTargetBinding
  game identity, platform/media identity, emulator/profile/version,
  artifact family, slot/card identity, confidence and provenance

SaveQuiescenceRequirement
  must be closed / verified closed / unknown, with process evidence

SaveRestorePreflight
  source snapshot fingerprint, destination fingerprint, binding,
  format compatibility, space, path/symlink checks, backup plan

SaveRestoreReceipt
  transaction ID, pre-backup snapshot, before/after fingerprints,
  destination, operation, verification result, rollback eligibility

SaveRestorePlan
  existing preview projection extended only after the above evidence is complete
```

The shared transaction/history executor should own durable journal state and
rollback coordination. The adapter should own format parsing, identity, and
post-restore verification. A generic planner must reject adapters that cannot
prove all required facts.

## TEST PLAN

Use synthetic legal fixtures and isolated temporary roots:

1. single-file save restore with exact hash;
2. target missing and target creation;
3. existing target with verified replacement;
4. stale destination after preview;
5. wrong game identity;
6. wrong emulator/profile/version;
7. emulator running;
8. emulator closed with positive lifecycle evidence;
9. multi-file save set staged and atomically published;
10. shared memory card with unrelated saves preserved;
11. rollback after clean restore;
12. rollback refused after external modification;
13. save-state version/core mismatch;
14. symlink destination or parent;
15. insufficient space including backup and staging;
16. interrupted staging cleanup and no partial publication;
17. pre-restore backup failure leaves destination unchanged;
18. snapshot checksum mismatch;
19. source snapshot changes after preview;
20. transaction/history receipt survives restart;
21. PS2 specialized restore remains byte-identical on failed verification;
22. PS2 stale undo refuses after card modification.

Every apply test must assert source snapshot immutability, destination
postcondition, exact backup bytes, and no writes outside approved roots.

## IMPLEMENTATION ORDER

1. Specify and persist `SaveTargetBinding` and adapter-specific identity
   evidence; keep generic Apply disabled.
2. Add quiescence/lifecycle evidence and refuse unknown/running targets.
3. Add destination/source tree fingerprints and bounded space/path checks.
4. Integrate mandatory pre-restore snapshots with the shared transaction
   journal and exact receipts.
5. Implement a single-file native-save executor with temp-file publication,
   post-write hash verification, and guarded undo.
6. Prove one adapter at a time, beginning with simple single-file saves; keep
   format/identity-specific refusals explicit.
7. Implement multi-file directory publication only where atomic semantics are
   proven; otherwise retain preview-only behavior.
8. Treat shared memory cards and virtual/system containers as separate
   adapters; do not generalize PS2 card logic.
9. Add save-state restore only after version/core/media compatibility is
   proven.
10. Add GUI Restore only from backend readiness, with novice wording and
    expandable technical evidence.

