# Multi-media set topology engine

Worktree: `/tmp/emuwiz-multimedia-set-topology-engine`.
Branch: `feature/multimedia-set-topology-engine`.
Starting main: `89da3a13e6c75054c11dad5c8c5c841359fc3a2f`, verified against a fresh
`git fetch origin main`. The former main worktree was on an integration branch;
this branch was created directly from the actual main commit, with no changes
to that checkout or any Needs Attention, recovery, DAT or MAME worktree.

## Inventory recorded before implementation

| Area | Existing implementation and established behavior | Topology boundary |
| --- | --- | --- |
| Optical references | `ingestion/cue_bin.rs` resolves bounded CUE tracks; `ingestion/gdi.rs` validates descriptors and resolves every track. `platform_evidence_fusion/cue_m3u_parsing.rs` safely parses CUE/M3U relative references. Existing support-file and set-destination models keep companions attached. | A BIN track is not another disc; an M3U expresses an order but cannot prove release identity. No playlist writer is reused. |
| Optical identity | `game_identity::{GameIdentityReport, IdentityEvidence, inspect_catalogued_game_identity_in_roots}` retains status, confidence, method and archive-member provenance. PS1/PS2 serials, PS2 executable CRC, Saturn/Dreamcast/Sega CD product codes, Dolphin game ID/revision/disc number/region, and other platform IDs are already exposed. CHD logical-media/specialist readers and CDI evidence exist. | Consume verified facts. PS serials/product codes are medium identifiers and are not automatically a common release key. Dolphin disc number is zero-based in the native header. |
| Optical equivalence | `optical_fingerprint::CanonicalOpticalFingerprint` and its comparator define a reviewed representation-independent sector identity. Current coverage is a single data-track CUE/CHD slice. CHD header hashes alone are explicitly insufficient for that equivalence. | Consume existing fingerprints; normal planning does not hash entire optical media. |
| Existing grouping | `platform_evidence_fusion/library_grouping.rs` actually consumes confident representation-aware DAT verdicts and `dat::classification::multidisc_group_key`. It supports strict `(Disc N of M)` declarations, retains release labels and feeds the full-library/set-destination report. | Extend through an independent generic core model; do not change this DAT-backed pipeline. Reuse the strict DAT token parser first. |
| Floppy structure | `disk_format::inspect_disk_format` uses `safe_read`, bounded structure checks, cancellation and explicit refusals. Metadata includes ST FAT12 geometry, STX/Pasti records, CPCEMU DSK sides and +3/PCW metadata, D64 BAM/directory names and IDs, FDS sides, TRD/SCL, D88, DFS, DC42 and other disk families. | Valid geometry proves format, not release identity. DSK and ST are shared formats; D64 is shared by Commodore machines. Labels and small disk IDs are supporting evidence only. |
| Amiga and preservation | The platform/content registry recognizes ADF/ADZ/IPF/DMS and ST/MSA/STX, G64, Apple NIB/WOZ/2MG and other extensions. Discovery already exposes structural ADF evidence. Amiga disk/filesystem and ADZ modules exist; the HDF/RDB traversal is not a general IPF/DMS decoder. | Do not manufacture native release IDs for preservation formats without an existing decoder/evidence source. Accept DAT/native evidence supplied through the narrow input API. |
| Tape analysis | `tape_analysis::analyze_tape` composes existing parsers and exposes entries, names, BASIC/CODE/data kinds, addresses, lengths, checksums, blocks, metadata and loader clues. `tape_identity` provides ZX TAP, TZX and CDT parsing. `commodore_tape` supplies pulse TAP machine facts and T64 directories. Other modules cover Oric, BBC UEF/WAV, MSX, Atari, Dragon/CoCo and audio analysis. | Entry names/loader fingerprints are not release or whole-medium identities. Pulse-only TAP has no exposed file directory. TZX/CDT structure alone does not settle ZX versus CPC. |
| DAT identity | `dat::library_identity_summary::LibraryDatIdentitySummary` distinguishes verified cryptographic matches from CRC-probable, ambiguous/conflicting and stale results. It retains canonical game/ROM names, region/revision, source revision, hash evidence and freshness. | Consume current verified summaries only for trusted claims. Define explicit future release/media membership input; no DAT parser, authority or completeness changes. |
| Platform identity | `platform::PLATFORMS`, `platform_for_alias`, `platform_by_id` and `PlatformIdentityResolution` are the existing canonical identity registry/resolver. Shared extensions do not settle platform. | Reuse these APIs, not a competing platform registry. |
| Catalogue/archive evidence | `SourceFolderView`, `ingestion::discovery::SourceDiscoveryReport`, archive listing/content observations, `archive_member_resolver` and `LibraryPlanInput` retain source paths, member indexes, evidence and precomputed physical/normalized hashes. | Accept catalogue-wide records and preserve container/member locators; never extract or treat one archive as one medium by default. |
| Launch/readiness | `launch::input_projection::VerifiedIdentityFact` feeds pure per-emulator request projections. `launch::readiness` carries readiness, blockers and warnings. Emulator-environment models expose read-only findings and canonical platform preferences. | Expose a pure swap plan for a later adapter. No launch execution, firmware inspection, GUI, config writes or current launch-path edits. |


## Public model and read-only boundary

`archivefs_core::media_set` exports `MediaRecord`, `MediaEvidence`, `index_media`,
`resolve_index`, `inspect_paths` and `media_swap_plan`. All planning functions
operate on owned evidence snapshots. They neither open a database nor return
mutation actions. The optional inspector opens explicit files for bounded reads;
it does not scan directories, extract archives, decompress ADZ to temporary
files, hash whole collections, launch processes, write playlists or change ROMs.

`MediaSetIdentity` is a release key, namespaced by its authority. `MediaIdentity`
is a medium key. A `MediaSetMember` is a logical/physical medium containing one
or more `MediaRepresentation`s; each retains its original `MediaRecord`, source
path, optional archive-member index/raw name, platform, format, availability,
full evidence, resolved ordinal, side, role, variant, count provenance,
confidence and conflicts. A product code is not automatically a content hash.

The model supports optical, floppy and tape families, all thirteen requested
roles, ordinal units (disc, disk, tape, part, reel), and separate side numbers.
Release variants contain region, revision, language, video standard and edition.
Requirements may identify a medium by authority ID, ordinal, role and/or side,
and distinguish required from optional media. Explicit relationships retain
load/swap transition kinds and are checked for missing targets and cycles.

The six states are `COMPLETE_SET`, `INCOMPLETE_SET`, `AMBIGUOUS_SET`,
`CONFLICTING_SET`, `UNVERIFIED_SET`, and `UNSUPPORTED_SET`. Every result retains
sorted structured conflict codes and deterministic explanations. Completeness
means the declared topology inventory is satisfied by observed representations
with sufficient release and positioning evidence. It is **not** a whole-payload
integrity audit, emulator readiness result or permission to launch.

## Evidence and safety rules

Precedence, strongest first:

1. Verified native facts.
2. Current, singly verified cryptographic DAT identity.
3. Explicit embedded metadata.
4. Other corroborating metadata, including an existing playlist order.
5. Filename title/ordinal/variant declarations.
6. Directory proximity.
7. Fuzzy-title evidence (retained, never used to join records).

Selection is deterministic. Different claims at any strength remain explicit
conflicts; the stronger value is retained but does not erase the disagreement.
Region/revision/language/video/edition tuples partition release buckets, including
known versus unknown variants. Incompatible partitions receive a diagnostic
explaining why they were kept separate. This deliberately misses some legitimate
sets rather than mixing releases. Different proven release keys are never joined
by similar titles, directories, labels or product IDs.

Filename parsing reuses the existing strict DAT multi-disc classifier and adds
bounded token parsing for Disc/Disk/CD/D/Tape/Cassette/Part/Reel, word numbers
One–Ten, side A/B/1/2, `1of2`, `1 of 2`, delimited `(1-2)` and prefixed `Disk1-2`.
Dots, underscores and punctuation are separators; sequel digits outside the
matched token range survive. Bare ambiguous title suffixes such as `Title 1-2`
are retained as title text. Common region, revision (`Rev A`, `RevA`, `v1.2`),
language and edition tags are retained separately. Unrecognized tags remain in
the title, conservatively preventing joins. Combined conflicting region tags
are not guessed to mean “World.” Role aliases include Loader/Program tape,
Boot disk and Workbench/Utility disk; they remain filename-strength claims.

A filename count stays filename evidence. Even when every named position exists,
filename-only sets remain unverified. Unknown expected totals are never inferred
from the largest ordinal. A known release/count with weak positions also remains
unverified. An unnumbered observed member is an unknown position, not proof that
all numbered positions are absent. Bonus/extras/save/demo/audio media do not
silently fill numbered game-media slots. Explicit manifests can require them.

## Optical rules and coverage

Representation vocabulary: CUE/BIN, CHD, GDI, CDI, ISO, GCM, RVZ, WIA and WBFS.
Recognition of a format does not promise a native decoder for every platform.
The inspector consumes the existing native reader's supported combinations and
retains its warnings/refusals for others. M3U is a descriptor input, not a medium:
its references are expanded within file/read limits, its order is supporting
evidence, and rejected/cyclic/empty references remain visible. CUE BIN tracks
and GDI track files are companions rather than extra discs.

Verified PS1/PS2 serials, PS2 executable CRC, Saturn, Dreamcast and Mega/Sega CD
product fields, PSP/3DO/PC-FX medium evidence are consumed without a new parser.
They do not prove common multi-disc release membership or representation
equivalence. GameCube/Wii game ID is a shared product key; revision/region remain
separate and the native zero-based disc field is converted to a one-based
ordinal. A native disc number does not reveal the total disc count. PC Engine CD
and other registry platforms can use DAT/explicit authority evidence; no missing
native release parser is invented.

Install/play manifests may establish an ordered role pair without fabricated disc
numbers. Optional bonus, extras and audio discs stay separate roles.

## Floppy rules and coverage

Amiga ADF/ADZ/IPF/DMS; Atari ST ST/MSA/STX; CPC DSK; Commodore D64/G64/D81;
Apple NIB/WOZ/2MG/DO/PO; D88/D77/FDI/XDF/DIM; FDS; TRD/SCL; SSD/DSD and DC42
can be represented. Platform IDs always come from the existing registry or
conclusive native evidence, never an ambiguous extension alone.

Existing bounded disk-format inspection supplies actual geometry, DSK sides,
D64 labels/directories and other supported structures. On Linux, ADF filesystem
inspection reuses the Amiga OFS/FFS reader through an already-approved pinned
file descriptor. It retains the volume label without promoting that label to a
unique release identity. The existing ADZ reader creates an anonymous temporary
decompression file, so this diagnostic deliberately does not call it. IPF/DMS,
G64/NIB and other preservation formats require external DAT/authority evidence
where existing structural readers cannot prove identity.

Side and ordinal are independent. Disk 1 A/B and Disk 2 A/B are two disks with
four side representations when a separate-side layout is supplied. Side labels
alone leave physical topology unverified. A validated whole-medium DSK/ST image
can contain both sides and produces one insertion step. D64 alone does not
prove flippy-disk topology. Different side roles (boot on A, data on B) remain on
one physical medium and are preserved in individual swap steps.

## Tape rules and coverage

TAP, TZX, T64, CDT, CAS, UEF and WAV are representable for the existing registry's
tape platforms. ZX Spectrum, Commodore 64 and CPC context is supported alongside
existing Oric, BBC, MSX, Atari and Dragon/CoCo analysis. The existing deep analyzer
supplies format, entries, BASIC/CODE/data kind, addresses, lengths, checksum,
block metadata and loader clues. No new tape decoder or loader classifier is
implemented. Shared TZX/CDT structure does not resolve ZX versus CPC context;
pulse Commodore TAP is explicitly reported as exposing no file directory.

Tape entry names and loader fingerprints are supporting facts, never unique
release IDs. Tape ordinals, sides and part/reel ordinals remain distinct. Explicit
loader→program or program→data relationships can order unnumbered media, and
cycles or contradictions with numbered order block the plan. Tape output is a
load/swap plan, never an imposed optical playlist format.

## Duplicate representations

Different formats do not imply equivalent content. Equivalence requires a
trusted medium ID explicitly marked `ExactFile`, `CanonicalContent`, or
`AuthorityMapping`, within a compatible release/variant grouping. Native product
codes and executable CRCs use `None`. An existing canonical optical fingerprint
can prove the narrowly supported CUE/CHD content equivalence without rehashing.
Matching trusted medium mappings can likewise relate ADF/IPF and TAP/TZX.
Conflicting same-position candidates stay ambiguous. Strong media identity is
not used as a bridge between conflicting or unrelated release identities.
Cross-name equivalence without a shared release key remains a conservative
limitation; a future authority adapter can provide proven membership.

## Catalogue, authority and future integration

The core indexes all supplied records; source directories are not grouping
boundaries. Existing `LibraryPlanInput` hierarchy is consumed through a narrow
adapter without invoking organisation actions. Archive-member locators can carry
previously acquired evidence without extraction. The explicit path diagnostic
does not enumerate archives or load the user's configured catalogue itself.

`attach_dat_identity` consumes `LibraryDatIdentitySummary` only when verification
and freshness permit it. Namespaced source/version keys prevent cross-version
identity collisions. `MediaTopologyAuthority::evidence_for` is the narrow future
interface for richer release manifests, medium equivalence, counts, variants and
load relationships. Its producer must bind evidence to the current source and
scan generation; JSON snapshots are assertions from that producer, not verified
by deserialization. No persisted derived cache or DAT authority implementation is
added, so there is no silent derived-state cache reuse.

`media_swap_plan(&MediaSet, Option<&MediaProfile>)` is the exact future launch
boundary. It returns ordered media, sides, preferred representations, alternatives,
transitions, profile/readiness hints, warnings, blockers, confidence and provenance.
Optical sequence, floppy swap and tape load are distinct semantics. Only the
selected compatible profile can rank formats; without one, multiple proven
representations have no universally preferred format. A blocked/unknown profile
readiness hint remains informational: the future launch adapter must enforce
its existing emulator, firmware and readiness rules separately.

Before GUI integration: bind this API to a current catalogue evidence snapshot,
add profile capability projection, decide how the user reviews unverified or
conflicting topology and alternative representations, and validate real library
cases per supported emulator. A later launch adapter must revalidate source
identity/availability and readiness, then translate family-specific plans to
emulator-supported swaps. Playlist serialization, writes and execution require
separate work and are absent here. GUI code is untouched.

## Diagnostic CLI

`emuwiz-cli media-set inspect <paths...> --platform "Amiga"`

`emuwiz-cli media-set explain <path> --catalogue records.json`

`emuwiz-cli media-set plan <paths...> --platform "ZX Spectrum" --profile profile.json`

Output is JSON (`--json` is accepted). `--catalogue` reads an explicit
`Vec<MediaRecord>` snapshot; `--profile` reads a `MediaProfile`. Default inspection
limits are 256 files and 128 MiB of bounded reader work. Native optical inspection
reserves the existing reader's 64 MiB maximum; floppy and tape reads use existing
bounds. `--max-files`, `--max-read-bytes` and `--no-optical-native` make diagnostic
cost explicit. No path arguments are recursively scanned. CLI snapshots are
limited to 100,000 records/128 MiB; the core API has no collection-size cap.

## Complexity and deterministic validation

Grouping uses ordered maps keyed by platform/family/release/variant, followed by
medium equivalence, ordinal, role and identity indexes. Lookup cost is O(log N),
with no catalogue-wide pairwise comparison. Input order, evidence order and
serialization order are normalized. Individual sets above 4,096 representations
are unsupported for completeness proof; malformed count/ordinal/side bounds
fail explicitly. Transition provenance is local to its endpoints, rather than
copying the entire collection's provenance into every transition.

All fixtures are synthetic and legal. They cover optical 2/3-disc sets,
install/play/bonus, missing middle discs, CUE/CHD equivalence and competing images;
floppy 2/4-disk sets, boot/data, four sides/two disks, ADF/IPF; tape pairs,
loader/program relationships, sides and TAP/TZX; variants, sequel titles,
colliding labels, unknown positions/counts, malformed relationships, DAT/native
adapters, archive-member locators, descriptor cycles and bounded read-only input.
Adversarial title/region/revision/language/edition/platform conflicts never join
incompatible sets. Exact validation commands and measured results follow below.

Confidence describes grouping evidence: a proven release can still have an
unverified expected inventory. The benchmark's `candidate_comparisons` counter
counts per-record candidate bucket probes plus manifest candidate predicate
checks; internal ordered-map key comparisons are not instrumented. It is not an
assertion that B-tree lookups take constant time.

## Measured performance

Measured on this host using the final implementation in an unoptimized test
build, with debug information and incremental compilation disabled. This is an
in-memory topology benchmark, not a disk scan or native image read benchmark.
It creates 100,000 mixed optical/floppy/tape records in 25,000 four-medium releases,
with per-record filename and trusted synthetic authority evidence, spread over
four source directories.

| Measurement | Result |
| --- | ---: |
| Fixture preparation | 2,863 ms |
| Catalogue indexing | 2,505 ms |
| Grouping and completeness | 2,688 ms |
| Candidate probes/checks | 100,000 |
| Logical releases | 25,000 |
| Entire test process wall time | 9.05 s |
| Maximum resident memory | 555,392 KiB (542.4 MiB) |
| Major page faults / swaps | 0 / 0 |

The maximum RSS is the whole test process, including fixture construction and
retained output, measured by `/usr/bin/time -v`; it is not an allocator-only
measurement. Timing is one observation on an active host, not a latency guarantee.
The benchmark asserts the expected release count, complete synthetic inventories,
and a linear bound on candidate probes. Reproduce its phases with:

```sh
CARGO_TARGET_DIR=/tmp/emuwiz-topology-benchmark-target \
CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 \
cargo test -p archivefs-core --lib \
  media_set::tests::hundred_thousand_record_index_benchmark -- --exact --nocapture
```

Run the resulting test executable directly under `/usr/bin/time -v` to measure
process RSS without including Cargo or compiler memory.

Role-only manifests with competing distinct media are ambiguous. A manifest that
only names medium IDs does not manufacture unknown ordinals or roles: its
inventory may be present while the topology remains unverified. Matching
requirements are indexed once regardless of how many records repeat them;
all original provenance remains on the input representations.

## Validation and baseline classification

Validation uses a target directory belonging only to this worktree, with
`CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0` and
`CARGO_INCREMENTAL=0`. No compiled artifacts are shared with another source tree.

Commands executed:

```sh
cargo fmt --check
cargo check --workspace --offline
cargo clippy --workspace --all-targets --all-features --offline -- -D warnings
cargo test -p archivefs-core --offline --lib media_set::tests -- --nocapture
cargo test --workspace --offline --no-fail-fast -- --test-threads=8
git diff --check
```

The 56 focused tests pass, including the final role-only ambiguity and unknown
position guards. CLI smoke tests exercise `inspect`, `explain` and `plan`, JSON
snapshots, cross-directory grouping, honest unverified results and plan blockers.
Input hashes and the input file list are identical before and after those calls.

The full workspace run exercised both CLI binaries (335 passed each), all CLI
and core integration targets, all three GUI aliases (2,627 passed and two ignored
each), and doctests. Those targets passed. The initial core run also exposed an
incorrect byte count in a new T64 test fixture; that fixture was corrected and
that fixture passes both focused and full-core reruns. Two final topology guards were subsequently
added and revalidated independently; no GUI or existing integration implementation
changed during this work.

Nine existing core failures were reproduced with a **separately compiled clean
export** of starting main `89da3a13e6c75054c11dad5c8c5c841359fc3a2f` at
`/tmp/emuwiz-multimedia-baseline`, using
`/tmp/emuwiz-multimedia-baseline-target`. All nine failed with the same assertions:

- `database::tests::dat_expected_inventory::migrations_0011_and_0012_are_registered`
- `database::tests::library_schema_contains_no_cheat_catalogue_journal_or_backup_tables`
- `diagnostics::tests::stage_1a_introduces_no_database_migration`
- `disk_format::tests::the_database_schema_and_migrations_are_unchanged`
- `database::tests::custom_alias_outranks_the_existing_filename_path_heuristic`
- `database::tests::removing_alias_and_rescanning_restores_the_built_in_alias_fallback`
- `database::tests::removing_alias_and_rescanning_restores_unknown_when_nothing_else_matches`
- `database::tests::saved_source_assignment_reclassifies_unknown_rvz_on_rescan`
- `database::tests::scan_while_manual_is_active_shadow_records_the_custom_alias_fallback`

The first four concern migration/schema expectations (main has migrations through
16 and a `scan_fingerprints` table). The other five concern existing alias/source
rescan behavior. No schema, scanner, recovery, DAT authority or GUI files were
changed to mask those baseline failures. The workspace is therefore not reported
as universally test-green.

Final source results:

| Check | Result |
| --- | --- |
| Formatting | Passed |
| Workspace check | Passed |
| Workspace Clippy, all targets/features, warnings denied | Passed |
| Focused topology tests | 56 passed |
| Full core rerun | 8,962 passed, nine reproduced baseline failures, one ignored |
| CLI aliases | 335 passed each |
| GUI aliases | 2,627 passed, two ignored each |
| Integration targets and doctests | Passed |
| 100k benchmark | Passed; measured above |
| Staged/unstaged whitespace checks | Passed |
