# EmuWiz release notes (0.9.0 and the work since)

> **Status: draft.** The workspace version is still 0.9.0, but the `v0.9.0` git
> tag (2026-09-13) was made long before most of what is listed under "New since
> the v0.9.0 tag". Retitle this file and move the list into `CHANGELOG.md` when
> the next version number is chosen. Everything below is on the current
> development branch; nothing here is a promise about a future release.

EmuWiz is a Linux-only, pre-1.0 local game-library tool. It keeps looking,
checking, previewing and changing visibly separate.

## In v0.9.0 (tagged 2026-09-13)

See [`CHANGELOG.md`](../CHANGELOG.md): unified evidence resolution, the DAT
authority dashboard, the Needs Attention workspace, media-set (multi-disc)
inspection, topology-aware launch planning and the first-run guided journey.

## New since the v0.9.0 tag

### A new default interface (GUI v2)

`bin/emuwiz` now opens the task-first GUI v2 interface. The sidebar groups
**Library**, **Play** and **Tools** and has pages for **Games**, **Check Games**,
**Problems & Repair**, **Organisation**, **Duplicates**, **Multi-disc games**,
**Storage**, **Emulator Setup**, **BIOS / Firmware**, **Mods & Cheats**,
**Saves & States**, **Sources**, **DAT Management**, **RomM Library**,
**Activity**, **History** and more. Technical detail is kept under **Details**
or **Why**. Specialist tools that are not native yet open the specialist
interface in a separate window from **Advanced**.

### Fixing and understanding your library

- **Missing games review** (in **Problems & Repair**): previews games EmuWiz can
  no longer find, lets you forget only the ones confirmed missing, and can undo.
- **Duplicates** also finds the *same game stored in two formats* (CUE/BIN and
  CHD, and N64 files with different byte orders) when the contents are proven to
  match. You preview first; the extra copy goes to a recoverable holding folder
  and the move can be undone.
- **Multi-disc games** shows which disc sets look complete, which are missing a
  disc, and which need review. It only reads.
- **Storage** shows where your space is going and what could shrink. It only
  looks.
- **Game Details** explains in plain words why a game is or is not ready to
  launch (emulator, firmware, identity, source drive).
- **RomM Library** is a native, read-only browser for a RomM server you run.
  RomM requests never go through an environment proxy.
- Plain-language wording across the Build/Organisation (including MAME help) and
  Mods pages, and clearer recovery buttons in compact windows.

### Disc conversion

- Wii U `.wud` and `.wux` files are now found by a normal scan. In **Game
  Details** you can convert between them with **Convert this disc image**:
  preview, confirm, then it runs in the background through a queue that survives
  a restart. Your original is never changed or overwritten, and the result is
  reviewable in **History & Undo**.

### Verification data (DATs)

- Arcade verification data can be imported from a local file, or read from a MAME
  you choose. Imported data is labelled as imported, not official, and works
  without an installed MAME. A capture that stops partway or is too large for
  memory is handled safely (large captures are spooled to disk; incomplete ones
  are refused). A folder can be checked against the data without changing it.
- Loading very large DAT collections now uses much less memory. In a synthetic
  benchmark, loading and indexing one million records peaked at about 3.0 GiB
  instead of about 9.3 GiB (results are indicative, from a single machine; see
  [`docs/research/LARGE_DAT_MEMORY_OPTIMISATION_RESULTS.md`](research/LARGE_DAT_MEMORY_OPTIMISATION_RESULTS.md)).

### Mods, cheats and history

- **Mods & Cheats** is native, with a unified history of what was applied and a
  direct way to recover. Texture packs and ordinary mods for supported emulators
  are previewed before they are applied.

### Saves

- **Saves & States** is a read-only overview of saves, memory cards and
  savestates in your configured emulator folders.
- Supported PS1/PS2 memory-card actions (inspect, export, and PS2 restore with a
  verified card backup and undo) live in the PS1/PS2 Save Vault in the
  specialist interface.

### Release and QA tooling (for maintainers and testers)

- A canonical `emuwiz-<version>-linux-<arch>.tar.xz` package with a manifest and
  SHA-256 checksums, a user-level `install.sh`, an optional offline CycloneDX
  SBOM and licence bundle, and optional detached GPG signing. See
  [Verify a release](VERIFY_RELEASE.md).
- A reproducibility check that builds the same commit twice in isolated
  checkouts and compares the archives (`scripts/compare-release-builds.sh`).
- Release acceptance and packaged-GUI smoke checks, a disposable release smoke
  harness, a read-only upgrade preflight and a read-only pending-operation
  recovery inspector.
- A deterministic synthetic game library for testing without real files.

## Known limitations

- Linux only. There is no Windows or macOS build.
- There is no universal emulator installer, updater or BIOS downloader, and no
  universal save restore. Savestates stay tied to their emulator and version.
- User-supplied remote and custom DAT sources are in the core library but have no
  screen yet. They cannot be added from the app.
- Automatic emulator disc swapping is not implemented; **Multi-disc games** is a
  read-only review.
- Multi-part Wii U `.wud` sets are not grouped: each part appears as its own
  entry. The conversion card sits inside the collapsed **Disc & ROM evidence**
  section of **Game Details**.
- PS3 saves are not inspected or restored.
- Some specialist tools remain in the specialist interface (**Advanced**).
- Drives, absolute paths, emulator profiles, DATs and firmware are still your
  environment's responsibility. EmuWiz cannot make a missing drive or missing
  system software valid.

For setup and upgrades, see [Quickstart](QUICKSTART.md) and
[Upgrading](UPGRADING.md). To check a download, see
[Verify a release](VERIFY_RELEASE.md).
