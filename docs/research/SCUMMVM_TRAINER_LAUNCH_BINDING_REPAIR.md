# ScummVM native trainer target binding repair

Date: 2026-09-30. Starting main: `54d0503b8fa2eaa48f3cf5a0ca71fe935a126384`.
`git fetch origin` completed; local `main`, `origin/main` and the authoritative
worktree HEAD agreed. Tracked main was clean. Existing untracked research/cache
files were left alone. Recent main changes concerned Browse & Play/catalogue
and artwork. Branch: `fix/scummvm-trainer-launch-binding`; worktree:
`/home/davedap/emuwiz-scummvm-launch-binding`.

## Current-tree inventory and selection

The audit used current source, registrations, tests, recent history and 366
registered worktrees. GUI finishing and Cheat Core 1–6 are separate lanes.
Historical adapter branches do not establish missing mainline support.
No branch was merged or cherry-picked.
The exact changed-file set has no overlap with the six Cheat Core branch
diffs or `feature/gui-finish-one-surface` at the inspected tips.

Current `launch/` contains 34 command modules, 32 corresponding execution
modules, and RetroArch's generic `execution.rs`. The command/execution pairs
all have focused tests; some have only a small command/adapter fixture set.
No TODO/FIXME or unimplemented/todo markers were found under `launch/`.

| Current native command family | Execution / notable audit boundary |
|---|---|
| Amiberry, Amiberry CD, Amiga WHDLoad, FS-UAE | Dedicated execution; profile/firmware and media scope remain adapter-specific |
| Azahar, Cemu, DeSmuME, melonDS | Dedicated execution; installed-content/firmware restrictions differ; small Azahar/DeSmuME fixture coverage |
| Dolphin, DuckStation, Flycast, PCSX2 | Dedicated preflight/execution and tests; selected profiles govern writable user state |
| DOSBox, ScummVM | Dedicated execution; DOSBox config inspection and ScummVM fresh detection; trainer config target had no revalidation |
| FBNeo, MAME | Dedicated execution; DAT/set/dependency evidence; MAME launches by machine name and search root rather than selected archive argv |
| Hatari, openMSX, RMG, Stella, VICE | Dedicated execution and tests; firmware/media scope is explicit |
| mGBA, Mesen, SameBoy, Snes9x | Dedicated execution; selected loose media/extension gates; native save-sidecar policy is not universally isolated |
| PPSSPP, RPCS3, Vita3K, xemu, Xenia | Dedicated execution and tests; installed layouts, firmware and writable profile resources are not interchangeable |
| Tsugaru, XRoar | Dedicated execution now exists; old missing-adapter research is stale |
| RetroArch | Generic execution; appendconfig/profile persistence research remains relevant; not touched |
| Fuse | Command/planning/readiness is present; no dedicated execution module found; excluded from this task |

The old native coverage audit calls openMSX execution absent, but current main
has it. The general launch-support document also lists fewer targets than
current code. Neither was used as an implementation authority or rewritten.
No native registration was found for Ryujinx, shadPS4, Atari800, Caprice32,
BeebEm or NP2. Recognition/installation evidence does not imply their launch
support. Existing limitations around archives, multidisc inputs, installed
layouts and BIOS remain explicit adapter gates, not targets for this repair.

Read-only command planners do not themselves rewrite emulator profiles.
Selected native profile flags can still select writable emulator state:
PCSX2 uses a reviewed profile `-datapath`; Dolphin can use `-u`; Xenia uses
its profile configuration. This audit did not runtime-certify those paths.
ScummVM trainer apply already uses staged materialization and the shared
confirmed transaction/journal/rollback path. Its actual emulator launch can
rewrite the selected configuration, as this experiment demonstrates.

**CHOSEN BACKEND TARGET: ScummVM trainer target binding at native launch.**
It wins on isolation and evidence: the existing native executable is installed,
the path has reproducible defects, and the change needs no GUI/shared cheat
models. mGBA/PCSX2/MAME cheat isolation requires additional frontend/version,
save/state and persistence proof. Dolphin/Xenia profile changes would overlap
larger route/isolation work. The next isolated candidate is MAME command
integrity: validate native machine-name grammar and single-root `-rompath`
semantics against MAME itself rather than relying only on direct argv safety.

## Defects and repair

1. The trainer command emitted `--config FILE`. Installed ScummVM 2.8.0
   rejects that form. It now emits documented `-c FILE`, with separate argv
   elements and no shell.
2. Generated target sections stored qualified `engine:game` in `gameid`.
   ScummVM's engine lookup expects the unqualified game ID beside `engineid`.
   Rendering now separates them; already generated qualified IDs can be
   regenerated through the existing preview/apply path. Native bare IDs are
   accepted on later previews, with an explicit conflicting engine refused.
3. Missing fields/options for an existing target were appended at the end of
   the file, potentially into another target's section. They now stay inside
   the selected section. Existing unrelated sections and savepath survive.
4. Preflight verified the selected folder using fresh detection, then switched
   to an unchecked config target. The selected target is now read and checked
   before detector invocation, and checked again immediately before spawn.
   Its `engineid`, unqualified `gameid` and literal folder path must match the
   authorized identity/folder. `-p <selected folder>` also pins runtime content
   selection without changing the config's saved location.
5. A new alternate config does not inherit the normal profile's global
   savepath. Trainer launch now requires an explicit absolute target savepath
   or an explicit global savepath in the owned config. Target overrides retain
   native precedence. Missing, empty, relative or parent-traversing savepaths
   block launch rather than silently selecting a different default directory.

Invalid configuration produces adapter-specific
`TrainerConfigurationInvalid`; a changed binding at the final spawn boundary
produces the existing spawn error with `InvalidInput`. Ordinary non-trainer
ScummVM launches retain their existing command/detection path.

## Bounds and preservation

The read is capped at 1 MiB plus one overflow probe byte; individual lines
are capped at 8 KiB. Absolute paths are capped at 4 KiB, target names at 96
ASCII identifier characters. Parent traversal, reserved application targets,
missing bindings, invalid UTF-8, control characters, malformed native INI,
duplicate native domains and keys (including game-domain/key case variants)
fail closed.
No value is repaired, unquoted, or interpreted as a filename heuristic.

Linux opens every configuration component with `openat`/`O_NOFOLLOW`; the leaf
is nonblocking and checked as a regular file before reading, so a FIFO cannot
hang preflight. The new trainer validation fails closed on other systems until
equivalent reading and native runtime behavior are proven. This does not
change ordinary ScummVM launch support.

Preflight/spawn validation performs no config writes. No ROM/archive/BIOS,
save or memory-card file is edited. No new savepath is invented or redirected;
existing global/target savepath settings remain in their native INI sections.
The caller must explicitly bind the intended save directory when initializing
an owned trainer config; the launcher never copies or edits the real profile.
Old invalid profiles require an explicit existing preview/apply operation;
launch does not rewrite them automatically. The renderer's persistent writes
remain governed by the existing confirmed transaction/journal/rollback path.

The supplied configuration must still be EmuWiz-owned, as required by the
existing trainer input contract. This repair validates target binding; it does
not establish ownership of arbitrary caller-supplied profile paths or provide
a filesystem sandbox. A concurrent writer after the final check can still
alter what the emulator opens. Immutable launch derivatives, lifecycle cleanup
and full launch isolation remain separate Cheat Core/integration work; no
unmerged interfaces were copied here.

## Runtime evidence (separate from source inference)

Executable `/usr/games/scummvm`: ScummVM **2.8.0**, SDL **2.30.0**.
All HOME/XDG config/data/cache, working directory, logs, media and saves were
under `/tmp/emuwiz-scummvm-binding-proof`. Xvfb supplied a private display;
no real desktop input, user ROMs or valuable saves were used.

| Disposable experiment | Observed result |
|---|---|
| `--config FILE --list-targets` | Exit 1: option requires an argument |
| `-c FILE --list-targets`, native separate IDs | Exit 0; recognized target description |
| Same target with qualified `gameid` | `<Unknown game>`; content run reports invalid game ID and returns to launcher |
| 71-byte synthetic AGI game with native target | Engine loaded; game ran its own quit instruction; exit 0 in about 2.5 seconds, no forced termination |
| 91-byte synthetic AGI game with save then quit | Engine loaded; created `emuwiz-saveproof.001` and `timestamps` in the configured global savepath; exit 0 without forced termination |

Representative repaired invocation, using separate argv:

```text
/usr/games/scummvm -c /tmp/emuwiz-scummvm-binding-proof/saveproof.ini
  -p /tmp/emuwiz-scummvm-binding-proof/synthetic-agi-save
  --logfile=/tmp/emuwiz-scummvm-binding-proof/saveproof-runtime.log
  emuwiz-saveproof
```

The runtime log recorded `Running Fanmade AGI game` and `Emulating Sierra AGI
v2.917`. The synthetic content had no borrowed game data. Files were `logdir`
(three zero bytes), empty pic/view/sound directory index files, `object`
(three zero bytes), `words.tok` (52 zero bytes) and a generated `vol.0` logic resource.
Quit-only logic used opcodes `86 01 00`; save/quit logic used
`72 00 01 AA 00 7D 86 01 00` and one encrypted `EmuWizProof` message.
The logic resource had the standard `12 34` volume header and little-endian
lengths, derived from the official AGI loader. The fixture was detected as
`agi:agi-fanmade` through native fallback detection.

SHA-256 snapshots confirmed all media remained unchanged during both runs.
The save sentinel remained identical; its save-proof digest was
`a53592bec48eb2a0d7d6b0177cecc8b62a1055b59c2a4eddc955114e9ad5ecad`.
The selected disposable config was rewritten with last-selected/version and
engine metadata. Mesa shader cache files and requested logs were generated
under the disposable cache/root. Saves and timestamps were written only to
the specified scratch savepath. No new media-side files appeared.

The Xvfb shell wrapper returned 1 despite child exit 0; its status was not
investigated further. Child exit/logs and stored JSON distinguish emulator
success from the wrapper status. The invalid qualified-ID run was stopped after 20 seconds at the
launcher; its reported exit 0 is not a successful game launch.

These prove native CLI/target/content/savepath behavior with synthetic AGI
content, not gameplay efficacy of Hypno trainer options. No Hypno commercial
content was used; its cheat effects are not claimed runtime-proven here.

## Source inference and references

- [Official command-line documentation](https://docs.scummvm.org/en/latest/advanced_topics/command_line.html)
  describes alternate `-c`/`--config=`, target names and path options.
- [ScummVM 2.8.0 commandLine.cpp](https://github.com/scummvm/scummvm/blob/v2.8.0/base/commandLine.cpp)
  distinguishes native target domains from qualified CLI identities.
- [ScummVM 2.8.0 main.cpp](https://github.com/scummvm/scummvm/blob/v2.8.0/base/main.cpp)
  reads separate engine/game IDs and flushes last-selected target metadata.
- [ScummVM 2.8.0 config-manager.cpp](https://github.com/scummvm/scummvm/blob/v2.8.0/common/config-manager.cpp)
  shows native INI parsing, unresolved duplicate handling and writes to the
  selected config. The validator deliberately rejects ambiguous evidence.
- [AGI loader](https://github.com/scummvm/scummvm/blob/v2.8.0/engines/agi/loader_v2.cpp)
  and native opcode/logic sources supplied the synthetic fixture contract.

Official tagged sources were fetched read-only into the same disposable root.
The persistence results above are runtime observations, not assumptions from
those sources.

## Original candidate validation

Focused ScummVM tests, a targeted core check, formatting and diff hygiene are
the validation scope. No full workspace suite, GUI suite/smoke or release build
was run for this branch.

- `cargo test -p archivefs-core --lib scummvm --offline`: **72 passed**,
  zero failed; includes native command, preflight/spawn, malformed/bounded
  config, source/save preservation, detection, trainer transaction and rollback
  tests, including explicit-savepath and native target-over-global precedence
  regressions. Execution after compilation took 0.33 seconds.
- `cargo fmt --all -- --check`: passed.
- `cargo check -p archivefs-core --lib --offline`: passed on the final code.
- `git diff --check`: passed.
- Exact seven-file scope guard and GUI boundary check: passed; zero GUI changes.
- Recorded runtime-result assertions: passed for engine load, child exit,
  byte preservation, new saves at configured savepath and native config writes.

## Promotion-review savepath repair

Starting reviewed candidate: `eb7252ddbbdb58f5fdc225567aee4356d6f2dfa9`.
The same `fix/scummvm-trainer-launch-binding` branch was retained. Before the
repair, it rebased cleanly onto main `973228c9fb3fbfd1d3dfb4de1121159ebe2aa9ea`.
Only the launch validator, its tests and this report were edited for the repair.

The promotion review reproduced two unsafe acceptances: `[SCUMMVM]` falsely
satisfied the global savepath requirement, and Unicode trimming falsely made
a U+00A0-prefixed value appear absolute. Native runtime respectively saved to
the default profile directory or failed to save. These observations supersede
the original candidate's implicit claim that all accepted savepath bindings
match native parsing.

The fixed rules are derived from the official **v2.8.0** sources, fetched into
`/tmp/emuwiz-scummvm-native-semantics-source`:

- `ConfigManager::addDomain` recognizes the application singleton only by
  exact `scummvm` spelling. `[SCUMMVM]` and `[ScummVM]` are other domains and
  cannot provide a global savepath. `[ scummvm ]` is invalid native syntax.
  Canonical `[scummvm]` and a differently spelled misc domain can coexist;
  only the canonical domain contributes application settings.
- Game/misc target names use the explicitly case-insensitive `DomainMap`
  declared in
  [config-manager.h](https://github.com/scummvm/scummvm/blob/v2.8.0/common/config-manager.h).
  Selected game-target matching now follows that comparison, while ambiguous
  duplicate native domains remain rejected. Reserved trainer target names
  retain the existing conservative refusal.
- Native keys use the case-insensitive `StringMap` in
  [hash-str.h](https://github.com/scummvm/scummvm/blob/v2.8.0/common/hash-str.h).
  Duplicate keys, including identical or case-variant savepaths, are refused
  in either the application or selected target domain.
- [Common::String::trim](https://github.com/scummvm/scummvm/blob/v2.8.0/common/str-base.cpp)
  calls [Common::isSpace](https://github.com/scummvm/scummvm/blob/v2.8.0/common/util.cpp),
  which first rejects bytes outside ASCII and then calls C `isspace`.
  Validation trims only space, tab, LF, CR, VT and FF, never Unicode whitespace.
  The existing strict line/control-character gate still applies. Native ASCII
  padding around keys/values is parsed internally without rewriting the file.
- A U+00A0/U+2003/U+202F/U+3000 prefix stays literal, so a prefixed savepath
  fails the absolute-path gate. Unicode whitespace inside or at the end of a
  genuinely absolute POSIX filename stays literal; no path content is silently
  repaired. The explicit selected target savepath retains precedence over the
  canonical global savepath, and an invalid target value does not fall back.

Six regression tests cover canonical and near-match application domains,
native target-case matching and duplicate targets, ASCII padding, Unicode
prefixes in paths/keys/identity evidence, literal Unicode path preservation,
and duplicate application/target savepaths. The original 72 tests remain in
the focused ScummVM test selection.

### Repaired runtime evidence

ScummVM 2.8.0 / SDL 2.30.0 was retested under
`/tmp/emuwiz-scummvm-savepath-repair-proof/repaired`, with private Xvfb,
HOME/XDG config/data/cache, media, logs and save directories. The existing
91-byte synthetic AGI save/quit fixture was copied only from the disposable
fixture root; no user content or saves were used. SHA-256 snapshots and
assertions checked every case, including all pre-existing save sentinels and
the entire media tree. Results are in that root's `results.json`.

The disposable Rust client links the production `archivefs-core` library,
calls the real preflight API, and invokes `spawn_scummvm` with the reviewed
`-c FILE -p FOLDER TARGET` command. The spawn API revalidates the config using
the same native-config gate. Invalid configs fail both gates with no emulator
process, no profile/cache/save files and no config changes.

**Existing detector limitation discovered:** real ScummVM 2.8.0 `--detect`
prints a qualified-ID table. Current EmuWiz only parses labelled `Game:` /
`Game ID:` records, so valid fixture configs pass the config gate but their
full preflight then refuses with `ScummVmGameIdUnavailable`. This unrelated
parser was deliberately not changed. For accepted-config runtime proof, the
client supplies the fixture's independently observed native
`agi:agi-fanmade` identity directly at the public spawn boundary. Every accepted
case records real native detection output before launch; there is no fake
detector executable or fabricated detector response. This proves the repaired
config/spawn behavior, not a successful end-to-end installed-detector launch.

| Case | Observed config gate / native runtime outcome |
|---|---|
| Canonical `[scummvm]` global savepath | Spawned; exit 0; save and timestamps in `global-saves` |
| `[SCUMMVM]`, no target savepath | Preflight `TrainerConfigurationInvalid`; spawn `InvalidInput`; no process or writes |
| Target-specific savepath, no global savepath | Spawned; exit 0; save and timestamps in `target-saves` |
| U+00A0-prefixed global savepath | Preflight `TrainerConfigurationInvalid`; spawn `InvalidInput`; no process or writes |
| Target-over-global savepath | Spawned; exit 0; writes only in `target-saves` |
| Leading/trailing ASCII space/tab around savepath | Spawned; exit 0; native-trimmed `global-saves` used |
| Mixed-case game target section | Spawned; exit 0; selected `target-saves` used |
| Absolute target savepath ending in literal U+00A0 | Spawned; exit 0; exact Unicode-named save directory used |

No accepted case created a save or timestamp in the disposable default
`data/scummvm/saves` directory. Source media and all existing save sentinels
were byte-identical. Accepted runs rewrote only their owned INI and generated
expected saves/timestamps, logs and disposable Mesa cache. All accepted runs
quit themselves; no forced termination was required. The Xvfb harness also
exited successfully. Hypno gameplay effects remain untested; the original
ownership, concurrent-writer and non-Linux limitations still apply.

### Repair validation

Fresh isolated test target:
`/tmp/emuwiz-scummvm-savepath-repair-target-Ss9xkq`. Test debug symbols were
disabled with `CARGO_PROFILE_TEST_DEBUG=0`; tests remained unoptimized with
debug assertions enabled. No artifacts from the prior review target were reused.

- Focused `cargo test -p archivefs-core --lib scummvm_execution::tests --offline`:
  **17 passed**, zero failures.
- Existing focused selection `cargo test -p archivefs-core --lib scummvm --offline`:
  **78 passed**, zero failures (original 72 plus six regressions).
- `cargo check -p archivefs-core --offline`: passed using the separate fresh
  runtime target `/tmp/emuwiz-scummvm-savepath-runtime-target-vtmfEv`.
- A targeted production library build in that runtime target enabled the
  disposable real-API harness; no release/workspace build was performed.
- `cargo fmt --all -- --check`, `git diff --check` and the exact three-file
  scope guard: passed.
- Runtime assertions for all eight cases: passed.

No GUI smoke, GUI suite or full workspace suite was run.
