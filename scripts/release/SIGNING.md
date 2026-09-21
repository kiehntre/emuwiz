# EmuWiz release signing

The release packager can create an optional detached OpenPGP signature for the
authoritative `SHA256SUMS` file. GPG is the currently supported local provider;
the provider boundary leaves room for another detached-signature tool later.

Signing is never implicit. A maintainer must explicitly provide a private key:

```text
scripts/release/package-release.sh package \
  --gui /path/to/emuwiz --cli /path/to/emuwiz-cli \
  --sign --signing-key /secure/path/emuwiz-release-private.asc \
  --public-key-output /secure/output/emuwiz-release-public.asc \
  --archive
```

The packager writes `RELEASE.SHA256SUMS.asc` beside the release directory and
archive. It does not put the signature or private key in the reproducible
release archive. The optional public key is also written separately. Keep the
private key outside the repository, release tree, logs, and CI artifacts.

Verify integrity and publisher authenticity independently:

```text
scripts/release/verify-release.sh verify --strict RELEASE_DIR
scripts/release/verify-release.sh verify --strict --verify-signature \
  --public-key /secure/path/emuwiz-release-public.asc RELEASE_DIR
```

`SHA256SUMS` proves that the payload bytes have not changed. A valid detached
signature additionally proves that the holder of the corresponding private
key signed that checksum manifest. The verifier reports `VALID SIGNATURE`,
`INVALID SIGNATURE`, or `SIGNATURE NOT PROVIDED`; unsigned is not treated as
invalid.

Signatures are outside the archive so unsigned archive reproducibility remains
unchanged. OpenPGP signatures themselves can vary with tool/key metadata, so a
signed sidecar is not part of the reproducibility claim for the archive.

Publish the public key through a separately authenticated project channel.
Rotate keys deliberately: publish the new fingerprint before switching,
overlap verification during the transition, and retain the old public key for
historical releases. A fingerprint is recorded in `manifest.json` and
`BUILD_INFO.txt` without exposing private key material.

The RC harness remains usable for unsigned development candidates. Signing is
an explicit final packaging step; release automation should enable it only
when the maintainer has selected the key and separately published its public
counterpart.
