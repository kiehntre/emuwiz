# GUI-v2 RomM browser parity matrix

Audited from starting revision `b745ef8c3b5818bac491c1949897e2c247579f7a`.

| Capability | Core / provider abstraction | Legacy GUI | GUI-v2 before this feature | GUI-v2 after this feature |
|---|---|---|---|---|
| Authenticated RomM reads | Bounded read-only client; heartbeat, capability, platforms, paged ROMs | Source controller/worker uses it | Not directly browsable | Cache snapshot is loaded in a worker; provider status/failure is explicit |
| Platforms | Normalised platform id, slug/fs_slug, name, canonical mapping, count | Source/import and browse filters | Local catalogue platforms only | Native RomM platform list and selection |
| Games | Normalised records with IDs, paths, hashes, file size, verification, artwork and enrichment | Cached records browser, search, filters, paging | Local games only | Native RomM rows, search, deterministic bounded projection |
| Selected game | Full cached evidence, conflicts, files and artwork references | Native legacy detail panel | Local game detail only | Native RomM detail panel |
| Local state | Path mapping and verification/presence | Presence/stale filters | Local attention state | Present/missing filter; local evidence remains labelled |
| Metadata/enrichment | RomM enrichment is display-only and cannot write identity fields | Displayed in legacy details | Provider-neutral artwork metadata only | RomM metadata/provenance is visibly separate |
| Artwork | Instance-owned artwork references and bounded fetch/cache | Cover/screenshot controls | Existing artwork worker | Artwork availability/reference is shown; fetching stays in existing pipeline |
| Admin writes/scans | No write endpoint in the client | Existing safe workflows where supported | Legacy handoff | Still retained outside the browsing route |
| Offline/malformed/auth failure | Typed errors preserve prior cache | Legacy failure/offline messaging | No RomM browser surface | Unavailable/stale cache state is explicit and local browsing remains usable |

The native route deliberately consumes the published identity cache. Refresh/import,
mapping administration, conflict repair, stale diagnostics, and other unsupported
operations remain on their existing legacy source surface.
