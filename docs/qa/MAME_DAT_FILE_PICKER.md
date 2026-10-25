# MAME DAT file picker QA

## Delivery

- Starting product SHA: 8c19bd0378699a2c47b03f16b4ccd7de57698814
- Result: uncommitted working tree on that lineage; no push or commit performed.
- Scope: native GUI v2 DAT Management only. The existing registry and Save path remain authoritative.
- Picker: existing native rfd::FileDialog, reused with .dat/.xml filtering.

## Implementation

The MAME Arcade source ID mame-0-174-arcade-xml now offers Choose DAT file….
Selection creates an in-memory candidate only. The card displays filename, path,
size, ecosystem, detected version, SHA-256, and contract validation. Only
Use this DAT changes the existing draft entry; the existing Save action
persists it through save_dat_sources_config_to.

The source ID, display name, enabled state, priority, ownership, provenance,
and unrelated registry entries are retained. Cancel and Revert discard the
candidate without changing the registry.

Candidates are rejected for empty/partial files, unreadable files, HTML or
challenge content, incomplete XML, MAME ecosystem/version mismatch, and the
MAME 0.174 contract SHA mismatch. The strict existing
load_verified_mame_0174 verifier remains the final acceptance gate.

## Focused tests

- mame_replacement_rejects_empty_and_partial_files
- mame_replacement_rejects_hash_mismatch_without_staging_a_source_change
- applying_mame_replacement_updates_only_existing_path_after_explicit_action

All passed. The full GUI library suite passed: **2,837 passed, 2 ignored**.
GUI package Clippy with -D warnings passed.

## Real :0 QA

Authenticated session variables:

    DISPLAY=:0
    XAUTHORITY=/home/davedap/.Xauthority
    XDG_RUNTIME_DIR=/run/user/1000
    DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus

Observed workflow:

1. Opened native emuwiz 0.9.0 · GUI v2 (native-v2).
2. Opened DAT Management and the existing MAME.0.174.Arcade.XML.dat source.
3. Clicked Choose DAT file…; the native GTK file picker appeared.
4. Selected /home/davedap/DATs/MAME/MAME.0.174.Arcade.XML.dat.
5. The preview showed 51.2 MiB, ecosystem MAME Arcade, version 0.174,
   and SHA-256
   df9938254e6299a9dc0499ac4d30ef562730d5e1e1d0f8f887402948980fae27.
6. Clicked Use this DAT, waited for validation, and observed Valid,
   32,071 entries, 253,236 ROMs · Logiqx XML.
7. Clicked the existing Save; the GUI displayed Catalogue sources saved.
8. Restarted the GUI and confirmed the registry reload was clean with no
   unsaved changes.

Screenshots:

- [native MAME picker](</tmp/emuwiz-mame-picker-dialog.png>)
- [validated candidate preview](</tmp/emuwiz-mame-picker-result.png>)
- [valid source before Save](</tmp/emuwiz-mame-picker-applied.png>)
- [Save confirmation](</tmp/emuwiz-mame-picker-saved.png)
- [post-restart DAT Management](</tmp/emuwiz-mame-picker-reloaded.png)

The configured path was already the requested replacement path when the first
live config read was taken, so this QA run exercised GUI-only selection,
validation, explicit apply, health refresh, persistence, and reload rather than
changing an old path value. The MAME source path and metadata after Save were:

    id: mame-0-174-arcade-xml
    path: /home/davedap/DATs/MAME/MAME.0.174.Arcade.XML.dat
    health: valid
    entries: 32071
    ROMs: 253236

## Source integrity

The representative DAT SHA-256 was the requested value both before and after
the GUI flow:

    df9938254e6299a9dc0499ac4d30ef562730d5e1e1d0f8f887402948980fae27

The DAT bytes were read and parsed only; no source media was written.

## Validation commands

- cargo check -p archivefs-gui --lib
- focused replacement tests
- cargo test -p archivefs-gui --lib
- cargo clippy -p archivefs-gui --lib -- -D warnings
- cargo build --release -p archivefs-gui
- git diff --check

The repository-clean release wrapper was not usable because this worktree
contains the intentionally uncommitted GUI-v2 theme-consolidation lineage;
the equivalent release Cargo build passed. task-postcheck.sh requires a
task-specific baseline file and allowlist, which were not supplied, so it was
not invoked with invented scope arguments.
