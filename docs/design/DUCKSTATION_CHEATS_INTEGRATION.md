# DuckStation cheats in Cheats & Mods

Recovered from `integration/resolver-duckstation-cheats-20261007` (2c3247a5).
Only the cheats part was taken. That branch also rewired how Launch, Doctor,
Setup and PPSSPP choose an installation; main's resolver
(`emulator_setup/profile_resolution`) stays the single authority.

## Which installation

`cheats_mods/duckstation_environment.rs::select_for_cheats` runs the same
resolution and the same launch authority (`select_for_launch_from_view`) as
Launch. Cheats are written only into the executable + profile pair Launch is
authorised to start: a PREFERRED fallback is the same fallback, announced the
same way; a FORCED pin that cannot be used, an ambiguous choice or nothing
usable leaves cheats unavailable. There is no second selection store.

## Identity

* A verified PS1 serial is required (filenames and titles never count).
* Single-disc proof (below); multi-disc sets are refused.
* A catalogue cheat file whose region text names a different region from the
  serial prefix (e.g. a USA file for an `SLES-` disc) is refused before
  planning. When either side names no recognisable region, the region check has
  no opinion.

## What "enabled" means

DuckStation turns a cheat on only through the per-game settings file
(`[Cheats] EnableCheats = true` plus `Enable = <name>`). The preview lists each
cheat as "switched on in this game's settings" or "added, left off". After
Apply both files are read back and verified. EmuWiz does not observe the game
running, and the result says so: it never claims the cheat is active in game.

## Emulator versions

The adapter writes DuckStation's sectioned cheat format and per-game `Enable`
lines, verified against upstream source and a 0.1-11894 dev build (see
`docs/research/DUCKSTATION_CHEAT_RUNTIME_FEASIBILITY.md`). Discovery never runs
the program, so the version is usually unknown and is not gated. The game
database disc proof accepts only the cache layout it was verified against
(version 33).

## Workflow

* One preview, one Apply, one grouped History row, one Undo. Two files
  (`SERIAL.cht`, `GameSettings/SERIAL.ini`) change atomically.
* Any number of ticked cheats install as one `AddMany` operation.
* Conflicts (duplicate names, incompatible duplicates, an existing cheat of the
  same name with a different body, unsupported opcodes, a conflicting INI enable
  state) are all reported before anything is written. Database-shadowing needs an
  explicit acknowledgement.
* History groups the two child journals under the adapter receipt
  (`patch_manager::group_shared_history`); a child whose receipt is missing is
  shown but never offered for individual undo. Undo calls
  `undo_duckstation_native` (INI first).

### Single-disc proof

Proof is a hierarchy of `DiscCountAuthority` providers; none is mandatory:

1. the library's trusted DAT media sets (a proven one-disc set);
2. the resolved DuckStation installation's own game database
   (`cache/gamedb.cache`): the serial is listed as a game and is in no
   multi-disc set - serial-keyed, so it works for discs that never matched a DAT;
3. further providers implement the trait.

Any authority naming a multi-disc set wins; a single-disc claim contradicted by a
set is a conflict and is refused. Filenames, "(Disc N)" text, fuzzy metadata and
the mere absence of a record are never evidence. A refusal names each route that
was tried and why it did not establish single-disc.

The cache reader accepts only the layout it was verified against (version 33) and
validates structure; anything else makes the authority silent with a stated
reason, never wrong.

## Remaining refusals

Multi-disc sets, hash-specific cheat files, conditional/non-direct-write codes,
games with no verifiable PS1 serial (translations, prototypes, homebrew), a
profile EmuWiz cannot launch from, and any FORCED selection that is unusable.

## Measured on the real library (read-only)

Without any DAT match (the library has none for these discs), the 291 PS1 CHDs in
`/mnt/usbdrive/games/psx` resolve as: **252 single-disc proven** by the portable
DuckStation install's own game database, **25 multi-disc** (refused, correctly),
**14 with no verifiable PS1 serial** (translations, prototypes and a few discs the
identity inspector cannot read a serial from), **0** with a serial but no proof.
Run it again with
`EMUWIZ_PROBE_LIMIT=1000 scripts/cargo-iso test -p archivefs-core --lib real_library_disc_proof_report -- --ignored --nocapture`.

## Undo and History with the current GUI-v2

GUI-v2 History (`gui_v2/history_view.rs`) shows rename receipts only; shared cheat
journals are not in its model. DuckStation recovery is therefore reachable from
every place a person looks, with ONE implementation underneath
(`undo_duckstation_native`, settings first):

| Where | What it offers |
|---|---|
| Cheats & Mods, result card | "Undo this change": preview, confirm, run |
| Cheats & Mods, "Earlier DuckStation changes for this game" | the same preview/confirm for any recorded change, rebuilt from disk (so it survives a restart); "Finish undo" when an earlier undo stopped partway |
| GUI-v2 History (`gui_v2/history_duckstation.rs`) | one row per operation (cheats, real apply time, outcome, undo status) with **Open in Cheats to undo / review**, a deep link to that game's Cheats tab |
| Legacy History page | the same grouped row and Undo |
| Mods page | shows neither child journal: `ModHistory` has never listed cheat adapters (tested) |

Safety: the undo preview is recomputed when Confirm is pressed, so a file changed
after the preview blocks the run; a changed output is never overwritten; an
orphaned child (receipt missing or damaged) is review-only; a failed apply that
rolled itself back is shown as a failure with nothing to undo.

Partial undo is recoverable: the settings file is restored first, so an
interruption leaves an inert cheat file, never settings enabling a missing cheat.
History then reports `PartiallyUndone` (which file is back, which is not), and
"Finish undo" resumes with only the remainder, re-verifying both files at the end.

## Disc authority assumptions (reviewed)

The game-database claim needs (1) the serial listed as a game and (2) the serial in
no disc set. The reader refuses a database too small to be the real one
(`MIN_DISC_SETS`, `MIN_LISTED_GAMES`), a different layout version, or anything
structurally off. The real database was cross-checked against the Redump
PlayStation DAT (the database has more sets than the DAT has multi-disc groups)
and against the real library (discs named like multi-disc releases are never
proven single-disc).

## Tests cannot reach real application data

`duckstation_environment::{shared_history_roots, duckstation_inputs}` return
nothing in test builds unless the test installs its own temporary machine.
