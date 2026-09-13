# DAT authority and collection completeness

Implemented on the 0.9 development branch, starting at
`1a3750679f1eba8b4ea62ef75a4746256d215523`. This is a read-only projection,
not a new identity store or an automatic DAT-maintenance workflow.

## User flow

Open **Sources → DAT Sources**. The first section is **DAT authority &
collection completeness**:

- **By platform** shows each explicitly assigned source separately. Filter
  platforms and page through 50 rows at a time. Expand a row for provenance
  and explanations. Overlapping catalogues are never summed.
- **Authority preparation** lists missing assignments, unknown variants,
  stale generations, missing inventories, and imported inventories ready
  for audit. Existing source controls below remain the only configuration
  and validation workflow; nothing downloads automatically.
- **Refresh impact** compares two already retained source inventories.
  Revisions replaced under the same source ID are not historical snapshots.
  Keep old/new sources separately if comparison is needed. Shared entry IDs
  are used for rename comparison only after confirming both sources belong
  to the same catalogue and variant. This confirmation performs no writes.
- **Refresh dashboard** re-reads database/configuration evidence. Source
  actions and catalogue reloads invalidate the in-memory projection.

Needs Attention routes concrete DAT issues back to this section. A single
platform/source item carries linked missing, ambiguity, BIOS, and authority
details; rebuilding replaces these items rather than recording separate
manual resolution flags. Publisher-freshness warnings can remain after all
collection entries are matched: those are different facts.

## Inventory: what is actually available

| Evidence | Existing source of truth | Limit |
| --- | --- | --- |
| Registered sources, platform assignment, enabled flag, origin, registration time, last validation health | `dat_sources.toml` | Registration is not proof of validation or publisher currency; recorded invalid/unreadable validation blocks a denominator |
| Managed source hash, revision, retrieval/check times, publisher name/ecosystem | Configured managed DAT state files | A successful download/check date alone does not establish latest publisher authority; managed sources currently have no explicit inventory platform assignment |
| Expected names, display names, optional publisher entry ID, ROM count | `dat_expected_entries` | One current inventory per source, not a history of versions; no full member hashes or BIOS declarations |
| Captured revision, ecosystem, expected count, duplicate names skipped, validation time | `dat_expected_inventory_meta` | Duplicate names or inconsistent counts invalidate the denominator |
| Verified/no-match/ambiguous states, candidate names, catalogue variant/source provenance, audited hashes/revision | `library_dat_identities` | Only audited library objects; absence of a row is not a negative match |
| Catalogued platform, size, known-missing state | `platform_assignments`, `archives` | Recorded evidence, not a live on-disk check |
| Set completeness and BIOS/parent/clone dependency outcomes | `dat_set_audit_results`, `dat_set_audit_dependencies` | Audited sets only; missing BIOS requirement counts are not a complete platform firmware inventory |
| Full DAT hash for ordinary local sources | Not generally retained | Displayed as unavailable, never recomputed on page load |
| DAT header date/author and full clone/parent/BIOS/ROM structure | Parsed DAT model, some audit provenance | Not comprehensively persisted in expected inventory; dashboard does not reparse files |

## Meaning of counts and authority

`DatAuthorityStatus` combines configured source identity/provenance with
persisted inventory and audit metadata. Freshness is **Unknown** unless a
revision difference proves it **Stale**. Import time never produces a
**Current** publisher claim. The model reserves Current for independently
established evidence; this offline projection does not emit it.
Local inputs reuse the existing registry decoder and refuse duplicate or
invalid source IDs. A single retained catalogue-header revision is comparable
to inventory metadata; a managed upstream Git reference is provenance only,
not a DAT header version and never used to manufacture revision drift.

`CollectionCompleteness` uses Complete, Incomplete, PartialAuthority,
Unverified, Ambiguous, and NoAuthority. Complete means **against this
imported DAT**, not every released game, every region, emulator readiness,
or a freshly rehashed filesystem. No percentage is manufactured.

Matched counts distinct expected identities, not archive rows. Known absent
archives are excluded. Existing catalogue size/revision/stale evidence gates
recorded verification. A stale or unexamined local file cannot be silently
called extra. Extra means an exhaustive no-match or a verified local identity
outside the retained inventory. Ambiguous candidate entries and pending
verification are separate from missing; unknown/unexamined local content
withholds the missing count when it could represent those entries.

Arcade and known multi-member entries require existing complete set and
dependency verdicts. Unknown/zero ROM member shape also requires set proof,
not a one-file inference. BIOS gaps are reported only from current,
exhaustive, source-scoped set/dependency evidence. Unsupported/incomplete
evidence stays visible instead of being replaced by a completeness claim.
A retained set verdict must also agree with the current canonical identity;
an older set name cannot fill another expected entry. A negative set verdict
overrides flat membership even for a single-ROM entry, unless another current
copy has a complete set verdict. This is independent of result ordering.

## Read-only refresh impact and limitations

Added/removed names come from two indexed, retained inventories. A shared
unique publisher entry ID can identify a rename within the user-confirmed
catalogue namespace; IDs are not assumed globally unique. Hash changes and
BIOS-requirement changes are explicitly unavailable because the current
expected-inventory representation does not retain those fields. The typed
result leaves these counts optional for richer future source evidence.
No rename, reorganisation, new authority selection, or file-impact plan is
applied or inferred from this comparison.

## Performance and safety

Database reads run in a background worker and a consistent read transaction
on `Database::open_read_only`. The projection makes bounded bulk queries over
existing indexed/catalogued data, with no per-ROM query, DAT parse, hash,
filesystem traversal, migration, or network request. Configured state files
are read once per refresh. Rendering uses cached summaries. Generations
invalidate cached results after catalogue/source changes; refresh is explicit
for out-of-process changes. Errors stay visible; an empty result is not a
substitute for a failed load.

The existing database restore busy guard includes both dashboard and
comparison readers. Their connections close before completion is signalled.
All automated fixtures use temporary SQLite files, not the production DB.

The focused ignored performance fixture can be run explicitly:

```sh
cargo test -p archivefs-core --release --lib \
  database::authority::tests::performance_100k_catalogue_and_inventory \
  -- --ignored --nocapture --test-threads=1
```

### Measured load time

Disposable fixture: 100,000 catalogued archives, 100,000 persisted identity
rows and 100,000 expected entries; one explicitly assigned source/platform.
This measures the complete database projection, not application startup or
opening the existing source-manager controls. OS caches were not flushed.

Environment: AMD Ryzen 9 5950X virtualized host, 24 visible CPUs, 46 GiB RAM,
Linux 6.8.0-139-generic, ext4 on `/dev/mapper/ubuntu--vg-ubuntu--lv`.
Other workloads were active. Compilation was bounded to two jobs.
Compiler: rustc 1.97.1 (8bab26f4f, 2026-07-14).

| Profile | Five projection reads, milliseconds | Result |
| --- | --- | --- |
| Final release, optimized | 772.3, 720.6, 799.2, 638.0, 745.8 | Median 745.8 ms; maximum 799.2 ms, within the 1 s typical / 2 s tested-worst target |
| Initial unoptimized test build | 2305.9, 2328.2, 2627.7, 2116.1, 2258.0 | Above target; not hidden or substituted for release measurements |

These are fixture measurements, not a guarantee for arbitrary DAT sizes,
storage latency, corrupt data, or all hardware. Rendering is cached and
separately bounded to 50 platform/source rows per page.

The final headless GUI test, in the unoptimized test build at 1366×768,
rendered a cached 120-platform summary (120,000 local items represented,
50 rows displayed) in 117.0 ms on the first platform-tab frame. Preparation
and comparison tabs took 32.9 ms and 5.3 ms. This checks actual egui layout
and read-only state preservation, not physical-display startup time.

## Remaining 0.9 comprehension work

Persisting richer DAT generation manifests (full member hashes, BIOS flags,
parent/clone structure, header dates and variant provenance) would enable a
more complete refresh-impact report. Explicit managed-inventory platform
assignment and independently evidenced publisher-freshness checks remain
separate work. First-run Understand → Fix → Organise → Play guidance and
cross-workflow refresh/reconcile explanations are not implemented here.

## Validation result

Final source validation passed:

- `RUST_TEST_THREADS=4 cargo test --workspace --no-fail-fast -j 2`:
  17,669 top-level test executions passed, zero failed, eight ignored.
  This includes executable aliases, not 17,669 unique test definitions:
  core has 8,962 passing unit tests, each CLI target 335, each of the three
  GUI targets 2,636, and the integration suites together 129.
- The ignored cases are the explicit 100k fixture and existing manual/live
  Downloads/Game Boy/PS2 tests (GUI cases repeat across aliases). No live
  collection was required for the new tests. The 100k fixture was explicitly
  run in release mode: all 16 focused database tests, including that fixture,
  passed. Four dashboard GUI tests and the Needs Attention projection test
  also passed.
- `cargo clippy --workspace --all-targets --all-features -j 2 -- -D warnings`:
  passed. Existing Cargo duplicate-executable-target notices are unchanged.
- `cargo fmt --check`, `scripts/security-scan.sh`, and `git diff --check`:
  passed. The security scan included all new tracked files.

The GUI adapter's optional-health handling and dependency-free registry test
were corrected after compiler feedback. Existing coverage-label assertions
were updated for the intentionally more precise wording. No unresolved test,
lint, baseline or environment failure remains in this final run. GUI
validation is headless egui rendering and regression testing, not a claim of
manual physical-display QA.

## Changed files

- `crates/archivefs-core/src/dat/authority.rs`: shared read-only models.
- `crates/archivefs-core/src/dat/mod.rs`: model export.
- `crates/archivefs-core/src/database.rs`: projection module registration.
- `crates/archivefs-core/src/database/authority.rs`: catalogue projection and retained-inventory comparison.
- `crates/archivefs-core/src/database/authority_tests.rs`: disposable correctness, non-mutation and 100k fixtures.
- `crates/archivefs-gui/src/dat_authority_dashboard.rs`: cached dashboard, preparation, comparison and GUI tests.
- `crates/archivefs-gui/src/dat_coverage_panel.rs`: clarify existing recorded-match labels.
- `crates/archivefs-gui/src/dat_coverage_panel/tests.rs`: assertions for the clarified labels.
- `crates/archivefs-gui/src/dat_sources_page/tests.rs`: updated source-page section label assertion.
- `crates/archivefs-gui/src/main.rs`: page, invalidation, attention and restore-reader coordination.
- `crates/archivefs-gui/src/needs_attention.rs`: source-derived DAT issues and routing/resolution test.
- `crates/archivefs-gui/src/tests/mod.rs`: dashboard state in the disposable app fixture.
- `docs/DAT_AUTHORITY_DASHBOARD.md`: evidence inventory, semantics, measured performance and limitations.

