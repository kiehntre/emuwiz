# DAT update and health coordinator recovery

Base: fresh `origin/main` `30802c9b95834385b99c0ce20dbeaf962b27a770`.
Historical source: `feature/dat-update-health-current-main`, `35cf7cca`.

Current main already had parser health, managed provider checks, local staged
No-Intro/TOSEC comparisons, provenance, immutable snapshots, rollback, and the
Blue DAT Sources journey. It lacked a shared installed overview, Check All,
expiring hash-bound check evidence, and an update-all plan. Recovery retains
those useful ideas from the historical coordinator without importing its second
version ordering or candidate parser/preview layer. Provider outcomes remain the
comparison authority, including opaque MAME Git revisions and Redump content
comparisons. Existing stage/preview/import workflows continue to own candidates.

Ownership was checked against all worktree status reports before editing.
An active change to `gui_v2/native_workflows.rs` was left untouched; the shared
DAT Sources renderer provides the Blue integration. Allowed areas are the core
`dat/update_health` module and registration, the DAT Sources page and its focused
`update_health` child, their tests, and this note. No GUI root, R2, journal,
database schema, or transaction code changes are needed.

## Blue journey

Open DATs & Verification / DAT Sources. The installed catalogue updates and
health card lists registered local/imported catalogues and typed installed
managed catalogues. Registry projections of the exact managed snapshot collapse
into that managed row. The card shows Current, Update available, Stale, Unknown,
or Check failed, installed and available versions when cached evidence knows
them, and the last session check timestamp. Validation remains a separate fact.

Click **Check All** to check eligible installed sources. The same coordinator
also handles individual installed-source Check actions. Ordinary browsing does
not start checks, hash games, or start an audit. The worker verifies installed
DAT SHA-256 before and after provider comparison; it never hashes the library.
Session evidence is bound to the exact installed SHA-256, expires after seven
days, and is not reconstructed as Current from persisted check timestamps.
Failures retain previous successful evidence but display Check failed. Restart
conservatively returns to Unknown until another explicit provider comparison.

The read-only update-all plan identifies eligible sources, current sources,
sources needing a fresh check, unsupported automatic providers, and conflicts.
Conflicts include unresolved managed metadata/objects, invalid local validation,
missing paths, and cached validation claims of the same catalogue identity.
Unknown identities remain unknown; no identity is inferred from a display name.
The plan has no Apply action and grants no replacement authority.

## Provider support

* MAME software lists: existing fixed GitHub revision check, for installed,
  configured sources with manual checks enabled.
* Redump BIOS and game/disc DATs: existing checks only for fixed descriptors
  whose acquisition mode is RemoteManaged.
* FBNeo, No-Intro, TOSEC, and user-local/custom DATs: visible, with unsupported
  automatic checking and manual/local/browser action required. Their existing
  local comparison and staged activation workflows remain the source of truth.

Concurrency is one, with a one-second gap between checks of datasets sharing a
service. A rate-limit response pauses that service across remaining datasets and
later explicit runs for Retry-After (60 seconds if absent). Other services keep
running. Provider failures are isolated. The existing HTTPS transport, request
bounds, cache metadata, and explicit proxy opt-out policy are reused. Redump's
existing explicit check may retrieve a temporary comparison payload; it never
publishes it into the snapshot store or replaces installed bytes.

No catalogue database handle reaches the coordinator or worker. Existing
provider checks may update their check/cache metadata. Installed catalogue
identity, snapshot hashes, validation evidence, and library verification results
are preserved. There are no automatic downloads, replacements, library hashes,
re-audits, or new network framework.

## Validation

The core tests cover current/outdated/unknown evidence, malformed and replaced
hashes, expiration and future timestamps, unavailable providers, retained
success evidence, known available revision, and planning dispositions.

The GUI coordinator tests use mocked providers/transports and disposable
catalogue databases. They exercise unchanged database bytes and installed
objects/provenance across real provider checks, missing/corrupt state and
objects, malformed upstream responses, rate-limit sharing and later retries,
offline/error isolation, serial pacing, ordinary page rendering without
workers, conflicting identities, and recovery after a provider failure.

The historical branch is retained as a reference; it is not cherry-picked or
deleted. Its missing coordinator behavior is superseded by this focused
integration, while historical preview/apply scaffolding is intentionally not
promoted.
