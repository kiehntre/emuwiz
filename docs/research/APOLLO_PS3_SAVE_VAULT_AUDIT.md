# Apollo PS3 Save Vault Audit

Research-only audit of Apollo Save Tool PS3, with emphasis on future EmuWiz Save
Vault restore safety. No production code, Save Vault implementation, GUI,
Publisher Profile, resigning, download, patch execution, or restore execution was
changed by this audit.

## Executive conclusion

An original PS3 native save must be preserved as an opaque, byte-addressable
directory tree. A successful restore cannot be inferred from a matching folder
name or `PARAM.SFO` alone. The minimum safe model is:

`title identity` + `save-directory identity` + `account/user binding` +
`PARAM.PFD` integrity state + `per-file protection state` + `complete original
bytes` + `destination/account context`.

Apollo's implementation demonstrates four important boundaries:

1. `TITLE_ID` identifies the game/product family, while the save directory and
   `PARAMS` data identify a particular save and owner context.
2. `Account ID`, local `User ID`, and `PSID` are distinct. `IDPS` is console-wide
   and is used for license-related operations, not as a substitute for save
   ownership.
3. Resigning is an integrity-preserving transformation over multiple layers. It
   is not a filename operation and is not equivalent to removing the copy-lock
   flag.
4. Native PS3 save protection is title-dependent. `PARAM.PFD` contains protected
   hashes and entries, and Apollo needs a title-specific `secure_file_id`/key
   configuration for many saves. Game payloads may add their own encryption,
   compression, and checksums.

The default EmuWiz policy should therefore remain: **preserve original bytes
first; inspect read-only where safe; never transform a PS3 save during ordinary
backup or restore.** A future restore planner should fail closed when account
binding is different or unknown, when the save is not a native PS3 format, when
integrity cannot be verified, or when a requested migration would require
resigning.

## 1. Scope, method, and inspected revisions

### 1.1 Repositories and revisions

The following public repositories were inspected through their source, README,
wiki, changelog, license, and database documentation. The PS3 source snapshot was
identified as `master` commit `687989878273` by the source index used during the
audit. Apollo-lib's current indexed source was identified as `main` revision
`98187c` by its source index. The two data repositories expose branch-tip content
through the read-only source mirror but did not expose a stable full commit hash
in this environment; they are recorded as `master` branch-tip inspections and
must be re-pinned before any implementation work.

| Repository | Revision inspected | Relevant material |
|---|---|---|
| [apollo-ps3](https://github.com/bucanero/apollo-ps3/tree/687989878273) | `master`, `687989878273` | native paths, save enumeration, SFO patching, PFD use, VMC/PSV/PS2 handling, online database integration |
| [apollo-lib](https://github.com/bucanero/apollo-lib/tree/98187c) | indexed `main`, `98187c` | patch engine, BSD operations, hashes/checksums, compression commands, host callbacks |
| [apollo-saves](https://github.com/bucanero/apollo-saves/tree/master) | `master` branch tip | database layout, title mapping, archive and description workflow |
| [save-decrypters](https://github.com/bucanero/save-decrypters/tree/master) | `master` branch tip | custom per-game decryptors, checksum fixers, sample-based verification workflow |

The exact source files/functions inspected in `apollo-ps3` were:

- `include/saves.h`: storage path macros, save flags/types, command codes,
  online URL, owner configuration, VMC and PS2 Classic entry points.
- `source/saves.c`: save list construction, local/USB/online enumeration,
  save-detail inspection, command registration, archive import/export, and
  VMC enumeration.
- `source/sfo.c`: `sfo_read`, `sfo_write`, `sfo_get_param_value`,
  `sfo_patch_account`, `sfo_patch_user_id`, `sfo_patch_psid`,
  `sfo_patch_directory`, `patch_sfo`, and trophy account patching.
- `source/pfd.c` and `include/pfd.h`: `pfd_init`, `pfd_import`, `pfd_export`,
  `pfd_validate`, per-entry hash calculation, HMAC-SHA1 tables, portability
  encryption, and protected-file encryption/decryption.
- `source/pfd_util.c`: title/game configuration, secure-file-ID lookup, PFD
  setup, and save resign orchestration.
- `source/exec_cmd.c`: command dispatch for copy, export, decrypt/import,
  SFO patches, resign, archive extraction, and online save download.
- `source/owner_xml.c`: user/account/PSID/IDPS override data.
- `source/ps1card.c`, `source/mcio.c`, and `source/ps2classic.c`: VMC and PS2
  Classic container paths, encryption, ECC, import/export, and enumeration.
- `source/menu_main.c`: displayed title, subtitle, folder, lock, user ID,
  account ID, owner status, and PSID details.
- `source/save_util.c`: Apollo's own application-settings save, useful as an
  example of a standard PS3 save created through the system save API, not as a
  general save-format specification.

The local EmuWiz inspection covered the current worktree at commit
`bea685e` (`feat(gui): add safe publisher apply and rollback workflow`) and
the existing PS3 identity files:

- `crates/archivefs-core/src/ps3_boot_evidence.rs`
- `crates/archivefs-core/src/ps3_disc_evidence.rs`
- `crates/archivefs-core/src/param_sfo.rs`
- `crates/archivefs-core/src/game_identity.rs`
- `docs/research/SONY_PLAYSTATION_SUPPORT_AUDIT.md`
- `docs/research/SONY_EXECUTABLE_FORMAT_AUDIT.md`
- `docs/research/SPACE_EFFICIENT_STORAGE_AND_CONVERSION.md`

No Save Vault implementation, PS3 save manifest schema, or
`SHARED_MEMORY_CONTAINER` implementation was found in this worktree. Existing
EmuWiz code and documents provide useful generic byte/hash/rollback principles,
but they do not currently model PS3 save ownership or restore execution.

## 2. PS3 save layout

### 2.1 Storage roots observed in Apollo

Apollo's `include/saves.h` defines these relevant roots:

| Storage | Apollo path | Interpretation |
|---|---|---|
| Internal HDD native saves | `/dev_hdd0/home/%08d/savedata/` | `%08d` is the active local PS3 User ID. |
| USB native saves | `/dev_usb%03d/PS3/SAVEDATA/` | The USB export/import tree. The save folder is beneath `SAVEDATA`. |
| HDD licenses | `/dev_hdd0/home/%08d/exdata/` | Per-user `.rif` licenses; separate from save data. |
| USB licenses | `/dev_usb%03d/exdata/` | `.rap` export/import material; not part of an ordinary save backup. |
| Apollo private data | `/dev_hdd0/game/NP0APOLLO/USRDIR/` | Apollo settings, cache, patches, and `owners.xml`; not game-save data. |

The HDD path is user-scoped. The USB path is not itself account-scoped, which is
why a USB save can be copied between users but may still fail to load until its
owner metadata and integrity data are compatible with the destination user.

### 2.2 Native save directory contents

A normal native save is a directory beneath `PS3/SAVEDATA` on USB or beneath the
current user's `savedata` tree on HDD. Apollo explicitly treats these as special
metadata files and exposes the remaining files as save data:

- `PARAM.SFO`: structured Sony metadata and owner fields.
- `PARAM.PFD`: the save's protected file table and integrity data when present.
- `ICON0.PNG`, `ICON1.PAM`, `PIC1.PNG`, `SND0.AT3`: presentation/media files.
- Remaining files: one or more game payload files, often with title-specific
  names and formats.

`PARAM.PFD` is not optional in the general native-save model. Apollo's own
documentation and maintainer responses explicitly note that RPCS3-style folders
may omit it and are therefore not equivalent to a hardware PS3 save.

The directory name is meaningful but not sufficient. It normally incorporates a
title/product identifier and a game-defined save suffix. Apollo reads the
directory from `SAVEDATA_DIRECTORY` in `PARAM.SFO` and has a title-ID change
operation; this is evidence that the directory name and SFO directory field must
remain coherent.

### 2.3 What identifies what

| Artifact/field | Identifies | Strength for future EmuWiz identity | Restore rule |
|---|---|---|---|
| `TITLE_ID` in `PARAM.SFO` | PS3 game/product identifier | Strong structured evidence when SFO is valid and the directory/layout corroborates it | Require exact match to destination title unless explicitly reviewed. |
| Save directory name | Particular save slot/profile and usually title prefix | Supporting evidence; not a complete owner proof | Preserve exactly; compare with `SAVEDATA_DIRECTORY`. |
| `SAVEDATA_DIRECTORY` | Save directory expected by the game/XMB | Confirmed metadata relationship | Preserve exactly for byte-preserving restore; mismatch is review/block. |
| `TITLE`, `SUB_TITLE`, `DETAIL` | Display metadata | Supporting evidence only | Preserve original bytes; may be displayed read-only. |
| `CATEGORY` | Content/save category | Supporting platform/content evidence | Do not rewrite during ordinary restore. |
| `APP_VER`, `VERSION` | Application/save version metadata | Supporting compatibility evidence | Record; do not use as sole compatibility proof. |
| `PARAMS.user_id_*` | Local PS3 user slot associated with save | Confirmed binding evidence when structurally parsed | Compare with destination user; changing it is a resigning operation. |
| `PARAMS.account_id` | PSN/account identity associated with save | Confirmed binding evidence when present | Compare; never silently replace. |
| `PARAMS.psid` | Console PSID recorded in save metadata | Confirmed console-binding evidence | Compare when available; replacement requires resigning/metadata rewrite. |
| `PARAM.PFD` entries/hashes | File set, sizes, protected hashes, and integrity state | Confirmed format evidence; not game identity by itself | Preserve and validate read-only; any changed bytes require regeneration. |
| Payload file bytes | Game state | Primary preservation artifact | Preserve byte-for-byte by default. |
| `IDPS` | Console identity used by license/key operations | Not a save-title identity; usually not stored as native save metadata | Record only as sensitive destination/context evidence if available; never treat it as an ordinary save field. |

## 3. Comparison with current EmuWiz evidence

EmuWiz's current PS3 identity path is deliberately narrower and read-only:

- `ps3_boot_evidence.rs` observes the `PS3_GAME/USRDIR/EBOOT.BIN` layout,
  `PS3_GAME/PARAM.SFO`, `TITLE_ID`, `TITLE`, `CATEGORY`, `APP_VER`, and SELF
  magic.
- `param_sfo.rs` is a bounded generic SFO parser. It produces structured entries
  and treats product-code extraction as evidence, not unconditional identity.
- `ps3_disc_evidence.rs` adds bounded `PS3_DISC.SFB`/PKG observations.
- The current identity spine persists or consumes `Ps3TitleId`; it does not parse
  native save `PARAMS`, `PARAM.PFD`, Account ID, User ID, PSID, or secure-file IDs.

Classification of the existing and proposed primitives:

| Primitive | Classification | Reason |
|---|---|---|
| Valid bounded SFO structure | `CONFIRMED_IDENTITY` for “this is an SFO-shaped object”; not by itself a save/game claim | The parser validates structure and bounds. |
| `TITLE_ID` in a PS3-shaped save directory with valid SFO | `CONFIRMED_IDENTITY` for title matching, subject to cross-checks | Apollo and EmuWiz both use the field as the title/product key. |
| `PS3_GAME`/`USRDIR/EBOOT.BIN` layout | `SUPPORTING_EVIDENCE` | It is a disc/install layout, not a native save layout. |
| `TITLE`, `SUB_TITLE`, `DETAIL`, `APP_VER`, `VERSION` | `SUPPORTING_EVIDENCE` | Useful metadata; not unique or load-bearing alone. |
| `PARAMS.user_id`, `PARAMS.account_id`, `PARAMS.psid` | `CONFIRMED_IDENTITY` for save binding when parsed from a valid native-save SFO | Apollo displays and patches these exact fields. |
| `PARAM.PFD` presence | `SUPPORTING_EVIDENCE` | It distinguishes a more complete native save, but presence does not prove valid hashes. |
| PFD top/bottom/entry/file validation | `CONFIRMED_IDENTITY` for integrity state | Apollo computes and compares protected hashes. |
| Filename/folder prefix alone | `HEURISTIC_ONLY` | It can be malformed, hand-created, region-mismatched, or emulator-generated. |
| `IDPS` as game identity | `DO_NOT_ADOPT` | It is console-wide and is not a title/save identifier. |

### Current genuine gap

There is no current EmuWiz primitive for native PS3 save binding. Phase 3 should
add a read-only evidence projection only after the manifest contract is designed:
title ID, save directory, SFO binding fields, PFD presence/validation state, and
per-file hashes. It should not reuse the disc `Ps3LayoutObservation` as if it were
a save manifest.

## 4. Account and user binding

### 4.1 Identifier matrix

| Identifier | Scope | Where Apollo uses it | What it means for migration |
|---|---|---|---|
| Local User ID | PS3-local user slot, commonly eight hexadecimal digits in HDD path | HDD root selection, `PARAMS` patching, owner display | A different local user is a different binding even if the PSN account is the same or unknown. |
| Account ID | User/PSN-account identity, represented by 16 hex characters in Apollo's owner data | `PARAMS.account_id`, Change Account ID, fake/offline owner options | Different Account ID requires resigning or an equivalent game/platform-supported migration. |
| PSID | Console-wide per-console identifier | `PARAMS.psid`, remove-console-ID option, owner configuration | Different console can require a PSID change; treat as a binding mismatch, not a cosmetic field. |
| IDPS | Console-wide identity pair | owner configuration and license import/export (`.rif`/`.rap`) | Do not put raw IDPS in ordinary cloud manifests by default. If captured, protect it and scope it to license workflows. |
| Secure File ID/key | Title/game-specific protection material | `games.conf`, PFD and protected-file handling | Missing or incorrect title key blocks trustworthy resigning/decryption. It is not a user identity. |

Apollo's `owners.xml` model is especially informative: each owner record groups a
local `user id` and `account_id` with console `psid` and `idps`. The UI can select
the desired account and Apollo then patches the save's SFO fields using the chosen
owner context. The `IDPS` is explicitly described as required for license
import/export, while save owner selection is expressed by user/account/PSID.

Apollo also exposes “Remove ID/Offline”, a fake owner value
`ffffffffffffffff`, and “Remove Console ID”. These are compatibility modes, not
proof that the original binding has been safely migrated. They must not be
silently emulated by Save Vault.

### 4.2 Save Vault binding states

Recommended manifest fields:

- `platform = ps3`
- `format = native_ps3_savedata | ps1_vm_container | ps2_vm_container |
  psv_export | ps2_classic_container | unknown`
- `title_id` and source of evidence
- exact save directory name and `SAVEDATA_DIRECTORY`
- `local_user_id` (redacted/display-safe representation plus exact value only if
  the user explicitly permits sensitive metadata)
- `account_id` (prefer encrypted/protected storage; do not expose in ordinary UI)
- PSID presence and a salted/fingerprint form by default
- IDPS presence only as protected, opt-in context; never log it casually
- whether the source is hardware-native, USB-exported, RPCS3-like, or community
- PFD presence and validation result
- payload protection classification: `unknown`, `unprotected`, `PFD-protected`,
  `game-specific`, `mixed`
- `requires_resign` decision and evidence
- original per-file byte hashes and sizes

The preflight decision should be:

| Evidence relationship | State |
|---|---|
| Exact title, same Account ID, same local User ID, same PSID or no PSID conflict, PFD valid, destination native PS3 root | `SAME_ACCOUNT` / `SAFE_TO_RESTORE` |
| Exact title, same Account ID but different local User ID | `DIFFERENT_ACCOUNT` is false, but `REVIEW_REQUIRED`; do not call it safe without a supported user-slot policy |
| Exact title, different Account ID | `DIFFERENT_ACCOUNT` + `REQUIRES_RESIGN` |
| Account field absent, redacted, fake, zeroed, or unreadable | `UNKNOWN_BINDING` + `REVIEW_REQUIRED`; if native PS3 load is expected, normally `BLOCKED` |
| Source is known community save or another user's save | `DIFFERENT_ACCOUNT` or `UNKNOWN_BINDING`; `REQUIRES_RESIGN` |
| Source/destination emulator format differs from native PS3 format | `REQUIRES_RESIGN` or `UNSUPPORTED`; never silently copy |
| Invalid PFD, missing required files, or changed bytes without regenerated integrity data | `BLOCKED` |

## 5. PARAM.SFO and save identity

Apollo's SFO parser is not merely display code. `source/sfo.c` reads and writes a
typed SFO table, then patches fields in-place while preserving the overall SFO
container. The relevant save fields are:

- `TITLE_ID`: product/title identity and the key used to find title-specific
  configuration.
- `TITLE`, `SUB_TITLE`, `DETAIL`: user-facing save metadata.
- `CATEGORY`: content category.
- `APP_VER` and `VERSION`: application/save version context.
- `SAVEDATA_DIRECTORY`: expected directory name.
- `PARAMS`: binary PS3 save-owner structure containing account/user/PSID data and
  copy-lock-related state.

Apollo's `menu_main.c` decodes `PARAMS.user_id`, `PARAMS.account_id`, and
`PARAMS.psid` for “View Save Details”. Its `sfo.c` patch path updates account ID,
both stored user-ID fields, PSID, directory, and lock flags, then writes the SFO
back to disk.

### Preserve versus regenerate

| Item | Preserve exact bytes? | May be regenerated? | Audit position |
|---|---:|---:|---|
| Whole original `PARAM.SFO` | Yes for backup/restore | Only in an explicit future format-aware migration | Treat any write as a transformation. |
| `TITLE_ID` | Yes | Only in a reviewed region/title migration | Never infer a game change from display name. |
| `TITLE`, `SUB_TITLE`, `DETAIL` | Yes | Usually display-only edits | Not load-bearing identity. |
| `SAVEDATA_DIRECTORY` | Yes | Only with coordinated folder rename and integrity review | Mismatch is unsafe. |
| `PARAMS.account_id/user_id/psid` | Yes | Resigning only | A changed value invalidates original provenance and may require PFD/file changes. |
| copy-lock flag | Yes | Unlock operation only | Unlocking is not resigning and changes metadata. |
| `PARAM.PFD` | Yes | Rebuilt/exported only by a trusted format-aware operation | A changed SFO/payload requires PFD updates. |

## 6. Resigning semantics

Apollo uses the following meanings, which should remain separate in EmuWiz
terminology:

### Resign save

“Apply Changes & Resign” means applying selected changes and making the result
internally acceptable to a target PS3 owner. In the generic native-save path it
can involve:

1. patching `PARAM.SFO` owner fields and/or copy-lock state;
2. decrypting protected files using the title's secure-file ID configuration;
3. applying a payload patch or accepting an imported decrypted file;
4. re-encrypting protected files;
5. recalculating file hashes, PFD table hashes, and PFD signature material;
6. writing the updated `PARAM.PFD`.

The exact sequence varies by title and protection mode. The operation is not
equivalent to copying the directory or changing a folder name.

### Unlock save

Apollo's “Remove copy protection” is an SFO patch that clears the lock/attribute
which prevents ordinary copying. It enables transfer of a save but does not
necessarily change Account ID, User ID, PSID, encrypted payloads, or every PFD
integrity layer. Save Vault must model it as a metadata transformation, not as a
successful account migration.

### Copy save between users

Copying is a filesystem operation. It can copy a USB save into a user's HDD root,
or copy an HDD save out to USB. It does not guarantee that the destination user
can load it. A copied save from another account normally needs resigning; a
copy-locked save may also need the lock removed before the normal XMB copy path.

### Fake-account support

Apollo's fake/offline owner options alter account metadata to support offline or
Rebug-style workflows. The use of `ffffffffffffffff` is an explicit sentinel in
Apollo's UI, not evidence of the original owner's account. EmuWiz should record
such a source as `UNKNOWN_BINDING` or `COMMUNITY/FakeOwner`, not as
`SAME_ACCOUNT`.

### Not all PS3 saves are equivalent

The maintainer's documented `games.conf`/discussion guidance says Apollo needs a
title-specific secure-file ID for many saves. Some titles are unprotected;
others use standard PFD protection; others add game-specific crypto or checksums.
The changelog names custom handlers for, among others, Naughty Dog, Diablo 3,
DmC, GTA V, NFS Rivals, MGS5, and Final Fantasy XIII fixes. This is direct
evidence that “PS3 save” is not one transformable format.

## 7. Checksums, HMAC, encryption, and compression

### 7.1 Generic PFD layer

Apollo's `source/pfd.c` shows a structured integrity pipeline:

- PFD v3/v4 headers are imported from `PARAM.PFD`.
- The signature contains a hash key; v4 derives the real hash key using HMAC-SHA1
  with a key-generation key.
- Top and bottom PFD tables are HMAC-SHA1 protected.
- Entry hashes cover PFD entry metadata and keys.
- File hashes are calculated with a key selected by file type or the title's
  secure-file-ID callback.
- `PARAM.SFO` and protected game files can have different hash-key derivation
  paths.
- Protected file data uses a title/configuration-derived AES-based scheme in the
  PFD implementation; the source is not a generic “AES any PS3 file” API.

Thus, PFD validation is more than SHA-256 of the outer files. A Save Vault
manifest should retain both ordinary cryptographic file hashes and a typed
format-validation result if a future read-only validator is added.

### 7.2 Game-specific layer

Apollo-lib exposes generic BSD operations such as `decrypt`, `encrypt`, `compress`,
`decompress`, and numerous hash/checksum functions. The save-decrypters repository
contains separate tools for specific games and separate checksum fixers. Examples
include custom handling for Final Fantasy XIII, GTA V, Naughty Dog saves, MGS5,
Diablo 3, DmC, Resident Evil, and multiple title-specific checksum families.

Classification:

| Capability | Classification | EmuWiz posture |
|---|---|---|
| Whole-file SHA-256/size/path capture | `GENERIC` | Adopt for opaque preservation manifests. |
| ZIP member/path validation | `GENERIC` but security-sensitive | Optional read-only archive inspection with strict path confinement. |
| PFD table and file-HMAC validation | `PLATFORM-SPECIFIC` | Optional future PS3 validator; no transform. |
| PS3 secure-file-ID encryption/decryption | `PLATFORM-SPECIFIC` plus title configuration | Never in ordinary Save Vault. |
| GTA/DmC/Diablo/Naughty Dog/MGS5/custom crypto | `GAME-SPECIFIC` | Never infer or generalize from one game. |
| Payload CRC/checksum repair | `GAME-SPECIFIC` | Only a future explicit transformation feature, if ever approved. |
| BSD/MicroPython scripts and arbitrary patch commands | `EXECUTABLE/UNTRUSTED INPUT` | Do not execute from Save Vault or community imports. |
| Deflate/LZ-style payload compression | `GAME-SPECIFIC` or patch-engine operation | Do not normalize or recompress opaque saves. |

### 7.3 Default storage rule

Save Vault should:

1. preserve the complete original directory tree and bytes;
2. store per-file size, mode where relevant, and cryptographic digest;
3. record whether `PARAM.SFO` and `PARAM.PFD` were observed, without rewriting
   them;
4. optionally run bounded validators that never write;
5. represent unknown or unvalidated protection honestly;
6. never transform a save as part of backup, deduplication, compression, restore,
   or account detection.

Do not store only decrypted payloads, only `PARAM.SFO`, or only a ZIP archive
without the original member paths and hashes.

## 8. PS1/PS2 virtual memory cards hosted on PS3

Apollo treats these as separate families, not ordinary PS3 native saves.

### 8.1 PS1

Observed Apollo paths/formats include:

- USB raw PS1 saves under `/PS1/SAVEDATA/` (`.mcs`, `.psx`).
- USB PS1 VMCs under `/PS1/VMC/` (`.mcr`, `.vm1`, `.vmp`, `.bin`, `.vmc`,
  `.gme`, `.vgs`, `.srm`, `.mcd`).
- HDD VM1 cards under `/dev_hdd0/savedata/vmc/`.
- PSV export/import under `/PS3/EXPORT/PSV/`.

Apollo's PS1-card code enumerates contained saves and supports VMP resigning and
conversion/import/export. A raw PS1 VMC is normally an emulator/memory-card image
with its own card format and checksums/ECC rules; it does not acquire PS3 native
Account ID, User ID, PSID, or `PARAM.PFD` merely because it is stored under a PS3
path. A `.VMP`/PSV wrapper may have PS3 transfer metadata and a resign operation,
so the wrapper must be preserved and classified separately from the contained
card bytes.

### 8.2 PS2

Observed Apollo paths/formats include:

- USB save-transfer formats under `/PS2/SAVEDATA/` (`.xps`, `.max`, `.psu`,
  `.cbs`, `.sps`).
- USB VMCs under `/PS2/VMC/` (`.vmc`, `.vme`, `.vm2`, `.bin`, `.ps2`, `.mc2`,
  `.mcd`).
- PS2 export cards under `/PS3/EXPORT/PS2SD/`.
- HDD VME cards under `/dev_hdd0/home/%08d/ps2emu2_savedata/`.
- HDD VM2 cards under `/dev_hdd0/savedata/vmc/`.
- PS2 Classic images under `/PS2ISO/`, with encrypted `.bin.enc` variants.

Apollo provides PS2 VMC crypto/ECC routines and PS2 Classic image import/export.
The important distinction for EmuWiz is wrapper versus card:

| Object | Extra PS3 wrapper/binding? | Recommendation |
|---|---:|---|
| Raw PS2 VM2/VM1/VMC image | Usually no PS3 Account ID/PFD semantics | Preserve as an opaque `SHARED_MEMORY_CONTAINER` candidate; inspect card geometry and contained directory read-only. |
| VME/PSV/export wrapper | Yes, transfer/container semantics may exist | Preserve wrapper and contained bytes; do not reduce it to a raw VMC. |
| PS2 Classic encrypted memory card/image | Yes, PS2 Classic encryption and PS3 package/context | `REQUIRES_RESIGN` or `UNSUPPORTED` for a native PS3 destination unless the exact wrapper/context is known. |
| Contained PS1/PS2 save | Game/card identity, not necessarily PS3 account identity | Record contained identity separately from host-container identity. |

### 8.3 Comparison to `SHARED_MEMORY_CONTAINER`

`SHARED_MEMORY_CONTAINER` is the right conceptual category for a memory card that
can contain multiple saves, but it must not erase host-format provenance. The
manifest should record:

- host platform/container format and exact outer bytes;
- card size/geometry and any ECC/checksum state;
- contained save directory entries and offsets, if read-only enumeration is
  supported;
- contained title/game IDs and confidence separately from host identity;
- whether the card is raw, wrapped, encrypted, or converted;
- source and destination emulator/PS3 expectations.

No evidence found in Apollo supports applying ordinary PS3 Account ID/User ID
resigning rules to raw PS1 or PS2 VMC bytes. Conversely, VME/PSV/PS2 Classic
wrappers must not be treated as ordinary emulator `.vmc` files.

## 9. Community-save database and provenance

Apollo's online database is a GitHub Pages-backed tree:

- root `games.txt` lists available games/title IDs;
- each game has a folder named by `TITLE_ID`, such as `BLUS12345`;
- save archives are numeric `.zip` files inside that title folder;
- each archive contains a save-game directory and data;
- each title folder has `saves.txt` with descriptions;
- Apollo downloads/extracts the archive to USB or HDD and tells users that
  downloaded files must be resigned before loading.

This is useful provenance metadata, not a cryptographic trust chain. The archive
description does not prove:

- who created the save;
- which Account ID/PSID it contains;
- whether the archive has been altered after upload;
- whether the title mapping is correct for the user's region/build;
- whether the payload is encrypted, unprotected, or game-specific;
- whether the archive is loadable on hardware or only in an emulator.

### 9.1 Should EmuWiz support community-save import?

Recommendation: **not in the Save Vault restore path**. If EmuWiz ever supports a
separate community-save workflow, it must be an explicit, isolated import feature
with a review screen and no automatic restore into a live emulator directory.

Minimum controls would include:

- HTTPS fetch with pinned source URL/commit or signed index where available;
- archive size/member count limits;
- reject absolute paths, `..`, symlinks, hardlinks, device nodes, and duplicate
  case-folding paths;
- extract into a quarantine directory outside any emulator root;
- verify archive/member hashes and record source URL, revision, description,
  download time, and local digest;
- inspect `TITLE_ID`, directory name, SFO, PFD, and binding before offering any
  import;
- never execute BSD scripts, MicroPython, patch files, binaries, or shell commands
  from the archive;
- require explicit user confirmation that the result may need resigning;
- keep the community artifact distinct from the user's own Save Vault snapshot.

Threats include malicious archives and path traversal, incorrect or deliberately
misleading title mappings, corrupt/truncated payloads, crafted parser inputs,
arbitrary patch/script execution, and account-binding mismatch. The Apollo save
database's convenience does not remove those risks.

## 10. Phase 3 restore-planning impact

The following should be read-only preflight checks; this audit does not authorize
or implement them.

| Preflight | Required evidence | Failure result |
|---|---|---|
| Snapshot integrity | Manifest parse, complete member set, per-file hashes, immutable snapshot ID | `BLOCKED` |
| Title ID match | Valid SFO `TITLE_ID`, directory/title corroboration, destination title identity | `REVIEW_REQUIRED` or `BLOCKED` on mismatch |
| Save-directory match | Exact `SAVEDATA_DIRECTORY` and destination naming policy | `REVIEW_REQUIRED`; `BLOCKED` if ambiguous |
| Account binding | Source Account ID, User ID, PSID state compared with destination context | `SAFE_TO_RESTORE` only for proven compatible context |
| Resign requirement | Same-account determination plus copy-lock/PFD/protection evidence | `REQUIRES_RESIGN` if another account or transformed source |
| Destination root | Explicit native PS3 user root, emulator root, or VMC target; no inferred broad root | `BLOCKED` if unresolved |
| Existing save | Exact destination member inventory and hashes; no silent overwrite | `REVIEW_REQUIRED` and pre-restore snapshot required |
| Encryption/checksum | PFD validation and known protection state; no assumption from extension | `BLOCKED` or `UNSUPPORTED` when unknown for native load |
| Running-emulator risk | Detect/ask about RPCS3/other process and active save files | `BLOCKED` until stopped/confirmed safe |
| Pre-restore snapshot | Verified snapshot of current destination before mutation | `BLOCKED` |

Recommended outcome classes:

- `SAFE_TO_RESTORE`: only exact title, compatible binding, verified snapshot,
  known destination, no unsupported transformation, and destination conflict
  explicitly handled.
- `REVIEW_REQUIRED`: metadata is coherent but user context, destination conflict,
  emulator state, or format support needs explicit review.
- `REQUIRES_RESIGN`: source belongs to another account/user/console or has an
  explicit transformation history; no execution in the current phase.
- `BLOCKED`: invalid/incomplete snapshot, invalid PFD, title mismatch, unsafe
  archive paths, unknown native protection where loadability is required, or
  running target risk.
- `UNSUPPORTED`: PS3 Classic/encrypted wrapper, emulator-only save representation,
  game-specific format not understood, or a transformation not implemented and
  not safely classifiable.

An ordinary same-account byte-preserving restore should copy the entire native
save directory, including metadata and media, after capturing the destination
snapshot. It must not regenerate SFO/PFD or normalize filenames. A restore to a
different Account ID is not a restore-only operation; it is a future migration
workflow and must be visibly separated.

## 11. Verification model

Apollo/save-decrypters provide a useful testing principle: do not trust a
transformation because it “looks right.” The save-decrypters CI applies a patch
to real encrypted samples and compares the resulting plaintext byte-for-byte with
the expected output from the C implementation. Apollo's PFD validator similarly
checks computed hashes against stored tables and reports failure by layer.

If EmuWiz ever adds PS3 format-aware transformation support, the minimum test
model should be:

1. a legally obtained, versioned known-good sample for each supported title and
   protection family;
2. a byte-for-byte expected output for decrypt/encrypt/resign round trips;
3. expected SFO binding-field changes, with all unrelated bytes compared;
4. PFD top/bottom/entry/file validation before and after transformation;
5. payload checksum validation where the title has a known custom checksum;
6. before/after whole-file and per-member hashes;
7. deterministic output tests across repeated runs;
8. malformed/truncated/wrong-title/wrong-key samples that fail closed;
9. tests proving no path traversal, script execution, symlink extraction, or
   destination clobbering;
10. a real hardware or accurately compatible target validation step before any
    “loadable” claim.

No single successful decryptor sample should be generalized to all PS3 saves.
The test matrix must name the title ID, game version/build, secure-file-ID
configuration, wrapper/protection mode, and expected output hashes.

## 12. Licensing and implementation strategy

| Project | License observed | Implication |
|---|---|---|
| `apollo-ps3` | GNU GPL v3 or later | Reuse/linking/copying source requires a GPL-compatible distribution strategy and corresponding obligations. |
| `apollo-lib` | GNU GPL v3 or later | Same; this is not a permissive format library. |
| `save-decrypters` | GNU GPL v3 | Game-specific source reuse has GPL obligations and may contain additional third-party provenance to review. |
| `apollo-saves` | GNU GPL v3 | Database code/content distribution must preserve the license and provenance; save files may have separate copyright/rights issues. |
| mbedTLS | Apache-2.0 (modern releases; Apollo also documents historical dependency variants) | Review the exact vendored/system version and notices before reuse. |
| zlib | permissive zlib license | Notice obligations remain; this does not change the GPL status of Apollo code. |
| libzip/libxml/PS3 SDK and related dependencies | dependency/version-specific | Audit exact build inputs and notices; do not assume a transitive dependency license. |

The safe default for EmuWiz is an independent implementation from documented
formats and independently verified samples, without copying Apollo source,
game-specific keys, patches, or scripts. Before reuse, obtain an explicit legal
review of GPL compatibility with EmuWiz's distribution model and of rights in
game-save samples. This audit does not make a legal determination.

## 13. Genuine gaps only

These are actual missing primitives, not speculative feature requests:

1. No Save Vault manifest fields for PS3 Account ID, User ID, PSID, PFD state, or
   native-save versus emulator representation.
2. No read-only native PS3 save-layout observer separate from disc/install
   `PS3_GAME` identity.
3. No typed model for `SAME_ACCOUNT`, `DIFFERENT_ACCOUNT`, `UNKNOWN_BINDING`,
   `REQUIRES_RESIGN`, `BLOCKED`, and `UNSUPPORTED`.
4. No Phase 3 preflight contract for destination user root, existing save
   conflicts, emulator-running state, or pre-restore snapshots.
5. No PS1/PS2 host-container versus contained-save provenance model.
6. No policy boundary for community-save quarantine and non-execution.
7. No stable pinned-revision record for the two Apollo data repositories in this
   audit environment; implementation work must re-pin them.

## 14. Ranked recommendations

### P0 — Preserve and classify

1. Define a Save Vault manifest that stores the complete original PS3 save tree,
   per-file hashes/sizes, exact directory names, title ID, SFO metadata evidence,
   PFD presence/validation status, and binding state.
2. Keep ordinary backup/restore byte-preserving and transformation-free.
3. Fail closed on title mismatch, invalid/incomplete snapshot, unsafe paths, and
   unknown native protection where hardware loadability is expected.

### P1 — Read-only evidence

4. Add a bounded native-save observer that parses `PARAM.SFO` and recognizes
   `PARAMS`, without writing it or claiming resign capability.
5. Add optional PFD structural/hash validation as a separate evidence result;
   never conflate “PFD valid” with “save belongs to this account.”
6. Add `SHARED_MEMORY_CONTAINER` provenance for raw/wrapped PS1/PS2 VMCs and
   contained-save identities.

### P2 — Restore planning

7. Add preflight-only outcome states and destination checks described in section
   10, including mandatory destination snapshot and running-emulator refusal.
8. Record sensitive account/console identifiers using protected or fingerprinted
   representations; avoid logging raw Account ID, PSID, or IDPS.

### P3 — Explicitly defer

9. Do not implement resigning, fake-account activation, PFD rewriting, game
   decryption, checksum repair, community downloads, arbitrary patch execution,
   or automatic emulator/native conversion in the current Save Vault phase.
10. If any transformation is later approved, implement it as a separately
    licensed, separately tested, title-specific toolchain with deterministic
    known-sample validation and explicit user consent.

## Sources

1. [Apollo Save Tool PS3 README](https://github.com/bucanero/apollo-ps3/blob/master/README.md) — PS3/USB/HDD paths, owner overrides, online database, resign warning, and GPL notice.
2. [`apollo-ps3/include/saves.h`](https://github.com/bucanero/apollo-ps3/blob/master/include/saves.h) — storage macros, save flags/types, commands, VMC paths, owner and online URLs.
3. [`apollo-ps3/source/saves.c`](https://github.com/bucanero/apollo-ps3/blob/master/source/saves.c) — save enumeration, metadata handling, SFO command registration, archive/VMC workflow.
4. [`apollo-ps3/source/sfo.c`](https://github.com/bucanero/apollo-ps3/blob/master/source/sfo.c) — SFO parsing and patching of account, user, PSID, directory, and lock fields.
5. [`apollo-ps3/source/pfd.c`](https://github.com/bucanero/apollo-ps3/blob/master/source/pfd.c) — PFD import/export, AES portability processing, HMAC-SHA1 tables, protected-file hashes, and validation.
6. [`apollo-ps3/source/pfd_util.c`](https://github.com/bucanero/apollo-ps3/blob/master/source/pfd_util.c) — title-specific PFD/secure-file-ID setup and resign orchestration.
7. [`apollo-ps3/source/owner_xml.c`](https://github.com/bucanero/apollo-ps3/blob/master/source/owner_xml.c) — owner/account/user/PSID/IDPS override data.
8. [`apollo-ps3/source/exec_cmd.c`](https://github.com/bucanero/apollo-ps3/blob/master/source/exec_cmd.c) — copy, resign, decrypt/import, archive extraction, and online-download command paths.
9. [`apollo-ps3/source/menu_main.c`](https://github.com/bucanero/apollo-ps3/blob/master/source/menu_main.c) — displayed save title, subtitle, folder, lock, User ID, Account ID, owner, and PSID.
10. [Apollo Save Tool changelog](https://github.com/bucanero/apollo-ps3/blob/master/CHANGELOG.md) — custom decryptors/checksums and secure-file-ID updates by title.
11. [Apollo Save Tool maintainer discussion on RPCS3 savedata](https://github.com/bucanero/apollo-ps3/discussions/28) — emulator save-layout/encryption/PARAM.PFD incompatibility warning and unprotected override caveat.
12. [Apollo Save Tool maintainer discussion on missing secure-file IDs](https://github.com/bucanero/apollo-ps3/discussions/27) — title-specific key requirement for resigning.
13. [Apollo Save Tool Core library](https://github.com/bucanero/apollo-lib) — GPL library, BSD/Python/Save Wizard engine, hashes, encryption, and compression operations.
14. [Apollo-lib documentation](https://github.com/bucanero/apollo-lib/tree/main/docs) — patch formats and validation guidance.
15. [Apollo Save Game Database README](https://github.com/bucanero/apollo-saves) — title-folder mapping, ZIP structure, `games.txt`, `saves.txt`, and upload workflow.
16. [PlayStation Save Decrypters](https://github.com/bucanero/save-decrypters) — game-specific decryptors/checksum fixers, sample list, byte-for-byte CI verification description, and GPL notice.
17. [EmuWiz PS3 boot evidence](../../crates/archivefs-core/src/ps3_boot_evidence.rs) — current bounded PS3 layout/SFO/SELF evidence.
18. [EmuWiz PS3 identity audit](SONY_PLAYSTATION_SUPPORT_AUDIT.md) and [executable-format audit](SONY_EXECUTABLE_FORMAT_AUDIT.md) — current PS3 identity scope and known boundaries.

This document is research only. No restore execution, resigning, downloads, patch
execution, production code, Save Vault implementation, GUI, or Publisher Profile
was added.
