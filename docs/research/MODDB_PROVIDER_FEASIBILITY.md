# ModDB Provider Feasibility for EmuWiz

**Research date:** 2026-09-20  
**Scope:** discovery and user-directed acquisition only. This document does not design or implement another installer.  
**Repository impact:** research document only; no Rust or GUI code was changed.

## Executive conclusion

ModDB is feasible as a **metadata/discovery provider with explicit user-directed acquisition**, but it is not a trustworthy compatibility authority and should not be treated as one.

The safe model is:

```text
ModDB discovery/RSS/HTML
    -> user selects a candidate
    -> EmuWiz records the canonical page and release metadata
    -> user explicitly acquires the file through an ordinary permitted link
    -> EmuWiz hashes and inspects the local package
    -> existing identity-gated installer previews and applies it
```

The provider must never install from a platform tag, title similarity, archive name, uploader claim, or external-host link alone. For PS2 in particular, the category is visibly noisy: the current PS2 mods list contains content associated with PC games, ports, total conversions, texture packs, patches, and emulator material alongside genuine PS2 work. The provider can safely surface candidates, but the existing EmuWiz inspection and identity machinery must decide applicability.

This research used public pages and ordinary public HTTP access only. It did not bypass CAPTCHA, Cloudflare challenges, login controls, rate limits, or download interstitials.

## Evidence and access model

### Public interfaces found

| Interface | Finding | Safe EmuWiz use |
|---|---|---|
| Platform/category HTML | Public HTML exposes lists, pagination, titles, dates, status, genre, and links. The PS2 mods page currently reports 2,434 entries and 82 pages. | Low-rate, on-demand discovery or a user-opened page; cache metadata, do not mirror pages wholesale. |
| Mod/addon/file HTML | Public pages expose human-readable title, summary/description, game link, uploader, date, filename, category, licence, size, download count, and sometimes an MD5. | Parse only documented/public fields; retain the source URL and fetch timestamp. |
| RSS | ModDB explicitly offers RSS 2.0 feeds for releases, downloads, addons, and per-profile content, and asks consumers to provide attribution. | Preferred incremental discovery channel where a relevant feed exists. Respect feed cadence and attribution. |
| Site index | `robots.txt` advertises `https://www.moddb.com/api/siteindex`. This is a machine-readable site index, not a documented mod metadata/search API. | Use only if a future provider review confirms its purpose, format, and permitted rate. Do not infer an API contract from the path name. |
| Search | The site has a public search surface, but the current robots policy disallows `/search` for `User-agent: *`. | Do not build a crawler around search endpoints. Prefer category pages, RSS, or user-supplied canonical URLs. |
| Download start | File pages link through `/addons/start/...` or equivalent start routes and expose a fallback download and mirror link. The public robots policy disallows `/addons/start/`, `/downloads/start/`, and mirror/download helper routes. | Do not crawl start/mirror routes. A user may follow an ordinary permitted download link; otherwise ask the user to download in a browser and import the local file. |

Sources: [PS2 mods](https://www.moddb.com/platforms/ps2/mods), [ModDB RSS feeds](https://www.moddb.com/rss), [ModDB robots.txt](https://www.moddb.com/robots.txt).

### API, keys, limits, and automation constraints

No current official public ModDB API for searching mods, enumerating compatibility, or obtaining releases was found in the public documentation reviewed. No API registration flow, key requirement, quota, or documented rate-limit contract was found. The `/api/siteindex` URL in `robots.txt` is evidence of a site index endpoint, not evidence of a supported JSON API.

The strongest machine-consumable interface that ModDB expressly documents for third-party use is RSS. The RSS page says feeds may be used for up-to-date headlines and asks consumers to credit Mod DB. This makes RSS a better provider input than broad HTML crawling, but it remains a news/release feed rather than an identity or compatibility API.

The robots policy is material to the design:

* `User-agent: *` disallows query-string URLs (`/*?`), search, AJAX, download/mirror/start/timeout helpers, and several dynamic paths.
* Several named crawler identities have `Crawl-delay: 1`.
* A separate group disallows the entire site for identities including `Scrapy`, `GPTBot`, `ClaudeBot`, `CCBot`, `Diffbot`, and others.
* The file advertises `Content-signal: search=yes,ai-train=no`.

These rules do not grant EmuWiz permission to scrape. A provider should be identifiable, low-volume, cache results, avoid query-heavy enumeration, stop on `403`, `429`, a challenge page, or other bot mitigation, and provide a browser handoff instead of attempting workarounds. No stable ModDB-published request quota was found; therefore the provider must use a conservative local request budget and treat a server response as authoritative.

Terms are also restrictive enough that unattended crawling is not an acceptable default. The current [Terms of Use](https://www.moddb.com/terms-of-use) say that third-party downloadable content may be inaccurate, mislabeled, deceptive, or harmful; prohibit obtaining data except by means the service intends to make available; prohibit activity that diminishes service availability; and disclaim responsibility for third-party links and content. These are documented constraints, not a legal opinion.

## What ModDB exposes

The field quality differs significantly between a project page and an individual file page.

| Field | Observed availability | Trust level for EmuWiz |
|---|---|---|
| Mod/addon/file URL | Stable human-readable canonical paths such as `/mods/darkwatch` and `/addons/sles-53564` | Strong source locator; not an identity proof |
| Numeric/internal ID | Numeric IDs appear in image/embed URLs, but no supported public ID contract was found | Retain only as an optional provider reference; key provenance by canonical URL plus retrieval timestamp |
| Title | Usually structured in the page heading | Display metadata only |
| Summary/description | Present on project pages or file pages, often free text and sometimes multilingual | Useful evidence; never parse as verified identity without corroboration |
| Game/platform | Related game links and platform/category context are often present | Candidate evidence; platform tags are not compatibility proof |
| Release status/date | Often structured as labels such as Released, Early Access, TBD, and a date | Good display metadata; version may still be absent |
| Version | Sometimes in title/description/filename; no universal version field observed | Free-text evidence unless a release record proves it |
| Uploader/team | Structured uploader/project links are present | Attribution and provenance only |
| Tags/genre | Structured links/tags are present | Discovery hints only; tags are community-controlled and noisy |
| Images | Project pages expose image links and previews | Artwork/discovery only; not identity evidence |
| Files/downloads | Project pages list addons/files; individual file pages expose download metadata | Strong candidate-release evidence, subject to local acquisition and hashing |
| Filename | File pages expose a filename | Useful for display and archive inspection; never target selection by itself |
| Size | File pages expose human-readable and byte sizes in observed examples | Useful preflight bound; recheck downloaded bytes locally |
| MD5 | Present on the observed Gun, Darkwatch, and Punisher file pages | Strong source fingerprint if recomputed locally; not a compatibility assertion |
| Installation instructions | Often in free-text descriptions; can be detailed and operational | Inspect as untrusted instructions; allow only existing safe installers to act |
| Dependencies/variants | May be described in prose or optional-file instructions | Must be represented as unverified choices until structurally confirmed |
| Licence | File pages can show a licence such as Proprietary | Retain and display; do not infer redistribution rights |

Representative file pages:

* [Gun HD Texture Pack PCSX2](https://www.moddb.com/addons/gun-hd-texture-pack-pcsx2) exposes filename `SLUS-21139.rar`, size `1,848,585,772` bytes, MD5 `3e0f78f856975252ab5473c2986eda5e`, uploader, category, licence, and download link.
* [Darkwatch SLES-53564](https://www.moddb.com/mods/darkwatch/addons/sles-53564) exposes filename `SLES-53564.rar`, size `3,353,127,342` bytes, MD5 `a2789929816064d679306c62d53a3823`, and a detailed texture-pack description.
* [Punisher Premastered](https://www.moddb.com/addons/punisher-premastered-for-pcsx2-ps2-emulator) exposes filename, byte size, MD5, and a long description with both texture-replacement instructions and an optional modified `tables.vpp` replacement.

## PS2 category quality

The PS2 platform list must be treated as a discovery bucket, not as a verified PS2 catalogue. The current [PS2 category](https://www.moddb.com/platforms/ps2) reports hundreds of games and thousands of mods, but the visible entries include projects associated with Half-Life, Deus Ex, GTA, and other PC-oriented ecosystems. The [PS2 tag page](https://www.moddb.com/tags/ps2) likewise contains games, mods, downloads, news, ports, texture packs, and unrelated tag associations.

Representative classification from the current public entries:

| Entry | Classification | Evidence and implication |
|---|---|---|
| [Darkwatch Texture Pack](https://www.moddb.com/mods/darkwatch) / [SLES-53564 file](https://www.moddb.com/mods/darkwatch/addons/sles-53564) | Genuine PCSX2 texture-replacement candidate | Description says 6x texture upscaling; tags include `ps2`, `pcsx2`, `texture`; filename carries `SLES-53564`. Strong candidate, still requiring local package inspection and verified serial match. |
| [Gun HD Texture Pack PCSX2](https://www.moddb.com/addons/gun-hd-texture-pack-pcsx2) | Genuine PCSX2 texture-replacement candidate | Filename `SLUS-21139.rar`, PCSX2 description, texture category, and PS2/PCSX2 tags. Strong candidate for identity binding, not proof until the local archive and selected game agree. |
| [Punisher Premastered for PCSX2](https://www.moddb.com/addons/punisher-premastered-for-pcsx2-ps2-emulator) | Mixed PCSX2 texture plus game-file modification | Instructions require PCSX2 and the USA `SLUS-20864` game, but also describe replacing `tables.vpp` and optional gameplay changes. Must not be silently routed through a texture-only installer. |
| [GTA SAN ANDREAS [PS2] Mods](https://www.moddb.com/mods/gta-san-andreas-mods) | Patch/cheat/ISO-rebuild material | Description discusses `.pnach` cheats, `main.scm`, replacing files, and rebuilding an ISO. It is not a single safe texture-pack shape. Route to the appropriate existing package/patch inspection path or leave browse-only. |
| [Half-Life: PlayStation 2 25th Anniversary Patch](https://www.moddb.com/addons/half-life-playstation-2-25th-anniversary-patch) | Patch intended for a PS2 title | A PS2-labelled patch is not equivalent to a PCSX2 texture replacement. Require package inspection and a proven patch/derived-output path. |
| [Half-Life: Xenolanth](https://www.moddb.com/mods/half-life-xenolanth) as shown in the current PS2 mods list | Engine/game mod merely appearing in PS2 category | The category page can surface a current GoldSrc/PC-style project under PS2. This is direct evidence that platform listing is noisy. |

The sample is not a statistical estimate of the entire catalogue. It is enough to establish the engineering rule: the platform tag is a candidate filter only. A provider should label a result `PS2 category candidate` until structured content, package layout, and verified EmuWiz identity agree.

## Game identity mapping

### Evidence tiers

| EmuWiz result | Required evidence | Examples |
|---|---|---|
| **VERIFIED** | The downloaded package or trusted structural metadata contains a PS2 serial that exactly matches the selected verified game; for texture packs, the expected PCSX2 path/serial is also consistent. | `SLES-53564`, `SLUS-21139`, or another serial extracted from the package and equal to the verified game identity. |
| **LIKELY** | Explicit installation text names a game/version/region and the selected game has corroborating title/region/version evidence, but no package-embedded serial or equivalent structural proof exists. | Description says “Punisher USA / SLUS-20864” but the archive does not expose a verifiable identity marker. |
| **TITLE-ONLY** | A title, tag, filename, or free-text description resembles the selected game without a stable serial/version proof. | `Darkwatch` title only, or a filename such as `GTA_HD.rar` without serial evidence. |
| **UNKNOWN** | Identity is absent, contradictory, ambiguous, or points to an unsupported emulator/game path. | A pack names multiple regional versions with no selection metadata, or the archive has no inspectable target evidence. |

Only **VERIFIED** may enter an automatic install preview. **LIKELY**, **TITLE-ONLY**, and **UNKNOWN** may be displayed for research or user inspection but must fail closed for automatic application. Explicit user selection can narrow a candidate only when EmuWiz still verifies compatibility; it cannot turn a fuzzy title into verified identity.

ModDB pages generally do not expose PS2 serial, region, executable CRC, or disc-version fields as first-class compatibility metadata. Occasionally the uploader puts a serial in a filename or instructions, as the examples above do. That is useful corroboration but must be parsed and checked against the actual local package and selected verified game. No ModDB evidence reviewed exposed a reliable executable CRC field.

## Download and acquisition safety

ModDB's normal flow is page -> `Download Now` -> a start/interstitial page -> file or mirror. The observed start pages say the download should begin and offer a direct fallback plus a mirror option. This means a download URL may be a redirecting or session-sensitive artifact rather than a permanent API URL.

Safe provider behaviour:

1. Store the canonical project/file URL, not a transient redirect as the only provenance.
2. Show uploader, licence, filename, declared size, declared hash, release date, and external-host/mirror status before acquisition.
3. Permit only ordinary public redirects and content-disposition responses from a user-selected download. Stop on login, CAPTCHA, Cloudflare challenge, JavaScript challenge, unexpected HTML, or an external host requiring a separate flow.
4. Do not crawl or synthesize `/addons/start/`, `/downloads/start/`, `/downloads/mirror/`, `/external`, or robots-disallowed helper URLs.
5. Hash the resulting local file (at minimum SHA-256; compare the ModDB MD5 when present) before inspection. A changed hash is a new release or a stale/mismatched acquisition, not a reason to trust the file.
6. Treat every archive as untrusted. Use existing bounded archive inspection: reject traversal, absolute paths, symlink/hardlink/special-file escapes, excessive entries/expanded bytes, unsupported executables/scripts, and malformed archives.
7. Keep the downloaded package user-local unless the user explicitly chooses to retain it. Never mirror or rehost third-party packages as part of discovery.
8. Pass only a locally inspected package to the existing identity-gated installer. External-host content inherits no ModDB trust; provenance must record the actual final host and all redirects.

Observed file shapes include RAR texture packs measured in gigabytes, optional files, modified game data, patches, and instructions to replace files inside an ISO. A “download” is therefore not automatically a texture layer and not automatically safe to execute. EmuWiz should support multiple release files as explicit alternatives, not concatenate every file on a project page.

## Legal, attribution, and retention posture

This section records documented evidence and engineering implications, not legal advice.

The [Terms of Use](https://www.moddb.com/terms-of-use) state that uploaders retain rights in submitted content while granting DBolical a broad service/distribution licence; they also state that third-party content may be inaccurate, mislabeled, harmful, or deceptive, disclaim responsibility for third-party sites, and prohibit attempts to obtain data except by means the service intends to make available. The terms are dated 2018 and can change, so a future implementation must re-check them and obtain clarification from ModDB if it moves beyond user-directed use.

The [RSS guidance](https://www.moddb.com/rss) expressly asks consumers to provide proper attribution when using ModDB content. Therefore the provider should:

* retain and display the ModDB project/file URL;
* display the uploader/team and licence exactly as observed;
* provide a “View on ModDB” link-back;
* retain retrieval time, declared metadata, and local hash;
* store metadata/provenance by default, not a mirrored copy of the package;
* leave downloaded packages in user-local storage and never redistribute them through EmuWiz;
* treat uploader licence claims as claims, not as a guarantee that all embedded assets are redistributable.

## Minimal provider boundary (proposal only)

No provider API is implemented by this research. A future provider boundary should be deliberately narrower than an installer:

```text
ModSearchQuery
  platform/game identity hint
  title terms
  page/cursor or RSS source

ModSearchResult
  provider = ModDB
  canonical project URL
  canonical release/file URL
  provider reference (optional)
  title/summary/status/date
  related game/platform labels
  uploader/team/tags/images
  evidence tier: verified | likely | title-only | unknown

ModProviderGameEvidence
  serial/region/version/CRC candidates
  evidence source: package | page | instruction | tag
  exact extracted text and confidence
  conflict list

ModRelease
  release/file URL
  filename, declared size, declared MD5 if present
  licence, uploader, release/update date
  variant/dependency text
  acquisition mode: direct | browser handoff | external host | unavailable

ModDownloadCandidate
  source page
  final URL/host after permitted redirects
  content type/disposition
  local fingerprint
  declared-vs-observed size/hash result
  acquisition warnings

ModProviderProvenance
  provider and canonical URLs
  retrieval timestamp
  source page hash or response fingerprint where practical
  project/release references
  uploader/licence attribution
  redirect/final-host record
  local package hash and inspection result
  identity evidence and confidence
  user confirmation and existing installer receipt/journal link
```

The provider should return evidence and warnings, not a destination path or a write plan. The existing package inspectors, verified identities, preview, shared transaction machinery, receipts, and rollback remain the only components allowed to decide and apply a mod.

## Fail-closed rules

| Condition | Discovery | Acquisition/inspection | Apply |
|---|---|---|---|
| Ambiguous game/serial/region/version | Show with warning | Allow local inspection | Refuse until one verified identity remains |
| Tag/category-only match | Show as candidate | Allow user download/inspection | Refuse |
| Unsupported archive or malformed package | Show source metadata | Reject safely, no panic | Refuse |
| Executable/script payload | Show metadata | Flag/reject according to existing package policy | Never execute; refuse installer path |
| Missing download or blocked start flow | Keep canonical link | Browser handoff or user-local import | No silent retry/bypass |
| External mirror/host | Label external and untrusted | Follow only ordinary user-directed permitted acquisition | Require local hash and full provenance; no inherited trust |
| Changed download/hash | Show stale/mismatch | Reinspect as a new artifact | Refuse stale preview |
| Multiple incompatible releases | Show variants separately | Inspect selected variant only | Refuse automatic choice |
| Missing installation evidence | Show as research candidate | Inspect archive shape | Refuse if destination/applicability cannot be proven |
| Login/CAPTCHA/Cloudflare challenge/rate limit | Stop provider requests | Ask user to use browser or import local file | Never bypass |
| No licence or conflicting ownership claims | Show attribution warning | Keep local only | Require existing policy review; never imply redistribution rights |

## Novice UX sketch

The eventual flow should remain under the existing Mods & Cheats area:

```text
Mods & Cheats
  -> Find Mods
  -> choose a verified game
  -> browse compatible ModDB candidates
  -> inspect candidate and release
  -> Download / Open in browser
  -> local safety and identity check
  -> existing install preview
```

Normal users should see confidence language rather than raw hashes:

* **Made for your version** — a package-embedded serial/version matched the verified game.
* **May match your game** — the page and local evidence are consistent but not conclusive; install is unavailable until verification succeeds.
* **Can't verify this mod** — title/tag/free-text evidence is insufficient or conflicting.

The inspection screen should show the source link, uploader/licence, selected release, file hash status, and why identity was or was not verified. A browser handoff should be a first-class outcome, not an error disguised as a retry button.

## Hard blockers and recommendation

Hard blockers for a fully automatic ModDB provider are:

1. No documented public compatibility/search API or quota contract was found.
2. Robots rules disallow important dynamic/search/download helper paths and explicitly block several automation identities.
3. Category/tag quality is insufficient for identity or compatibility decisions.
4. Releases vary from texture packs to patches, ISO modifications, installers, and optional chains; the page schema does not normalize these shapes.
5. Download start/mirror flows can be session-sensitive or external, and ModDB disclaims responsibility for linked third-party sites.
6. PS2 serial/region/CRC evidence is generally free text or embedded only in filenames/instructions; strong identity is the exception, not the default.

Recommended first release:

* implement a **read-only discovery record** fed by RSS, user-opened canonical URLs, and narrowly scoped public HTML where permitted;
* require an explicit user acquisition or local-file import;
* retain metadata and attribution, not mirrored packages;
* hash and inspect locally;
* pass only `VERIFIED` packages to the existing safe installer;
* keep `LIKELY`, `TITLE-ONLY`, and `UNKNOWN` browse-only;
* request ModDB permission/documentation before any scheduled broad crawl or direct download automation.

On current evidence, ModDB is a useful candidate index and provenance source, but not a verified mod registry. This boundary gives EmuWiz discovery value without weakening its existing fail-closed installation guarantees.

## Sources consulted

* [ModDB PS2 mods category](https://www.moddb.com/platforms/ps2/mods)
* [ModDB PlayStation 2 platform](https://www.moddb.com/platforms/ps2)
* [ModDB PS2 tag index](https://www.moddb.com/tags/ps2)
* [ModDB RSS feeds](https://www.moddb.com/rss)
* [ModDB robots.txt](https://www.moddb.com/robots.txt)
* [ModDB Terms of Use](https://www.moddb.com/terms-of-use)
* [Darkwatch Texture Pack](https://www.moddb.com/mods/darkwatch)
* [Darkwatch SLES-53564 release](https://www.moddb.com/mods/darkwatch/addons/sles-53564)
* [Gun HD Texture Pack PCSX2 release](https://www.moddb.com/addons/gun-hd-texture-pack-pcsx2)
* [Punisher Premastered for PCSX2 release](https://www.moddb.com/addons/punisher-premastered-for-pcsx2-ps2-emulator)
* [GTA San Andreas PS2 Mods](https://www.moddb.com/mods/gta-san-andreas-mods)
* [Half-Life: PlayStation 2 25th Anniversary Patch](https://www.moddb.com/addons/half-life-playstation-2-25th-anniversary-patch)

