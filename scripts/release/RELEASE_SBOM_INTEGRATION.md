# Release packager and SBOM integration

The release pipeline keeps SBOM production and packaging as explicit stages:

1. Generate the offline SBOM with `generate-sbom.sh`.
2. Verify it with `verify-sbom.sh`.
3. Package the verified directory with `package-release.sh --sbom-dir`.
4. Verify the release directory and its independent checksums.
5. Create and verify the archive, then verify the extracted release.

Example:

```sh
SOURCE_DATE_EPOCH=1700000000 \
  scripts/release/generate-sbom.sh generate \
    --source-root "$PWD" --output-dir /tmp/emuwiz-sbom/SBOM
scripts/release/verify-sbom.sh --source-root "$PWD" /tmp/emuwiz-sbom/SBOM
scripts/release/package-release.sh \
  --target-dir /path/to/target \
  --source-root "$PWD" \
  --output-root /tmp/emuwiz-release \
  --sbom-dir /tmp/emuwiz-sbom/SBOM \
  --require-sbom --archive --reproducible
scripts/release/verify-release.sh --strict \
  --source-root "$PWD" /tmp/emuwiz-release/emuwiz-<version>-linux-x86_64
```

The input bundle is allow-listed and reverified offline. It must contain only
the four SBOM payload files and `SBOM_SHA256SUMS`; members must be regular,
non-executable files with no symlink substitution. The packager checks the
CycloneDX 1.5 schema, bundle schema, Cargo.lock SHA, product source SHA, and
SBOM generator provenance before any package payload is created.

The release manifest records separate `product_source_sha`,
`packaging_tool_sha`, and `sbom_tool_sha` values. The inner checksum file
covers only the SBOM payload, while the release `SHA256SUMS` covers binaries,
documentation, provenance, and every SBOM member. Verification never executes
packaged binaries and never regenerates an SBOM implicitly.

Known unresolved licence metadata remains represented in the generated SBOM
and `BUILD_INFO.txt`; normal packaging does not reject those documented
warnings. `THIRD_PARTY_LICENSES.txt` is copied byte-for-byte.
