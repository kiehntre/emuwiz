# RomM browser decision

Status: explicitly deferred.

## Current architecture

RomM is an optional source. `romm_source.rs` owns connection/setup, import,
offline-cache, conflicts, stale records, artwork provenance, and read-only
linkage operations. `romm_browse.rs` provides the records, detail, conflicts,
and stale-summary projections. `sources_page.rs` still opens those projections
through `show_romm_browse_window`, so browsing remains a legacy modal surface.

## Decision

Retain the existing browser behind the current Sources route until the
provider-neutral metadata/artwork model and active GUI route ownership are
consolidated. There is no recoverable native-v2 browser implementation to
migrate, and a new one would duplicate the existing cache/model projections.

## Re-entry criteria

Build a native route only when it can consume the existing `romm_source` and
`romm_browse` view models directly, preserve offline and error states, keep
RomM optional, and demonstrate parity for records, detail, conflicts, stale
data, artwork provenance, controller navigation, mouse, and keyboard input.
Until then no production browser changes are justified and the legacy browser
must remain available.
