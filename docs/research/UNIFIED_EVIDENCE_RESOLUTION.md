# Unified Evidence Resolution

Status: implemented as a read-only core service. This document describes the
current boundary; it is not a replacement for the existing DAT, topology,
identity, launch, or recovery records.

## Existing evidence inventory

The resolver consumes facts already produced by these current-main primitives:

| Evidence area | Existing producer | Resolver treatment |
| --- | --- | --- |
| Native media/content identity | `content_evidence`, platform boot/header evidence, `platform_evidence_fusion` | `NATIVE_VERIFIED` or `CONTENT_VERIFIED` claims |
| DAT authority and completeness | `dat::authority`, `dat::library_identity_projection`, database authority queries | `AUTHORITY_VERIFIED` claims with digest, coverage and freshness scope |
| Archive/member observations | archive inspector, ingestion and archive-member resolution | structured/content claims, with the source observation as derivation root |
| Filename and directory context | `media_set::naming`, platform detection and source-folder context | bounded weak claims; never exact identity by themselves |
| Media topology | `media_set` records and resolved `MediaSet` results | membership, ordinal, side, role and expected-count claims |
| Launch identity | `launch::evidence_bridge` and existing platform-specific facts (PS1/PS2 serial, CRC, title IDs, product codes) | typed game identity, platform and launch-target claims |
| BIOS/readiness | `launch::readiness` projections and emulator/profile reports | readiness claims, kept separate from content identity |
| Explicit user decisions | existing manual selections and user-confirmed layers where available | first-class `USER_CONFIRMED` claims; never a Safe Apply operation |

The resolver does not parse a file or re-open the database. Adapters pass it
already-observed facts, so there is no second truth store and no hidden scan.

## Claim model

`EvidenceClaim` has a subject, a typed `ClaimProperty`, typed
`EvidenceValue`, source class, strength, polarity, provenance and
`EvidenceScope`. Supported properties include platform, release identity,
region, revision, language, media-set membership/ordinal/side/role/count,
BIOS dependency, launch target and emulator compatibility.

`EvidenceScope` carries DAT ecosystem/platform/digest/version, whether
coverage is complete, authority state, scan generation, source identity,
parser schema and emulator generation. Missing fields remain `None`.

`EvidenceProvenance::derivation_root` is the independence boundary. A filename
parser and a TOSEC parser that both read that filename share one root and count
once. A native header and DAT entry normally have different roots and can
corroborate one another.

## Precedence and resolution

Precedence is domain-specific, not a universal score. The current table is:

| Domain | Highest to lowest |
| --- | --- |
| Platform | native, content, authority, human, structured, filename, directory, fuzzy |
| Game/release identity | native, authority, content, human, structured, filename, directory, fuzzy |
| Region/revision | native, content, authority, human, structured, filename, directory, fuzzy |
| Topology/readiness/launch properties | native, authority, content, human, structured, filename, directory, fuzzy |

Strength (`verified`, `strong`, `corroborated`, `weak`) breaks ties within a
source class. Independent agreeing roots produce a transparent corroboration
line; they do not turn a weak filename claim into native proof.

A stronger claim may select a value while preserving weaker incompatible claims
as a conflict. Equal-strength incompatible claims are `CONFLICTING` and
`BLOCKED`. A native claim conflicting with a user confirmation is always
surfaced as `CONFLICTING`; it is never silently overwritten. User confirmation
is guidance, not automatic metadata mutation.

`NO_SUPPORT` means a source did not find support. It is not a contradiction.
Only an explicit contradictory claim can create a contradiction, and DAT
absence is negative evidence only when the authority scope says coverage is
complete for the relevant platform/version.

Results distinguish:

* `VERIFIED` — strong system evidence or independent agreement;
* `PROBABLE` — bounded non-authoritative evidence;
* `AMBIGUOUS` — multiple plausible interpretations without a deterministic winner;
* `CONFLICTING` — incompatible claims require review;
* `UNVERIFIED` — no positive claim;
* `UNSUPPORTED` — no-support only, not a negative identity;
* `STALE` — only stale authority/evidence remains usable.

Action safety is separate: `SAFE_TO_ACT`, `REVIEW_REQUIRED`, or `BLOCKED`.
Verified identity can be safe to act on, while probable identity requires
review and conflicts are blocked. BIOS/readiness remains independent: a
verified game with missing BIOS is valid content identity but not launch-ready.

Each unresolved result returns a typed missing-evidence requirement, such as
native identity, matching DAT authority, revision evidence, a media-set member,
verified BIOS readiness, user confirmation, or fresh evidence.

## Media-set, launch and future Needs Attention boundaries

The resolver consumes current `media_set` topology results. It can corroborate
release membership, ordinal, side, role and expected count with DAT evidence;
it does not alter topology algorithms. A topology/DAT identity disagreement is
preserved as a conflict and is not auto-repaired.

The launch bridge remains authoritative for verified launch facts. The resolver
offers a narrow typed projection opportunity; it does not rebuild launch plans
or execute a process. Readiness claims are similarly explanatory only.

No Needs Attention code is changed here. A future adapter can map results to
`IDENTITY_CONFLICT`, `REGION_CONFLICT`, `REVISION_CONFLICT`,
`INSUFFICIENT_EVIDENCE`, and `STALE_EVIDENCE` using the existing pipeline.

## Staleness and determinism

Authority state and generation fields are carried on claims. Stale-only
resolutions are `STALE` and require fresh evidence. Callers should invalidate
derived caches when scan generation, DAT digest/version, parser schema or
emulator generation changes. The resolver itself is pure and has no persistent
cache.

Claims are sorted by subject, property, typed value, derivation root and claim
ID. Results, support, conflicts, explanations and `resolution_digest` are
therefore independent of insertion order.

## Real collection audit

This pass remains read-only. A bounded inspection of the available local
catalogue at `<catalogue-path>` found
102,343 archive rows, 22 source-folder rows, and schema 12; `quick_check`
returned `ok`. The current database has 0 `verified_identity_facts`, 0
`library_dat_identities`, 0 DAT expected entries and 0 DAT audit results, so
it cannot honestly produce platform-level agreement, DAT completeness, or
real conflict examples. No identity values are fabricated. The next real
collection audit should feed a bounded sample through `Database::open_read_only`
after identity/DAT observations exist, and record only counts and provenance
summaries—not paths, credentials or raw hashes.

## Performance

`EvidenceIndex` indexes by `(subject, property)`, so resolution examines only
the relevant claim bucket and avoids global O(N²) comparison. The companion
synthetic example generates one million claims and reports generation,
ingestion, resolution, conflict count, candidate-comparison count, peak RSS
(`VmHWM`) and a SHA-256 result digest:

```text
cargo run -p archivefs-core --release --example evidence_resolution_benchmark
```

The measured numbers belong to the machine/run and should be recorded beside
the commit rather than treated as universal budgets. The result digest is the
repeatability check.

## Future integration

The safe next seams are read-only adapters: convert existing DAT authority,
`MediaSet`, launch facts and readiness reports into claims; expose resolution
and explanations to GUI/Needs Attention; and invalidate disposable derived
results when source generations change. GUI routing, Safe Apply, database
migrations, ROM mutation, DAT refresh/download and launch execution remain
outside this module.
