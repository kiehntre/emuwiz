# EmuWiz architecture

EmuWiz is a local-first, Linux-first library and launch platform. It keeps
source archives and media untouched, builds a shared evidence model, and lets
the CLI and GUI use the same core planning, persistence, diagnostics, and
execution logic. The Rust crate names retain the archivefs-* compatibility
namespace.

The detailed references are [docs/architecture.md](docs/architecture.md),
[docs/domain-model.md](docs/domain-model.md),
[docs/LAUNCH_SUPPORT.md](docs/LAUNCH_SUPPORT.md), and [ROADMAP.md](ROADMAP.md).

## System shape

sources / archives / direct media
  -> ingestion, media recognition, structural evidence
  -> identity evidence, verified facts, DAT/hash authority
  -> catalogue, launch/readiness plans, library plans
  -> CLI and GUI, emulator execution, views, Playing Library, RomM, ES-DE

archivefs-core owns this logic. archivefs-cli and archivefs-gui are thin
presentation and interaction layers. There is no separate daemon or
GUI-owned identity system.

## Architectural boundaries

- Source archives are read-only. Inspection may read an archive member or
  direct image, but does not rewrite the source.
- Filename, folder, extension, and weak platform hints are evidence only.
  They never silently become verified game identity.
- The catalogue persists observations, identity reports, DAT results, set
  verdicts, and verified facts as useful, freshness-aware projections. Fresh
  launch and apply paths revalidate the relevant content.
- Launch separates compatibility, identity, emulator/profile discovery,
  readiness, command planning, and process execution. See
  [docs/LAUNCH_SUPPORT.md](docs/LAUNCH_SUPPORT.md).
- Cheats and mods separate discovery/provider browsing/preview from selected,
  verified apply. Supported apply paths use shared transaction, journal,
  backup, verification, and rollback machinery.
- Doctor and database-check are diagnostic/read-only paths; repair and other
  explicitly confirmed operations are separate.

## Main subsystems

Ingestion, media registries, archive-member readers, and format observers turn
paths into bounded content and platform evidence. Identity fusion combines
that evidence with verified direct-media facts and DAT/hash results. SQLite
persists the catalogue and its evidence lineage.

The launch subsystem discovers emulator profiles, resolves verified identity,
computes readiness, plans a command, and only then executes it. The support
matrix is the current reference for supported families; coverage is not
uniform across platforms or emulators.

Playing Library and Library Views are plans over the evidence-backed library.
RomM and ES-DE are projections/exports, not alternate identity authorities.
A current, exact DAT verification outranks RomM's platform: RomM disagreeing is
reported for review and never retires the verified assignment (manual choices
still outrank both). RomM only fills a platform a game does not have. Beside any
current platform (a scan's, a manual choice, or RomM's own earlier answer)
agreeing writes nothing, and disagreeing is shown for review and never applied.
The RomM client is local-path-aware and read-only toward RomM; publishing an
ES-DE projection is a separate local operation. The one exception is optional
and off by default: with "Rename also updates RomM's title" turned on (and a
separate title-update token carrying `roms.write`, used for nothing else; the
read-only token stays the one for browsing and import, and may not be the same
file), a Rename changes the displayed title
of that game's RomM entry, and History Undo puts it back
(`identity_source::romm::title_sync`). It writes only when the entry is the one
EmuWiz's RomM import links to that exact file (by mapped path, with size and
hashes re-checked live), RomM never identified the game, and its title is still
the one RomM derived from one of the file's names (the current one or one
Rename History proves). It sends only the title; Undo restores the recorded
original only while RomM still shows exactly the title EmuWiz wrote. Nothing
else in RomM is ever written, and no RomM failure undoes the local rename.

The patch-manager subsystem contains provider/source validation, local
inspection, mod-package planning, adapter-specific materialization, and the
shared safe-apply pipeline. Unsupported formats fail closed. A local mod
package can be inspected and planned without implying that its patch format
is executable or apply-capable.

## Historical design context

Older documents and release notes may call the project ArchiveFS, describe a
flat launch_plan, or frame PCSX2 as the only adapter. Those descriptions are
provenance, not the current ownership model; current behavior is defined by
the references above and the live registry/support matrix.
