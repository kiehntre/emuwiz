# Large DAT memory optimisation — results

Measured on `saltbox26` (Ubuntu 24.04, 24 vCPU KVM, release build, `/usr/bin/time -v`),
base `7d8a19b5`. Synthetic Logiqx DATs, one ROM per game, unique hashes/names, generated
by `crates/archivefs-core/examples/dat_memory_bench.rs` and driven by
`scripts/bench-large-dat-memory.sh` (opt-in; nothing here runs under `cargo test`; no
generated DAT is committed). "baseline" = unmodified base; "candidate" = this branch.
Wall times are single runs on a shared VM — treat as indicative; RSS is stable.

## What was wrong

- The XML parsers already stream. Memory was the retained catalogue plus the index.
- `DatIndex::build` deep-cloned each `DatRomRef` (strings, checksum vec, classification,
  metadata) into up to five buckets (CRC32/MD5/SHA-1/SHA-256/filename).
- Each parsed game's ROM `Vec` kept the first-push reservation of four `DatRomEntry`
  slots, so a one-ROM game held three unused ROM-sized slots for the life of the catalogue.

## What changed (no semantic change)

1. `DatIndex` buckets hold `SharedRomRef` (an `Arc<DatRomRef>` that derefs to the same
   `DatRomRef`, with transparent `Debug`/`Eq`). One canonical record per ROM; clones are
   refcount bumps. Lookup results, ordering, collision counts and `Debug` output are
   identical. Consumers that need an owned `DatRomRef` use `to_owned_ref()` (matched
   result sets only).
2. The Logiqx, ClrMamePro and MAME listxml parsers drop spare ROM-vector capacity when
   handing a game to the catalogue.

Not done, deliberately: compact `u32` record IDs (no ID space exists, so no overflow
path to guard — the `Arc` design needs none), string interning (not measured as the
dominant cost once records are shared), streaming persistence (see below), any change to
`replace_expected_dat_inventory` (already one transaction, one prepared statement).

## Results (peak RSS, MiB)

| records | phase | baseline RSS | candidate RSS | reduction | baseline time | candidate time |
| --- | --- | --- | --- | --- | --- | --- |
| 100k | parse | 247.5 | 147.2 | 41% | 1.05 s | 0.78 s |
| 100k | persist | 267.6 | 166.7 | 38% | 2.58 s | 2.21 s |
| 100k | full index | 963.6 | 320.6 | 67% | 5.08 s | 2.85 s |
| 500k | parse | 1227.0 | 723.2 | 41% | 5.12 s | 4.21 s |
| 500k | persist | 1341.9 | 838.4 | 38% | 9.01 s | 7.62 s |
| 500k | full index | 4781.5 | 1570.0 | 67% | 27.93 s | 14.26 s |
| 1m | parse | 2450.4 | 1444.5 | 41% | 11.88 s | 8.47 s |
| 1m | persist | 2680.2 | 1673.9 | 38% | 18.97 s | 15.46 s |
| 1m | full index | 9530.2 | 3106.4 | 67% | 80.61 s | 26.01 s |
| 2m | parse | 4899.0 | 2884.9 | 41% | 20.47 s | 16.84 s |
| 2m | persist | 5357.6 | 3343.1 | 38% | 35.87 s | 30.97 s |
| 2m | full index | not run (projected >19 GiB) | 6182.7 | n/a | n/a | 53.23 s |

"full index" = parse + `DatIndex::build` + lookup phase. Baseline 2m index was not run:
1m already needed 9.5 GiB, and doubling risked the host. Index-build phase alone, 500k:
9.36 s → 5.44 s.

Database size is unchanged (persistence code untouched): 100k 18,051,072 B; 500k
89,329,664 B; 1m 178,425,856 B; 2m 356,614,144 B (identical before/after).

## Lookup timings (ns per lookup, 100k sampled keys)

| records | sha1 b→c | md5 b→c | crc32 b→c | filename b→c |
| --- | --- | --- | --- | --- |
| 100k | 505→545 | 428→495 | 403→470 | 434→598 |
| 500k | 710→756 | 594→645 | 677→697 | 831→748 |
| 1m | 988→865 | 775→882 | 828→886 | 776→892 |

Within run-to-run noise (an extra pointer hop per hit; no regression worth chasing).

## Semantic equivalence

`dat_memory_bench golden` builds a duplicate/conflict corpus (3,000 games: shared hashes,
shared filenames with different hashes, case variants, multi-ROM games, missing SHA-1,
SHA-256 on some, clone-of links) and prints per-bucket digests of every key and bucket
`Debug` (so every field, in order), clone-of map, the public lookup API results, collision
counts, the expected-inventory projection, and the malformed-DAT error. Baseline and
candidate output are byte-identical (`golden-baseline.txt` vs candidate: `diff` empty).

Differences: NONE.

## Failure safety

Existing parser limit/malformed-input tests pass unchanged (file-size, entry-count and
malformed-attribute refusals still fail closed; the golden run records the same error
text). The index is a pure in-memory build with no partially-active state to interrupt.
Persistence is unchanged: still a single all-or-nothing transaction.

## Remaining headroom

Persistence peak is now dominated by the typed parse model (about 1.6 KiB per game);
reducing that further needs a streaming projection or a slimmer `DatGameEntry`, which
touches parser/model semantics and was out of scope here.
