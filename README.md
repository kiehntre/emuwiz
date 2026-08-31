# EmuWiz

<p align="center">
  <img src="assets/branding/emuwiz-logo-256.png"
       alt="EmuWiz logo"
       width="220">
</p>

EmuWiz helps you turn a messy emulation collection into a verified,
organised, and more playable library.

It identifies supported games from their files and archive contents, checks
them against preservation databases, highlights missing or questionable
files, helps discover and prepare emulator profiles, builds organised
libraries for tools such as RomM and ES-DE, and provides safe cheat, patch,
and selected mod workflows.

Potentially destructive operations are previewed before they run and use
confirmation, verification, and rollback safeguards where supported.

EmuWiz can also mount supported ZIP, 7z, and RAR archives read-only, so you
can browse and use their contents without permanently extracting everything.

It runs locally on Linux. Your collection stays yours.

EmuWiz is alpha software under active development. It does not identify,
launch, or modify every system, emulator, archive, or media format.

## What EmuWiz is and is not

EmuWiz is a local library tool with a command-line interface and desktop
GUI. It works with files you already have and keeps the original collection
separate from generated views, plans, caches, and journals.

It is not a ROM or game download service, storefront, source of BIOS or
firmware, DRM platform, cloud account service, or replacement for an
emulator. It does not distribute copyrighted game content.

## What you can do

### Identify & Verify

- Scan supported archives and direct game media without changing them.
- Identify games using file contents, archive members, metadata, hashes, and
  preservation DAT evidence where available.
- See when evidence is confirmed, probable, ambiguous, or unknown. EmuWiz
  fails closed instead of silently guessing.
- Import and audit DAT sources, including collection-completion reporting and
  managed local snapshot history.

See [platform and media notes](docs/PLATFORM_REGISTRY_AND_DIRECT_IDENTITY.md)
and the [current adapter support matrix](docs/ADAPTER_SUPPORT_MATRIX.md).

### Organise Libraries

- Build named, symlink-based Library Views without moving or copying the
  original collection.
- Plan a Playing Library with canonical names and 1G1R-style selection where
  the available identity evidence supports it.
- Prepare and apply RomM-compatible library layouts through explicit,
  no-clobber transactions.
- Prepare ES-DE launch-entry exports where the current support path allows
  them; EmuWiz does not silently rewrite ES-DE configuration.

See [Managed Library Views](docs/library-views.md) and the
[adapter support matrix](docs/ADAPTER_SUPPORT_MATRIX.md).

### Emulator Setup & Launch

- Discover supported native, Flatpak, portable, and explicitly configured
  emulator profiles.
- Check firmware, identity, content, and profile readiness.
- Preview the command that would be used before launching.
- Execute launches for selected verified targets, including supported
  RetroArch, Dolphin, PCSX2, DuckStation, Flycast, MAME, PPSSPP, RPCS3,
  ScummVM, xemu, and Xenia paths.

Launch support is platform- and emulator-dependent. Some targets provide
readiness or command planning only. Ambiguous or incomplete identity is not
launched. See [Launch support](docs/LAUNCH_SUPPORT.md).

### Cheats & Mods

- Discover emulator profiles and inspect existing cheat or patch files
  without executing them.
- Use trusted or explicitly selected sources where a provider exists.
- Preview compatibility, destinations, backups, and replacements before
  applying anything.
- Apply supported cheat, patch, and Dolphin texture-mod workflows only after
  explicit confirmation. Supported operations use verified transactions,
  journals, and rollback where available.
- Inspect selected local mod packages and build or install supported Dolphin
  texture packs.

Not every cheat, patch, or mod format is supported. Unsupported, ambiguous,
or unverified content remains browse- or inspection-only. EmuWiz never
executes cheat, patch, or mod content.

See the [Cheats & Mods safety model](docs/CHEATS_MODS_SAFETY.md),
[user policy](docs/CHEATS_MODS_USER_POLICY.md), and
[adapter matrix](docs/ADAPTER_SUPPORT_MATRIX.md).

### Doctor & Repair

- Check configuration, source folders, storage, mount tools, databases, and
  emulator profiles.
- Explain what is wrong and what can be done next.
- Plan repairs, renames, duplicate quarantine, and other supported changes
  before applying them.
- Recheck plans immediately before mutation and keep journals for supported
  rollback and recovery paths.

Doctor reports many problems read-only. Repairs are deliberately bounded and
require an explicit user action.

### Mount Without Extracting

- Mount supported ZIP, 7z, and RAR archives read-only through
  [ratarmount](https://github.com/mxmlnkn/ratarmount).
- Browse archive contents without permanently extracting everything first.
- Mount or unmount individually or in bulk, with explicit actions and
  cleanup/recovery checks.

Direct game images are catalogued as files; they are not mounted by EmuWiz.

## Typical workflow

```text
Add games → Identify → Verify → Organise → Configure → Play
```

1. Point EmuWiz at one or more source folders.
2. Scan the collection and review detected identities and problems.
3. Import or select the DAT sources you want to use for verification.
4. Build a Library View, Playing Library, or RomM plan if useful.
5. Check emulator readiness and launch a supported verified game.
6. Review any cheat, patch, mod, repair, rename, or organisation plan before
   confirming it.

## Local-first and safety

- No required cloud account or telemetry.
- Scanning, inspection, identity, and previews are read-only.
- Source archives are not rewritten by library organisation or mounting.
- Existing files are not silently overwritten.
- Network retrieval is limited to explicit supported source workflows.
- Applied changes are verified and journaled where the workflow supports it.
- Rollback is available for supported transactions, subject to the original
  files and destination remaining verifiable.
- EmuWiz does not silently change emulator configuration or execute retrieved
  content.

Read the [security model](docs/security.md) and [security policy](SECURITY.md)
for detailed guarantees and boundaries.

## GUI overview

The desktop GUI provides library browsing, scanning, mounting, sources,
identity and DAT review, Library Views, RomM planning, launch readiness,
Cheats & Mods, Doctor, repair review, duplicates, and History & Logs. It uses
the same core logic and safety rules as the CLI.

The GUI requires a Linux desktop session using X11 or Wayland. There is no
headless GUI mode.

## Quick install

Prebuilt Linux release bundles and checksums are published on the
[Releases page](https://github.com/kiehntre/emuwiz/releases). Choose the
version you want, then run:

```sh
VERSION=v0.8.0-alpha
curl -LO https://github.com/kiehntre/emuwiz/releases/download/$VERSION/archivefs-$VERSION-x86_64-linux.tar.gz
curl -LO https://github.com/kiehntre/emuwiz/releases/download/$VERSION/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
tar -xzf archivefs-$VERSION-x86_64-linux.tar.gz
cd archivefs-$VERSION-x86_64-linux
./install.sh
```

The installer is per-user, uses no `sudo`, and does not edit shell startup
files. Install [ratarmount](https://github.com/mxmlnkn/ratarmount) separately
for archive mounting. Copy [`config.toml.example`](config.toml.example) to
`~/.config/emuwiz/config.toml`, set your source folders and mount root, then
run:

```sh
emuwiz-cli config-check
emuwiz-cli doctor
emuwiz-cli library-scan
```

For manual installation, uninstalling, legacy ArchiveFS compatibility,
foreign-file protection, and upgrade details, see the installer notes in
[`docs/RELEASE_ENGINEERING.md`](docs/RELEASE_ENGINEERING.md).

## Supported systems and media

EmuWiz supports a broad, centrally maintained set of retro consoles,
computers, handhelds, optical systems, arcade formats, and PC game layouts.
Support varies by system: a format may be recognised and catalogued without
having complete identity, DAT, organisation, or launch support.

Recognised media includes ZIP/7z/RAR archives and many direct formats,
including optical images, CHD, CUE/BIN layouts, console and handheld ROMs,
computer and floppy images, executable game layouts, and platform-specific
disk formats. The [media registry](docs/PLATFORM_REGISTRY_AND_DIRECT_IDENTITY.md)
and [current adapter matrix](docs/ADAPTER_SUPPORT_MATRIX.md) describe the
current boundaries; they are more reliable than a hard-coded count or short
extension list.

Important examples:

- CHD identity is supported only for selected optical layouts and platforms.
- ZIP-contained identity is supported for selected archive-member layouts,
  not every ZIP.
- Virtual Boy `.vb` and `.vboy` media are recognised, but that does not imply
  a Virtual Boy launch adapter.
- CUE/BIN to CHD conversion is available only where the optical fingerprint
  can be independently verified.

## Current limitations

- Linux is the supported operating system; macOS and Windows are not
  supported by the current mount and watcher design.
- This is alpha software. Coverage and workflows continue to change.
- Not every platform has complete identity, DAT, organisation, or launch
  support.
- Not every archive, direct-media, cheat, patch, or mod format is supported.
- Some emulators provide readiness or command planning rather than native
  launch execution.
- Cheat and mod installation requires supported content, a valid identity,
  an eligible destination, preview, and explicit confirmation.
- Some content remains browse- or inspection-only, including unsupported or
  ambiguous formats and selected unverified sources.
- EmuWiz does not bundle or distribute games, ROMs, BIOS, firmware, or
  copyrighted patches.
- EmuWiz does not silently edit emulator configuration files.
- ES-DE export and RomM integration are scoped workflows, not universal
  frontend integration.
- A rollback can refuse to act when a destination, backup, or journal no
  longer matches the verified state.

## CLI quick start

```sh
emuwiz-cli config-check
emuwiz-cli doctor
emuwiz-cli library-scan
emuwiz-cli library-list
emuwiz-cli library-find "game name"
emuwiz-cli mount-one "game name"
emuwiz-cli unmount-one "game name"
```

Useful next steps include:

```sh
emuwiz-cli sources
emuwiz-cli source add /data/more-games
emuwiz-cli duplicates
emuwiz-cli view list
emuwiz-cli view preview "By Platform"
emuwiz-cli retroarch-environment
emuwiz-cli retroarch-patch-preview
```

Run `emuwiz-cli --help` for the complete command list. JSON output is
available for several commands; see the [JSON API](docs/json-api.md).

## Advanced and developer documentation

- [Architecture overview](ARCHITECTURE.md) and [full architecture reference](docs/architecture.md)
- [Domain model](docs/domain-model.md)
- [Database design](docs/DATABASE_DESIGN.md) and [recovery diagnostics](docs/DATABASE_RECOVERY.md)
- [Library Views](docs/library-views.md)
- [Adapter support matrix](docs/ADAPTER_SUPPORT_MATRIX.md)
- [Cheats & Mods documentation](docs/CHEATS_MODS_BEGINNER_WORKFLOW.md)
- [Shared preview and safe apply](docs/SHARED_CHEAT_PREVIEW.md) / [rollback](docs/SHARED_SAFE_APPLY_ROLLBACK.md)
- [JSON API](docs/json-api.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

Historical release notes, research, design records, and QA plans are kept in
the `docs/` tree for provenance and are not current capability promises.

EmuWiz is dedicated to [my dad](DEDICATION.md).

## Documentation index

- [Roadmap](ROADMAP.md)
- [Vision](VISION.md)
- [Changelog](CHANGELOG.md)
- [Configuration example](config.toml.example)
- [Launch support](docs/LAUNCH_SUPPORT.md)
- [Current adapter matrix](docs/ADAPTER_SUPPORT_MATRIX.md)
- [Security model](docs/security.md)
- [Contributing](CONTRIBUTING.md)

## Release status

The committed workspace is `0.8.1-alpha` and is in release preparation. It
has not been tagged or published in this checkout. The latest published
release documented here is `v0.8.0-alpha`; use the Releases page to choose an
actually published download rather than assuming the candidate is available.

EmuWiz was previously known as ArchiveFS. Legacy executable names,
configuration paths, data paths, and release artifact names remain supported
for compatibility.
