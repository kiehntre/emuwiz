# EmuWiz release SBOM tooling

Generate an offline CycloneDX 1.5 SBOM and deduplicated third-party licence
bundle from the exact lockfile and locally available Cargo package sources:

```sh
export SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)"
scripts/release/generate-sbom.sh \
  --output-dir target/release-sbom/SBOM

scripts/release/verify-sbom.sh target/release-sbom/SBOM
```

`--strict` fails when the dependency graph is incomplete, a registry checksum
is missing, or third-party licence metadata is missing, ambiguous, or
unavailable locally. Generation and verification never download packages.
Full offline Cargo metadata is preferred; when the local cache is incomplete,
the exact graph comes from `Cargo.lock` and workspace identity comes from
offline `cargo metadata --no-deps` or workspace manifests.

The five generated files are:

- `emuwiz-sbom.cdx.json`
- `third-party-licenses.json`
- `THIRD_PARTY_LICENSES.txt`
- `dependency-summary.json`
- `SBOM_SHA256SUMS`

The source commit, workspace version, Cargo.lock SHA-256, generator version,
and `SOURCE_DATE_EPOCH` are recorded. JSON keys, packages, relationships,
licence texts, notices, and checksums are deterministically sorted.

## Release packager integration

The SBOM branch is intentionally independent of
`feature/release-artifact-packager`. After integrating both commits, the clean
integration path is:

1. generate and verify `target/release-sbom/SBOM`;
2. pass `--sbom-dir target/release-sbom/SBOM` to the packager (and
   `--require-sbom` for a release that requires it);
3. let the packager validate that directory with the existing verifier before
   copying it as `SBOM/`;
4. include `SBOM/` in the release manifest and top-level `SHA256SUMS`; and
5. retain `SBOM/THIRD_PARTY_LICENSES.txt` byte-for-byte.
   only if a second copy is desired.

Until that small integration change lands, generate the release payload and
SBOM side by side. Do not copy an unverified or partially generated directory.
