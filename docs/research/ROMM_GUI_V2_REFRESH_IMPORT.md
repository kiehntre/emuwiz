# GUI-v2 RomM refresh/import audit

## Legacy flow

The legacy RomM source card dispatches `RommOperation::Refresh` and
`RommOperation::FullImport` to the same worker path. The worker:

1. loads the configured URL and token file;
2. validates the source and performs the capability check;
3. uses the reviewed `RommClient` to issue authenticated `GET /api/platforms`
   and paginated `GET /api/roms?limit=&offset=` requests;
4. normalises, bounds, matches, and validates the complete response;
5. atomically publishes EmuWiz's provider-owned identity cache.

Authentication is a read-scoped client token loaded from the configured token
file and sent as an `Authorization` header. The token itself never enters GUI
state or diagnostics. Failed, malformed, partial, oversized, timed-out, or
cancelled imports preserve the previous published cache.

`Refresh` therefore means “refresh EmuWiz's RomM identity cache”. It does not
request a RomM server scan, and it does not move or mutate ROM files. Importing
metadata and importing library records are one atomic cache publication in this
path; they are not separate server-side actions. Platform database enrichment is
an optional legacy side effect, so GUI-v2 invokes the worker with no database
path and does not perform that enrichment.

The existing `RommOperation::SampleImport` is a bounded, non-publishing import.
GUI-v2 exposes it as an explicit “Preview import (25 records)” action. It is a
sample preview, not a claim that the complete catalogue has been previewed.

## API boundary and refusals

The client deliberately contains no POST, PUT, PATCH, or DELETE method. No
safe RomM scan-initiation endpoint is present or proven, so GUI-v2 does not
invent or expose “Request RomM scan”. Mapping administration, conflicts, stale
diagnostics, configuration, artwork management, and other administrative
actions remain in the legacy Sources surface.

## Native GUI-v2 safety

The native page can refresh the EmuWiz cache and display the last refresh time,
platform/game counts, and deterministic added/removed/unchanged counts based on
provider game IDs and platform slugs. A preview never publishes a cache. Both
actions preserve the browser selection and filters because the browser state is
updated in place rather than recreated. RomM remains external/provider
evidence; no native identity claim is overwritten, and no ROM file is touched.
