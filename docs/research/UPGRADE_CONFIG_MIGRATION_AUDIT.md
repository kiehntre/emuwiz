# EmuWiz upgrade and configuration migration audit

> **Recovered historical research — status against current main (`b66422c2`).**
> Source: branch `research/upgrade-config-migration` at `c31c2f75`. Recovered unchanged below this block except where marked `[refreshed]`.
> - **Research only; no migration code was changed.** Audited against main `41c9a645`, which was at **schema 20**.
> - **[refreshed] Current main is at schema 24** (migrations `0021_mame_member_evidence`, `0022_scan_source_coverage`, `0023_catalogue_safety_bindings`, `0024_source_enablement` were added after this audit). The current user-facing statement is in [`UPGRADING.md`](../UPGRADING.md). Every "schema 20" below means "at inspection time".
> - **Risk model that remains the valuable part:** split or stale state, absolute paths, TOML/JSON side state, old journals, and ArchiveFS-versus-EmuWiz directory coexistence (the EmuWiz-first, ArchiveFS-fallback resolver reuses the old directory in place and can make a half-migrated install look empty).


Status: research-only audit against `main` at `41c9a645464a3fec9fa68cad7de78cfadd19cf12`.

This document does not implement migration behavior. It records what the current
build actually does, what it preserves, and what a release must prove before
claiming safe upgrades.

## Executive conclusion

Recent ArchiveFS users are supported by an EmuWiz-first, ArchiveFS-fallback
directory resolver. The resolver reuses a legacy directory in place; it does
not copy or rename it. If both names exist, EmuWiz wins as a complete directory.
That rule is deterministic, but it can make a partially migrated installation
appear empty if the user has created a new EmuWiz directory while the real data
remains in ArchiveFS.

The SQLite catalogue has a real forward migration path through schema 20 [refreshed: schema 24 on current main]. It
backs up an existing database before an explicit upgrade, applies each migration
transactionally, refuses newer schemas, and has no downgrade support. The
backup is a recovery mechanism, not proof that every application entry point
will automatically upgrade. In particular, read-only/no-scan loading refuses
an outdated database and does not silently mutate it.

The main unresolved release risk is not SQLite corruption; it is split or stale
state: absolute paths, separately persisted TOML/JSON files, old journals, and
external emulator configuration are not globally rewritten. A release should
therefore require a user-visible preflight/backup procedure and an explicit
policy for both-directory installations. ArchiveFS-era compatibility is
credible for the standard roots, but old nonstandard locations, desktop files,
environment overrides, and external emulator configuration are not migrated.

No equivalent upgrade/configuration migration audit was found in reachable
documentation or history. Existing storage, database, and recovery documents
are component-specific rather than an end-to-end upgrade contract.

## Baseline and source anchors

- Authoritative starting SHA: `41c9a645464a3fec9fa68cad7de78cfadd19cf12`.
- Main status at audit start: `main...origin/main [ahead 293]`; unrelated
  untracked research documents were present and were not touched.
- This research worktree is clean on branch
  `research/upgrade-config-migration`.
- Current directory behavior is implemented in
  `crates/archivefs-core/src/app_dirs.rs` and consumed by
  `database.rs`, configuration, DAT, identity, view, and transaction modules.
- Database schema and upgrade behavior are in
  `crates/archivefs-core/src/database.rs` and
  `crates/archivefs-core/src/migrations/`.
- The ArchiveFS compatibility change is commit `92d72143`,
  `core: EmuWiz-first app-directory resolution with legacy ArchiveFS reuse`.
- Representative historical tags inspected: `v0.5.0-alpha`, `v0.7.0`,
  `v0.8.0-alpha`, `v0.8.1-alpha`, `v0.8.2`, `v0.8.3`, and `v0.9.0`.

## Current locations and directory selection

On Unix with the usual environment, current defaults are:

| Kind | EmuWiz path | Legacy fallback |
|---|---|---|
| Configuration | `~/.config/emuwiz` | `~/.config/archivefs` |
| Data/catalogue | `~/.local/share/emuwiz` | `~/.local/share/archivefs` |
| Main config | effective config dir/`config.toml` | same relative name |
| SQLite catalogue | effective data dir/`library.sqlite3` | same relative name |
| Index | effective data dir/`index.json` | same relative name |

`XDG_CONFIG_HOME` and `XDG_DATA_HOME` are honored when absolute. Explicit
`EMUWIZ_CONFIG_HOME` and `EMUWIZ_DATA_HOME` take precedence. Relative override
values are rejected. The same resolver is used for dependent defaults, so a
legacy-only installation also finds its DAT registry, emulator profile memory,
rename journals, identity roots, and other data below the selected legacy
directory.

Selection is at directory level:

1. use the EmuWiz directory if it exists;
2. otherwise use the ArchiveFS directory if it exists;
3. otherwise use the EmuWiz directory for a fresh install.

An existing path includes a broken symlink or other present filesystem object.
There is no automatic copy, merge, rename, conflict resolver, or per-file
fallback. A user with both roots must be told which root is active before any
scan or write.

## Historical format eras

| Era | Observed persistent baseline | Meaning for upgrade |
|---|---|---|
| ArchiveFS/early alpha | `archivefs` config/data roots; SQLite migrations had reached roughly schema 3–6 in the inspected tags | Standard roots remain discoverable through the legacy fallback; old binaries and custom paths are not renamed. |
| 0.8.x | Config gained `master_rom_root`; database versions progressed through 10–12; DAT and provider projections expanded | Current parser defaults the new optional field, while database upgrade preserves rows through forward migrations. |
| 0.9.0 | Database schema 16; source roles and newer evidence were still later additions | Current upgrade applies 17–20, but old clients cannot understand the newer state. |
| Current main | Database schema 20 at inspection [refreshed: 24 on current main]; structured source config; provider/cache-specific version markers; managed install manifests | This is the release target. No complete application-wide config schema version exists. |

The historical tags demonstrate incremental SQLite migrations, not a universal
configuration migration framework. The app-directory compatibility change is
newer than the older tags and is the principal ArchiveFS-to-EmuWiz bridge.

## Persistent data inventory

“Preserve” means copy/backup before an upgrade even if the application can
rebuild a projection. “Rebuildable” does not mean harmless to delete while a
transaction is active.

| Persistent item | Old location/format | Current location/format | Versioned? | Migration? | Preserve? | Rebuildable? | Upgrade risk | Recommended action |
|---|---|---|---|---|---|---|---|---|
| Main config and source definitions | `config.toml` under ArchiveFS; legacy `source_folders` list | effective config dir/`config.toml`; structured `[[source]]` plus compatibility aliases | No global config version | Parser compatibility only; no rewrite until a save | Yes | No | Both roots or a save can select a different source set | Back up and expose active root; do not merge automatically. |
| SQLite catalogue | `library.sqlite3` in old data root | effective data dir/`library.sqlite3` | Yes, `PRAGMA user_version` and `schema_migrations` | Yes, forward 1→20 at inspection [refreshed: 1→24] | Yes | No | Path split, interrupted later migration, manual copying while open | Close app, make the application upgrade backup, retain the generated backup until verified. |
| JSON index | `index.json` | effective data dir/`index.json` | No application-wide marker | No | Usually | Usually | Stale or absent index can disagree with catalogue | Treat SQLite as authoritative; rebuild only after confirming callers do not need the index. |
| DAT source registry | `dat_sources.toml` in config root | same name in effective config root | TOML fields, no global version | No central migration | Yes if selections matter | No | Old root selection can hide sources; unknown fields depend on TOML structs | Back up; validate paths and source IDs before scan. |
| Managed DAT sources | historical source settings | `managed-dat-sources.toml` (effective config root) plus `managed-dats/` data root | Managed state and source snapshots carry typed/schema fields | No cross-file migration | Yes | Objects may be re-fetched, but selections/state are not disposable | Missing DAT root or stale absolute source path | Preserve config, state, manifests, and selected packs; rebuild only objects after verification. |
| DAT objects/snapshots | local DAT folders and older imported snapshots | effective data root/`managed-dats` and provider-specific roots | Managed DAT state is typed; imported formats have parser/schema metadata | No general relocation | Yes for provenance and selected versions | Raw downloads may be reacquired | Duplicate imports or changed path can create a second source | Keep source identity and snapshot metadata; do not deduplicate by filename alone. |
| Emulator profiles/selections | early GUI settings or external emulator config | effective config root/`emulator_profiles.toml` plus explicit setup/profile files | No global marker; profile file is hand-parsed | No migration framework | Yes | No | absolute executable/profile roots become stale | Back up and revalidate each binding; never silently substitute an emulator. |
| Emulator executable paths | user-selected external paths | profile/setup state and external emulator config | Usually no EmuWiz schema | No | Yes | No | moved mount or installation appears missing | Report unavailable path and request re-selection. |
| Managed emulator installs | install-specific directories | install root/<`manifest.json`>, manifest schema 1, side-by-side install tree | Yes, manifest schema and executable SHA-256 | Manifest validation, not relocation | Absolutely | A downloaded install can be recreated, but ownership/provenance must survive | moved install root becomes stale; older binary cannot understand newer manifest fields | Preserve manifests and install roots; mark stale rather than overwrite. |
| BIOS/readiness evidence | emulator-local firmware and DAT/provider evidence | mostly derived from local files, emulator adapters, catalogue/provider records | Evidence has component-specific formats; no universal readiness DB migration | No blanket migration | User selections and authoritative evidence yes | Derived probes can rerun | stale firmware paths or reclassification after adapter changes | Preserve explicit bindings/provenance; re-probe and distinguish unknown from missing. |
| Playing Library views/manifests | older view/history folders under selected data root | config `library_views.json`; data `library_views/<id>.manifest.json` and history | JSON models, no single app version | No global migration | Yes | No, history is user-visible state | stale destination/source paths can change planning | Back up manifests/history; require explicit revalidation before execution. |
| Transaction/history records | ArchiveFS-era transaction directory if under standard root | data `rename-transactions/`; durable journals plus `recovery-history-state` | Journal model includes state/envelopes; sidecar has serde defaults | No directory migration beyond root fallback | Absolutely while actionable | No | ignored journals if both roots or custom directory; unsafe resume after path move | Preserve, inventory, and show recovery attention; never delete as cache cleanup. |
| Duplicate quarantine journals | transaction/recovery machinery and quarantine paths | under data-root transaction/recovery areas and recorded absolute paths | Journal/state fields; no app-wide version | No | Yes | No | duplicate actions can be repeated or hidden | Back up with rename journals; do not rescan/reorganize automatically. |
| Mod/cheat transaction journals | provider-specific early files | data-root shared cheat history/backups and install/rollback run records | Individual JSON results have schema versions | Component readers reject incompatible documents or rebuild caches | Yes for rollback/history; caches conditional | Provider catalogue caches yes; applied transaction evidence no | old journals ignored or unreadable; rollback paths stale | Preserve all active/incomplete runs and backups; retain refusal files. |
| Provider configuration | scattered old provider settings | effective config/identity provider JSON and provider-specific TOML/JSON | Provider/cache-specific versions; no global config version | Usually parser compatibility only | Yes, including disabled state and mappings | No for user choices | corrupt config is an error, not a reset; legacy root may be inactive | Back up config and tokens separately; do not reset on parse error. |
| Provider provenance | old cache metadata/receipts | SQLite evidence plus identity/provider cache metadata | Many records include schema/source/retrieved fields | No universal migration | Yes | No when needed to explain evidence | losing provenance weakens trust and causes duplicate imports | Preserve even if raw cache is rebuilt. |
| Identity/provider cache | provider-specific cache files | data root/`identity/<provider>/identity-cache.json`, format 1, bounded records | Yes, cache format, provider/server/source fingerprint | Refuse incompatible cache and re-import; no destructive conversion | Prefer yes | Yes | cache refusal can look like empty provider if not explained | Keep old file, surface stale/refused, re-import explicitly. |
| Artwork metadata/cache | provider cache and artwork roots | data root/`identity/artwork/thumbnails`, `index.json`, lock; platform art roots | Artwork index/cache-specific | No global migration | Custom artwork yes; downloaded thumbnails usually no | Yes, if no active transaction depends on it | large cache hides path/provider failures; wrong root looks blank | Back up custom art/index if user-created; thumbnails can be rebuilt. |
| Thumbnail cache | provider-specific old cache | identity artwork thumbnail directory | Index/format local | No | Usually no | Yes | blank artwork only | Delete/rebuild only after confirming provider settings remain. |
| ScreenScraper settings | external/session configuration in older workflows | credentials are session-only in current path; accepted descriptive receipts are database schema 19 | Receipt is SQLite-versioned | Database migration only | User intent/settings yes; passwords/tokens separately | Media URLs/remote metadata can be reacquired | reset session or missing credentials can look like lost provider state | Do not claim settings survived if current build intentionally does not persist them. |
| RomM state | external server settings and token files | provider config/cache under identity; token remains user-supplied/external | Identity cache format 1; server/source fingerprint | Refuse mismatched cache, re-import | Yes | Cache yes, mappings and token no | server mismatch or moved token produces empty/refused view | Back up config and token location; verify server identity before re-import. |
| ES-DE integration | external `settings/es_settings.xml`, generated lists | external destination plus EmuWiz recovery/sidecar state where generated | External format, not owned by EmuWiz | No | Yes for user edits and recovery | Generated lists can be regenerated only with review | changed external path can duplicate or overwrite lists | Back up ES-DE settings and generated-list recovery state; no blind regeneration. |
| LaunchBox integration | external LaunchBox database/media paths | inspected as external integration; EmuWiz stores references/projections, not a universal owned DB | External schema | No | Yes externally | EmuWiz projection may rebuild | path move or duplicate import | Treat LaunchBox as external authority; revalidate paths and avoid re-import by default. |
| Activity/history | event/projection records in database and view/provider histories | database plus component history directories | SQLite or component schema | SQLite forward migration; component-specific readers | Yes if user-facing or audit evidence | Some derived activity can rebuild, not transaction history | blank/duplicated activity after wrong root | Preserve database and history dirs together. |
| Recovery markers | sidecars beside journal/view/provider state | `recovery-history-state`, locks, staging/temp files | Sidecar serde fields; temp names not durable formats | No | Active marker yes; stale locks only after inspection | Temporary files generally | deleting a marker can hide recoverable work | Inspect before cleanup; never treat all `.tmp` as disposable. |

## Configuration format evolution

The main config is not serde-deserialized as one versioned document. The
hand-written parser accepts:

- legacy `source_folders = [...]`;
- alias `sources = [...]`;
- current `[[source]]` blocks with `path`, `enabled`, and optional
  `created_at`;
- `ratarmount_bin` and legacy alias `ratarmount`;
- optional `master_rom_root`.

Unknown top-level keys and unknown keys in a source block are ignored. Missing
`ratarmount_bin` defaults to `ratarmount`; missing `master_rom_root` defaults
to no override. If structured sources are present they take precedence over
the legacy list. This is backward-compatible for old single-list files, but a
file containing both representations is not merged. That case is a
`NeedsMigration`/operator-review condition even though parsing succeeds.

Classification:

| Change | Classification | Reason |
|---|---|---|
| `archivefs` directory to legacy fallback | BackwardCompatible | Current resolver deliberately reuses it. |
| `source_folders` to `[[source]]` | BackwardCompatible for read; NeedsMigration for a clean canonical write | Structured sources supersede the list and disabled/created metadata cannot be reconstructed from the list. |
| `sources` alias | BackwardCompatible | Explicit parser alias. |
| `ratarmount` alias | BackwardCompatible | Explicit parser alias. |
| `master_rom_root` introduction | Defaulted | Optional and absent means no override. |
| unknown fields | IgnoredLegacy | Preserved only where the owning writer promises preservation; not a general round-trip guarantee. |
| invalid TOML/required `mount_root` missing | Unsafe/Incompatible | Current build reports configuration error; it does not guess. |
| provider/cache JSON version mismatch | Unsafe/Incompatible for that cache, not the library | Component refuses/rebuilds the cache rather than misreading it. |

There is no `config_version` marker that lets the application distinguish an
old valid config from a new valid config with changed semantics. Release notes
and preflight diagnostics are therefore required until a future migration
contract is added.

## Database migration audit

The current registered sequence is 1 through 20:

1. initial catalogue and scan tables;
2. platform aliases;
3–5. source scan status and source platform assignment;
6. bounded direct-image identity reports;
7. ingestion discovery details;
8–12. DAT identity, set verdicts, verified facts, expected inventory, and metadata;
13–16. scan fingerprints, archive listings, reused discovery evidence, and
   deterministic non-archive outcomes;
17. source roles;
18. provider-neutral mod catalogue records;
19. accepted ScreenScraper descriptive metadata and receipts;
20. trusted catalogue-wide media-topology evidence with producer/source
   provenance.

The database uses `PRAGMA user_version` plus the `schema_migrations` table.
Each migration runs in its own SQLite transaction, including its migration-row
and user-version update. A failure rolls back that migration. Earlier
migrations in the same upgrade may already be committed, so the upgrade path
creates a consistent pre-upgrade SQLite backup beside the live database before
applying the chain. The backup is mode 0600, verified, and SHA-256 recorded in
the upgrade report; on failure it is retained.

`upgrade_library_database` rejects a future schema before mutation. There are
no down-migrations. Read-only database opening requires exactly the current
schema; GUI no-scan loading reports an outdated database rather than silently
upgrading it. Scan-triggered upgrade uses the explicit upgrade path.

### Schema 19 versus 20

The recurring stale expectation is identifiable: older test assumptions use
19 as “latest” while this checkout registers
`0020_media_topology_evidence.sql`. The current source has an explicit test
that a schema-19 database has pending version 20, and the schema-16 upgrade
test expects `[17, 18, 19, 20]`. This is expectation drift in stale tests or
branches, not evidence that the runtime should stop at 19. Migration 20 is a
real additive table/provenance migration and must be included in release
fixtures and upgrade checks.

The practical concern is operational: a binary built from a schema-19 branch
will not understand a schema-20 database. Current behavior is safe refusal,
not downgrade or silent deletion. A release process must not run old tests or
old binaries against a database upgraded to 20 without expecting that refusal.

## ArchiveFS-to-EmuWiz compatibility

Supported by current code:

- standard `~/.config/archivefs` and `~/.local/share/archivefs` roots;
- old `config.toml`, `library.sqlite3`, `index.json`, DAT registries, view
  data, emulator profile memory, identity roots, and transaction roots below
  the selected legacy directory;
- partial legacy installations, provided the legacy directory is the selected
  effective root;
- EmuWiz wins deterministically when both top-level directories exist.

Not demonstrated as migrated:

- arbitrary old roots such as a user-created `~/.archivefs` tree;
- old executable names, desktop entries, shell scripts, or service files;
- stale `EMUWIZ_*_HOME`/XDG overrides pointing elsewhere;
- environment variables for emulator-specific firmware or ROM directories;
- emulator-owned configs outside EmuWiz’s roots;
- user-created copies of a database or transaction directory outside the
  standard resolver;
- a merge of files split between ArchiveFS and EmuWiz directories.

The application does not rewrite an ArchiveFS path to an EmuWiz path. This is
good for data safety but means “rename the application” is not equivalent to
“migrate all persistent state.” A release must print the selected roots and
warn when both exist.

## Absolute path and mount risks

Source folders, mount root, master ROM root, emulator executable/profile roots,
DAT files, BIOS directories, artwork roots, view destinations, managed install
roots, and transaction journal entries can contain absolute paths. The code
generally records and validates those paths; it does not prove that a moved
mount point is the same object and does not globally rewrite paths.

Consequences:

- a temporary unavailable USB/NVMe mount should appear unavailable, not empty;
- moving a mount can make a source look new and can cause an unsafe user-led
  re-registration if the user does not repair the path first;
- a moved transaction destination must remain under review because exact
  resume/rollback identity may no longer match;
- a moved managed install is a stale manifest condition, not permission to
  adopt or overwrite it;
- a worktree or application binary move is harmless only when data/config
  overrides remain stable; default home-based roots do not follow a checkout.

No automatic path rewriting is justified by the inspected code. A future
migration may offer an explicit path-repair tool based on user confirmation,
filesystem identity, and a backup, but release behavior should remain
fail-closed.

## Cache versus authority

Safely rebuildable when no transaction is active:

- thumbnails and downloaded artwork derivatives;
- identity/provider cache records when provenance/config are retained;
- temporary provider metadata and staging files after inspection;
- derived scan fingerprints and projections when the authoritative source and
  catalogue remain intact;
- downloaded DAT objects only when the selected source/version and provenance
  are preserved and reacquisition is intentional.

Must be preserved:

- `config.toml`, source enabled/disabled choices, and DAT/provider settings;
- `library.sqlite3` and its upgrade backup;
- transaction journals, rollback backups, cheat/mod run records, and recovery
  markers while actionable;
- library-view manifests/history and user-created artwork;
- emulator bindings and managed install manifests;
- provider provenance, accepted receipts, source fingerprints, and explicit
  user selections;
- external integration settings that EmuWiz does not own but needs to avoid
  duplicate imports.

Deleting a cache can explain a missing projection. Deleting authority,
provenance, or an active journal can destroy the ability to explain or undo an
operation. The release cleanup policy must use these categories rather than a
filename-only cache heuristic.

## Upgrade failure modes and recovery

| Failure | Severity | Current behavior | Recovery |
|---|---|---|---|
| Both EmuWiz and ArchiveFS roots exist | High | EmuWiz root wins wholesale | Show both paths; user chooses/copies after backup; never merge silently. |
| Legacy root is temporarily unavailable | High | Depending on resolver/environment, a new effective root can be created | Do not scan; verify mount and print active root before writes. |
| Old database is opened read-only | Medium | Refused as outdated | Use explicit upgrade path after backup. |
| Newer database opened by old binary | High | Newer schema is rejected | Restore/use matching newer binary; do not downgrade in place. |
| Migration fails | High | Current step rolls back; earlier steps may remain; verified backup retained | Restore backup or rerun after fixing the cause; never delete backup automatically. |
| Source path moved | High | Stored absolute path becomes unavailable/stale | Repair explicitly, then scan; do not register a second source automatically. |
| Provider cache incompatible | Medium | Component refuses it and leaves file in place | Re-import/rebuild cache while retaining config/provenance. |
| Old transaction journal outside selected root | Critical | It is not discovered | Locate it manually before any reorganizing operation; add release preflight detection. |
| Old emulator selection missing | High | Path validation fails or adapter re-detects | Re-select explicitly; do not silently choose another executable. |
| Cache removed | Low/Medium | Artwork/provider projection may be blank | Rebuild from retained settings and source metadata. |
| Rescan after path confusion | High | Can create duplicates/reclassification | Block scan until active root and source identity are confirmed. |

## Downgrade safety

SQLite downgrade is not supported. A newer binary writing schema 20 and an old
binary expecting 19 or less should fail closed because the current open and
health paths compare against the known schema. There is no evidence of a
down-migration that could preserve newer columns. Component JSON/TOML files
have mixed behavior: some readers reject incompatible versions, while the
main config ignores unknown fields. Therefore a downgrade can still lose
newer settings if an older writer rewrites a file it does not understand.

Recommended release policy: warn that downgrade is unsupported, require a
backup, and never advertise “rollback” as “run the old binary against the same
data.” A true rollback must restore the pre-upgrade database/config/journal
set, not merely replace the executable.

## Fresh install versus upgrade

| Scenario | Current behavior | Risk difference |
|---|---|---|
| Fresh install | Creates EmuWiz roots and a new schema-20 database when needed; defaults are applied | Lowest state risk, but no historical selections exist. |
| Recent EmuWiz upgrade | Reuses EmuWiz roots; explicit database upgrade preserves rows; component caches are read or rebuilt according to their own versions | Main risks are schema backup handling, absolute paths, and component writers. |
| Older ArchiveFS upgrade | Uses the legacy roots if no EmuWiz roots exist; database can migrate forward from shipped historical schemas | Main risks are both-root precedence, custom old locations, old journals, and old config semantics. |

The difference between “directory absent” and “directory exists but is empty”
is material. A fresh EmuWiz directory can mask a populated ArchiveFS root.
This is the most important unintentionally surprising fresh-install/upgrade
behavior to gate before release.

## Minimum pre-upgrade backup

The practical minimum, after closing EmuWiz, is:

1. the entire effective configuration directory, including `config.toml`, DAT
   registries, provider settings, emulator profiles, and selected-pack files;
2. `library.sqlite3` plus any application-generated schema backup;
3. the effective data-root transaction/recovery directories, shared cheat/mod
   history and rollback backups, and library-view history;
4. managed emulator install manifests and their install roots (or at least the
   manifests and a record of the install roots);
5. user-created artwork and integration recovery sidecars;
6. separately stored provider tokens/credentials, if the user wants to retain
   access (EmuWiz intentionally does not own all secrets).

Do not require copying thumbnails, temporary staging, or disposable provider
cache bodies for the minimum safety backup. Keep them until the first verified
launch only if rebuilding is expensive. Backups must preserve paths or include
an inventory; a random collection of files cannot restore directory-level
root selection safely.

## Release gate

Before release, the project should prove on disposable copies:

1. a recent EmuWiz config opens without changing source/emulator/provider
   selections;
2. an ArchiveFS-era standard-root installation either opens/migrates or gives
   a clear unsupported-location message;
3. a schema-3/6/12/16/19 database upgrades to 20 with rows and provenance
   intact;
4. an interrupted migration leaves a valid backup and a recoverable database;
5. a schema-20 database opened by an older binary fails safely and visibly;
6. transaction/history records remain readable and actionable after upgrade;
7. rebuildable caches can be removed without losing catalogue, settings,
   provenance, or rollback authority;
8. explicit emulator paths, source selections, DAT selections, and managed
   install manifests survive unchanged;
9. both-directory precedence is shown before any scan or write;
10. an unavailable mount does not create a blank replacement source or trigger
    a reorganization.

These are release checks, not claims that all ten are currently automated.

## Blockers and recommended follow-up

Release blockers for a strong “safe upgrade” claim:

- no end-to-end config version or migration manifest;
- no automatic merge or user-facing conflict flow when both EmuWiz and
  ArchiveFS roots exist;
- no universal inventory/preflight for old custom roots, desktop entries,
  environment overrides, and external emulator paths;
- component-specific cache/version behavior is not surfaced as one upgrade
  report;
- no supported database downgrade, and older writers may rewrite newer
  config fields they do not know;
- path repair remains manual and absolute-path-heavy;
- the schema-19/20 expectation drift must be kept out of release branches and
  fixtures must treat 20 as current.

Recommended next work is documentation/preflight first: print effective roots,
detect both-root and old-root states, inventory journals, show database schema,
and produce a backup manifest before offering upgrade or scan. A later
migration tool can address explicit, user-approved path repair. Neither should
silently rewrite or merge data.

## Final classification

### Must survive

Configuration and user selections; SQLite catalogue; DAT/provider provenance
and selections; emulator bindings; managed install manifests; active rename,
duplicate, mod, and cheat journals/backups; library-view history; accepted
receipts and recovery markers.

### Can be rebuilt with safeguards

Thumbnails, downloaded artwork derivatives, bounded provider caches, scan
fingerprints, temporary staging, and derived projections whose source data and
provenance remain intact.

### Currently unsupported or manual

Arbitrary pre-standard ArchiveFS directories, old desktop/service integration,
external emulator-owned settings, global absolute-path rewriting, database
downgrade, and automatic merging of split EmuWiz/ArchiveFS roots.
