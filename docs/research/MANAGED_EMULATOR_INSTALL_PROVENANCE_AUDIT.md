# Managed Emulator Install Provenance Audit

**Status:** research only; no production implementation is included.

**Repository inspected:** `/home/davedap/emuwiz-main-release-fix`

**Inspection date:** 2026-09-14

## Executive conclusion

An emulator installation is managed by EmuWiz only when EmuWiz has a durable,
atomic provenance record binding a specific installation identity to a
specific owned root, verified artifact, executable bytes, channel, platform,
and rollback lineage. Detection, writability, a familiar pathname, or a
successful `--version` probe is not ownership evidence.

The smallest safe implementation that can unlock real E4 updates is a managed
AppImage lane. It should install into an EmuWiz-owned root, verify the official
artifact before publication, write a versioned per-install manifest outside
the replaceable payload, retain one verified previous version, and launch
through a stable EmuWiz resolver. Portable archives can follow after the same
manifest and ownership rules are proven for complete trees.

Existing Flatpak and system-package installations remain externally managed.
Existing manual installations remain detected but unmanaged unless the user
explicitly adopts one through a strict, proof-based workflow.

## 1. Current EmuWiz inventory and lifecycle

The current implementation was inspected in:

- `crates/archivefs-core/src/emulator_inventory.rs`
- `crates/archivefs-core/src/emulator_download.rs`
- `crates/archivefs-core/src/emulator_update.rs`
- `crates/archivefs-gui/src/emulator_inventory_page.rs`
- `docs/research/EMUHAVEN_EMULATOR_MANAGER_AUDIT.md`

The E1 inventory already represents Dolphin, RPCS3, PCSX2, PPSSPP, DuckStation,
and xemu with executable path, installation root, version, version source and
confidence, channel, installation type, update capability, preference, save-
state risk, and warnings. Its installation types include system package,
Flatpak, AppImage, portable, manual, managed, and unknown.

The inventory is intentionally read-only. It probes a bounded set of executable
names and bounded version output; it does not establish ownership.

`emulator_download.rs` already has a managed-AppImage installation lane. It
validates an AppImage-shaped payload, can compare a supplied SHA-256, stages a
same-directory file, publishes atomically, sets executable permissions, and
writes an `install.json` marker. The current marker is a small install receipt,
not yet a complete installation lineage manifest. The bounded managed check
requires a regular non-symlink binary and a regular non-symlink marker.

E2 attaches channel-aware metadata results to a particular inventory
installation. Metadata is read-only and has bounded requests/cache behavior.

E3/E4 add staged update execution and GUI review/confirmation/rollback. The
execution path currently requires, in substance:

1. a non-running emulator;
2. AppImage, portable, or managed installation type;
3. `PortableManaged` update capability;
4. known installed version and matching current metadata;
5. a regular, non-symlink target whose hash still matches the review;
6. HTTPS artifact provenance and a published SHA-256;
7. staged download, checksum verification, atomic replacement, and a rollback
   file.

The update journal records the transaction and rollback path, but it does not
prove how the installation originally came under EmuWiz ownership. The current
real-machine result, **LIVE UPDATE BLOCKED BY INSTALL PROVENANCE**, is therefore
correct: the detected installations lack a durable proof that EmuWiz created or
explicitly adopted them, that the replacement root is exclusively owned by
EmuWiz, and that the current bytes correspond to a known artifact lineage.

Existing Doctor/profile primitives remain useful evidence, not ownership:
profile-derived executable/config paths, installation form, version probes,
and `AppearsWritable` are insufficient by themselves to authorize replacement.

## 2. Definition of managed ownership

Use explicit ownership states rather than a boolean:

| State | Meaning | Update authority |
|---|---|---|
| `MANAGED` | EmuWiz installed the version from a verified artifact into an EmuWiz-owned root and committed its manifest. | EmuWiz, subject to current verification. |
| `ADOPTED_MANAGED` | The user explicitly adopted an existing supported install after all adoption proofs passed. | EmuWiz, subject to current verification. |
| `DETECTED_UNMANAGED` | EmuWiz can inspect the install but cannot prove ownership. | None; inspect only. |
| `EXTERNAL_PACKAGE_MANAGER` | Ownership belongs to a package manager or distribution. | External manager. |
| `SYSTEM_INSTALL` | A system-managed installation, even if its exact package owner is not identified. | External/system manager. |
| `FLATPAK_MANAGED_EXTERNALLY` | Flatpak owns the application/runtime deployment. | Flatpak/remote. |
| `UNKNOWN_PROVENANCE` | Evidence is too weak or contradictory to classify. | None. |
| `BROKEN_MANAGED_STATE` | A manifest says EmuWiz owned it, but required records or paths are missing/inconsistent. | Refuse until reviewed. |
| `STALE_MANIFEST` | The owned installation changed outside the recorded envelope or its pointer/root moved. | Refuse until revalidated. |

`MANAGED` requires all of the following:

- a stable `EmulatorInstallationId` and manifest schema version;
- an install root inside an EmuWiz-owned directory, validated without unsafe
  symlink traversal;
- a relative executable path and verified executable bytes;
- version/build, channel, platform, and architecture evidence;
- official source/artifact provenance and a cryptographic artifact identity;
- an atomic completed-install record;
- an owned-path inventory sufficient to know what an update may replace;
- a retained, verified rollback lineage when a prior version exists;
- an association with the launch installation, if one is selected.

`ADOPTED_MANAGED` has the same ongoing requirements, but additionally records
the explicit adoption event and the evidence that justified it. Adoption is not
an override for missing proof.

## 3. Provenance manifest

Use one human-readable JSON manifest per installation as the authoritative
provenance record. A database may index it for fast inventory, but must not
become a second conflicting authority.

### Mandatory fields

- `manifest_schema_version`;
- `emulator_id`;
- `installation_id`;
- `ownership_state`;
- `install_root` and `executable_relative_path`, both confined to the managed
  root representation;
- `installation_type` and platform/architecture;
- installed version/build identity and channel;
- source provider and official release/artifact provenance;
- artifact filename, byte size, and SHA-256 (or a stronger verified identity);
- installed executable byte size and SHA-256;
- creation/adoption timestamp and EmuWiz version;
- verification status and last verification envelope;
- update policy and allowed channel lineage;
- current/previous install references and rollback availability;
- portable-profile association, if any;
- separate config/data roots, explicitly marked as not owned payload paths;
- an owned-path or owned-tree policy describing what replacement may touch.

### Optional fields

- upstream signature or signing-key identity, without private material;
- release API and asset identifiers;
- critical auxiliary-file fingerprints for portable trees;
- source artifact cache/reference, if retained;
- adoption evidence summary and operator confirmation;
- save-state risk classification;
- transaction IDs that created or changed the lineage.

The manifest must not contain secrets, passwords, private keys, raw emulator
credentials, or user save contents. It should use relative paths in the managed
root and absolute paths only for separately validated, user-visible external
data locations when needed.

## 4. First-install flow

The future install flow should be:

1. select an official provider, emulator, platform, channel, and exact asset;
2. fetch only bounded metadata and an HTTPS artifact;
3. verify signature where a trustworthy upstream signature exists, otherwise
   verify an official published digest;
4. stage the artifact outside the active install;
5. validate archive/path structure and extract only into a newly created,
   confined version directory for portable releases;
6. verify the staged executable and critical files against the artifact;
7. write a manifest in a separate provenance directory through a temporary
   file, flush/sync it, and atomically rename it;
8. publish a `current` pointer or installation record atomically;
9. verify the published path, bytes, version, and manifest linkage;
10. expose it to launch planning only after the completed state is durable.

An interrupted operation must leave a recognizable staging/journal state and
must never produce a managed-looking completed manifest.

Recommended local root:

```text
~/.local/share/emuwiz/emulators/<emulator-id>/
  installs/<installation-id>/<version-or-build>/   # replaceable payload
  current                                          # atomic pointer/record
~/.local/share/emuwiz/emulator-provenance/<emulator-id>/<installation-id>.json
~/.local/share/emuwiz/emulator-provenance/transactions/<transaction-id>.json
```

The exact root is configurable; the important properties are explicit local
ownership, path confinement, and a manifest that survives replacement of the
payload tree.

## 5. Existing-install adoption

Adoption should initially be limited to a regular AppImage or a self-contained
portable tree that can be copied or moved into an EmuWiz-owned root without
touching its user data. The first adoption implementation should copy into a
new owned location rather than claiming ownership of an arbitrary original
path.

Required evidence:

- explicit user action and a recorded adoption timestamp;
- supported installation type and a self-contained root;
- exact version/build and channel from a reliable probe or official release
  metadata;
- exact artifact identity: official signature or official release asset plus
  locally computed hash;
- no package-manager or Flatpak ownership;
- no unsafe symlinks, special files, or paths escaping the proposed owned root;
- writable destination and sufficient space;
- no ambiguous sibling files that an update would overwrite or destroy;
- separated configuration/save/data paths, or a plan that preserves them;
- a newly written manifest and post-adoption verification.

If the original artifact, release identity, or complete ownership boundary
cannot be proven, leave it `DETECTED_UNMANAGED`. Never offer “adopt anyway”.

## 6. Installation-type boundaries

### AppImage

AppImage is the best first managed format. A verified AppImage is a naturally
bounded executable payload, can be retained side by side, and can be replaced
by an atomic pointer switch. Its external configuration and data must still be
identified separately. Embedded AppImage update information is useful
provenance, but it is not by itself proof that EmuWiz owns the file.

The official AppImage documentation describes external and embedded update
information, while AppImageUpdate demonstrates that an AppImage can carry
update metadata and retain the old image during an update. EmuWiz should still
bind updates to its own manifest and checksum policy rather than delegating
ownership to embedded metadata. See [AppImage update information](https://docs.appimage.org/packaging-guide/optional/updates.html) and [AppImageUpdate](https://github.com/AppImageCommunity/AppImageUpdate).

### Portable archives

Portable installs are the second candidate. The whole executable tree must be
staged and verified, with a manifest outside the tree. The tree may contain
mutable caches or user files, so adoption requires an explicit owned-path
policy and preferably a separate profile/data root. Updates should replace
only a verified immutable version directory, never merge blindly into a live
tree.

### Flatpak

Flatpak owns the deployed application and runtime through an application ID,
remote, and local deployment. EmuWiz may inventory it, report its version and
available metadata, and guide the user to the external manager. It must not
replace files in the Flatpak deployment. The official documentation describes
`flatpak list`, `flatpak run`, and `flatpak update`; those commands demonstrate
the ownership boundary. See [Using Flatpak](https://docs.flatpak.org/en/latest/using-flatpak.html).

Represent it as `FLATPAK_MANAGED_EXTERNALLY`, retaining application ID, remote
if known, installation scope, and observed version as evidence—not as EmuWiz
ownership.

### System packages

APT/dpkg, DNF/RPM, pacman, and similar managers own their package database and
files. EmuWiz must not overwrite a package-owned executable even if the user
can write it. Represent these as `EXTERNAL_PACKAGE_MANAGER` or
`SYSTEM_INSTALL`, with package name/source when read-only inspection proves it.
Future integration may offer a handoff instruction, but direct replacement is
outside EmuWiz ownership.

### Manual and unknown installs

An AppImage or portable binary found in `/usr/bin`, `~/Downloads`, a ROM tree,
or a user-chosen directory is not managed merely because it is executable or
has a recognizable version. It remains `DETECTED_UNMANAGED` or
`UNKNOWN_PROVENANCE` until the adoption process proves the boundary.

## 7. Side-by-side versions and rollback

Managed versions should be side by side, with one atomically selected current
version:

```text
<emulator>/
  installs/<install-id>/2509/
  installs/<install-id>/2603/
  current -> installs/<install-id>/2603
```

The pointer is an implementation detail; launch planning must validate its
target and manifest every time it resolves it. A stable EmuWiz launch resolver
is preferable to a mutable global PATH entry. An optional wrapper may be
provided later, but it must itself be an EmuWiz-owned, verified indirection and
must not become the only provenance record.

Side-by-side storage costs space, but it makes rollback and save-state
compatibility explicit. Retain one verified previous version by default. For a
save-state-sensitive emulator/channel, retain at most two previous versions as
a bounded policy, subject to available space and explicit cleanup. Never delete
the only rollback version automatically.

Rollback should atomically select the retained previous version, verify its
manifest and executable hash, and record a journal entry. It must not require
re-downloading the old artifact. Configurations and saves remain outside the
replaceable binary tree unless the emulator explicitly proves otherwise.

## 8. Channels and lineage

Channel belongs to the installation lineage, not merely to a display string.
Stable, beta, nightly, development, canary, rolling, custom, and unknown are
distinct evidence values. The source provider/release endpoint must be able to
justify the value.

Changing channel should create a new installation lineage or an explicitly
approved channel branch. It must not silently replace a stable install with a
development binary. A channel switch should also preserve the prior pointer so
rollback remains meaningful. Do not infer channel from a folder name or from a
version token unless upstream semantics make that authoritative.

## 9. Artifact verification hierarchy

Use this evidence order:

1. verified upstream signature with a pinned/trusted key policy;
2. official published SHA-256 or equivalent digest;
3. official release asset identity plus locally computed digest;
4. a previously recorded manifest digest, only for validating the already
   installed bytes;
5. unverified download metadata, which is not sufficient for managed update
   execution.

The existing E3 checksum/staging/atomic-replacement protections must remain
binding. A URL, filename, version string, or successful process exit is not an
artifact proof.

Official provider lanes should be recorded per emulator, rather than using one
generic updater. Relevant starting points include [Dolphin downloads](https://dolphin-emu.org/download/?lang=en), [RPCS3 downloads](https://rpcs3.net/download), [PCSX2 downloads](https://pcsx2.net/downloads/), [PPSSPP downloads](https://www.ppsspp.org/download/), [DuckStation releases](https://github.com/stenzek/duckstation/releases), [xemu releases](https://github.com/xemu-project/xemu/releases), [Azahar releases](https://github.com/azahar-emu/azahar/releases), and the [Ryubing Stable releases](https://github.com/Ryubing/Stable/releases) page. These are source/provenance candidates, not permission to download or install in this research phase.

## 10. Manifest location and authority

Use a file manifest as the authoritative record:

```text
~/.local/share/emuwiz/emulator-provenance/
  <emulator-id>/
    <installation-id>.json
    history/<transaction-id>.json
```

The payload tree may contain a convenience marker, but the authoritative
manifest must not live only inside a directory that an update can replace. A
catalogue/database may cache parsed manifests and provide indexes, but every
record must be rebuildable from the manifest and the database must never
silently override it. Atomic temporary-file write plus rename, followed by
directory synchronization where supported, is required for manifest changes.

## 11. Stale, tampered, and moved installs

An installation becomes stale or untrusted when any of these occurs:

- executable hash/size differs from the manifest;
- a critical auxiliary file changes for a portable tree;
- `current` points outside the approved root or to an unknown version;
- the root is moved, replaced by a symlink, or no longer has the expected
  ownership boundary;
- the manifest is missing, malformed, schema-unsupported, or references absent
  payloads;
- package-manager/Flatpak evidence now claims ownership;
- the installed channel/version no longer matches the manifest lineage.

The result is `STALE_MANIFEST` or `BROKEN_MANAGED_STATE`; update execution is
refused. EmuWiz must show what proof failed and must not silently overwrite the
user's changed binary or reconstruct missing provenance from a pathname.

## 12. Stable launch path and multiple installations

Inventory should represent every installation as its own
`EmulatorInstallationId`, including external Flatpak, system, manual, and
EmuWiz-managed copies. The preferred launch installation is a separate choice
supported only by existing profile/launch evidence or explicit user selection.

Recommended resolution order:

1. an explicit launch profile installation ID;
2. an EmuWiz-managed `current` pointer whose manifest verifies;
3. an existing proven configured installation;
4. no preference when candidates are ambiguous.

Never merge “Dolphin Flatpak”, “Dolphin AppImage”, and “Dolphin portable” into
one fake installation. Show external ownership plainly:

- “Managed by EmuWiz.”
- “Detected, managed by Flatpak.”
- “Detected manual installation. EmuWiz will not modify it.”
- “Managed installation changed since its last verification.”

## 13. Save-state compatibility boundary

Save-state risk is separate from saved-game ownership. The existing E1 signal
that a version may be save-state-sensitive should remain informational. A
managed manifest should record emulator/core/channel/version lineage so a user
can retain an older binary for a state that may not load after an update.

Side-by-side versions are safer than in-place replacement for this reason, but
EmuWiz must not claim that retaining a binary guarantees compatibility. It must
not trigger Save Vault, snapshot saves, or alter save files as part of an
emulator update.

## 14. Adoption and user experience

The inventory UI should distinguish observation from ownership:

- **Managed by EmuWiz** — manifest and current bytes verify.
- **Detected, but not managed** — EmuWiz can inspect it but will not replace it.
- **Managed by Flatpak/system package manager** — use that manager for updates.
- **Managed installation needs review** — recorded provenance no longer
  matches the installed files.
- **Adoption unavailable** — official artifact identity or ownership boundary
  could not be proven.

Adoption should be a separate explicit workflow with a review listing source,
destination, version, channel, artifact digest, external data paths, and all
owned paths. It should never be an automatic consequence of clicking an
inventory row.

## 15. Security and TOCTOU controls

The future implementation must:

- confine every managed destination beneath the configured EmuWiz root;
- reject absolute, traversal, special-file, and unsafe symlink paths in
  artifacts and portable archives;
- use direct process arguments, never shell interpolation from release metadata;
- verify staged bytes after download and immediately before publication;
- revalidate source metadata, target pointer, manifest, version, channel, and
  running-emulator state immediately before replacement;
- use no-clobber publication for a new version and atomic pointer replacement;
- write journals before risky transitions and mark incomplete work explicitly;
- never treat directory writability as ownership permission;
- preserve external config/save paths and never recursively delete them;
- avoid logging raw credentials, tokens, or unrelated user paths where not
  needed.

## 16. Performance

The GUI should read the manifest and a bounded executable fingerprint during
normal inventory. It should not hash every file of every portable tree on every
render. Deep verification belongs to adoption, update preflight, explicit
verification, or recovery. A manifest should include enough size/mtime and
critical-file evidence to detect likely staleness cheaply, while treating those
envelopes as hints rather than cryptographic proof.

Metadata caches must be keyed by emulator, executable/build identity, channel,
provider, parser/schema version, and fetch timestamp. A cache for one release
must never prove a different installed build.

## 17. Comparable projects and transferable patterns

- **Flatpak:** application IDs, remotes, deployments, and `flatpak update`
  make package ownership explicit. Transferable lesson: respect the external
  manager boundary, not its internal files.
- **AppImage/AppImageUpdate:** self-contained payloads and embedded update
  information support portable replacement and keeping the old image. Lesson:
  AppImage is a good bounded first format, but embedded update metadata is not
  EmuWiz ownership proof.
- **EmuDeck:** its manager distinguishes Flatpaks from AppImages/binaries and
  updates them through different paths. Lesson: installation form determines
  authority; do not use one updater for all formats. See [Manage Emulators](https://manual.emudeck.com/using-app/2_manage-emulators/) and [SteamOS updating](https://emudeck.github.io/emudeck-maintenance/steamos/updating/).
- **RetroDECK:** the all-in-one Flatpak model provides a coherent externally
  managed bundle and data boundary. Lesson: a bundle can simplify ownership,
  but EmuWiz should not claim control of a Flatpak deployment.
- **Batocera:** system images and channels are managed as a platform-level
  release/update unit. Lesson: channel and release lineage must be explicit;
  this is not equivalent to replacing one arbitrary emulator executable.
- **Steam/runtime managers and asdf/mise-style version managers:** stable
  indirection plus side-by-side version directories separate selection from
  installed versions. Lesson: a current pointer and install ID are useful, but
  EmuWiz still needs cryptographic artifact provenance and owned-path rules.

These projects provide patterns, not code or authority for EmuWiz. EmuDeck's
documentation explicitly routes Flatpak updates through Discover/Flatpak while
handling AppImages and binaries in its own manager, which supports the proposed
ownership split.

## 18. Real-machine case study

The current inventory includes known installations such as Dolphin, RPCS3,
DuckStation, xemu, RetroArch, PCSX2, PPSSPP, Azahar, and Ryubing. The E4 live
result is correctly blocked because the inventory can identify executables and
versions but cannot prove an EmuWiz-created or explicitly adopted installation
with a complete artifact digest, owned root, manifest lineage, and verified
rollback state.

The safe classifications are therefore generally:

- Flatpak evidence: `FLATPAK_MANAGED_EXTERNALLY`;
- system-package evidence: `EXTERNAL_PACKAGE_MANAGER`/`SYSTEM_INSTALL`;
- PATH/manual/portable/AppImage with no EmuWiz manifest: 
  `DETECTED_UNMANAGED` or `UNKNOWN_PROVENANCE`;
- an existing EmuWiz `install.json` lane: potentially managed, but only after
  the marker, executable, root, digest, and current lineage all verify;
- contradictory or missing manifest state: `BROKEN_MANAGED_STATE` or
  `STALE_MANIFEST`.

No installation was altered, adopted, updated, hashed wholesale, or given a
new manifest during this audit.

## 19. Final decisions

1. **What makes an install managed?** A durable EmuWiz manifest plus verified
   artifact/executable identity, confined owned root, channel/version/platform
   lineage, completed atomic install record, and rollback lineage.
2. **Can an existing manual install be adopted?** Yes, but only explicitly and
   only when exact official artifact identity, self-contained ownership, and
   data separation are proven. Otherwise it stays unmanaged.
3. **Which format first?** AppImage.
4. **Side-by-side or in place?** Side-by-side version directories with one
   verified current pointer; do not overwrite the only version.
5. **Mandatory manifest fields?** Schema, emulator/install IDs, ownership,
   confined root/executable, type, platform/architecture, version/build,
   channel, source/artifact provenance, artifact and installed hashes/sizes,
   timestamps, EmuWiz version, verification state, update policy, rollback
   lineage, data/profile boundaries, and owned-path policy.
6. **Where is truth stored?** One per-install JSON file in a separate local
   provenance directory. Database records are indexes/cache only.
7. **Rollback retention?** One verified previous version by default; at most
   two for explicitly save-state-sensitive lanes, with bounded explicit cleanup.
8. **How represent Flatpak/system packages?** External ownership states with
   read-only inventory/update guidance; never direct binary replacement.
9. **What makes a managed install stale?** Hash/size or critical-file change,
   root/current-pointer movement, manifest loss/corruption, ownership conflict,
   or version/channel/lineage mismatch.
10. **Smallest next implementation slice?** A typed provenance manifest and
    atomic side-by-side AppImage install/current-pointer foundation, integrated
    with the existing download and E3 eligibility checks, with disposable
    tests for tampering, stale pointers, rollback, and external-install refusal.

## 20. Bounded roadmap

### M0 — typed provenance model

Add ownership, installation ID, manifest, artifact identity, lineage, and
verification types. Keep inventory read-only and external install states
explicit.

### M1 — managed AppImage install foundation

Use the existing verified AppImage download path. Install into an EmuWiz-owned
version directory, atomically publish a manifest/current pointer, and verify
the completed state. Preserve external configs/data.

### M2 — side-by-side history and rollback

Retain one previous verified version, journal pointer changes, verify rollback,
and expose only the existing E3 rollback authority.

### M3 — strict adoption

Adopt only exactly matching supported AppImage installs or self-contained
portable trees, copying into a new owned root where necessary. No override path.

### M4 — E4 live update enablement

Make E3 eligibility consume the manifest/ownership proof and revalidate it at
the final preflight. Enable only managed lanes that satisfy all existing
checksum, running-state, staging, and rollback protections.

### M5 — bounded cleanup and channel management

Add explicit user-controlled cleanup of owned old versions and explicit channel
branches only after ownership, rollback, and save-state interactions are
proven.

## 21. Explicitly do not build

- taking ownership of Flatpak or system-package files;
- “managed” inferred from a pathname, executable bit, or version probe;
- manifestless managed installs;
- automatic adoption or an “adopt anyway” override;
- arbitrary replacement of manual binaries;
- deletion of emulator configs, saves, or external data;
- provenance stored only in volatile GUI state;
- disabling E3 checksum, staging, stale-target, or rollback protections;
- channel switching by silently overwriting the active version;
- re-downloading an old artifact as the only rollback mechanism;
- broad whole-tree hashing on every GUI render;
- treating Save Vault as an implicit prerequisite or update side effect.

## 22. Research references

### EmuWiz

- `docs/research/EMUHAVEN_EMULATOR_MANAGER_AUDIT.md`
- `crates/archivefs-core/src/emulator_download.rs`
- `crates/archivefs-core/src/emulator_inventory.rs`
- `crates/archivefs-core/src/emulator_update.rs`
- `crates/archivefs-gui/src/emulator_inventory_page.rs`

### Official/project sources

- [Dolphin downloads](https://dolphin-emu.org/download/?lang=en)
- [RPCS3 downloads](https://rpcs3.net/download)
- [PCSX2 downloads](https://pcsx2.net/downloads/)
- [PPSSPP downloads](https://www.ppsspp.org/download/)
- [DuckStation releases](https://github.com/stenzek/duckstation/releases)
- [xemu releases](https://github.com/xemu-project/xemu/releases)
- [Azahar releases](https://github.com/azahar-emu/azahar/releases)
- [Ryubing Stable releases](https://github.com/Ryubing/Stable/releases)
- [AppImage update information](https://docs.appimage.org/packaging-guide/optional/updates.html)
- [AppImageUpdate](https://github.com/AppImageCommunity/AppImageUpdate)
- [Flatpak usage and updates](https://docs.flatpak.org/en/latest/using-flatpak.html)
- [EmuDeck emulator management](https://manual.emudeck.com/using-app/2_manage-emulators/)
- [EmuDeck SteamOS update paths](https://emudeck.github.io/emudeck-maintenance/steamos/updating/)
- [Batocera current and previous releases](https://wiki.batocera.org/current_and_previous_releases)

## Scope and validation note

This document is the only intended change for this task. No production code,
configuration, emulator installation, manifest, database, GUI, Save Vault,
BIOS Projection, arcade compatibility, or launch-planning file is changed by
the research. Validation is limited to `git diff --check` as requested.
