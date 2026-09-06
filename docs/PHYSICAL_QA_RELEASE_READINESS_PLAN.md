# EmuWiz physical QA and release-readiness plan

> Current AppImage note (2026-09-06): the fresh-home command-line harness
> supports hosts without FUSE by explicitly switching to extract-and-run only
> after a recognised FUSE-environment failure. This does not replace the P0-1
> interactive same-session onboarding/Home acceptance below.

## 1. Executive summary

This checkpoint has strong automated confidence in core state machines, safety
guards, mapping/publication contracts, persistence, and recovery branches. That
is not the same as proving a coherent desktop application. The remaining proof
is primarily an end-to-end desktop exercise: real display and input, real
filesystem permissions, real source/DAT data, real emulator profiles and
process handoff, real ES-DE, and restart or interruption at each write boundary.

The release gate should be a disposable physical QA run with an ordinary
desktop resolution (also check 1024x600), a copied existing-user profile, and
fixtures that include both valid and deliberately ambiguous or malformed data.
Never use the production ROM collection or an irreplaceable emulator profile.

At the inspected HEAD (`544895eb`), ES-DE Batch 3 is present and MegaDrive is
intentionally still a regional-policy partial. The physical plan treats ES-DE
mapping coverage, publication, recovery/idempotency, and actual ES-DE launch as
separate checks. It does not freeze parity counts from an older audit.

## 2. Current automated confidence

The following areas have meaningful automated coverage and should be used as
preconditions, not as substitutes for physical QA:

- onboarding state parsing, serialization, malformed-sidecar fallback, step
  progression, skip, and restart semantics;
- source/config persistence, bounded discovery projections, paged collection
  rendering, library filtering, and restart-oriented state tests;
- DAT parsing/registry/managed-source behavior, Verify projections, identity
  evidence, and Doctor read-only/repair gating;
- emulator profile discovery and readiness assessments, profile-kind selection,
  launch planning, and refusal of ambiguous or unsafe candidates;
- archive/library rename and Playing Library transaction planning, confirmation,
  journals, rollback, and recovery review;
- RomM configuration/source parsing, cache and error states, browser paging,
  mapping decisions, and read-only import boundaries;
- ES-DE export mappings, fail-closed unknown handling, publication preview,
  atomic write, idempotency, rollback, restart recovery, malformed recovery
  records, and path/name escaping;
- RetroArch `.cht`, PCSX2 `.pnach`, Dolphin Gecko/Action Replay `.ini`, and
  Xenia `.patch.toml` staging, identity gates, preview/apply/undo state, and
  transaction error paths;
- database/config recovery rules and GUI recovery-history action gating.

The main confidence gap is integration. Unit and GUI tests do not prove that a
real file picker, permissions boundary, window size, network timeout, emulator
binary, ES-DE installation, or desktop process behaves as the model expects.

## 3. P0 physical QA journeys

P0 is the checkpoint gate. A failed P0 journey blocks release-readiness until
the failure is understood and either fixed or explicitly accepted by the
release owner. Ten P0 journeys are defined; the nominal hands-on total is about
7 hours 45 minutes, excluding environment setup and reruns.

### P0-1 — Fresh install, onboarding, and restart (45 minutes)

Starting state: a disposable app data/config directory with no config, database,
onboarding sidecar, source, DAT registry, emulator profile, or recovery journal.
Use a copied test source, not a real collection.

Steps: launch the exact candidate binary; wait for diagnostics; complete Welcome,
Add source, optional DAT step, Emulator Setup, and Verify; exercise Back/Next,
Skip setup on a second disposable profile, close during an in-progress step,
relaunch, finish, then relaunch after completion.

Expected: the overlay opens once for a genuine first run; the five steps use the
real pages; source and optional DAT choices persist; emulator readiness is honest;
Verify is a summary rather than an unrequested mutation; completion survives
restart; Skip does not delete configuration; setup does not reopen for the
completed or skipped user.

Failure signs: blank or trapped overlay, duplicate scans, lost step, onboarding
reopens unexpectedly, silent source mutation, a false “ready” result, or a
crash on missing/unreadable sidecar.

State/files: config, database, onboarding sidecar, DAT registry, and activity
history may change; source content must not. Restore/delete only the disposable
profile after collecting logs. Network is optional; emulator installation is
not required for the first pass, though one known profile improves readiness
proof. Recovery check: restart at every persisted boundary and verify the
sidecar’s state is consistent with the visible step.

### P0-2 — Source addition, scan, identity, Verify, and Doctor (60 minutes)

Starting state: completed onboarding with a fixture source containing supported
archives, direct images, duplicate content, unknown platform folders, a
malformed sidecar, and one unreadable or permission-restricted item.

Steps: add the source; scan it; inspect Sources and Discovery; select representative
items in Gamer/selected-game view; open Verify; run Doctor; inspect findings;
confirm a repair only on a copied fixture; restart and rescan.

Expected: counts and rows converge; identity confidence is distinguished from
filename guesses; unknown/ambiguous data remains visible; Verify is read-only;
Doctor reports rather than silently repairs; explicit repair changes only the
approved fixture and records outcome; the selected-game view does not lose or
invent evidence.

Failure signs: zero or inflated counts, wrong platform, a malformed sidecar
blocking the whole source, synchronous UI freeze, Verify changing files, Doctor
repairing without confirmation, or a row disappearing instead of becoming
explicitly incomplete.

State/files: database observations, source/config records, identity evidence,
history, and any explicitly confirmed repair journal may change. Source files
must remain byte-for-byte unchanged unless the specific repair was confirmed.
Network is not required. No emulator installation is required. Recovery check:
restart with the incomplete fixture and confirm it remains diagnosable.

### P0-3 — Safe library mutation and rollback (60 minutes)

Starting state: copied source and target Playing Library roots with a small
fixture containing one planned rename, one duplicate, one collision, one
symlink-sensitive path, and enough free space for the operation.

Steps: build a reviewed rename/1G1R or canonical-organisation plan; inspect the
preview; cancel once; re-open and confirm; interrupt only using a controlled
process stop at a safe test point; reopen Recovery/History; preview rollback;
rollback; compare source and target trees and database state.

Expected: no write before confirmation; source/master ROMs are not silently
changed; collision and unsafe-path cases are blocked; the transaction is
recoverable after interruption; rollback restores the pre-change target and
leaves a truthful history record; repeat preview/apply is idempotent.

Failure signs: partial unjournalled mutation, source deletion, wrong link target,
rollback offered after manual tampering, stale UI claiming success, or a second
apply duplicating files/links.

State/files: target links/copies, transaction journal, database projection,
history, and recovery records. Network is not required. Emulator installation
is not required. Recovery check is the purpose of this journey; retain before
and after manifests and hashes.

### P0-4 — Emulator readiness, launch planning, and handoff (60 minutes)

Starting state: one disposable game with verified identity and at least two
known emulator profile shapes where available: Native, Explicit, AppImage or
Portable. Include a deliberately missing, non-executable, and ambiguous profile.

Steps: run Emulator Setup/Doctor; inspect discovered profiles and readiness;
open Gamer and launch planning; verify the selected platform/game; launch using
one real emulator; repeat with an external profile kind; test the blocked and
ambiguous cases.

Expected: readiness names the actual blocker; no emulator is guessed when
candidates conflict; launch uses the selected profile and content path; the
emulator opens and receives the intended game; EmuWiz remains usable after the
child exits; blocked cases explain remediation and do not spawn a process.

Failure signs: wrong executable or content, shell-like quoting failure, silent
launch no-op, launch of an unapproved candidate, false readiness, or GUI lockup.

State/files: remembered profiles, activity history, and emulator process state
may change; emulator configuration must not be rewritten without an explicit
feature action. Network is not required. Emulator installation is required.
Recovery check: remove/rename the profile or binary, restart, and confirm the
state becomes blocked rather than retaining stale readiness.

### P0-5 — ES-DE publication, recovery, idempotency, and launch (60 minutes)

Starting state: a disposable ES-DE installation/profile and a small reviewed
Playing Library projection. Include one supported Batch 3 platform, one unknown
platform, and a MegaDrive case that demonstrates regional ambiguity.

Steps: confirm ES-DE profile discovery; build the publication preview; verify the
canonical platform and exact target system; cancel; rebuild and confirm; inspect
the resulting `gamelist.xml`; repeat publication; restart between a controlled
write interruption and recovery; recover the pending record; launch the published
game from ES-DE.

Expected: preview identifies the reviewed target and destination; unknown mapping
fails closed; Arcade/MAME/FBNeo remains publication-neutral; Amiga, CD32, CDTV,
DOS, ScummVM, Atari ST, and other distinct identities are not collapsed; existing
bytes are preserved; repeat publication is unchanged; recovery restores or
finalizes safely; ES-DE itself launches the intended entry using its own emulator
configuration. MegaDrive remains PARTIAL until a regional policy exists.

Failure signs: wrong system folder, duplicate XML entry, malformed escaping,
truncated gamelist, publication selecting an emulator, stale recovery record,
or ES-DE launching the wrong content/emulator.

State/files: ES-DE `gamelist.xml`, recovery sidecar/journal, Playing Library
projection, and history. Network is not required. Emulator installation is
required for actual ES-DE launch. Recovery check is mandatory; preserve file
hashes and byte snapshots before each write.

### P0-6 — RetroArch local `.cht` install (30 minutes)

Starting state: disposable RetroArch profile and verified game identity; a valid
local `.cht`, an already-installed equivalent, and a wrong-identity `.cht`.

Steps: discover/select the profile; choose the local file; inspect preview and
exact destination; cancel once; explicitly confirm; inspect the resulting file
and RetroArch directory; reapply; undo; repeat with wrong identity.

Expected: preview is complete, confirmation is explicit, install is confined to
the selected cheat root, reapply is idempotent, undo restores the prior state,
and wrong identity is refused. No original ROM is changed.

Failure signs: guessed identity, overwrite without preview/backup, duplicate
entries, undo deleting a pre-existing file, or RetroArch not seeing the result.

State/files: target `.cht`, transaction/history/backup state. Network is only
needed if the selected path uses a remote catalogue; local install should work
offline. RetroArch installation is required for activation proof. Recovery check:
manually alter the target before undo and confirm rollback is blocked or explains
the conflict.

### P0-7 — PCSX2 local `.pnach` install (30 minutes)

Starting state: disposable native or supported PCSX2 profile, verified PS2
identity/CRC, valid `.pnach`, pre-existing equivalent, and wrong-identity file.

Steps: discover/select the profile; inspect the PNACH preview; cancel; confirm;
inspect the resolved `cheats`/`cheats_ws` destination and file contents; reapply;
undo; test wrong identity and a profile that is discovered but ineligible.

Expected: the current install path requires exact evidence and explicit
confirmation; destination resolution is safe; reapply is idempotent; undo is
available only when safe; wrong identity and ineligible profiles are refused.

Failure signs: the UI still presents an install action for an unverified game,
wrong CRC filename, silent directory creation, mutation of an unrelated profile,
or inability to distinguish a pre-existing file during undo.

State/files: PCSX2 PNACH file, backup/journal/history. Network is not required.
PCSX2 installation is required. Recovery check: change the target externally and
verify the rollback guard reports the conflict.

### P0-8 — Dolphin Gecko/Action Replay `.ini` install (30 minutes)

Starting state: disposable Dolphin profile with a verified GameCube/Wii identity,
valid local GameSettings `.ini`, existing equivalent, and wrong Game ID.

Steps: discover/select profile; preview exact sections/codes and destination;
cancel; confirm; inspect `GameSettings`; reapply; undo; repeat with a candidate
whose identity is only filename-level or incompatible.

Expected: verified identity gates the write, preview lists exact changes, install
is confined to the chosen profile, reapply is unchanged, undo restores prior
bytes, and weak/wrong identity is blocked.

Failure signs: filename evidence treated as verified, wrong Game ID, code
execution/evaluation, unrelated INI modification, or rollback overwriting a
user edit.

State/files: Dolphin `GameSettings/*.ini`, backup/journal/history. Network is
not required. Dolphin installation is required for activation proof. Recovery
check: manually edit the destination before undo and verify safe refusal.

### P0-9 — Xenia `.patch.toml` install (30 minutes)

Starting state: disposable explicitly supplied Xenia Canary directory, verified
Xbox 360 Title ID, valid patch TOML, a second candidate, existing patch, and
wrong Title ID.

Steps: type/select the Xenia directory; discover profiles; retrieve or use the
local patch candidate; preview selected patches and exact target; cancel; confirm;
inspect the target; reapply; undo; test incompatible and partially verified
candidates.

Expected: Xenia’s explicit-directory requirement is visible; candidate selection
does not silently choose among multiple files; exact-compatible patches can be
installed only after confirmation; reapply is idempotent; undo and conflict
handling are safe; activation status remains honestly unknown where the adapter
cannot verify it.

Failure signs: guessed Xenia path, wrong Title ID, automatic multi-candidate
selection, patch installation into an unrelated directory, or UI claiming the
patch is active without evidence.

State/files: Xenia patch TOML, backup/journal/history, remembered explicit path.
Network is required only for provider refresh, not local staged install. Xenia
installation is required for launch/activation proof. Recovery check: mutate the
target before undo and verify conflict handling.

### P0-10 — Existing-user upgrade and recovery continuity (60 minutes)

Starting state: a copy of a real representative user profile made while the app
is closed: config, database plus SQLite sidecars, source/DAT registry, emulator
profiles, onboarding state, history, and a pending or completed recovery record.

Steps: open with the candidate build; check migration and Home; navigate all
major destinations; restart; perform a read-only scan/Doctor; inspect existing
profiles and recovery history; resolve only a deliberately disposable pending
transaction; compare state manifests to the pre-upgrade copy.

Expected: existing configuration, sources, DATs, profiles, journals and history
survive; onboarding does not unexpectedly reopen; startup does not scan, mount,
download, or mutate source images; a pending journal remains usable and is not
silently discarded. Downgrade is tested only by restoring the pre-upgrade copy,
not in-place.

Failure signs: reset configuration, missing rows, replayed destructive action,
lost recovery option, migration loop, or a startup write outside the expected
managed state.

State/files: migration metadata and managed state may change; record exact diffs.
Network is not required. Emulator installation is optional for continuity, but
profiles must be present. Recovery check is mandatory.

## 4. P1 physical QA journeys

P1 is important operational confidence but does not block the next checkpoint if
P0 is clean and the limitation is documented.

### P1-1 — Full source/DAT lifecycle and degraded network (45 minutes)

Use a disposable source and valid, duplicate, mismatched, malformed, and missing
DAT fixtures. Register managed DATs, use the optional DAT path, refresh Verify,
then disable or interrupt a network-backed catalogue operation. Expect cached
state and errors to be explicit, no source mutation, no false identity, and no
UI deadlock. Check managed registry and history diffs; restore the disposable
state. Network is required for the online branch; no emulator is required.

### P1-2 — Doctor findings and explicit repair matrix (45 minutes)

Prepare missing paths, stale profiles, bad permissions, unsafe links, malformed
sidecars, and a recoverable journal. Run Doctor and inspect each category,
severity, remediation, and action gate. Confirm one safe repair and refuse one
unsafe repair. Expect read-only scan behavior and truthful post-repair
verification. Network is not required; emulator installation is useful but not
mandatory.

### P1-3 — Emulator Setup and adapter/profile matrix (75 minutes)

Exercise native, explicit, Flatpak, AppImage, and Portable profiles supported by
the machine, plus missing/non-executable paths. Check discovery, remembered
profiles, readiness, launch planning, and restart persistence for RetroArch,
PCSX2, Dolphin, and standalone/external adapters. The expected result is
profile-specific readiness and no guessed executable. Network is not required;
real installations are required for the profiles being tested.

### P1-4 — RomM connect, browse, and read-only import (60 minutes)

Use a disposable RomM account/server or controlled mock endpoint with valid
credentials, empty results, pagination, an unresolved platform, a transient
failure, and stale cache. Connect from Sources, browse, inspect detail, import or
apply only the supported read-only projection, restart, and disconnect. Expect
no upload or source mutation, bounded error messages, and honest unresolved
identity. Network is required; no emulator is required. Preserve and remove
only the disposable RomM config/cache.

### P1-5 — Playing Library / 1G1R boundary cases (60 minutes)

On a copied collection, plan 1G1R with regional duplicates, equal candidates,
missing evidence, existing links, collisions, and a previously applied plan.
Verify preview ordering, explicit policy choices, confirmation, idempotency,
rollback, and restart. Expect source preservation and no silent choice where
policy is ambiguous. Network is not required; no emulator is required.

### P1-6 — Gamer and selected-game continuity (30 minutes)

Use supported, unknown, ambiguous, archived, direct-image, and missing-path
rows. Open Gamer from Home, select a game, inspect identity/evidence, navigate to
Cheats, launch planning, Playing Library, and back. Expect selection and context
to remain correct or be cleared explicitly when invalidated. Check no stale
identity authorizes launch or apply. Network/emulator are optional.

### P1-7 — ES-DE publication breadth (60 minutes)

Using a disposable ES-DE tree, sample the current actual mapping report rather
than a stale document count: one target from each recently promoted parity
batch, Arcade, MAME/FBNeo aliases, Amiga/CD32/CDTV, DOS, ScummVM, Atari ST,
unknown, and MegaDrive. Confirm previews are correct and neutral about emulator
choice. This is mapping/publication coverage only; actual launch is P0-5.

### P1-8 — Packaged/extracted release smoke test (45 minutes)

Use the candidate release artifact on a clean desktop account or VM. Launch it
from its intended extracted layout, confirm writable managed-state placement,
file-picker behavior, restart, and one read-only source scan. Test at normal
size and 1024x600. Expect no dependency on the repository checkout and no
permission prompt hidden behind a blank page. No network is required; emulator
installation is optional.

## 5. P2 polish/deferred checks

P2 does not gate the coherent-application checkpoint unless it exposes a safety
or data-integrity defect.

- resize, keyboard focus, scrolling, long names, high-DPI rendering, and
  accessibility labels across all pages;
- empty states and recovery wording for optional RomM, DAT, catalogue, and
  emulator features;
- repeated navigation and long idle sessions for stale cache or repaint drift;
- catalogue refresh UX and offline cache freshness beyond the bounded P1 pass;
- unsupported/general local mod imports and other providers not covered by the
  four implemented local-install adapters (deferred, not silently supported);
- ES-DE artwork/media behavior and broader regional policy; MegaDrive remains
  partial and must not be “fixed” by selecting a region;
- notification/background service, Android Auto, Chromecast, public access,
  scheduled work, and other explicitly out-of-scope features.

## 6. Fresh-install journey

Use a temporary application-data root or clean VM. Do not delete a user’s real
config. Capture a directory manifest before launch and after each step.

1. Start with no config/database/onboarding sidecar, no source, no managed DAT,
   no emulator profile, and no recovery journal.
2. Launch and verify a friendly loading/diagnostic state, then the Welcome
   overlay. Confirm no scan, mount, download, or source write occurs by merely
   opening the app.
3. Add a disposable source and confirm the source record, scan progress, counts,
   and read-only source behavior.
4. Take the optional DAT path with a valid managed fixture, then repeat once
   with no DAT and confirm that “optional” does not block completion.
5. Exercise emulator discovery with no emulator, then add one known profile and
   rerun readiness. Confirm the first result is not falsely ready.
6. Run Verify and inspect the summary; explicitly confirm any later repair or
   mutation rather than treating Verify as permission.
7. Finish onboarding, close, relaunch, and confirm state, sources, DAT registry,
   and profile choices survive without onboarding reopening.
8. In a copy, delete or corrupt `onboarding_state.txt` and remove a managed
   sidecar. Relaunch and confirm safe `NotStarted`/missing-evidence behavior,
   recovery wording, and no destructive repair.

## 7. Existing-user upgrade journey

Make a complete copy while the application is stopped. Include `library.sqlite3`
and `-wal`/`-shm` sidecars when present, config, managed DAT registry, source
definitions, remembered profiles, onboarding state, activity history, and one
recoverable journal. Record hashes and file sizes.

Open the copy with the candidate. Verify migration, Home, Sources, DATs,
emulator profiles, Gamer selection, Playing Library history, Cheats & Mods
history, ES-DE recovery, and restart. Compare manifests and database counts.
The first open must not unexpectedly scan, mount, download, rename, rewrite a
source, reopen completed onboarding, or discard a recovery option. If a pending
transaction is present, use the documented preview/restore path and capture its
result. Test downgrade only by replacing the entire copy with the pre-upgrade
snapshot; never downgrade a live database in place.

## 8. Source/DAT/Verify/Doctor journey

This is one evidence chain: source discovery creates observations; DATs and
identity evidence refine them; Verify summarizes; Doctor diagnoses health and
only explicitly confirmed repairs write. Test it with a mixed fixture containing
valid ZIP/7z/RAR, direct images, duplicate content, unknown folders, malformed
metadata, missing files, unsafe links, and permission failures.

The acceptance condition is honest degradation: uncertain identity stays
uncertain, unsupported content stays visible with a bounded terminal reason,
and a bad item does not hide the rest of the source. Check database rows,
history, source bytes, and managed-state diffs after every operation.

## 9. Emulator Setup and launch journey

Exercise the real installed profiles, not only discovered labels. Cover Native,
Explicit, AppImage, Portable, Flatpak where supported, and standalone/external
launch profile kinds. Use one known-good game per adapter and one candidate that
must be blocked. Confirm executable path, working directory, arguments,
environment/portal constraints, content path, and child-process lifecycle.

A pass requires the emulator to open the intended game and the GUI to remain
responsive. A readiness card or automated profile test alone is insufficient.
Do not allow this journey to alter emulator configuration unless a separate
explicit user action is being tested.

## 10. Gamer/RomM journey

From Home, reach Gamer for a local selected game, inspect evidence and launch
readiness, then browse RomM from Sources if configured. Verify local and RomM
identity are not silently merged when evidence differs. For RomM, test login or
connection failure, empty results, pagination, unresolved platform, stale cache,
and reconnect. RomM is an optional/read-only source in this plan; no download,
upload, or source mutation is implied by browsing.

## 11. Playing Library / ES-DE journey

Treat the Playing Library planner as the policy boundary and ES-DE as a later
publication projection. Review the 1G1R decision, confirm the target library,
publish a small set, inspect exact gamelist bytes, repeat, restart, recover, and
then launch from ES-DE. Include a target already present and one conflicting
entry.

The ES-DE check has four independent results: current mapping coverage, preview
target correctness, safe publication/recovery/idempotency, and actual ES-DE
launch. Report them separately. Arcade/MAME/FBNeo publication remains neutral
about emulator choice. Amiga variants and DOS/ScummVM remain distinct. Do not
use physical QA to justify a guessed MegaDrive regional mapping.

## 12. Cheats & Mods journey

Run P0-6 through P0-9 with the same sequence for each adapter: valid install,
preview, cancel, explicit confirmation, inspect resulting file/config, reapply,
undo, wrong identity refusal, and external-edit rollback conflict. Record exact
destination, original bytes, backup/journal/history records, and post-undo hash.

Use only disposable emulator profiles. The local install should work offline
when the source file is local; provider/catalogue refresh is a separate network
branch. Verify activation by opening the emulator/profile where practical, but
do not claim that file presence proves runtime activation when the adapter says
activation is unknown. General arbitrary local/community mod importing remains
deferred; do not expand this sequence into an unsupported feature claim.

## 13. Recovery/rollback journey

Build a recovery ledger before testing: database/config snapshot, source and
target manifests, ES-DE gamelist bytes, cheat target bytes, journal names,
history entries, and process IDs where applicable. Test cancellation before
write, failure during staging, interrupted finalization, restart discovery,
explicit recovery preview, successful restore, already-restored state, manual
target modification, missing backup, permission denial, and duplicate retry.

Expected behavior is copy-first, fail-closed, and diagnostic-led. A rollback must
never overwrite a changed user file silently. Recovery state must be visible and
actionable after restart. Restore the disposable fixture after each scenario;
never use `git reset`, `git clean`, or an in-place production collection as a
test recovery mechanism.

## 14. Large-library performance checks

### Automated 94k RomM cache/projection pass (2026-09-06)

The reproducible release guard generates 94,000 varied RomM records at test
time; no user cache, network service, or committed bulk fixture is involved.
It exercises the production cache publication/read path
(`publish_cache` → `load_cache` → JSON decode + validation) and the production
Sources → RomM record-page projection (`build_record_page`). Home itself stays
SQLite-backed and does not eagerly join this optional RomM cache; selected-game
metadata is cache-only and selection-scoped, while Gamer artwork builds its
path index once on its worker.

Debug baseline on this QA host: a 63,713,437-byte generated cache loaded and
validated in 1.728 s first-read / 1.772 s repeated-read. The 94k browser page
projection took 125 ms first pass / 121 ms repeated, returned its bounded page,
preserved all 94,000 records and platform variation, and made zero presence
filesystem probes with the default filter. The cache parse/deserialize is the
dominant cold cost; the page builder is a bounded background projection and
does not perform N+1 database, network, or stat work in its ordinary path.

The regression tests intentionally use only a 30-second catastrophic-regression
ceiling plus structural assertions (count, deterministic page, bounded page,
and zero default-filter probes), not a machine-specific target. This closes the
automated 94k RomM P1 risk. Physical desktop/resource-watch coverage remains
P1 follow-up, not a prerequisite for the cache/projection contract.

Use a RomM snapshot or controlled fixture near 94,000 records, plus a small local
source. This is a bounded pass, not a benchmark of every page.

Measure by observation: time to open Sources/Discovery, first usable paint,
scroll/page transitions, filter changes, selection changes, RomM reconnect, and
return to Home. Watch CPU, memory, disk, network requests, and logs. Repeat
after restart to expose cache invalidation problems.

Pass criteria: no multi-second repaint stalls for ordinary paging/filtering, no
full-record list rendered when a page is requested, no pathological repeated
scan/network work on repaint, no unbounded memory growth, and no stale totals or
selection after a cache refresh. Stop if the fixture causes uncontrolled disk or
network activity; preserve logs and report the boundary.

## 15. Test fixtures/data needed

- clean and existing-user app-data snapshots, including SQLite WAL/SHM and
  onboarding/recovery sidecars;
- disposable source trees with supported containers, direct images, duplicates,
  unknown/ambiguous names, malformed metadata, missing files, permission errors,
  symlinks, collisions, and long Unicode names;
- valid/mismatched DATs and managed-DAT registry entries;
- at least one verified game for RetroArch, PS2, GameCube/Wii, Xbox 360, and a
  representative ES-DE platform from each promoted batch;
- disposable native/Explicit/AppImage/Portable/Flatpak profile fixtures and
  real installed emulator binaries for launch proof;
- valid and wrong-identity `.cht`, `.pnach`, Dolphin `.ini`, and Xenia
  `.patch.toml` files, with pre-existing target variants;
- a disposable ES-DE installation with writable gamelist, existing entries,
  conflicting entries, and a way to interrupt/copy the publication boundary;
- controlled RomM endpoint/account or captured test service with success, empty,
  pagination, auth failure, malformed JSON, timeout, and stale-cache cases;
- a near-94k RomM-record snapshot and log/resource observation tooling.

## 16. Recommended execution order

1. Freeze the candidate binary and record commit/artifact hashes; create clean
   and existing-user copies.
2. Run P0-1, P0-2, and P0-3 before connecting any real emulator or ES-DE.
3. Run P0-4 with one native and one external profile kind.
4. Run P0-6 through P0-9 with disposable emulator profiles.
5. Run P0-5 publication and actual ES-DE launch.
6. Run P0-10 upgrade/recovery continuity.
7. Run P1 source/DAT/Doctor, RomM, 1G1R, profile matrix, ES-DE breadth, and
   packaged smoke tests.
8. Run the bounded large-library pass, then P2 usability/deferred checks.
9. Preserve artifacts: logs, screenshots, manifests, hashes, failure replay
   steps, and the exact environment/profile fixture identifiers.

## 17. Checkpoint Definition of Done

The checkpoint is physically release-ready when:

- all ten P0 journeys pass on a real desktop, or every exception has a named
  owner, reproducible evidence, and explicit release acceptance;
- fresh install and existing-user upgrade both preserve the stated safety and
  persistence boundaries;
- at least one real emulator launch and one ES-DE launch open the intended game;
- all four local-install adapters prove preview, confirmation, idempotent
  reapply, undo, wrong-identity refusal, and external-edit protection;
- source/DAT/Verify/Doctor behavior is honest for valid, missing, malformed,
  ambiguous, and unsafe fixtures;
- no rollback/recovery path silently loses data or overwrites a user edit;
- the 94k RomM pass has no release-blocking stalls, runaway scans, or cache
  corruption;
- all artifacts and deviations are recorded against the exact build.

| JOURNEY | AUTOMATED STATUS | PHYSICAL QA | PRIORITY | ESTIMATED TIME |
|---|---|---|---|---:|
| Fresh install/onboarding/restart | PARTIAL / DEGRADED | Required: real clean profile and desktop input | P0 | 45m |
| Source scan/identity/Verify/Doctor | AUTOMATED VERIFIED for contracts | Required: mixed files, permissions, real UI | P0 | 60m |
| Library mutation and rollback | AUTOMATED VERIFIED for transactions | Required: disposable filesystem and interruption | P0 | 60m |
| Emulator Setup and launch handoff | AUTOMATED VERIFIED for planning | Required: installed emulators/process launch | P0 | 60m |
| ES-DE publication/recovery/launch | AUTOMATED VERIFIED for contracts | Required: real ES-DE and gamelist launch | P0 | 60m |
| RetroArch `.cht` install | AUTOMATED VERIFIED for state/adapter | Required: real profile and activation check | P0 | 30m |
| PCSX2 `.pnach` install | AUTOMATED VERIFIED for state/adapter | Required: real profile and activation check | P0 | 30m |
| Dolphin Gecko/AR `.ini` install | AUTOMATED VERIFIED for state/adapter | Required: real profile and activation check | P0 | 30m |
| Xenia `.patch.toml` install | AUTOMATED VERIFIED for state/adapter | Required: explicit path and real profile | P0 | 30m |
| Existing-user upgrade/recovery continuity | PARTIAL / DEGRADED | Required: copied real state and restart | P0 | 60m |
| DAT managed lifecycle/offline degradation | AUTOMATED VERIFIED for parsing/state | Required: real files and network interruption | P1 | 45m |
| Doctor repair matrix | AUTOMATED VERIFIED for gating | Required: permissions and real repair fixtures | P1 | 45m |
| Emulator profile-kind matrix | AUTOMATED VERIFIED for discovery shapes | Required: installed Native/Flatpak/AppImage/Portable variants | P1 | 75m |
| RomM connect/browse/import | AUTOMATED VERIFIED for state/projections | Required: controlled service and network failures | P1 | 60m |
| Playing Library / 1G1R edge cases | AUTOMATED VERIFIED for planning | Required: copied collection and restart | P1 | 60m |
| Gamer/selected-game continuity | AUTOMATED VERIFIED for projections | Required: real navigation and stale selections | P1 | 30m |
| ES-DE mapping breadth | AUTOMATED VERIFIED for mapping contracts | Required: disposable target sampling | P1 | 60m |
| Packaged/extracted desktop smoke | Not verified by unit tests | Required: clean desktop/account | P1 | 45m |
| 94k RomM bounded performance | AUTOMATED VERIFIED for cache/projection | Optional: desktop resource-watch follow-up | P1 | 60m |
| Resize/focus/accessibility/long-session polish | PARTIAL / DEGRADED | Required: representative desktop sweep | P2 | 90m |
| Unsupported general local mods/providers | DEFERRED | Not a current supported journey | P2 | — |
| MegaDrive regional policy | PARTIAL / DEGRADED | Do not accept guessed mapping | P2 | — |

## 18. Execution record: ES-DE publication and recovery Wave 2

**Run:** 2026-09-06 against authoritative commit
`76aff27d413363f8a981e0eb13697de071e13d19`, clean
`feature/archivefs-unified-platform` checkout.

### Disposable environment

- Root: `/tmp/emuwiz-esde-qa-K9l0pB`.
- Synthetic source placeholders covered SNES, MegaDrive, TurboGrafx-16, PC
  Engine, PC Engine CD, PC-98, Dreamcast, and PlayStation 2. They were
  ordinary empty files used only as bounded path fixtures; no real ROMs or
  user library paths were read or written.
- The root also contained an unrelated sentinel and an unrelated ES-DE-home
  fragment. SHA-256 verification after the run confirmed both were unchanged.
- The actual production path was exercised over temporary ES-DE-home-shaped
  directories beneath that root: `discover_es_de_environment` constructs an
  `EsDeProfile`; `plan_es_de_gamelist_publication` creates the preview;
  `apply_es_de_gamelist_publication`,
  `rollback_es_de_gamelist_publication`, and
  `recover_es_de_gamelist_publication` perform real filesystem operations.

### Results

| Check | Result |
| --- | --- |
| Preview/no-write boundary | PASS — preview captured exact target bytes and entries before apply. |
| Exact mappings | PASS — `tg16`, `pcengine`, `pcenginecd`, and `pc98` were verified; PC-98 and NEC PC-9801 share only the approved `pc98` export target. Export coverage also retained SNES, MegaDrive, Dreamcast, and PS2 mappings. |
| Deferred/unknown refusal | PASS — unmapped/unknown platforms fail closed; no target is fabricated. |
| Existing content | PASS — existing gamelist bytes, comments, and unrelated entries are preserved byte-for-byte apart from the appended owned game entry. |
| Apply/idempotence | PASS — a second identical plan reports the existing destination as already present and produces no duplicate XML. |
| Rollback/recovery | PASS — rollback restores exact previous bytes; restart-style recovery restores prior bytes or removes a newly-created gamelist, then clears the sidecar. |
| Failure injection | PASS — simulated interruption leaves the durable sidecar and blocks a second plan/apply until recovery; corrupt/mismatched records fail closed. |
| Path safety | PASS for supported boundary — symlinked or directory recovery targets, malformed and oversized gamelists, and unconfigured systems are refused without mutation. Parent-directory symlink traversal remains explicitly outside this helper's scope because profiles provide the parent chain. |

The focused commands were run sequentially with `CARGO_BUILD_JOBS=2`:

```text
cargo test -p archivefs-core es_de_publish -- --test-threads=1
cargo test -p archivefs-core es_de_export -- --test-threads=1
```

Both passed. The publication suite uses the real atomic writer and disposable
filesystem, rather than a mocked executor. It covers generated XML escaping,
pre-existing-user-content preservation, fresh-file removal on rollback,
unresolved/corrupt recovery refusal, and final-component symlink refusal.

### Remaining physical boundary

No `es-de` executable, `DISPLAY`, or `WAYLAND_DISPLAY` was available in this
environment. Therefore actual ES-DE process launch and click-through GUI
confirmation/recovery presentation are **NOT TESTABLE here**, not failures.
The disposable production-path filesystem checks above do not modify a real
ES-DE profile and do not prove ES-DE itself consumes the resulting list; that
remaining desktop check is still the P0-5 real-frontend step in this plan.

### Findings

- **P0 defects:** none.
- **P1 follow-up:** run one real ES-DE GUI publication/launch against a
  disposable `--home` profile when an ES-DE desktop session is available.
- **P2 policy:** unchanged. MegaDrive remains the intentional deterministic
  partial mapping; Atari 8-bit, Commodore 128, Hyper Neo Geo 64, and generic
  PC remain unmapped/deferred by current policy.

## 19. Execution record: local cheat install and rollback Wave 3

**Run:** 2026-09-06 against authoritative commit
`d1c0d1f3db4176dc7170771d1ff935bb77ae6a08`, clean
`feature/archivefs-unified-platform` checkout.

### Disposable environment and production paths

- Root: `/tmp/emuwiz-cheat-qa-IR4Liw`, used as `TMPDIR` for the existing
  real-filesystem end-to-end suites. It held isolated RetroArch, PCSX2,
  Dolphin, Xenia, source-game, journal, and sentinel folders; no host emulator
  config or game path was supplied to the tests.
- Each installer used its existing local-import planner and the shared,
  journal-backed apply/rollback executor. No QA-only writer or identity path
  was introduced.
- SHA-256 checks before and after covered all four disposable source-game
  fixtures plus every seeded unrelated emulator file and sentinel. Every digest
  matched after preview/apply/rollback coverage.

### Results

| Family | Physical disposable-filesystem result |
| --- | --- |
| RetroArch `.cht` | PASS — selected cheats reached the real RetroArch browse destination, preserved enabled state, were idempotent on identical repeat, and restored replaced bytes through rollback. Cross-platform/no-match/malformed inputs never became install candidates. |
| PCSX2 `.pnach` | PASS — local PNACH was staged to the CRC-derived target with journal/rollback; existing content and zero-byte targets were handled safely. Wrong CRC, unresolved identity, malformed input, source symlink, and unwritable profile all refused before writes. |
| Dolphin Gecko | PASS — selected Gecko code used the real `GameSettings/<game-id>.ini` destination; existing sections and unrelated files survived, rollback restored prior bytes, and wrong game/revision identity never reached apply. |
| Dolphin Action Replay | PASS — the existing local-Dolphin parser/stager accepted only valid AR syntax, keeps Gecko and AR sections distinct, preserves unrelated codes, and stages mixed Gecko/AR files without relabelling either family. Malformed AR input is rejected as a whole. |
| Xenia `.patch.toml` | PASS — verified local Title-ID patch preview/apply/undo is atomic; existing patch bytes and pre-existing directories survive rollback. Mismatched/missing identity, oversized source, replacement without approval, and symlinked destinations fail closed. |

### Safety and recovery evidence

- **Preview immutability:** the shared journey proves planning/confirmation is
  read-only; `cheat_installer` also proves dry runs create neither destination
  nor journal.
- **User-content preservation:** existing `.cht`, `.pnach`, Dolphin INI, and
  Xenia patch cases preserve unrelated bytes/sections; older journal rollback
  cannot destroy a later operation or externally changed destination.
- **Identity refusal:** ambiguity, stale or missing identity, wrong Dolphin
  game/revision, wrong PNACH CRC, malformed local input, and mismatched Xenia
  identity were all blocked before the write path. Filename resemblance is not
  accepted as a substitute for bound identity.
- **Failure/restart recovery:** journal-backed replacement failure retains the
  original or verified backup; failed verification is surfaced; repeated
  rollback is safe; rollback restores the recorded prior state rather than
  relying on preview-only state. These are exercised through the real shared
  executor and fresh journal reads in the end-to-end suites.
- **Source immutability:** source-game fixture SHA-256 values were unchanged;
  the PCSX2 unwritable-profile path additionally proves a refused apply does
  not touch the ROM or create a PNACH.

Focused commands were run sequentially with `CARGO_BUILD_JOBS=2`:

```text
cargo test -p archivefs-core --test local_cheat_install_journey --test retroarch_cheat_install_end_to_end -- --test-threads=1
cargo test -p archivefs-core --test pcsx2_local_pnach_install_journey --test pcsx2_pnach_install_end_to_end -- --test-threads=1
cargo test -p archivefs-core --test dolphin_gecko_install_end_to_end -- --test-threads=1
cargo test -p archivefs-core local_cheat_install_dolphin -- --test-threads=1
cargo test -p archivefs-core --test xenia_local_patch_install_journey --test xenia_patch_install_end_to_end -- --test-threads=1
cargo test -p archivefs-core --test cheat_journey_orchestration -- --test-threads=1
cargo test -p archivefs-core cheat_installer -- --test-threads=1
```

All passed. The selected end-to-end counts were RetroArch 3+12, PCSX2 6+9,
Dolphin Gecko 14 plus 19 local-Dolphin parser/stager tests, Xenia 3+9, shared
journey 3, and shared installer 30.

No `DISPLAY`/`WAYLAND_DISPLAY` session was available, so an interactive Cheats
& Mods GUI apply/rollback is **NOT TESTABLE here**, not a defect. The remaining
desktop check is confirmation wording, result visibility, and recovery-action
discoverability over one disposable local cheat; it does not block the passed
production-path filesystem safety evidence.

### Findings

- **P0 defects:** none.
- **P1 follow-up:** one desktop-session confirmation/rollback pass for the
  existing Cheats & Mods GUI.
- No real emulator config, game library, ES-DE state, onboarding work, DAT GUI
  work, or LBC content was read or modified.

## 20. Execution record: firmware / BIOS readiness Wave 4

**Run:** 2026-09-06 against authoritative commit
`6ad241dc2b8f024cdfd7cea56d7230316e65cad1`, clean
`feature/archivefs-unified-platform` checkout.

### Disposable environment and production path

- Root: `/tmp/emuwiz-firmware-qa-BqoWjF`, used as `TMPDIR` for existing
  PCSX2, DuckStation, and PC Engine CD firmware tests. It contained only
  empty disposable game fixtures and a sentinel; SHA-256 values were checked
  before and after all scans.
- The exercised path is the existing no-follow firmware discovery/hash matcher
  (`FirmwareIdentityRecord` evidence through the adapter inspector), then
  `FirmwareReadiness`, `LaunchBlockerKind::RequiredFirmwareMissing`, and the
  launch-readiness GUI projection. No QA evaluator or BIOS content was added.

### Results

| Case | Result |
| --- | --- |
| Required verified | PASS — matching PCSX2/DuckStation evidence becomes `Verified`; the GUI projects `Ready` and says EmuWiz recognised required firmware by hash. |
| Present, wrong/unverified | PASS — plausible names with CRC/MD5/SHA-1/size mismatch remain unverified/unknown and are never promoted to Ready. |
| Required missing | PASS — absent required firmware produces `Missing` and only the real `RequiredFirmwareMissing` blocker yields the blocking “Required firmware missing” wording and Doctor action. |
| Optional/non-blocking missing | PASS — missing firmware without that blocker projects the non-blocking “Firmware missing” state; no blocker is fabricated from readiness alone. |
| Not required | PASS — Stella, VICE, and PPSSPP command/readiness coverage retains `NotRequired`, with no firmware warning or blocker. |
| Wrong path/name/unsafe path | PASS — directory and symlink BIOS candidates are unsafe/refused; wrong hash or plausible filename never verifies; missing directories remain missing. |
| Multiple candidates | PASS — one verified candidate is selected deterministically; conflicting verified candidates are ambiguous, and several unverified candidates do not create a false verified result. |

The 70-test GUI launch-readiness suite passed and covers plain-language
firmware summaries, the `RequiredFirmwareMissing`-only blocking distinction,
NotRequired presentation, and the `OpenDoctor` action rather than a DAT-source
route. The summary explicitly retains the no-download boundary: EmuWiz
verifies firmware but does not supply BIOS or system ROM files.

Focused commands ran sequentially with `CARGO_BUILD_JOBS=2`:

```text
cargo test -p archivefs-core pcsx2_firmware -- --test-threads=1
cargo test -p archivefs-core duckstation_firmware -- --test-threads=1
cargo test -p archivefs-core pcengine_cd_firmware -- --test-threads=1
cargo test -p archivefs-gui launch_readiness -- --test-threads=1
cargo test -p archivefs-core stella_command -- --test-threads=1
cargo test -p archivefs-core vice_command -- --test-threads=1
cargo test -p archivefs-core ppsspp_never_requires_firmware -- --test-threads=1
```

All passed. No `DISPLAY`/`WAYLAND_DISPLAY` session exists here, so the
interactive GUI cases (Ready, Found not verified, Required firmware missing,
and Not required) remain **NOT TESTABLE on a desktop**, not failures. They are
a P1 desktop follow-up; the production filesystem/hash and projection paths
were verified in disposable tests.

### Findings

- **P0 defects:** none.
- **P1 follow-up:** one desktop GUI/Doctor navigation sweep with a disposable
  emulator profile and legal synthetic test firmware evidence.
- No real BIOS collection, emulator configuration, game library, onboarding
  work, DAT GUI, ES-DE, Cheats & Mods, or LBC content was accessed or changed.

## 21. Execution record: Playing Library / 1G1R Wave 5

**Run:** 2026-09-06 at `cc78b75cf0dd2bfcd4190079a01e99ea5058e5f1` on a clean
authoritative checkout. Disposable root:
`/tmp/emuwiz-playing-library-qa-fzjirc`.

The root held tiny synthetic multi-region/revision/beta/unmatched placeholders,
a destination sentinel, and a DAT fixture area. It was used as `TMPDIR` for
the existing real-filesystem Playing Library tests; all source placeholders and
the sentinel retained their SHA-256 values after the run.

The exercised production path was `build_playing_library_plan` and its real
`ElectionExplanation`, followed by `build_playing_library_transaction` and the
shared journaled rename-apply executor/rollback. No separate election or
linking implementation was introduced.

`CARGO_BUILD_JOBS=2 cargo test -p archivefs-core playing_library --
--test-threads=1` passed **100 tests**. Coverage includes preference-ordered
region/revision election and explanation evidence, unresolved/missing-track
refusal, destination conflicts, preview-only planning, exact symlink creation,
multi-file release atomicity, an induced mid-apply failure with no partial
release, idempotent reapply, and rollback that removes only EmuWiz-created
links. The transaction tests also preserve sources and unrelated destination
content; a conflict is refused rather than overwritten.

Confirmation is still the existing exact `CREATE {count} LINKS` form, produced
from the planned operation count; no confirmation semantics changed. The
automated contract covers stale/conflicting operations and journal-backed
recovery/rollback rather than relying on in-memory winners.

No `DISPLAY`/`WAYLAND_DISPLAY` desktop was available, so the Library
Organisation click-through/confirmation/rollback presentation is **NOT
TESTABLE here**, not a defect. It remains a P1 desktop follow-up. No real ROM,
RomM, ES-DE, onboarding, DAT GUI, firmware, Cheats & Mods, or LBC state was
accessed or modified. **P0 defects: none.**

## 22. Current V1 release-readiness summary (audit at `4763dd3`)

## 23. Execution record: Playing Library / 1G1R Wave 7

**Run:** 2026-09-06 at `38aefe1da03ef2baeb538afffb6b99964a9c174c`.
Disposable-only root: `/tmp/emuwiz-playing-library-qa-20260906-063100` (with
`source`, `destination`, `dat`, sentinels, and durable transaction state).
No real ROM, RomM, ES-DE, or mounted-game path was read or written.

The run used a generated, real Logiqx XML DAT parsed through
`parse_dat_file`, and production whole-file SHA-1 matching through
`match_loose_files_against_dat`. It contained Game A (USA/Europe/Japan), Game
B (Europe/Japan), Game C and E (USA Rev 1/Rev 2), Game D (retail/Beta), one
unmatched file, one one-file/two-DAT-game SHA-1 ambiguity, and unrelated
content. The matcher returned 11 verified matches; it correctly omitted the
unmatched and ambiguous candidates. The Beta was explicitly excluded.

With Europe > USA > Japan, the production planner elected A Europe, B Europe,
C Rev 2, D retail, and E Rev 2. Its own `ElectionExplanation` reported
preferred-region decisions for A/B, verified-revision decisions for C/E, and
the sole eligible release for D. With USA > Europe > Japan, only Game A
changed (to USA); B remained the Europe fallback and revision/exclusion
results were unchanged. Planning left source SHA-256 values and the
destination sentinel unchanged.

The five planned operations required `CREATE 5 LINKS` (the established typed
confirmation threshold remains greater than 8). The real
`build_playing_library_transaction` plus shared journaled `rename_apply`
executor created five exact symlinks to the elected source files, without
copying, moving, or altering sources. A fresh harness process reloaded the
journal and performed production rollback: only those links disappeared and
the unrelated destination sentinel remained. A collision introduced after
planning was rejected by `AbortAll` preflight before any mutation. This also
confirms persisted recovery is not dependent on in-memory state.

The automated core contracts remain the authority for destination/source
symlink escapes, malformed/non-directory components, stale confirmation,
threshold wording, idempotent reapply, and multi-file atomicity. No graphical
desktop (`DISPLAY`/`WAYLAND_DISPLAY`) was available for the Library
Organisation click-through; that is a P1 desktop follow-up, not a filesystem
defect. No P0/P1 defect was found in the exercised production path.

## 24. Real desktop GUI smoke attempt

**Attempt:** 2026-09-06 at `8f7aecfadcbc1004fcfcbad6dff50aa370e22e46`.
The authoritative host had neither `DISPLAY` nor `WAYLAND_DISPLAY`, no visible
Xorg/Xwayland/KWin/GNOME/Xfce session, and `loginctl` could not access a user
session bus. No known Nobara desktop clone was available under the accessible
user home directories. Consequently, no GUI process, fixture state, AppImage,
or real user data was launched or modified.

The physical checks for Recently Found, Identify & Rename, Duplicate Finder,
DAT Identity, Firmware, ES-DE, Cheats & Mods, Playing Library, and same-session
onboarding/Home remain **P1 desktop follow-up work**. This is an environmental
limitation, not a product defect; it must be rerun in a real X11/Wayland
session using a disposable HOME and synthetic fixtures.

Read-only reconciliation of everything above against current source, run
2026-09-06 at authoritative commit
`4763dd34b8d5f4a02b2f5927dbdd995d7b41d7ce`. This section is the single current
answer to "what actually remains before V1"; it supersedes reading the raw
wave/table history above for that question. No production code was changed to
produce it.

### Closed since the original plan (§1-§17) was written

- DAT Identity GUI P0 (`00713a2`) — selected-game DAT identity/verification
  presentation, distinct from structural identity, real Verify Games route;
  P1 items (all-library aggregate, BIOS-missing summary, cross-family
  conflict model, source-variant projection) explicitly deferred, not
  reopened.
- Doctor/Emulator Setup 12-test regression (`76aff27`) — confirmed by direct
  re-run (`cargo test -p archivefs-gui --bin archivefs-gui doctor_and_repair`,
  124 passed, 0 failed) and by diff inspection: the fix touched only
  `tests/doctor_and_repair.rs` (57 lines), zero production files. All 12 were
  `STALE_TEST_EXPECTATION` against a `#[cfg(any())]`-disabled dead function
  (`show_emulator_setup_summary`) whose strings ("Emulator readiness",
  "Setup incomplete", "Download managed emulators" gated on a scan) never
  render; the live page (`emulator_setup_page::show`) already used current
  strings ("Emulator candidates", "Check emulators", "Not checked"). No
  `REAL_GUI_REGRESSION` was found.
- ES-DE final safe gaps (`48140d0`) — TurboGrafx-16 and PC-98/NEC PC-9801 now
  map live; confirmed directly in `es_de_export.rs`
  (`platforms_still_unmapped_after_final_safe_gaps_remain_refused` asserts
  exactly `["Atari 8-bit", "Commodore 128", "NeoGeo64", "PC"]` remain
  unmapped). Registry has 76 canonical platforms; 4 unmapped + 1 intentional
  partial (MegaDrive) yields the 71 COMPLETE / 1 PARTIAL / 4 MISSING ceiling.
  Confirmed unchanged from `docs/ESDE_FINAL_GAP_DECISIONS.md`'s decision.
- Firmware/BIOS GUI (`d478e47`) — plain-language firmware summary shipped;
  Wave 4 (§20) proved the backend projection paths; Stella/VICE/PPSSPP retain
  `NotRequired`, `RequiredFirmwareMissing` remains the sole source of the
  blocking wording.
- ES-DE publication/recovery Wave 2 (§18), Cheats & Mods Wave 3 (§19),
  Firmware Wave 4 (§20), Playing Library Wave 5 (§21) — all report **P0
  defects: none** against real filesystem operations, journals, and rollback;
  each wave's remaining gap is exactly one desktop GUI click-through pass
  (see below), not a functional defect.
- Physical Launch QA Wave 1 (`dae99e5`) — real RetroArch core launches (GBA,
  PSX) reached actual content; one PSX core crash was RetroArch/Mednafen's
  own (confirmed via `/var/crash`, real BIOS present, ruled out as a firmware
  gap); EmuWiz's own handoff and fail-closed behavior were exactly correct.
  No EmuWiz launch blocker found.
- Recalbox competitive audit's two "P0 — before 1.0" recommendations
  (5-step onboarding wizard; per-platform plain-language firmware summary)
  are **both already implemented** (onboarding feature + `d478e47`) — that
  doc is stale on those two rows; not edited here per this audit's
  documentation-only scope, but the recommendations are no longer open work.

### Home P0 — CLOSED (`P0_CLOSED`)

**Update (this promotion):** fixed and promoted onto authority as commit
`c379183c0d3add8fa24d98ef3f52b12e7c62bac5` (`fix(gui): complete Home load
after onboarding`, cherry-picked from `ecae382286d539a0c9b7449362b1e6fac0b94661`).
Root cause was two related invariant violations, both confirmed by direct
code inspection before any fix was written:

1. `ArchiveFsApp::new()`'s very first archive-snapshot load runs before
   onboarding ever adds a source or writes a config file, so it resolves -
   once, terminally - before the user finishes onboarding, and nothing in
   the onboarding flow ever called `self.refresh(context)` afterward
   (adding a source only reloads the separate `database_state`, used by
   Advanced View's Library tab, never Gamer View's own snapshot).
2. Gamer View's `data: Option<&LoadedData>` collapsed `LoadState::Error`
   (a worker that already finished, terminally, with a failure) and
   `LoadState::Loading` (genuinely still in flight) into the same `None` -
   a fresh install's missing config file produced a hard `Err`, exactly
   the terminal state (1) then left frozen, directly contradicting
   `create_starter_config`'s own comment that "a fresh install with zero
   sources loads normally."

Fix: `onboarding_advance_from`/`onboarding_skip_entirely` now call the
existing `self.refresh(context)` exactly once, only on their two terminal
transitions, reusing the same "state changed, reload now" pattern every
other completion call site already used - no new state machine, no timer,
no polling loop, and `poll_load`'s stale-generation rejection is untouched.
`load_read_only_snapshot` now treats a missing (not merely unreadable)
config file as the empty library a freshly-written starter config would
produce, narrowly scoped to `io::ErrorKind::NotFound`; any other read
failure still fails closed exactly as before.

Both new regression tests
(`finishing_onboarding_in_the_same_session_retries_the_stale_archive_load`,
`skipping_onboarding_entirely_also_retries_the_stale_archive_load`,
`read_only_snapshot_resolves_to_an_empty_library_when_no_config_file_exists_yet`)
were verified to fail on the pre-fix code before being proven green
against the fix. Re-verified again at promotion time: `onboarding`
(26/26), `home_page` (40/40), `gamer_view` (103/103), and core
`read_only_snapshot` (3/3) all pass; `cargo check -p archivefs-gui`,
`cargo fmt --all -- --check`, and `git diff --check` are all clean; the
release build succeeds.

**Classification: `P0_CLOSED`.** The AppImage artifact remains stale and
still requires a rebuild (see below) before this fix is reflected in a
packaged artifact and re-verified end-to-end via the fresh-install QA
harness - that rebuild/retest pass is the next, now-unblocked step.

### AppImage artifact — `STALE_ARTIFACT`, additionally `BLOCKED_BY_HOME_P0`

- `dist/EmuWiz-x86_64.AppImage` (83,687,928 bytes, mtime unchanged since
  `2026-09-06 00:47`) was built from commit `c16f486` — 15 authoritative
  commits behind current HEAD, missing VICE, Stella's final-QA context,
  ES-DE final safe gaps, DAT GUI P0, firmware GUI, and the Doctor test fix.
- `4763dd3` recorded a rebuild attempt: `cargo check -p archivefs-gui` and
  the release `emuwiz` binary build both passed, but the AppImage itself was
  **not** packaged because this host has neither an approved `appimagetool`
  executable nor the required pinned type-2 runtime file
  (`docs/APPIMAGE_PACKAGING.md` requires both as explicit host inputs; the
  build script fails closed rather than substituting an unapproved tool).
- **Update (this promotion):** the Home P0 that previously blocked a
  meaningful rebuild is now fixed on authority (`c379183`, see above). The
  artifact itself has **not** been rebuilt by this promotion (no AppImage
  packaging was performed, per this task's own scope) and remains the same
  stale `c16f486` build.

**Classification: `STALE_ARTIFACT` (provenance only; the Home P0 blocker on
a *meaningful* rebuild is now cleared).** A rebuild is no longer blocked by
an unresolved defect, only by provenance staleness and this host's missing
`appimagetool`/pinned runtime inputs. Do not reuse the existing artifact as
evidence of current-authority behavior for anything beyond the
packaging/AppRun-mechanism findings already recorded in
`docs/APPIMAGE_FRESH_INSTALL_QA.md`. Rebuilding and re-running the
fresh-install QA harness against the new artifact remains the next step.

### Remaining desktop-smoke items (no `DISPLAY`/`WAYLAND_DISPLAY` in any QA
environment used so far)

All of the following are **P1**, each with disposable-filesystem/backend
correctness already proven green (per §18-§21); none is promoted to P0
without direct evidence of a functional defect, per this audit's own
instruction:

| Desktop smoke | Backend evidence | Priority |
| --- | --- | --- |
| ES-DE GUI publish + real ES-DE launch | Wave 2 (§18): preview/apply/idempotence/rollback/recovery all PASS | P1 |
| Cheats & Mods GUI apply/rollback confirmation wording | Wave 3 (§19): RetroArch/PCSX2/Dolphin Gecko+AR/Xenia all PASS, source hashes preserved | P1 |
| Firmware/BIOS GUI wording + Doctor navigation | Wave 4 (§20): Verified/Found-not-verified/Required-missing/non-blocking/NotRequired all PASS at the projection layer; the 70-test GUI suite already covers the presentation logic itself, only live rendering is unverified | P1 |
| Playing Library GUI confirmation/apply/rollback | Wave 5 (§21): 100 tests PASS on election, transaction, idempotent reapply, rollback | P1 |

None of these four is a release blocker: each wave's underlying safety
contract (no silent mutation, fail-closed on ambiguity, rollback restores
exact prior state) is proven by real disposable-filesystem tests, not merely
mocked. The desktop pass would confirm presentation/wording only.

### Remaining real-installed-emulator physical-launch gaps

Per `docs/PHYSICAL_QA_LAUNCH_WAVE1.md`, still genuinely untested with a
physically installed emulator on any host used so far:

- RMG, Mesen 2, Snes9x, Stella, VICE — none of the five installed on the
  Wave 1 QA host; each adapter's own focused core/command/execution/
  integration test suite is green (verified earlier in this session's work
  promoting each adapter), so command construction, readiness, fail-closed
  behavior, and RetroArch coexistence are proven at the unit level. Only the
  "does a real installed binary actually open and load the game" step is
  unverified.
- RetroArch AppImage — no RetroArch AppImage installed on the Wave 1 host;
  the underlying `feat(launch): support verified RetroArch AppImage
  profiles` (`e6ebe22`) automated coverage is unaffected.

**Classification: P1 coverage gap for all six, not a release blocker.**
RetroArch itself (the emulator actually covering the overwhelming majority
of the real library, per Wave 1's 68,853-item real-library evidence) was
physically proven to reach real content on this exact host. A V1/alpha does
not require physical proof of every adapter before shipping when: (a) the
platform's RetroArch fallback path is itself physically proven, (b) each
adapter's own unit/integration suite is green, and (c) no adapter is
advertised to a user as "physically verified" anywhere in the GUI (readiness
language is honestly sourced from discovery/hash evidence only, never from
an unrun physical test).

### openMSX / shared machine-profile seam

Confirmed still deferred per `docs/OPENMSX_STANDALONE_ADAPTER_AUDIT.md`'s own
Definition of Done — no machine-profile seam exists yet, no `openmsx`
adapter code exists anywhere in `crates/`. **P1/P2 post-V1**, matching the
existing roadmap; not a release blocker.

### DAT backend/GUI deferred items

Per `docs/DAT_GUI_WIRING_AUDIT.md` §"P0 / P1 / P2" (P0 already promoted,
confirmed above): P1 items remaining open are an all-library aggregate
count, carrying parsed-catalogue-variant into selected-game provenance, and
a separate BIOS/firmware readiness view (the last one is now effectively
superseded by the firmware/BIOS GUI work, `d478e47`, though the audit doc
itself was not edited to reflect that — a stale-doc note, not a functional
gap). P2 items (candidate/source comparison, raw DAT graph browsing,
evidence export, saved filters, durable cross-evidence conflict model)
remain untouched and unnecessary for V1. **None is release-blocking.**

### Version/tag state

- Workspace version in `Cargo.toml`: `0.8.1-alpha` (shared via
  `version.workspace = true` across all three crates).
- Latest tag reachable from HEAD: `v0.8.2` (`git merge-base --is-ancestor
  v0.8.2 HEAD` succeeds; HEAD is 40 commits ahead of that tag).
- No tag exists at or after current HEAD. The workspace version string
  (`0.8.1-alpha`) predates even the `v0.8.2` tag it is already behind, and a
  further 40 commits of adapter/GUI/QA work have landed since that tag with
  no version bump.
- **Not resolved here by design** (this audit does not force a version
  decision): whichever version a release actually ships as, the current
  `Cargo.toml` value does not reflect it, and this will need a real decision
  (e.g. `0.8.3-alpha` or `0.9.0-alpha`) at release-cut time, separate from
  the Home P0 fix.

### One newly-observed small polish item (not fixed here — read-only audit)

`crates/archivefs-gui/src/emulator_setup_page.rs`'s `adapter_name()` has no
match arm for `"rmg"` (confirmed live: `platform_map.rs` registers
`standalone_adapters: &["rmg"]` for N64), so an RMG candidate row would
currently render the generic fallback label "Supported emulator" instead of
"RMG" in Emulator Setup. **Classification: P2 polish** (cosmetic label gap
only; readiness/launch behavior for the RMG adapter itself is unaffected and
already covered by its own green test suite). Left unfixed per this audit's
read-only scope; worth a one-line fix in a future GUI-only pass.

### Authoritative release-readiness matrix

| AREA | STATUS | EVIDENCE | SEVERITY | RELEASE BLOCKER? | NEXT ACTION |
| --- | --- | --- | --- | --- | --- |
| Home same-session load hang | CLOSED | `c379183` (cherry-picked from `ecae382`); onboarding/home_page/gamer_view/read_only_snapshot suites re-verified green at promotion | NONE | No | none |
| AppImage artifact currency | OPEN | artifact still from `c16f486`, now 17+ commits stale (includes the Home P0 fix) | P1 | No (packaging gate only; the P0 defect blocking a *meaningful* rebuild is closed) | Once `appimagetool`/pinned runtime are available on a build host, rebuild and rerun the fresh-install harness |
| DAT Identity GUI P0 | CLOSED | `00713a2`; 23 focused + 191 re-verified GUI tests | NONE | No | none |
| Doctor/Emulator Setup regressions | CLOSED | `76aff27`; re-run 124/124 green, test-only diff | NONE | No | none |
| ES-DE final safe gaps (TG-16, PC-98) | CLOSED | `48140d0`; live export-table assertion | NONE | No | none |
| ES-DE publication/recovery (Wave 2) | CLOSED (filesystem layer) | §18 | NONE | No | desktop GUI pass is P1, not a gate |
| Cheats & Mods install/rollback (Wave 3) | CLOSED (filesystem layer) | §19 | NONE | No | desktop GUI pass is P1, not a gate |
| Firmware/BIOS readiness (Wave 4) | CLOSED (projection layer) | §20; `d478e47` | NONE | No | desktop GUI pass is P1, not a gate |
| Playing Library / 1G1R (Wave 5) | CLOSED (filesystem layer) | §21 | NONE | No | desktop GUI pass is P1, not a gate |
| Physical Launch QA Wave 1 | CLOSED | `dae99e5`; real RetroArch GBA/PSX launches reached content | NONE | No | none |
| ES-DE desktop GUI publish/launch | DESKTOP_SMOKE | §18 | P1 | No | run when a desktop/ES-DE session is available |
| Cheats & Mods desktop GUI apply/rollback | DESKTOP_SMOKE | §19 | P1 | No | run when a desktop session is available |
| Firmware/BIOS GUI wording live render | DESKTOP_SMOKE | §20 | P1 | No | run when a desktop session is available |
| Playing Library GUI confirm/apply/rollback | DESKTOP_SMOKE | §21 | P1 | No | run when a desktop session is available |
| RMG/Mesen 2/Snes9x/Stella/VICE physical launch | DEFERRED | `PHYSICAL_QA_LAUNCH_WAVE1.md`; unit suites green, no installed binary | P1 | No | run when any is installed on a QA host |
| RetroArch AppImage physical launch | DEFERRED | `PHYSICAL_QA_LAUNCH_WAVE1.md`; no AppImage installed | P1 | No | run when one is installed on a QA host |
| ES-DE intentional gaps (Atari 8-bit, C128, NeoGeo64, PC) | DEFERRED | `ESDE_FINAL_GAP_DECISIONS.md`; live export-table confirms exactly these 4 unmapped | P2 | No | policy decision only, not a defect |
| MegaDrive regional PARTIAL | DEFERRED | `ESDE_FINAL_GAP_DECISIONS.md` | P2 | No | do not accept a guessed mapping |
| openMSX adapter | DEFERRED | `OPENMSX_STANDALONE_ADAPTER_AUDIT.md`; no seam, no code | P2 | No | sequence after a machine-profile seam exists |
| DAT P1 items (aggregate count, catalogue-variant provenance) | DEFERRED | `DAT_GUI_WIRING_AUDIT.md` | P1 | No | separate GUI-only follow-up pass |
| DAT P2 items | DEFERRED | `DAT_GUI_WIRING_AUDIT.md` | P2 | No | none planned for V1 |
| 94k RomM bounded performance pass | NOT_REQUIRED for this audit's scope | original plan §17 row, unchanged | P1 | No | run when a large snapshot + resource-watch environment is available |
| Resize/focus/accessibility/long-session polish | DEFERRED | original plan §17 row, unchanged | P2 | No | representative desktop sweep, post-V1 acceptable |
| RMG `adapter_name()` fallback label | OPEN (newly observed) | `emulator_setup_page.rs`, no `"rmg"` arm | P2 | No | one-line GUI fix in a future pass |
| Recalbox audit's two "P0" items | CLOSED (doc stale) | onboarding feature + `d478e47` | NONE | No | optionally refresh `RECALBOX_COMPETITIVE_AUDIT.md`'s status column (not done here) |

### Shortest true critical path

**Update (this promotion):** item 1 below is now closed (`c379183`). The
remaining critical path is packaging-only:

1. ~~Fix the Home same-session "Loading your games…" hang.~~ **Closed** -
   `onboarding_advance_from`/`onboarding_skip_entirely` now call
   `self.refresh(context)` on their terminal transitions, and
   `load_read_only_snapshot` treats a missing config file as an empty
   library rather than a terminal error. Regression tests
   (`finishing_onboarding_in_the_same_session_retries_the_stale_archive_load`,
   `skipping_onboarding_entirely_also_retries_the_stale_archive_load`,
   `read_only_snapshot_resolves_to_an_empty_library_when_no_config_file_exists_yet`,
   `advancing_through_a_non_final_onboarding_step_does_not_reload_the_archive_snapshot`)
   are on authority and green.
2. **Rebuild the AppImage** once an approved `appimagetool` + pinned
   type-2 runtime are available on the build host (no longer blocked by
   an open defect, only by tool availability).
3. **Re-run the fresh-install QA harness** (`packaging/appimage/
   test-fresh-home.sh` plus the manual onboarding-completion walk in
   `docs/APPIMAGE_FRESH_INSTALL_QA.md`) against the new artifact, confirming
   the Home hang no longer reproduces in the exact repro steps already
   documented.
4. **Perform the four desktop-smoke passes** (ES-DE, Cheats & Mods,
   Firmware/BIOS GUI, Playing Library) opportunistically wherever a
   `DISPLAY`/`WAYLAND_DISPLAY` session becomes available — P1, not gating,
   but cheap to close out given every backend contract is already green.

Everything else audited in this pass (DAT GUI, Doctor/Emulator Setup, ES-DE
mapping ceiling, five newest adapters' physical launch, openMSX, version/tag
housekeeping) is confirmed closed, correctly deferred, or a non-blocking
P1/P2 — none of it belongs on the critical path to V1.

## Final release-cut reconciliation (2026-09-06, `7ac525a`)

This section supersedes the older point-in-time matrix and critical-path notes
above where they describe pre-closure AppImage tooling, the DAT variant gap,
the RMG label, or the 94k RomM pass. Historical QA observations remain useful
evidence; they are not a statement of the current release-cut state.

### Closed automated gates

- Home post-onboarding same-session reload P0: `c379183`.
- DAT Identity GUI P0: `00713a2`.
- Doctor/Emulator Setup regression cleanup: `76aff27`.
- Pinned AppImage tooling provenance: `ce33fdf` (appimagetool 1.9.1 and
  type-2 runtime 20251108, checksum-verified by the packaging contract).
- FUSE-less fresh-home harness fallback: `d36e304`; only explicit
  FUSE-unavailable failures switch to extract-and-run, and other failures
  remain fatal.
- DAT catalogue/variant provenance: `9888bd5`.
- RMG fallback label: `99e511d`.
- Generated 94k RomM cache/load and browser-projection guard: `7ac525a`.

### Manual-pending release gates

The current AppImage technical mechanism is green: pinned tooling, the fresh
artifact's extract-and-run version smoke, and the FUSE-less harness are proven;
desktop `:0`/Xauthority access and normal GUI initialization have also been
proven. This host lacks FUSE, so normal AppImage mode is an environment
limitation, not an artifact defect.

**Still required before an alpha/V1 cut:** complete the current artifact's
interactive fresh-user GUI acceptance on a real desktop, including onboarding,
the same-session Home transition immediately after Finish, empty and small
source cases, restart consistency, Sources/Discovery, DAT and Doctor
navigation, and isolated-HOME/source-immutability checks. This is manual
pending, not a packaging or product defect. The broader P0 physical journeys
in §3 remain release-owner acceptance work wherever the relevant emulator,
ES-DE, and disposable filesystem environment is available.

### Non-blocking deferred items

- ES-DE V1 policy gaps remain intentionally unsupported: Atari 8-bit,
  Commodore 128, NeoGeo64, and generic PC; MegaDrive remains intentionally
  partial rather than guessing a region.
- `openMSX` is deferred post-V1/P2: no adapter and no shared machine-profile
  seam exists. It is greenfield multi-file adapter work, not a small existing
  seam.
- Physical launches for optional standalone adapters, desktop smoke sweeps,
  resize/focus/accessibility/long-session polish, and optional RomM resource
  watching are P1/P2 confidence follow-ups, not release blockers.
- DAT aggregate count and other P2 browsing/reporting ideas are non-blocking;
  catalogue/variant provenance is no longer in that list.

### Release-cut actions not yet performed

1. Finish and record the manual current-AppImage interactive acceptance above.
2. Decide the target release version. The workspace currently advertises
   `0.8.1-alpha`, which is stale relative to reachable tag `v0.8.2`.
3. Bump the workspace version consistently after that decision.
4. Rebuild the final AppImage from the versioned authority, then verify its
   final SHA-256 and `--version` in extract-and-run mode (and normal mode where
   FUSE is available).
5. Create the release tag only after the final artifact/version checks pass.

Current tag state: `v0.8.2` points at `db8092d`; current authority is not
tagged (`git describe --tags` is `v0.8.2-48-g7ac525a`). CLI and GUI version
output both derive from `env!("CARGO_PKG_VERSION")`, so no independent source
version string was found.
