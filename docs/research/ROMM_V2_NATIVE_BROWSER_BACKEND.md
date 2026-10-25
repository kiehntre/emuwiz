# Native RomM browser backend for GUI-v2

Starting EmuWiz main: `370f059f48a03469be4da2757d5028edc216e515`.
Final validation base: `8648e5665e6827941e84e4e043716bc69ce0b303`.
Main independently advanced through a GUI media-set review (`73970bfdd66c`)
and media/ingestion registry additions (`8648e5665e68`); both were incorporated
by rebasing this isolated branch. Neither intervening commit touched RomM.
This candidate provides live read-only browsing in
`identity_source::romm::browser`. Here **v2 means the future EmuWiz GUI-v2**;
requests use RomM's existing `/api` endpoints, not an invented `/api/v2`.

## API evidence and version gate

The existing EmuWiz client/capability fixtures were verified against RomM
5.1.0. Current upstream was inspected on 2026-10-03 at
[`rommapp/romm` commit `893c77f47d238c5509985dd049747e2d372f00e4`](https://github.com/rommapp/romm/tree/893c77f47d238c5509985dd049747e2d372f00e4),
committed 2026-10-03 02:56:24 UTC. The latest published release was
[`5.3.1`](https://github.com/rommapp/romm/releases/tag/5.3.1), published
2026-09-23. Upstream is AGPL-3.0; no upstream implementation was copied.

Contract evidence:

- [ROM list/detail routes](https://github.com/rommapp/romm/blob/893c77f47d238c5509985dd049747e2d372f00e4/backend/endpoints/roms/__init__.py):
  list pagination, suppression controls, and selected-item `GET /api/roms/{id}`.
- [Filter parameter definitions](https://github.com/rommapp/romm/blob/893c77f47d238c5509985dd049747e2d372f00e4/backend/handler/database/rom_filters.py):
  `search_term` and the plural, repeatable `platform_ids` query parameter.
- [Platform response](https://github.com/rommapp/romm/blob/893c77f47d238c5509985dd049747e2d372f00e4/backend/endpoints/responses/platform.py)
  and [ROM/file responses](https://github.com/rommapp/romm/blob/893c77f47d238c5509985dd049747e2d372f00e4/backend/endpoints/responses/rom.py):
  field names, provider identifiers, media references and related files.
- [Pagination parameters](https://github.com/rommapp/romm/blob/893c77f47d238c5509985dd049747e2d372f00e4/backend/endpoints/responses/base.py):
  offset/limit are server-side; EmuWiz imposes a smaller page limit.

The browser retains the existing minimum supported major, 4, and limits this
new API to studied major families 4 and 5. Older/future major versions return
`UnsupportedVersion`; existing import/version policy is unchanged. Runtime
OpenAPI discovery remains authoritative for each operation/filter. Major 4
compatibility is conditional on that declaration and the actual bounded
responses; it is not a claim of a real-server test for every 4.x release.
Missing optional capabilities produce `PartiallySupported`, never a guessed
endpoint. A 404 heartbeat permits OpenAPI version fallback. Conflicting major
versions, absent version evidence or incompatible shapes fail explicitly.

## Read-only operations

| Backend operation | Endpoint | Behaviour |
|---|---|---|
| `discover` | `/api/heartbeat`, `/openapi.json` | Public bounded reads; versions and advertised GET capabilities. |
| Authentication probe | `/api/roms?limit=1&offset=0` | At most one authenticated item; platform listing fallback if paging is absent. |
| `platforms` | `/api/platforms` | Typed IDs, names, slugs, counts, system identifiers and mapping results. |
| `games` | `/api/roms?limit=…&offset=…` | One page per call; optional server search/platform filter. |
| `game_detail` | `/api/roms/{id}` | One selected record with bounded typed file/hash relationships. |

Construction performs zero I/O; callers explicitly discover before browsing.
Discovery reports `Supported`, `PartiallySupported`, `UnsupportedVersion`,
`Unreachable`, `AuthRequired` or `AuthFailed`, with a safe typed error when an
attempt failed. Missing optional features remain visible as capability flags.
No write method or arbitrary endpoint method is public on the browser.
Upload/delete/rename/rescan/metadata/favourites/artwork/collection mutation and
ROM download are absent. No cache, catalogue or database writer is accepted.

## Pagination and filtering

Pages request 1–200 items; invalid limits fail before network I/O. Callers
receive offset, page size, optional total and explicit next/previous offsets.
The adapter checks echoed offsets/limits, item count, duplicate IDs, totals,
short non-terminal pages and arithmetic overflow. A server that ignores the
platform filter is refused rather than filtered locally. Unknown totals remain
unknown; a full page offers one next offset, without an automatic paging loop.
Concurrent library changes can make a page inconsistent; callers should
explicitly refresh rather than silently accepting/retrying it.

Only advertised `search_term` and `platform_ids` are exposed. Unsupported
filters fail before a request. Search is URL-encoded, bounded to 512 bytes and
rejects control characters. No whole-library client-side filter exists.
Advertised `with_files`, `with_char_index`, `with_filter_values`,
`with_rom_id_index` and `group_by_meta_id` are sent as `false`. Current upstream
otherwise includes a full-library ID index and sidecar filters by default.
Selected detail is the place to obtain file relationships; game rows do not
eagerly fetch files or artwork. On older servers without suppression controls,
the same hard response/list limits remain authoritative.

## Typed projections and authority

`RommServerInfo`, `RommBrowseCapabilities`, `RommPlatformSummary`,
`RommPlatformMapping`, `RommBrowseFilter`, `RommBrowsePage`, `RommGameSummary`,
`RommGameDetail`, `RommRelatedFile`, `RommArtworkRef` and
`RommBrowseProvenance` contain no public `serde_json::Value`.
`RommIdentityHint` reuses `ExternalIdentityRecord` rather than inventing another
identity representation. It carries title, provider/platform IDs, original
provider path, size, regions/revision, validated CRC32/MD5/SHA-1, provider
timestamps, synopsis/genres, related files/siblings and normalization evidence.
Per-file hashes use the same `ExternalHash::parse` validation. Invalid hash
fields remain rejected evidence and never become valid checksums.

Both `normalise_platform` and `normalise_rom` are called unchanged. Platform
mapping reports exact canonical labels, known aliases, conflicting recognized
platform/filesystem slugs as ambiguous, or unknown. It uses the existing RomM
normalizer/registry and adds no fuzzy matcher or platform alias table.
Unknown custom platforms and original provider slugs remain visible.
`KnownAlias` describes an existing normalization association; compatibility
groupings such as FDS-to-NES remain external candidates, not verified hardware
equivalence. Provider fields outside the existing normalizer/detail projection
are not invented or promoted into a new metadata representation.

Every identity remains `ExternalVerification::Unmatched`. The normalizer's
output is retained unchanged, including existing path/evidence semantics;
tests compare its serialized bytes with the browser projection. Provenance
includes provider, approved server ID, fixed endpoint and caller-supplied
observation time. No `VerifiedIdentityFact`, DAT binding, source-health fact,
catalogue truth or user metadata is written. Reconciliation/conflict resolution
belongs to the existing explicit evidence workflow, outside this browser.
Related files/siblings are provider relationships, not an inferred disc order.

Cover references reuse `ArtworkReference`; current `merged_screenshots` and
`screenshot_path` receive lightweight `MediaReference` projections. Hosted and
public references remain distinct. URLs/paths are provenance, not authorization
to fetch them. Future lazy artwork fetches must pass the existing approved
RomM artwork policy; browser row/detail calls fetch zero media bytes.

## Security, bounds and errors

The browser receives the existing `ValidatedRommSource` and `RommTransport`,
and uses `RommClient` for every GET. It adds no configuration, credential store,
HTTP agent, endpoint policy or normalizer. `get_json` gains only sibling-module
visibility, not a public generic request API. Existing token file permission/
regular-file policy and redacted `RommToken` remain the caller's configuration
boundary. Authentication uses an Authorization header, never a URL. Public
probe rejection is `AuthRequired`; authenticated rejection is `AuthFailed`.

The production `UreqTransport` ignores environment proxies, verifies TLS,
permits no redirects, enforces 5-second connect/30-second browser request
timeouts and bounds body reads to 8 MiB. No automatic retry exists. The shared
client now preserves a timeout occurring during body reading and uses a fixed
message for unexpected transport failures. Browser errors discard arbitrary
transport/provider text and distinguish unreachable, timeout, TLS, auth,
endpoint refusal, HTTP 4xx/5xx, rate limiting, malformed JSON, oversized body,
pagination inconsistency, incompatible schema, unsupported features and bounds.
Browser/configuration Debug output never renders the credential.

Additional bounds: 1,024 platforms/document collection or object entries,
200 games/page, 64 per-record list entries/related files/siblings/media refs,
8,192 bytes/string or object key, nesting depth 32, 100,000 document nodes,
positive signed-64-bit-compatible IDs and checked numeric conversions. Local
OpenAPI parameter references have bounded resolution; external references and
cycles are refused without another network request. Parsing holds one bounded
response and its typed projection, so memory is independent of library size.
Details with more than 64 files fail explicitly instead of silently dropping
relationships; the existing importer/normalizer's own policy is unchanged.

## Future GUI-v2 wiring and validation

A future controller can retain the configured source and canonical transport,
construct a browser, explicitly discover, list platforms, ask for one filtered
page and fetch one selected detail. It should display the typed capabilities,
mapping uncertainty, hash refusals, external provenance and errors; refreshes
and artwork fetches remain explicit. Calls belong in the existing background
worker path, with the caller's cancellation flag. No GUI/readiness code changes in this
candidate. The historical checkpoint was inspected only for context and was
not cherry-picked: current-main configuration/client/normalizer are authoritative.

Deterministic local HTTP fixtures exercise the actual approved client: discovery,
auth, versions, platforms, pagination, encoding/filtering, detail, hints/media,
malformed/oversized bodies, header/body timeouts, HTTP failures, cancellation,
credentials, GET-only methods and process-isolated environment proxy handling.
Additional focused assertions cover schema/list/depth/numeric bounds, local
OpenAPI references, unchanged normalization and deterministic projections.
An ignored, explicitly selected real-server smoke test reuses existing settings
and `load_token_file`; it only discovers, lists platforms, reads three rows and
one detail. It never changes settings or caches.

Validation used the isolated `/tmp/emuwiz-romm-native-browser-QgZxol/target`
with offline/locked Cargo, four build jobs and dev/test debug information
disabled. The full core suite used eight test threads.

| Check | Result |
|---|---|
| New browser cases | 29 passed; one explicitly opt-in real-server test ignored. |
| RomM focused suite | 159 passed; one opt-in test ignored. |
| Identity/provider/artwork/proxy suite | 814 passed; two opt-in tests ignored. |
| Core `--lib` on final base | 10,865 passed; zero failures; four ignored; 227.05 seconds. |
| `cargo check --offline --locked --workspace` | Passed; four existing GUI warnings, no GUI edits. |
| `cargo fmt --all -- --check`, `git diff --check` | Passed. |
| Scope and collision check | Only five allowed files; no required file dirty elsewhere. |
| Protected normalizer | Byte-for-byte unchanged. |

The optional configured real-instance attempt stopped at the canonical
endpoint policy: its hostname did not resolve. Existing credential loading
passed; no HTTP request was emitted and no real-server verification is claimed.
Deterministic fixtures cover both successful and failing network paths.
Production source/config/cache files, GUI, migrations and authoritative main
were not modified by this task; no push or promotion was performed.
