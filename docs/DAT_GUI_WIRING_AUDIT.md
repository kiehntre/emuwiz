# DAT / Verified Identity GUI Wiring Audit

## Executive summary

The authoritative code already has a mature DAT/identity backend and a real
read-only selected-game DAT panel. The missing work is not a new scanner or
identity engine: it is clearer terminology, recovery guidance, and a bounded
library-level presentation of data that is already persisted.

The smallest safe P0 is GUI/projection work around the existing
`LibraryDatIdentitySummary` and selected-game panel. It must not open a DAT,
hash a ROM, scan sources, contact a network service, write the database, or
choose a winner among conflicting sources. A true all-library aggregate needs
one later, explicitly specified database read because source-level counts
cannot be summed without double-counting games checked by multiple DATs.

No optional implementation was made. The relevant GUI already exists, but a
useful P0 needs a deliberate view-model/placement pass rather than an isolated
label edit.

## Existing backend capability

| Area | Authoritative seam | Existing capability |
| --- | --- | --- |
| DAT model/parsers | `dat/model.rs`, `dat/parser.rs`, `dat/parsers/*` | `DatSource`, `ParsedDat`, games, ROMs, disks, BIOS declarations, checksums, metadata, warnings and Logiqx/ClrMamePro/MAME parsing. |
| Ecosystems | `DatEcosystem` | No-Intro, Redump, TOSEC, MAME software list/listxml, FBNeo and honest generic Logiqx/ClrMamePro labels. |
| Sources | `dat/sources/{mod,config,validation}.rs`, `dat/managed_sources.rs` | Local/managed source path, ownership/origin, enabled state, platform assignment, validation health and revisions. |
| No-Intro packs/variants | `identity_source/no_intro/{import,pack_import,managed_lifecycle,status_reporting}.rs` | Headered, headerless, aftermarket, BIOS and unknown catalogue variants, derived only from catalogue metadata. |
| Matching/audits | `dat/audit.rs`, `dat/index.rs`, `dat/sources/audit_run.rs`, `dat/disk_audit.rs`, `dat/archive/*` | Exact/probable/ambiguous/not-in-DAT/no-evidence verdicts for loose files, archive members and discs. |
| Sets/BIOS | `dat/set.rs`, `dat/firmware_evidence.rs` | Dependency-aware arcade/set state and typed Redump BIOS evidence; BIOS component identity is kept distinct from game identity. |
| CHD/discs | `chd_identity.rs`, `chd_logical_media.rs`, `disc_evidence_collector.rs`, `dat/archive/chd.rs` | CHD header/hash/metadata and disc identity facts; specialist handling refuses unsafe guesses. |
| Structural identity | `game_identity.rs`, `content_evidence.rs`, `platform_evidence_fusion/*`, platform header/boot modules | Product/serial/title/disc IDs, executable CRCs, headers and provenance/confidence independent of a DAT audit. |
| Persistence | migration 0008 and `Database` DAT identity APIs | One persisted `PersistedLibraryDatIdentity` per archive/DAT source; stale revision marks; reconstruction without a re-audit. |
| Cached aggregation | migrations 0011/0012, `Database::platform_dat_coverage` | Indexed SQL-only per-platform/source coverage: checked, verified current/stale, probable, unmatched, ambiguous, unknown, duplicates and gated expected/missing/full-set. |
| Structural fact cache | `verified_identity_cache.rs`, `diagnostics/verified_identity.rs` | Persisted launch-gating facts and Doctor information findings for missing/stale/unknown structural evidence. |

`Database::library_dat_identity_summary_for_item` reconstructs the selected
item's `LibraryDatIdentitySummary` from persisted data plus current source
availability/revision and known file facts. It does not reopen a DAT, rehash
the item or re-run an audit. `dat/sources/audit_cache.rs` is an explicit-audit
hash cache, not a render-time cache.

## Existing GUI exposure

| Surface | Facts currently exposed | Limitation |
| --- | --- | --- |
| Library selected game | `selected_game_panel.rs` calls `dat_identity_panel::show_dat_identity_section` | Detail exists only after selecting an item; ordinary rows/Home do not explain overall DAT health. |
| DAT Identity panel | Status/explanation, source/ecosystem/revision, canonical game/member, region/revision, matched algorithm/value, freshness, candidates, conflict detail and set details | Empty state is terse; set details include raw debug-style state strings; recovery guidance is incomplete. |
| Structural selected evidence | `selected_evidence_page.rs` shows header/boot/platform evidence, local No-Intro lookup and optional Hasheous lineage | This is a separate inspection/evidence family, not persisted DAT verification. |
| Verify Games / DAT Sources | `dat_sources_page.rs`, `dat_coverage_panel.rs` show source health, validation/audit controls and on-demand coverage | Coverage is source/platform scoped and hidden behind a source expansion. |
| Doctor | `doctor_page.rs` plus verified-identity diagnostics | Correctly reports non-current launch facts, not general DAT coverage. |
| Playing Library | `playing_library_page.rs` | Uses a chosen DAT for a deliberate planning workflow, not the current Library explanation. |
| RomM | `identity_sources_page.rs`, `romm_source.rs`, `romm_game.rs` | Provider-specific identity/conflict information; must never be labelled DAT verification. |

The selected-game wiring is live, not a dormant component. `load_snapshot_from`
loads persisted DAT identities into `CachedLibrarySnapshot.dat_identities`;
`library_view.rs` selects them by archive ID; `selected_game_panel.rs` renders
them. This path is read-only.

## Gap matrix

| User question | Current backend data | Current GUI | Gap | Action |
| --- | --- | --- | --- | --- |
| Is this ROM verified by a DAT? | `DatVerificationState` persisted per source | Selected game badge | Available after selection only | P0 clearer selected status and Library route to Verify Games. |
| Which DAT / ecosystem? | `DatSourceProvenance`, `DatEcosystem` | Source/Ecosystem rows | Available now | Preserve with novice labels. |
| Which entry matched? | `DatCanonicalIdentity` | Canonical DAT name/member | Available now | Label “Verified as” and “Catalogue entry”. |
| Match basis? | `DatHashEvidenceSummary`; structural facts separately | Matched algorithm/value | Available now but easy to conflate | P0 explain DAT hash basis versus game identity evidence. |
| Confidence? | exact/probable/ambiguous/conflicting/no-match states | Badge plus prose | Available now | Use stable plain-language labels below. |
| Conflicts? | candidate names, conflict detail, multiple source rows | Accordions/multi-source warning | Present but buried | P0 primary “Needs review” and an evidence CTA. |
| Headered/headerless? | `NoIntroVariant`, `CatalogueVariant` | DAT source/pack workflows | Available with small projection | P1 carry source variant into game provenance. |
| BIOS identity? | firmware evidence and set dependencies | Source configuration/specialist consumers | No general readiness view | P1 separate BIOS card; never include in game verified count. |
| CHD/disc identity? | CHD/disc modules and selected evidence | Applicable structural inspection | Not in DAT summary | P1 read-only selected detail where fact exists. |
| Why did it fail? | no-match/no-evidence/filename-only/conflict/freshness/source health | Partial explanations | Available with small projection | P0 state-specific recovery wording. |
| What should I do next? | existing Validate/audit/source routes | Only rename guidance is explicit | Hidden | P0 link to real Verify Games, no duplicate workflow. |

### Classification

- **AVAILABLE_NOW:** single cryptographic verification, probable CRC32+size
  result, source/ecosystem/revision, canonical names, matched hash,
  candidates, conflict detail, source availability and stale provenance.
- **AVAILABLE_WITH_SMALL_PROJECTION:** plain-language recovery text; a source
  variant row; selected CHD/disc fact where already inspected; a selected-item
  distinction between structural and DAT evidence.
- **NEEDS_BACKEND_SEAM:** a correct all-library all-enabled-source aggregate
  that counts each archive once; BIOS-missing totals; exact current hash
  freshness for every row without an audit; durable cross-family DAT versus
  structural conflict facts.
- **NOT_CURRENTLY_MODELED:** a universal source-independent DAT-covered flag;
  generic serial/title-ID/product-code DAT matching; remote freshness beyond
  recorded source metadata.

## User-facing status model

| Label | Exact model state | Novice explanation / next step |
| --- | --- | --- |
| Verified | `VerifiedSingleMatch` and current provenance | “Matches one catalogue entry by [algorithm].” No action. |
| Verified, needs re-check | verified with stale source/file provenance | “The file or catalogue changed since this check.” Validate/audit again. |
| Likely match | `Probable` | “CRC32 and size found one likely entry; this is not cryptographic proof.” |
| Needs review | ambiguous candidates, `Conflicting`, or incompatible multi-source results | “EmuWiz found more than one plausible answer and did not choose.” |
| Not found in this DAT | `NoMatch` | “The checked catalogue has no matching entry.” It does not mean corruption. |
| Not checked yet | no persisted summary | “No DAT result is stored for this game.” Configure/validate a relevant DAT, then audit. |
| More evidence needed | filename-only or no-usable-evidence | “A filename is not proof” / “No comparable checksum was available.” |
| Source unavailable | persisted summary whose source is disabled/unconfigured | “This is a past result; the catalogue is not currently enabled.” |
| BIOS needs attention | future firmware/set projection only | Must name the BIOS/firmware component; never describe it as a game-DAT result. |

Structural facts stay separate: use **Game identified**, **Platform likely** or
**Identity check needs attention** for `GameIdentityReport` and name its
actual basis (serial, product/title ID, executable CRC, disc/boot structure or
header) in advanced details.

## Per-game identity panel

Keep the selected-game location; it is the minimum correct detail surface.
Present two visibly distinct sections:

1. **Game identity** — existing structural platform/product/disc facts. The
   default is one sentence; technical details retain raw values and lineage.
2. **DAT check** — existing summary. Default rows: **Verified as**,
   **Catalogue**, **Match basis**, **Checked against**. Advanced details hold
   hash value, revision/freshness, region, candidates and set data.

When multiple sources exist, retain the existing fail-closed grouping and say
“Multiple catalogue results.” Do not turn a filename match into verification
or silently pick a source.

## Library summary

The backend can cheaply produce correct **per-platform/per-source** counts
today through `Database::platform_dat_coverage`: owned, checked, verified
current/stale, probable, unmatched, ambiguous, unknown, duplicate identities
and, only when the explicit platform/current inventory gate holds,
expected/missing/full-set. Verify Games already reads this without scanning.

It cannot safely produce a global “verified N of M” by adding source rows:
one archive may have No-Intro and Redump results. P0 should therefore not add a
global total. P1 needs a typed SQL aggregate grouped by current archive ID,
with an explicit multi-source counting policy and conflict preservation. BIOS
missing is separate and cannot be inferred from game coverage.

## Conflict handling

| Condition | Current model | Required GUI behaviour |
| --- | --- | --- |
| Multiple DAT candidates | `AmbiguousMultipleCandidates` | Needs review; list candidates; never select one. |
| Conflicting DAT evidence | `Conflicting { detail }` | Explain and route to source/audit review. |
| Different source outcomes | multiple persisted summaries | Expand each result; never count as corroboration automatically. |
| No matching hash | `NoMatch` | “Not found in this DAT,” not “bad file.” |
| Variant mismatch | `UnsupportedVariant` / variants | Explain representation mismatch; do not fall back headered↔headerless. |
| Stale result | provenance freshness/revision mark | Retain past result, label it non-current, offer revalidation. |
| Unsupported/incomplete evidence | audit/inspection refusal | State what was not inspected and why; do not fabricate identity. |

## DAT source presentation

Use only parsed/configured names: No-Intro, Redump, TOSEC, MAME software
list/listxml, FinalBurn Neo, Generic Logiqx and Generic ClrMamePro map directly
to `DatEcosystem::label()`. Local sources should be **Local DAT — [ecosystem]**
and managed entries **Managed — [provider]**; path and origin belong in
advanced details. “Imported pack” describes installation, not verification
strength. Redump BIOS entries must explicitly say BIOS/firmware component.

## Header / variant presentation

`NoIntroVariant` and `CatalogueVariant` already model **Headered**,
**Headerless**, **Aftermarket**, **BIOS**, and **Unknown** from catalogue
metadata. The No-Intro pack UI already labels aftermarket/Love Pack, BIOS, and
unknown variants.

For novices, show a source pill only when the representation affects the
result: **Headered ROM set**, **Headerless ROM set**, **Homebrew / aftermarket
set**, or **BIOS / firmware set**. Say that headered/headerless expects a
different file representation; do not imply one is better. Keep “variant not
stated by catalogue” in advanced details. Current `DatSourceProvenance` does
not carry the variant to the selected-game panel, so this is P1 projection
work, not a new detector.

## P0 / P1 / P2

### P0

- Improve the existing selected-game DAT panel hierarchy, novice labels,
  empty/stale/unavailable/conflict guidance and real Verify Games route.
- Keep structural identity and DAT verification distinct.
- Reuse snapshot summaries only; no scanner, parser, matching engine,
  database migration, network call or render-time write.

### P1

- Add one typed database aggregate for all-library counts with defined
  multi-source semantics.
- Carry parsed catalogue variant into selected-game provenance.
- Add separate BIOS/firmware readiness and selected CHD/disc detail views.

### P2

- Candidate/source comparison, raw DAT disk/set graph browsing, evidence
  export, saved verification filters and a durable cross-evidence conflict
  model.

## Exact implementation plan

| Likely file | Bounded change |
| --- | --- |
| `crates/archivefs-gui/src/dat_identity_panel.rs` | Extract a pure display/status/guidance projection; replace novice-facing raw/debug terms, including set-state debug formatting. |
| `crates/archivefs-gui/src/selected_game_panel.rs` | Distinguish “Game identity” from “DAT check” and expose existing navigation request only. |
| `crates/archivefs-gui/src/library_view.rs` / navigation owner | Wire the existing Verify Games route while preserving selected-card responsive layout. |
| Existing panel/GUI tests | Cover verified, probable, no DAT, no-match, conflict, stale/unavailable, variants and absence of raw enum/debug strings. |

P0 must not change `archivefs-core`, migrations, source scanning, DAT parser,
audit runner, identity inspector, RomM integration or configuration. It must
not read files, hash content, parse a DAT, open network connections or write
database state while Library renders.

For P1, add one `Database` read API adjacent to `platform_dat_coverage` only
after the all-library counting contract is documented. It must group by archive
ID, return typed unverifiable states, preserve conflicts and use persisted SQL
data only.

## Definition of Done

- A selected game clearly states whether a current DAT result exists, its
  source/ecosystem and entry, evidence basis, confidence and safe next step.
- Structural identity and DAT verification cannot be mistaken for each other.
- No-match, stale, no-evidence and conflict states cannot produce a false
  positive or destructive recommendation.
- No global count appears until its multi-source semantics are defined/tested.
- Verify Games remains the sole audit/coverage workflow; Library links to it
  rather than duplicating a scanner.
- Rendering remains read-only with no new backend engine, migration, scan,
  network request or LBC change.
