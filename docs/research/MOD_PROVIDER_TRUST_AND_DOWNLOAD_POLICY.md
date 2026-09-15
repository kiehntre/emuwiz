# Mod Provider Trust and Download Policy

Research date: 2026-09-15  
Scope: policy and architecture only. No network client, provider adapter,
payload download, installation, or production Rust change is included.

## 1. Executive Summary

EmuWiz should treat provider trust, catalogue-record trust, payload integrity,
game compatibility, and legal permission as separate decisions. A known provider
may publish a bad, changed, or unsuitable payload; a valid SHA-256 proves that
bytes match an expected digest but does not prove that the payload is safe,
compatible, or redistributable.

The safe future path is:

`catalogue record → explicit payload selection → URL policy → bounded download → observed SHA-256 → archive/patch inspection → selected-game compatibility → review → transaction plan → explicit apply`

The first implementation slice should be a pure URL/payload-policy validator
plus a deterministic fake transport contract. It must be usable in tests without
opening sockets. Real transport should be added only after this boundary is
reviewed.

## 2. Existing EmuWiz Foundation

The local catalogue model already provides:

- `ModCatalogueProvider` with provider name, record ID, source page, schema
  version, and import time;
- provider-neutral record fields for title, author, version, platform,
  category, declared identities, region/revision, destination intent, and
  provenance;
- payload IDs, URLs, advisory sizes, versions, regions, archive hints, and
  supplied `SHA-256`, `SHA-1`, or `MD5` declarations;
- validation for bounded text, payload count, duplicate IDs/hashes, URLs,
  identities, and unsafe destination intent;
- catalogue compatibility against selected-game verified native evidence;
- a review projection that keeps supplied hashes unverified until bytes are
  available, distinguishes rights status, and asks for bounded download and
  inspection as later checks.

`archived_mod_package` and `standalone_patch` already provide the local,
read-only inspection handoff. Existing compatibility states remain the source
of truth; this policy does not add a second game identity or package model.

The missing policy is an outer trust/transport layer: provider status, payload
host provenance, redirect rules, byte identity, cache lifecycle, and the point
at which a downloaded object is eligible for inspection or apply.

## 3. Provider Trust Model

Use discrete states, not a numerical score:

| State | Meaning | Permitted use |
| --- | --- | --- |
| `ProviderKnown` | Stable identity, documented source, and a reviewable provenance path are established | Display; record matching; payload may be offered if the record and URL also pass policy |
| `ProviderUnverified` | Provider identity or terms are incomplete, but metadata is structurally usable | Display with warning; matching remains evidence-based; download requires review |
| `ProviderBlocked` | Explicit policy, security, abuse, or rights reason prevents use | Do not offer records or payloads for download |
| `ProviderUnavailable` | Temporary fetch/import failure, not a trust judgment | Preserve last-known data as stale; do not silently refresh or present it as current |

Provider status should consider stable provider identity, source documentation,
terms visibility, transport capability, metadata consistency, and record
provenance. It must never be inherited by the payload. A provider record should
retain provider identity, source page, import time, and any terms URL rather than
only a display name.

## 4. Catalogue Record Trust

Record validation is staged:

1. **Display:** structurally valid metadata may be shown with provider, source
   page, freshness, and rights status. Missing terms are a warning, not an
   implied licence.
2. **Match:** declared platform, Title ID, Media ID, serial, region, or revision
   is candidate evidence only. Strong compatibility comes from selected-game
   verified evidence and the existing compatibility assessor.
3. **Offer:** a user may be shown a download choice only when the payload has a
   valid permitted URL, the provider is not blocked, the record is not malformed
   or contradictory, and warnings are visible. “Offer” is not “verified”.
4. **Apply:** only a locally downloaded payload that has a calculated identity,
   completed bounded inspection, compatible selected-game evidence, safe output
   planning, and explicit confirmation may reach an apply transaction.

Block or require review for malformed identities, conflicting identity fields,
unsupported categories, invalid URLs, duplicate payload identity, stale records
whose currentness matters, and missing provenance where the action would rely on
it. A record may remain displayable while its payload remains blocked.

## 5. URL / SSRF Policy

This is a future downloader contract, not an implementation in this phase.
OWASP recommends strict validation, explicit allowlists where possible, disabling
unsafe redirect following, and validating both URL and resolved addresses; it
also notes that SSRF is not limited to HTTP and can involve schemes such as
`file`, `ftp`, `gopher`, and `data` ([OWASP SSRF Prevention Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Server_Side_Request_Forgery_Prevention_Cheat_Sheet.html)).

### Accepted by default

- Absolute `https` URLs only.
- Standard host syntax parsed by one canonical URL library.
- No username, password, embedded credentials, or ambiguous escaping.
- Normal HTTP(S) ports only unless a separately reviewed provider policy allows
  another port.

### Refused by default

`file:`, `ftp:`, `data:`, `gopher:`, `blob:`, custom schemes, relative URLs,
URLs with userinfo, malformed/ambiguous authority, localhost names, loopback,
unspecified, link-local, multicast, benchmarking, private RFC1918 ranges, and
IPv6 equivalents or IPv4-mapped forms. RFC 1918 defines private IPv4 space and
RFC 6890 records special-purpose ranges that must not be treated as ordinary
public destinations ([RFC 1918](https://www.rfc-editor.org/rfc/rfc1918),
[RFC 6890](https://www.rfc-editor.org/rfc/rfc6890)).

HTTP should be refused for unattended transport. If product requirements later
need it, HTTP can be an explicit `RequiresReview` choice with a clear insecure
transport warning; it must not silently downgrade from HTTPS.

### Redirects and DNS

- Cap redirect hops at a small fixed number, recommended five.
- Reparse and revalidate every `Location` hop; never validate only the first
  URL. Refuse loops and repeated equivalent hops.
- HTTPS-to-HTTPS host changes may be permitted for ordinary CDN/object-store
  delivery only when the final host and every resolved address pass policy.
  Record the complete chain and show the final payload host. A scheme change is
  refused by default.
- Resolve A and AAAA records, reject if any selected destination is disallowed,
  and handle IPv4-mapped IPv6 consistently. Recheck at connection time and bind
  the request to the validated address where the transport permits it.
- A hostname that first resolves publicly and later resolves privately is a
  rejection, not a successful download. DNS validation is not a substitute for
  network egress controls.

The canonical URL grammar should be treated as authoritative; RFC 3986 defines
the generic URI syntax ([RFC 3986](https://www.rfc-editor.org/rfc/rfc3986)).
Parser disagreement is a refusal condition.

## 6. Download Size Bounds

Provider-declared size and `Content-Length` are advisory inputs. The future
transport must enforce a hard byte ceiling while streaming, including responses
with no length and responses that lie about their length. A declared size over
the ceiling is rejected before transfer; an actual stream crossing the ceiling
is aborted and the partial object is not publishable.

Maintain separate bounds for:

- response bytes on disk;
- headers and redirect metadata;
- archive member count, member size, total expanded size, path depth, and
  inspected metadata bytes;
- patch operation/record count and declared output size.

The compressed payload limit does not replace existing archive expansion limits.
No allocation should be based directly on an untrusted archive or patch size.
Limits should be named, configurable within a safe range, and recorded in the
inspection provenance.

## 7. Hash / Signature Policy

Keep three facts separate:

1. **Supplied hash:** provider metadata, unverified;
2. **Observed hash:** SHA-256 calculated over the exact downloaded bytes;
3. **Verified expectation:** observed hash equals the normalized supplied digest
   using a supported algorithm.

SHA-256 is the preferred payload identity and integrity check. SHA-1, MD5, and
CRC32 may be retained for legacy ecosystem matching or display, but are not
cryptographic authenticity claims. NIST guidance covers approved hash use and
security-strength considerations ([NIST SP 800-107 Rev. 1](https://csrc.nist.gov/pubs/sp/800/107/r1/final)).
Store the raw declared algorithm/value plus a normalized representation; do not
discard meaningful provider text.

A detached signature, signed manifest, or release attestation can add
authenticity only when it binds the exact bytes to a recognized signing identity
and the verification key/trust path is itself known. It does not replace archive
inspection or game compatibility. GitHub release assets and artifact attestations
are examples of provider ecosystems with distinct metadata/provenance surfaces,
not a universal trust guarantee ([GitHub release assets API](https://docs.github.com/en/rest/releases/assets)).

## 8. No-Hash Payloads

No expected hash is not automatically a denial. A future user-reviewed flow may:

`download bounded bytes → calculate SHA-256 → inspect locally → show the new local identity → require review before apply`.

The result is `DownloadedUnverified`, never “verified against provider”. No-hash
payloads must not be auto-applied, silently substituted for a prior payload, or
treated as equivalent solely because the URL and filename match.

## 9. Catalogue Provider vs Payload Host

Record at least two provenance roles:

- **Catalogue provider:** supplied the record, game declarations, description,
  source page, terms, and expected payload metadata.
- **Payload host:** served the bytes, including final host, URL, redirect chain,
  retrieval time, response filename, and transport result.

For example, a provider page may point to a GitHub release or a CDN. The
catalogue provider does not automatically vouch for an unrelated host. A
cross-host redirect is not inherently unsafe, but it requires per-hop URL/IP
validation and visible provenance.

## 10. Cache / Content-Addressed Storage

Catalogue metadata may be cached with provider ID, record ID, fetched-at time,
schema version, source page, terms URL, and freshness/expiry. Stale metadata may
be displayed as stale but must not silently become a current download offer.

Downloaded payloads should be stored as immutable, content-addressed objects by
observed SHA-256 after the bytes are fully received. A supplied expected hash is
additional provenance, not the local key until verified. URL, record ID, version,
redirect chain, provider host, payload host, and retrieval timestamp remain in a
sidecar provenance record.

Benefits are deduplication, stable review/apply input, and protection from URL
drift. Costs are disk lifecycle, privacy, rights retention, and the need to
garbage-collect unreferenced objects. Failed or blocked payloads should be
deleted by default; a user-requested quarantine may retain them with an explicit
unsafe/unverified label and no apply eligibility.

Thumbnails and inspected manifests are derived caches and must be invalidated by
content identity, not just URL. Cache deletion must never delete a user-owned
file or a derived artifact still referenced by provenance.

## 11. Licence / Rights Boundary

The Arisen audit already separates application-code licensing from catalogue
metadata and individual mod/content rights. EmuWiz must retain that distinction.
The following is a product-policy matrix, not a legal conclusion:

| Material | Index | Link | Download for user | Cache locally | Mirror/redistribute | Bundle |
| --- | --- | --- | --- | --- | --- | --- |
| Descriptive metadata | Usually possible with attribution/provenance review | Usually possible | Not applicable | Bounded metadata cache | Only with rights/terms review | Only after review |
| Screenshots/artwork | Rights review | Source link preferred | User choice | Opt-in, bounded | No by default | No by default |
| IPS/BPS/UPS or other patches | Source/record link | Yes if permitted | User-selected, terms-visible | Content-addressed, lifecycle-controlled | No by default | No by default |
| Replacement assets/saves | Record/link with rights provenance | Yes if permitted | Explicit user choice | Review required | No by default | No by default |
| Cheats/trainers/plugins | Describe/link | Yes if permitted | Explicit warning and review | No execution; lifecycle-controlled | No by default | Legal/security review |
| Executables/scripts | Link with warning | Yes if permitted | Explicit user action only | Quarantine or opt-in only | No by default | Never by default |

Missing licence information means `Unknown` or `RequiresReview`, not permission
to redistribute. “Download for the user” and “mirror/bundle” are different
actions. Terms can change, so provenance used by a historical review must be
immutable. Questions about copyrighted game-derived bytes, patches, saves,
cheats, screenshots, and jurisdiction-specific exceptions require legal/policy
review rather than an automated claim.

## 12. Executable / Script Policy

EXE, DLL, ELF, AppImage, shell, PowerShell, batch, Python, trainer, plugin, and
other executable-like members are content, not instructions to EmuWiz. They may
be downloadable only after explicit user selection and policy checks, but are
always flagged `UNSAFE_TO_AUTOMATE` / `RequiresReview` and are never executed,
loaded, invoked, or granted emulator/device access by the mod pipeline.

Archive inspection may list and classify them. A package containing one is not
necessarily rejected if the user wants documentation or a tool, but it cannot
cross an automatic apply boundary.

## 13. Download-to-Inspection Pipeline

Every future payload follows this sequence:

1. Validate the catalogue record and select exactly one payload/version.
2. Show provider, source page, payload host if known, URL, declared size/hash,
   rights state, compatibility evidence, and warnings.
3. Validate URL scheme, authority, redirect policy, resolved addresses, and
   size limits before transport.
4. Stream into EmuWiz-owned staging storage under a hard byte ceiling.
5. Calculate SHA-256; compare with the supplied expectation when supported.
6. Publish only an owned immutable content-addressed object after size/hash
   verification; record URL/host/redirect/retrieval provenance.
7. Run existing standalone patch or archived-package inspection, including
   executable, nested-archive, traversal, and decompression limits.
8. Match against selected-game verified evidence. A provider declaration alone
   cannot upgrade compatibility.
9. Produce the existing review projection and only then a separate transaction
   plan. Apply remains explicit and immutable-base/ownership protected.

No stage may infer verification from the previous stage’s success.

## 14. Record / Payload Drift

Historical provenance must be append-only. These are distinct events requiring a
new review:

- the same URL serves different bytes;
- the same record/version receives a different hash;
- a payload URL changes host or path;
- declared Title ID, Media ID, platform, region, or revision changes;
- terms/licence change or disappear.

The observed SHA-256 identifies each byte version. A cached prior object remains
usable only by its own identity and provenance. Never silently rewrite a reviewed
transaction’s record to follow current provider metadata. Mark old records stale
or superseded and require fresh compatibility/integrity review.

## 15. Privacy

Catalogue lookup and device credentials must be separate. A provider request may
expose IP address, user-agent, referrer, timing, game title, platform, Title ID,
Media ID, or account/session identifiers. EmuWiz should avoid sending selected
game identity unless required by a documented provider API, minimize headers,
avoid telemetry, and make source-page/payload requests visible in logs without
logging credentials. Console credentials and device identity must never be sent
to a catalogue provider merely to browse mods.

## 16. Failure States

Conceptual outcomes are:

- `ReadyForDownload`: record, URL, provider status, rights visibility, and
  limits pass; no bytes have yet been verified.
- `RequiresReview`: weak/no hash, cross-host delivery, incomplete rights,
  executable content, weak compatibility, or another visible warning.
- `Blocked`: unsafe URL/IP, blocked provider, malformed record, rights/security
  policy denial, unsupported scheme/category, or limit violation.
- `DownloadedUnverified`: bounded bytes received and local SHA-256 calculated,
  but no expected digest matched or no digest was supplied.
- `DownloadedVerified`: bytes match a supported supplied digest; this does not
  imply compatibility or safe execution.
- `InspectionFailed`: local patch/archive inspection failed or was malformed.
- `IdentityMismatch`: bytes do not match the expected digest, or strong selected
  game evidence contradicts the payload.
- `ProviderUnavailable`: metadata or transport endpoint could not be reached;
  preserve stale provenance without presenting currentness.

Errors must retain the stage, provider/payload identity, and whether any bytes
were retained or deleted. No failure may fall back to filename similarity or a
confident update/apply state.

## 17. Testable Transport Contract

The future transport should depend on a fakeable interface, not a global network
client. A request fixture should specify URL, response status/headers, body
bytes, redirect location, resolved addresses, and connection outcome. Tests must
cover:

- permitted HTTPS response;
- malformed URL and unsupported schemes;
- HTTP refusal or explicit review;
- redirect depth limit, loop, HTTPS downgrade, host change, and private-address
  redirect;
- DNS returning public plus private A/AAAA records, IPv4-mapped IPv6, and
  rebinding between validation and connection;
- declared size over limit, unknown length, truncated body, and stream overrun;
- SHA-256 match, mismatch, unsupported/weak hash, and no expected hash;
- server error, timeout, cancellation, and changed bytes at the same URL;
- successful staging followed by failed publish, with no final artifact and
  owned staging cleanup.

Assertions should include the exact request/redirect decision, observed byte
count, SHA-256, retained provenance, and final state. No test needs internet
access.

## 18. Recommended First Implementation Slice

Implement one pure core policy boundary, without sockets:

**`ModDownloadPolicy` URL/payload validator plus a deterministic fake transport
contract.**

It should accept existing catalogue payload/provider data and policy settings,
return a typed decision (`ReadyForDownload`, `RequiresReview`, or `Blocked`),
validate schemes/authority/ports and a supplied expected size/hash, and model
redirect/IP decisions through injected fake observations. It should not resolve
DNS itself, open files outside caller-owned staging, or download bytes. The next
slice can add a bounded real transport behind the same contract only after these
tests pass.

Likely scope for that later implementation is a new narrowly named core module
plus tests; reuse `ModCataloguePayload`, `ModCatalogueProvider`, existing URL
validation, hash representations, archived-package inspection, and review
projection. Do not add provider-specific scraping or duplicate catalogue fields.

## 19. Deferred Work

- real HTTPS transport with DNS/IP pinning and OS/network egress defense;
- provider adapters and signed catalogue update policy;
- content-addressed payload cache and garbage collection;
- signature/attestation verification where an actual mod ecosystem supplies a
  stable trust key;
- user-facing download review and explicit consent UX;
- archive/package-to-apply integration after local inspection is complete;
- device deployment adapters for PS3/Xbox, with separate credentials,
  capability checks, backups, and remote transactions;
- legal review of metadata, payload, patch, save, cheat, screenshot, and bundle
  rights by jurisdiction.

### Research references

- [EmuWiz Arisen Studio workflow audit](ARISEN_STUDIO_MOD_WORKFLOW_AUDIT.md)
- [OWASP SSRF Prevention Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Server_Side_Request_Forgery_Prevention_Cheat_Sheet.html)
- [RFC 3986: URI Generic Syntax](https://www.rfc-editor.org/rfc/rfc3986)
- [RFC 1918: Private Internets](https://www.rfc-editor.org/rfc/rfc1918)
- [RFC 6890: Special-Purpose IP Address Registries](https://www.rfc-editor.org/rfc/rfc6890)
- [NIST SP 800-107 Rev. 1: Approved Hash Algorithms](https://csrc.nist.gov/pubs/sp/800/107/r1/final)
- [GitHub REST API: Release Assets](https://docs.github.com/en/rest/releases/assets)

