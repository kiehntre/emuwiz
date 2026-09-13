# Unified Needs Attention

## Isolated integration

Authoritative main is deliberately untouched at `89da3a13e6c75054c11dad5c8c5c841359fc3a2f`
in `/home/davedap/emuwiz-main-release-fix`.
Worktree: `/home/davedap/emuwiz-needs-attention-integration`.
Branch: `feature/needs-attention-integration`.
No branch merge or push is part of this task.

Only these prerequisites were cherry-picked, in dependency order, followed by
the inspected draft. All applied without conflicts; no old branch history was merged.

| Original commit | Isolated commit | Subject |
| --- | --- | --- |
| `8f6527a6cd6f3b540c5035a6e702e5a6a9ef31d4` | `db0cd38cff6c360eb0eae32f52f73636bd98fb1f` | feat: add unified operation receipts foundation |
| `91abfb09b2f65d3670bb914b79208d7294d85069` | `ff1e471c794b6160db34bde2fecc405634203e9d` | feat: expand unified recovery coverage |
| `e4e73cd5fd9ce8ab3cc9fd7e79a7b7fa144b2ab0` | `443ecb53ef403388b1aa43767c13d7f72ae66cf2` | feat(recovery): project library publication operations |
| `b3ab08c5b371a78d45eea484a55439b89cd84a35` | `e46adc335f21366178eed1e965a1d2a3be772217` | feat(recovery): project database recovery operations |
| `889916a10abc3b32537527d2411057e0841727f6` | `1764bcd02b43a2e6dfaebd7a1f015fce21357568` | feat(recovery): project disc conversion operations |
| `e31137079907fcc62e37d906a411813f50799db5` | `85f64f70d2fb48dbccb47230f38a70e1eaa0973c` | feat(recovery): add verified database restore |
| `1a3750679f1eba8b4ea62ef75a4746256d215523` | `0dbdd28cbddb0680216e9f4f8ea81910bb333891` | feat(gui): add unified Needs Attention workspace (draft) |

## Source inventory and coverage

The core model is `attention::AttentionItem`. It is a projection, not a table:
stable source-owned ID, category, severity, title/summary, affected resource and
platform, workflow, source references, optional first/last observation time,
derived unresolved/resolved state, recommended action, typed destination,
recoverability, provenance, and represented evidence count. Unknown timestamps
remain unknown. No manual resolved flag is stored.

| Attention source | Authoritative evidence / stable identifier | When actionable / route |
| --- | --- | --- |
| Failed, partial, interrupted, stale, rollback-blocked operations | OperationRegistry projections; transaction/operation ID | Saved failure/recovery state; originating workflow and existing recovery |
| Recovery review | Registry recovery classification and review capability | Review-required active operation; completed rollback availability alone is not an issue |
| Duplicate quarantine / repair apply | Existing rename transaction; operation ID | Failed/partial/stale; Problems & Repair |
| Duplicate review candidates | Current catalogue normalized name + platform | One summary per platform; explicitly **possible** duplicates, never asserted equal content; Library duplicate review |
| Verified exact duplicates | Current ExactDuplicateScanReport and its remaining review groups, scan root | Duplicate Finder; quarantined groups are removed by that workflow, not an attention flag |
| Saved repair review | Current non-stale LibraryRepairPlan, source/generation | Proposed executable repairs; Problems & Repair; stale post-apply plans are not offered again |
| DAT mismatches / authority | `library_dat_identities`, archive + DAT source key, stored verification state and revision staleness | No match, conflicting, ambiguous, insufficient/filename-only evidence, stale revision; Identify & Rename / DAT review |
| Known set completeness / dependencies | `dat_set_audit_results`, archive/source/game key, saved set and dependency verdicts | Incomplete, bad metadata, needs review, unsatisfied dependency, stale authority; DAT review |
| Collection completeness | Existing scoped expected-inventory/coverage workflow | Not generalized into a global percentage; no unaudited game is invented as missing |
| Missing BIOS / emulator / broken profile | Completed Doctor findings (stable finding code + affected profile); selected-game LaunchPlan blockers/firmware | Only known required failures block; Emulator Setup |
| Launch refusal / missing identity | Existing selected-game planner and completed discovery lanes | No ready known launch option / unresolved identity; selected-game launch readiness |
| Missing tools / optional config | Typed Doctor finding severity and category | Diagnostics or Emulator Setup; optional warnings remain warnings |
| Library View / RomM / ES-DE outcomes | Saved LibraryViewHistoryRecord, typed profile kind, view ID + destination; OperationRegistry projections | Latest outcome for that view/destination replaces earlier failure; publication workflow |
| Current RomM / ES-DE refusal and recovery | Current PlayingLibraryPageState's core-derived publication errors, applied transaction, unresolved recovery path | One primary item per current publication action; opens the corresponding publication mode without discarding its error/preview |
| Cheat / patch / mod failures | SharedApplyJournal; operation ID and adapter kind | Cheats & Mods; same existing recovery workflow |
| Successful shared rollback | Durable SharedRollbackPreview marker, original operation ID and destination, successful entry outcomes | Resolves original apply outcome; mismatched/failed/unsupported marker cannot resolve it |
| Managed cheat/patch problems | Already-completed Doctor managed-entry findings | Cheats & Mods; no new file scanning |
| Disc conversion failure | Existing rename/conversion transaction and registry state | Disc Conversion / existing review; no output re-hash on the page |
| Database recovery / verified restore failure | DatabaseRestoreReceipt; operation ID and saved backup/restore state; Doctor database findings | Blocking while failed; History & Logs / recovery; no claim a backup is currently intact without verification |
| Broken source path / failed scan | `source_folders.id`, last_scan_status/error, configured membership | Sources; error text is displayed, never reparsed to invent a failure type |
| Missing catalogue files | `archives` presence evidence + source ID | Sources; missing detection remains owned by the existing scanner; one failed source suppresses its redundant missing-file cards |
| Unknown platform | Current platform assignment for present configured-source archives | Warning summary; discovery review |
| Ambiguous / unsupported discovery | Latest completed scan's typed skip counters, scan ID (including existing reused-detail references) | Warning summaries; discovery review. Scope explicitly remains that scan, not every historical source |
| Other library / configuration repair findings | Completed Doctor findings not superseded by indexed source/operation evidence | Problems & Repair |
| Unreadable / future / oversized receipts | Bounded registry reader coverage report | Visible partial coverage and full-history route; never reported as a healthy empty source |

## Severity, deduplication, refresh, and actions

- **Blocking:** failed database recovery, rollback-blocked operations, required
  BIOS/launch refusal, and Doctor's existing error/critical findings.
- **Action needed:** failed/partial/stale workflow review, DAT authority/set
  review, duplicate candidates, publication/recovery review, source failures.
- **Warning:** unsupported/ambiguous classification and optional Doctor issues.
- **Info:** completed/rolled-back history and genuinely informational diagnostics.

Counts at the top include unresolved summary cards only, not all historical
operations and not a fabricated distinct-file total. Details show represented
record counts. DAT identity and set verdicts for one platform share one review
card; several evidence records may concern the same file. Same operation IDs
deduplicate and retain up to eight technical references. The newest saved
Library View outcome for a view/destination wins. Different destinations can
require different cleanup actions and are not silently collapsed.

Filters: severity, category, platform, workflow, unresolved/resolved/all, search,
and newest first. Default: unresolved only, severity then newest. Actions only
navigate; no repair/apply logic exists in this workspace.

Saved catalogue/receipt state refreshes in a background worker on startup,
catalogue generation changes, and at most every 30 seconds independently of
the selected page. Completed in-memory diagnostics, launch plans, and current
publication outcomes replace their projection every second. This is ordinary
ephemeral UI state, not a new durable truth store. A BIOS installation becomes
known after the existing BIOS/diagnostic check updates; the page does not probe
the filesystem to discover it. Likewise, source repairs become known through
existing source checks/scans, and publication fixes through their workflow.

## Bounded loading and safety

Eight fixed catalogue queries run in one consistent read-only transaction.
The missing-file summary explicitly uses the existing `archives_missing`
partial index: SQLite otherwise chose the full source/path identity index to
serve the grouping order. The indexed and original queries returned identical
32-row results on the real catalogue; warm isolated query time was 0.05 s versus
0.20 s. No index, schema, or persistent cache was added.
SQL aggregates before returning rows, with a maximum of 1,025 groups per query;
the in-memory snapshot holds at most 1,024 summary groups and each rendered page
at most 50. A limit is shown as partial coverage and lower-bound counts, with
the source workflow retaining full detail. Informational history cannot crowd
out a newly observed blocker. Filtering/sorting is over these bounded summaries,
never a vector of all problematic catalogue files.

The background receipt reader enumerates only four known flat application
history directories: at most 4,096 entries each, 4 MiB per receipt and 32 MiB
total read payload. Oversized/unknown/malformed records are reported as coverage
gaps. It never enumerates library roots, probes publication destinations,
previews rollback, hashes database backups, or calls a provider. Page rendering
and filtering perform no filesystem or network calls and no mutations. Receipt
I/O is not hidden as page-load work: it is separately scheduled background input.
Technical details expand only for the displayed cards.

No schema migration, attention table, cache payload, source observation row,
or database growth is introduced.

## Known product-comprehension limits

- A stored success cannot prove that a destination has not subsequently drifted.
  Live drift/rollback safety is still revalidated by the existing workflow;
  an unperformed check is not fabricated as either failure or success.
- Legacy generic rename receipts cannot retrospectively identify a RomM/ES-DE
  target. Typed Library View receipts and current publication context can.
- ES-DE sidecars live next to selected gamelists, not in a global index. They
  enter this workspace when the existing ES-DE preview/recovery discovers them;
  the workspace does not walk destinations to find undiscovered sidecars.
- Readiness is limited to completed diagnostic/discovery inputs and the selected
  game's existing planner, not an invented library-wide launch audit.
- Resolved history is available where the source retains completion/rollback
  receipts. Vanished diagnostic problems disappear rather than becoming a new
  indefinitely growing historical record here.
- Very large receipt histories explicitly show partial coverage; the original
  full-history workflows remain the place to inspect omitted records.

## Validation and measurement

The reproducible read-only benchmark is:

```sh
cargo run -p archivefs-core --example attention_benchmark -- /path/to/current-schema/library.sqlite3
```

It reports distinct input-table cardinality, fixed query count, returned summary
count, filtered page size and query/page time for three runs. Input cardinality
is not claimed to be SQLite's physical row-visit count across repeated joins.
The deterministic 100,000-archive test also compares database bytes and verifies
zero connection changes while referencing nonexistent source paths.

Validation commands and focused results:

| Check | Result |
| --- | --- |
| `cargo check --workspace` | Pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass on final source, including the invalid-timestamp safeguard |
| `cargo fmt --check` | Pass |
| `scripts/security-scan.sh` | Pass; 1,248 tracked files examined |
| `git diff --check` and `git diff --cached --check` | Pass |
| Core `attention` test filter | 18 passed, including saved DAT identity/set refresh, invalid publication timestamps, matching complete rollback evidence, bounded 100k queries and no writes |
| GUI `attention` test filter (`--bin emuwiz`) | 22 passed; typed routing, missing BIOS, refresh, empty state, publication resolution, paged mutation-free rendering |
| Core `operation::` test filter | 32 passed |
| Core `database::restore::` test filter | 5 passed |
| `cargo test --workspace` followed by `--no-fail-fast` | Nine proven baseline core failures; CLI/integration tests and all three GUI aliases pass (2,635 passed, two ignored per GUI alias) |
| Complete rebuilt core test binary, final source (`--test-threads=4`) | 8,955 passed, the same nine baseline failures, one ignored; 265.92 s |

The final focused core build explicitly compiled the integration source with
`--config 'profile.test.package.archivefs-core.debug=1'`; it ran 18 matching
tests out of 8,965. An intermediate shared-target invocation that incorrectly
reused the baseline's two matching tests was discarded, not counted as a pass.
The frozen GUI/source validation and fresh core validation are both retained;
no baseline source, formatting or security fixture was repaired here.
Disposable loopback-server fixtures require running the full suite outside the
network-restricted sandbox. The syscall trace likewise required ptrace access;
these environment restrictions were resolved for validation, not worked around
in application code.

The deterministic 100k fixture's final query took **292 ms** (100,001 input
records, eight queries, two summary cards). Three headless GUI frames over 120
summary cards took **148 ms** total, rendering at most 50 cards on each page.

Measured catalogue: `/tmp/emuwiz-second-gen/.local/share/archivefs/library.sqlite3`,
schema 16, 102,343 archive records and 22 source records. This existing realistic
catalogue has no saved DAT identity/set rows; deterministic fixtures separately
exercise those inputs. Eight queries produce 47 unresolved summary cards, all
on one 50-item page, with no coverage truncation.

After the full GUI run finished, the final three query/page measurements were
**593 / 625 / 622 ms**, with peak RSS **8.4 MiB** and 1.86 s wall time for all
three queries together. The indexed query's earlier repeat reads were
**539 / 514 ms**. Another run
during concurrent compilation measured **1,270 / 1,248 / 742 ms**, with peak
RSS **8.5 MiB**. An earlier indexed first read under contention took **2,540 ms**;
before the index-selection correction a stressed first read took **3,157 ms**.
These outliers are retained: typical sub-second loading is demonstrated, but
a universal two-second background-refresh bound under system contention is not.
The page renders the last bounded snapshot without waiting for that worker.
Tracing is a separate correctness check, not a timing benchmark: `strace -f -c
-e trace=network,getdents64` recorded no network or directory-enumeration calls
for the catalogue benchmark. The background receipt adapter separately reads
its bounded flat application-history directories as described above.

The database remained **253,648,896 bytes**, SHA-256
`5e4fe5d7989641607413442744874d62057540d4b86baf89d6fc1250ebb92225`.
SQLite may use temporary sort files; zero persistent writes does not mean zero
operating-system temporary I/O. No real library contents were changed.

### Proven baseline failures

A detached disposable worktree at the exact main SHA reproduced all nine core
test failures seen during workspace validation (8,906 passed, nine failed, one
ignored at the baseline). No baseline fixes were imported:

| Test (core module prefix) | Existing failure |
| --- | --- |
| `database::tests::dat_expected_inventory::migrations_0011_and_0012_are_registered` | Schema assertion expects 12, database has 16 |
| `database::tests::library_schema_contains_no_cheat_catalogue_journal_or_backup_tables` | Expected table list omits existing scan fingerprints |
| `diagnostics::tests::stage_1a_introduces_no_database_migration` | Schema assertion expects 12, database has 16 |
| `disk_format::tests::the_database_schema_and_migrations_are_unchanged` | Expected migrations stop at 12 instead of 16 |
| `database::tests::custom_alias_outranks_the_existing_filename_path_heuristic` | Existing alias/rescan classification failure |
| `database::tests::removing_alias_and_rescanning_restores_the_built_in_alias_fallback` | Existing alias/rescan invalidation failure |
| `database::tests::removing_alias_and_rescanning_restores_unknown_when_nothing_else_matches` | Existing alias/rescan invalidation failure |
| `database::tests::saved_source_assignment_reclassifies_unknown_rvz_on_rescan` | Existing source-assignment/rescan invalidation failure |
| `database::tests::scan_while_manual_is_active_shadow_records_the_custom_alias_fallback` | Existing shadow-classification/rescan failure |

The five classification failures are not dismissed as formatting or stale
schema assertions; they remain a separate baseline correctness workstream.
