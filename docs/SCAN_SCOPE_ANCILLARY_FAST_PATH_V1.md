# Scan Scope and Ancillary-Media Fast Path V1

EmuWiz keeps three explicit scan scopes:

* **Normal** (recommended): every new, changed, stale, unknown, ambiguous,
  conflicting, game-like, or dependency-related item is deeply analysed. An
  unchanged ancillary item may reuse its stored classification only when the
  classification producer is current and no relationship requires deeper work.
* **Full**: re-analyses every item, including artwork and supporting media. It
  never resets human truth, metadata, or transaction history.
* **Targeted**: re-analyses a selected item/category (for example stale,
  changed, unknown, ambiguous, or problems) and bypasses reuse for the selected
  scope.

The policy is implemented as a pure decision boundary in
`platform_evidence_fusion::scan_scope`. It records files checked, deeply
analysed, and classifications reused; reuse does not remove a file from totals,
provenance, relationships, or later queries.

## Eligibility

Reuse requires all of:

1. a previous positive ancillary role (`Artwork`, `Manual`, `Readme`,
   `Metadata`, or `SaveOrState`);
2. matching file freshness;
3. a current classification/evidence producer;
4. no dependency/set requirement;
5. no conflict or ambiguity;
6. Normal mode and no targeted request.

Unknown files, unidentified game content, primary media, BIOS/firmware, CUE/M3U,
archives, tape/audio candidates, and disk/optical images are never eligible by
this policy. A Spectrum tape WAV remains game-content work; a CUE remains set
support work.

First scans have no prior positive classification and therefore cannot reuse the
fast path. If a parser or optional tool changes, producer freshness must mark the
stored role stale; the next Normal scan analyses it once. A moved file can only
reuse evidence through the existing identity/freshness rules, never by pathname
alone.

## Reporting and safety

Normal scans should report both `files_checked` and the split between deep work
and reused classifications. “Reuse current classification” means a cheap
freshness check, not exclusion or deletion. Full remains available as the user
authority to re-check everything.

This V1 supplies the shared policy and testable counters. Wiring it into the
persisted scanner/UI must consume the existing classification and evidence
version seams; it must not create a competing freshness architecture or alter
game/set semantics.
