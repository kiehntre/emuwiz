# Game recovery engine

When a game cannot start, the recovery engine looks at the game file itself and at
the emulators on the machine, says what is wrong with evidence, separates what can be
fixed from what cannot, and, where a transformation has been validated end to end,
prepares a verified playable copy without changing the original.

It lives in `archivefs_core::recovery`. The CLI (`emuwiz-cli recovery`) and Blue
(Game recovery page, the card on a game's page) are thin callers.

## What it does today

One complete slice: **an Amiga CD32 disc stored as a ZIP** (a cue sheet, a data
track and WAV audio tracks). No emulator opens that ZIP. Before this engine the
game's page said "Archive format not supported" and stopped.

1. **Diagnose** (read-only). The ZIP's index is read, the cue sheet is decoded and
   checked against the ZIP's own checksum, and every file the sheet names must be in
   the ZIP. The installed RetroArch PUAE core and the CD32 firmware files are looked
   for. Each finding carries its evidence, a confidence, what it blocks and the
   actions it proposes.
2. **Prepare** (new files only, after confirmation). The disc files are extracted
   into EmuWiz's prepared-media folder, checked against the ZIP's checksums, read
   back from disk and checked again, and published in one step.
3. **Play**. The game's Launch page plans the launch from the prepared copy. Every
   existing launch check still runs: the system is known only from the folder, so
   the page still asks for "Try anyway" or a remembered choice; RetroArch, the core,
   the firmware and the file are re-checked at click time.
4. **Undo**. Removing the prepared copy deletes only the files the engine created.

Everything else is reported honestly as unsupported or not assessed (see the
matrix). The engine never guesses a system, never downloads anything, and never
treats a folder name or a prepared copy as identity.

## Architecture

```
RecoverySubject (path, recorded system, how it was recorded)
        |
        v
  diagnose()  ---- reads ---->  capability matrix (reviewed rules)
        |                       disc_zip + cue_sheet (bounded inspection)
        |                       prepared_media store (receipts, quick check)
        |                       HostFacts (RetroArch cores, firmware files)
        v
RecoveryReport { verdict, findings[], plan?, prepared? }
        |
        +--> CLI text / JSON
        +--> Blue: Game recovery page, game-page card
        |
   (person confirms)
        v
PreparedMediaStore::prepare()  -> verified, create-new prepared copy + receipt
        |
        v
Launch readiness seam (GUI) + RetroArch preflight/spawn check (core)
```

| Module | Role |
|---|---|
| `recovery::model` | `Finding` (code, evidence, `Confidence`, `Blocking`, actions), `ProposedAction` (`ActionEffect`: read-only / creates new files / changes existing state; `ActionPerformer`: engine / person), `RecoveryReport`, `RecoveryVerdict`. |
| `recovery::capability` | The matrix below, as data. A test keeps this document's table equal to the code. |
| `recovery::cue_sheet` | Strict, bounded cue-sheet reader. Anything it does not understand is refused. |
| `recovery::disc_zip` | Read-only inspection of a ZIP holding one disc set; produces the content fingerprint. |
| `recovery::diagnose` | The rules. Pure reads; meant for a worker thread. |
| `recovery::prepared_media` | The store: plan, prepare, quick check, full verification, removal, interrupted-work clean-up, launch-path check. |

### Reused, not duplicated

| Need | Existing component used |
|---|---|
| Safe ZIP reading (central directory incl. ZIP64, CRC-checked decode, ratio and size limits, outer-file re-check) | `dat::archive::zip_preflight::preflight_zip`, `dat::archive::zip::{stream_zip_member_into, copy_zip_member_into}`, `ArchiveLimits` |
| Member-name rule | `dat::archive::member_path::check_member_name` |
| Symlink-free opens | `safe_read::{open_bounded_read, open_directory_no_symlinks, open_regular_file_nonblocking}` |
| No-replace publish | `dat::rename_apply::noclobber::rename_noreplace` (`renameat2(RENAME_NOREPLACE)`) |
| Streaming CRC-32 / SHA-1 / SHA-256 | `dat::archive::hash::hash_member_stream` |
| RetroArch discovery (cores, system folder) | `emulator_environment::retroarch::discover_retroarch_environment` |
| "Game not identified" launch with folder-only confirmation | `launch::unverified_media` and the GUI lane in `emulator_setup::readiness_input` (unchanged policy) |
| RetroArch route for CD32 | Already present: the PUAE core's own `.info` names "Commodore - CD32"; no platform mapping was added |
| Launch click-time checks | `launch::execution::preflight_retroarch_launch` / `spawn_retroarch_guarded` (one refusal added, nothing relaxed) |

Not reused, with reasons: `zip_cartridge_stage` (one cartridge ROM of at most 64 MiB,
into a temporary folder deleted at exit; a CD set is many files and hundreds of MiB,
so it is prepared once and kept); `zip_converter` (128 MiB cap, ext4 only);
`transform/` and `archive_workflow` extraction (dormant); `patch_output_recovery`
(holds the whole output in memory); the Repair rename journal (its operations move
or copy existing files; an extraction is neither, see "History" under limitations).

## Capability matrix

| System | Stored as | Contents | Detection evidence | Emulator route | Firmware | Safe preparation | Status | Limits |
|---|---|---|---|---|---|---|---|---|
| AmigaCD32 | `.zip` | cue sheet + data track + WAV audio tracks | ZIP index; cue sheet decoded and CRC-checked; every named track present | RetroArch PUAE core | kick40060.CD32 and kick40060.CD32.ext in RetroArch's system folder (your own files) | verified extraction of the set to new files | fixable by verified preparation | top-level files only; at most 128 files, 1 GiB per file, 2 GiB per set; deflate or stored |
| AmigaCD32 | `.zip` | a single .iso with no cue sheet | ZIP index | RetroArch PUAE core | as above | none yet | not assessed | extraction would be the same operation; launch of a bare ISO is not validated |
| AmigaCD32 | `.cue` | loose cue sheet with its tracks | catalogue row | RetroArch PUAE core | as above | not needed | playable directly | system known from the folder only: the Launch page asks before the first start |
| AmigaCD32 | `.chd` | compressed CD image | catalogue row | none proven | - | none | unsupported | CHD support in the installed Amiga emulators is not proven; no conversion is offered |
| Commodore CDTV | `.zip` | cue sheet + tracks | ZIP index | none validated | CDTV Kickstart and extended ROM | none yet | not assessed | same container as CD32, but the CDTV route has not been validated |
| Amiga | `.lha` | WHDLoad package | existing WHDLoad inspection | FS-UAE or Amiberry WHDLoad | Kickstart named by the game's slave | not needed | playable directly | needs the person's 7-Zip with LHA support |
| Amiga | `.hdf` | hard-disk image | existing HDF inspection | RetroArch PUAE core, from a private working copy | Kickstart for the chosen machine | not needed (existing working-copy feature) | playable directly | - |
| Amiga | `.7z` | any | extension | none | - | none | unsupported | EmuWiz has no bounded 7z member extractor; nothing is extracted |
| Xbox | `.iso` | XISO or full disc image | existing XDVDFS identity | xemu | MCPX boot ROM, flash BIOS, EEPROM, hard-disk image | not needed | playable directly | - |
| Xbox | `.zip` | a disc image (.iso) | ZIP index (existing Launch page message) | xemu, once extracted | as above | none yet | unsupported | a 7.3 GiB DVD image is above this engine's validated size; whether xemu starts a full disc image as extracted is not proven here |
| Xbox | `.7z` | any | extension | none | - | none | unsupported | no bounded 7z member extractor; building a disc image from loose files is not a validated transformation |
| Xbox | `.xbe` | loose executable or extracted game folder | existing XBE identity | none | - | none | unsupported | xemu starts disc images only; EmuWiz does not build one |
| Xbox360 | `.iso` | disc image | catalogue row | none (Xenia route accepts .xex only) | - | none | unsupported | no installed route starts an Xbox 360 disc image |
| PS3 | `.iso` | disc image, or an extracted PS3_GAME folder | existing PARAM.SFO identity | RPCS3 | PS3 system software installed in RPCS3 | not needed | playable directly | encrypted disc images are not handled |
| PS3 | `.pkg` | install package | package header (existing Launch page message) | none | - | none | unsupported | installing a package changes RPCS3's own state and needs a licence file; EmuWiz does neither |

"Fixable by verified preparation" is used only for a transformation that has been
validated end to end. No format conversion is offered anywhere: the one preparation
is a byte-exact extraction.

## Safety properties of preparation

| Property | How |
|---|---|
| Original untouched | The ZIP is opened read-only through the bounded reader; its device, inode, size, modification and change times are compared before and after. A change during preparation fails it. |
| Create-new only | Files are created with `O_EXCL\|O_NOFOLLOW` in a fresh private (0700) staging folder; the finished folder is published with one no-replace rename. An existing copy is reused if it verifies and refused if it does not; it is never overwritten. |
| Bounded | At most 128 files in the ZIP, 1 GiB per file, 2 GiB per set, compression ratio 1000:1, cue sheet 64 KiB, central directory 16 MiB. Free space must cover the set plus 1 GiB. |
| Path safety | Top-level members only; every name must pass the shared member-name rule and be one plain file name; links, special files, encrypted entries, duplicate names (ignoring case) and unknown compression methods are refused. The cue sheet may only name plain file names that are members of the same ZIP, with the same spelling. The store refuses to work if its own `sets`, `staging` or `sources` folder is a link. |
| Independent verification | (1) decoded size and CRC-32 against the ZIP's central directory, by the member reader; (2) each new file read back from disk and hashed again (CRC-32 and SHA-1 must match the decode; SHA-256 goes into the receipt); (3) the extracted cue sheet re-parsed and compared with the inspected one; (4) each WAVE file must be RIFF/WAVE; (5) the data track must carry an ISO 9660 primary volume descriptor. |
| Atomic and interruption-safe | A set is visible under `sets/` only complete, with its receipt. A killed preparation leaves a staging folder with an intent record; it is never used for launch, is reported, and is removed by `clean`. |
| Reversible | `remove` first moves the set out of sight, then deletes only the files the receipt lists that still have the recorded size. Anything else is left and reported. |
| Journal | `v1/journal.jsonl` records every preparation, failure, removal and clean-up. |
| Launch gate | Readiness uses a prepared copy only for the exact unchanged ZIP it was prepared from (one index record plus file metadata; the ZIP is not re-read). RetroArch preflight and spawn refuse any path inside the store that is not the cue sheet of a set whose files still have the recorded inode, size and times. |
| No network, no firmware | Nothing is downloaded. Firmware is only looked for and compared with known checksums; a missing file is the person's to add. |

Prepared files are made read-only (0444).

## Using it

```
emuwiz-cli recovery capabilities
emuwiz-cli recovery diagnose <game.zip> --system AmigaCD32 [--system-source folder_alias] [--json]
emuwiz-cli recovery prepare  <game.zip> --system AmigaCD32
emuwiz-cli recovery list [--json]
emuwiz-cli recovery verify <copy-id>
emuwiz-cli recovery remove <copy-id>
emuwiz-cli recovery clean
```

`--store <folder>` points every command at another prepared-media folder. The CLI
never opens the library database; the system must be given, because the engine never
guesses one.

In Blue: open a CD32 game stored as a ZIP. Its page shows a "Game recovery" card;
the page behind it lists the findings with their evidence and offers "Prepare a
playable copy" behind a confirmation tick. Afterwards the game's Launch page offers
the usual folder-only confirmation and then Play.

The store is `<data>/prepared-media` (`EMUWIZ_PREPARED_MEDIA_DIR` overrides it with
an absolute path). Do not point it inside a library source folder: a scan would then
list the prepared files as games.

## Extending it

A new format needs:

1. a row in `CAPABILITY_MATRIX` (start as `NotAssessed` or `Unsupported`);
2. an inspector that turns the file into evidence within fixed bounds;
3. a rule in `diagnose` producing findings from that evidence;
4. only after end-to-end validation, a preparation in the store and a
   `FixableByPreparation` row.

The model, the store's guarantees (staging, verification, receipt, publish, removal,
launch gate), the CLI and Blue's page are format-independent. `DiscSetInspection` is
the store's input today; a second preparation kind would introduce a small trait
with `members()`, `fingerprint()` and `verify_structure()` rather than a rewrite.

## Limitations

- One preparation only: CD32 cue + tracks in a ZIP, top-level members, deflate or stored.
- Not handled yet, and reported as such with the exact reason: a bare `.iso` in a
  ZIP; disc files inside a folder in the ZIP; a cue sheet that names a file the ZIP
  does not hold or spells it differently; a cue sheet that names the same file in
  two `FILE` lines; `MODE2/2336` tracks; MP3/FLAC audio tracks; 7z and RAR; CDTV; CHD.
- "Verified" means faithful to the ZIP: every prepared byte matches what the ZIP
  decodes to, and the decode matches the ZIP's own size and CRC-32. It is not a
  statement about which game it is, or that the dump is good. No catalogue (DAT)
  is consulted.
- The system stays a folder hint. A disc is prepared as CD32 only if its own ISO 9660
  system identifier is `CDTV`, which CDTV and CD32 discs both carry; that cannot tell
  the two apart, so the Launch page still asks for confirmation.
- A second ZIP with the same member names, sizes and CRC-32s reuses an existing copy
  only after it has been decoded in full and matches the copy's SHA-256.
- The quick check before launch compares inode, size and times, not contents. A full
  re-hash is `verify` / "Check the copy again". The receipt is not signed: anything
  running as the same user can write to the store.
- The launch gate (refusing files inside the store that are not a verified set's cue
  sheet) is on the RetroArch route, the route CD32 uses. Other emulator routes do
  not start prepared copies and do not have the check.
- History: prepared copies are listed, verified and removed on the Game recovery
  page and by the CLI, with their own journal. They do not appear in Blue's History
  page yet.
- While one process prepares a disc, another process's `list` shows that work as
  "unfinished"; `clean` and `remove` answer "busy" rather than waiting.
- Like every `emuwiz-cli` command, `recovery` announces itself in EmuWiz's per-user
  runtime registry (`/run/user/<uid>/emuwiz`) while it runs and removes its record on
  exit. That location is fixed by design and is not redirected by a scratch profile.
- Preparation goes through the shared ZIP member reader once per file, and that
  reader re-scans the ZIP each time; a 219 MB, 11-file disc takes about 7 s in an
  optimised build. Diagnosis reads only the ZIP's index and the cue sheet.
- Prepared copies use disk space (the size of the disc). There is no automatic
  clean-up; `list` shows them.
- Saves and RetroArch per-game settings key on the prepared cue sheet's name.

## Evidence (2026-11-20, branch `feat/phoenix-recovery-engine-20261120`)

Synthetic tests (run locally through `scripts/cargo-iso`):

- `archivefs-core`: `recovery::` 68 tests (cue parser, capability matrix, the slice
  end to end, adversarial archives, interruption at five points, cancellation,
  concurrency, tampering, rollback, the review regressions).
- `archivefs-cli`: `recovery::` 3 tests (help never runs a command, a system is never
  guessed, headless diagnose → prepare → verify → remove).
- `archivefs-gui`: `gui_v2::retroarch_journey_tests::prepared_disc`: Blue, in a child
  process with a disposable profile and a recording fake RetroArch: game page →
  Game recovery → confirm → prepare → Launch page → "Try once as Amiga CD32" → Play;
  the emulator is handed the prepared cue sheet, never the ZIP; ZIP and library
  database byte-identical afterwards.

Real data, on a disposable profile (originals only read):

- 79 real CD32 ZIPs diagnosed: 70 fixable by preparation, 9 refused with a specific
  reason (3 cue sheets naming a file twice, 2 with `MODE2/2336`, 3 naming files the
  ZIP does not hold, 1 spelling difference).
- All 70 prepared into a scratch store: 446 files, 12,977,606,752 bytes. Every file
  re-decoded from its ZIP with an independent implementation (Python `zipfile`) and
  compared with the prepared file and the receipt: 446 of 446 match.
- The 79 ZIPs before and after: size, mtime, ctime, inode and SHA-256 identical.
- Two prepared copies started in the installed RetroArch (PUAE 5.3.1) on a private
  display with the controller hidden: Akira reached its menu; Alfred Chicken reached
  its title screen and the log shows CD audio playing from an extracted WAV track.

Not verified: clicking Play in Blue against the real RetroArch in one run (Blue was
driven against the recording fake; the real emulator was started by hand with the
argument list Blue plans), audio output (the sandbox has no sound device), gameplay
beyond the first screens, and the other 68 prepared games.

Performance, on a copy of the 163,332-game library:

- Per game, on any thread: the applicability check is two string comparisons, about
  310 ns (51 ms for all 163,332 rows). It is the only recovery code a game page runs
  for a game the engine has no preparation for. Game cards run none.
- Readiness for a candidate (a CD32 ZIP): one index record, the ZIP's metadata and
  the set's `stat` calls, about 80 µs, remembered for 750 ms. The ZIP is not read.
- Diagnosis and preparation run on a worker thread only.

## Roadmap

1. CD32, from the nine real refusals: cue sheets that name one file in two `FILE`
   lines (3 games) and `MODE2/2336` tracks (2 games) are parser extensions; cue sheets
   that name a missing or differently spelled file (4 games) need a decision on
   writing a corrected sheet. Then a bare ISO in a ZIP, and MP3/FLAC tracks if the
   core is shown to play them.
2. CDTV through the same container once the CDTV route is validated.
3. Other CD systems whose games arrive as ZIPs of cue + tracks (PC Engine CD, Sega
   CD, Saturn, PlayStation): same inspector and store, one capability row and one
   launch validation each.
4. Xbox: ZIP holding a disc image. Needs the size class raised (DVD), and proof of
   what xemu starts as extracted.
5. Blue History integration and a storage view of prepared copies.
6. Core-side diagnosis for the Launch page's existing "known unplayable" messages,
   so the page and the engine share one rule set.
7. 7z/RAR only if a bounded member extractor is added to the archive readers.
