# Cheat record provenance and source evidence

This candidate starts at `4c980d184584dd5f1a22b5fbbf67e6c58ff204c1`.

## Audit

- The catalogue already retained `EncodedPath`, exact source-file SHA-256,
  source region/revision/serial/content hash, and declared `.cht` indexes.
  Its per-cheat definitions dropped code bodies. Each source game record stayed
  separate. Catalogue JSON is an availability manifest, not a cheat store.
- The strict `.cht` parser retained decoded descriptions/codes and warnings,
  including raw malformed lines. Decoding escaped text could lose the original
  spelling. Rendered output renumbers indexes, while source indexes remain.
- User imports already had a typed user-supplied origin, original path/filename,
  SHA-256, game match evidence, scan timestamp and duplicate-file relationships.
  User-supplied files do not establish user authorship.
- Neutral documents, conversion previews and reconciliation entries used string
  provenance. Reconciliation retained source entries and raw codes and grouped
  semantic/raw duplicates and same-title conflicts. Semantic comparison ignores
  source metadata. Resolved duplicate plans aggregated strings but cloned only
  the canonical document. GUI reports deserialize the core model directly.
- Existing provider abstractions retain provider identity, provenance/licensing,
  immutable fingerprints and validation. BSFree/CheatBase rows retain upstream
  row IDs, device, credits, original text and truncation diagnostics. These
  databases remain immutable; this change does not alter their SQLite schemas.
- Bundled patch manifests use their own `SourceSnapshot` with metadata hashes and
  pinned revisions; they are not converted into cheat records here. This task
  adds explicit bundled classification for cheat catalogue adapters rather than
  guessing it from a path or source display name.
- DAT/MAME identity, archive discovery and artwork aggregation distinguish
  observations from authority. The shared platform evidence lineage model has
  artifact identity, categorical claim strength and lineage relationships.
  Launch readiness/cheat applicability already distinguish exact hashes/serials
  from filenames and retain conflicts. No generic numeric trust score is reused.

## Record model and boundaries

`CheatRecordProvenance` extends the cheat-provider/domain boundary and reuses
`SourceArtifactIdentity`, `LineageRelation` and `ClaimStrength` from the shared
lineage module. It does not introduce another generic evidence engine.

Source kind is explicit: local file, RetroArch pack, bundled, manual user entry,
imported database, community database, emulator native, generated derivative or
unknown. Source quality describes history, never safe/unsafe behaviour. A known
local path is `LocalKnown`; an imported catalogue is `ImportedUnverified`.
Bundling and pack membership do not establish verification. Verified-source and
community-curated classifications require explicit evidence from an adapter.
The catalogue source constructors allow the caller to declare kind explicitly;
`.cht` format alone does not establish RetroArch pack membership.

Records retain internal paths, display filename, provider ID/name, record index
or key, source format, source artifact/hash/version, optional known upstream
record identity, original values, optional comparison values and normalization
status. New timestamps are not manufactured. Existing import timestamps remain
in their original reports. Bare original text has explicitly unknown origin.
The decoder's own implementation references never become source authority.

Catalogue definitions retain raw `.cht` description/code values, including
quotes, and JSON manifests can optionally supply an original code. Strict `.cht`
entries retain raw values before decoding; missing values remain missing.
Native neutral-document conversions retain available originals. Generated ZX
POK projections have derivative status and do not manufacture missing raw text.
User imports retain record evidence and their existing import provenance.
Candidate projections retain the typed evidence internally, so nested launch
journey candidates preserve it through discovery, selection and preview.
Ordinary GUI display continues to use existing relative paths; the new audit
paths are not rendered. Explicit JSON audit exports can contain internal paths.

## Applicability, corroboration and conflicts

Source region/revision/serial/hash declarations are weak evidence until matched
against a selected game. Filename association is weak; it never becomes a DAT
or verified hash claim. Existing candidate match evidence/classification remains
attached alongside source evidence. Reconciliation's already-verified game
association is projected as strong identity evidence without upgrading source
quality. Manual association is weak and remains user-authored after equivalence.

Groups expose every source observation in deterministic order. Known identical
artifact+record deliveries and known shared upstream record identities count
once for delivery-source counts. Observations are not deleted. This count is
not an independent-confirmation count: unknown lineage stays unknown.

There is no source-authority precedence. Existing reconciliation plans use the
first supplied entry for duplicate display; saved choices select among conflicts.
All duplicate evidence is retained in the resolved document. Conflict/skipped
and unsupported diagnostics retain typed evidence. Resolved plans also retain
the existing source report as an audit snapshot, including unselected conflicts
and original codes. A canonical label or review choice never erases that report.
Sorting audit observations does not select a winner or change semantic identity.

## Persistence and deferred work

Neutral documents, conversion previews, reconciliation JSON, user import JSON
and resolved plans serialize evidence. Additive fields use serde defaults for
legacy records/reports and partial provenance. Catalogue snapshots remain runtime
availability data with JSON projections; no new persistence backend or migration
is needed. Launch journey discovery, selection and preview remain runtime-only,
carrying evidence in the candidate; existing generic installer journal storage is
unchanged. Legacy provenance strings remain supported and are not interpreted
as claims of authority.

Provider retrieval, authentication, scraping, new provider adapters, source
badges/trust meters, and user-facing wording are deferred. There are no new
network operations. Full workspace/GUI/release validation and live GUI smoke are
reserved for batch integration.

## Changed files

- `crates/archivefs-core/src/open_retro_cheat_providers.rs`
- `crates/archivefs-core/src/patch_manager/cheat_candidates.rs`
- `crates/archivefs-core/src/patch_manager/cheat_catalogue.rs`
- `crates/archivefs-core/src/patch_manager/cheat_conversion.rs`
- `crates/archivefs-core/src/patch_manager/cheat_coverage.rs`
- `crates/archivefs-core/src/patch_manager/cheat_install_plan/tests.rs`
- `crates/archivefs-core/src/patch_manager/cheat_ir.rs`
- `crates/archivefs-core/src/patch_manager/cheat_provenance.rs`
- `crates/archivefs-core/src/patch_manager/cheat_provenance/tests.rs`
- `crates/archivefs-core/src/patch_manager/cheat_reconciliation_plan.rs`
- `crates/archivefs-core/src/patch_manager/cheat_reconciliation_plan/tests.rs`
- `crates/archivefs-core/src/patch_manager/cht_document.rs`
- `crates/archivefs-core/src/patch_manager/classic_game_genie.rs`
- `crates/archivefs-core/src/patch_manager/dolphin_onframe_install_plan.rs`
- `crates/archivefs-core/src/patch_manager/dolphin_onframe_source.rs`
- `crates/archivefs-core/src/patch_manager/mod.rs`
- `crates/archivefs-core/src/patch_manager/n64_gameshark.rs`
- `crates/archivefs-core/src/patch_manager/saturn_action_replay.rs`
- `crates/archivefs-core/src/patch_manager/three_ds_cheat.rs`
- `crates/archivefs-core/src/patch_manager/user_cheat_import.rs`
- `crates/archivefs-gui/src/cheat_reconciliation_review/persistence/tests.rs`
- `crates/archivefs-gui/src/onframe_install_session.rs`
- `crates/archivefs-gui/src/onframe_install_state.rs`
- `crates/archivefs-gui/src/tests/emulator_profiles_and_setup.rs`
- `crates/archivefs-gui/src/tests/mod.rs`
- `crates/archivefs-gui/src/user_cheat_import_page.rs`
- `docs/cheats/PROVENANCE_EVIDENCE.md`

## Focused validation

- 253 affected cheat/provenance unit tests passed, including 34 new provenance
  tests; 9,763 unrelated unit tests were filtered out.
- 49 strict `.cht` parser and cheat-coverage module tests passed.
- Targeted journey tests passed: orchestration 3, local installation 3,
  RetroArch cheat installation 12.
- `cargo check -p archivefs-core -p archivefs-gui --lib --offline` passed,
  with six existing GUI warnings.
- `cargo fmt --all -- --check`, `git diff --check`, the file-scope guard and
  the GUI root boundary guard passed.
- No full workspace or GUI suite, release workspace build, live GUI smoke,
  new network provider operation, push or main promotion was performed.
