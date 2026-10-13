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
