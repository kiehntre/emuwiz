# Missing platform-folder identities — 2026-10-17

Starting main: `b55c0a2a07a96f70de2e801e5d7cf56104f6e13a`.
Integrated main: `609462a8` (shadPS4 integration and CHD header-identity wording; no alias/scanner/assignment-policy overlap).
The only production changes are three normalized aliases in the existing
platform registry. No canonical IDs, scanner precedence, cache version,
ES-DE mappings, database records or emulator backends change.

| Audited folder | Recognition policy | Existing ES-DE route | Unknown before | Unknown after copy-only refresh |
|---|---|---|---:|---:|
| pc-8800-series | Add `pc8800series` → NEC PC-8801 | pc88; full name already NEC PC-8800 Series | 3,898 | 0 |
| acpc | Add → Amstrad CPC | amstradcpc | 94 | 0 |
| sega32 | Add → Sega 32X | sega32x | 56 | 28 |
| dc | Existing Dreamcast alias; no duplicate addition | dreamcast | 58 | 26 |
| msx-turbo | No turbo-R canonical platform; existing negative test deliberately distinguishes it from MSX/MSX2 | No canonical route for this hardware | 42 | 42 |
| vc-4000 | No supported canonical platform | None | 76 | 76 |
| daphne | Emulator/collection name; no supported canonical platform with this name, and no per-game evidence to justify Arcade | None for this name | 18 | 18 |
| beena | No supported canonical platform | None | 11 | 11 |
| ipf | Media-format folder, not platform evidence | Amiga and Atari ST have their own routes; no ipf route | 248 | 248 |
| sega-pico | Unsupported Sega Pico, kept distinct from other Sega systems | None | 432 | 432 |

Canonical platform totals in the same snapshot, before → after the validated
copy-only assignment delta: NEC PC-8801 **11,721 → 15,619**, Amstrad CPC
**0 → 94**, Sega 32X **0 → 28**, Dreamcast **27 → 59**. These totals retain
historical rows and are not unique-game counts or production updates.

`acpc` and `sega32` already have reviewed inbound RomM mappings in
`identity_source/romm/normalise.rs` and `platform_evidence_fusion/romm_platform_mapping.rs`.
PC-8800 family spelling is established by the existing NEC PC-8801 ES-DE row,
its tests and `docs/ESDE_BATCH4_MAPPING_PLAN.md`. Registry normalization already
handles punctuation/case; matching remains exact on whole folder components,
never a filename or substring. These are probable folder inferences, not
verified game identities.

The registry, scanner and frontend are separate authorities: a provider slug
was missing from local folder recognition even though the canonical platform
and frontend route already existed. Unsupported hardware does not gain support
merely because another emulator might run it. No broader alias was added.

## Copy and read-only source validation

The production catalogue was opened with SQLite `mode=ro` and backed up to
`/home/davedap/emuwiz-audit-ledger/2026-10-17-platform-folders/library-copy.sqlite3`.
All catalogue analysis used that snapshot. Filesystem checks were limited to
exact affected catalogue paths; no collection scan or archive extraction ran.
Three 32-byte samples under `preservation/ipf` begin with CAPS/IPF container
headers. This confirms the media format only: IPF is shared with Atari ST, so
neither the extension, preservation directory nor game title proves Amiga.
Authoritative per-game evidence or an explicit owner classification is needed.

The ignored opt-in validation opens the supplied snapshot read-only, creates
another SQLite backup in its own TempDir and uses canonical `assign_platform`
on that disposable copy only. It excludes custom aliases, explicit source
assignments, non-game sources, existing identity facts and stronger DAT states.
It requires current Unknown, a present file and the expected exact folder alias.
The snapshot's affected DAT states are `no_match`, not verified identities.

The copy-only refresh can recognize **4,052 existing Unknown catalogue rows**:
3,898 PC-8800, 94 CPC, 28 32X and 32 Dreamcast. Of these, **4,020** benefit from
new aliases; Dreamcast's 32 were already recognizable by current main. Another
54 rows have unavailable paths (28 32X, 26 Dreamcast) and are excluded. There
are also three already-assigned PC-8800-folder rows, left untouched. Counts
are catalogue entries, not deduplicated or DAT-verified games. No production
counts have changed as a result of this task.

Protected-table hashes compare every value in archives, verified identity
facts, DAT identities, custom aliases, source folders and scan fingerprints
before/after the disposable assignment phase. Paths, game identities, DAT
results, user overrides, missing flags and caches must remain unchanged.
The input snapshot is never migrated or written; any required schema upgrade
is confined to the second disposable copy.

## Cache constraint and exact production proposal — NOT EXECUTED

Unchanged scan fingerprints reuse their saved platform. Ordinary cached rescan
alone can therefore leave these rows Unknown. No global cache invalidation or
scanner redesign is included. Synthetic tests demonstrate cached Unknown
versus fresh per-record `Archive::from_path_in_root` inspection.

Separate explicit owner approval is required for a **targeted per-record
identity refresh**, bypassing old cached platform results, for the allowlisted
4,052 IDs in the external `proposed-production-refresh.json` manifest:

- `/mnt/usbdrive/games/pc-8800-series`: 3,898 rows.
- `/mnt/usbdrive/games/acpc`: 94 rows.
- `/mnt/usbdrive/games/sega32`: 28 rows.
- `/mnt/usbdrive/games/dc`: 32 rows.

After approval, first take a fresh SQLite backup and revalidate each ID, source
root, relative/absolute path, current Unknown state, user/custom/source
assignments, DAT/identity authority and file device/inode/size/mtime. Require
canonical containment and fresh folder identification agreeing with the plan.
Skip and report every changed, unavailable, ambiguous or protected record.
Coordinate with the live application. Recheck current Unknown and all stored
authority within the same coordinated SQLite write transaction as assignment
(or an equivalent conditional guard); refuse execution if that cannot be
established. Persist only the accepted platform through canonical
`assign_platform` with `folder_alias` provenance, retaining IDs and all other
evidence. No source
registration, broad directory traversal, missing sweep, metadata/path rewrite,
DAT mutation, cache invalidation or production full-collection scan is proposed.
The count is provisional until that fresh revalidation; this is not an
executable approval or an already-performed production rescan.

## Validation

Focused integration regressions cover every alias, normalization, exact
components, filename and source-root boundaries, existing ES-DE routes,
unsupported folders, manual precedence, repeated synthetic scans preserving
IDs/paths/hashes/bytes, DAT/custom assignments and stale cache behavior.
Focused existing registry, scanner and database-rescan tests also run through
`scripts/cargo-iso --low-debug --incremental` in the owned warm worktree.
Final focused results: **149 passed** (5 new synthetic regressions, 1 explicit
copy-validation run, 122 registry tests, 7 scanner tests, 14 database-rescan
tests). One large scanner stress test remains intentionally ignored. After
the final CHD-wording upstream rebase, the 5 new regressions and copy-validation
run were repeated; existing registry/scanner/assignment behavior was unchanged.
Logs and the before/after count reports are retained in the external ledger.

Independent review requested correction of the copy helper's authority filters;
those exclusions and a custom-alias boundary regression were added. A test
isolation tripwire observed a GUI preference update during validation; a
separate EmuWiz process was running, but the update's cause is unattributed.
Task validation input is read-only and all assignment writes are explicitly
confined to its owned TempDir copy. No task production rescan or production
database write was performed; global database quiescence is not claimed.
