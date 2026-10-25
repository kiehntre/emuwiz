# EmuWiz Mods and Patches Architecture Audit

## Scope and conclusion

This audit covers non-cheat modifications, user-supplied patch artifacts,
texture packs, ROM hacks, translations, emulator-native patches, and future
mod discovery. It does not propose downloads, scraping, patch application, or
configuration mutation in this document.

The central conclusion is that “mod” is an umbrella product category, not one
installation format. A ROM patch is a transformation that produces a new
content object. A texture pack is a game-identity-keyed read-only asset set. A
PCSX2 or RPCS3 patch is an emulator-interpreted runtime program. A loader mod
may be a directory tree with ordering and dependencies. These must share
provenance, identity, review, and transaction vocabulary while retaining
adapter-specific semantics.

The smallest useful next step is a read-only local artifact inspector and
manifest model. Much of that foundation already exists in EmuWiz as
`mod_package` and the existing `patch_manager`; the immediate product gap is
not another parser or installer, but a coherent cross-adapter manifest and
explicit base/derived relationship. Any future mutation must preserve the
original, require exact identity evidence, produce a preview, and use the
existing staged transaction machinery.

## Repository state inspected

Inspection was performed in `/home/davedap/emuwiz-main-release-fix`.

- Branch: `main`
- Starting HEAD: `c6514ef05256c72db5ecf64e1cb64784f94a030e`
- No production files were changed by this audit.
- Existing unrelated dirty and untracked work was preserved.

The most relevant current source paths are:

| Area | Existing source paths and functions | Finding |
| --- | --- | --- |
| Generic local mods | `crates/archivefs-core/src/mod_package.rs`: `inspect_local_mod_package`, `inspect_local_mod_package_candidates`, `build_local_mod_package_transaction_plan`, `execute_local_mod_package_transaction`, `undo_local_mod_package_transaction` | Real local package inspection and apply/undo seam already exists; it is directory-based and uses `emuwiz.mod.json`. |
| Generic mod GUI | `crates/archivefs-gui/src/local_mod_package_page.rs`, `crates/archivefs-gui/src/cheats_mods_preview.rs` | Existing preview/apply/undo presentation is integrated into Cheats & Mods; this is not a blank surface. |
| Package safety | `mod_package.rs`, `patch_manager/import_safety.rs`, `patch_manager/destination_safety.rs` | Bounds, path confinement, symlink/special-file refusal, identity checks, and trust states already exist. |
| Shared transaction | `patch_manager/shared_transaction.rs`: `build_shared_transaction_plan`, `execute_shared_apply`, `execute_shared_rollback`, `require_local_mod_package_verification` | Reusable journal, backup, atomic-write, ownership, and rollback boundary. |
| PCSX2 | `patch_manager/pcsx2_identity.rs`, `pcsx2_pnach.rs`, `pcsx2_install_plan.rs`, `local_cheat_install_pcsx2.rs` | PNACH parsing, identity-keyed naming, preview, staged write, and rollback exist. Runtime patch semantics remain PCSX2-specific. |
| RetroArch | `patch_manager/retroarch.rs`, `retroarch_materialization.rs`, `retroarch_cheat_*`, `cht_document.rs` | Strong cheat/source/preview/install/history path; it is not a universal ROM-mod adapter. |
| Dolphin cheats | `dolphin_code.rs`, `gecko_document.rs`, `dolphin_gecko_install_plan.rs`, `local_cheat_install_dolphin.rs` | Gecko/Action Replay-like runtime codes and safe generated-file workflows exist. |
| Dolphin textures | `dolphin_texture_pack.rs`, `dolphin_texture_mod.rs` | Read-only inspection, game-ID matching, archive preview, transaction planning, and apply exist. |
| Other gamehacking | `gamehacking_*`, `bsfree_*` | Provider/catalogue and GameCube/Wii code workflows are cheat/gamehacking features, not proof of a general mod system. |
| Xenia | `xenia_patch_document.rs`, `xenia_install_plan.rs` | Emulator-specific patch document support exists; it must not be generalized from filename similarity. |
| Archive safety | `archive_workflow.rs` and related archive modules | ZIP/7Z/RAR safe extraction work is the correct future import boundary. |
| Identity | `game_identity`, `identity_source`, `platform_evidence_fusion`, DAT and optical modules | Hashes, serials, title IDs, platform and topology evidence can support exact mod matching. |
| Launch isolation | `launch/resource_grants.rs` | `LaunchResourceGrantSet` is now the future typed boundary for exposing mod assets/config/state; no projection executor is present. |
| Storage/conversion | `repair/optical_conversion.rs`, storage conversion and topology modules | Conversion is topology-first; a patch artifact must never assume CHD is equivalent to a raw BIN/CUE target. |

The repository also contains research describing safe local import and
patch/cheat boundaries, notably `docs/PATCH_CHEAT_MANAGER_DESIGN.md`,
`docs/RETROARCH_CHEAT_WORKFLOW.md`, `docs/RETROARCH_ENVIRONMENT.md`,
`docs/CHEAT_FORMAT_SEMANTICS_V5.md`, and the local-mod package documentation
in `mod_package.rs`.

## What “mod” means

### Taxonomy

| Class | Typical examples | Primary identity | Typical mechanism | EmuWiz policy |
| --- | --- | --- | --- | --- |
| `ROM_PATCH` | IPS, BPS, UPS, xdelta/VCDIFF, PPF | Exact input hash and declared output hash | Derive a new file | Future controlled apply only; source immutable |
| `TRANSLATION_PATCH` | BPS/IPS fan translation | Base hash, region, revision | Derived ROM or disc image | A derived game, not a replacement of the base |
| `ROMHACK` | Gameplay, level, balance or graphics hack | Base hash plus patch identity | Derived ROM or image | Preserve base and patch provenance |
| `EMULATOR_PATCH` | PCSX2 PNACH, RPCS3 `patch.yml`, Dolphin GameINI patch | Serial/title ID plus executable/build hash where required | Runtime memory or emulator patch file | Adapter-specific, reviewable, never filename-only |
| `TEXTURE_PACK` | Dolphin, PCSX2, PPSSPP replacement textures | Game ID/serial plus texture hash/key convention | Read-only emulator asset directory/archive | Identity-keyed asset package; often huge |
| `HD_TEXTURE_PACK` | High-resolution replacement images | Same as texture pack plus emulator/version constraints | Layered texture directory | Separate storage/performance class |
| `WIDESCREEN_PATCH` | Aspect-ratio runtime patch or GameINI | Game revision/executable identity | Emulator patch/config/code | Not equivalent to a display setting |
| `60FPS_PATCH` | Frame pacing/runtime code patch | Exact executable/build identity | Runtime patch and often emulator settings | High compatibility risk; explicit warning |
| `MODEL_REPLACEMENT` | Game data or texture replacement | Game/region/revision and target paths | Layered files or loader | Adapter-specific conflict/load-order rules |
| `AUDIO_REPLACEMENT` | Replacement music/voice assets | Game/revision and target paths | Layered files or derived image | Treat as content replacement, not cheat |
| `SCRIPT_MOD` | Loader scripts or code modules | Game and loader/version | Mod-loader directory | High-risk active content; never auto-execute unknown scripts |
| `GAME_DATA_MOD` | Filesystem replacement, DLC-like content | Game title/revision and relative paths | Staged owned files | Manifest and conflict transaction required |
| `MOD_LOADER_CONTENT` | Riivolution, Brawl file patches, LayeredFS-style trees | Game ID and loader | Loader-specific virtual filesystem | Do not flatten into ordinary file replacement |
| `UNKNOWN_MOD` | Unclassified package | None until inspected | None | Inspect-only and fail closed |

“Cheat” remains separate. A cheat is primarily a runtime code, memory write,
trainer, or code database entry. A mod changes or supplies persistent content,
assets, scripts, or emulator patch configuration. A 60 FPS PNACH is technically
an emulator patch even if its user purpose is a gameplay enhancement. A cheat
database may contain entries called “patches”; classification should follow
the adapter's execution semantics, not the marketing label.

When classification is ambiguous, retain both the source label and EmuWiz's
conservative classification, mark the result `UNKNOWN_MOD` or
`REVIEW_REQUIRED`, and do not make it installable. Cheat storage and mod
installation must not be merged merely because both can alter a running game.

## Source immutability and derivatives

The original game is an authoritative source object. The safe relationship is:

```text
BASE GAME (immutable)
  + PATCH ARTIFACT (verified and attributed)
  -> DERIVED GAME (new path, new hash, linked provenance)
```

The derived record should retain the base identity, expected base hash, patch
identity and version, patch hash, source/license, output hash, platform,
region/revision, and the exact tool/adapter semantics used. A failed or
partially produced output is not a game and must not replace the base record.

Reflinks can be considered as a storage optimization only after filesystem
semantics are proven. A hardlink is not an independent derived output because
later writes alias the original inode. A plain copy is the correctness
baseline. A temporary output is appropriate for preview or verification; a
persistent derived game requires an explicit user-approved destination and
its own identity record.

For multi-track or multi-disc material, a patch target must name the topology
it expects. A patch designed for a raw BIN/CUE sector stream is not proven
safe against a CHD, M3U, or an entire multi-disc set. Conversion to the
expected representation is a separate reviewed operation, and the original
topology must remain preserved.

## Proposed manifest model

The existing `emuwiz.mod.json` is a useful local v1 foundation. A future
cross-adapter manifest should extend the concept without making every field
mandatory:

```text
ModManifest {
    format_version
    mod_id
    title
    version
    author
    description
    classification
    source { kind, url, license, attribution }
    targets [{ platform, emulator, emulator_version_range }]
    base_requirements [{ identity_kind, value, region, revision, topology }]
    patch { format, input_hash, output_hash, tool, tool_version }
    files [{ path, sha256, size, role }]
    install_method
    dependencies [{ mod_id, version_range }]
    conflicts [{ mod_id, target_path, reason }]
    load_order
    provenance
    user_notes
}
```

Manifest fields are claims, not trust anchors. Unknown or absent fields remain
unknown. `source_url` is provenance and discovery metadata; it is not an
authorization to fetch. Licenses and attribution should be displayed even
when the package is user-supplied.

The manifest should distinguish an artifact checksum from the resulting game
hash, and distinguish a required base hash from a weak filename or title
claim. A complete game image, executable, script, or opaque installer inside
a “mod” archive must not become automatically executable or importable.

## Identity matching and outcomes

Filename similarity is insufficient. The matching order should reuse existing
EmuWiz evidence:

1. exact base content hash (SHA-1/SHA-256, with format-appropriate hash scope);
2. authoritative DAT identity and revision/region;
3. platform-specific identity such as PS1/PS2 serial and executable CRC, PS3
   title ID, PSP disc ID, GameCube/Wii game ID, Dreamcast product code, Xbox
   title/media ID;
4. verified media topology and region/revision;
5. title/filename only as display context or weak candidate evidence.

Suggested outcomes:

| Outcome | Meaning | Install eligibility |
| --- | --- | --- |
| `COMPATIBLE` | Required base identity and all required constraints match | Eligible for a later reviewed plan |
| `LIKELY_COMPATIBLE` | Strong platform/identity evidence but missing an exact source or revision proof | Preview only; never silent apply |
| `REVIEW_REQUIRED` | Ambiguous, conflicting, incomplete, or topology-sensitive evidence | No automatic apply |
| `INCOMPATIBLE` | Exact identity, region, revision, emulator or topology contradicts the manifest | Refuse |
| `UNKNOWN` | Not enough evidence to decide | Refuse and explain what is missing |

The current local-mod implementation already has `Compatible`, `Incompatible`,
and `Unknown`, platform/identity/region/revision checks, and explicit blockers.
The genuine future gap is adding a separate `LikelyCompatible` and
`ReviewRequired` distinction where the UI needs to explain strong-but-not-exact
evidence; it should not weaken the existing exact eligibility gate.

## Patch-format audit

| Format | Technical characteristics | Verification opportunity | Tooling evidence | Recommendation |
| --- | --- | --- | --- | --- |
| IPS | `PATCH` header, offset/data records, optional RLE and truncation extension | Input size may be checked; format has no universal source/target cryptographic hashes | The small Rust `ips` crate parses hunks and truncation; [docs.rs/ips](https://docs.rs/ips/latest/ips/) | Independent bounded parser or carefully reviewed crate; require manifest/input/output hashes for safe apply |
| BPS | `BPS1`, variable-length integers, source/target/patch CRC32; copy/insert operations | Strong built-in source and target CRCs, but a product should still record SHA-256 | Flips implements BPS and publishes its spec; [Flips](https://github.com/Alcaro/Flips) | Good first ROM-patch candidate after exact base verification |
| UPS | `UPS1`, XOR-style changes, source/target/patch CRC32 and sizes | Source/target checksums and sizes are useful; still add SHA-256 | The Rust `ups` crate exposes source/target/patch CRC and `apply`; [docs.rs/ups](https://docs.rs/ups/latest/ups/struct.UpsPatch.html) | Consider after BPS; bounded input/output and exact hashes required |
| xdelta/VCDIFF | VCDIFF delta streams; commonly used for large binary files | Xdelta 3.2 armor mode can embed BLAKE3 source/target verification when built with it | [xdelta](https://github.com/jmacd/xdelta) is Apache-2.0 in current 3.2.x; `xdelta3` Rust bindings exist at [docs.rs/xdelta3](https://docs.rs/xdelta3/latest/xdelta3/) | Evaluate as a separate dependency; avoid shelling out to arbitrary user commands |
| PPF | Disc-image patch family with versions/extensions and optional block checks | Image size and optional checks can help, but target topology and sector semantics matter | No sufficiently established permissively licensed Rust baseline was identified in this audit | Research further for PS1/Saturn/Dreamcast disc use; do not offer generic apply |

The `rompatch-rs` project is useful evidence of modern pure-Rust apply-only
coverage and format signatures, but its license, maintenance and supported
surface require an independent dependency review before adoption. Flips is
GPL-3.0 and is in maintenance mode according to its README; copying it into
EmuWiz would create GPL obligations and is not justified for a first
implementation. Algorithms and publicly documented formats can be
independently implemented, subject to the format's documentation and any
separate patents or tool licenses.

No format should be treated as lossless merely because an application returns
success. The safe pipeline is: verify input identity, parse bounded patch,
produce a new output, verify output hash and declared size, then register the
derivative. Applying multiple ROM patches in sequence is not generally safe
unless each stage declares the exact preceding hash.

## Emulator-specific audit

| Emulator | Native mod/patch surface | Identity/key | State and install implications |
| --- | --- | --- | --- |
| RetroArch | Core/content-specific cheats and core-specific behavior; patch support is not one universal external mod format | Content hash/CRC, playlist/core/system and existing catalogue identity | Use a core-specific adapter and `LaunchResourceGrantSet`; do not expose or rewrite the whole config/cheat tree |
| PCSX2 | PNACH runtime patches, widescreen/no-interlace patches, built-in GameDB patches, and texture replacement | Serial plus executable CRC; PCSX2 docs name files like `SCES-50916_6A8F18B9.pnach` | PNACH is runtime code, not a derived disc patch. Exact serial/CRC and patch version are required; patch files are adapter-owned |
| RPCS3 | `patch.yml`, keyed by PPU executable hash and title; patch entries can include version/configuration and memory operations | Title/serial plus PPU hash and RPCS3 version context | YAML is active emulator input. Parse and validate schema, but never execute downloaded YAML or assume title-only matching |
| Dolphin | GameINI patches, Gecko/Action Replay codes, resource packs, custom textures, and loader-specific content such as Riivolution/File Patch Code | Game ID, region/revision and texture/resource path conventions | Resource packs have a documented manifest and `textures/GAMEID` layout; use the existing Dolphin texture path and conflict model |
| PPSSPP | Texture replacement packs with `textures.ini`, hashes and game-ID directories | PSP game ID and texture hash/key | Official docs describe `PSP/TEXTURES/<GAME_ID>` and zipped packs; large packs have storage/VRAM/performance implications |
| DuckStation | Emulator settings and cheat/patch ecosystem are adapter-specific; no universal content-mod install contract was established here | PS1 serial/disc identity and emulator-specific patch key | Treat as future research; do not infer a generic PNACH/texture layout |
| xemu | Console filesystem/HDD/mod-loader possibilities are distinct from disc media; no general safe universal mod surface established here | Xbox title/media identity and exact filesystem/topology | Requires explicit writable HDD/NAND/state policy; no broad folder injection |
| Azahar | Custom textures and title-ID-keyed assets are documented in ecosystem guidance; version/config behavior is evolving | 3DS title ID | Treat texture packs as title-ID-keyed assets with explicit config capability and bounded package inspection |
| Ryujinx/Ryubing | LayeredFS-style mod directory trees and emulator-specific metadata are common in the ecosystem | Title ID and release/build context | High active-content and load-order risk; inspect-only first, no universal installer or arbitrary scripts |

PCSX2's official patch documentation distinguishes runtime patches from
cheats and GameDB patches, and specifies serial/executable-CRC naming. RPCS3's
patch documentation and source use title/PPU hash keys and expose patch
versions/configuration. These are strong evidence that emulator-native mods
must remain adapter-specific. See [PCSX2 patch documentation](https://pcsx2.net/docs/advanced/writing-patches/),
[PCSX2 patches](https://github.com/PCSX2/pcsx2_patches),
[RPCS3 game patches](https://github.com/RPCS3/rpcs3/wiki/Game-Patches), and
[RPCS3 patch implementation](https://github.com/RPCS3/rpcs3/blob/master/Utilities/bin_patch.cpp).

### Texture-pack findings

Dolphin's Resource Pack Specification v2 defines a ZIP with `manifest.json`,
optional `logo.png`, and `textures/GAMEID/...`; packs can target complete IDs
or region-neutral prefixes. Deactivation is by removing the pack files from
the Dolphin user `Load/Textures` area. This is a strong model for manifest and
identity matching, but not proof that every texture pack is safe or compatible.
See [Dolphin Resource Packs](https://github.com/dolphin-emu/dolphin/blob/master/docs/ResourcePacks.md).

PPSSPP's documentation specifies `textures.ini`, selectable hash behavior,
hash-to-file mappings, and `PSP/TEXTURES/<GAME_ID>` placement. It supports
opening a ZIP and installing it, but EmuWiz should not copy that destructive
or implicit behavior: inspect first, validate the identity key, estimate
storage, and offer an owned staged plan. See [PPSSPP texture creation](https://www.ppsspp.org/docs/reference/texture-replacement/)
and [PPSSPP texture use](https://dev.ppsspp.org/docs/reference/use-texture-replacement/).

PCSX2 texture replacement is an emulator/profile feature and should be keyed
to the same exact game identity used by its PNACH patching. It is not safe to
assume that a directory named after a title is sufficient. Texture packs can
be multi-gigabyte, can collide by key, and can require emulator settings;
these belong in a texture adapter with a read-only preview and explicit
`LaunchResourceGrantSet` assets later.

## Multi-disc, CHD, and topology

ROM patch formats usually target one byte stream. Optical games may instead
have a cue sheet, multiple BIN tracks, subchannel/SBI data, CHD metadata, or a
multi-disc M3U. A patch must declare one of:

- exact single-file image hash and format;
- exact track/image topology and member hashes;
- a filesystem-relative replacement operation inside a mounted/extracted
  game-data tree; or
- unsupported/unknown topology.

EmuWiz should refuse a BIN/CUE patch against a CHD unless a reviewed adapter
proves an equivalent patch target and verifies the resulting media identity.
It should not convert a CHD solely to make a patch appear applicable. A
derived optical result needs a new topology record, content hash and launch
projection; M3U ordering and disc relationships remain owned by the existing
media-set engine.

## Enablement, conflicts, order, and rollback

Every imported or installed artifact should have an explicit state:

| State | Meaning |
| --- | --- |
| `AVAILABLE` | Known/imported artifact not installed |
| `INSTALLED_DISABLED` | Owned installation exists but is not selected |
| `INSTALLED_ENABLED` | Explicitly selected in the active mod profile |
| `CONFLICT` | Target, version, identity, or order conflict prevents selection |
| `MISSING_DEPENDENCY` | Declared dependency is absent or incompatible |
| `INCOMPATIBLE` | Exact game/emulator/topology constraint fails |
| `UNKNOWN` | Inspection or identity evidence is insufficient |

Conflicts must be computed before writes:

- two file-layer mods target the same normalized path;
- two ROM patches require different predecessor hashes;
- emulator patches target incompatible executable hashes or versions;
- two texture packs provide the same texture key;
- load-order constraints form a cycle or rely on an unspecified tie-break;
- one mod requires a loader/config change that another disables.

ROM-derived patches generally cannot be stacked by arbitrary order. Each patch
must consume the exact output hash of the prior stage. Layered texture/data
mods may support order, but only the adapter can define whether “last wins” is
valid. Unknown ordering is a conflict, not an invitation to sort filenames.

Mutable installs should use the existing shared transaction pattern: preview,
explicit confirmation, owned destination, staged writes, backup of only
replaced owned files, manifest, journal, and rollback. Rollback removes or
restores only files EmuWiz can prove it created or replaced. Unknown user files
remain untouched. A derived ROM rollback is normally deletion of the derived
object and its relationship, subject to user confirmation and no active
references.

Named mod profiles are useful later:

```text
Vanilla
HD Textures
Translation + Quality of Life
Experimental
```

Profiles should select immutable artifacts and adapter settings; they should
not copy or rewrite the base game, and they should not become a second launch
planner. The selected profile can eventually contribute adapter-approved
resources to `LaunchResourceGrantSet`.

## Archive and active-content safety

ZIP/7Z/RAR mod import must reuse the existing archive workflow. It must inherit
absolute/traversal path rejection, case-fold collision checks, member and
expansion limits, symlink/special-file refusal, staged extraction,
verification, overwrite refusal, and cleanup.

Archive recognition is not trust. A package containing scripts, executables,
DLLs, installers, macros, or opaque post-install hooks is active content and
must not be executed. A patch byte stream such as IPS/BPS can be permitted as
a narrowly identified data artifact, but only its reviewed parser may inspect
it and only a later explicit transaction may apply it. No archive member may
choose an arbitrary emulator directory or command.

## Download and legal boundary

EmuWiz should distinguish metadata/indexing from hosting or distributing game
content:

| Source/content | Policy |
| --- | --- |
| User-created patch-only artifact | Safe first import category, subject to license/provenance and exact base matching |
| Translation patch | Same; retain author and license, do not bundle the game |
| Texture pack | Inspect and link to source; redistribution depends on pack license and included assets |
| Mod-loader content | Review package contents and license; do not execute unknown scripts |
| Package containing copyrighted game assets | High-risk; do not redistribute or silently repackage |
| Full prepatched ROM/ISO | Do not build or host; preserve original-user-supplied boundary |

Potential future discovery connectors include GitHub Releases, Nexus Mods,
GameBanana, ModDB, and project-specific repositories. They differ in API
availability, authentication, rate limits, metadata quality, hash/signature
availability, license visibility, and anti-scraping terms. A connector should
be an optional metadata provider, not the trust authority. It must never turn
an HTML page, release asset, or “latest” label into an automatically installable
mod. Start with user-supplied local import and explicit source links.

## Existing EmuWiz gap matrix

| Feature | Exists | Partial | Missing | Reuse |
| --- | --- | --- | --- | --- |
| Cheat parser/catalogues | Yes | Native semantics vary | Cross-adapter mod distinction | `patch_manager/cheat_*`, `cht_document`, provider registries |
| PCSX2 PNACH | Yes | Runtime patch vs cheat UX still specialized | Cross-mod manifest link | `pcsx2_pnach`, identity and install-plan modules |
| Dolphin codes | Yes | Codes are not texture/data mods | Universal profile model | Dolphin code/install modules |
| Dolphin texture packs | Yes | Adapter-specific; not general mods | Cross-adapter manifest/derivative relation | `dolphin_texture_pack`, `dolphin_texture_mod` |
| Generic local package | Yes | Directory-only, limited v1 operations; patch operations are not applied | Archive-backed import, broader taxonomy | `mod_package`, existing GUI and shared transaction |
| Safe archive extraction | Yes | Import wiring is separate | Mod-specific package inspection | Existing archive workflow, no second extractor |
| Identity evidence | Yes | Some adapter mod keys need explicit projections | Universal base requirement schema | Existing hashes, DAT, serial/title-ID modules |
| Preview/apply transaction | Yes | Adapter-specific plans | Cross-adapter ModInstallPlan vocabulary | `shared_preview`, `shared_transaction`, rollback/history |
| Launch planner | Yes | No selected mod-profile input | Future adapter resource binding | `LaunchPlan` remains authority; use `LaunchResourceGrantSet` later |
| Resource grants | Yes | Vocabulary only; no executor | Mod asset projection integration | `launch/resource_grants.rs` |
| Provenance | Yes | Mod-specific attribution/license fields are incomplete | Unified artifact/source provenance | Existing provenance, source registries and journals |
| Rollback | Yes | Different adapters have different ownership scopes | Universal mod manifest ownership | Shared transaction and adapter-specific rollback |
| Remote mod discovery | No | Existing cheat providers are not mod discovery | Connectors/API policy | Reuse source trust and retrieval safety concepts only |
| ROM patch application | No general implementation | Patch operations are recognized/rejected in local package v1 | Bounded IPS/BPS/UPS/xdelta/PPF apply | Independent format adapters; preserve base |

## Recommended roadmap

### ADOPT

1. **MOD0: local read-only artifact inspection and manifest normalization.**
   Reuse `mod_package`, archive safety, existing identity evidence, and shared
   preview vocabulary. Add classification, license/attribution, artifact
   hashes, base requirements, emulator target, dependencies, conflicts, and
   topology fields without applying anything.
2. **Explicit base/derived identity.** Preserve the original and model a
   derived variant with patch provenance and output verification. This is the
   most important missing cross-adapter concept.
3. **User-visible states and review reasons.** Separate available, disabled,
   enabled, conflict, missing dependency, incompatible and unknown. Do not
   make an unselected mod affect normal Ready-to-Play.
4. **Reuse existing safe archives and transactions.** Do not create a second
   extraction engine, preview engine, journal, or rollback implementation.

### RESEARCH FURTHER

1. **MOD1: BPS first, then IPS/UPS.** BPS has useful input/output checksums;
   IPS needs product-supplied exact hashes because the format itself is less
   self-verifying. Establish bounded parsers and output verification before
   any apply action.
2. **xdelta/VCDIFF for large images.** Evaluate Apache-2.0 xdelta 3 and Rust
   bindings, including memory/window limits and armor verification. Avoid a
   mandatory C dependency until packaging and security review is complete.
3. **PPF and optical topology.** Require fixtures for sector mode, multi-track,
   SBI/subchannel, CHD and conversion boundaries before offering it.
4. **Cross-adapter mod manifest.** Define which fields are universal and which
   belong to PCSX2, RPCS3, Dolphin, PPSSPP, Azahar, Ryujinx, or future
   adapters.
5. **Texture storage accounting.** Measure pack size, duplicate texture keys,
   compression, and launch-time memory cost before offering automated
   enablement.

### ALREADY COVERED

- Local package inspection and identity-gated preview
- Path/archive safety
- Shared staged apply, backup, journaling and rollback
- PCSX2 PNACH workflows
- Dolphin code and texture-pack workflows
- RetroArch cheat source and installation workflows
- Launch planning and typed launch resource grants as future integration
- Evidence-based identity and topology infrastructure

### NOT SUITABLE / REJECT

- A universal installer that treats ROM patches, PNACH, texture folders,
  LayeredFS and cheats as the same operation
- Filename-only game matching
- In-place modification of original ROMs or disc images
- Silent patch application or automatic enablement
- Arbitrary shell scripts, installers, hooks, or downloaded executables
- Blind extraction into emulator directories
- Full prepatched game downloads or redistribution
- Scraping sites as the primary architecture
- Guessing patch load order or using lexical order as a semantic rule
- Making an unselected mod profile create a Ready-to-Play failure
- Treating a successful emulator launch as proof that a mod is correct

## Direct answers

1. **What should “Mods” mean?** A curated umbrella for persistent content,
   patches, replacement assets, runtime emulator patches, and loader content,
   with explicit subtypes and adapter ownership.
2. **Should Cheats and Mods merge?** No. Share provenance, identity and
   transaction infrastructure, but keep runtime code databases separate from
   content/asset modifications.
3. **Should EmuWiz apply ROM patches?** Eventually, but only after exact base
   verification, bounded parsing, new-output creation, output verification and
   derivative registration. BPS is the strongest first candidate.
4. **Should the original remain?** Always, by default. A derived output is a
   new content object, not a replacement.
5. **What is the first real feature?** Local import inspection and normalized
   manifest preview, followed by BPS/IPS/UPS derived-output work in a separate
   implementation phase.
6. **Which emulator mod systems are safest to model first?** Existing Dolphin
   texture packs and PCSX2/RPCS3 identity-keyed patch metadata, because their
   identity keys and current EmuWiz adapters are already visible. Their
   mutation semantics still require adapter review.
7. **What about texture packs?** Model them as identity-keyed, potentially
   huge, read-only assets with explicit enablement and conflict detection; do
   not copy a pack into a live emulator profile automatically.
8. **What about CHD and multi-disc?** Refuse ambiguous patch targets. Require
   the patch's declared topology and keep conversion separate.
9. **What should remote discovery do?** Provide optional metadata and source
   links first. Do not download or redistribute content by default.
10. **How should future launch integration work?** The selected game plus an
    explicitly selected mod profile can contribute verified adapter resources
    to `LaunchResourceGrantSet`; it must not bypass the launch planner or
    create a second readiness authority.

## Sources

Repository sources are listed above with exact paths and function names. Key
external primary sources, accessed 2026-09-14, are:

1. [PCSX2: Writing Patches](https://pcsx2.net/docs/advanced/writing-patches/)
2. [PCSX2 patch repository](https://github.com/PCSX2/pcsx2_patches)
3. [RPCS3: Game Patches](https://github.com/RPCS3/rpcs3/wiki/Game-Patches)
4. [RPCS3 patch implementation](https://github.com/RPCS3/rpcs3/blob/master/Utilities/bin_patch.cpp)
5. [Dolphin Resource Pack Specification v2](https://github.com/dolphin-emu/dolphin/blob/master/docs/ResourcePacks.md)
6. [PPSSPP: creating texture replacement packs](https://www.ppsspp.org/docs/reference/texture-replacement/)
7. [PPSSPP: using texture replacement packs](https://dev.ppsspp.org/docs/reference/use-texture-replacement/)
8. [Flips patcher and format implementation](https://github.com/Alcaro/Flips)
9. [xdelta3/VCDIFF project and license](https://github.com/jmacd/xdelta)
10. [Rust `ips` crate](https://docs.rs/ips/latest/ips/)
11. [Rust `ups` crate](https://docs.rs/ups/latest/ups/struct.UpsPatch.html)
12. [Rust `xdelta3` bindings](https://docs.rs/xdelta3/latest/xdelta3/)
13. [rompatch-rs format inventory](https://github.com/GregTheGreek/rompatch-rs)

## Licensing note

EmuWiz should independently implement documented patch-format behavior or
evaluate a compatible dependency rather than copy source from GPL projects.
Flips is GPL-3.0; incorporating its source would require complying with that
license for the resulting covered distribution. Current xdelta 3.2.x source is
Apache-2.0 according to its repository, while bindings and other crates carry
their own licenses and transitive obligations. Each dependency must be
audited from its exact release, including build-time libraries and bundled
code, before redistribution. Format ideas, signatures and documented
behavior are not a license to copy implementation code.

The preferred boundary is therefore: reuse EmuWiz's existing safety,
identity, preview, transaction and provenance concepts; independently
implement or separately license narrowly reviewed format readers; and never
bundle copyrighted game content or proprietary mod assets without clear
redistribution rights.
