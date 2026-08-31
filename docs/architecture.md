# Current architecture

## CURRENT BEHAVIOR

EmuWiz is a local-first, Linux-first application for discovering game media,
preserving identity evidence, planning launches and library projections, and
performing explicitly approved local operations. Linux is the primary tested
environment; source archives and direct media remain unmodified.

archivefs-core owns ingestion, evidence fusion, DAT audits, SQLite, emulator
discovery, launch planning/execution, projections, diagnostics, and cheat/mod
transactions. archivefs-cli exposes commands and reports. archivefs-gui
presents the same core models and operations with background work and
stale-result checks. There is no GUI-owned replacement for core identity or
transaction logic.

## Evidence and identity flow

path/archive member/direct media
  -> media/content recognition
  -> structural parsing and platform evidence
  -> verified identity facts where safe
  -> DAT/hash exact release or set authority
  -> persisted evidence and projections

An archive item is a durable library record. Ingestion/content/media
registries describe what was found and how it can be read. Archive-member and
loose-file paths use the same identity pipeline where their evidence is
equivalent. A filename, folder, extension, or weak heuristic is never
promoted to verified game identity merely because it is convenient.

DAT sources provide release identity and hash authority. Audits also persist
set/dependency verdicts where the DAT ecosystem supports them. Cached facts
are explanations and projections: launch and apply paths revalidate the
content needed for their decision.

## Launch flow

Launch is deliberately staged:

1. a compatibility row says that a platform/emulator family may be supported;
2. identity resolution obtains the required verified facts;
3. emulator/profile discovery finds an eligible local installation;
4. readiness/preflight checks content, firmware, paths, and configuration;
5. command planning produces the exact executable and arguments;
6. execution is the separate process-spawn step.

A compatibility row does not create identity and does not authorize launch.
RetroArch core-database matching, reviewed single-entry core preferences,
MAME shortname authority, PCSX2 direct-content restrictions, ScummVM detector
evidence, and the launch-evidence bridge are family-specific rules. See
[LAUNCH_SUPPORT.md](LAUNCH_SUPPORT.md) for the current matrix.

## Persistence and safety

SQLite is an additive catalogue and evidence store, not the live mount or
filesystem safety authority. Read-only catalogue/report commands open it
without creating or migrating it. Scan and explicitly mutating catalogue
commands use the migration-capable path. Applied migrations are immutable;
new schema work is append-only.

Doctor gathers read-only environment, database, profile, managed-path, and
identity findings. Repair and apply commands require their own plan,
confirmation, freshness checks, and safety gates.

## Cheats, mods, and transactions

Provider browsing, source validation, local inventory, and preview are
read-only. Where an adapter supplies a verified materialized source and an
exact destination, selected entries can be applied after explicit
confirmation. The shared engine performs revalidation, safe temporary writes,
backups for replacement, durable journals, verification, and rollback.

PCSX2, Dolphin, RetroArch, GameCube/Wii provider paths, and selected local
mod/texture workflows have different coverage. No adapter is writable merely
because a destination exists; unsupported patch formats and missing identity
fail closed. See [SHARED_SAFE_APPLY_ROLLBACK.md](SHARED_SAFE_APPLY_ROLLBACK.md),
[CHEATS_MODS_SAFETY.md](CHEATS_MODS_SAFETY.md), and
[ADAPTER_SUPPORT_MATRIX.md](ADAPTER_SUPPORT_MATRIX.md).

## Projections

Library Views create managed symlink layouts. Playing Library/1G1R planning
selects and groups evidence-backed entries. RomM mapping/import and ES-DE
export are projections with their own preview/read-only or apply boundary;
none silently changes source identity. See [library-views.md](library-views.md).

## HISTORICAL DESIGN CONTEXT

Older sections in design records may describe flat launch plans, a PCSX2-only
adapter boundary, or preview-only cheats. They remain useful as provenance
but are superseded by the staged launch pipeline, shared identity model, and
transaction-capable adapters described here.
