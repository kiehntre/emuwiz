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
| Sessions | None persisted. Recently Played reads RetroArch's history only; `daedalus` records failed launches | Session journal: requested / started / ended, process identity, descendants, abnormal-end and unseen-exit handling | **Not yet called from Blue's launch path** |
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
| `preceding_session` | How did the emulator end, the last time it could have written this save? | clean exit · fatal signal (observed directly) · fatal termination recorded (by the system's crash handler) · forced stop · error exit · exit not seen · unknown |

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

### How the emulator last ended

A safe copy of a file that an abnormal end left half-written is still
half-written. So each snapshot records how the emulator last ended, **only**
when a recorded session can be tied to that exact save:

- it is the most recent started session of the same emulator and install on
  this device;
- it recorded this save, by id;
- where the saves were compared after the session, the save still has the
  content it had then;
- a normal exit counts only if the saves were compared afterwards.

Anything else is "not known".

What counts as evidence, strongest first. Each is a separate source.

| Evidence | What it establishes | Shown as |
|---|---|---|
| The kernel reports a terminating signal for the emulator's own process (EmuWiz started it and waited on it) | That process was ended by that signal | Prominent: "Abnormal termination detected" |
| The system's crash handler recorded a fatal termination for exactly the emulator process identified for this launch (see below) | That process was ended by a fatal signal; which one is not recorded | Prominent: "Abnormal termination detected" |
| A non-zero exit code from an emulator on the demonstrated list | It did not close normally | Prominent: "Abnormal exit detected" |
| Exit status 0 from the emulator's own process, saves compared afterwards | It closed normally | Nothing |
| A non-zero exit code from any other emulator | Nothing that can be interpreted | Quiet: "not known whether that was a normal close" |
| Only a launcher's or wrapper's end was seen; EmuWiz was closed; no session | Nothing | Quiet: "not known how the emulator last ended" |

Rules that hold throughout:

- **An exit code is never turned into a signal.** 139 or 134 coming back
  through a sandbox or wrapper stays an exit status.
- **No wording claims a crash, or that a save is damaged.** The text says what
  was observed and that the save may be incomplete if the emulator was writing.
- **A launcher's own end is never the emulator's**, clean or otherwise. A
  relayed status is considered only when the process EmuWiz started is itself
  the Flatpak sandbox and the emulator is its direct command.
- Capture safety and structure are decided separately and do not depend on any
  of this.

#### The demonstrated list

An emulator's exit code is believed only after, on a real install and **from
inside a running game**, ordinary ways of closing returned 0 every time and a
deliberate abnormal end returned something else. What has been observed
(`scripts/qa/*_exit_probe.py`; results in the audit ledger):

| Emulator | From inside a running game | Status |
|---|---|---|
| RetroArch, native 1.21.0 and Flatpak 1.22.2 | 18 ordinary endings per route (quit command, quit key, window close, self-exit, polite stop, quit with an unwritable save state): 0 every time. 3 deliberate faults per route: 139 every time. Nothing hung | Meets the standard for in-game exits. One earlier polite stop **at the menu with no game loaded** returned 139 and is unexplained: menu-only and wrapper paths are not covered |
| PPSSPP, Flatpak 1.20.4 | 9 ordinary endings (window close, in-app exit, polite stop): 0 every time. 3 deliberate faults: the process **hung**, and was only ended by the probe 40 s later | Not demonstrated: held as "not known" |
| PCSX2, Flatpak 2.8.2 | Not yet closed from inside a game. At its first-run window a close returned 1 | Not demonstrated |
| RPCS3, Flatpak | Not yet closed from inside a game. A window close returned 134 and the crash handler confirms it was ended by a fatal signal while closing; after a deliberate fault it kept running | Not demonstrated |
| DuckStation, Dolphin, others | Not probed | Not demonstrated |

#### Crash records for sandboxed emulators

`crash_evidence` reads Ubuntu's crash handler log (`/var/log/apport.log`),
which, for a process inside a container or sandbox that was ended by a
core-dumping signal, may record its **host process id**. Not every such
process is certain to get a line. It only reads; it needs no privilege
beyond the user's own (group `adm`); nothing about Flatpak or an emulator
changes.

A record counts only when all of this holds:

- its host process id is the one tracked for the emulator of this launch;
- its time is inside that process's tracked lifetime (a few seconds' slack);
- the process was last seen alive no more than ten minutes before it was
  found gone. While EmuWiz is running it looks regularly and keeps that time
  current (written at least once a minute), so this only bites when nobody
  was looking;
- where the system can say, fewer than half of all process ids were issued
  since then (so the id cannot have come round to another process).

Everything unclear is "unavailable", which is treated as not known and can
never fail a launch or a snapshot: another crash handler (most other
distributions; systemd-coredump is not supported yet), an unreadable or
missing log, a log that does not reach back to the process's start, an entry
for that process that cannot be read, two candidate entries, a record just
outside the lifetime (within two minutes: timing unclear), a time in the hour
that repeats when the clocks go back, a time zone set for EmuWiz alone (`TZ`),
an entry inside another program's quoted command line, or a kernel that
does not wait for its handler (`core_pipe_limit` 0 or unreadable: "no record"
then establishes nothing).

What a record cannot say: which signal, why, or anything about the game or
its save. What its absence cannot say: that the emulator closed normally. A
process ended by SIGKILL or SIGTERM, or one that handles the fault itself and
carries on (RPCS3 and, in a game, PPSSPP did), leaves no record at that time.

Measured on the development machine: the record's time is when the handler
wrote its line, 0.15 to 0.25 s after the signal and about 35 ms before the
parent saw the process gone. That ordering follows from `core_pipe_limit`
being non-zero there; it is not assumed elsewhere.

### Keeping recovery points

Two separate questions about a snapshot:

- **Is the copy sound?** Taken at a verified safe point and with no structural
  problem found (`safe_copy`).
- **What is known about the session before it?** Clean, abnormal, or not known.

Pruning proposals never include: the newest safe copy; the newest safe copy
that did not follow an observed abnormal termination; the newest safe copy
after a session known to have ended normally. A snapshot after an abnormal end
is kept, labelled, and never replaces or hides an earlier one.

### What a snapshot cannot do: progress that was never written

A snapshot copies what is on disk. In the RetroArch probes, every deliberate
fault left **no save file at all**: the game's battery save had not been
written yet. RetroArch's `autosave_interval` decides how often battery saves
are written while a game runs; with it at 0, progress may stay only in memory
until RetroArch closes normally. `emuwiz-cli continuity retroarch-saving`
reports the setting for each install (read-only) and explains this. It does
not change the setting, and turning it on is not a guarantee for every core
or game. Save states are a separate feature and are reported separately.

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

## Independent reviews

| Review | Scope | Verdict |
|---|---|---|
| 1 | Snapshot store, recovery, continuation copy, device model, CLI (at 035f56a2) | PASS WITH CONDITIONS: no path overwrites or deletes an original save; eight conditions, all addressed |
| 2 | Process identity, safe point, exit tracking (at 035f56a2) | PASS WITH CONDITIONS: no torn file published as a safe point while a lease is held; one latent blocker (a wrapper's exit 0 read as clean) and five should-fix, all addressed |
| 3 | Delta review of the fixes (cdc75795) | PASS WITH CONDITIONS: earlier conditions closed; four further points, addressed |
| 4 | Delta review of cdc75795..4fef4825 (direct signals, crash records, retention, diagnostic) | PASS WITH CONDITIONS: no blocker; five should-fix and six notes. Addressed: last-seen time kept current, daylight-saving and time-zone ambiguity, CLI crash source, wording when saves were not compared, document overclaims, bounded reading, forged-line guard, id-reuse guard, old manifest value still readable, override caveat. Left as it was: a compiled wrapper named like an emulator is taken for the emulator (older code, listed below) |

Remaining strict false positives the reviewers listed (they can only refuse a
snapshot, never permit one): this repository's own test binaries named
`retroarch_cheat_...`; a script named like an emulator; `flatpak update
<app id>`; any other user's process whose command line names the emulator.

One case that is not refuse-only: a **compiled** program whose file name
starts with an emulator's name is taken to be that emulator, so a compiled
wrapper's exit would be read as the emulator's. Script wrappers are
recognised; compiled ones are not. No such wrapper is known on the
development machine.

## Limits of this slice

- Session tracking is not yet called from Blue's launch path, so real launches
  produce no session and "how the emulator last ended" reads "not known".
- No Blue page yet.
- No emulator is on the demonstrated list in shipped code yet.
- The safe-point probes started each emulator idle, with no game: they show
  the emulator is *seen*, not what happens while it writes a save. Real save
  writing was exercised for RetroArch only (the synthetic game in the exit
  probes). The native and Flatpak RetroArch safe-point probes were
  inconclusive and have not been re-run.
- Crash records: Ubuntu only; group `adm` needed.
- Emulator and core versions are not established for any save.
- Standalone Dolphin: not installed, not tested against a real launch, always
  "unknown".
- No game-level save validation of any kind.
- A lease needs a local filesystem (ext4, xfs, btrfs, f2fs, tmpfs) and a file
  with a single name; otherwise the answer degrades to "unknown".
- The existing `save_snapshots` feature still limits itself to PCSX2 and
  RetroArch; it was not changed. The installed EmuWiz still has the old
  process-scan rule that reads "retroarch" in any command line as RetroArch
  running; the fix is on this branch only.
