# Manic EMU Import Pipeline Audit — EmuWiz (RESEARCH ONLY)

> **Research snapshot.** This document records findings from Manic EMU's *public documentation website* (`manicemu.site`) as of the date recorded below, plus targeted web search. It proposes no code, and no production Rust file in this repository was modified while producing it. It does not implement, sketch, or scaffold a downloader, SMB/WebDAV/cloud connector, ingestion pipeline change, or save importer for EmuWiz. Save Vault implementation code, Publisher Profiles implementation code, existing ingestion code, and any downloader/SMB/WebDAV/cloud-connector code were not modified — Save Vault and Publisher Profiles were re-confirmed absent from `crates/` this pass (see §2), and no remote-source connector code exists in this repository either (also re-confirmed, §2).
>
> **This audit does not propose:** a downloader, an SMB/WebDAV/cloud client, an installer, ingestion-pipeline changes, or a save importer for EmuWiz. Every "flow" described below (§13, §6's state machine, §8's routing recommendation) is a conceptual design sketch for future research, not a specification ready to implement.
>
> **A note on method, stated up front because it changes what this document can honestly claim:** the task brief that produced this audit assumed Manic EMU is closed-source (an App Store-only iOS app), and instructed doc-site research rather than source inspection on that basis. That assumption turned out to be **wrong** — `Manic-EMU/ManicEMU` is a real, AGPL-3.0-licensed GitHub repository (501 stars, 112 commits on `main`, 42 forks, confirmed via a single `WebFetch` of the repo page) containing what its README describes as the actual Xcode project and app source. **This audit does not pivot to a source-code audit.** Per the task brief's explicit scope, the research below stays confined to `manicemu.site`'s guide/FAQ pages and general web search — the same posture used for a genuinely closed product. This is stated honestly rather than silently treated as "close enough": a future audit that wants source-grounded claims about Manic EMU's *actual* import implementation (not just its documented behavior) should target that repository directly as its own pass, with its own depth-of-inspection disclosure, not be folded into this one after the fact.

**Scope:** Manic EMU (`manicemu.site`) import/save/format documentation — UX and workflow patterns for a future EmuWiz unified import system (multi-source: local, removable media, SMB, WebDAV, cloud, RomM, existing-library), evaluated against EmuWiz's existing universal-ingestion infrastructure (`crates/archivefs-core/src/ingestion/`), identity/evidence model (`game_identity.rs`, `identity_source/`), and multi-disc grouping (`platform_evidence_fusion/library_grouping.rs`, `cue_m3u_parsing.rs`).
**Branch:** `feature/archivefs-unified-platform`
**Method:** `WebFetch` of `manicemu.site/guides/import/`, `manicemu.site/guides/` (index), `manicemu.site/guides/faq/`; `WebSearch` for format lists, cloud-provider lists, and save-format lists not fully covered by the two direct fetches; read-only `Read`/`Bash grep` inspection of real EmuWiz source files in `crates/archivefs-core/src/`, none modified.

---

## 1. Purpose and scope

EmuWiz's task brief for this audit describes a future goal: a unified, multi-source import system that treats local files, removable media, SMB shares, WebDAV servers, cloud-drive connectors, RomM, and an existing on-disk library as variations on one import model, rather than as separate one-off features. Manic EMU is named as useful prior art specifically for its *import UX* — it documents (not necessarily uniquely invents) a source-agnostic "Import" surface covering local files, iCloud, Wi-Fi transfer, Handoff, drag-and-drop, five cloud providers, and WebDAV/SMB, plus multi-disc and save-file handling, all presented as one screen with a "+" button for adding new source types.

This document extracts *documented UX and workflow shape* — never Manic EMU's internal implementation, which per §2's method note this pass deliberately does not inspect — classifies what EmuWiz's existing ingestion/identity infrastructure already covers versus what would be genuinely new, and proposes research-stage design sketches (a unified import flow, a capability matrix, a remote-transfer state machine) for whoever designs EmuWiz's actual unified import feature next. Nothing here is a decision; it is raw material plus an honest evidence-quality read.

**Explicit non-goals, restated:** no downloader, no SMB/WebDAV/cloud connector implementation, no change to the ingestion pipeline, no save importer, no change to Save Vault or Publisher Profiles code (neither exists as implemented code — see §2).

## 2. Sources inspected and grounding note

### 2.1 Manic EMU sources — depth of inspection, stated honestly

This is documentation research on what is functionally a closed product from the *user's* vantage point (the App Store listing is the distribution channel almost every real user encounters), even though a source repository exists. Per §0's method note, this pass stayed within the doc site and web search, exactly as instructed for the closed-source case the task brief described.

| Source | Method | Depth |
|---|---|---|
| `manicemu.site/guides/import/` | `WebFetch`, full page | Read in full via the fetch tool's summarization; this is the primary source for §3-§13 below |
| `manicemu.site/guides/` (index) | `WebFetch` | Read for the guide list only — confirmed the FAQ and Import Guide are the two relevant pages; confirmed no separate "cloud storage," "WebDAV," or "supported formats" page exists as its own guide |
| `manicemu.site/guides/faq/` | `WebFetch`, full page | Read in full — this is where the CUE/GDI UTF-8 requirement, the Multi-Disc Assistant, the CHD-vs-multi-file distinction, iCloud sync (membership-gated), manual backup path, and the save-vs-save-state distinction were found. These specifics were **not** present in the Import Guide page itself |
| `WebSearch` "Manic EMU import formats supported platforms CHD RVZ WBFS" | Search snippet aggregation, not a direct page fetch | Format/platform lists below are aggregated from search-result snippets referencing the Import Guide and an emulation wiki page — **not independently re-verified against the primary page's own text**, since the Import Guide `WebFetch` summarized rather than quoted its full format table. Treated as directionally reliable, not word-for-word verified |
| `WebSearch` "Manic EMU save file import" | Search snippet aggregation | Save-format list (`.sav`, `.srm`, `.mcd`, `.mcr`, `.eep`, `.dsv`, `.bkr`, Dreamcast VMU) aggregated the same way — **flagged as not independently re-verified word-for-word against a primary source**, since no dedicated "save formats" page was located and reachable in this pass |
| `github.com/Manic-EMU/ManicEMU` | `WebFetch`, repo landing page only | Read only to confirm the licensing/legitimacy question raised in §0 — **no source file inside the repository was opened**, per the deliberate scope decision in §0 |

**What this means for the claims below:** every claim attributed to the FAQ or Import Guide pages is reasonably solid — those two pages were fetched directly and read in full. Claims about the *complete* format list, the *complete* save-format list, and the *complete* cloud-provider list are aggregated from search snippets that themselves cite the Import Guide, and should be read as "documented, probably close to complete, not independently word-for-word verified" rather than a verbatim transcription. Where a specific claim in the task brief's brief (e.g. "PSP savedata," "Saturn," "3DS," "DS DSV" as distinct save formats) could not be independently confirmed against a directly-fetched page, this is marked **not independently verified** rather than presented as confirmed.

**What was not found at all, honestly:** no page documenting WebDAV/SMB authentication flow in technical detail (token type, retry behavior, resumability), no page describing archive-extraction safety behavior (traversal protection, decompression-bomb limits), no page describing exact duplicate-detection logic, no page describing exact metadata fields captured per import, no page describing space-efficiency behavior (copy vs. reference). These are stated as **not documented / not verifiable** throughout, rather than guessed.

### 2.2 EmuWiz-side grounding (read-only, this pass, not modified)

- `crates/archivefs-core/src/ingestion/` (7 files: `container.rs`, `content_registry.rs`, `cue_bin.rs`, `discovery.rs` (2,555 lines), `gdi.rs`, `structural_probe.rs`, `mod.rs`) — this **is** EmuWiz's existing "universal ingestion" work the task brief refers to. Its own module doc (`mod.rs`) states the design explicitly: `container::ContainerKind` answers "how is this stored" (zip, tar, folder, direct file); `content_registry::ContentKind` answers "what does it represent" (ROM cartridge, disc image, Amiga image, WHDLoad install, extracted game folder) — a second, coarser registry deliberately kept separate from the pre-existing `media_registry`; platform assignment stays entirely `crate::platform`'s job, reused unmodified. `discover_source` is documented as strictly read-only: "no file is renamed, moved, deleted, extracted, or otherwise written by anything reachable from `discover_source`." `discovery.rs`'s `SkipReason` enum (`UnsupportedExtension`, `RecognizedContentNoIdentityMatch`, `MissingPairedFile`, `AmbiguousPlatform`, `InvalidContent(String)`) is EmuWiz's existing typed vocabulary for "why wasn't this imported," each with a `label()` and `suggested_action()` — this is directly comparable to §11's duplicate/conflict vocabulary this audit is asked to apply.
- `crates/archivefs-core/src/ingestion/cue_bin.rs` (577 lines) and `gdi.rs` (640 lines) — CUE/BIN and GDI pairing is **already implemented**, bounded (`MAX_CUE_BYTES = 256 KiB`, `MAX_CUE_FILE_REFERENCES = 99`), and its own doc comment is explicit that a `.cue` is the only anchor: "A lone `.bin` with no matching `.cue` is never guessed at here." `CueError` is a closed enum (`TooLarge`, `NoFileReferences`, `Malformed`, `UnsafeReference`, `MissingDataFile`, `AmbiguousDataTracks`, `UnsupportedTrackMode`) — typed, fail-closed outcomes, not a permissive best-effort parse.
- `crates/archivefs-core/src/platform_evidence_fusion/cue_m3u_parsing.rs` (138 lines) and `library_grouping.rs` (227 lines) — M3U/multi-disc grouping is **already implemented**, and — critically — its own module doc states the safety rule directly: multi-disc membership is derived *only* from a confident DAT audit verdict's `game_name`, run through the same `multidisc_group_key` parser `classify_catalogue` itself already uses internally, "never a second looser parser," with an explicit warning that "filenames that merely *look* similar are never evidence." `SetMembership::MultiDiscPart { base_title, part, total }` is the typed grouping outcome; a file with no confident DAT match is never grouped, full stop.
- `crates/archivefs-core/src/game_identity.rs` (12,767 lines) — strictly read-only, bounded identity inspection ("Identity is evidence: only values obtained from reviewed on-disc structures are `Verified`. Archive and member names can only produce `Candidate` values" — module doc, first two lines). This is the multi-signal evidence model §4's classification is compared against: per-platform header/boot-sector parsers (PSX `SYSTEM.CNF`, N64 header+CIC, NES iNES header, Saturn system ID, Sega CD product code, PC-FX boot sector, PS3 disc evidence, PSP PBP header, etc.) feed a shared evidence/confidence model, never filename alone for a `Verified` result.
- `crates/archivefs-core/src/content_detector.rs` (504 lines) and `archive_member_content_evidence.rs` (1,414 lines) — the detector-contract layer and the bounded archive-member content-evidence layer. The latter's module doc states its own multi-member policy explicitly: "never picks a member... A ZIP with `game1.rom` and `game2.rom` producing two different, confident product identities is reported as `ArchiveContentClassification::ConflictingStrongMembers`, not resolved to one" — directly relevant to §11's conflict-outcome vocabulary and §5's archive-handling section.
- `crates/archivefs-core/src/identity_source/net_policy.rs` (558 lines) — the **only** remote-endpoint-reaching infrastructure found anywhere in `crates/` this pass, and it is a refusal policy, not a connector: it validates a user-typed identity-source URL (e.g. a RomM server) against a local-network-only allowlist (loopback, RFC 1918, IPv6 unique-local), resolves every DNS answer and requires *all* of them to be approved (closing the DNS-rebinding hole), refuses embedded credentials in the URL, refuses all redirects, and names cloud/link-local metadata addresses (`169.254.169.254`, the AWS/Azure/GCP/Alibaba/Oracle metadata IPs) as explicitly-refused by identity, not just by range. Grep this pass confirms **no** SMB client, WebDAV client, or general-purpose cloud-storage connector exists anywhere in `crates/` — `net_policy.rs` is a fetch-refusal policy for one narrow identity-lookup use case (matching RomM's shape), not a general remote-import subsystem.
- `crates/archivefs-core/src/safe_read/mod.rs` — the one bounded, read-only file-open policy this build has: absolute paths only, no `.`/`..` components, a symlink is only followed when it and its canonicalized target both lie inside an explicitly configured trusted root, opened `O_NOFOLLOW | O_CLOEXEC` with a post-open device/inode re-check. This is the existing local-filesystem safety discipline §15's recommendations are held against.
- `crates/archivefs-core/src/playing_library/romm_library_plan.rs` — has an existing `RommLibraryBlockReason::DuplicateDestination { other_dat_entry_name }` variant with label "Duplicate planned destination" — confirms EmuWiz already has *a* typed duplicate-outcome concept, just scoped to one library-planning path (RomM projection), not a general import-time duplicate/conflict model.
- **Save Vault / Publisher Profiles:** `grep -rli "save_vault\|savevault\|publisher_profile\|publisherprofile" crates/ --include=*.rs` returned **zero matches** this pass — re-confirmed absent, consistent with both `APOLLO_SAVE_TOOL_AUDIT.md` and `EMUHAVEN_EMULATOR_MANAGER_AUDIT.md`.
- **SMB/WebDAV/cloud connectors:** `grep -rli "smb\b\|webdav\|\bcloud\b" crates/ --include=*.rs` returned only `identity_source/net_policy.rs` and `identity_source/tests.rs` (its test module) — **confirmed absent** as a real connector; the only "cloud" awareness in the codebase is `net_policy.rs`'s refusal-by-name of cloud *metadata* endpoints, which is a security control against a different concern (SSRF into a cloud provider's own instance-metadata service), not a cloud-storage import feature.

## 3. Import source model

**Documented (FAQ + Import Guide):** Manic EMU presents import sources behind what reads, from the documentation's structure, as one "Import" screen with several entry points rather than separate top-level features per source:

| Method | Documented mechanism |
|---|---|
| File Browser | Navigate Import → Files, select target file (local device storage and iCloud, both reachable through the same file-browser control) |
| Wi-Fi Transfer | Import → Wi-Fi Transfer; app displays an IP address; user opens that address in a desktop/other-device browser and uploads through a web page served by the app |
| Clipboard / Handoff | Copy a file in another app, open Import, tap "Read Clipboard" for automatic recognition (Apple Handoff cross-device paste) |
| Drag-and-drop | Long-press a file in Files (or a compatible app), drag into the Manic EMU interface, release |
| Cloud services | Tap "+" on the Import screen, select a cloud service (Google Drive, Dropbox, OneDrive, Baidu Cloud, Alibaba Cloud per search-aggregated sources — **not independently word-for-word verified**, §2.1), follow standard file-import steps |
| WebDAV / SMB | Same "+" flow, "enter protocol parameters" |

**What is genuinely documented as one abstraction:** every source funnels into the same "select target file(s), then import" step once connected — the FAQ and Import Guide never describe a source-specific *destination* or *review* step; import appears to be a single action once a file is selected, regardless of source.

**Not documented / not verifiable:** streaming-vs-download/cache behavior for remote sources; resumability of an interrupted Wi-Fi/cloud/WebDAV/SMB transfer; any staging area or intermediate location visible to the user before a file "lands"; auth/token storage or refresh behavior for the five cloud providers or WebDAV/SMB credentials; error/retry behavior on a failed transfer; whether duplicate detection happens at import time or only later. None of these are addressed on the two pages fetched, and no dedicated technical page (network architecture, security whitepaper) was located.

**Comparison to EmuWiz:** EmuWiz's `discover_source` (`ingestion/discovery.rs`) already models "one abstraction over heterogeneous inputs" for *local* sources — archives, loose ROMs, disc images, WHDLoad folders, extracted game folders are all scanned through one entry point and produce one typed `SourceDiscoveryReport`. What EmuWiz does not have, confirmed absent by grep (§2.2), is any source *beyond* the local filesystem: no removable-media-specific handling, no SMB/WebDAV client, no cloud-drive connector. Manic EMU's "+"-button pattern — one screen, source type chosen at the point of adding it, same downstream flow after that — is a reasonable UX shape to hold EmuWiz's own future design against, but the actual streaming/staging/resumability behavior behind it is undocumented, so there is nothing concrete to adopt beyond the shape of the screen itself.

## 4. Platform/format detection

**Documented:** the Import Guide's format table is organized *by platform* (the user or the file's extension apparently determines platform — the fetched summary describes "extensive format support across ~30 gaming platforms" listed per-platform, e.g. "Nintendo Wii: .rvz .wbfs .ciso .wia .iso .wad .dol .elf"). Nothing on either fetched page describes *how* a file is matched to a platform once selected — whether by extension alone, by directory/folder convention, by header/magic-byte inspection, by embedded metadata, or by user selection. The FAQ's CUE/GDI section implies at least **filename-driven pairing** (a `.cue`/`.gdi` "bundled with various binary formats," must be UTF-8) but says nothing about content-level verification of what's inside those binary files.

**Classification against this audit's vocabulary** (STRONG_IDENTITY / SUPPORTING_EVIDENCE / HEURISTIC / USER_OVERRIDE):

| Signal | Documented? | Classification if assumed |
|---|---|---|
| File extension | Yes — the entire format table is extension-keyed | HEURISTIC at best — extension alone never proves platform in EmuWiz's own model either |
| Filename/directory convention | Implied only for CUE/GDI pairing, not for platform assignment generally | Not documented enough to classify |
| Magic-byte/header inspection | **Not documented** — no page states Manic EMU inspects file contents to confirm platform | Not verifiable |
| Embedded metadata (e.g. disc volume label, SFO) | **Not documented** | Not verifiable |
| Archive-contents inspection | **Not documented** — nothing states what happens when a ZIP/7Z is imported: does it inspect contents, or trust the archive's own name? | Not verifiable |
| User-selected system | **Not documented as a fallback**, though plausible for ambiguous extensions (e.g. `.iso` used by several platforms) — not confirmed either way | Not verifiable |

**Honest conclusion:** Manic EMU's documentation gives no basis to classify its detection signals beyond "extension-keyed, at minimum." This is not a criticism of the product — it is a limit of what a user-facing guide page discloses, consistent with §0's framing that this is UX research, not implementation verification.

**Comparison to EmuWiz (grounded in real code):** EmuWiz's own detection stack is dramatically more evidenced than anything documented for Manic EMU. `content_registry.rs`'s `ContentKind` table is extension-keyed for the *category* question only ("this is a ROM cartridge, a disc image, an Amiga image" — never a platform), explicitly leaving platform to `crate::platform`. `game_identity.rs` (12,767 lines) then does real structural verification per platform — PSX `SYSTEM.CNF` boot-path parsing, N64 header + CIC lookup + CRC validation, NES iNES header parsing, Saturn system ID, Sega CD product code, PC-FX boot sector — with the module's own house rule that only reviewed on-disc structures produce a `Verified` result; archive/member names can only ever produce `Candidate`. This is a categorically deeper evidence model than anything documented for Manic EMU, which appears (per what's disclosed) to stop at extension-based classification for the general case. There is nothing in the Manic EMU documentation that suggests EmuWiz should weaken or replace any part of this — if anything, the comparison reinforces that EmuWiz's evidence discipline is already ahead of what a mainstream, well-regarded emulator frontend documents doing.

## 5. Format coverage

**Documented (aggregated from Import Guide + search snippets, §2.1 caveat applies):**

| Family | Formats documented |
|---|---|
| Nintendo Wii | `.rvz` `.wbfs` `.ciso` `.wia` `.iso` `.wad` `.dol` `.elf` |
| Nintendo GameCube | `.gcm` `.gcz` `.rvz` `.iso` `.dol` `.elf` |
| PC-Engine/TurboGrafx | `.pce` `.sgx` `.cue` `.ccd` `.chd` `.toc` `.m3u` |
| Other ~27 platforms (Nintendo 3DS/N64/NDS/GBA/GBC/GB/NES/SNES/Virtual Boy/PokeMini; Sony PS1/PSP; Sega Dreamcast/Saturn/Master System/Game Gear/SG-1000/32X/Sega CD/Genesis; Atari 2600/5200/7800/Jaguar/Lynx; Arcade/MAME; DOOM) | Exact per-platform extension lists for these were **not** captured in full by the fetches this pass performed — the fetch tool's summarization reported platform names and format *categories* (CHD, ZIP, 7Z as generic compressed containers) more reliably than a complete per-platform extension table |

**Documented per §2.1 FAQ fetch, more reliably:** `.chd` is explicitly described as "a compressed image file (a whole disc in one file)," contrasted with `.cue+.bin` or `.gdi+(bin/iso/rom)` as "multiple files but still one disc" — i.e. the FAQ explicitly teaches the user the container-vs-content distinction EmuWiz's own `container::ContainerKind` vs. `content_registry::ContentKind` split also models (§2.2), though nothing suggests Manic EMU exposes that as two separate concepts internally — it reads as a single explanatory FAQ answer, not evidence of an internal architectural split.

**Not documented / not verifiable per format:** conversion behavior (does importing a `.cue+.bin` ever get converted to `.chd`, or kept as-is?), source-structure preservation (are original files kept alongside an imported copy, or consumed?), whether direct-launch is supported for every listed format or only some, and single-vs-multi-file classification beyond what the FAQ states for CUE/GDI/CHD specifically.

**Comparison to EmuWiz:** EmuWiz's `content_registry.rs` (partially read this pass, lines 1-150 of a larger table) already recognizes essentially the same disc-image extension set Manic EMU documents for Wii/GameCube (`iso`, `gcm`, `gcz`, `rvz`, `wbfs`, `ciso`, `chd`, `pbp`, `gdi`, `cdi`, `xiso` all present as `ContentKind::DiscImage` entries) plus a broader cartridge/tape/snapshot/Amiga taxonomy Manic EMU's documentation does not mention at all (Amiga ADF/HDF/RDB, computer-disk formats, cassette/tape images, machine snapshots — none of these appear in Manic EMU's documented platform list, which is unsurprising since Manic EMU does not document Amiga or 8-bit-computer support). **Genuine potential gap, format-coverage only:** `.wia` (Wii ISO-archive, a losslessly-compressed Wii/GameCube format related to RVZ) does not appear in the 150-line excerpt of `content_registry.rs` read this pass — this is a partial-read finding, not a confirmed absence across the full file, and should be verified with a full grep before being treated as a real gap.

## 6. Multi-file/disc-set import

**Documented, and this is the most concretely useful section of the FAQ:**
- `.cue`/`.gdi` files must be **UTF-8 encoded plain text** — stated as a hard requirement ("Make sure they are saved in UTF-8 encoding"), a specific, actionable, and verifiable claim.
- Multi-disc games (Dreamcast, PS1, Mega-CD, Saturn) require an `.m3u` file that "tells the emulator how many discs the game has."
- A documented feature called the **"Multi-Disc Assistant"** handles the compound case explicitly: "an `.m3u` file, several `.cue` files, [and] each `.cue` links to multiple `.bin` files" — named as a distinct assistive UI flow, not just passive file recognition.

**Not documented:** what happens when a companion file is missing (does import fail, warn, or silently proceed with a broken multi-disc set?); atomicity (if one of several `.bin` files fails to import, does the whole set roll back?); renaming behavior; duplicate-companion handling (two `.cue` files both claiming the same `.bin`).

**Comparison to EmuWiz (grounded in real code — this is where EmuWiz already has more than Manic EMU documents):**
- CUE/BIN pairing is implemented in `ingestion/cue_bin.rs` (577 lines): a `.cue` is the only valid anchor, bounded at `MAX_CUE_BYTES = 256 KiB` and `MAX_CUE_FILE_REFERENCES = 99`, with a **closed, typed error enum** (`CueError::TooLarge`, `NoFileReferences`, `Malformed`, `UnsafeReference`, `MissingDataFile`, `AmbiguousDataTracks`, `UnsupportedTrackMode`) — every one of the "not documented" failure modes above (missing companion, ambiguous tracks, unsafe reference) already has a named, fail-closed outcome in EmuWiz, whereas Manic EMU's documentation is silent on what happens in those cases.
- GDI handling exists as a parallel module (`ingestion/gdi.rs`, 640 lines).
- M3U/multi-disc grouping (`platform_evidence_fusion/cue_m3u_parsing.rs`, `library_grouping.rs`) is **evidence-gated, not filename-gated**: `library_grouping.rs`'s own module doc states multi-disc membership is derived *only* from a confident DAT audit verdict's `game_name`, reusing the exact same `multidisc_group_key` parser the catalogue-classification code already uses internally — "never a second looser parser" — with an explicit stated warning that "filenames that merely *look* similar are never evidence." A file with no confident DAT match is never grouped into a multi-disc set at all.
- `SetMembership` is a closed enum (`SingleFile` / `MultiDiscPart { base_title, part, total }`), not an implicit UI behavior.

**Conclusion:** Manic EMU's documented "Multi-Disc Assistant" is a genuinely useful *UX* concept (naming and surfacing the compound-import case as its own assisted flow, rather than leaving it to be discovered implicitly) — that naming/surfacing pattern is worth learning from. But on the *engineering* side, EmuWiz's existing evidence-gated grouping (never trusting filename similarity alone) is already a stricter, more defensible model than anything Manic EMU documents; there is no format-handling capability here EmuWiz needs to catch up on, only a UI-surfacing idea (see §12, §18 Q6).

## 7. Archive handling

**Documented:** compressed files (`.zip`, `.7z`) are listed as supported input formats, no more. Nothing on either fetched page describes: extract-vs-browse-in-place behavior, staging location, traversal protection, decompression-bomb limits, nested-archive handling, multi-ROM-archive behavior (does importing a ZIP with several ROMs import all of them, or ask which one?), cleanup-after-failure, or filename-encoding handling for archive members. **Entirely undocumented** beyond "ZIP and 7Z are supported."

**EmuWiz's own posture (grounded):** `archive_member_content_evidence.rs` (1,414 lines) is bounded and read-only by design — it decompresses only a bounded prefix (or an explicitly bounded complete read for reviewed observers), pre-filters via `inspector::classify_entry` before decompressing anything at all, and — most relevantly to the multi-ROM-archive case Manic EMU's docs don't address — has an explicit, **never-silent** multi-member policy: two confident-but-different product identities in one archive produce `ArchiveContentClassification::ConflictingStrongMembers`, never a "largest file wins" guess. `MAX_ARCHIVE_MEMBERS = 4_096` and similar bounded constants appear throughout `game_identity.rs`.

**Recommended minimum EmuWiz safety rules for any future unified-import archive path (design-only, not implementation):**
1. Every archive extraction/browse operation stays bounded (member count, per-member size, nesting depth) — EmuWiz already has this discipline in `archive_member_content_evidence.rs`; a unified import path must not introduce a second, looser archive-reading path that bypasses it.
2. Multi-ROM archives must never silently resolve to one winner — extend the existing `ConflictingStrongMembers`-style outcome to the import-time decision ("this archive contains two candidate games; ask, or import both"), not invent a new heuristic.
3. Nested archives should be bounded and explicit (EmuWiz's `MAX_NESTED_CONTAINER_DEPTH = 1` constant in `game_identity.rs` is the existing precedent to extend, not replace).
4. Filename encoding for archive members should be validated before use as a destination path component — this was not found to be explicitly handled anywhere grepped this pass and is worth a dedicated look before a unified import path trusts archive member names for anything beyond display.
5. Any staging location used during extraction should be cleaned up on failure — no code path found this pass to compare against (EmuWiz's current archive-reading paths never write, so this rule is new territory for a future write-capable import path, not a regression risk against existing behavior).

## 8. Remote/network import (SMB, WebDAV, cloud)

**Documented:** existence only — SMB and WebDAV are named as supported protocols, cloud services are named (Google Drive, Dropbox, OneDrive, Baidu Cloud, Alibaba Cloud per aggregated search results, §2.1 caveat). **Nothing else.** No streaming-vs-staging detail, no resumability claim, no partial-download handling, no hash/integrity verification claim, no credential-handling detail, no network-interruption-recovery behavior, no large-file handling detail, no retry policy, no concurrency behavior. This is the single largest documentation gap relative to the task brief's questions — stated as **not documented / not verifiable**, not guessed.

**EmuWiz's own posture (grounded):** the only remote-reaching code in `crates/` is `identity_source/net_policy.rs` — a *metadata lookup* endpoint-validation policy (for something RomM-shaped: a user-typed server URL, later queried with a bearer token), not a file-transfer subsystem. It is instructive as a security model even though it solves a different problem: local-network-only allowlist (loopback/RFC1918/IPv6-ULA), every DNS answer must be approved (not just the first, closing DNS rebinding), no embedded credentials in the URL, zero redirects followed, named refusal of cloud/link-local metadata addresses. Any future SMB/WebDAV/cloud-connector work should treat this module as the house security bar to match or exceed, not as directly reusable code (it validates *metadata* endpoints, not file-transfer endpoints, and a file-transfer connector has a different threat model — large binary payloads, long-lived transfers, resumability — that this module does not address).

**Proposed EmuWiz remote-transfer state machine (design-only, per task brief, no implementation):**

```
DISCOVERED    -- source listed the item (name, size, remote path); nothing fetched yet
   |
   v
QUEUED        -- user or policy selected it for import; nothing fetched yet
   |
   v
TRANSFERRING  -- bytes are being read/staged; progress is observable; can move to INTERRUPTED
   |
   v
VERIFYING     -- bytes-received are checked against a claimed hash/size, if the source offers one
   |
   v
COMPLETE      -- verified and available to the rest of the import pipeline (§13)
```
With two off-path states reachable from TRANSFERRING or VERIFYING:
```
INTERRUPTED   -- connection dropped, no verified data yet, or a partial write exists but unverified
   |
   v
NEEDS_RESUME  -- a resumable partial exists and the source is believed to support resuming (protocol-dependent: HTTP-range-style resume is plausible for WebDAV/cloud-HTTP sources; SMB has its own semantics; neither was verified against a real implementation this pass)
   |
   v (back to TRANSFERRING, or)
FAILED        -- resume not possible or repeatedly failed; partial data discarded, never silently kept as if complete
```

This is offered as a conceptual state shape only — it does not specify wire protocols, an SMB/WebDAV library choice, or how "resumable" would actually be determined per source type, none of which this pass researched.

**Answering the task brief's stream-vs-stage question (§18 Q4) is addressed there, not duplicated here.**

## 9. Space-efficient import

**Documented:** not addressed on either fetched page. No statement about whether importing from local storage copies the file, references it, or moves it; no statement about deduplication against an already-imported identical file.

**EmuWiz comparison (design-only, no implementation proposed):** a future unified import system should preview which storage action it will take, never silently fall back to one path if another fails, and never guess. Candidate actions, drawn from the task brief's own vocabulary:

| Action | When it would apply | Storage cost |
|---|---|---|
| `REFERENCE_IN_PLACE` | Source is already on a filesystem EmuWiz can read persistently (a local drive, a mounted NAS share) and the user does not want a copy | Zero additional space; fragile if the source is later moved/unmounted — must be surfaced, not hidden |
| `HARDLINK` | Same filesystem/volume as the destination library, filesystem supports hardlinks | Zero additional space, survives source-file rename but not deletion-and-recreate |
| `SYMLINK` | Cross-volume reference where hardlinks aren't possible | Zero additional space, fragile to source removal, visible to the user as a link |
| `REFLINK` | Copy-on-write filesystem (btrfs, XFS, APFS) | Zero additional space until either copy is modified |
| `COPY` | Cross-device, removable media about to be ejected, or explicit user choice | Full additional space, most durable |
| `CONVERT_TO_COMPRESSED_FORMAT` | E.g. CUE/BIN → CHD, where the user explicitly opts in | Reduced space, but a lossy-to-original-bytes transform that must never happen silently (breaks byte-for-byte DAT/hash verification unless the conversion is itself DAT-aware) |

No silent fallback between these — this echoes the same discipline already present in `net_policy.rs` (explicit refusal over silent degrade) and `cue_bin.rs` (explicit typed errors over best-effort guessing). Storage implications (how much space each action would actually take, whether the source can safely be removed afterward) should be previewed to the user before the action runs, not discovered afterward.

## 10. Save import

**Documented (FAQ, read directly):**
- Native game saves use "universal formats like `.sav` (GBA), `.dsv` (NDS), `.srm` (SNES)" — three examples given directly by the FAQ, described as "seamlessly transferable across devices/emulators."
- **Save states are explicitly excluded**: "does not support importing/exporting" — stated as a deliberate limitation, not an oversight, and a single DS save state is cited as "approximately 20MB" (explaining why, plausibly: state snapshots are large binary memory dumps, unlike small save files).
- Backup path (non-members): `Files app > On My iPhone/iPad > Manic EMU > Datas folder`, with a "one-click export" via long-press on a game icon.
- Membership-gated iCloud sync: "automatically sync game data via iCloud Drive... Files app > iCloud Drive > Manic EMU folder" — a subscription feature, not free-tier behavior.

**Additional save formats, aggregated from search snippets, §2.1 caveat (not independently word-for-word verified against a primary fetch):** `.mcd`, `.mcr`, `.eep`, `.bkr`, Dreamcast VMU "Memory Units." The task brief's specific list (PS1 save/card, Dreamcast VMU, Saturn, PSP savedata, 3DS, DS DSV, GB/GBA SAV) is **partially confirmed** — `.sav` (GBA), `.dsv` (NDS/DS), `.srm` (SNES, and per search aggregation also PS1-adjacent), Dreamcast VMU are reasonably supported by what was fetched or search-aggregated; PSP savedata, 3DS, and Saturn-specific formats beyond `.srm`/`.bkr` were **not independently verified** against any directly-fetched page in this pass — stated as not documented/not verifiable rather than assumed present.

**Extension-recognition-only vs. content-parsing:** not documented either way. The FAQ names save formats by extension only; nothing states whether Manic EMU inspects save-file contents (e.g. to verify it matches the currently-loaded game) before accepting an import.

**Game-identity matching, destination selection, overwrite behavior, account/user binding, backup-before-import, conflict handling:** **none of this is documented** on either fetched page. The FAQ's own text implicitly assumes the user has already selected the correct game before importing its save (nothing describes an automatic game-identity check), and says nothing about what happens if a save file already exists at the destination.

**Save Vault re-verification (per task instructions, not assumed from prior audits):** `grep -rli "save_vault\|savevault" crates/ --include=*.rs` returned zero matches this pass (§2.2) — consistent with the prior Apollo and EmuHaven audits' findings, freshly re-confirmed rather than carried over as an assumption.

**Should EmuWiz save import always route through IDENTIFY → REVIEW → PRE-IMPORT SNAPSHOT → APPLY → VERIFY rather than direct file copying?**

**Yes**, and the justification does not actually depend on anything Manic EMU documents (which, per the finding above, documents *no* safety steps around save import at all) — it follows directly from EmuWiz's own existing discipline elsewhere in the codebase:
- `game_identity.rs`'s house rule ("only values obtained from reviewed on-disc structures are `Verified`... never filename alone") already establishes that EmuWiz does not trust weak signals for identity anywhere else in the pipeline; a save file landing in the wrong game's directory because "the filename looked right" would be a direct regression from that standard.
- `cue_bin.rs`'s closed, fail-closed `CueError` taxonomy and `archive_member_content_evidence.rs`'s "never silently resolved" multi-member policy both establish that EmuWiz's house style treats "I'm not sure, so I'll refuse or ask" as the default over "best-effort and hope" — a save overwrite is a strictly higher-stakes operation than an ambiguous ROM import (it can destroy hours of a user's progress), so it should get *at least* the same discipline.
- The prior `APOLLO_SAVE_TOOL_AUDIT.md` (§10, referenced not re-derived here) already established, from Apollo's own source, that "download-and-hope, told to back up in a text label" is the wrong bar and that a pre-import snapshot should be automatic, not left to the user to remember — that conclusion applies with equal force to a *local* save import from Manic-EMU-style sources, not just a community-save-download scenario.
- Manic EMU's own documented behavior — silent about overwrite handling, silent about identity verification — is exactly the *absence* of this discipline, which makes it a caution rather than a model to follow for this specific piece.

The proposed flow, stated as design-only: **IDENTIFY** (confirm which game this save belongs to, using EmuWiz's existing evidence model, never filename alone) → **REVIEW** (show the user what would be overwritten, if anything) → **PRE-IMPORT SNAPSHOT** (capture the existing destination state before touching it, automatically) → **APPLY** → **VERIFY** (confirm the write landed as expected). This composes cleanly with a future Save Vault (once it exists) rather than requiring Save Vault to be built first — a minimal version of PRE-IMPORT SNAPSHOT could exist as a plain file copy today, upgraded to a real Save Vault snapshot primitive later, without changing the flow's shape.

## 11. Save format depth

Classified per this audit's vocabulary (EXTENSION_ONLY / CONTAINER_AWARE / IDENTITY_AWARE / FORMAT_AWARE / UNKNOWN), applied honestly to what's documented — not assumed from extension support:

| Save type | Documented support | Classification | Rationale |
|---|---|---|---|
| GBA `.sav` | Named explicitly (FAQ) | EXTENSION_ONLY | Only the extension and platform pairing are stated; no parsing/validation behavior described |
| NDS `.dsv` | Named explicitly (FAQ) | EXTENSION_ONLY | Same |
| SNES `.srm` | Named explicitly (FAQ) | EXTENSION_ONLY | Same |
| Dreamcast VMU | Named ("Memory Units") | UNKNOWN | Whether VMU images are treated as a structured multi-file container (VMU images can hold multiple game saves, analogous to Apollo's PS1 card model per `APOLLO_SAVE_TOOL_AUDIT.md` §3) or as an opaque blob is not documented |
| PS1 `.mcr`/`.mcd` | Search-aggregated, not independently verified | UNKNOWN | Not confirmed against a primary source this pass |
| Saturn `.bkr` | Search-aggregated, not independently verified | UNKNOWN | Same |
| PSP savedata, 3DS saves | Named in the task brief, not independently found in either fetched page | UNKNOWN | Not documented / not verifiable this pass |

**Overall:** nothing found in Manic EMU's documentation supports classifying any save format above EXTENSION_ONLY. This is stated plainly rather than assumed — a save-management feature can be genuinely CONTAINER_AWARE or IDENTITY_AWARE internally without ever describing that in a user guide, so this classification describes *what is documented*, not necessarily the product's true internal depth, consistent with §0's framing.

## 12. Metadata captured during import

**Documented:** none, explicitly. Neither the Import Guide nor the FAQ states what metadata (title, platform, region, game ID, serial, disc number, version, filename, source location, artwork, hashes, source provider, timestamps) is captured or displayed after an import completes. The FAQ's backup-path description (`Files app > ... > Manic EMU > Datas folder`) implies *some* on-disk organization exists, but nothing about its structure or what metadata it encodes was documented on either fetched page.

**EmuWiz comparison (grounded in real code):** EmuWiz's existing evidence model already captures substantially more than anything documented for Manic EMU: `game_identity.rs`'s `IdentityEvidence`/`GameIdentityReport` types (referenced, not re-derived in full here — 12,767 lines, not read end-to-end this pass) model `IdentityStatus`, `IdentityKind`, `IdentityConfidence`, `IdentityPlatform`, `IdentityImageFormat`, and `IdentityProvenance` as distinct typed fields (per the prior Apollo audit's own grounding, §11 there, independently re-confirmed by this pass's grep). `library_grouping.rs`'s `GameReleaseSetHierarchy` additionally carries `platform`, `game_label` (with an explicit `game_label_is_dat_confirmed` boolean distinguishing a confirmed DAT title from a basename fallback), `set` membership, and `revision`/`cloneof` lineage where available. This is already a richer provenance model than anything Manic EMU documents.

**Useful missing provenance fields, if a unified import system were built (genuinely new, not already covered):** *source location and source type* (which of local/removable/SMB/WebDAV/cloud/RomM this item came from) is not currently modeled anywhere in the grounding files read this pass, because no remote-source path exists yet to need it — this would be a real, new field for a unified import system to add, not a gap in the *existing* local-import evidence model.

## 13. Duplicate/conflict handling

**Documented:** not addressed on either fetched page — same-file-twice, same-game-different-dump, region conflicts, filename conflicts, destination-exists, partial-prior-import, and newer-save-over-older are all undocumented.

**EmuWiz comparison (grounded):** `playing_library/romm_library_plan.rs` already has a typed `RommLibraryBlockReason::DuplicateDestination { other_dat_entry_name }` (label: "Duplicate planned destination") — a real, if narrowly-scoped (RomM library projection only), precedent for typed duplicate outcomes. `discovery.rs`'s `SkipReason` enum is the broader precedent: every "this wasn't imported" case is a named variant with both a `label()` and a `suggested_action()`, never a silent skip.

**Recommended typed outcomes for a future unified import system**, using this audit's own vocabulary (design-only):

| Outcome | Meaning | Existing EmuWiz precedent to extend |
|---|---|---|
| `ALREADY_PRESENT` | Byte-identical file already in the library (hash match) | None found yet — would need a library-wide content-hash index, not present today per this pass's grounding |
| `SAME_CONTENT_DIFFERENT_SOURCE` | Same verified identity, different container/source (e.g. a `.chd` vs. the equivalent `.cue+.bin`) | `content_registry`'s container/content split is the right conceptual foundation; no existing cross-container identity-equality check was found this pass |
| `IDENTITY_CONFLICT` | Two items claim the same identity but differ in verified evidence | `archive_member_content_evidence.rs`'s `ConflictingStrongMembers` outcome is the direct within-archive precedent; a library-wide version would be new |
| `NAME_COLLISION` | Same destination filename, no identity evidence either way | `RommLibraryBlockReason::DuplicateDestination` is the closest existing precedent |
| `PARTIAL_MEDIA_SET` | A multi-disc/CUE-BIN set imported with a companion missing | `CueError::MissingDataFile`/`SkipReason::MissingPairedFile` already model this for the single-item case; a set-level version doesn't yet exist |
| `REVIEW_REQUIRED` | Anything not cleanly resolved by the above | The `discovery.rs` house style (typed reason + suggested action, never silent) is the pattern to keep using |

## 14. UX findings

Documented, taken at face value from the two fetched pages:
- **Steps required:** each import method is described in 2-4 short steps (e.g. Wi-Fi transfer: enable → enter IP in browser → upload → done) — low apparent friction per the documentation's own framing.
- **Platform/system selection surfacing:** not described as a distinct step anywhere on either page — consistent with the extension-driven detection inferred in §4, though this remains undocumented in detail.
- **Error explanation:** not documented at all — no example error message, no described failure-recovery UI.
- **Progress legibility:** not documented — no mention of progress bars, transfer speed, or ETA for remote sources.
- **Pre-execution inspection / preview:** not documented — nothing describes a review step before an import commits.
- **Batch handling:** implied only by the Multi-Disc Assistant (§6) as a compound-file batch, not a general multi-item batch-import UX.
- **Novice guidance:** the FAQ's own existence, and its plain-language explanations (e.g. spelling out the CHD-vs-CUE/BIN distinction in beginner terms, explicitly warning about UTF-8 encoding for CUE/GDI files — a real, specific pitfall a novice would otherwise hit blind) is itself the strongest documented UX signal in this whole audit: **proactively naming a known gotcha before the user hits it** is a genuinely transferable UX habit, independent of any implementation detail.

## 15. Proposed unified EmuWiz import flow

`SOURCE → DISCOVER → INSPECT → IDENTIFY → GROUP MEDIA → CHOOSE STORAGE ACTION → PREVIEW → TRANSFER/LINK/CONVERT → VERIFY → CATALOGUE`

Mapped against what EmuWiz already has (grounded, not proposing a new ingestion engine where one already exists):

| Stage | Existing EmuWiz primitive | Genuinely new work |
|---|---|---|
| SOURCE | None beyond local filesystem — `discover_source` assumes a readable local path | A source abstraction covering removable/SMB/WebDAV/cloud/RomM — the actual new-connector work, entirely unbuilt (§2.2 grep-confirmed) |
| DISCOVER | `ingestion::discovery::discover_source` — already a general, typed, read-only scan producing `SourceDiscoveryReport` | Extending discovery to remote listings (SMB directory listing, WebDAV PROPFIND, cloud API listing) — new, but the *report shape* (`GameDiscovery`, `SkipReason`, always-populated explanations) generalizes without needing to be reinvented |
| INSPECT | `content_detector`/`archive_member_content_evidence`/`structural_probe` — bounded, read-only content inspection already exists for local bytes | Bounded inspection over a byte range fetched from a remote source without downloading the whole file first (a streaming-inspection capability) — new |
| IDENTIFY | `game_identity.rs` — the deep, evidence-graded identity model | None — this is already the right layer and should be reused unmodified, exactly as `library_grouping.rs` already does for the local case |
| GROUP MEDIA | `platform_evidence_fusion/cue_m3u_parsing.rs` + `library_grouping.rs` — evidence-gated multi-disc grouping | None for the grouping logic itself; possibly new work only if remote sources need to group across multiple *separate* remote listings (e.g. a CUE on SMB referencing a BIN on WebDAV) — an edge case worth flagging, not designing here |
| CHOOSE STORAGE ACTION | Not found anywhere in `crates/` this pass — no reference/hardlink/reflink/copy decision logic exists yet | New — §9's action taxonomy |
| PREVIEW | Not found as a general concept — `discovery.rs`'s `SourceDiscoveryReport` is arguably already close to a "preview" (nothing is written by discovery itself), but no explicit user-facing preview-before-commit step was found | Mostly new UI/workflow work, built on an already-safe read-only discovery layer |
| TRANSFER/LINK/CONVERT | Not found — no remote-transfer code exists | New — §8's state machine |
| VERIFY | `dat/hash.rs` and the broader `dat/` DAT-verification machinery already exist for local content | Extending verification to freshly-transferred remote content is largely reuse, not new design |
| CATALOGUE | `library_views.rs`, `playing_library/` already exist as the cataloguing layer | Extending to record source-location/source-type provenance (§12) is the main new field |

**Conclusion, directly answering the task brief's framing:** the identification, grouping, and eventual cataloguing stages of this flow are **already well-served** by existing EmuWiz code and should not be reinvented. The genuinely new work is concentrated at the two ends — SOURCE/DISCOVER for remote connectors, and CHOOSE STORAGE ACTION/TRANSFER for the write-side transfer/staging logic — which matches this audit's overall finding that EmuWiz's *evidence and identity* engineering is already strong, while *multi-source transfer* is a real, currently-unstarted gap.

## 16. Import source capability matrix

Conceptual/research only — no implementation. `✓` = capability plausible/expected for this source type in general (not a claim about any specific Manic EMU or EmuWiz behavior beyond what's cited); `?` = genuinely source/implementation-dependent; `—` = not applicable.

| Source | LIST | READ | STREAM | RANDOM_ACCESS | RESUME | WRITE | AUTH | HASH_VERIFY | SPACE_EFFICIENT_REFERENCE |
|---|---|---|---|---|---|---|---|---|---|
| Local filesystem | ✓ | ✓ | ✓ | ✓ | — | ✓ | — | ✓ | ✓ (hardlink/reflink/reference) |
| Removable media (USB/SD) | ✓ | ✓ | ✓ | ✓ | ? | ✓ | — | ✓ | ? (fragile — media can be ejected) |
| SMB | ✓ | ✓ | ✓ | ✓ | ? | ✓ | ✓ | ? | ✓ (if mounted persistently) |
| WebDAV | ✓ | ✓ | ✓ | ? | ? (HTTP range plausible) | ✓ | ✓ | ? | — (not typically mountable as a persistent reference target) |
| Cloud connector (Drive/Dropbox/etc.) | ✓ | ✓ | ? | — | ? | ? | ✓ | ? | — |
| RomM | ✓ (EmuWiz already has `net_policy.rs`-gated identity lookups against a RomM-shaped server) | ✓ | ? | — | ? | — | ✓ (existing) | ✓ (DAT/hash infra already exists for local verification, not yet wired to a remote fetch) | — |
| Existing EmuWiz library | ✓ (already, via `discover_source`) | ✓ | — | ✓ | — | — | — | ✓ | ✓ (it's already in place — trivially "space efficient": no transfer needed) |

## 17. Security/privacy

Recommended fail-closed behaviors for a future unified import system, extending patterns already established elsewhere in EmuWiz (design-only, no implementation):

- **Archive traversal / decompression bombs:** extend `archive_member_content_evidence.rs`'s existing bounded-read discipline (`MAX_ARCHIVE_MEMBERS`, bounded-prefix reads) to any newly write-capable import path — never introduce a second, looser extraction path.
- **Malicious filenames:** archive-member and remote-listing filenames must be validated (no `..`, no absolute paths, no control characters) before being used as any part of a destination path — `safe_read/mod.rs`'s existing symlink/trusted-root discipline is the house bar to match for the local-filesystem side of this.
- **Untrusted remote servers:** `identity_source/net_policy.rs`'s local-network-only, all-addresses-approved, no-redirects, named-metadata-refusal model is the existing house security bar; a general file-transfer connector (which by definition must sometimes reach a *public* WebDAV/cloud endpoint, unlike `net_policy.rs`'s RomM-shaped local-only case) would need its own, separately-reviewed threat model — this audit does not attempt to design one.
- **Credentials/tokens:** never logged, never embedded in a URL (`net_policy.rs`'s `EmbeddedCredentials` refusal is the precedent to extend), stored with OS-appropriate secret storage rather than plaintext config.
- **Cloud privacy:** a cloud-connector integration inherently grants EmuWiz visibility into a user's cloud account contents beyond the files they intend to import — scope any future OAuth grant as narrowly as the provider's API allows, and never request cloud-account-wide read access.
- **Interrupted transfers:** never treat a partial transfer as complete (§8's `VERIFYING` state exists specifically to prevent this).
- **Symlink attacks:** `safe_read/mod.rs`'s trusted-root model is the existing discipline; any new write path should apply the same bar, not a looser one.
- **Destination escape:** validate every destination path is confined to the intended library root before any write.
- **Save overwrite:** covered in depth in §10 — never a direct copy without the IDENTIFY → REVIEW → SNAPSHOT → APPLY → VERIFY discipline.
- **Executable/imported scripts:** no script-execution capability should ever be part of an import pipeline — consistent with the prior Apollo audit's explicit rejection of save-patch scripting engines, restated here because the concern is adjacent (an import pipeline is exactly the kind of surface where "just run this helper script the archive included" temptation could recur).

## 18. Legal/distribution boundary

Manic EMU's own documentation, as fetched, describes only *import of user-supplied files* — nothing on either fetched page describes Manic EMU itself downloading or distributing ROMs, firmware, or keys (a meaningfully different posture from EmuHaven's DOWNLOAD/DISTRIBUTE firmware/keys behavior documented in the prior `EMUHAVEN_EMULATOR_MANAGER_AUDIT.md` §11). This audit finds nothing in Manic EMU's import documentation that would tempt EmuWiz toward a ROM-distribution boundary violation — the entire documented surface is "bring your own files, from wherever you already have them," which is the correct posture and the one this audit's proposed flow (§15) already assumes throughout. EmuWiz should stay exactly there: a unified import system moves and organizes files a user already possesses; it does not source them.

## 19. EmuWiz gap-analysis matrix

| Feature | Manic EMU approach (as documented) | EmuWiz current support | Genuine gap? | Value | Risk | Recommendation |
|---|---|---|---|---|---|---|
| Unified multi-source import screen | One "Import" surface, "+" to add a source type, same downstream flow per source (documented UX, not verified implementation) | Local-only; `discover_source` is a strong local abstraction but no other source type exists | Yes | HIGH | LOW (UI/workflow layer, doesn't touch identity engineering) | ADOPT the *screen-shape* concept; RESEARCH FURTHER the connectors themselves |
| CUE/GDI UTF-8 requirement + explicit user warning | Explicitly documented as a named pitfall in the FAQ | `cue_bin.rs`/`gdi.rs` parse these files but this pass did not confirm whether EmuWiz surfaces a *user-facing* warning for non-UTF-8 CUE/GDI content specifically (encoding handling exists in the parser; a proactive warning UX was not verified either way this pass) | Possibly — needs verification | LOW-MEDIUM | LOW | RESEARCH FURTHER — check whether `cue_bin.rs`/`gdi.rs` already reports encoding problems in a user-legible way before treating this as a gap |
| "Multi-Disc Assistant" naming/surfacing | Named, distinct assisted UI flow for the CUE+M3U compound case | Grouping logic exists and is stricter (evidence-gated) than anything documented for Manic EMU, but this pass did not confirm a comparably-named/surfaced *UI* flow exists in `archivefs-gui` | Possibly, UI-only | LOW-MEDIUM | LOW | USEFUL LATER — a naming/surfacing idea, not an engineering gap |
| Remote source connectors (SMB/WebDAV/cloud) | Documented as supported, implementation undocumented | Confirmed absent (§2.2 grep) | Yes | HIGH | MEDIUM (new attack surface — untrusted remote servers, credentials) | RESEARCH FURTHER — the single largest real gap this audit found, but must be designed against EmuWiz's own security bar (§17), not against anything Manic EMU discloses, since nothing about its remote-transfer safety is documented |
| Space-efficient import (reference vs. copy) | Not documented | Not found anywhere in `crates/` this pass | Yes | MEDIUM | LOW | RESEARCH FURTHER — §9's action taxonomy is a starting sketch |
| Typed duplicate/conflict outcomes at import time | Not documented | Narrow existing precedent (`RommLibraryBlockReason::DuplicateDestination`) plus the general `SkipReason` pattern | Yes, for the general case | MEDIUM | LOW | RESEARCH FURTHER — extend the existing `SkipReason`-style typed-outcome pattern rather than inventing a new vocabulary |
| Save import safety gate (IDENTIFY→REVIEW→SNAPSHOT→APPLY→VERIFY) | Not documented at all — Manic EMU's own docs are silent on overwrite/conflict handling | Not found (Save Vault absent) | Yes | HIGH | HIGH (touches user save data directly) | RESEARCH FURTHER, explicitly gated behind Save Vault existing, consistent with the prior Apollo/EmuHaven audits' identical conclusion |
| Deep save-format parsing (CONTAINER_AWARE/IDENTITY_AWARE) | Not documented — appears EXTENSION_ONLY at best per what's disclosed | Not found | Unclear — Manic EMU itself may not have this either | LOW (per what's documented) | LOW | DO NOT ADOPT as a priority based on this audit alone — nothing here demonstrates Manic EMU does this, so there is no concrete capability to catch up to |

## 20. The ten questions, answered directly

**1. Does EmuWiz need a unified Import Sources screen?**
As a future UI/workflow concept, yes — this is the one area where Manic EMU's documentation offers a genuinely clean, replicable *shape* (one screen, sources added via a consistent "+" pattern, same downstream flow after connection) that EmuWiz's current local-only import experience does not have. This is a workflow/UX recommendation, not an engineering one — it does not require rebuilding anything in `ingestion/`.

**2. Does current EmuWiz ingestion already have the backend primitives needed?**
Partially, and unevenly. **Yes** for identification, multi-disc grouping, and archive-member evidence (§4, §6, §7 all found existing, arguably stronger-than-documented primitives). **No** for anything past "the bytes are already on a local filesystem EmuWiz can read" — there is no remote-source discovery, no space-efficiency decision logic, and no general typed duplicate/conflict outcome beyond one narrow RomM-specific variant. The honest summary: the *identity and organization* half of a unified import system is largely already built; the *multi-source acquisition and storage-decision* half is essentially unstarted.

**3. Which remote sources would add the most value first?**
This audit's research does not produce strong evidence for ranking cloud providers or protocols against each other — nothing in Manic EMU's documentation or EmuWiz's own code gives a basis for that judgment. The one source this audit can speak to with real grounding is **RomM**, because `identity_source/net_policy.rs` already exists specifically shaped for a RomM-like local-network server, meaning it is the lowest-incremental-effort remote source to extend into a genuine import path (metadata-fetch infrastructure and its security model already exist; only the actual file-transfer half would be new). Beyond that, this audit defers rather than guesses.

**4. Should SMB/WebDAV import stream directly or stage locally?**
Not resolvable from Manic EMU's documentation (entirely silent on this) or from EmuWiz's current code (no remote-transfer code exists to observe). As a design-only recommendation: **stage locally for anything that will be verified (§10's snapshot discipline, DAT/hash verification) before being trusted as complete**, since EmuWiz's own house style throughout `cue_bin.rs`, `archive_member_content_evidence.rs`, and `game_identity.rs` is "read fully into a bounded, controlled state before trusting it" rather than acting on data mid-stream. A read-only *browse* of an SMB/WebDAV listing (to show what's available before committing to import) is a different, lower-risk operation that could reasonably stream/list without staging. This is a recommendation, not a finding — no existing code or Manic EMU documentation settles it either way.

**5. Which Manic EMU format-support gaps are genuinely relevant?**
Based on the (partial, §2.1-caveated) format comparison in §5, the one concrete, not-fully-verified candidate is `.wia` (Wii ISO-archive) — absent from the 150-line excerpt of `content_registry.rs` read this pass, but this needs a full-file grep before being treated as a confirmed gap, not this audit's partial read. No other genuine format gap was identified with confidence — EmuWiz's disc-image extension coverage (§5) already closely matches what Manic EMU documents for the platforms both projects cover, and EmuWiz additionally covers platforms (Amiga, 8-bit computer disk/tape formats) that Manic EMU's documentation does not mention at all.

**6. Does Manic provide any multi-file import behavior better than EmuWiz's topology engine?**
No, on the engineering side — §6 found EmuWiz's evidence-gated grouping (confident DAT match required, filename similarity explicitly rejected as evidence) to already be stricter and more defensible than anything documented for Manic EMU. What Manic EMU does offer that's worth noting is a **UX naming/surfacing pattern** (the "Multi-Disc Assistant" as a distinctly named assisted flow) — a presentation idea, not a capability gap.

**7. Should save imports always route through Save Vault safety primitives?**
Yes, per §10's reasoning — grounded in EmuWiz's own existing fail-closed discipline elsewhere (`game_identity.rs`, `cue_bin.rs`, `archive_member_content_evidence.rs`) rather than in anything Manic EMU documents (which documents no safety steps around save import at all, making it a caution rather than a model here). The IDENTIFY → REVIEW → PRE-IMPORT SNAPSHOT → APPLY → VERIFY flow should be the standing rule once Save Vault exists, and a minimal snapshot-only version is a reasonable interim step before Save Vault is built, not a reason to skip the discipline in the meantime.

**8. Which archive safety rules are mandatory?**
Per §7: bounded extraction (member count/size/nesting, extending existing `MAX_ARCHIVE_MEMBERS`-style constants), no silent multi-ROM-archive resolution (extend `ConflictingStrongMembers`-style typed outcomes to import time), bounded nested-archive depth, validated archive-member filenames before any use as a destination path, and cleanup-after-failure for any future write-capable staging path.

**9. How should EmuWiz minimise duplicate storage during import?**
Per §9: an explicit, previewed storage-action choice (`REFERENCE_IN_PLACE`/`HARDLINK`/`SYMLINK`/`REFLINK`/`COPY`/`CONVERT_TO_COMPRESSED_FORMAT`) chosen deliberately per situation, never a silent fallback between them, with the storage-space implication shown to the user before the action runs. Neither Manic EMU's documentation nor EmuWiz's current code offers a ready-made answer here — this is a genuinely open design question this audit can only frame, not resolve.

**10. What should EmuWiz explicitly NOT copy from Manic EMU?**
Per §21 below — most importantly: undocumented (and therefore, on the evidence available, probably absent or unverified) safety behavior around save overwrite, duplicate handling, and remote-transfer integrity. Silence in a user-facing FAQ is not proof of a safe default underneath it, and EmuWiz should not assume Manic EMU has solved these problems just because its guide doesn't mention them going wrong — build EmuWiz's own versions against EmuWiz's own already-higher evidentiary bar (§4's identity model, §10's snapshot discipline) instead.

## 21. Explicit DO-NOT-ADOPT list (consolidated)

- **Extension-only platform/format detection as the primary or only signal** — even if that's genuinely what Manic EMU's documentation implies it uses (§4), EmuWiz's own `game_identity.rs` evidence model is already stronger and should not be weakened to match a documentation-inferred baseline for a different product.
- **Treating documentation silence as evidence of safety** — every "not documented" finding in this audit (save overwrite behavior, remote-transfer integrity, archive-extraction safety, duplicate handling) must not be read as "so it's probably fine" when EmuWiz designs its own equivalent; build against EmuWiz's own fail-closed house style instead (§10, §17).
- **A single "import" action with no review/preview step**, if that is indeed Manic EMU's documented flow (§3, §14) — EmuWiz's own `discover_source`/`SourceDiscoveryReport` design already produces a rich, inspectable report before anything is written; a future unified import system should keep and extend that preview step, not collapse it away for a faster-feeling single-tap flow.
- **Filename-similarity-based multi-disc grouping** — restated even though nothing suggests Manic EMU actually does this (its CUE/GDI/M3U handling is filename-driven by necessity, not evidence of loose grouping); EmuWiz's own `library_grouping.rs` module doc already explicitly warns against this, and this rule should not regress if a unified import path is ever built on top of remote sources where DAT verification might be slower or unavailable at import time.
- **A save-import path that copies directly to destination with no identity check, no preview, and no pre-import backup** — the documented Manic EMU behavior (or rather, the *undocumented* absence of any described safety step) is not a bar to match; see §10, §18 Q7.
- **Silent storage-action fallback during import** (e.g. silently converting a CUE/BIN to CHD, or silently falling back from a hardlink attempt to a full copy without telling the user) — §9, consistent with the `net_policy.rs`/`cue_bin.rs` house style of explicit refusal/typed-outcome over silent degrade.
- **Pivoting this specific audit into a source-code audit of `Manic-EMU/ManicEMU`** — restated from §0: this document deliberately did not do that, and a future pass that wants source-grounded Manic EMU claims should scope that as its own audit with its own depth-of-inspection disclosure, not retroactively blur into this one's doc-only framing.

---

*This document adds no code. No file under `crates/` was modified. Save Vault, Publisher Profiles, and every downloader/SMB/WebDAV/cloud-connector code path were not read or touched during this research pass (remote-connector code was re-confirmed absent from the repository via a fresh grep, §2.2). All proposed flows, state machines, and matrices in §8, §9, §13, §15, §16, and §17 are design-stage sketches only, not implementation specifications.*
