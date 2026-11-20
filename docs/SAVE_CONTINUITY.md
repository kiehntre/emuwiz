# Save continuity

Find the saves on this machine, say which game each belongs to and how that is
known, keep content-verified snapshots, tie them to play sessions, and plan a
safe way to continue on another emulator or device.

It lives in `archivefs_core::save_continuity`. The CLI (`emuwiz-cli continuity`)
is a thin caller. This is a first slice: the limits are listed at the end and are
part of the design, not an afterthought.

Three rules hold everywhere:

1. **The original save is only ever read.** Snapshots are new objects in EmuWiz's
   own folder, a recovery writes a new folder, a continuation copy is a new file.
   No function in the module can overwrite or delete a save.
2. **A claim is only as strong as its evidence.** A file name is never proof of
   which game a save belongs to. Newer or larger never means "more progress". A
   snapshot that was copied safely is not thereby a good save.
3. **Nothing is deleted, restored or synchronised automatically.**

## What existed before, and what this adds

| Area | Already in EmuWiz | Gap this slice closes | Still open |
|---|---|---|---|
| PS1/PS2 memory cards | `memory_card_manager` (#521): content-based format detection, two readers, export/import of single saves into a copy | Reused as is. Added only a read-only per-save fingerprint | Folder cards are listed, not snapshotted |
| Save inventory | `persistent_state_inventory` (`saves` CLI): by file extension, no `.srm`/`.gci` typing, ids not stable | Typed discovery record with identity evidence, stable ids, in-use state | Standalone emulators' cartridge saves |
| GameCube / PS3 / PSP saves | Not parsed | `.gci` header and `PARAM.SFO` read; identity from content | Wii NAND, raw GC cards, PS3 signature (`PARAM.PFD`) |
| Snapshots | `save_snapshots`: full copy per snapshot, restore only onto the original path, no pruning | Content-addressed store, deduplication, recovery into a *new* folder, pruning proposals, storage limit | The two stores are separate; merging them is future work |
| "Is the emulator closed?" | `ProcScanQuiescence` + kernel lease, PCSX2 and RetroArch only | Identities for DuckStation, RPCS3, PPSSPP (real launches checked) and Dolphin (synthetic only, kept off) | Dolphin real launch |
| Save compatibility | Planner types with caller-asserted conversions; no converter | Five-level verdict; one implemented, checked conversion (card save onto a copy of another card) | Any other conversion |
| Save states | `save_state_compatibility`: no producer of "Supported" | Stated plainly as not portable | Per-core format/version inspection |
| Sessions | None persisted. Recently Played reads RetroArch's history only; `daedalus` records failed launches | Session journal: requested / started / ended, process identity, descendants, crash and unseen-exit handling | **Not yet called from Blue's launch path** |
| Game identity | Verified facts per game file | `match_game_file`: save's product code against the identifier EmuWiz verifies inside the game file | Library lookup by serial (no index, facts table empty in the checked library copy) |
| Devices | None | Offline model with signed lists, history-based ordering, resumable transfer | Any real transport |
| Database | Schema 30 | **No schema change.** State is JSON under `<data>/save-continuity`, like other features outside the library database | See "Schema proposal" |

## Discovery

`discover::discover` looks only in known emulator save folders, opens every file
read-only without following links, is bounded (file count, bytes), cancellable
between files, and reuses earlier results for files whose size, modification
time, change time, device and inode are unchanged.

| Save | Tie to a game | Strength |
|---|---|---|
| PS1 / PS2 card save | product code in the card's own entry | content-proven |
| GameCube `.gci` | game code in the file header | content-proven |
| PS3 / PSP save folder | `PARAM.SFO` inside the folder | content-proven |
| Cartridge battery save | nothing in the bytes. "Named by the emulator" only when RetroArch's own history lists exactly one game file of that name | emulator convention, or name only |
| Save state | as a battery save | name only; never portable |

`compat::match_game_file` goes one step further for a chosen game file: it
compares the save's product code with the identifier EmuWiz's identity inspection
*verifies inside the game file* (for a PS2 disc, the boot file named by
`SYSTEM.CNF`). File names play no part.

## Snapshot status: three separate answers

A snapshot records these separately. There is no combined "verified".

| Field | Question | Values |
|---|---|---|
| `boundary` (capture safety) | Was the copy taken while nothing could be writing? | emulator closed and observed · not confirmed (best-effort) · received from another device |
| `structure` | Does the file have the shape its format requires? | passed (says exactly what was walked) · problems · not checked |
| `game_integrity` | Is the data valid as the game sees it? | **not checked**, always. No game checksum is verified today, and there is deliberately no "verified" value |
| `preceding_session` | How did the emulator end, the last time it could have written this save? | clean exit · crashed · forced stop · error exit · exit not seen · unknown |

What the structural check covers: memory cards (directory and allocation walked by
two independent readers that must agree); GameCube `.gci` (file length against the
header only). PS3, PSP, cartridge saves and save states get no structural check,
and the record says so.

### Capture safety (the "safe point")

A snapshot is recorded as taken at a safe point only when, **before and after the
copy**:

- no process of that emulator exists in any form: native binary, AppImage (both
  the mounted and the extracted layout), Flatpak app id or app scope, a launcher
  or wrapper naming it; and
- no program has the save file open. With a kernel lease this is answered for
  every process on the machine, including ones that cannot be inspected, and any
  program opening the file for writing during the copy breaks the lease and the
  copy is thrown away. Without a lease, one uninspectable process makes the
  answer "unknown".

"Active" and "unknown" are both refusals. A best-effort copy is possible only when
asked for explicitly while the answer is "unknown"; it is stored as
`stable_read_only`, labelled UNVERIFIED, and is never treated as the same snapshot
as a verified one even when the bytes are identical. A running emulator is refused
even then.

Reading the file twice is *not* what makes a copy safe: a writer that pauses
half-way leaves a file that reads the same twice. A test caught this; the lease is
the guarantee.

### Crash awareness

A safe copy of a file that a crash left half-written is still half-written. So
each snapshot records how the emulator last ended, **only** when a recorded
session can be tied to that exact save:

- it is the most recent started session of the same emulator and install on this
  device;
- it recorded this save, by id;
- where the saves were compared after the session, the save still has the content
  it had then;
- a normal exit counts only if the saves were compared afterwards.

Anything else is "unknown", shown as a caution. Crash, forced stop, error exit and
unseen exit are shown as a warning on the snapshot, in the timeline and in every
recovery plan. Such snapshots are kept and recoverable. They never replace or hide
an earlier one, and pruning never proposes the newest snapshot that was taken
safely after a clean exit.

**Where crash detection cannot be trusted**

- An emulator started outside EmuWiz leaves no session: the answer is "unknown",
  never "clean".
- Through `flatpak run` or a wrapper script EmuWiz sees the launcher's exit, not
  the emulator's: recorded as "exit not seen".
- If EmuWiz is closed when the emulator stops: "exit not seen".
- A game can corrupt its own save and exit cleanly. Only a game-level check would
  catch that, and there is none.
- An emulator can exit cleanly having failed to write (full disk). Not detected.

## Emulator evidence matrix

Real-launch results come from `scripts/qa/emulator_safe_point_probe.py`, which
starts the installed emulator in an isolated scratch profile and checks the
verdict before, during and after. See the ledger for the run that produced them.

| Emulator | Discovery | Identity from content | Safe-point rules | Real launch checked | Verified snapshot | Recovery to new folder | Continue elsewhere |
|---|---|---|---|---|---|---|---|
| PCSX2 | cards, per save | yes (serial) | reviewed (existing) | see ledger | yes | yes | card save onto a copy of another card: implemented and checked |
| RetroArch | battery saves, states, PS1 cards | cards only | reviewed (existing) | see ledger | yes | yes | conditional (depends on core); states never |
| DuckStation | PS1 cards | yes (serial) | added | see ledger | yes | yes | card route as PCSX2 |
| RPCS3 | save folders | yes (`PARAM.SFO`) | added | see ledger | yes (files copied; signature not checked) | yes | unknown: signature not verified |
| PPSSPP | save folders | yes (`PARAM.SFO`) | added | see ledger | yes (as RPCS3) | yes | unknown |
| Dolphin (standalone) | `.gci` files | yes (game code) | added, **synthetic tests only** | **NOT TESTED: not installed** | **no: always "unknown"** | best-effort copies only, labelled unverified | conditional |
| Anything else | not looked for | - | none | - | no | - | - |

## Compatibility

| Verdict | Meaning | Given when |
|---|---|---|
| Verified | The destination reads this exact save where it is | Same emulator and install |
| After a checked conversion | EmuWiz performs the step and checks the result | PS1/PS2 card save to another emulator that uses the same card format |
| Conditional | Format matches; listed conditions are not checked | GCI between Dolphin installs; battery saves between RetroArch cores |
| Incompatible | Cannot work | Different release (product code differs); save states across emulators; wrong console |
| Unknown | No evidence | PS3/PSP between installs (signature); standalone emulators' battery saves |

The one implemented conversion (`compat::prepare_card_continuation`) exports the
save and imports it into a **copy** of the destination card in a new folder. It is
kept only if: the new card reads cleanly with both readers; the save's fingerprint
equals the source's; every save already on the destination is unchanged; exactly
one save was added; both originals have the same SHA-256 as before. If the
destination already holds a save of that name, nothing is written and both are
reported for a decision.

## Sessions

One JSON record per launch under `<root>/sessions/`, three distinct steps:
requested, started (process id, start time, executable, descendants, Flatpak app
scope), ended. A recycled process id is never mistaken for the session. A launcher
finishing does not end a session while a process it started is alive. Records left
open are settled on the next look as "never started" or "end not seen", never as
clean. "Running" is not reported as "played": the record says how long the process
ran and whether a save's content changed.

## Two devices (offline model only)

**There is no network code.** `device` defines what two devices would exchange and
tests every rule against two local folders.

- A device is an Ed25519 key; its id is a hash of the public key. Lists are
  signed. Nothing is read from a device a person has not paired.
- Order comes from each save's history of contents, never from clocks.
- Two different continuations, or two unrelated histories, are a conflict for a
  person. There is no "newest wins".
- A list older than one already accepted is refused; a transfer is refused if the
  sender has moved on since its list was read.
- Content arrives into a work file, resumes after an interruption, and enters the
  store only once its SHA-256 matches. The record goes in last.
- A received snapshot is stored under the sender's name. Using it is a separate,
  confirmed step.

A real transport (LAN discovery, an authenticated encrypted channel carrying
exactly these messages, pairing by comparing the code on both screens) is future
work and needs separate approval.

## Schema proposal (not applied)

No migration is included. Everything is files under `<data>/save-continuity`.
If library integration is wanted later, the proposal is one additive table,
`save_continuity_links(archive_id, save_key, evidence_kind, evidence_value,
observed_at)`, plus an index on `verified_identity_facts(kind, value)` so a save's
product code can be looked up without a scan. Both need a schema number from the
coordination ledger and an approved upgrade; neither is needed for anything this
slice does.

## Limits of this slice

- Session tracking is not yet called from Blue's launch path, so real launches
  produce no session and "how the emulator last ended" reads "unknown".
- No Blue page yet.
- Emulator and core versions are not established for any save.
- Standalone Dolphin snapshots are unverified by design until checked for real.
- No game-level save validation of any kind.
- A lease needs a local filesystem (ext4, xfs, btrfs, f2fs, tmpfs) and a file
  with a single name; otherwise the answer degrades to "unknown".
- The process scan refuses on any process whose command line names the emulator
  (for RetroArch also "libretro"), so an unrelated command mentioning it can block
  a snapshot. That errs on the safe side.
- The existing `save_snapshots` feature still limits itself to PCSX2 and
  RetroArch; it was not changed.
