# Large DAT Streaming Audit

## Scope and starting point

Starting HEAD: `af80bf029defc7801a6bca5e23ac54e27c57e174`.

The audit followed the shared DAT path rather than the unrelated catalogue and
library subsystems:

```text
parse_dat_file
  ├─ Logiqx XML parser       (No-Intro, Redump, XML DATs)
  ├─ MAME listxml parser     (MAME <mame><machine> XML)
  └─ ClrMamePro parser       (TOSEC and legacy text DATs)
        ↓
  ParsedDat / DatIndex / managed import adapters
```

## Current-state matrix

| Input family | Parser | Input behavior | Final representation | Checksums / malformed input | Progress / cancellation |
|---|---|---|---|---|---|
| Logiqx XML / No-Intro | `quick-xml::Reader` over `BufReader` | Event streaming; bounded XML depth, identifiers, descriptions, entries and ROMs | `Vec<DatGameEntry>` and per-entry ROM vectors | CRC/MD5/SHA-1/SHA-256 normalized; malformed XML returns typed error; truncation is warned and fail-closed | No parser callback; caller is synchronous |
| Redump Logiqx XML | Same Logiqx parser | Same as No-Intro | Same | Same | Same |
| MAME `-listxml` | Dedicated `quick-xml::Reader` over `BufReader` | Event streaming; one `Machine` in flight | `Vec<DatGameEntry>` | CRC/MD5/SHA-1 normalized; malformed XML returns typed error | No parser callback; caller is synchronous |
| MAME software-list XML | Logiqx parser after root detection | Event streaming | `Vec<DatGameEntry>` | Same Logiqx validation | Same |
| ClrMamePro / TOSEC | line parser | **Now bounded line streaming**; no whole-file `read` or `lines()` collection | `Vec<DatGameEntry>` and per-entry ROM vectors | CRC/MD5/SHA-1/SHA-256 normalized; Windows-1252 fallback preserved; unterminated tail now refuses rather than returning partial data | No parser callback; caller is synchronous |
| Managed No-Intro snapshots | ZIP/member inspection plus existing DAT parser and content-addressed staging | Snapshot files are staged and validated before activation | Parsed snapshot/index remains in memory when loaded | SHA-256 source/pack/snapshot provenance preserved; failed validation is not activated | Existing lifecycle supports stage/activate separation |
| Managed MAME/Redump snapshots | Download-to-staging, streamed download hashing, existing DAT parser, atomic object/state publication | Bytes are hashed while downloaded; parser reads staged file | Parsed index/model remains in memory | Failed parse/hash leaves current snapshot intact | Existing update path is worker-callable but has no parser progress callback |

## Finding

The XML parsers were already event-based. Their unavoidable memory cost is the
typed `ParsedDat` catalogue and downstream `DatIndex`; changing their input
reader would not make a 500k-entry index small. The genuine avoidable overhead
was ClrMamePro/TOSEC:

1. `fs::read` allocated the entire file.
2. UTF-8 conversion retained another whole-file `String` in the normal case.
3. `content.lines().collect()` retained a vector of slices for every line.
4. The parser then allocated the final owned catalogue.

That path could therefore hold several representations of a large text DAT
before the required index was built.

## Implemented streaming change

`dat::parsers::clrmamepro::parse_clrmamepro` now:

- reads through a 64 KiB `BufReader`;
- consumes one bounded line at a time;
- retains only the current line, current game and current ROM state before
  appending the same owned model entries;
- preserves Windows-1252 fallback and its diagnostic;
- preserves field parsing, checksum normalization, ordering, duplicate behavior
  and fail-closed `unsupported_structure` semantics;
- refuses an oversized line through the existing typed limit error instead of
  allocating an unbounded irrelevant metadata field;
- refuses an unterminated final game/ROM block instead of returning an
  apparently complete partial catalogue.

No database or spill index was introduced. The final `Vec<DatGameEntry>` and
`DatIndex` are still required by the existing identity semantics and therefore
remain the dominant memory cost for very large catalogues.

## Stress coverage

The parser tests now generate a legal 10,000-game TOSEC-style catalogue with
two ROMs per game, long-ish names and checksum fields. They verify record count,
ordering and per-game ROM count. Existing tests continue to cover duplicate
names/hash fields, Windows-1252 text, multiple ROMs, and field non-leakage.

The malformed-tail test verifies that an interrupted final record returns a
typed parse error and cannot be mistaken for a complete import.

100k/500k generated files were not committed: the model itself intentionally
retains every entry, so larger fixtures are useful for benchmark runs but not
necessary to establish the input-layer allocation defect fixed here.

## Memory and timing interpretation

The pre-existing limits documentation records measured XML amplification of
approximately 8–9× input size for the owned model (`63 MB` peaking around
`565 MB`, `32 MB` around `251 MB`). Those figures describe the final catalogue
and are not evidence that XML parsing is DOM-based.

After this change, ClrMamePro input memory is bounded by the reader buffer,
one bounded line, current game/ROM state, warnings, and the final catalogue.
Peak memory should therefore be dominated by the same final model/index rather
than input-file-sized `Vec<u8>`, `String`, and line-slice overhead. The 10k
stress test completes in the focused parser test suite; wall time and RSS of a
full 100k/500k model remain machine-dependent and are not presented as a
portable product guarantee.

## Snapshot safety

The managed lifecycle already satisfies the required activation boundary:

- source/download bytes are hashed with SHA-256 while staged;
- parsing and validation happen before publication;
- staged candidates are separate from current/previous state;
- activation requires a valid, complete candidate and retains the prior active
  snapshot;
- failed or interrupted parsing does not replace the active pointer.

The streaming change does not alter those hashes, object paths, snapshot IDs,
or activation rules.

## GUI impact

No GUI redesign was necessary for this focused fix. Existing managed import
operations already run through background workflow surfaces and expose failure
state. A future parser progress callback could add records processed and bytes
read, but adding a fake progress indicator without a shared parser callback
would be misleading.

## Not implemented

- no SQLite/temp spill index: measurements and the current model show that the
  final identity/index representation is the dominant cost;
- no change to Logiqx/MAME model ownership semantics;
- no parser-wide cancellation API added solely for this audit;
- no change to checksum algorithms, duplicate handling, or provider identity.

