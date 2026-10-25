# EmuWiz Synthetic Library Lab

The Synthetic Library Lab builds a bounded, deterministic, legally clean
game-library corpus for release QA, regression checks, performance smoke tests
and screenshots. It never downloads anything and uses no real ROMs, BIOS dumps,
firmware, keys, saves, artwork, credentials or user configuration.

It is Python 3 standard-library code (`scripts/qa/synthetic_library.py`) with
small Bash wrappers. It does not modify EmuWiz production code and does not
implement a second scanner, DAT parser, cheat parser or mod installer.

## Quick start

```sh
scripts/qa/build-synthetic-library.sh --profile tiny --output /tmp/emuwiz-lab --seed 1
scripts/qa/validate-synthetic-library.sh /tmp/emuwiz-lab
scripts/qa/remove-synthetic-library.sh --yes /tmp/emuwiz-lab
```

The output folder must be an absolute path to a dedicated child directory. A
`tiny` lab currently has 137 fixtures. Run
`python3 scripts/qa/synthetic_library.py --help` (and `build --help`) for every
option; `EMUWIZ_SYNTH_ROOT` can stand in for `--output`.

To run EmuWiz's own headless scanner over a lab in isolated HOME and XDG
folders:

```sh
EMUWIZ_CLI=/path/to/existing/emuwiz-cli scripts/qa/run-synthetic-scan.sh /tmp/emuwiz-lab
```

The scan harness uses an already-built `emuwiz-cli` (`EMUWIZ_CLI`, then
`target/release`, `target/debug`, then `PATH`). It never runs Cargo. It writes a
disposable config and catalogue under a temporary folder, runs `library-scan`
and `library-list`, and checks the catalogue with SQLite `PRAGMA quick_check`.

## Profiles

| Profile | Purpose | Size cap |
|---|---|---:|
| `tiny` | Fast smoke corpus: representative cartridge, optical, arcade, DAT, cheat, mod, artwork, duplicate, path and safety cases | 20 MiB |
| `standard` | Broad platform coverage plus provider, frontend and error scenarios | 100 MiB |
| `full` | Standard plus per-platform malformed and misnamed cases and archive boundary cases | 500 MiB |

Formats whose complete validity would need proprietary structures are labelled
`DiagnosticOnly`, `WeakCandidate`, `Unsupported` or `Malformed`; the manifest
never turns a file name into false proof.

Optional scale fixtures add tiny logical entries, not large payloads:

```sh
scripts/qa/build-synthetic-library.sh --profile standard \
  --scale 10000 --arcade-sets 10000 --output /tmp/emuwiz-scale-lab
```

`--scale` and `--arcade-sets` are each capped at 50,000. Pass
`--allow-large-scale` to go higher, up to an absolute 250,000. Profile size caps
still apply, and free inodes are your responsibility.

## Safety and ownership

- The output root must be absolute, must not be a symlink, and must not be `/`,
  `/home`, `/mnt`, `/tmp` or your home folder, or sit directly under `/`.
- It must either not exist or be an existing lab that EmuWiz owns, passed with
  `--recreate`. An existing directory that is not an owned lab is never reused
  or deleted.
- Cleanup also needs `--yes`, refuses mount points, refuses a lab whose
  generator process is still alive, and removes only the lab folder. It does not
  follow symlinks out of it.
- Generation starts with `.emuwiz-synthetic-lab.incomplete.json` and a
  `.emuwiz-synthetic-lab.lock/` folder recording the generator's process ID. A
  second generator refuses a root that is locked or incomplete. A failed build
  leaves the incomplete marker so the partial lab can be cleaned up explicitly.
- Success atomically publishes `.emuwiz-synthetic-lab.json` (schema and
  generator versions, repository commit, seed, profile, fixture count and the
  SHA-256 of `manifest.json`) and removes the marker and lock.
- Every fixture path passes a relative-path guard. Symlink targets are relative
  and stay lexically inside the lab. Traversal, absolute and drive-prefix paths
  exist only as inert ZIP member names; the generator never extracts them.

## Determinism

Payload bytes depend only on generator version, seed, fixture ID and a block
counter. ZIP timestamps are fixed and `manifest.json` is canonical JSON with
sorted keys. Two builds with the same commit, generator version, profile and seed
have byte-identical manifests. Wall-clock times appear only in `report.json`.

## Layout

```text
<root>/
  .emuwiz-synthetic-lab.json   ownership marker
  manifest.json  MANIFEST.md  report.json  report.md
  library/          media, extracted sets, archives, path cases
  metadata/         DAT, provider, provenance and election intent
  support/          explicitly fake BIOS-like bytes
  cheats/           invented harmless cheat records
  mods/             archive, patch and emulator-specific packages
  artwork/          code-generated images and corrupt image cases
  documents/        manual and document fixtures
  frontends/        RomM projection and ES-DE fixtures
  source-scenarios/ source-health intent
  scale/            optional tiny logical-entry corpus
```

Empty source folders are intentional and are described in
`source-scenarios/source-matrix.json`. An unreadable source is described as
metadata rather than by changing permissions, so cleanup cannot be stranded.

## GUI recovery fixtures

Every build includes eight deliberately broken or absent files, all named
`ux.*`, for manual GUI recovery testing: truncated 7z, RAR, CBZ and PDF files,
a manual that is present as pages and one that is missing, and a valid and a
corrupt bezel image. They are fixtures for checking that the interface reports
and recovers honestly, never commercial data.

## What validation proves

Validation checks recognised ownership and schema, no incomplete marker, that the
ownership marker and manifest digest agree, every physical fixture's size and
digest, that no unexpected file exists, that symlink targets stay inside the lab,
exact ZIP member lists (without extracting), and that the profile stays under
its cap. It writes `report.json` and `report.md`. A tampered lab makes the
validator exit non-zero. To keep a failed lab for inspection, do not run
cleanup.

## Adding a platform or scenario

1. Use a stable fixture ID; never derive identity from output order.
2. Add files with `Builder.add_file`, `add_text`, `add_zip`, `add_symlink`,
   `add_hardlink` or `add_virtual`, which enforce path safety and record
   manifest evidence.
3. Generate bytes with `Builder.bytes(fixture_id, size)` unless a documented
   structural header is needed.
4. State the weakest truthful identity, health and expectation. Use
   `DiagnosticOnly` or `ManualReviewExpected` for uncertain behaviour.
5. Record relationships for companions, sets, dependencies, duplicates and
   provenance.
6. Add a self-test if the change touches a safety or determinism rule.

Never copy bytes from a commercial game, BIOS, firmware, save, artwork or online
database. If a format cannot be made from public structural facts or repository
logic, record a placeholder or a documented gap.

## Self-tests

```sh
python3 -B -m unittest discover -s scripts/qa/tests -v
(cd scripts/qa && python3 -B -m unittest test_synthetic_ux)
```

The first suite (20 tests) covers deterministic manifests, seed variation, path
and traversal refusal, unowned-directory refusal, ownership-gated cleanup,
dangerous-root refusal, cleanup staying inside the lab (siblings untouched,
symlinks not followed, symlinked roots refused, live generator lock respected,
explicit confirmation required), contained traversal archives, safe symlinks,
profile and scale caps, tamper detection, offline `standard` generation, and
cleanup of an incomplete lab. The second checks the GUI recovery fixtures. All
test labs live in temporary folders.
