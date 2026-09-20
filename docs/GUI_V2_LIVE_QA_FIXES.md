# GUI v2 live-QA regression audit

Starting commit: `76a516da1e4d8b457b385d72635c7949568ae413`.
Read-only catalogue/folder inspection: 20 September 2026. No main, catalogue,
configuration or original-media changes are part of this fix.

## Scrolling

Check Games rendered its entire platform list directly in CentralPanel. There
was no ScrollArea, so content below the viewport was clipped with no wheel
handler or usable scrollbar. It now has one bounded vertical scroller below
the header, outside the permanent sidebar. Platform list and selected-platform
views have independent stable scroll IDs. PageUp/PageDown/Home/End work even
with a focused button, without intercepting Alt+Home navigation. Tests exercise a
100-platform list, wheel input, final-row reachability, return navigation,
resizing and static sidebar placement.

## Why Arcade showed 34

The read-only database at `~/.local/share/archivefs/library.sqlite3` really has
34 current `Arcade` assignments. GUI v2 loads all archives through the existing
Database::load_archives LEFT JOIN to current platform_assignments, then applies
exact platform, search and attention filters. There is no artwork requirement,
source restriction, duplicate collapse or pagination limit in this projection.
Virtualization limits painting, not the logical result count.

All 34 assignments use `folder_alias`, source ID 22 (`/mnt/usbdrive/games`).
32 paths are under `arcade/`; two are FBNeo sample ZIPs under `bios/fbneo/samples/`.
The kinds are 30 direct_game_image, two sevenzip and two zip. Current on-disk
contents can differ from the saved catalogue. No current platform rows use
MAME or FBNeo as separate IDs. The backend Arcade ID/display name is `Arcade`;
its folder aliases are arcade, mame, fbneo, finalburnneo and fba. Those are
folder aliases, not additional catalogue platform IDs to merge at render time.

The read-only file listing of `/mnt/usbdrive/games/arcade` found 256,932 files
in 30,972 containing directories, including only two lowercase zip/7z/chd paths
and 22 lowercase img paths. Many files are extracted chip/ROM parts, not
independent game archives. This file count is not a verified game count.
Saved discovery run 195 contains these records beneath that folder:

| Saved classification | Records |
| --- | ---: |
| Unsupported extension | 174,410 |
| Missing paired file | 44,179 |
| Accepted discovery | 97 |
| Skipped, no skip_reason | 2 |

For example, loose `.bin` ROM parts were diagnosed as lacking a `.cue` sheet.
Discovery records and persisted browser archives are different projections;
97 accepted discovery records do not imply 97 persisted Arcade game rows.
Source 22's last recorded scan was successful at 2026-09-18T00:52:15Z.
These observations establish an ingestion/catalogue limitation, not a GUI
34-row cutoff. This focused change does not rewrite ingestion or misrepresent
loose parts as verified games. A scanner/arcade-set follow-up is needed to
represent that extracted collection fully; rescanning alone is not promised
to resolve unsupported parts.

The browser now says **catalogued games**, shows the exact platform total
before filters, explains the extracted-Arcade limitation, and links to folder
review/scanning. Advanced details shows exact ID/label, counts, search/health
filters and represented source roots/IDs. Changing platforms clears old search
and health filters. While async filtering is pending, an old count is not
presented as the new result. Separate platforms and real duplicate files remain.

### Catalogue counts

103,165 rows, 103,165 distinct absolute paths; 52 platform buckets including
Unknown system. Source 5 (`/mnt/games/roms`) has 13,405 rows; source 22
(`/mnt/usbdrive/games`) has 89,760. Other registered sources have no archive rows.

| Exact platform ID/display bucket | Rows |
| --- | ---: |
| Acorn Archimedes | 882 |
| Acorn Electron | 382 |
| Amiga | 1 |
| AmigaCD32 | 158 |
| Apple II | 339 |
| Arcade | 34 |
| Atari 8-bit | 630 |
| Atari Jaguar | 50 |
| Atari Lynx | 202 |
| Atari2600 | 712 |
| Atari5200 | 81 |
| Atari7800 | 58 |
| AtariST | 1,388 |
| BBC Micro | 454 |
| ColecoVision | 305 |
| Commodore 128 | 31 |
| Commodore 64 | 58 |
| Dreamcast | 27 |
| Enterprise | 1 |
| Game Boy | 5,715 |
| Game Boy Advance | 6,655 |
| Game Boy Color | 2,473 |
| GameCube | 6 |
| GameGear | 553 |
| MSX | 439 |
| MSX2 | 67 |
| MasterSystem | 320 |
| MegaDrive | 1,696 |
| N64 | 490 |
| NEC PC-8801 | 11,769 |
| NES | 387 |
| Neo Geo CD | 184 |
| Neo Geo Pocket Color | 106 |
| Nintendo DS | 821 |
| PC Engine | 285 |
| PC Engine CD | 230 |
| PS2 | 12 |
| PSP | 2 |
| PSX | 291 |
| Philips CD-i | 1 |
| PlayStation Vita | 2 |
| SNES | 8,522 |
| ScummVM | 6 |
| Sharp X68000 | 9,781 |
| TurboGrafx-16 | 428 |
| Unknown system (NULL assignment) | 5,986 |
| VIC-20 | 6 |
| Virtual Boy | 53 |
| Wii | 2 |
| Xbox | 32 |
| Xbox360 | 4 |
| ZX Spectrum | 40,048 |

Source split: platforms not listed below are entirely source 22.

| Platform | Source 5 | Source 22 |
| --- | ---: | ---: |
| Acorn Archimedes | 294 | 588 |
| BBC Micro | 146 | 308 |
| GameCube | 2 | 4 |
| GameGear | 22 | 531 |
| MegaDrive | 844 | 852 |
| NEC PC-8801 | 3,923 | 7,846 |
| PS2 | 4 | 8 |
| PSP | 1 | 1 |
| PlayStation Vita | 1 | 1 |
| SNES | 4,023 | 4,499 |
| Sharp X68000 | 3,376 | 6,405 |
| Unknown system | 65 | 5,921 |
| Virtual Boy | 23 | 30 |
| Wii | 1 | 1 |
| Xbox | 9 | 23 |
| Xbox360 | 4 | 0 |
| ZX Spectrum | 667 | 39,381 |

## Artwork and detail hierarchy

Exact original-path associations remain unchanged. ES-DE local and LaunchBox
local indexes are consulted before RomM; local screenshots are retained even
when RomM matches but provides none. No filename/title guessing is introduced.
Diagnostics now report local candidates alongside the matched RomM record ID
and its candidate count. An empty matched record explicitly says:
“RomM matched this game, but that record has no screenshots.” Source lookup
warnings or competing RomM records prevent a definitive zero-total claim.
Candidate counts describe metadata references, not successfully decoded images;
existing bounded/lazy delivery and failure/retry states still apply.

Screenshot lookup and timing diagnostics live under collapsed Advanced details.
Normal detail content prioritizes Play, platform, concise verification/file
status, discovered emulator readiness and game actions. Redundant status prose
has been removed; diagnostics have not been removed.

## Live retest

1. Check Games: wheel/touchpad through the list; End reaches its last platform.
2. PageUp/PageDown/Home, resize small/large, leave and return; sidebar stays put.
3. Games: select Arcade after a search/health filter; see 34 catalogued rows,
   exact source details and the extracted-ROM explanation, not a disk inventory.
4. GBA 007 detail: normal actions remain prominent; expand Advanced details to
   inspect original path, RomM record and screenshot search outcome.
5. A game with local screenshots and none in RomM still offers its screenshots.

## Validation

All Cargo commands used `CARGO_TARGET_DIR=/home/davedap/.cache/emuwiz-cargo-target`
and `CARGO_INCREMENTAL=0`; no worktree target directory or `/dev/shm` was used.

| Check | Result |
| --- | --- |
| Focused GUI v2 (including scroll, filter/projection, diagnostics) | 42 passed |
| Navigation subset | 29 passed, 1 existing naming failure |
| Catalogue/database subset | 105 passed |
| Artwork subset | 75 passed |
| Full core library | 9,518 passed, 3 ignored |
| Full GUI library | 2,779 passed, 3 existing failures, 2 ignored |
| Workspace Clippy, all targets/features, `-D warnings` | Passed |
| Targeted rustfmt check, git diff check, scoped task-postcheck | Passed |
| Release build, `archivefs-gui --bin emuwiz-v2` | Passed |

The three unchanged legacy GUI failures are:

- `health_and_platform_actions::library_renders_multiple_complete_rows_at_desktop_and_small_viewports`
- `platform_shelf_and_library_shell::every_navigation_destination_has_a_title_and_width_policy`
- `platform_shelf_and_library_shell::major_workflows_are_reachable_from_home_sidebar_and_top_menu`

They concern the legacy 1024x600 Library layout and Converter/Disc Conversion
naming, not these v2 fixes. The subset runs used the GUI test executable produced
by Cargo. Logs are `/tmp/emuwiz-v2-liveqa-*.log` on the development machine.

Release output: `/home/davedap/.cache/emuwiz-cargo-target/release/emuwiz-v2`.
A release read-only measurement loaded 103,165 games/52 platforms in 911 ms and
indexed 18,940 covers/5,095 screenshot groups in 2,723 ms with no artwork network
requests. These are one-run observations, not timing guarantees. The running
GUI was not restarted; interactive acceptance of the new binary remains a
live-user step.
