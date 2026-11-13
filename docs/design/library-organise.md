# Organise my games — mixed-library identification and organisation

Status: implementation plan (2026-11-08). Owner requirement: SELECT FOLDER →
IDENTIFY GAMES → PREVIEW ORGANISATION → APPROVE → ORGANISE → VERIFY, for every
platform, as the normal import and maintenance workflow.

## Why 4,023 NES games sat in `snes/` unflagged (verified in a2fffe3c)

1. **Folder beats bytes at scan.** `detect_platform_with_details_and_problems`
   (`lib.rs:7317-7335`) returns the folder alias before any content detector
   runs; `platform/detect.rs:525` — "the folder wins". DB precedence
   (`database.rs:10438-10579`, `provenance_priority` 3993) keeps it.
   `archive_platform_members` only knows ZX/Amiga/CPC members, so nothing
   reads a `.unh` member.
2. **No content evidence for headerless NES during scan.** `nes_header_evidence`
   requires `NES\x1A`; `platform_evidence_fusion` runs only on the Selected
   Evidence page.
3. **DAT checks only compare a file with its *assigned* platform's DATs.**
   `library_verification.rs:438-455`, `dat_sources_page.rs:8379-8413`
   (`audit_targets_for`), `dat/identity.rs:407-421`, `dat/coverage.rs:19-27`.
   The NES Headerless DAT never saw SNES-assigned rows; the SNES DAT could only
   say "not in SNES" — and no DAT audit is automatic, so the SNES folder was
   never audited at all ("not checked", shown as nothing).
4. **The one cross-platform detector is a dead end.** Smart ROM Folder Check
   (`folder_platform_check.rs`) does compare against every cartridge catalogue,
   but it is manual, capped at 1,000 per click, rehashes every file, keeps
   results in memory only, rates headerless NES as Probable at best, and has no
   action ("Nothing is assigned, moved or saved").
5. **Organise & Export trusts the wrong answer.** `dat/rom_organisation`
   (plan → approve → apply over `rename_apply`) treats a folder-alias
   assignment as a resolved platform (`database.rs:5616` → `Inference`), so it
   would "organise" these games straight back into SNES.
6. **They could not launch either.** `.unh` was not an accepted NES ZIP member
   (`game_identity.rs:1559`), so staging refused them under any platform.

## Classification (one shared model, every platform)

Per game, stored with the file identity it was computed for:

| State | Meaning | Moves? |
|---|---|---|
| Verified identity | member SHA-1 in a DAT for system X (any system, not only the assigned one) | yes, to X |
| Platform confirmed, identity unverified | a calibrated validator for X passes (header with checksum, or structural rule with measured false-positive rate 0) and nothing contradicts | yes, to X |
| Ambiguous | evidence for two systems, or evidence contradicting a DAT hit | never; shown with the conflicting lines |
| Unknown platform | checked; no validator or DAT matched | only to `Unnamed/Unknown/` on an approved run |
| Not checked / missing reference data | never examined, or the relevant DAT is not installed | never; the next step is shown |

No DAT match never means "unknown platform" and never means "invalid".
Manual choices are stored as `manual` assignments (existing, highest priority)
and kept separate from evidence; later evidence that contradicts a manual
choice is shown as a conflict and nothing changes without approval.

## Shared architecture changes (one PR)

1. **Evidence engine** — `folder_platform_check` becomes the library-wide
   platform check: all platforms' header validators + structural rules
   (headerless NES gains the calibrated PPU/OAM/JOY register-store rule:
   2,628/2,640 DAT-confirmed NES pass, 0 of ~1,700 6502-family controls) +
   SHA-1 lookup in every installed catalogue. Results persist in a new
   `platform_check_results` table keyed by archive id + size + mtime +
   evidence version + catalogue generation, so rechecks only re-read files
   whose identity or reference data changed. No run cap; batches with
   cancel and progress; read-only.
2. **Destination resolution** — sibling folder in the same source root
   matching the target platform's configured alias (built-in or custom); if
   none exists, the canonical registry folder name is created there. Unknown →
   `Unnamed/Unknown/` in the same source root, with provenance (original
   relative path, evidence, time) recorded in the journal and on the row.
3. **Organiser** — `rom_organisation` gets a "correct platform" input: the
   plan uses the check's detected platform instead of the folder alias, only
   for Verified/Platform-confirmed (and Unknown on approval). Moves reuse
   `rename_apply` (no-clobber renameat2, durable journal, rollback); package
   companions (`.cue/.bin`, `.m3u` sets, multi-disc, artwork sidecars) move
   as one unit or not at all; collisions are reported, never resolved.
   After commit: `organisation_paths` updates paths in place (archive id kept,
   so favourites/history/saves keyed by id stay), and the platform assignment
   becomes `dat_verified` / `content_verified` with evidence.
4. **External consumers** — RomM: one targeted quick scan of the affected
   platforms via its job queue, refused if a scan is already running; never
   a direct DB edit. RetroArch: `.lpl` JSON playlists/history rewritten for
   moved paths only, after a backup; saves are keyed by file name, which is
   unchanged. Other formats (gamelist.xml, Pegasus metadata) are reported,
   not rewritten.
5. **Blue** — Manage Library → **Organise my games**: pick folder → identify
   (progress) → preview grouped by result (will move / already correct /
   unknown / ambiguous / collision) with plain reasons → one approval →
   organise → verify summary (organised, already correct, unknown, ambiguous,
   collision, failed) with Undo. **Unknown games** view: Recheck, Choose
   system, Move to correct folder, Leave here. Optional auto-organise policy
   for future confirmed identifications, off by default, never enabled
   silently.
6. **Recheck triggers** — catalogue install/update, evidence-version bump,
   user Recheck; only affected rows re-run.

7. **Manual system choices survive rebuilds** — a manual choice is stored as
   today (`manual` assignment) *and* mirrored to
   `<data_dir>/manual-platform-choices.json`, keyed by content SHA-1 plus size
   (path as a hint only). A rebuilt or rescanned library re-applies a choice
   only when the content matches exactly. Verified evidence that disagrees
   later is shown as a conflict; nothing changes without approval.
8. **Game packages move complete** (`game_package.rs`,
   `folder_platform_check/organise/folder_move.rs`).
   - *Boundary first, never "every parent folder is one game".* A file set is
     a `.cue`/`.gdi`/`.m3u` plus exactly the files it names (an `.m3u`'s
     `.cue`/`.gdi` discs bring their tracks), all in the descriptor's own
     folder. A game folder is a folder whose validated layout says it is one
     game (PS3: `PS3_GAME/PARAM.SFO` parses with a `TITLE_ID` and
     `PS3_GAME/USRDIR/EBOOT.BIN` exists) or a folder the person picked and
     gave a system. No disc image is hashed to recognise a package.
   - *Refused, source untouched, reason shown:* a descriptor naming a
     missing, unsafe or sub-folder file; a file another descriptor also
     uses; a track or disc chosen on its own (choose its set); a folder that
     holds other library games, is a system/library folder, contains links,
     special files or a mount point, holds a second game folder, or would
     land on another filesystem; any existing destination (also by case).
   - *File sets* move in one organiser transaction: the lead plus members
     marked `organisation_companion_v1` (a member with its own library row
     keeps its catalogue binding). Approving a set approves exactly its
     files; a failure part-way rolls the whole transaction back. A picked
     game the library has no row for (for example a `.cue` outside a
     PlayStation folder, which the scanner never catalogues) is marked
     `organisation_uncatalogued_v1` and moves with no catalogue change.
   - *Game folders* move by one `renameat2(RENAME_NOREPLACE)` on the same
     filesystem (no copy fallback). Journal of intent first
     (`<journal>/folder-moves/`), created system folders recorded after
     creation, library path changes staged in an open SQLite transaction,
     the rename, the folder layout (paths, sizes, times, inodes) re-checked
     at the destination, then commit; any failure puts the folder back.
     Undo reverses it and refuses if the folder changed; an interrupted move
     is settled from disk + library state before the next run.
   - Blue: game folders are found automatically; "Add a game folder…",
     "Add a disc set or file…" (or a typed path) with a filterable system
     list add others. The preview lists each folder/set and why.
   - RetroArch: playlist entries inside a moved folder follow it.

9. **Proof before production** — a disposable fixture library drives the
   whole import → identify → preview → approve → move → verify → undo journey
   in the real Blue interface before any further production moves. Production
   moves beyond the already-approved snes→nes batch need a verified manifest
   and the owner's approval.

RomM: EmuWiz never edits RomM's database. After a move it reports which RomM
platforms need a quick scan; when RomM's own job queue is reachable (local
docker) it can queue that targeted scan, refusing while another scan runs.

## Out of scope here / follow-ups

Cross-filesystem moves (refused: folders and files are only ever renamed);
sets naming files in sub-folders; game-folder recognition beyond PS3 layouts
(other folder games are picked and given a system); History page listing of
folder moves (Undo is on the Organise page and the journal is durable);
gamelist.xml/Pegasus rewriting by EmuWiz; SNES/NES multicart identity.
