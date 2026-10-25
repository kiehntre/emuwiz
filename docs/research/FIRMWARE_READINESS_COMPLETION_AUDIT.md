# BIOS / Firmware Readiness Completion Audit

**Status:** implementation-ready audit; research only.

**Starting point audited:** `33ecab378c54cfdfa4dd0303e1008b114b920878`.

**Question:** can EmuWiz reliably answer whether the selected game has the firmware required by the selected emulator/profile?

## CURRENT ARCHITECTURE

EmuWiz currently has three related but distinct layers:

1. **Adapter-specific inspection.** DuckStation, PCSX2, RPCS3, xemu, Flycast, Hatari, PC Engine CD, RetroArch core metadata, and other adapters own their detailed firmware/system-file observations.
2. **Shared launch projection.** [`launch/readiness.rs`](../../crates/archivefs-core/src/launch/readiness.rs:1) maps those adapter states into `FirmwareReadiness`: `Verified`, `PresentUnverified`, `Missing`, `Unknown`, and `NotRequired`.
3. **Global BIOS projection/setup.** [`bios_projection.rs`](../../crates/archivefs-core/src/bios_projection.rs:1) inventories a bounded master firmware tree and plans read-only/link/copy/external actions. It explicitly does not copy, link, or change emulator configuration while planning.

The launch planner then applies firmware to a concrete `LaunchCandidate`. `Verified` and `NotRequired` do not block; `PresentUnverified` warns; `Missing` creates `RequiredFirmwareMissing`; `Unknown` currently remains honest uncertainty without automatically blocking or warning ([`launch/planning.rs`](../../crates/archivefs-core/src/launch/planning.rs:256)).

The GUI already renders a firmware summary beside a launch candidate: “Ready” means recognized by hash, “Found, not verified” is a warning, “Required firmware missing” blocks, “Firmware needs attention” is pending/unknown, and “Not required” is explicit ([`launch_readiness_page.rs`](../../crates/archivefs-gui/src/launch_readiness_page.rs:2170)). The GUI also states that EmuWiz verifies firmware but does not supply BIOS/system ROM files.

**Overall classification:** shared vocabulary and several adapter paths are **COMPLETE/PARTIAL**; uniform game + selected emulator/profile binding is **PARTIAL**; global BIOS projection is **GLOBAL-ONLY** unless joined to a concrete launch candidate; a single reusable game-specific firmware projection consumed by all surfaces is **MISSING as a formal seam**.

## PLATFORM MATRIX

This matrix reports only requirements represented in current code.

| Platform/system | Current representation | Classification | Completion note |
|---|---|---|---|
| PlayStation / DuckStation | DuckStation BIOS inspection, profile roots, firmware evidence, shared readiness | **PARTIAL → GAME-SPECIFIC at launch** | Exact verification is available through the firmware evidence path; global BIOS projection also has filename-oriented requirements. |
| PlayStation 2 / PCSX2 | PS2 BIOS evidence and fresh preflight hash verification | **COMPLETE for the current native PS2 launch slice** | Candidate/profile/firmware evidence is passed into launch preflight; missing or hash mismatch blocks. |
| PlayStation 3 / RPCS3 | Dev-flash firmware status, version presence, firmware unavailable blocker | **PARTIAL** | Current status distinguishes present/missing/unknown but is not a full exact firmware hash/revision requirement for each game. |
| PSP / PPSSPP | Shared readiness is constant `NotRequired` | **COMPLETE** | Code explicitly models no conventional external BIOS requirement. |
| Sega Saturn | No dedicated Saturn firmware requirement found in the audited launch/readiness model | **UNKNOWN / NOT PROVEN REQUIRED** | Do not invent a BIOS requirement; platform-specific optical identity is separate from emulator firmware. |
| Dreamcast / Flycast | Dreamcast BIOS/flash/system-file state and verified/present/missing/unknown projection | **PARTIAL → GAME/PROFILE candidate** | The selected Flycast candidate receives firmware state, but exact per-game region/revision choice is not a universal model. |
| Sega CD | No dedicated Sega CD firmware adapter found; RetroArch core metadata may report a core firmware need | **GLOBAL/CORE-DEPENDENT** | Only a selected core’s proven requirement may affect readiness. |
| PC Engine CD | System-card inspection and `PceCdFirmwareReadiness`, including emulator-provided firmware | **PARTIAL / STRONG SPECIALIST PATH** | Correctly distinguishes emulator-provided, verified sufficient, hash-unknown, no verified firmware, and unknown requirement. |
| 3DO | No dedicated firmware requirement found in the audited shared model | **UNKNOWN / NOT PROVEN REQUIRED** | Do not treat a generic BIOS directory entry as a 3DO per-game requirement. |
| Neo Geo | No dedicated firmware requirement found; arcade/core dependencies may be represented externally | **GLOBAL/CORE-DEPENDENT** | MAME/RetroArch dependency evidence must remain distinct from generic BIOS readiness. |
| Arcade/MAME | BIOS/device ROM dependencies in MAME collection/romset model | **PARTIAL, NOT GENERIC BIOS** | MAME dependencies are set/parent/device-specific; current BIOS projection intentionally leaves them to the arcade dependency model. |
| Nintendo DS / melonDS | Current launch adapters model `NotRequired` in the audited path; no generic DS BIOS requirement was found | **PARTIAL / ADAPTER-SPECIFIC** | Only claim NotRequired for adapters whose current launch contract proves it. |
| 3DS / Azahar | Bios projection labels external system data/keys; native launch readiness is separate | **EXTERNAL / PARTIAL** | Do not claim a verified BIOS file; report external system-data/key readiness only when the selected profile exposes it. |
| Game Boy Advance / mGBA | Configured BIOS is optional evidence; mGBA module explicitly avoids making it required | **COMPLETE for no-required-BIOS policy** | A user BIOS may improve fidelity but must not block a launch path that uses built-in behavior. |
| GameCube/Wii / Dolphin | BIOS projection knows GameCube IPL and writable SYSCONF; current Dolphin launch comments model no BIOS/firmware requirement for the supported slice | **AMBIGUOUS ACROSS MODES** | Do not turn global IPL inventory into a per-game blocker without a selected-mode requirement. |
| Wii U / Cemu | System/state inventory knows Cemu but no complete firmware requirement projection was found | **GLOBAL/EXTERNAL / MISSING GAME BINDING** | Preserve external system-data limitations; no generic BIOS claim. |
| Xbox / xemu | MCPX, flash BIOS, EEPROM, HDD system-file states | **PARTIAL** | Four-file readiness exists; firmware versus writable state is distinguished, but per-game requirement binding is profile/system-level. |
| Xbox 360 / Xenia | Shared readiness constant `NotRequired` in the audited model | **ADAPTER-SPECIFIC COMPLETE** | Do not infer Xbox-style BIOS requirements for Xenia. |
| Amiga / FS-UAE | Kickstart candidates and profile inspection | **PARTIAL** | Filename/presence requirements exist; exact hash verification and game/profile binding vary by path. |
| Atari ST / Hatari | TOS health, configured image, verified/present/missing/unreadable | **PARTIAL** | Shared projection exists; TOS version/region compatibility is not uniformly game-bound. |
| PC-98 and supported computers | No general firmware requirement model found beyond adapter-specific cases | **MISSING/UNKNOWN** | Remain unknown or adapter-owned; never claim NotRequired by absence of a record. |

## REQUIREMENT STATES

The current common states are `Verified`, `PresentUnverified`, `Missing`, `Unknown`, and `NotRequired`. The requested product states map as follows:

| Product state | Existing source | Presentation rule |
|---|---|---|
| `NOT_REQUIRED` | `FirmwareReadiness::NotRequired`, including PPSSPP and emulator-provided PC Engine CD firmware | Say “Not required” only for the selected launch candidate. |
| `READY` | `FirmwareReadiness::Verified` | Requires the adapter’s trusted evidence, normally exact hash or equivalent verified state. |
| `MISSING` | `FirmwareReadiness::Missing` plus required-firmware blocker | Hard block only when that candidate requires firmware. |
| `WRONG_REGION` | Not a shared common state in current code | Must remain adapter-specific/unknown until a candidate exposes region mismatch. |
| `WRONG_VERSION` | Usually adapter detail/version status, not common state | Do not collapse into Missing; expose as a typed adapter issue when proven. |
| `HASH_MISMATCH` | Adapter-specific evidence or BIOS match status | Must map to blocked or warning according to candidate policy; never to Ready. |
| `UNKNOWN` | `FirmwareReadiness::Unknown` | Show “Needs attention / cannot verify”; do not silently claim missing or ready. |
| `OPTIONAL` | Adapter-specific optional BIOS/config evidence, e.g. mGBA | Show as optional enhancement, never as a required launch blocker. |
| `EMULATOR_MANAGED` | Emulator-provided/HLE/external-system-data paths | Show who owns the requirement and whether the selected profile reported it ready. |

The shared enum is intentionally coarser than adapter state. The completion work should preserve detailed adapter evidence rather than expand the common enum with claims that some adapters cannot prove.

## GAME + EMULATOR BINDING

The required truth is:

```text
selected GameId/release identity
  + platform/region/revision
  + selected emulator/profile/core
  + adapter-declared firmware requirement
  + actual firmware evidence for that profile
  = game-specific firmware readiness
```

Current strengths:

- launch candidates carry firmware readiness;
- candidate readiness participates in launch blockers/warnings;
- PCSX2 and selected specialist paths pass real firmware evidence into preflight;
- selected Game Details starts relevant profile/firmware checks;
- profile changes can rebuild the launch input.

Current gaps:

- the global BIOS projection is not itself keyed to GameId;
- `BiosRequirement` is primarily emulator/name/expected-filename/target based and often has no expected SHA-256;
- many platforms have no adapter-specific per-game requirement;
- firmware state may be “present” or “filename only” without a version/region relationship to the selected game;
- GUI Doctor/BIOS pages can show global readiness that is not sufficient to claim selected-game readiness;
- the shared `Unknown` path is intentionally non-blocking in launch planning, so the UI must not phrase it as verified.

**Completion rule:** only a concrete `LaunchCandidate` may answer “firmware ready for this game”. A global BIOS inventory can supply evidence, but cannot independently answer the question.

## IDENTITY

Evidence strength should be ordered:

1. exact authoritative firmware hash matched to the adapter requirement;
2. exact emulator-reported identity/version where the adapter documents that it is authoritative;
3. known filename plus verified size/signature/hash if the adapter’s rules accept it;
4. filename and expected directory only;
5. filename-only or presence-only.

Current code correctly labels filename/presence-only evidence as `PresentUnverified` or `FilenameOnly` in the BIOS projection. However, several requirement tables use expected filenames without populated expected SHA-256, so they remain discovery hints rather than exact verification.

The firmware identity must remain separate from game identity. A matching BIOS filename does not verify the game release, and a verified game does not make an unverified BIOS acceptable.

## REGION / REVISION

Region/revision is currently strong in some adapter-specific firmware paths but not a uniform cross-platform contract. PCSX2/PS1/Dreamcast/PC Engine CD and arcade systems can have meaningful region or system-card choices; absence of a common field is not proof of compatibility.

Rules for completion:

- if the selected candidate declares a region/version requirement and evidence disagrees, use typed `WrongRegion`/`WrongVersion` adapter detail and block or warn according to existing launch policy;
- if the candidate does not declare such a requirement, do not invent one from the game title or filename;
- if multiple valid BIOS revisions exist, preserve the candidate’s accepted set and show the selected match, not an arbitrary global winner;
- do not treat region metadata from an artwork/provider record as firmware evidence.

## MULTIPLE VALID OPTIONS

Systems may legitimately accept multiple BIOSes, regions, revisions, or HLE/real-firmware modes. The model should represent:

```rust
struct FirmwareRequirementProjection {
    selected_emulator: EmulatorId,
    selected_profile: ProfileId,
    accepted_options: Vec<FirmwareOption>,
    selected_option: Option<FirmwareOption>,
    status: FirmwareRequirementState,
    choice_required: bool,
    evidence: Vec<FirmwareEvidenceRef>,
}
```

Choice is required when:

- multiple options are valid but produce materially different behavior;
- region/revision is game-sensitive;
- one candidate uses HLE and another uses external firmware;
- emulator configuration has multiple profiles with different system roots.

Choice is not required when the selected profile has one verified accepted option or the emulator explicitly provides the same requirement internally.

## HLE / BUILT-IN FIRMWARE

Current code already has honest examples:

- PPSSPP projects to `NotRequired` because the current PSP launch slice does not require a conventional BIOS;
- PC Engine CD can report `EmulatorProvidesFirmware`;
- mGBA treats configured BIOS as optional evidence;
- Xenia is represented as `NotRequired` in the shared readiness projection;
- Azahar/Ryubing are represented as external system data/keys rather than a generic BIOS file.

The UI must say “Not required for this emulator” or “Provided by the emulator” rather than “Firmware missing”. HLE is not automatically equivalent to verified original firmware; if fidelity/compatibility differs, that belongs in a warning/advanced detail supplied by the adapter.

## DISCOVERY

Global BIOS projection:

- bounds depth, entries, and file/hash bytes;
- distinguishes immutable firmware from writable state;
- records filename, size, optional SHA-256, source, status, platform, and warnings;
- detects missing, ambiguous, hash mismatch, and filename-only matches;
- deliberately treats MAME BIOS/device dependencies as an arcade dependency model rather than a generic BIOS folder;
- plans links/copies/external actions but does not mutate configuration or files by inspection.

Adapter-specific discovery is stronger in selected cases:

- PCSX2 performs strict evidence-backed BIOS verification and fresh preflight rechecks;
- DuckStation has a dedicated firmware verifier and narrower readiness projection;
- Flycast reads Dreamcast BIOS/flash/system states and can hash trusted firmware;
- Hatari evaluates TOS health;
- RPCS3 identifies installed firmware/version presence;
- xemu checks MCPX/flash/EEPROM/HDD states;
- PC Engine CD inspects system-card structure and hashes;
- RetroArch uses core metadata, but core-level firmware requirements remain dependent on the selected core/profile.

## CONFIG OWNERSHIP

Current policy is appropriately least-invasive:

- BIOS projection is read-only planning by default;
- immutable firmware may be represented by direct path or link plans where an existing explicit apply workflow supports it;
- writable state is kept local and is not linked from an immutable master store;
- RPCS3 firmware and Azahar/Ryubing external data are emulator-managed/external-install categories;
- MAME dependencies remain in the set/rompath model;
- GUI guidance says EmuWiz verifies but does not supply copyrighted firmware.

The game-specific readiness completion should prefer verification of the emulator’s existing configured path. It must not silently copy firmware into emulator folders, rewrite emulator configuration, or change profile selection. A future explicit projection action must remain separate from “is this game ready?”.

## MISSING FIRMWARE UX

For a selected candidate, show:

```text
This system needs firmware before this game can start.

Required by: PCSX2
Status: Missing
Expected: Verified PS2 BIOS evidence

[Check BIOS / Firmware]
```

Other states:

- “Firmware: Ready”;
- “Firmware: Found, but not verified”;
- “Firmware: Not required for this emulator”;
- “Firmware: We could not verify this yet”;
- “Firmware: The selected profile uses a different region/version”.

Allow local selection/configured-directory verification only where current architecture already supports it. Do not provide copyrighted firmware downloads or external links that imply redistribution.

## LEGAL POLICY

Allowed workflows:

- user-provided firmware dump;
- verification against local/authoritative identity data already supported by EmuWiz;
- user-selected configured firmware directory;
- emulator-managed legal/system-data installation where the emulator owns the operation;
- open replacement firmware only when licensing is independently proven.

Refusals:

- no firmware bundling or scraping;
- no automatic BIOS downloads;
- no upload of firmware bytes or hashes to providers without explicit future provider design;
- no silent copying into emulator directories;
- no claim that a filename-only file is verified.

## READINESS INTEGRATION

Define one reusable projection consumed by Game Details, the First Verified Game journey, Emulator Setup, Problems & Repair, and launch preflight:

```rust
struct GameFirmwareReadiness {
    game_id: GameId,
    platform: PlatformId,
    release_identity: Option<VerifiedIdentityRef>,
    emulator: EmulatorId,
    profile: ProfileId,
    requirement: FirmwareRequirementState,
    selected_option: Option<FirmwareOptionSummary>,
    evidence: Vec<FirmwareEvidenceSummary>,
    action: FirmwareAction,
    freshness: ReadinessFreshness,
}
```

This is a projection, not a new source of truth. It should be built from the selected `LaunchCandidate` plus adapter detail and current profile/evidence generation. All consumers must refresh it after:

- game identity/revision changes;
- emulator/profile/core changes;
- firmware file addition/removal/replacement;
- firmware directory/config changes;
- provider/DAT evidence refresh where the adapter relies on it.

## P0 GAPS

P0 means EmuWiz cannot reliably answer the selected-game/selected-emulator question for the affected path.

1. **No formal shared GameId + profile + requirement + actual evidence projection.** Launch candidates contain the final status, but global BIOS/Doctor/Game Details consumers do not share a named reusable projection.
2. **Unknown firmware is not uniformly actionable.** Current launch planning does not block or warn on `Unknown`; this is honest for some adapters but insufficiently explicit for a user asking whether the game is ready.
3. **Several requirement tables are filename-oriented and omit expected hashes.** Those paths cannot claim exact firmware identity.
4. **Region/version mismatch is not a common typed result.** Affected adapters need adapter-owned mismatch evidence before a generic Ready answer is possible.
5. **Multiple valid BIOS options are not uniformly represented as an explicit selected option/choice.** Global inventory ambiguity must not be silently resolved.
6. **Global BIOS projection can be mistaken for game readiness.** The UI needs a hard boundary: only a selected launch candidate answers the game-specific question.

## P1 GAPS

1. Expand adapter-specific evidence into a consistent advanced detail contract without flattening semantics.
2. Make profile/config source and firmware evidence generation visible for freshness checks.
3. Improve per-platform coverage for systems currently represented only through RetroArch core metadata or external data.
4. Distinguish `PresentUnverified`, `HashMismatch`, `WrongRegion`, `WrongVersion`, and `Unknown` in GUI copy where adapter evidence supports it.
5. Link BIOS/Firmware and Doctor actions back to the selected GameId/profile context.
6. Document HLE/built-in behavior for each adapter that supports it rather than inferring from absence.

## QUICK WINS

- Make Game Details always display the selected candidate’s existing firmware status, not the global BIOS page status.
- Keep “Not required” visible for PPSSPP, Xenia, emulator-provided PC Engine CD, and other proven adapters.
- Show “Found, not verified” distinctly from Ready everywhere.
- Add a plain-language “Required by: [emulator/profile]” line to missing-firmware blockers.
- Keep exact hashes, expected filenames, regions, versions, and paths under Advanced details.
- Treat `Unknown` as “We could not verify this yet” in the UI even where launch policy currently permits it.
- Preserve the existing no-download/no-redistribution copy.

## IMPLEMENTATION SEAMS

Narrow reusable seams for future work:

1. **GameFirmwareReadiness projection** built from one selected `LaunchCandidate` and adapter detail.
2. **FirmwareRequirementDescriptor** containing accepted hashes/options, region/version constraints, HLE policy, and requirement ownership.
3. **FirmwareEvidenceSummary** retaining source path, hash status, filename, size, provider/catalogue source, and freshness.
4. **Profile binding** carrying selected emulator/profile/core and the firmware root/config source used for the candidate.
5. **Explicit option selection** for multiple legitimate BIOSes, without arbitrary ranking.
6. **GUI adapter** shared by Game Details, First Verified Game, Emulator Setup, Problems, and launch readiness.
7. **Freshness invalidation** on file/profile/config/evidence changes.

Do not create a second BIOS scanner, global firmware database, or per-screen readiness enum.

## TEST PLAN

Synthetic fixtures and adapter projection tests should cover:

1. firmware not required;
2. exact valid firmware hash;
3. missing required firmware;
4. wrong hash with matching filename;
5. wrong region;
6. wrong revision/version;
7. multiple valid BIOS options;
8. filename match with hash mismatch;
9. emulator HLE/built-in firmware;
10. emulator-specific requirement;
11. profile change invalidates the projection;
12. firmware removed after readiness becomes stale/non-ready;
13. duplicate files with identical hashes;
14. duplicate filename with different hashes;
15. user-selected local firmware path;
16. stale emulator config path;
17. MAME parent/BIOS/device dependency remains set-scoped;
18. RetroArch core requirement follows selected core, not platform alone;
19. no network activity or firmware upload;
20. global BIOS Ready does not make an unrelated selected game Ready;
21. Game Details, Setup, Problems, and launch preflight receive identical projection facts;
22. HLE/NotRequired does not show a false missing-firmware blocker.

## IMPLEMENTATION ORDER

1. Define the projection contract around existing `LaunchCandidate` and adapter-specific evidence.
2. Add freshness/profile binding and selected-option fields without changing current adapter enums.
3. Normalize UI copy for Verified, PresentUnverified, Missing, Unknown, NotRequired, and adapter-supported mismatch states.
4. Surface the projection beside Play in Game Details and in the First Verified Game journey.
5. Pass the same projection/context to Emulator Setup, Problems & Repair, and launch preflight.
6. Add explicit multiple-option handling for one carefully selected adapter family.
7. Expand exact hash/region/version evidence adapter by adapter; keep unsupported systems Unknown.
8. Add focused tests for stale profile/file/config changes and global-versus-game readiness separation.

## CONCLUSION

EmuWiz already has the core of safe firmware readiness: adapter-specific inspection, a shared candidate-level vocabulary, exact verification in selected paths, HLE/NotRequired handling, and legal-safe no-download policy. Completion requires making the selected launch candidate the sole game-specific authority, preserving adapter detail, and exposing freshness, region/version, and multiple-option uncertainty instead of deriving readiness from a global BIOS catalogue.
