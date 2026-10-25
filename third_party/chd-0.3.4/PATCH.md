# Vendored `chd` 0.3.4 (temporary security patch)

## Provenance
- Crate: `chd` 0.3.4 from crates.io, registry checksum
  `03e16e43ebc68e33e0c66f7158ca00f0a6eb2c1241556a8ceb36deaeb4299986`
  (matches the previous `Cargo.lock` entry).
- Upstream: https://github.com/SnowflakePowered/chd-rs, path `chd-rs`,
  commit `a379d8eecc6d535e1bcb73bbb9365aaf1b97ac08` (`.cargo_vcs_info.json`).
- Licence: BSD-3-Clause, Copyright (c) 2022 Ronny Chan. `LICENSE.md` is the
  upstream repository-root licence at that commit (the published crate does not
  ship a licence file).

## The change
Exactly one line, in `Cargo.toml`:

    [dependencies.ruzstd]
    -version = "0.8.0"
    +version = "0.9.1"

`src/`, `benches/` and `README.md` are byte-identical to the published crate.
`Cargo.toml.orig` is the untouched upstream manifest; the published
`Cargo.lock` is omitted (the workspace lock governs).

## Why
`ruzstd` < 0.9.1 has RUSTSEC-2026-0344: a small crafted zstd input can force a
multi-gigabyte allocation (denial of service). `chd` 0.3.4 is the latest
release and requires `ruzstd ^0.8.0`, so the fix cannot be selected without
this patch. CHD files are user-supplied, so the decoder is reachable.

## Wiring
Root `Cargo.toml`: `[patch.crates-io] chd = { path = "third_party/chd-0.3.4" }`
and `exclude = ["third_party/chd-0.3.4"]` (not a workspace member).

## Removal condition
Delete this directory, the `[patch.crates-io]` entry and the `exclude` once a
released upstream `chd` accepts `ruzstd >= 0.9.1` (check the `ruzstd`
requirement in the new release), then run `cargo update -p chd`, `cargo audit`
and the CHD regression tests. Do not carry local edits beyond the one line.

## Additional patch: CD-ROM decoder bounds (EmuWiz, separate from the ruzstd change)
`src/compression/cdrom.rs`, `CdCodec::decompress`: upstream indexed
`input[ecc_bytes + 0..2]` and sliced `input[header_bytes..][..len]` using a
length read from the hunk itself, panicking (cdrom.rs:196, :203) on a short or
damaged hunk. Hunk CRC16 is verified only after decoding, so a structurally
valid CHD with a damaged hunk reached it. The patch returns
`Error::InvalidData` when the input is shorter than the header or the declared
sector-stream length exceeds the input, and `InvalidParameter` for an output
buffer that is not whole frames. Valid input follows the identical path.
Fixtures and tests: `crates/archivefs-core/tests/chd_cdrom_decoder_bounds.rs`,
`tests/fixtures/chd_cdrom/`. Upstream-reportable; drop with this directory.
This supersedes the "Do not carry local edits beyond the one line" rule above
for this single, separately reviewed file.

## Additional patch: V5 hunk-size limit (EmuWiz, separate from the two above)
`src/header.rs`, `Header::validate`: upstream returns `true` for every V5
header without checking it ("Do not validate V5 header"), so a 124-byte header
could advertise a hunk of up to 4 GiB. `Chd::open` then sized the hunk map and
every codec buffer from it - `CdCodec::new` allocates the hunk plus two
frame-derived engines per slot, and a header may declare four slots. The patch
refuses a V5 header whose hunk size is zero or greater than
`MAX_V5_HUNK_BYTES` (1 MiB) with `Error::InvalidParameter`, in
`Header::try_read_header`, i.e. before the map is read or any codec exists.
V1-V4 headers keep upstream's existing `< 16 MiB` check. No new `Error`
variant is added (the enum mirrors libchdr's ABI).

The limit is the largest hunk MAME `chdman` 0.264 will create: `createraw -hs
4194304` fails with "Invalid hunk size (maximum 1048576)", and `-hs 1048576`
produces a valid V5 CHD (`tests/fixtures/chd_hunk/hunk_1mib.chd`). EmuWiz's
own `CHD_V5_MAX_HUNK_BYTES` was 512 KiB and wrongly described as a format
limit; it is now 1 MiB and must equal this value.
Tests: `crates/archivefs-core/tests/chd_hunk_size_limit.rs` (synthetic extreme
headers, boundaries at 512 KiB / 1 MiB / 1 MiB + 1, real chdman files).
Upstream-reportable; drop with this directory.
**NOT covered here, and still exploitable:** `Map::try_read_map` sizes its
buffers from header-controlled values with no bound against the file. A
124-byte file can request ~17 GiB (uncompressed map), ~25 GiB (geometry that
passes `optical_preservation`) or ~48 GiB (4,294,967,295 hunks, compressed
map) via `vec![0u8; hunk_count * map_entry_bytes]`, 4 GiB via the declared
compressed-map length, and `logical_bytes` near `u64::MAX` overflows in
`read_v5_header` (panic in debug builds). `logical_bytes` is attacker
controlled, so the map allocation is NOT meaningfully bounded by it. This
patch is a hunk-size-only correction: the decoder must not be described as
allocation-safe. The map issue is handled by a separate change.

## Additional patch: V5 map bounds (EmuWiz, separate from the hunk-size patch)
Closes the "NOT covered" map issue above. A 124-byte header could make
`Chd::open` request 4 GiB to ~48 GiB and abort the process, or overflow
arithmetic. Changes (`src/header.rs` `read_v5_header`, `src/map.rs`
`read_map_v5`):

* `hunk_count`/`unit_count` use `u64::div_ceil` (no `logical + hunk - 1`
  overflow). A hunk count that does not fit the format's `u32` returns
  `InvalidData` instead of being truncated (exactly 2^32 hunks used to become 0).
* Map size is `hunk_count * entry_bytes` with `checked_mul`. An uncompressed map
  must fit in the file (`map_offset + size <= file length`, checked add) or the
  open fails with `InvalidFile`. The compressed-map preamble and declared
  `map_bytes` must lie inside the file before any buffer is sized from them.
* All map buffers are allocated with `try_reserve_exact` (`OutOfMemory`, never an
  abort). The expanded compressed map is grown one entry at a time while the
  bitstream is decoded, so memory follows decoded data, not the declared count.
* Entry length > 24 bits or offset > 48 bits (reachable through
  `length_bits`/`self_bits`/`parent_bits`, previously a `byteorder` panic) and
  running-offset overflow return `DecompressionError`.
* Budget: the expanded map may not exceed `MAX_V5_MAP_BYTES` = 1 GiB
  (89,478,485 compressed-map entries). For compressed maps this refuses
  images of more than 1.7 TB at the
  19,584-byte CD hunk, 366 GB at 4 KiB, 46 GB at 512 B. Raise the constant if a
  real image is ever refused. Legacy (V1-V4) maps already grow per entry read
  from the file and are unchanged.

Not claimed: a compressed map whose Huffman stream is legitimately very
repetitive can still expand to the 1 GiB budget from a small file; that is the
format, bounded by the constant above. Hunk-size guard (1 MiB), the CdCodec
panic fix and the ruzstd requirement are untouched.
Tests: `crates/archivefs-core/tests/chd_map_allocation_bounds.rs` (malformed
headers run in `ulimit -v` child processes; truncation at every prefix).
Upstream-reportable; drop with this directory.

### Recovered integration hardening (2026-10-16)
The V5 map patch above was recovered from `dcd7b1f5`, along with its hunk-size
prerequisite and tiny real chdman fixtures. Existing ruzstd, CD-ROM decoder
bounds and EmuWiz's short-hunk-read refusal are retained.

Both compressed-input length and expanded-map length have a 1 GiB per-buffer
policy ceiling, in addition to file-range checks. Explicit Vec capacity growth
is capped and fallible; the two map buffers can coexist, so this is not a
1 GiB whole-process memory bound. Header-overlapping nonempty maps refuse.
Parent/self progression and unit mapping use checked u64 arithmetic, including
on 32-bit targets. Original format decoding and CRC verification are retained.
Very large otherwise-valid maps may be refused by policy. V1-V4 map handling
is unchanged. These local bounds do not establish general decoder safety or
full payload/hash verification; header inspection stays separate.
