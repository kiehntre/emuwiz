# PS Multi Tools audit for EmuWiz

Research-only audit. No production code, Save Vault code, Publisher Profiles, GUI, converters, downloaders, or tests were changed.

## Scope and revisions

The PS Multi Tools repository inspected was:

- Repository: [SvenGDK/PS-Multi-Tools](https://github.com/SvenGDK/PS-Multi-Tools)
- Commit: `8aa0022254240b7b8c8b1867dc7b5b85e8a1201b`
- Date inspected: 2026-09-14
- EmuWiz starting SHA: `4c71ffe7a9bc282bcb86e13367fa45f9a516479a`

The audit verified source rather than relying on the README alone. PS Multi Tools generally launches a bundled executable, captures stdout/stderr, and moves the expected output. It rarely validates an input format itself, checks an exit status beyond the expected output, hashes the result, or performs a round trip.

## Source paths and functions inspected

### PS Multi Tools

| Area | Source path and functions | Finding |
|---|---|---|
| PSP CSO | `PSMultiTools/PSP/Tools/CISOConverter.axaml.cs`: ISO/CSO click handlers | Launches `Tools/mciso(.exe)`. ISO→CSO passes the selected level and paths; CSO→ISO passes level `0`. Captures output, but has no magic/version validation, hash comparison, or post-output structural verification. |
| CSO levels | `PSMultiTools/PSP/Tools/CISOConverter.axaml`: `CompressionLevelComboBox` | Levels 1–9 are UI choices; default selected index 6 is level 7. The UI does not document algorithm or CSO version. |
| BIN/CUE conversion | `PSMultiTools/PS2/Tools/BINCUEConverter.axaml.cs`: conversion handlers | Calls bundled `bchunk`; PSX mode adds `-p`. It expects an ISO output and moves it, but does not preserve a complete multi-track topology or verify a canonical fingerprint. |
| BIN merging | `PSMultiTools/PS1/Tools/MergeBinTool.axaml.cs`: merge handlers; bundled `Tools/macOS/binmerge` | Calls `binmerge`. The bundled Python source parses CUE tracks, INDEX positions, block modes, and binary tracks, and emits merged/split CUE layouts. It is a layout transformation, not byte-identical reproduction of the original files. |
| PS2 memory cards | `PSMultiTools/MemoryCard/PS2MCManager.axaml.cs`: `LoadPS2MC`, `LoadPS2MCDirectory`, extraction/add/delete/format handlers | Calls `ps3mca-tool` for card info, free space, listing, extract, inject, remove, and format. Output is parsed by fixed line positions and pipe-delimited text. There is no in-process offline `.ps2` filesystem parser or game-title recognizer. |
| PBP | `PSMultiTools/PSP/Tools/PBPPacker.axaml.cs`: pack/unpack handlers | Calls `zPBPTool`; extracts PARAM.SFO and assets and repacks them. Only existence/output checks are visible. |
| PBP↔ISO | `PSMultiTools/PSP/Tools/PBPISOConverter.axaml.cs` | Calls `IsoPbpConverter.exe`; no explicit round-trip or byte verification. |
| SFO | `PSMultiTools/Classes/SFONew.cs`: `ReadSfo(Stream)` | Reads the common `\0PSF` format and exposes a dictionary. The parser is not bounded as defensively as EmuWiz's parser. |
| PS3 identity | `PSMultiTools/PS3/Tools/PS3ISOTools.axaml.cs` | Opens `PS3_GAME/PARAM.SFO` through `DiscUtils.Iso9660.CDReader`, reads `TITLE_ID`, and uses it to look up a key. |
| PS5 metadata | `PSMultiTools/PS5/Tools/Editors/PS5ManifestEditor.axaml.cs`: `LoadManifestParamFile`; `PS5PKGViewer.axaml.cs` | Deserializes JSON and displays fields including title ID, application name/version, content ID/version, required system software version, and repository URL. |
| Patch discovery | `PSMultiTools/PS5/Tools/GamePatches/PS5GamePatches.axaml.cs`, `PS5GamePatchSelector.axaml.cs`; `Classes/GamePatchesDownloadHandler.cs`, `GamePatchesRequestHandler.cs` | Uses `prosperopatches.com/<GameID>` in an embedded browser and permits `.pkg` downloads. Game IDs can be inferred from filenames. No cryptographic hash/signature or trustworthy provenance record is created; TLS certificate errors are explicitly bypassed. |
| External-tool catalogue | repository `README.md`, external `Tools/` tree | Names bchunk, binmerge, DiscUtils, IsoPbpConverter, maxcso, mCiso, ps3mca-tool, sfo, zPBPTool, and other utilities. The README is attribution, not a complete redistribution/licence audit. |

### EmuWiz

The comparison covered `crates/archivefs-core/src/disc_evidence_collector.rs`, `chd_identity.rs`, `repair/optical_conversion.rs`, `param_sfo.rs`, `psp_pbp_evidence.rs`, `ps3_disc_evidence.rs`, `ps4_layout_evidence.rs`, `playstation_boot_evidence.rs`, `ps2_boot_evidence.rs`, `gamecube_wii_boot_evidence.rs`, `raw_cd_sector.rs`, `ingestion/cue_bin.rs`, `ingestion/gdi.rs`, and `patch_manager/pcsx2_local.rs`.

EmuWiz already has bounded SFO and PBP parsing, PS1/PS2/PSP/PS3 evidence paths, Xbox and GameCube/Wii structural evidence, CHD media-class inspection, GD-ROM-aware optical handling, and a deliberately narrow verified CUE/BIN→CHD transaction. Its closest current memory-card model is `Pcsx2MemcardKind::Shared` in `patch_manager/pcsx2_local.rs`: it reports presence and does not claim a shared card belongs to one game. No literal `SHARED_MEMORY_CONTAINER` symbol was found in the inspected tree.

## A. PSP CSO compression

PS Multi Tools does not implement CSO compression itself. `CISOConverter` delegates to bundled `mciso`. The source proves a level selector from 1 to 9 and the two command shapes, but not the bundled binary's version, supported CSO revisions, algorithm, block size, or compatibility policy. The separate `maxcso` entry in the README must not be conflated with the `mciso` binary actually invoked by this UI.

CSO sector compression is lossless when the compressor and decoder correctly implement the format: decompression should reproduce the ISO byte stream, including padding. A newly compressed CSO is not byte-identical to an ISO, but a valid CSO→ISO reconstruction can be byte-identical to the source ISO. PS Multi Tools does not establish that property because it performs no hash or round-trip check.

For future EmuWiz work, a CSO adapter is reasonable, but the useful design is: validate the header/version and size table, bound decompression, select a documented compatibility profile, write to a new path, and compare source/output hashes for a decompression or round-trip operation. `maxcso` documents zlib, 7-Zip deflate, Zopfli, experimental CSO v2/ZSO and LZ4 options; larger blocks can be incompatible with old PSP software. This is evidence for evaluating maxcso, not evidence about mCiso.

Current EmuWiz does not expose a PSP CSO converter; its PPSSPP launch slice deliberately accepts only a direct ISO. This is a future capability gap, not a defect in current identity detection.

## B. PS1 and PS2 conversion

`bchunk` is a sector/data-track extractor. The `-p` PSX mode is useful for PlayStation sector conventions, but the resulting ISO is not a general preservation of the source CUE/BIN set.

| Operation | Classification | Reason |
|---|---|---|
| BIN/CUE with one data track → ISO | Content-equivalent, sometimes byte-identical in the data stream | Only if the selected track, sector mode, pregap policy, and output geometry match. PS Multi Tools does not prove this. |
| PS1 mixed-mode or audio-track CUE → ISO | Topology-losing / unsafe as a preservation conversion | Audio tracks, pregaps, indexes, and track boundaries are not represented by a single ISO. |
| PS2 BIN/CUE → ISO | Unsafe as a generic PS2 conversion | A shared bchunk path is not proof of PS2 DVD topology support; a CD-oriented extractor must not be advertised as a lossless PS2 converter. |
| BIN merge with emitted CUE | Content-equivalent and generally reversible at track-payload level | `binmerge` preserves binary track order, block modes, indexes, and timestamps in a new CUE, but changes file layout and cannot restore discarded source-file boundaries or malformed source intent. |
| ISO/PBP conversion | Content/container-equivalent only | The PBP wrapper and metadata layout differ; no source hash or byte-identical round trip is shown. |

EmuWiz's topology-first CUE/BIN→CHD path accepts only a supported single MODE1/2048 track and independently compares canonical optical fingerprints after `chdman`. That is materially safer than offering PS Multi Tools' bchunk route as a generic identity-preserving conversion. No change is justified.

## C. PS2 memory cards and Save Vault

PS Multi Tools provides useful operational coverage through `ps3mca-tool`: card title/info, page and block sizes, capacity, ECC/bad-block text, free-space reporting, directory navigation, file listing, extraction, injection, removal, and formatting. The application itself does not parse the PS2 memory-card filesystem. It assumes positional output from the external tool and exposes write operations.

It does not identify which game each save belongs to from save contents, provide a per-save cryptographic identity, or perform a complete corruption audit. Therefore its implementation is not sufficient evidence for adopting its parser or its game-identification claims.

EmuWiz can safely inspect a shared PS2 memory card while preserving a shared-container model, but only with a new read-only, bounded parser. The card snapshot remains the authoritative `SHARED_MEMORY_CONTAINER`; enumerated directories/files are child observations, never ownership reassignment to a title. The parser should validate card geometry, allocation units, directory chains, names, bounds, and consistency, and report corruption/unknown states rather than repairing or writing. Current EmuWiz has no such primitive; it has only presence/kind detection.

Classification: `SUPPORTING_EVIDENCE` for save inventory, never `CONFIRMED_IDENTITY` from a directory name alone. A save serial or embedded title code could become corroborating evidence only after format-specific validation and collision analysis.

## D. PlayStation identity and metadata

PS Multi Tools confirms several familiar sources but does not reveal a stronger identity primitive than EmuWiz already has:

- `PARAM.SFO`: PS Multi Tools has a generic reader; EmuWiz's shared parser is bounded, fails closed, preserves unknown value types, and projects platform-specific keys as evidence.
- PBP: PS Multi Tools extracts PBP contents with an external packer; EmuWiz validates the fixed `\0PBP` header, all eight offsets, file bounds, and bounded embedded SFO without extracting the payload.
- PS1/PS2 disc identity: PS Multi Tools mostly relies on the selected CUE/BIN and external conversion. EmuWiz's disc evidence and topology-aware optical paths are stronger for conservative identity.
- PS3: reading `PS3_GAME/PARAM.SFO` and `TITLE_ID` is already represented by EmuWiz's PS3 disc evidence and bounded PKG header evidence.
- PS5 JSON: PS Multi Tools exposes useful fields such as `titleId`, `contentId`, application/content version, required system software, and repository URL. No corresponding bounded `param.json`/`manifest.json` evidence parser was found in the inspected EmuWiz core. This is a real future primitive only for PS5 content that EmuWiz intends to inventory; it must be metadata evidence, not platform proof.
- Vita: PS Multi Tools' relevant functionality is largely external-tool/PKG/PFS plumbing rather than a clearly implemented identity parser. EmuWiz already has Vita3K platform/launch identity gates; no additional PS Multi Tools primitive is proven here.

## E. Patches and updates

The PS5 patch flow is a discovery/download workflow, not a trust model. It accepts a user-supplied or filename-derived title ID, opens a third-party patch site, and downloads `.pkg` URLs. It does not verify hashes, package signatures, release provenance, version-to-title consistency, or whether a result is official.

The useful concept for future EmuWiz is a provenance-bearing catalogue record:

`target platform + title ID + source URL + source class + version/build + required firmware + observed hash + signature/verification status + retrieval time`.

The source class must remain explicit: official update, community patch, mod, firmware, or homebrew. A community patch catalogue must never be presented as an official update; firmware and homebrew require separate policy and compatibility handling. No downloader should be adopted from this implementation.

## F. External-tool matrix

| Tool | Function in PS Multi Tools | Licence / maintenance finding | Cross-platform finding | EmuWiz disposition |
|---|---|---|---|---|
| mCiso | PSP ISO↔CSO | Version and licence are not established by the pinned PS Multi Tools source | Bundled per platform in PS Multi Tools | IGNORE until provenance and format compatibility are independently established |
| maxcso | PSP CSO/DAX/ZSO conversion | Upstream documents ISC licensing and multiple algorithms; public repository is active-looking at inspection | Windows/macOS/Linux | REUSE TOOL candidate after pinning, compatibility tests, and output verification |
| bchunk | BIN/CUE→ISO | Upstream repository is archived; confirm licence before redistribution | Portable C tool, but not a topology-preserving answer | IGNORE as a generic EmuWiz conversion path |
| binmerge | Merge/split binary tracks and CUE | PSMT credits `putnam`; licence/version obligations need upstream review | Python source/bundles vary by platform | RESEARCH FURTHER for fixture generation only; independently validate topology |
| ps3mca-tool | Physical PS2 memory-card adapter access | Old/provenance-sensitive tool; original repository availability is problematic and bundled output parsing is brittle | Adapter/driver and platform constraints | IGNORE for offline Save Vault parsing; evaluate only as an optional external hardware backend |
| DiscUtils | ISO/UDF filesystem access | PSMT uses the LTRData project; verify exact package licence/version before reuse | .NET library, portable within supported runtimes | ALREADY COVERED conceptually; no need to copy PSMT code |
| zPBPTool / IsoPbpConverter | PBP packing and ISO/PBP conversion | Source/version/licence not established in the pinned repository | Bundled platform binaries | IGNORE for identity; independent bounded parsing is preferable |
| 7-Zip/7zz | Archive extraction around downloads/assets | 7-Zip components have LGPL/GNU LGPL obligations depending on component | Broad platform coverage | IGNORE for this audit's core features; separately review if archive support is needed |

The relevant upstream references are [maxcso](https://github.com/unknownbrackets/maxcso), [bchunk](https://github.com/extramaster/bchunk), [binmerge](https://github.com/putnam/binmerge), and [ps3mca-tool](https://github.com/jimmikaelkael/ps3mca-tool). “Active-looking” and “archived” are repository-state observations, not a security or compatibility guarantee.

## G. Licence and code-use boundary

The pinned PS Multi Tools repository contains a root `LICENSE` identifying GNU Affero General Public License version 3 (AGPL-3.0). Copying PS Multi Tools source into EmuWiz would require a deliberate licence decision and compliance with AGPL obligations, including source and network-use obligations applicable to the resulting covered work. The README's third-party table does not establish that every bundled binary is redistributable under the same terms.

Documented file formats and publicly documented signatures may be independently implemented without copying AGPL expression. Each external tool must be audited separately for licence, version, source availability, notices, and redistribution terms. This audit recommends independent implementations for identity and Save Vault parsing.

## H. Comparison and genuine gaps

| Feature | PS Multi Tools approach | EmuWiz current support | Genuine gap? | Value | Risk | Recommendation |
|---|---|---|---|---|---|---|
| PSP CSO compression/decompression | UI wrapper around mCiso, levels 1–9, no verification | No CSO converter; launch currently accepts only direct PSP ISO | Yes, future capability only | Medium | Medium/high compatibility and decompression risk | RESEARCH FURTHER |
| BIN/CUE→ISO | bchunk data-track extraction | Topology-first CUE/BIN→CHD with fingerprint verification | No; flattening would regress safety | Low | High topology loss | REJECT generic path |
| BIN merge | binmerge CUE/track transformation | Existing topology-aware optical model | No identity gap | Medium for tooling | Medium | ALREADY COVERED conceptually |
| PS2 memory-card inventory | External ps3mca-tool listing/extraction | Shared/per-game presence only | Yes | High for Save Vault read-only browsing | Medium/high parser complexity | ADOPT as a separate research/implementation task |
| SFO/PBP identity | Generic SFO/PBP external tooling | Bounded SFO/PBP evidence already stronger | No | Low incremental | Low | ALREADY COVERED |
| PS3 title identity | PARAM.SFO / PKG fields | PS3 disc and PKG evidence | No proven gap | Low | Low | ALREADY COVERED |
| PS5 JSON metadata | Direct JSON deserialization/display | No bounded PS5 manifest/param JSON evidence found | Yes, if PS5 inventory is in scope | Medium | Medium metadata confusion | RESEARCH FURTHER |
| Patch discovery | Website/browser downloader; TLS/hash/provenance weak | No downloader, but has patch catalogue concepts | No safe implementation to copy | Medium | Very high trust/supply-chain risk | REJECT downloader; adopt data model only |

### Ranked recommendations

**ADOPT**

- Research and design a read-only PS2 memory-card filesystem observer that retains card-level shared-container identity and emits per-save supporting evidence.
- Preserve provenance fields as a future patch/update catalogue concept, with source class and verification status mandatory.

**CONSIDER**

- Evaluate maxcso independently for a bounded PSP CSO adapter, with compatibility profiles and hash-verified round trips.
- Consider bounded PS5 `param.json`/`manifest.json` metadata evidence if PS5 content inventory is an explicit EmuWiz scope.
- Evaluate binmerge only for controlled fixture/topology workflows, not as a generic conversion shortcut.

**ALREADY COVERED**

- PARAM.SFO, PBP, PS3 identity, PS1/PS2 optical evidence, CHD media classification, GD-ROM-aware handling, and verified topology-first CUE/BIN→CHD routing.

**REJECT**

- Generic PS1/PS2 BIN/CUE→ISO as a preservation conversion.
- Copying PS Multi Tools' AGPL implementation or adopting its unverified mCiso/zPBPTool wrappers without separate licence and compatibility review.
- Patch/firmware download behaviour that bypasses TLS validation or lacks cryptographic integrity and provenance.

## I. Proposed minimal test vectors

These are synthetic and contain no copyrighted payloads.

| Primitive | Synthetic vector | Expected result | Nearby negative / truncation / conflict |
|---|---|---|---|
| CSO v1 reader | Valid header with standard magic, version 1, block size 2048, two index entries and one compressed zero sector | Accept container; decompress to exactly the original 4096 bytes; hash equality | Wrong magic/version, index beyond file, truncated final block, conflicting ISO filesystem identity: refuse or retain format-only evidence |
| PS2 card observer | Synthetic card geometry/header and two valid directory entries chained to two distinct save directories | Accept card as `SHARED_MEMORY_CONTAINER`; enumerate two child saves as supporting evidence | Bad page/block geometry, looped chain, entry points outside card, truncated directory: report corruption/unknown and never assign a title |
| SFO | `00 50 53 46`, 20-byte header, one bounded `DISC_ID` text entry | Parse and emit product-code evidence only in PSP/PS3 context | Wrong magic, table offset outside input, oversized count, truncated value: no evidence |
| PBP | `00 50 42 50`, version, eight monotonic little-endian offsets, bounded synthetic SFO in section 0 | Accept PBP container and read `DISC_ID` | Non-monotonic offsets, last offset past EOF, truncated 0x28-byte header, valid PBP plus conflicting platform context: keep container evidence but do not force platform |
| PS5 JSON | Minimal object with `titleId`, `contentId`, `applicationVersion`, `contentVersion`, `requiredSystemSoftwareVersion` | Emit metadata supporting evidence, never platform proof from JSON alone | Duplicate/wrong-type fields, oversized/deep JSON, conflicting title IDs across files: reject conflicting field or mark ambiguous |
| CUE topology gate | Synthetic single MODE1/2048 data track plus a separate AUDIO track case | Accept only the supported single-track case; reject or preserve topology for audio/mixed-mode case | Missing INDEX 01, invalid sector mode, conflicting track length: fail closed |

No tests were added because this was explicitly a research audit.

## J. Explicit answers

1. **Should EmuWiz add PSP CSO compression/decompression?** Yes as a future, separately scoped capability, preferably evaluated with maxcso or an independently verified implementation. PS Multi Tools' mCiso wrapper alone is not sufficient evidence for direct adoption.
2. **Can PS2 memory cards be safely inspected internally while preserving Save Vault's shared-container model?** Yes, with a bounded read-only parser. Keep the complete card as the shared snapshot and expose contained saves as child observations; never infer exclusive ownership from a shared card.
3. **Does PS Multi Tools reveal identity evidence EmuWiz lacks?** No for SFO, PBP, PS3, PS1, or PS2 identity. It suggests a possible future bounded PS5 JSON metadata primitive, subject to confirmed EmuWiz scope.
4. **Are PS1/PS2 conversion paths unsafe because they destroy track/topology information?** Yes. Generic BIN/CUE→ISO can discard audio tracks, pregaps, indexes, and mixed-mode topology; PS2 use through the shared bchunk route must not be advertised as lossless.
5. **Are patch/update concepts useful?** Provenance-bearing records, explicit source classes, version/build fields, and verification state are useful. The downloader/security behaviour is not.
6. **Which external tools are worth evaluating separately?** maxcso first; binmerge only for controlled topology work; DiscUtils only where a filesystem library is needed. Do not use mCiso, bchunk, ps3mca-tool, or opaque PBP binaries without separate provenance, licence, and compatibility review.

## Final audit result

- Platforms/features compared: 11 requested platform families plus CSO, PBP, memory-card, metadata, and patch workflows.
- Already covered: 8 of the 11 platform identity areas (PS1, PS2, PS3, PSP/PBP, Dreamcast/optical, Xbox, GameCube, Wii); Saturn/Sega CD/PCE/Neo Geo are outside this PS Multi Tools overlap and were not counted as PS Multi Tools findings.
- Genuine gaps: 2 actionable future primitives: read-only PS2 memory-card filesystem inventory, and conditional PS5 JSON metadata evidence; PSP CSO is a capability gap but requires separate compatibility research.
- Strongest candidate improvement: read-only shared PS2 memory-card inventory.
- Highest false-positive risk: treating a flattened BIN/CUE→ISO result, a save directory name, or a third-party patch URL as authoritative identity.
- CHD/conversion finding: EmuWiz's topology-first, fingerprint-verified CUE/BIN→CHD path is safer; PS Multi Tools provides no evidence that its BIN/CUE→ISO route preserves topology. No CHD routing change is justified.
- Licence finding: PS Multi Tools is AGPL-3.0; independently implement documented formats and audit every external tool separately.

