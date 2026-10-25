# Job 40: external artwork and metadata provider validation

The available **real ES-DE exports and saved RomM evidence pass** their Blue
checks after two focused repairs. **Live RomM and ScreenScraper retrieval are
BLOCKED**, for the specific availability reasons below. This report does not
certify successful online retrieval, login, quota recovery or a fresh live
provider refresh where none could be executed.

Baseline: freshly fetched `origin/main`,
`c2c7e9f2c20c215b02b3c3473b14800d5c965717`, including merged PR #149.
Final production source: `4799a0e93a33324677f67e673f4c144c816caf92`.
Worktree: `/home/davedap/emuwiz-job36-artwork`, reused without creating another
Cargo cache; branch `qa/job40-provider-validation`.
Evidence: `/home/davedap/emuwiz-qa-artifacts/job40-providers/evidence/`.
Normal CI and exact-candidate independent review are tracked by the pull
request containing this report; this document alone does not claim promotion.

## Inventory and prior validation

[Job 36](JOB36_ARTWORK_METADATA.md) already validated the complete synthetic
local journey: cover/gallery delivery, per-field precedence and alternatives,
source toggles/refresh, missing art, local cache replacement and restart,
620×480 usability and preservation. Its 80 focused checks, native evidence,
green full CI and completed Codex review were retained rather than repeated.
Job 40 adds real exported/cached evidence and the two regressions below.

Gold's Gamer View and detail actions reuse Core's bounded artwork cache; Blue
uses `MediaIndex` and bounded local/network workers over the same provider
framework. Existing descriptive sources are local catalogue data, ES-DE,
RomM and accepted ScreenScraper metadata. LaunchBox local XML/media supplies
artwork; bundled platform hardware is an explicitly labelled fallback.
ScreenScraper in EmuWiz is explicit metadata enrichment, with session-only
credentials and reviewed accepted values; it does not download its media
references into Blue. ES-DE is a local exported-data provider here, not an
EmuWiz online scraper.

Priority remains per field and artwork kind: local override, verified facts,
local evidence, provider cache, bundled fallback. Equal-priority Blue inputs
retain catalogue, ES-DE, RomM, accepted ScreenScraper order. There is no
configurable provider-order preference. Provider metadata cannot verify game
identity. Neither repair changes precedence, matching or identity authority.

RomM identity snapshots and artwork thumbnails are separate. Artwork keys bind
server, record, kind and provider reference/timestamp; the cache is bounded to
1 GiB. Local display thumbnails use canonical file identity, size and precise
timestamps and are bounded to 512 MiB. Workers check caches first. Recheck
rebuilds the media index; importing a fresh RomM catalogue is a separate explicit
operation. No new time-to-live, stale-match heuristic or cache format was added.

The worktree audit inspected 211 worktrees. Other isolated worktrees have
uncommitted changes in shared GUI files, in different feature areas; their
physical files and branches were untouched. The unfinished identity-provider
POC and RomM-manual work were also left untouched. `worktree-overlap.json`
records exact paths/hunks; `inventory.json` records provider details.

## Provider results

| Provider/case | Result | Executed evidence and limitation |
| --- | --- | --- |
| RomM live retrieval / refresh | **BLOCKED** | Existing source is enabled, with a configured nonempty owner-only token file. Its configured hostname fails DNS; read-only local Docker discovery finds zero running RomM candidates. `live-availability.json` records this without credentials. No account or service was created or reconfigured. |
| RomM saved cover and metadata in Blue | **PASS** | Two unchanged existing provider records and one existing thumbnail were copied into private QA storage, without copying the token. `03-fixed-cached-cover.png`, `05-metadata-cards.png` and `06-romm-field-provenance.png` show the real saved cover, description, year, genre, players and rating with RomM labels. `10-offline-cache-hit.png` shows `cache_hit: true`, zero network time, DNS failure and “Cached image used: yes” together. |
| RomM uncached screenshots | **BLOCKED** for successful retrieval; **PASS** for honest failure display | The selected record contains three public-only screenshot references and no cached screenshots. No public scraper URL was downloaded. `08-romm-outage-gallery-detail.png` reports “Picture unavailable / RomM cannot currently be reached,” with retry controls, rather than claiming no picture exists. A fresh hosted screenshot response requires an available service and suitable provider evidence. |
| ES-DE real exported media and metadata | **PASS** | Existing default ES-DE root contains 20 gamelists. Three catalogue references were used in the private profile overall. The actual exported cover, description, developer, release date, genre, players and screenshot reach Blue (`15-esde-game-details.png`, `16-esde-metadata.png`, `17-esde-field-provenance.png`, `19-real-esde-screenshot.png`). Local covers also remain visible beside cached RomM art during the outage (`12-esde-selected.png`). No ES-DE scraping or account access was performed. |
| ScreenScraper live enrichment and real accepted-record display | **BLOCKED** | EmuWiz credentials are session-only and unavailable in this session. No provider environment credentials exist; ES-DE's saved ScreenScraper username/password are empty, and the real library contains zero accepted ScreenScraper records. `availability.json` and `esde-credential-presence.json` retain presence facts only. Existing accepted-metadata/Blue integration evidence is reused from Job 36; it is not evidence of a successful live account request. |
| Local LaunchBox and bundled fallback | **PASS — prior framework validation reused** | Job 36 already covered local LaunchBox galleries, exact ownership, precedence and truthful hardware fallback. Inventory additionally finds a default Wine LaunchBox root with two platform XML files. Those real XML catalogues were not certified by this run. No separate online LaunchBox account/client integration is claimed; Core's existing exact approved image-host policy remains unchanged. |

## Cross-provider and failure checks

| Requested behaviour | Result and evidence |
| --- | --- |
| Per-field provenance, precedence and conflicts | **PASS** for supported saved evidence. Real ES-DE and RomM fields carry the correct source labels (`06`, `17`). Controlled contradictory sources and settings precedence reuse Job 36's native evidence; resolver code is unchanged. A new conflicting live-provider pair is **BLOCKED** by live-provider availability, not claimed as tested. |
| Refresh and invalidation | **PASS** for the existing cache contract. The new public-Core regression checks a matching cached small cover after reopening the cache, changed provider timestamps missing the old key, and refusal to borrow another server's image. Existing local-source refresh/invalidation evidence is reused from Job 36. A fresh live-provider refresh is **BLOCKED**. |
| Persistence | **PASS**. After normal close/restart, all four cached PNG paths and SHA-256 values are identical (`cache-before-restart.json`, `cache-after-restart.json`). `22-stale-provenance-and-cache.png` again shows a cache hit with zero network time. |
| Known stale evidence | **PASS**. Only the private copied record's existing `Stale` status was fault-injected. The retained real cover receives “Saved artwork · stale record” (`20`), and provenance explains that its game association needs refreshing (`22`). The unit regression also verifies clearing the badge when the record becomes current. The private source document was restored after normal close. Stale association is not inferred merely from an old timestamp. |
| Outages, auth errors and partial responses | **PASS** for controlled failure handling and the actual DNS outage; **BLOCKED** for successful live auth/outage recovery. The 23 affected artwork-worker checks cover DNS/backoff, refusal, timeout, 503, 401, offline cache hits, local-artwork independence and recovery. Existing unchanged Core/ScreenScraper tests cover quota/Retry-After, authentication, malformed/partial responses and response limits; normal CI executes its selected tests. Fault injection is not presented as real-provider availability. Missing RomM fields remain absent rather than invented. |
| Provider failure versus local artwork / Play | **PASS** for local artwork and native Play preflight. Real ES-DE media remains usable while RomM fails. A focused regression starts native launch readiness with a failed RomM picture. **Actual emulator gameplay was not run**: the isolated references have no verified local release evidence or configured matching emulator; Blue truthfully gates them on that independent requirement. This run does not certify emulator execution. |
| Original preservation | **PASS**. All 41 real settings/ES-DE input hashes and 12 selected original-file hashes match. Three native traces show zero write operations in protected real settings, data, artwork and game roots. Original RomM identity/cache files and the selected thumbnail were preserved. `preservation-audit.json` records the checks; the private database has integrity `ok` and three rows. Native GUI sessions exited normally. Network tracing records connection metadata only, not credential-bearing payloads. |

## Confirmed defects and repairs

1. **Offline small-cover cache fallback was unreachable.** On the real record,
   the stored key matched its small reference while the record also named a
   large cover (`native-cache-mismatch.json`). `ArtworkCache::lookup` removed
   the large reference and then tested the already-empty field, always returning
   before checking the small key. Baseline Blue consequently reported an
   unavailable picture despite a valid matching thumbnail (`02-loaded-details.png`).
   The one-line correction tests the original request before constructing the
   fallback. The cache format, keys, invalidation and server boundary are unchanged.
2. **Known stale provider associations were unlabelled on retained pictures.**
   Blue now paints a small badge and explains the existing stale association in
   artwork provenance. It retains the image and does not claim verified identity.

Failing evidence: `stale-before.log`, `cache-before.log`, the baseline native
screenshot and key comparison. Final local validation is **26 passing focused
checks**: three Job 40 regressions and 23 affected artwork-worker checks, with
two existing real-provider diagnostics intentionally ignored. The available
native real-data paths were exercised separately as described above. Formatting,
scope and GUI-root checks pass. See `final-checks.json`, `final-job40.log`,
`final-artwork.log`, `final-format.log`, `final-scope.log` and
`native-provenance.json`; 22 screenshot hashes are recorded in the manifest.

No new provider, scraping engine, bulk download, API-limit bypass, credential
storage, identity authority or shared Cargo target was introduced. Linux/X11
software rendering is the tested native environment. Blocked live-provider and
actual gameplay cases remain limitations; this is not an all-providers-online
PASS. Required CI/review and authorised integration remain separate gates.
