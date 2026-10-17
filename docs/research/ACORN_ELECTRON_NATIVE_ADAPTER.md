# Recovered Electron inspection in Blue

Recovery baseline: freshly fetched GitHub main
`ac0a7572b826e5670360bece710b1b24ebbc7304`. Historical source:
`90ad028d8a4c3ec508b28b8f048a5e1a95ad56bf` on
`feature/acorn-electron-native-adapter-foundation`.

## Current integration and evidence limits

Already implemented on main: Electron registration/aliases/artwork, BBC versus
Electron family ambiguity, standard DFS and UEF parsers, bounded native executable
discovery, source hashing/revalidation and Blue Game Details. Missing and recovered:
the historical Elkulator preview adapter/tests, a preview-only native registry
entry, and a focused Blue view composing those existing inspection facilities.
No new platform identity, scanner, parser, storage or launch subsystem is added.

Open an Electron game (or shared SSD/DSD/UEF media) from Games, then expand
**Acorn media · Elkulator preview-only** in Game Details. Choose **Inspect /
re-inspect Acorn media**. Mr Wiz explains the platform ambiguity; the view shows
media type, structural/completeness evidence, source path/size/SHA-256, and actual
Elkulator executable discovery observations. Advanced evidence remains an
inspection snapshot. Library identities, DAT evidence and file paths are never
updated. BBC evidence is never converted into Electron evidence.

Inspection supports standard DFS SSD (one 40/80-track side), interleaved DSD (two
40/80-track sides), and plain or gzip-wrapped UEF containers through the existing
parsers. Unknown UEF chunks and absent/unsupported data remain incomplete; a
well-formed container is not a compatibility or boot result. Other media,
malformed/truncated files and unsafe paths are refused with an explanation.

The private source-binding plan is read-only and never exposed for preparation.
Existing limits remain: 32 MiB source binding, 8 MiB UEF input, 16 MiB decoded UEF
and 65,536 UEF chunks; DFS only accepts standard image geometry. Filesystem reads,
hashes, parser work, discovery and freshness rechecks run off the GUI thread.
Displayed evidence is bounded. After parsing, the source is re-hashed before a
result is published. While the panel is visible, background freshness checks run
every two seconds; previous evidence is labelled as a snapshot while checking.
A changed/missing source is marked stale and requires explicit re-inspection.
The worker's check timestamp is retained, so delayed replies cannot look newly
verified. These are point-in-time observations, never ongoing launch authority.

Elkulator availability is based only on bounded native discovery of regular
executable files on the checked PATH. Symlinks follow main's existing refusal
policy. Nothing is executed, probed with flags, downloaded or automatically
selected. Failure to find a PATH executable is not proof that no custom install
exists. Inspection itself needs no emulator.

Elkulator remains **PreviewReadinessOnly** and is absent from execution target
mapping. No command, preparation, spawn, writable-copy creation, emulator setting
change, media write or compatibility claim is exposed. The historical containment,
firmware and writable-media questions below remain unverified. A structurally
recognised file cannot create a runnable candidate. Future launch behaviour is
BLOCKED; this recovery does not establish it.

The pinned historical audit below is retained as provenance. Its upstream findings
and dates are historical source observations, not a new installed-build audit or
certification. Preserve the historical branch and unique commits until the normal
cleanup process accounts for them; semantic recovery alone does not delete them.

---

# Acorn Electron native adapter foundation

Audited 2026-10-02. Starting main and origin/main:
`006f391169b4c23484f26acca56a23234467883d` (authoritative repository
`/home/davedap/archivefs`). Classification: **PreviewReadinessOnly**.
This candidate validates and binds inputs; it cannot prepare or execute a launch.

## Authoritative upstream and provenance

Elkulator is a dedicated Acorn Electron emulator. The community upstream is
[stardot/elkulator](https://github.com/stardot/elkulator), descended from Sarah
Walker's original project, whose [official site](https://elkulator.acornelectron.co.uk/)
still advertises the 2010 Linux release. The newer development integration
branch audited for this candidate is
[demrepofdave/allegro5_integration](https://github.com/demrepofdave/elkulator/tree/demrepofdave/allegro5_integration).
Do not confuse that branch with either repository's older default `master`.
The audit identifies published source activity; it does not promise ongoing
maintenance beyond the observed commits.

| Source inspected | Exact revision | Commit date | Role |
| --- | --- | --- | --- |
| stardot/elkulator master | `6cab45aba68fc3d3bdaea4c28b5de4de0307e00e` | 2024-09-08 | Community baseline, Allegro 4 |
| demrepofdave/elkulator, demrepofdave/allegro5_integration | `6785521aba2c237861f29d9dee9cfc6725989b1e` | 2025-05-29 | Newer v2 / Allegro 5 development source, optional Allegro 4 build |
| demrepofdave/elkulator, demrepofdave/al5int/joystick_support | `94c821ddb04e279cecc70396028499cfe0b47aa9` | 2025-05-31 | Newer feature head; diff against integration changes joystick/host handling only, none of the audited CLI/config/media/ROM/state/path routines |
| dmcoles/elkulator master | `17b17e7d298975a2c10852b22b9e2d5afc3ca1e5` | 2015-08-14 | Older fork; not the current design authority |

Published community/fork branch refs and dates were checked directly with Git;
source-only sparse checkouts were read.
No upstream build scripts, emulator binary or firmware were executed. No
Elkulator executable was found on this environment's PATH. These are source
findings, not a real-emulator launch certification.

The existing [BBC audit, Task C](BBC_MICRO_NATIVE_ADAPTER_AUDIT.md#task-c--machine-coverage)
establishes separate BBC Micro and Acorn Electron platforms and excludes Electron
from b-em. This candidate does not change the b-em adapter.

## Linux and machine coverage

The development [README](https://github.com/demrepofdave/elkulator/blob/6785521aba2c237861f29d9dee9cfc6725989b1e/README.md)
documents Linux builds with Allegro 5, OpenAL, ALut and zlib, plus a legacy
Allegro 4 build. It describes a base 32 KiB Electron, Elektuur/Slogger turbo
board, Slogger/Jafa Master RAM board with off/turbo/shadow modes, Plus 1,
Plus 3 and ROM cartridges. These are Electron expansions, not BBC Micro models.
Build instructions exist for Ubuntu/Debian and Fedora; the Fedora subsection
contains inconsistent package-manager commands, so it is not an installation
recipe supplied by EmuWiz.

Expansion and memory selections come from config/menu state, not a documented
machine-selection CLI. The relevant config fields include `plus1`, `plus3`,
`dfsena`, `adfsena`, `turbo`, `mrb` and `mrbmode`.
[Config loader](https://github.com/demrepofdave/elkulator/blob/6785521aba2c237861f29d9dee9cfc6725989b1e/src/config.c#L163)
is the authority for those fields. No unimplemented expansion choice is added
to the preview profile.

## Executable CLI and deterministic attachment

Both audited generations implement these token sequences:

```text
elkulator -tape /absolute/path/tape.uef
elkulator -disc /absolute/path/disc.ssd
elkulator -disc1 /absolute/path/second.dsd
elkulator -rom BANK /absolute/path/cartridge.rom
```

`-disk` is a source-supported alias of `-disc`; `-disc` selects drives 0/2
and `-disc1` selects 1/3. The parser also handles `-debug` and Linux serial/
parallel flags. `--help` exits unsuccessfully and is reached after config
loading (and in the newer source after host initialisation), so discovery does
not run it. No `--config`, `--rom-dir`, `--readonly`, `-c`, positional-media
shortcut or auto-typing flag is invented here.
[v2 parser and attachment calls](https://github.com/demrepofdave/elkulator/blob/6785521aba2c237861f29d9dee9cfc6725989b1e/src/main.c#L133)
and [master parser](https://github.com/stardot/elkulator/blob/6cab45aba68fc3d3bdaea4c28b5de4de0307e00e/src/main.c#L70)
show one well-formed media argument attaches the specified path without a GUI
chooser. Attachment is not automatic tape boot: the README describes typing
`CHAIN` or `*RUN` after loading. Fixed-buffer copying in the parser also needs
an installed-build/path-length contract before execution is offered. The
preview exposes no argv at all.

## Media support and source-write behaviour

| Media | Upstream source evidence | This preview |
| --- | --- | --- |
| UEF / HQ-UEF | Tape loader uses `gzopen(..., "rb")`; gzip/plain UEF input; README describes read-only tape | Bounded existing UEF structural parser; plain and gzip accepted; this is container validation, not a promise of every chunk's playability |
| SSD | DFS loader, read/write with read-only fallback | Existing standard DFS inspector, one side |
| DSD | Interleaved two-sided DFS loader, same write risk | Existing standard DFS inspector, two sides |
| CSW | Dedicated tape reader, `fopen(..., "rb")`; README supports it | Refused; no new CSW validation/launch audit |
| WAV | No WAV tape decoder in the audited load path; WAV references are sound-effect assets | Refused |
| ROM | Bank/cartridge loading reads raw files | Refused; no ROM-media validation or firmware provisioning contract |
| Snapshot | GUI `.snp` load/save; `ELKSNAP1` header, explicit file read/write | Refused; no snapshot import or launch CLI |
| ADF / ADL / FDI | Present in upstream disc loader table | Refused; outside this preview's audited media subset |

Primary media authorities:
[disc/tape routing](https://github.com/demrepofdave/elkulator/blob/6785521aba2c237861f29d9dee9cfc6725989b1e/src/disc.c#L40),
[UEF open](https://github.com/demrepofdave/elkulator/blob/6785521aba2c237861f29d9dee9cfc6725989b1e/src/uef.c#L32),
[CSW open](https://github.com/demrepofdave/elkulator/blob/6785521aba2c237861f29d9dee9cfc6725989b1e/src/csw.c#L29),
[snapshot read/write](https://github.com/demrepofdave/elkulator/blob/6785521aba2c237861f29d9dee9cfc6725989b1e/src/savestate.c#L24).
Passing a WAV to `-tape` would route it toward CSW based on its extension;
that does not establish WAV support. Disc-loader size guesses likewise do not
establish identity or acceptable structure.

`ssd_load` and `dsd_load` first open `rb+`, falling back to `rb` only on failure.
Sector writes and formatting can call `ssd_writeback`, which writes the loaded
image. Menu write-protect and `defaultwriteprotect` are mutable config/UI
state, not a proven read-only CLI contract.
[SSD/DSD opens and write-back](https://github.com/demrepofdave/elkulator/blob/6785521aba2c237861f29d9dee9cfc6725989b1e/src/ssd.c#L33)
establish the source-write risk. A future disk launch must hash/revalidate the
source before a private scratch copy, pass only that copy, discard it and
never sync writes back. None of those launch operations is exposed now.
Tape input is read-only upstream; this preview creates no copy of any media.
The shared preview's private prospective scratch plan is provenance only,
not an executed copy policy for tape.

## Config, state, firmware and exit containment

The config loader/saver concatenates `exedir` with `/elk.cfg`; saving opens
`wt` and persists machine settings and mounted disk paths. `closeelk` calls
`saveconfig` on normal exit. There is no config-directory option or HOME/XDG
selection in those routines.
[Config save](https://github.com/demrepofdave/elkulator/blob/6785521aba2c237861f29d9dee9cfc6725989b1e/src/config.c#L309)
and [close/initialisation](https://github.com/demrepofdave/elkulator/blob/6785521aba2c237861f29d9dee9cfc6725989b1e/src/main.c#L332)
show why redirecting HOME alone cannot certify containment.

Crucially, the two implementations differ:

- Master/Allegro 4 obtains the executable filename, removes the filename and
  uses that directory. Changing cwd or HOME does not select a private config
  in this audited application contract.
  [Master initialisation](https://github.com/stardot/elkulator/blob/6cab45aba68fc3d3bdaea4c28b5de4de0307e00e/src/main.c#L75).
- The development Allegro 5 wrapper named `fileutils_get_executable_name`
  actually creates an Allegro path from `.`. Its root is relative, making cwd
  relevant; it does not obtain the executable directory. This is a possible
  containment route for that specific build, not proof for every binary named
  Elkulator. Path concatenation, resource provisioning and exit behaviour
  have not been confirmed by bounded execution.
  [Allegro 5 implementation](https://github.com/demrepofdave/elkulator/blob/6785521aba2c237861f29d9dee9cfc6725989b1e/src/host_abstraction_layer/allegro_5/fileutils.c#L28).

`loadroms` constructs an `exedir`-relative `roms` path, attempts `chdir`, and
unconditionally reads `os`, `os300.rom`, `basic.rom`, `adfs.rom`, `dfs.rom`,
`sndrom` and `plus1.rom`, including ROMs for disabled expansions. The current
loader exits on a missing ROM. Its directory change is not checked; a safe
future contract must verify resource layout and failure behaviour rather than
trust a guessed cwd/HOME arrangement. ROM inputs are opened read-only, not
saved by this loader. Sound resources also use `exedir`-relative directories.
[ROM loader](https://github.com/demrepofdave/elkulator/blob/6785521aba2c237861f29d9dee9cfc6725989b1e/src/mem.c#L70).
No ROMs are bundled, copied, downloaded or otherwise provisioned by this adapter.

No separate automatic CMOS-file persistence was found in either audited
application tree. This does not establish absence of all state writes:
snapshots are explicitly writable to user-selected paths, screenshots/movie
outputs exist, and CSW code contains cwd-relative diagnostic-file handling.
The generic scratch helper is preservation infrastructure, not an OS security
sandbox. Without a proven selected-build config/resource/state contract,
actual execution remains unavailable.

## EmuWiz identity and implementation contract

Only `launch/elkulator.rs`, its synthetic tests and this document are added.
The implementation reuses `native_support::discover_executables`,
`plan_preview`, `read_prefix`, the existing DFS inspector, `uef_tape::parse_uef`
and the safe-launch provenance/limits. No media parser, new dependency,
unsafe code, identity authority or generic launch behaviour is introduced.

The caller supplies a resolved canonical identity with platform exactly
`Acorn Electron`, a nonblank game key, `Verified` platform evidence and the
verified SHA-256 of these same source bytes. A selected executable/profile
does not manufacture platform or canonical identity. Existing upstream
resolution may incorporate a person's explicit platform selection under its
own policy; this module accepts its resolved result and never overrides
`Unknown`, `Conflicting` or `BBC Micro`.

DFS evidence identifies a shared BBC/Electron family; the disk inspector's
legacy platform label is not used as an Electron decision. The expected
SSD/DSD side count must also agree with the inspected layout. UEF parsing
retains its 8 MiB input, 16 MiB decoded-output and chunk-count limits; a UEF
magic prefix alone is insufficient. Shared preview planning retains source
symlink refusal, source-size bounds, hash binding and executable freshness.

`NativePreview` always reports `PreviewReadinessOnly`, blocked readiness and
`is_launchable() == false`. Only provenance is exposed; its `SandboxPlan` is
private. There is no adapter prepare/spawn/argv API, no config write, no media
write and no automatic executable probing.

## Collisions and deferred integration

Before editing, 428 registered worktrees were inspected. `launch/mod.rs` was
dirty in these 11 worktrees: `archivefs`, `archivefs-fsuae-adapter`,
`archivefs-vice-c64-adapter`, `emuwiz-082-batch3-launch`,
`emuwiz-082-batch3-launch-clean`, `emuwiz-082-batch4-dat-dryrun`,
`emuwiz-arcade-readiness`, `emuwiz-custom-dat-lifecycle`,
`emuwiz-scummvm-dosbox-readiness`, `emuwiz-snes9x-stella-readiness` and
`emuwiz-vita3k-readiness` (all under `/home/davedap`). Unrelated support-file
dirt was also left untouched.

Another writer subsequently added `launch/elkulator.rs`,
`launch/elkulator/tests.rs` and a `launch/mod.rs` registration in the initial
`/home/davedap/emuwiz-acorn-electron-native-adapter` worktree. The user explicitly
instructed preservation of that work and a separate fresh worktree. This
candidate is therefore on `feature/acorn-electron-native-adapter-foundation`
in `/home/davedap/emuwiz-acorn-electron-native-adapter-foundation`.
A follow-up scan of 431 registered worktrees confirmed those collisions and
only the three intended additions in this candidate worktree.

Registration in `launch/mod.rs` and the native adapter registry is deliberately
deferred under the user's central-file collision exception. The new module
is self-contained and separately compiled/tested; normal workspace builds
do not yet discover it or expose Electron routing. Once registration ownership
is clear, add the Linux-gated module declaration and a preview-only registry
entry, with the existing registry tests reviewed for that separate integration.

## Reproduce the unregistered adapter tests

From this candidate worktree on Linux, use the existing Cargo dependencies;
the temporary harness registers the module without editing `launch/mod.rs`.

```sh
export EMUWIZ_ELECTRON_VALIDATION_DIR="$(mktemp -d /tmp/emuwiz-electron-check.XXXXXX)"
export CARGO_TARGET_DIR="$EMUWIZ_ELECTRON_VALIDATION_DIR/target"
cargo --config 'profile.dev.package.archivefs-core.debug=0' build --offline --locked -p archivefs-core --message-format=json > "$EMUWIZ_ELECTRON_VALIDATION_DIR/build.jsonl"
python3 - <<'PY'
import json, os, subprocess
from pathlib import Path
task = Path(os.environ['EMUWIZ_ELECTRON_VALIDATION_DIR'])
target = Path(os.environ['CARGO_TARGET_DIR'])
artifacts = {}
for line in (task / 'build.jsonl').read_text().splitlines():
    item = json.loads(line)
    if item.get('reason') == 'compiler-artifact':
        for name in item.get('filenames', []):
            if name.endswith('.rlib'):
                artifacts[item['target']['name']] = name
module = Path.cwd() / 'crates/archivefs-core/src/launch/elkulator.rs'
prefix = '''extern crate archivefs_core;
pub use archivefs_core::{disk_format, identity_source, safe_read, tape_analysis, uef_tape};
mod launch {
    pub use archivefs_core::launch::{native_support, planning, readiness, safe_launch_sandbox};
'''
source = task / 'elkulator_harness.rs'
source.write_text(prefix + '#[path = ' + json.dumps(str(module)) + ']\npub mod elkulator;\n}\n')
command = ['rustc', '--edition=2024', '--test', str(source), '-o', str(task / 'elkulator-tests'),
           '-L', 'dependency=' + str(target / 'debug/deps')]
for name in ['archivefs_core', 'tempfile', 'sha2', 'flate2']:
    command += ['--extern', name + '=' + artifacts[name]]
subprocess.run(command, check=True)
subprocess.run([str(task / 'elkulator-tests')], check=True)
for name, expression, diagnostic in [('prepare', 'p.prepare()', 'E0599'),
                                    ('spawn', 'p.spawn()', 'E0599'),
                                    ('argv', 'p.argv()', 'E0599'),
                                    ('sandbox', 'p.scratch', 'E0616')]:
    refused = task / ('refused_' + name + '.rs')
    refused.write_text('#![allow(dead_code, unused_imports)]\n' + source.read_text()
                       + '\npub fn denied(p: launch::native_support::NativePreview) { let _ = '
                       + expression + '; }\n')
    check = ['rustc', '--edition=2024', '--crate-type=lib', '--emit=metadata', str(refused),
             '-o', str(refused.with_suffix('.rmeta')), '-L', 'dependency=' + str(target / 'debug/deps'),
             '--extern', 'archivefs_core=' + artifacts['archivefs_core']]
    result = subprocess.run(check, capture_output=True, text=True)
    assert result.returncode != 0 and diagnostic in result.stderr, result.stderr
    print('PASS: refused NativePreview ' + name + ' access (' + diagnostic + ')')
PY
```

Run `cargo --config 'profile.test.package.archivefs-core.debug=0' test --offline --locked -p archivefs-core --lib launch::`,
`cargo check --offline --locked --workspace`, `cargo fmt --all -- --check`,
targeted `rustfmt --edition 2024 --check` for both new Rust files (unregistered
files are not visited by Cargo fmt), and `git diff --check`.

## Candidate validation

The standalone harness compiled the actual candidate module against the
starting main core library: **23 tests passed**, none failed/ignored (2.70 s).
Cases cover executable discovery/ambiguity/refusals, resolved Electron and
refused BBC/unknown/conflicting identity, shared DFS ambiguity, valid SSD/DSD,
plain/gzip UEF, malformed/oversized/decompression-limited UEF, unaudited media,
SHA-256 binding/source drift, executable drift, symlink refusal, acknowledgement,
config and write-risk classification, and absence of source/config/workspace/
process changes during preview.

Four additional compiler refusal checks passed: `.prepare()`, `.spawn()` and
`.argv()` on the returned `NativePreview` fail with E0599, and access to its
private `.scratch` plan fails with E0616. They compiled the same wrapper plus
a function attempting each access; no executable was produced.

Core library and test compilation use package-specific debug-symbol overrides
to bound memory while other lanes compile. Test/debug assertions stay enabled;
the workspace check uses the standard requested command.
The existing `archivefs-core --lib launch::` suite passed **992 tests**, with
no failures/ignored tests (12.36 s; 9,796 unrelated tests filtered out).
The unregistered Electron tests are separate from that total.
`cargo check --offline --locked --workspace` passed (3m 43s), with four existing
GUI warnings in unchanged files. `cargo fmt --all -- --check`, targeted
`rustfmt --edition 2024 --check` for the unregistered module/tests,
`git diff --check` and the task scope guard passed. The candidate contains
only the three named additions; `launch/mod.rs` matches starting main.
No main modification, promotion or push is part of this task.

Remaining blockers to execution: a proven selected-build CLI/config/state and
firmware/resource contract, verified private disk-copy launch/cleanup with no
write-back, and clear ownership for the deferred registration. Source evidence
alone is not converted into a verified launchable classification.

## Recovery validation

Fixtures are synthetic; no emulator or firmware is executed. The recovered tests
exercise SSD/DSD/UEF, gzip UEF, bounds and malformed input, wrong/ambiguous platform
identity, missing/unsafe executable discovery, source/executable drift, and the
absence of configuration writes, media writes, scratch preparation or processes.
Additional tests inspect without an emulator, expose incomplete UEF containers,
recheck changed/missing sources, retain worker-side timestamps across delayed
replies, preserve existing platform identity, and keep Elkulator outside execution
mapping. The native adapter registry must retain its existing launchable set.

Independent validation executed the 30 core tests and five new Blue tests
successfully. Existing Game Details and Launch readiness-summary tests also pass.
Focused parser/platform/native-discovery checks and exact-commit review are
recorded with the final promotion evidence outside the source tree. Validation is
synthetic and headless, not certification of a real Elkulator build or game.
