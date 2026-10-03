# Verify an EmuWiz release

> **Version note.** The packaged-release tooling described here was added after
> the `v0.9.0` git tag (2026-09-13). File names use `<version>`: replace it with
> the version in the file name you downloaded.

EmuWiz release checks answer three different questions:

- **SHA-256**: are these bytes unchanged from the published release?
- **GPG signature**: did the holder of the publisher key sign the checksum list?
- **SBOM**: which software components and declared licences are in the release?

An unsigned release is not the same as a release with a bad signature.

## Check the downloaded archive

Keep the archive and its checksum file together:

```sh
sha256sum -c emuwiz-<version>-linux-x86_64.tar.xz.sha256
```

Then extract it and check the files inside:

```sh
tar -xf emuwiz-<version>-linux-x86_64.tar.xz
(cd emuwiz-<version>-linux-x86_64 && sha256sum -c SHA256SUMS)
```

The extracted folder also contains a short `VERIFY.txt` with the same basic
steps. The checksum list covers every file in the folder except itself.

If you have an EmuWiz source checkout, the strict verifier also checks the
manifest, file types, sizes, hashes, archive paths and expected layout:

```sh
scripts/release/verify-release.sh --strict emuwiz-<version>-linux-x86_64

scripts/release/verify-release.sh --strict \
  --checksum emuwiz-<version>-linux-x86_64.tar.xz.sha256 \
  emuwiz-<version>-linux-x86_64.tar.xz
```

The verifier never runs the packaged programs. Run
`scripts/release/verify-release.sh --help` to see every option. It exits with
`0` on success and a non-zero code for each kind of failure (see
[`scripts/release/README.md`](../scripts/release/README.md)).

## Verify who published it

The signature is a separate file kept next to the release; it is not inside the
archive. For a release directory it is named
`emuwiz-<version>-linux-x86_64.SHA256SUMS.asc` and sits beside the directory.
Get the official EmuWiz public key from the release page or project
documentation, then verify:

```sh
scripts/release/verify-release.sh --strict --verify-signature \
  --public-key /secure/path/emuwiz-release-public.asc \
  emuwiz-<version>-linux-x86_64
```

Use `--signature PATH` if the signature file is somewhere else. The tool prints
one of:

- `VALID SIGNATURE`: the public key verifies the signature on `SHA256SUMS`;
- `INVALID SIGNATURE`: the signature or the signed list does not verify;
- `SIGNATURE NOT PROVIDED`: no signature was supplied.

This document does not name a release public key. Do not trust a key because it
was in some download or on a forum. Compare its fingerprint through an official,
authenticated release channel.

Maintainers sign explicitly. A key on disk is never used automatically:

```sh
scripts/release/package-release.sh \
  --target-dir /path/to/already-built-target \
  --output-root /tmp/emuwiz-dist \
  --sign --signing-key /secure/path/emuwiz-release-private.asc \
  --public-key-output /secure/output/emuwiz-release-public.asc \
  --archive
```

Keep private keys out of the repository, release folders, logs and CI
artifacts. See [`scripts/release/SIGNING.md`](../scripts/release/SIGNING.md).

## Read the SBOM and licence bundle

When a release includes a software inventory, the extracted folder has an
`SBOM/` directory:

```text
SBOM/emuwiz-sbom.cdx.json
SBOM/third-party-licenses.json
SBOM/THIRD_PARTY_LICENSES.txt
SBOM/dependency-summary.json
SBOM/SBOM_SHA256SUMS
```

`emuwiz-sbom.cdx.json` is the machine-readable CycloneDX inventory.
`THIRD_PARTY_LICENSES.txt` groups the declared licences and packages in plain
text. The bundle is tied to the exact `Cargo.lock` used to build it. Missing or
unclear licence information is reported, never guessed. To check the bundle
files:

```sh
(cd emuwiz-<version>-linux-x86_64/SBOM && sha256sum -c SBOM_SHA256SUMS)
```

Maintainers generate and verify a bundle offline (no downloads, `Cargo.lock`
unchanged):

```sh
scripts/release/generate-sbom.sh --output-dir /tmp/emuwiz-sbom
scripts/release/verify-sbom.sh /tmp/emuwiz-sbom/SBOM
```

Not every release has an SBOM; it is included only when it was generated for
that release.

## Check that two builds match (maintainers)

`scripts/compare-release-builds.sh --output-dir /tmp/emuwiz-reproduction` builds
the current clean commit twice in separate, disposable folders and reports
whether the two archives are byte-identical. See
[`scripts/release/README.md`](../scripts/release/README.md) for what this does
and does not prove.

## If verification fails

Do not run or share a package with a failed checksum or an invalid signature.
Download it again from the official release location and compare the archive
name, checksum file, signature file and public-key fingerprint. A missing
signature is a gap in proof of who published it, not proof that the files were
changed.
