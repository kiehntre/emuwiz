# RetroBIOS provider feasibility research

Date: 2026-09-20<br>
Scope: research only; no BIOS/firmware blobs were downloaded or added to this checkout.<br>
Authoritative EmuWiz source inspected: `/home/davedap/emuwiz-main-release-fix`.

## Executive conclusion

RetroBIOS is useful to EmuWiz as a supplementary, read-only metadata provider. It can materially improve readiness explanations by contributing canonical filenames, aliases, per-file size and hash evidence, emulator/core mappings, region/version variants, optional-vs-required signals, and a link to a permitted acquisition route. It must not become an automatic BIOS downloader and it must not replace emulator-official requirements.

The canonical RetroBIOS source is the public GitHub repository [Abdess/retrobios](https://github.com/Abdess/retrobios). The supplied [SourceForge project](https://sourceforge.net/projects/retrobios-emulator.mirror/) identifies itself as an exact mirror and explicitly says SourceForge is not affiliated with RetroBIOS. SourceForge is therefore a discovery/mirror transport only, not the canonical authority.

The safe answer to “can EmuWiz identify required BIOS → expected checksum → local file → exact match?” is **yes, conditionally**. RetroBIOS supplies enough file-level evidence for exact local verification in many cases, but its evidence describes what an emulator/platform expects; it does not prove that the file is legally redistributable, that the platform declaration is the strongest requirement, or that a filename is sufficient when the emulator only checks presence.

Recommended trust order:

1. Emulator- or vendor-official requirement and verification behavior.
2. Existing EmuWiz local requirements and authoritative DAT evidence, including Redump/MAME-family evidence already accepted by EmuWiz.
3. RetroBIOS supplementary metadata, only when its provenance, snapshot/ref, and validation fields are retained.
4. Filename-only or unreviewed community claims: never sufficient for `Verified`.

## Sources and research method

No proprietary firmware was fetched. Research used public repository pages, source/configuration pages, documentation, release metadata, and the existing EmuWiz source tree. The source snapshot was not pinned to a commit in this document because the upstream `main` branch and release metadata are mutable; any future provider implementation must pin a tag or commit and record the metadata snapshot digest.

Primary sources:

- [RetroBIOS README](https://github.com/Abdess/retrobios/blob/main/README.md): project scope, coverage, hashes, release signatures, platform packs, and licensing boundary.
- [RetroBIOS NOTICE](https://github.com/Abdess/retrobios/blob/main/NOTICE): explicit separation of MIT tooling/metadata from third-party BIOS/firmware/keys/ROMs.
- [RetroBIOS repository tree](https://github.com/Abdess/retrobios/tree/main): `bios/`, `emulators/`, `platforms/`, `install/`, `provenance/`, `schemas/`, `scripts/`, `wiki/`, `database.json`, `allowed_signers`.
- [RetroBIOS verification modes](https://github.com/Abdess/retrobios/blob/main/wiki/verification-modes.md): existence, MD5, SHA-1, emulator-level checks, aliases, variants, and crypto-only checks.
- [RetroBIOS installer](https://github.com/Abdess/retrobios/blob/main/install.py) and [bootstrap](https://github.com/Abdess/retrobios/blob/main/install.sh): manifest URLs, ref pinning, HTTPS checks, per-file validation, and release asset behavior.
- [RetroBIOS releases](https://github.com/Abdess/retrobios/releases): versioned packs and release assets.
- [SourceForge mirror](https://sourceforge.net/projects/retrobios-emulator.mirror/): mirror statement and non-affiliation statement.

EmuWiz source inspected:

- `crates/archivefs-core/src/bios_projection.rs`: local inventory, expected filenames, optional expected SHA-256, file size, and match status.
- `crates/archivefs-core/src/dat/firmware_evidence.rs`: provider-neutral firmware records carrying size, CRC32, MD5, SHA-1, and DAT provenance; no downloads or blob embedding.
- `crates/archivefs-core/src/dat/audit.rs` and `dat/hash.rs`: exact hash matching, including SHA-256/SHA-1/MD5/CRC32 normalization.
- `crates/archivefs-core/src/launch/readiness.rs`: common `Verified`, `PresentUnverified`, `Missing`, `Unknown`, and `NotRequired` projections.
- Adapter modules for DuckStation, PCSX2, RPCS3, Flycast/Dreamcast, Hatari/TOS, PC Engine CD, Xemu, and PPSSPP.

## Authoritative upstream and mirror relationship

### Canonical source

`https://github.com/Abdess/retrobios` is authoritative for the project’s code, metadata, manifests, source references, release tags, signing policy, and license/NOTICE statements. The README says the project reads emulator source code, cross-references platform lists, and builds versioned packs. The repository currently exposes:

- platform declarations under `platforms/`;
- per-emulator profiles under `emulators/`;
- generated and exported metadata including `database.json`;
- per-platform installer manifests under `install/` and target manifests under `install/targets/`;
- source/provenance material under `provenance/`;
- BIOS paths under `bios/`;
- scripts for scraping, cross-reference, validation, database generation, pack generation, and download;
- schemas, tests, release documentation, and `allowed_signers`.

### SourceForge

The requested SourceForge URL is a SourceForge-created mirror project. Its landing page says it is an “exact mirror” of `github.com/Abdess/retrobios` and that SourceForge is not affiliated with RetroBIOS. The files page presents mirrored release directories and a “Download Latest Version” entry. That is useful as a fallback transport/browser destination, but it creates an extra mirror and redirect trust boundary. EmuWiz should display it as a mirror only and should not use it to establish canonical metadata authority.

### Releases and update mechanism

RetroBIOS uses GitHub releases for versioned packs. The README states that releases publish `SHA256SUMS.txt` and a detached signature verifiable against the repository’s `allowed_signers`. Large packs are split into numbered volumes, while the installer can fetch individual files or release assets.

The installer defaults to a GitHub raw-content base at the selected ref (`main` by default) and supports `RETROBIOS_REF` to pin a tag/ref. It fetches `install/{platform}.json` and, optionally, `install/targets/{platform}.json`. The bootstrap fetches `install.py` over HTTPS and checks its expected SHA-256 before executing it. The installer validates manifest size, path safety, total download limits, file size, and at least one of SHA-1/SHA-256 before writing atomically. Release assets are addressed through GitHub release URLs. These are good integrity controls, but a mutable `main` ref and a third-party mirror are not equivalent to a signed emulator/vendor requirement.

## Manifest and archive quality

### Per-file fields observed

RetroBIOS’s documented generated database and validation model can expose, per file:

| Field | Available? | Research interpretation |
|---|---:|---|
| Canonical path / destination | Yes | Platform and pack placement; may be a subdirectory, ZIP member, or emulator-specific path. |
| Primary filename | Yes | Useful identity hint, never sufficient by itself for content verification. |
| Alternative filenames / variants | Yes | The documentation describes aliases and `.variants/` candidates; preserve them as accepted alternatives, not as one canonical filename. |
| Exact file size | Yes where the emulator/profile declares it | Can be a hard validation, a range, or informational metadata; retain that distinction. |
| SHA-256 | Yes in the generated database/manifest model and release checksums | Strong local identity evidence; release SHA-256 also protects archive/metadata artifacts, not automatically the legal status of a BIOS blob. |
| SHA-1 | Yes | Used as the primary platform check for BizHawk and available in file metadata. |
| MD5 | Yes | Used by multiple frontend/platform profiles. EmuWiz may use it as corroboration or legacy evidence, not as the strongest hash. |
| CRC32 | Yes | Present in the generated per-file metadata and useful for MAME/arcade-style references. |
| Adler-32 | Yes | Additional corroboration in RetroBIOS; not currently required by the EmuWiz BIOS model. |
| Core/emulator mapping | Yes | Platform files name cores; emulator profiles carry per-emulator source references and checks. |
| Required/optional / fallback | Yes, with caveats | Platform manifests and emulator profiles distinguish required/optional/HLE fallback, but the semantics must not override an EmuWiz adapter’s official requirement. |
| Region/version | Often | Encoded in names, variants, system metadata, and accepted hash sets; should remain explicit rather than collapsed. |
| Legal permission | No authoritative per-file license field found | The repository’s NOTICE expressly disclaims a license for bundled third-party system software. |

### Exact-match feasibility

For a local file, an EmuWiz-style match can be:

`requirement identity + accepted filenames/aliases + expected size + accepted hash set + local observed hashes + source provenance`

That supports these outcomes:

- **Verified match**: local file hash and size exactly match an accepted requirement record.
- **Wrong version/content**: filename/path matches but no accepted hash/size matches.
- **Present unverified**: a filename/path exists, but the only upstream behavior is presence-only or no expected digest is available.
- **Ambiguous**: more than one region/core/version requirement remains valid and no target context selects one.
- **Missing**: no local candidate is present.
- **Unknown**: the source metadata is incomplete, cryptographic validation is required, or the local path cannot be inspected.

RetroBIOS itself documents an important limitation: RetroArch, Lakka, and RetroPie use existence mode, so a correctly named wrong dump still appears present to those platform checks. Its emulator-level validation can catch some such discrepancies, but that is supplementary evidence and must not be mistaken for the runtime’s own guarantee.

## Platform and emulator mapping

The repository claims broad system coverage and profiles hundreds of emulator/core configurations. The following mapping is representative and deliberately conservative.

| EmuWiz area | RetroBIOS value | Naming/semantic risk | Recommended treatment |
|---|---|---|---|
| RetroArch cores | Strongest broad coverage: `platforms/retroarch.yml` plus per-core emulator profiles; platform check is often filename/presence only | Core names, system directory layout, region variants, and aliases do not necessarily equal EmuWiz canonical platforms | Import as supplementary per-core metadata. Keep core-qualified identities and do not turn presence-only records into verified content. |
| PCSX2 | Profiles and platform entries can identify PS2 BIOS filenames, versions, regions, sizes, and hashes | A pack label may aggregate several accepted PS2 revisions; PCSX2’s actual BIOS discovery/configuration remains the local authority | Existing PCSX2 official/Redump evidence wins. RetroBIOS can add aliases and explanatory links. |
| DuckStation | PS1 filenames and hashes/variants are useful | `scph*.bin` naming is not a complete identity; region and revision matter | Map only through an explicit PS1 BIOS requirement/version record. Do not infer correctness from name alone. |
| RPCS3 | RetroBIOS includes PS3-related files in broad coverage | RPCS3 firmware is installed system software with different semantics from a console BIOS; package/install state matters | Keep RPCS3’s firmware installer/status semantics authoritative. RetroBIOS is at most a pointer to official Sony acquisition and local version metadata. |
| PPSSPP | RetroBIOS may list PSP support files in packs | Ordinary PPSSPP PSP game launches do not require a separate BIOS in the current EmuWiz model; a pack entry is not proof of a mandatory PSP BIOS | Preserve `NotRequired` for the ordinary path unless PPSSPP itself establishes a separate requirement. |
| Dolphin | Dolphin normally emulates required console behavior without a general external BIOS requirement; some optional IPL/region files may exist for specific modes | A pack’s inclusion does not mean every Dolphin title requires IPL | Keep Dolphin’s current per-mode requirement semantics. Treat optional IPL as optional metadata only. |
| Saturn | Saturn BIOS is commonly region/revision-sensitive and is consumed by multiple cores/emulators | Core filenames and accepted sets vary; “Saturn BIOS” is not one interchangeable item | Model system, core, region, revision, and hash as separate alternatives. |
| Dreamcast | Flycast/Dreamcast system files and region-specific BIOS names are useful | Dreamcast may involve BIOS plus flash/NVM/state semantics; not every file has the same content class | Keep immutable firmware distinct from writable state and use Flycast/official emulator semantics first. |
| Neo Geo | MAME/FBNeo-style BIOS/device dependencies are valuable, including set relationships and CRC32 | BIOS may be a parent/device ROM dependency inside an arcade set, not a standalone emulator BIOS path | Join only through existing MAME/FBNeo dependency models; do not flatten a device ROM into a generic console BIOS. |
| Arcade BIOS/device files | RetroBIOS covers many arcade sets and device files; CRC32 and set relationships help | A filename may be shared across sets, clones, ZIP members, or devices; “required” is game-set-specific | Preserve set/device/clone context. Existing MAME software-list and arcade DAT evidence remains stronger. |
| Amiga Kickstart | Kickstart revisions, filenames, sizes, and hashes are useful; Amiga/TOS-style versioning benefits from exact matches | Kickstart ROMs are vendor-authored system software; a repository pack license does not grant redistribution permission | Show missing/wrong-version/readiness and link to an official/licensed source where one exists; never auto-download from RetroBIOS. |
| Atari systems | TOS and Atari BIOS-like files can be mapped by system and revision | “Atari” spans systems with different firmware semantics; Atari ST TOS is not interchangeable with console firmware | Use explicit system/revision records and current Hatari/TOS semantics; do not infer from broad platform labels. |
| Nintendo systems | RetroBIOS metadata may cover Nintendo system files, keys, and files used by modern/legacy emulators | Many are proprietary, cryptographic, user-generated, or emulator-specific; a pack cannot establish permission | Default to `UserMustProvide`, `DoNotAutomate`, or `UnknownLicense` unless a separately verified official/open source route exists. |

### Naming disagreement with EmuWiz

RetroBIOS uses its own system slugs, platform names, core names, paths, and emulator profile names. EmuWiz has canonical identity and adapter-specific contracts. The mapping must therefore be an explicit table, not string normalization. Examples of likely disagreements include:

- `sony-playstation` versus EmuWiz’s PS1/PlayStation canonical identity;
- `sony-playstation-2` versus PCSX2’s BIOS identity and region/version records;
- `retroarch` as a distribution/platform versus a particular core’s actual requirement;
- `arcade`, `mame`, and `fbneo` files that are set/device dependencies rather than one BIOS item;
- `ps3` firmware package semantics versus a BIOS file;
- `amiga` Kickstart and `atari-st` TOS, which are firmware revisions with different emulator contracts;
- optional files and HLE fallbacks that should not become launch blockers in EmuWiz.

## Licensing and redistribution findings

This research does not make a legal determination. It records the evidence needed for a conservative product policy.

### Repository license boundary

RetroBIOS’s `LICENSE` applies to project-authored tooling. Its `NOTICE` says scripts, schemas, emulator profiles, platform configurations, documentation, and generated database are MIT-licensed, while files under `bios/` and release packs are third-party system software, remain the property of their manufacturers, and are not covered by MIT. The NOTICE also says the project’s personal-backup/archival/interoperability position is good-faith reasoning, not legal advice, and has not been tested in court.

Therefore:

- MIT-licensed metadata/tooling is not a license to redistribute any referenced blob.
- A checksum, a public URL, or inclusion in a GitHub/SourceForge pack does not prove redistribution permission.
- EmuWiz must not copy, cache, host, or auto-download a blob merely because RetroBIOS contains it.
- Per-file rights must be independently established before any future `Redistributable` route is enabled.

### Representative conservative classifications

| Representative item/category | Evidence-based classification | Reason |
|---|---|---|
| RetroBIOS scripts, schemas, generated metadata, profiles, docs | `Redistributable` only for the metadata/tooling, subject to MIT notices | Explicitly stated by the project license/NOTICE. This does not include referenced blobs. |
| PlayStation/PS2 BIOS dumps used by DuckStation/PCSX2 | `Copyrighted/Proprietary firmware`; route `UserMustProvide` or `OfficialVendorSource` if an official route is identified | Vendor firmware; RetroBIOS NOTICE does not grant a license. |
| RPCS3 PS3 firmware package | `OfficialVendorSource` / `UserMustProvide`; never RetroBIOS auto-download | Firmware installation semantics are vendor/emulator-specific; use RPCS3/Sony’s official route where applicable. |
| Dreamcast/Saturn Sega BIOS and flash/NVM-like files | `Copyrighted/Proprietary firmware`; `UserMustProvide` or `OfficialVendorSource` if documented | No per-file redistribution license established by RetroBIOS. Separate immutable firmware from writable state. |
| Amiga Kickstart ROMs | `Copyrighted/Proprietary firmware` unless a separately licensed source is proven | The existence of licensed commercial distributions or open replacements in the ecosystem must be checked per file; RetroBIOS itself is not proof. |
| Atari ST TOS | `Copyrighted/Proprietary firmware` or `UnknownLicense`; `UserMustProvide` unless an official/licensed route is verified | Project metadata cannot grant a TOS redistribution license. |
| Neo Geo/MAME arcade BIOS/device ROMs | `Copyrighted/Proprietary firmware` or `UnknownLicense`; `UserMustProvide` | Set/device relationship and CRC32 do not establish rights. Existing DAT provenance also does not establish redistribution permission. |
| Nintendo BIOS, keys, console system files | Usually `Copyrighted/Proprietary firmware`; `UnknownLicense` where provenance is unclear; `DoNotAutomate` | Often vendor software, keys, or user-generated material. Treat as high-risk and never infer permissibility from pack inclusion. |
| Open-source replacement firmware, if a specific file is separately identified | Potentially `Redistributable`, but only after file-specific license and source verification | “Open-source emulator” or “open-source repository” is not enough; the exact artifact and license must be recorded. |
| Dolphin/PPSSPP no-external-BIOS cases | `NoBiosRequired` / no acquisition item | Absence of a requirement is a safer result than importing a broad pack entry. |

The correct UI wording for uncertain/proprietary cases is “EmuWiz cannot provide this file” plus “Get it from the official source” when a verified vendor/emulator route exists—not a RetroBIOS download button.

## Acquisition, redirects, anti-bot, and operational constraints

Observed/documented behavior:

- GitHub provides repository raw content, release pages, release assets, and redirects for asset downloads.
- RetroBIOS’s bootstrap follows HTTPS redirects for the installer and verifies the downloaded installer SHA-256 before execution.
- The Python installer reads manifests from the GitHub raw-content ref and downloads file paths or GitHub release assets declared by the manifest.
- `RETROBIOS_REF` can pin a tag/ref, but the default `main` ref is mutable.
- Releases may contain large multi-volume ZIP assets; a browser or frontend must handle numbered volumes and extraction separately.
- SourceForge mirrors can add download-page navigation, mirror selection, redirects, and rate/availability variability. SourceForge’s own project page says it is not affiliated with RetroBIOS.
- GitHub and SourceForge can rate-limit or challenge automated clients. No CAPTCHA, authentication, or anti-bot control should be bypassed. Browser handoff is the appropriate response when direct metadata access is unavailable.

Safe EmuWiz behavior:

1. Read a pinned, signed or otherwise integrity-checked metadata snapshot when possible.
2. Use browser handoff for human navigation to GitHub releases or a verified official vendor/emulator page.
3. Never scrape around a challenge, guess a mirror, follow an untrusted redirect chain, or silently retry through an anti-bot control.
4. Never download a proprietary BIOS blob in the background.
5. If metadata cannot be fetched, retain the local requirement and show `Unknown`/`BrowserHandoff`, not `Missing` solely because the provider was unavailable.

## Comparison with existing EmuWiz readiness data

EmuWiz already has the important safety primitives:

- `BiosEvidence` records local path, filename, size, optional SHA-256, evidence source, match status, and platform.
- `BiosRequirement` records an emulator, expected filenames, optional expected SHA-256, target, content class, and projection method.
- Inventory distinguishes `VerifiedMatch`, `FilenameOnly`, `HashMismatch`, `Ambiguous`, `Missing`, and `Unknown`.
- The shared readiness layer distinguishes `Verified`, `PresentUnverified`, `Missing`, `Unknown`, and `NotRequired`.
- Redump firmware evidence can carry size, CRC32, MD5, SHA-1, and DAT version/provenance for PlayStation, PlayStation 2, and Xbox BIOS-image datasets. Xbox evidence is deliberately limited to the BIOS/flash component and does not prove MCPX or EEPROM readiness.
- Adapter-specific logic exists for DuckStation, PCSX2, RPCS3, Flycast/Dreamcast, Hatari/TOS, PC Engine CD, Xemu, and PPSSPP. PPSSPP’s normal path is explicitly `NotRequired`.
- MAME/Neo Geo dependency models already distinguish BIOS dependencies from clone/parent/device relationships.

RetroBIOS adds real information where EmuWiz is currently sparse:

- broader canonical filename and alias discovery;
- multi-core and platform mappings;
- accepted hash sets and exact sizes for more systems;
- region/version alternatives and optional/HLE fallback descriptions;
- emulator-source references explaining why a file is expected;
- cross-platform path/destination metadata;
- a useful “official source / browser handoff / cannot provide” explanation even when no blob can be downloaded.

It conflicts or risks conflict when:

- a platform manifest says “required” but an emulator officially supports HLE or no BIOS;
- RetroArch presence-only data is treated as content verification;
- a broad pack includes a file that is optional for the selected game/core;
- the same filename represents multiple regions/revisions;
- arcade/device files are flattened into a generic BIOS list;
- RetroBIOS’s collection state is treated as a legal license;
- RetroBIOS metadata overwrites a stronger emulator-official or existing EmuWiz requirement.

Existing EmuWiz requirements must remain authoritative. RetroBIOS should attach supplementary evidence and a provenance reference, never rewrite the canonical requirement in place.

## Safe provider model

The provider should be provider-neutral and item-oriented. A future implementation can conceptually expose:

```text
BiosMetadataItem {
    stable_id,
    canonical_platform,
    emulator_or_core,
    component_kind,          # bios | firmware | tos | kickstart | key | device | state
    canonical_filenames,
    accepted_aliases,
    destination_hints,
    region_variants,
    version_or_revision,
    requiredness,             # required | optional | hle_fallback | not_required | unknown
    size_bytes_or_range,
    hashes: { sha256, sha1, md5, crc32, adler32 },
    verification_mode,        # exact_hash | size | presence_only | crypto | unknown
    local_match_policy,
    metadata_source,          # emulator_official | redump | mame | retrobios | vendor
    source_ref_and_snapshot,
    acquisition_state,
    permitted_route,
    notes,
}
```

Recommended acquisition states:

| State | Meaning and allowed action |
|---|---|
| `Redistributable` | File-specific license/source evidence says EmuWiz may distribute it. Still verify transport and hash; do not infer this from MIT metadata. |
| `OfficialVendorSource` | A vendor/emulator-maintainer source is the permitted route. Browser handoff or an explicitly reviewed provider may be offered. |
| `BrowserHandoff` | Human navigation is required; EmuWiz opens or copies a verified URL and does not fetch the blob. |
| `UserMustProvide` | User must dump, install, or supply the file from their own lawful source. EmuWiz may inspect and verify locally. |
| `UnknownLicense` | Metadata exists but redistribution status is not established. No download or cache. |
| `DoNotAutomate` | High-risk/proprietary/credential/key/anti-bot or otherwise excluded item. Show a clear explanation and stop. |

These states are independent from readiness. A local file can be `Verified` while acquisition is `UserMustProvide`; a missing file can have rich verified metadata while acquisition is `DoNotAutomate`. This separation prevents the common error of treating “we know the hash” as “we may provide the bytes.”

Conceptual flow:

```text
official requirement
        + existing EmuWiz evidence
        + RetroBIOS supplementary metadata
                |
                v
       resolve canonical item / variants
                |
                v
     inspect local candidate and hash it
                |
       +--------+---------+
       |                  |
 exact match        no exact match
       |                  |
 Verified          Missing / Wrong version / Unknown
                          |
              show permitted route only
```

Suggested user-facing messages:

- “Missing BIOS — `scph5501.bin` is expected for DuckStation.”
- “Your BIOS matches — SHA-256 and size verified.”
- “Wrong version — filename matches, but no accepted hash matches.”
- “Optional BIOS — the selected core can use HLE; this file is not a launch blocker.”
- “Get this from the official source — EmuWiz will open the vendor/emulator instructions.”
- “EmuWiz cannot provide this file — metadata is available for local verification only.”

## Blockers and unresolved questions

1. **Per-file legal status is unresolved.** RetroBIOS explicitly does not grant a license to bundled system software. A future `Redistributable` classification needs independent file-level evidence.
2. **Mutable metadata unless pinned.** `main`, generated `database.json`, platform YAML, and release assets can change. A provider must pin a ref/tag and store a metadata SHA-256/signature result.
3. **Source-code claims vary in strength.** Some emulator profiles identify exact size/hash checks; others identify only filenames, accepted ranges, crypto checks, or a source location. The check type must be preserved.
4. **Crypto-only files cannot always be verified.** RetroBIOS documents signature/crypto checks that need console-specific keys. EmuWiz should surface `Unknown` or a limited verification result rather than claim exact correctness.
5. **Platform and core semantics differ.** RetroArch platform coverage is broad but often presence-only; standalone emulator requirements and EmuWiz adapter contracts must win.
6. **Archive/container semantics differ.** ZIP members, MAME device files, multi-volume packs, and writable NVM/state files cannot all be represented as ordinary immutable BIOS files.
7. **Network behavior is not a provider contract.** Redirects, rate limits, mirror availability, and anti-bot challenges can change. Browser handoff is the safe fallback.
8. **No implementation in this lane.** The provider mapping, legal review, UI wording, and trust enforcement require a separate implementation/design review.

## Recommendation

Proceed only with a metadata/readiness integration, not a RetroBIOS blob downloader. Import or inspect RetroBIOS data as a versioned supplementary provider, preserve per-emulator/core provenance and verification mode, map names through explicit canonical mappings, and compare local files using the strongest available hash/size evidence. Keep official emulator/vendor requirements and current EmuWiz requirements authoritative. For acquisition, default proprietary/unclear files to `UserMustProvide`, `OfficialVendorSource`, `BrowserHandoff`, `UnknownLicense`, or `DoNotAutomate`; reserve `Redistributable` for separately proven files.

This gives EmuWiz the valuable part of RetroBIOS—“what file is expected, how can I recognize it, and where can the user learn more?”—without turning EmuWiz into a generic BIOS piracy downloader.
