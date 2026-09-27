# Canonical release contract

EmuWiz's current transparent Linux release is
`emuwiz-<version>-linux-<arch>.tar.xz` plus its `.sha256` sidecar. The
extracted tree is self-consistent: `bin/emuwiz` and `bin/emuwiz-cli` run
directly, while `install.sh` performs a safe per-user install. The same tree
contains the config example, desktop template, icons, generated instructions,
licences, provenance, manifest, and checksums.

The packager and strict verifier are `scripts/release/package_release.py` and
its shell wrappers. They reject unsafe payload paths and ensure the canonical
support files are checksum-covered. AppImage and distro packages remain
optional separate lanes. Legacy ArchiveFS names are migration compatibility,
not the current release contract.
