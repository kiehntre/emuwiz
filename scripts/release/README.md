# EmuWiz release artifact packager

This tool packages already-built binaries. It never invokes Cargo and its
verifier never executes packaged artifacts.

```sh
scripts/release/package-release.sh \
  --target-dir /home/davedap/.cache/emuwiz-cargo-target \
  --output-root /tmp/emuwiz-dist \
  --sbom-dir /tmp/emuwiz-sbom/SBOM \
  --require-sbom \
  --archive

scripts/release/verify-release.sh --strict \
  /tmp/emuwiz-dist/emuwiz-0.9.0-linux-x86_64

scripts/release/verify-release.sh --strict \
  --checksum /tmp/emuwiz-dist/emuwiz-0.9.0-linux-x86_64.tar.xz.sha256 \
  /tmp/emuwiz-dist/emuwiz-0.9.0-linux-x86_64.tar.xz
```

Explicit `--gui` and `--cli` paths may replace `--target-dir`. An existing
AppImage can be added with `--appimage`; it is inspected and copied but never
mounted or executed. Its independently known metadata can be recorded with
`--appimage-version` and `--appimage-channel`. `--require-clean` rejects dirty
source provenance. The build profile is inferred as `release` only for
`--target-dir` discovery; explicit paths default to `unknown` unless
`--build-profile` is supplied.

For reproducible metadata and archives, export an integer
`SOURCE_DATE_EPOCH` and pass `--reproducible`. Entries, JSON keys, manifest
records, checksum records, and archive members are sorted. Archive ownership,
modes, and timestamps are normalized. Without reproducible mode, the packaging
timestamp is intentionally variable.

An output package may be replaced only with `--overwrite` and only when its
`.emuwiz-release-package.json` ownership marker is valid. Broad system and home
destinations are refused. User configuration, databases, ROMs, BIOS, saves,
journals, identity caches, and managed DAT data are outside the allow-listed
payload.

The manifest schema is version 1. `SHA256SUMS` covers every payload file except
itself. `manifest.json` cannot contain its own hash, so its hash is stored only
in `SHA256SUMS`; every other payload hash appears in both places.

`--sbom-dir` accepts only a previously verified, exact SBOM bundle. The
packager validates it offline with the SBOM verifier before copying the five
allow-listed files into `SBOM/`. `--require-sbom` makes omission an error;
without it, the historical no-SBOM package remains supported. `SBOM/SBOM_SHA256SUMS`
is retained as an independent integrity layer while the top-level
`SHA256SUMS` also covers every SBOM member.

The optional manifest `sbom` section records CycloneDX and bundle schema
versions, package count, every SBOM file's size/hash/kind, Cargo.lock identity,
product-source identity, and separate packaging/SBOM tool provenance. Known
unresolved licence metadata is preserved as a warning and is not treated as a
packaging failure.

Exit codes are stable:

- `0`: success
- `1`: invalid input
- `2`: verification failure
- `3`: unsafe output/path operation
- `4`: source/provenance failure
- `5`: packaging or host-tool failure

The dependency list is read from ELF metadata when `readelf` is available. It
is informational and does not prove that libraries exist on another machine.
The project license files are included; a complete third-party licence
inventory is not generated.

The existing smoke harness can consume the result without coupling:

1. package the release;
2. verify it with `verify-release.sh --strict`;
3. pass the packaged `bin/emuwiz-cli` path to `release-smoke.sh` according to
   that harness's CLI.
