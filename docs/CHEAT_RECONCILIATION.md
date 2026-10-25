# Cross-source cheat reconciliation

The core neutral cheat-IR exposes a pure `reconcile_cheats_for_game` service.
It accepts entries already tied to a verified, platform-specific game identity
and returns groups for review. It never writes emulator files, installs or
deletes cheats, merges sources, or chooses a winner.

## Identity gate

Every entry must carry the same non-empty verified identity. The identity is
expected to be the platform's authoritative key (for example a Dolphin Game ID
and revision, a PS2 serial/CRC, or a verified DS identity), not a display title.
Unverified, mixed-game, or mixed-platform input returns `Unavailable` and is not
reconciled.

## Relationships

- `ExactSemanticDuplicate`: fully understood IR operations match in order.
- `ExactRawDuplicate`: opaque entries have the same conservatively normalized
  raw code; this is never promoted to semantic equivalence.
- `SameTitleDifferentCode`: normalized titles match but comparable code differs.
- `RelatedUnproven`: titles group while at least one entry lacks enough code
  evidence to compare safely.
- `Unique`: no safe relationship was established.

Semantic fingerprints include platform, operation kind, address, width, value,
operation order, and execution policy. Consequently a normal `Write32` is not
equal to `OnFrameWrite32`. Source formatting and provider naming do not affect
semantic identity. Unsupported operations and encrypted/proprietary forms are
never decoded or fingerprinted semantically.

Raw fingerprints normalize only line endings and surrounding whitespace. Line
order, control tokens, and code case are preserved unless a parser has already
proven that case is irrelevant. Reordered opaque lines therefore remain
distinct.

Titles are normalized only for conservative grouping: surrounding whitespace,
repeated internal whitespace, and ASCII case are ignored. Similar-sounding
titles are not fuzzy-merged. All source/provider/provenance records remain in
the returned entries and group indexes.

The result includes evidence-quality signals, operation differences for title
conflicts, and `auto_winner: None` by construction. A future GUI can render
duplicate groups and conflicts without understanding individual formats, while
existing native apply/install paths remain unchanged.

## CLI report

The read-only CLI exposes the same service without adding another reconciliation
engine:

```text
emuwiz-cli cheats reconcile entries.json
emuwiz-cli cheats reconcile --input entries.json --relationship conflicts --json
```

`entries.json` is a JSON array of `CheatReconciliationEntry` values produced by
an existing source-ingestion path. The core identity gate requires every entry
to carry the same verified game identity and platform; titles are never used to
resolve a game. Human output summarizes duplicate, conflict, related, and unique
groups and always states that there is no automatic winner. `--json` emits the
serialized result, retaining group relationships, differences, quality, source,
and provenance. Filters are presentation-only. The command never edits source
data, provider caches, emulator files, or the input file; a conflict is a
successful report, not a failure exit status.
