# Side-by-side AppImage review (R18-2)

In Blue **Emulator Setup**, the **Side-by-side AppImage review** section lists
PCSX2, PPSSPP, RPCS3, DuckStation and xemu from the existing download catalogue.
This expands review coverage without changing the PCSX2/PPSSPP bootstrap policy.

- **Review side-by-side** explicitly fetches release metadata through the existing
  curated HTTPS resolver. It downloads no executable.
- **Verify local AppImage** asks for an already downloaded file, resolves current
  release metadata again, and checks the opened regular file's bounded size,
  Linux x86_64 type 2 AppImage header, SHA-256 and identity/freshness. It never
  executes the file. A mismatch refuses review instead of accepting a filename.
- The result shows source, release, exact asset URL, published digest, proposed
  new path in the existing `emulators/<id>/installs/<install-id>` layout,
  destination-adjacent staging parent, available/required bytes and current
  binary/pointer paths. Existing receipt/manifest versions and hashes are
  informational record claims; existing binaries are not adopted or reverified.
- Destination checks reject collisions (including dangling symlinks), unsafe
  ancestors, non-private ownership/permissions, existing filesystem boundaries,
  read-only filesystems and insufficient measured space. Missing directories
  remain pending creation-time ownership/device/space revalidation.

All results are snapshots, never approval tokens. No directories, executable
bits, installations, configurations, current pointers, journals or approvals
are written. Existing ordinary download/install controls retain their existing
behavior and are a separate workflow.

Publication remains unavailable even when review inputs pass. The historical
`install_managed_appimage` publisher uses process-temp staging, switches the
current executable, and has no reviewed approval/journal/Undo integration for
new files. Wiring it would bypass preservation boundaries. A future publisher
must stage adjacent to the destination, reserve a fresh non-overwriting ID,
revalidate source/digest/destination/space under approval, preserve selections
and support journaled Undo restricted to its own newly installed files. Runtime
and FUSE compatibility are also pending; the review does not claim launch readiness.

Focused validation uses `scripts/cargo-iso` with task `bigboy-r18`:

```
test -p archivefs-core --lib managed_emulator_install::review
test -p archivefs-gui --lib emulator_download_page
```

Fixtures use private temporary roots and synthetic bytes; no real AppImage is
acquired, installed or executed. Tests cover read-only success, evidence
substitution, missing/conflicting digests, corrupt/unsupported artifacts,
symlinks/FIFOs, unsafe permissions, collisions and preservation of existing
binaries/current pointers, plus separation of review results from install state.
