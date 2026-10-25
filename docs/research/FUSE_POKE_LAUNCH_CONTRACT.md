# Classic POKE launch contract (Job #11)

Based on main `433389c0` and issue #111; reuses #112 discovery, #116 target
pins and #124 explicit pin creation. No new emulator or ROM patching path.

## Gold semantics

A POK `M`/`Z` record is bank, address, write value, restoration value. The last
field is not an equality test. Zero captures the memory value at activation;
a nonzero value supplies the later restore value. Gold now retains this in
`PokeOperation.restore_value` and sets no memory condition for POK imports.
Manual POKE inputs and old serialized `original_value`/`OriginalEquals`
operations retain their existing guard meaning. Missing new fields deserialize
to `None`; a guard is never silently reinterpreted as restoration.

Fuse projections reject all memory guards. VICE projections reject both guards
and restoration requirements they cannot implement. Restore differences and
guard differences have distinct conflict kinds. Existing parsers, association
assessment and selected-only RetroArch composition remain authoritative.

## Verified backend and format

The installed native core reports `Fuse 1.6.0 bce196f` and has SHA-256:

```
5d07fc75358a76be48ecf62e6c90a81efa285aa72aee164921be27b2552c4c15
```

Only the tested backend pair is accepted. A different file, including a core upgrade,
requires another compatibility test before adding its digest. The Flatpak core
reports a different build (`958105a`) and is not approved by this bridge.
Standalone Fuse was absent from the checked installation locations.

Native RetroArch 1.21.0 (`/usr/games/retroarch`, reached by `/usr/bin/retroarch`)
was exercised with private synthetic content. Its executable SHA-256 is
`07ecc7728586b3297c7b9101738a03d6f40fd6b6d87911ba051318eb7fdbd4e8`;
that digest is required too. Both canonical preflights must retain its exact
executable path, and its bytes join the final source bindings. A PATH wrapper script invoking
Flatpak is not a verified native frontend and is refused. Existing canonical
preflight also refuses ambiguous executable/profile selections; this feature
does not change PATH or choose a different emulator behind the user's back.

The core's `retro_cheat_set` accepts **M/Z records only**, separated by literal
backslash+n, through RetroArch's emulator/native handler (`cheatN_handler=0`).
It does not consume the complete `N...M...Z...Y` POK document at this interface.
Gold extracts validated operation records from its existing Fuse projection;
the existing RetroArch parser/selected-only renderer retains the native
handler and literal separators. Generic RetroArch memory cheats are not used.

Primary implementation reference:
[upstream Fuse core interface](https://github.com/libretro/fuse-libretro/blob/e997e2bc32c888348f862f69f2c53babfedf7791/src/libretro.c).
Installed core disassembly was also inspected; upstream HEAD is not assumed
to be the installed build.

## Deliberate limits

- The tested Linux native RetroArch/Fuse binary pair only; Flatpak, AppImage,
  executable wrappers and unverified frontend/core upgrades refuse.
- Uncompressed v1 48K `.z80` snapshots only. Exact length, nonzero PC and
  compression flag are checked. Tape loading can overwrite startup POKEs,
  so tapes are refused. Other snapshots and machine variants are not inferred.
- Bank 8, RAM addresses 16384–65535, byte values only. Banked writes, ROM
  addresses, overlapping writes (even equal ones) and value prompts refuse.
- One same-name `.pok` or `.POK`, a valid `.pok.target`, current source-aware
  DAT evidence, actual matching game bytes and unchanged trainer/pin required.
  Unsafe files, multiple links, ambiguous sidecars and withdrawn roots refuse.
- Trainer launches start from the snapshot with automatic state loading
  disabled in the disposable config. Ordinary Play is unchanged.
- Trainer association asserts a release link, not gameplay compatibility.

## Launch boundary and presentation

The chooser is optional and starts OFF, with every entry unselected. Loading
entries enables nothing. Only the separate explicit launch action submits
selected indices. Changing the game discards choices and late worker results;
a submitted choice resets OFF. Last requested titles and runtime receipt titles
remain visible. Generic cheat-file choices are separate; the POKE action
launches only its explicitly selected entries.

The launch worker reloads source-aware DAT evidence. Gold reads and compares
the snapshot, complete trainer file, target pin and exact selected entries to
the preview again. Their hashes and both approved backend hashes join the existing
runtime's source bindings and digest. The shared runtime checks them before
materialisation and again after argument preparation immediately before spawn.
No source file is written. Normal process ownership, disposable profile,
protected saves, receipt and cleanup mechanisms are reused.

Runtime UI reports **prepared / process started, activation NOT confirmed**.
A process exit of zero, a verified association, or preparation alone never
becomes an activation-success claim. The local probe below establishes
synthetic core API acknowledgement; it does not establish gameplay success
for an arbitrary release, and is not used to claim per-launch activation.

## Reproducible installed-backend probe

The opt-in test creates a disposable 48K snapshot, trainer and pin, calls Gold's
real preview/prepare bridge and selected-only renderer, and launches the real
frontend with private HOME/XDG, config, saves and states. It verifies both approved backend digests and checks that Fuse
acknowledges both generated M and Z writes and that source files remain equal.
It has a bounded child timeout. No downloaded or user game is needed.

```
EMUWIZ_FUSE_PROBE_RETROARCH=/usr/games/retroarch \
EMUWIZ_FUSE_PROBE_CORE=/usr/lib/x86_64-linux-gnu/libretro/fuse_libretro.so \
  scripts/cargo-iso test -p archivefs-core --lib \
  installed_fuse_acknowledges_gold_generated_m_and_z_records -- --ignored --nocapture
```

Normal CI leaves this installed-software probe ignored. Focused unit tests
cover semantic regressions, generated native code, unsupported operations,
stale association/identity/bytes/entries, unsafe sources, the final runtime
handoff check and actual chooser clicks. The unmodified normal CI workflow
is the final automated gate; independent review covers the launch boundary.


## Source-aware snapshot launch authority (Job #20)

`classic_trainer_evidence::snapshot::SnapshotEvidence` is a private-field proof
shared by Blue readiness and canonical RetroArch execution. Its constructor
loads the configured trusted roots, the read-only library and Gold's current
source-aware DAT projection, then checks actual uncompressed v1 48K snapshot
bytes against the audited strong hashes. It refuses missing, incomplete,
withdrawn, stale, ambiguous, conflicting or member-only evidence, unsafe files,
and unsupported snapshot variants. A filename, trainer pin, expected key or
caller-supplied verification boolean cannot create this authority.

Readiness resolves proofs on one bounded background worker and reuses stable
results without hashing catalogues on the GUI thread. Selection changes cannot
adopt another snapshot's pending result.

The normal request retains the exact readiness proof. Canonical preflight
revalidates it and compares the canonical snapshot SHA-256 key; the command
retains it for another fresh revalidation immediately before spawn. Snapshot
commands also bind and recheck the selected frontend/core object identities,
permissions and ctime across that resolver, including plain baseline launches.
The other
platform paths keep their existing identity logic. The POKE projection and
approved binary digests are unchanged.

The POKE chooser separately requires the existing planner review
acknowledgement. Its approval is scoped to the request, exact preview and
selected entries, and resets on refresh, entry or request changes and after
submission. The worker rechecks scope, sources and current identity before
passing the explicit acknowledgement through `CheatLaunchSelection`; neither
the applicability state nor hard refusal policy is changed. Ordinary Play
supplies no POKE selection and creates no cheat runtime.

The opt-in real Blue journey uses original CC0 content and an actual scan,
DAT audit, pin creation, normal readiness, chooser/review clicks and the normal
launch workers. Its probe child executes the production isolated core probe;
there is no fabricated core response or diagnostic identity bypass. Normal CI
leaves this installed-software test ignored. Build the GUI library tests with
`scripts/cargo-iso test -p archivefs-gui --lib --no-run`, then pass the resulting
`archivefs_gui-*` test executable to:

```
python3 scripts/qa/fuse-snapshot-smoke.py /absolute/path/to/archivefs_gui-test-binary
```

The driver creates private HOME/XDG/application roots and a private Xvfb
display. It keeps screenshots, process arguments, Blue UI text, the generated
profile, truthful receipts and source hashes in a printed disposable directory.
It checks baseline red, explicitly selected green and OFF-again red, with the
unselected lower screen remaining blue. Only observed visual changes establish
activation for this fixture/backend; receipts continue to report UNKNOWN.
