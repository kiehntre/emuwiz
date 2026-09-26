# Local Save Snapshots

## Scope

This foundation adds local-only save preservation for GUI-v2. It is deliberately
not a cloud-save or synchronization feature: no account, upload, remote storage,
or background network operation is involved.

## Existing support audited

EmuWiz already had a provider-neutral, read-only persistent-state inventory. It
recognises native saves, memory cards, save states, emulator/profile context,
portability, local provenance, sizes and local SHA-256 values. The inventory
uses remembered emulator profiles and bounded configured roots; symlinks are
refused and it never recursively searches a home directory. PS1/PS2 also has a
specialised Save Vault with verified backup/restore behaviour. Generic restore
transactions, however, were not proven across all adapters, so this feature
does not pretend to provide one.

## Snapshot model

`archivefs_core::save_snapshots` introduces typed `SaveLocation`,
`SaveArtifact`, `SaveSnapshotManifest`, `SaveSnapshot`, `SaveSnapshotRequest`,
and `SaveRestorePlan` values. Artifact types distinguish memory cards, SRAM,
EEPROM, flash saves, VMU, NVRAM, platform memory-card containers, savedata
directories, and emulator save states. A save state is never flattened into an
ordinary in-game save.

Each manifest records game identity when available, platform, emulator and
profile, original path, artifact type, provenance, snapshot time, source size,
relative artifact paths, sizes, modification times, SHA-256 values, format
version, snapshot ID and completeness. The managed root is the existing
EmuWiz application data directory under `snapshots/`, not repository data.

Creation copies in bounded chunks into a `.partial-*` directory, hashes while
copying, rechecks source metadata, writes a complete manifest, syncs files and
publishes with a directory rename. A partial copy is never presented as a
complete snapshot. Sources and originals are read-only. Limits bound the
number of files, total bytes and directory depth; absolute paths and symlink
escapes are refused.

## Restore foundation

Restore planning is read-only. It compares the snapshot with the current
destination, reports added/replaced/unchanged files, detects a newer current
save, and records source-path, emulator, profile, incomplete-snapshot and
emulator-use conflicts. A pre-restore snapshot is always required before any
future apply operation. Generic apply is explicitly disabled until a complete
cross-platform transaction is available, so no newer live save can be silently
destroyed. This also means rollback is represented by the future pre-restore
snapshot rather than an unsafe claim of filesystem undo.

Snapshot and restore work require `NotDetected` emulator use status. `Running`
and `Unknown` refuse safely; the current GUI communicates that the emulator
must be closed and never kills a process automatically.

## GUI-v2 surface

Game Details now has a contextual **Saves & Backups** panel for present local
game media. It explains local backup language, keeps save states distinct, and
links to the existing read-only Saves & States inventory. Compare and Restore
are visible as reserved controls but disabled while generic transactional apply
is unsupported. Advanced details explain local hashing, atomic publication and
emulator-close safety. The main Saves & States page and the established PS1/PS2
Save Vault remain reachable; no existing advanced controls are hidden.

## Space, identity and privacy

Snapshot readiness can be assessed against an observed available-space value;
the implementation requires at least the source size and refuses insufficient
space. Hashes stay local and are used only to verify the copied snapshot and
future restore inputs. Identity is copied as evidence from existing inventory
records; the snapshot layer does not invent a stronger game identity.

## Tests and limitations

Core tests cover single-file SRAM, deterministic directory manifests, save-state
distinction, hash verification, symlink and space refusal, restore-change
preview, emulator-use refusal and profile mismatch. The generic layer still
needs adapter-specific source-lock/process detection and a reviewed transactional
restore implementation before Apply can be enabled. Retention, deduplication,
cloud sync and automatic deletion are intentionally out of scope.
