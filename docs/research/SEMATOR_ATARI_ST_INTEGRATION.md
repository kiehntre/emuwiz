# SemaTor Atari ST enhanced-runner research

Research date: 2026-09-20<br>
Authoritative EmuWiz source: `/home/davedap/emuwiz-main-release-fix`<br>
Research branch: `research/semator-atari-st`<br>
Scope: research only; no production Rust, GUI, library media, or SemaTor files were changed.

## Executive conclusion

SemaTor is a credible specialist enhanced Atari ST runtime, but it is not a
general Atari ST emulator replacement. The current public preview is a private,
compiled translation layer with a small, edition-sensitive title catalogue. It
has a usable non-GUI Linux launch surface, but it does not expose a public
profile-selection API or a machine-readable compatibility database.

An EmuWiz integration is therefore feasible only as a conservative optional
runner:

```text
verified Atari ST image + exact SemaTor edition evidence
    -> visible Enhanced launch
otherwise
    -> normal Hatari/Steem launch only, or Needs Review
```

The current evidence supports no automatic enhanced launch for the local
collection. A bounded existing catalogue inspection found seven title-level
matches, but not the exact SemaTor edition/hash proof required to promote any
of them safely.

## Upstream and project state

The authoritative upstream is [SirBaron/SemaTor on GitHub](https://github.com/SirBaron/SemaTor).
The repository description calls it an “Atari ST Translation Layer for Modern
Systems”. It contains the website, public documentation, issue tracker and
licence; the README explicitly says the source is private. The current public
preview observed was **1.1.230**, released on 2026-09-20, with Linux x86-64 and
Windows x64 desktop packages. The repository was created on 2026-09-19 and the
visible releases/builds are a rapid public-preview series (1.1.222, 1.1.226,
1.1.227 and 1.1.230), not an established long-term release cadence.

The public release provides no commercial game disks and no Atari TOS ROM.
SemaTor states that no TOS ROM is required.

### Licence

SemaTor's own [LICENSE](https://github.com/SirBaron/SemaTor/blob/main/LICENSE)
is not an open-source licence: released binaries may be downloaded, installed
and used, but modified redistribution, disassembly/decompilation/reverse
engineering for reuse, bundling in something sold, and resale are prohibited
without written permission. The release lists separate third-party notices for
SDL2, Vulkan headers, stb_image and other components. EmuWiz must not bundle,
modify, reverse engineer, or redistribute SemaTor. A future adapter should
launch a user-installed binary and keep installation provenance explicit.

### Platforms and packaging

| Area | Current evidence |
|---|---|
| Linux | Public x86-64 ZIP; requires system SDL2 runtime. Install script also requires Python 3. |
| Windows | Public x64 ZIP; `SemaTor.exe` with `SDL2.dll` beside it. |
| AppImage | No public AppImage observed. |
| Flatpak | No public Flatpak observed. |
| Source | Not public; source is private. |
| Android/AYN Thor | Development/test work only, not in desktop downloads; not an EmuWiz desktop target. |

The Linux ZIP contains `semator-linux-x86_64`, `install.sh`, `Media/` and empty
`Games/` and `User/` roots. The default install location is `~/Games/SemaTor`.
The installer creates an application launcher and preserves Games, Media and
User data during updates.

### Update mechanism and configuration locations

The public README documents an in-app update button and
`Library Settings -> Updates`. The release bundle includes `Media/semator-update`
and signed `.supdate` assets on GitHub. Automatic in-app installation requires
a manually installed 1.1.219-or-newer public build. GitHub release downloads
remain available. This is a vendor update path, not an EmuWiz profile-feed API.

Installed Linux layout:

```text
~/Games/SemaTor/
  semator-linux-x86_64
  run-semator.sh
  Games/                  # disks / configured collection roots
  Media/                  # runtime assets and artwork
  User/
    settings/
    saves/
    logs/
    data/
    display-calibration/
    zip-cache/
    music-cache/
```

The installer also uses XDG data/config locations for desktop integration and
records an installation manifest under the XDG data area. `SEMATOR_DATA_DIR`
can select the User directory in the generated launcher. The release strings
show binary catalogue caches named `User/settings/launcher-catalogue-*.bin`;
these are implementation/cache artifacts, not a supported profile interchange
format.

## How SemaTor works

The primary description is explicit: SemaTor runs 68000 game code, reproduces
the hardware and operating-system interfaces that code needs, and adds native
enhancements for supported titles. That is a translation-layer architecture,
not a conventional full Atari computer emulator. It is not equivalent to
Hatari/Steem's broad machine emulation model and must not replace those runners.

The public material verifies:

- no Atari TOS ROM is required;
- guest 68000 code is run through SemaTor's translation/runtime path;
- hardware and OS interfaces are implemented as needed by the supported game;
- compatibility is title/edition-specific, because translation and native
  replacements depend on known game code, disk layout, timing, graphics and
  audio behavior.

The public source does not disclose the translator, CPU implementation,
instruction coverage, or complete hardware model. Claims beyond the points
above would be speculation. Unsupported or unrecognised disks may be visible
in the library, but the runtime can reject them (“cannot start this disk”,
“cannot load the game program”, or “no supported startup program or bootable
loader”). A recognised but untested edition may use a title profile with an
on-screen warning; the guide expressly says a matching filename does not verify
an edition.

## Supported games, profiles, and enhancements

The [current enhanced-game catalogue](https://sirbaron.github.io/SemaTor/games/)
lists **11 titles across 14 edition profiles**:

1. Arkanoid II: Revenge of Doh
2. Black Lamp
3. Mega lo Mania
4. Return to Genesis
5. SWIV
6. Time Bandit
7. Turrican II
8. Xenon
9. Xenon 2: Megablast
10. Zak McKracken
11. Zynaps

The catalogue is HTML guide material, not a downloadable profile manifest.
Edition pages contain human-readable controls, options, warnings and edition
descriptions. The private runtime/profile data is compiled into the binary or
private build payload; no public JSON/YAML/CSV profile format or profile ID
contract was found.

Enhancement classification from the current guides and release binary:

| Capability | Classification | Evidence/limit |
|---|---|---|
| 16:9 / 21:9 / widescreen | Profile-controlled, game/edition-specific | Listed per game; scenery coordinate mapping and borders vary by title. |
| Ultrawide | Profile-controlled, game-specific | 21:9 is listed for selected titles, not a universal stretch. |
| Slowdown fixes / faster drawing | Profile-controlled native replacements | Guides describe verified sprite/scenery loops and guarded drawing clocks. |
| Game speed/timing | User-controlled plus profile options | Global game-speed control exists; title timing options preserve or alter selected behavior. |
| Frame presentation | User-controlled/global | CRT/presentation controls and optional frame generation; frame generation requires Vulkan and depends on GPU/driver. |
| Controls | Profile defaults plus user-controlled remapping | Per-game keyboard/gamepad mappings; one active host gamepad is documented. |
| Music/SFX/speech | Global volumes plus profile/edition options | Separate players and clean/stereo/test options exist only for applicable editions. |
| Visual effects | Global and per-game | CRT, bezel, crop and display settings are global/display settings; some game picture modes are title-specific. |
| Patching/native replacement | Profile-controlled and compiled | Public strings describe replacing known routines with native ones; no patch-file format is published. |
| Cheats/practice/save/rewind | Profile/user controls where available | Public guides and binary strings show title-specific cheats, checkpoints, saves and rewind. |

“Automatic” means selected by recognised title/edition profile or normal runtime
defaults. “User-controlled” means exposed in F12/game settings. The public
material does not identify a separately user-editable profile file.

The public documentation strongly implies virtual runtime behavior: native
replacements, host-drawn pointers, host audio players, and reading speech from
the mounted disk image while retaining original protection reads. No workflow
modifies the original image. Because the source is private, EmuWiz should state
this as observed/publicly documented behavior, not promise a byte-for-byte
proof of every enhancement implementation.

## Identity matching: the critical blocker

The public repository and release were searched for CRC, SHA, TOSEC, No-Intro,
filename-only identity, profile IDs, and a public profile schema. No public
SemaTor hash table or stable profile-ID contract was found.

What is observable:

- library scanning accepts disk extensions and presents title/profile
  recognition internally;
- the guide says supported editions matter and says a filename alone does not
  verify an edition;
- recognised untested editions can receive a warning rather than a clean
  certification;
- there is no CLI option for selecting a profile or overriding identity;
- private binary strings include `TITLE PROFILE / VERIFY ON LAUNCH` and cached
  catalogue filenames, but those do not expose a supported external identity
  protocol.

Therefore the available identity quality is:

| Evidence | EmuWiz decision |
|---|---|
| Exact SemaTor-published image hash/structured identity, if the vendor ever publishes one | Verified; offer enhanced launch. |
| A future signed vendor manifest that binds image bytes, disk/member identity, edition, profile ID and required runtime version | Verified after signature/schema validation. |
| Existing EmuWiz DAT/TOSEC/No-Intro match with no SemaTor hash/edition crosswalk | Candidate only; Needs Review. |
| Filename, normalized title, internal label, or RetroArch CRC alone | Candidate only; never auto-offer. |
| Fuzzy title match, compilation, trainer/crack/translation/repack, or unknown disk | Do not offer enhanced launch. |

The preferred future contract is exact bytes or a vendor-supplied exact
identity crosswalk. For multi-disk games, every required disk/member must be
bound to the profile, not just disk 1's title. EmuWiz can retain its existing
read-only identity evidence and add a SemaTor-specific `Needs Review` state,
but must not infer a SemaTor profile from a game name.

## Media formats and preservation

The current README explicitly lists **ST, IMG, MSA, full-sector DIM, STX, and
supported disk images inside ZIP archives**. Multi-disk/companion-disk handling
is documented in the 1.1.226/1.1.230 release notes and in the guides' disk
request behavior.

| Format | Current finding |
|---|---|
| ST | Documented and scanned. |
| MSA | Documented and scanned. |
| STX | Documented and scanned. |
| DIM | Full-sector DIM documented and scanned. |
| IMG | Documented as an accepted image. |
| ZIP | Supported when its disk image/member is supported; encrypted/unreadable members are reported by the runtime. |
| IPF | Not listed in the current collection-scan documentation. The Linux installer registers an IPF MIME type, but that is not proof the application can scan or launch IPF. Treat as unconfirmed/unsupported for an adapter. |
| Multi-disk | Supported operationally for recognised games through companion-disk matching and an in-game disk picker; exact edition membership still needs verification. |

The public workflow is user-owned, mounted disk images. The README and release
notes say original disks remain unchanged. Enhancement behavior is runtime
profile/native behavior; no public patch application or modified-image output
format exists. An EmuWiz adapter must pass the source path read-only and never
rewrite, normalize, unpack over, or patch the source by default. If a future
adapter needs a transformed working image, it must use an explicit disposable
copy and record that fact.

## CLI and adapter feasibility

Running the current Linux public binary with `--help` produced:

```text
SemaTor 1.1.230
Usage: semator-linux-x86_64 [disk] [--disk path] [--dir folder]
       [--renderer opengl|vulkan] [--safe] [--help]
```

This is sufficient for a non-GUI process launch. There is no documented
profile-selection flag, enhancement-selection flag, fullscreen flag, controller
mapping flag, save-path flag, or log-path flag.

Recommended future adapter shape:

```text
semator-linux-x86_64 --disk <verified-source-image>
```

Use `--dir <folder>` only when the whole SemaTor collection root is intentionally
managed. Use `SEMATOR_DATA_DIR=<private-per-game-or-run-user-dir>` only after
vendor compatibility testing; it is an observed launcher behavior, not a
promised public API. `--renderer opengl|vulkan` and `--safe` are optional
runtime controls, not profile selectors. Fullscreen/window selection is exposed
inside SemaTor settings, not on the CLI.

Controller handling is through SemaTor's SDL/gamepad settings and per-game
remaps. An adapter can launch the process and pass no synthetic GUI input; it
cannot safely select a game-specific mapping from the CLI. Exit behavior is
normal process termination after the session/library exits; no documented
machine-readable exit protocol was found. Logs live under User/logs and the
public build offers an optional local support report, but the report is not a
profile/identity API.

### Safe launch contract

An eventual adapter should require:

1. an installed, user-selected SemaTor executable whose version is recorded;
2. a regular source image opened read-only;
3. an exact EmuWiz identity record;
4. an exact SemaTor profile/edition crosswalk supplied by a signed or otherwise
   trusted data source;
5. a visible pre-launch summary of title, edition, disk set, runtime version,
   and enabled enhancements;
6. a fail-closed check immediately before process spawn.

If any step fails, leave normal Hatari/Steem launch available and do not silently
route to SemaTor. Enhanced and normal sessions should have distinct mode labels
in activity/history.

## Compatibility data and updates

The supported catalogue is currently human-readable HTML plus private compiled
runtime data. It is not a safe machine-readable source for EmuWiz to scrape.
The public GitHub release API is suitable for discovering vendor binary updates,
but not for deriving exact profile identity. Do not scrape around anti-bot or
website restrictions.

A safe future cache model would require a vendor-published, signed manifest with
at least:

```text
manifest_version
runtime_version_range
profile_id
title / edition / language / region
disk_set and ordered disk members
accepted format
exact image hashes (and, if needed, canonical sector/byte policy)
enhancement schema and defaults
source URL, release, signature, fetched_at, expires_at
```

EmuWiz should cache the last verified manifest, retain provenance and signature
status, refresh only from the official release/source endpoint, and never turn a
stale or unverifiable manifest into an automatic enhanced-launch offer.

## Local-library opportunity

The accessible existing catalogue evidence was inspected read-only and bounded:

- `/home/davedap/.config/retroarch/playlists/Atari - ST.lpl`
- 407 playlist entries were counted; disk files were not opened or hashed.
- The playlist contains title-level evidence for seven SemaTor names:
  **Black Lamp, Return to Genesis, Time Bandit, Turrican II, Xenon, Xenon 2:
  Megablast, and Zynaps**.
- No playlist evidence was found for Arkanoid II, Mega lo Mania, SWIV, or Zak
  McKracken.
- Several matches include TOSEC-like filename/release metadata and RetroArch
  CRC fields, but the playlist does not establish that those bytes, editions,
  disk sets, or CRC semantics match a SemaTor profile.

Opportunity estimate from this bounded evidence:

| State | Estimate | Reason |
|---|---:|---|
| Known SemaTor-supported titles locally | 7 | Existing playlist title/path evidence. |
| Strong SemaTor matches | 0 | No published SemaTor hash/profile crosswalk was available. |
| Needs Review | 7 | Candidates require exact edition/image evidence. |
| Not evidenced locally | 4 | No matching title in the existing playlist. |

This is a catalogue estimate, not a full filesystem scan. No image bytes were
hashed and no files were mutated.

## Blockers and recommendation

Blockers before implementation:

- no public source or open profile format;
- no public CRC/SHA/image identity table or stable profile-ID API;
- no CLI profile/enhancement selection;
- IPF support is not confirmed despite installer MIME registration;
- no documented machine-readable launch/exit/log contract;
- restrictive licence prohibits bundling/modification/reverse engineering;
- public preview cadence and compatibility coverage are still immature.

Recommendation: keep Hatari/Steem as the normal Atari ST path. Track SemaTor as
an explicit optional runner that can be offered only after an official exact
identity manifest or a manually verified local profile record exists. Surface
the selected enhancements before launch, keep source disks untouched, fail
closed on profile mismatch, and preserve normal launch for every title.

## Sources and evidence

- [SemaTor README](https://github.com/SirBaron/SemaTor/blob/main/README.md)
- [SemaTor licence](https://github.com/SirBaron/SemaTor/blob/main/LICENSE)
- [SemaTor changelog](https://github.com/SirBaron/SemaTor/blob/main/CHANGELOG.md)
- [SemaTor releases](https://github.com/SirBaron/SemaTor/releases)
- [Enhanced games overview](https://sirbaron.github.io/SemaTor/games/)
- [SemaTor repository metadata/API](https://api.github.com/repos/SirBaron/SemaTor)
- [Current public Linux release asset](https://github.com/SirBaron/SemaTor/releases/download/v1.1.230/SemaTor-1_1_230-linux-public-preview.zip)
- EmuWiz local evidence: `docs/SHARED_GAME_IDENTITY.md`,
  `docs/research/ATARI_FAMILY_SUPPORT_AUDIT.md`, Atari ST platform/media
  registry and the existing RetroArch Atari ST playlist noted above.

SEMATOR RESEARCH COMPLETE — EMUWIZ NOW KNOWS WHEN AN ATARI ST GAME CAN SAFELY OFFER ENHANCED LAUNCH
