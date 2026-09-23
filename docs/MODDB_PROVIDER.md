# ModDB provider

EmuWiz's ModDB provider is a bounded metadata and browser-handoff adapter. It
can inspect an explicitly supplied ModDB project, addon, or download page,
retain the page provenance, expose releases and file metadata, and provide a
canonical “open in browser” URL.

It does not crawl ModDB, use the site's search surface, follow download/start
or mirror routes, fetch payloads, execute installers, or install a package.
ModDB metadata is untrusted evidence. A manually acquired ZIP, RAR, patch, or
other package must still pass the existing local package inspection, identity,
preview, and durable transaction flow.

Compatibility uses the existing `Verified`, `Likely`, `TitleOnly`, `Unknown`,
and `Conflicting` vocabulary. A title, filename, platform category, or PS2 tag
cannot produce `Verified`; only an existing EmuWiz identity fact that exactly
matches provider evidence can do that. The existing checksum-backed local
package join remains authoritative after manual acquisition.

The provider accepts only HTTPS URLs on the bounded ModDB host allowlist and
rejects credentials, unsafe schemes, path traversal, unrelated hosts, unsafe
redirects, excessive response bodies, and redirect loops. It uses the existing
public-address SSRF checks, bounded `ureq` timeouts, and a descriptive
EmuWiz User-Agent. HTTP 401/403 and challenge pages become browser-required
states; 429 is rate-limited; 404 is a missing page; other failures remain
provider-unavailable rather than “no mod exists”.

Parsed metadata is retained in an EmuWiz-owned, schema-versioned JSON cache
with a 256-entry bound, atomic publication, source SHA-256 fingerprint,
retrieval timestamp, and release/file provenance. Fresh cache, stale cache, and
offline fallback are distinguishable. No page HTML is cached as an unbounded
mirror.

The provider implements the generic provider interface for metadata, release
listing, and browser handoff. `BrowseSearch` and machine acquisition are not
advertised because the researched ModDB interfaces do not provide a stable,
permitted search or trusted automatic-download contract.
