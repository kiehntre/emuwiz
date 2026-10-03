# EmuWiz canonical Linux release artifact packager

This tool packages already-built binaries. It never invokes Cargo and its
verifier never executes packaged artifacts.

The canonical artifact is `emuwiz-<version>-linux-<arch>.tar.xz`. Its
extracted root is directly runnable and contains `bin/emuwiz`,
`bin/emuwiz-cli`, the user-safe `install.sh`, `config.toml.example`, the
desktop template, icons, generated docs/licences, provenance, manifest, and
checksums. `SBOM/` is optional and present only when generated.

```sh
scripts/release/package-release.sh \
  --target-dir /home/davedap/.cache/emuwiz-cargo-target \
  --output-root /home/davedap/.cache/emuwiz-release-dist \
  --sbom-dir /home/davedap/.cache/emuwiz-sbom/SBOM \
  --require-sbom \
  --archive

scripts/release/verify-release.sh --strict \
  /home/davedap/.cache/emuwiz-release-dist/emuwiz-0.9.0-linux-x86_64

scripts/release/verify-release.sh --strict \
  --checksum /home/davedap/.cache/emuwiz-release-dist/emuwiz-0.9.0-linux-x86_64.tar.xz.sha256 \
  /home/davedap/.cache/emuwiz-release-dist/emuwiz-0.9.0-linux-x86_64.tar.xz
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

## Independent release reproduction

`scripts/compare-release-builds.sh --output-dir /tmp/emuwiz-reproduction`
builds the current clean commit twice. Each run uses a separate disposable,
detached local checkout, Cargo target, and package/output directory. Only
immutable Git objects and the downloaded Cargo dependency cache are shared;
compiled outputs are not. The packager creates its own staging directory
inside each output root. No remote is contacted by the checkout operation.
Set `CARGO_NET_OFFLINE=true` to require already-cached dependencies.

Both runs use the commit timestamp as `SOURCE_DATE_EPOCH`. `build-release.sh`
remaps Rust source, generated target, Cargo, Rustup and home paths to stable
`/build/...` locations, including when the input roots contain spaces. Caller
Rust flags are retained. This is necessary for embedded panic locations and
`include!`-generated bindings even without shipped debug information. A small
real-rustc regression demonstrates different unremapped binaries and identical
remapped binaries; the complete release comparison remains the final gate.

The existing packager is unchanged: sorted PAX tar members, fixed mtimes,
zero uid/gid, empty owner/group names, 0755 directories/executables and 0644
other files, xz preset 9. Long-name PAX records are preserved. Links/special
entries are rejected by canonical package verification. Generated provenance
contains commit/tool versions and host kernel/architecture, so the guarantee
is two independent builds in the **same toolchain/host environment**, not
identity across different kernels/toolchains. Optional SBOM generation and
signing remain explicit packager capabilities. Detached signatures remain
outside the reproducible archive; the comparison does not generate keys,
download SBOM data, or change signing policy.

Success requires byte-identical `.tar.xz` archives, SHA-256 sidecars that match
the actual archives, and identical ordered member manifests. On failure the
comparison prints member paths, payload hashes (including binaries, generated
text and SBOM files), ownership/mode/timestamp/PAX/link metadata differences.
If member manifests match but compression bytes differ, `cmp` reports the
first differing archive byte. Outputs remain available for inspection; the
disposable checkouts and targets are removed on exit. Existing artifacts can
be diagnosed without rebuilding using:

```sh
scripts/compare-release-builds.sh --archives /path/a.tar.xz /path/b.tar.xz
python3 -B scripts/release/test_release_reproducibility.py
```

The historical reproducibility branch's independent-build/remapping intent
is retained, not its obsolete gzip packager or old payload layout. Release
and CI workflows now consume the current `.tar.xz` artifact; release naming
continues to use the shared release helpers.
