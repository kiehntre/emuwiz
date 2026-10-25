# ScreenScraper metadata enrichment boundary

Research checked 2026-09-18 against the official ScreenScraper API v2 page,
the official member registration/usage page, and the API endpoint descriptions.

## Current facts

The public API page calls API v2 beta and warns that changes may occur without
notice. It documents `jeuRecherche.php` for title search (up to 30 results) and
`jeuInfos.php` for a game lookup. The latter accepts a `gameid` without sending
ROM information; hash/size/file-name parameters are separate optional lookup
inputs. The documented response includes game ID, ROM ID, regional names,
system ID/name, publisher, developer, players, rating, synopsis, dates,
genres, ROM hashes/size/serial/region/language, and media URLs.

The API requires developer credentials (`devid`, `devpassword`, `softname`).
User credentials (`ssid`, `sspassword`) are optional in the endpoint
documentation but may be required by the service/account state. The API
documentation says quota handling in the client is mandatory. It publishes
dynamic `ssuser` fields including requests today, negative requests today,
maximum requests per minute/day, maximum negative requests per day, and thread
limits. It documents distinct HTTP responses for authentication (403), no
match (404), service closure (401/423), blacklisted client (426), concurrency
limits (429), and daily positive/negative quota exhaustion (430/431).

The first POC uses only HTTPS, JSON GET requests, a response-size bound, zero
redirects, at most two bounded retries for temporary failures, and no whole-
library/background loop. It never uploads ROM bytes. A ROM basename, size, and
hashes may be sent only when the caller explicitly supplies them; absolute or
path-bearing names are refused.

## Terms and rights

The official API page says integration is allowed for fully free distributed
applications; other applications require prior permission and conditions from
the ScreenScraper team. EmuWiz must re-check this before any commercial or
bundled distribution model.

The official registration page asks contributors to accept Creative Commons
licensing for their contributions. Official site pages display CC BY-NC-SA
wording for ScreenScraper content and identify third-party/community sources.
That is not sufficient proof that every artwork/media item has one uniform
redistribution licence. Therefore this adapter stores media URLs/references
only and does not download, cache, mirror, or redistribute artwork.

## EmuWiz boundary

ScreenScraper is an optional metadata enrichment provider. Its returned model
has an explicit `IdentityContribution::None`; it cannot establish or upgrade
EmuWiz identity, cannot replace MAME/ScummVM/native authority, and cannot turn
a ScummVM CoverageGap into Exact. Multiple search results remain candidates.
Each field carries ScreenScraper, provider game ID, retrieval time, and match
basis provenance.

There is no ScreenScraper cache or persistent credential store in this POC.
Credentials are in-memory only and redact their debug representation. Provider
failure is returned separately from identity and does not block offline library
use.

## Safe integration options

1. **Preferred:** explicit user-triggered metadata lookup through the bounded
   client, with local provenance and media references only.
2. **Also safe:** a user-provided export imported through the existing local
   catalogue boundary, with attribution and the same identity firewall.
3. **Link-out:** open the provider record/source page without importing data.
4. **Not recommended:** undocumented frontend scraping. API v2 is explicitly
   beta, and no documented stable bulk catalogue/snapshot endpoint was found.

Metadata use is lower risk than artwork or payload redistribution, but the
license/attribution and non-commercial boundary still needs product/legal
review. Automated querying is medium risk because quota enforcement is dynamic;
bulk scraping and artwork caching are high risk and intentionally absent.

## Official references

- API v2 documentation: <https://www.screenscraper.fr/webapi2.php?alpha=0&numpage=0>
- Member/usage terms: <https://main.screenscraper.fr/membreinscription.php>
- Creative Commons reference linked by the official site:
  <https://creativecommons.org/licenses/by-nc-sa/4.0/>
