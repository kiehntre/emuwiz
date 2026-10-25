# Job 36: Blue artwork and metadata GUI validation

The synthetic Linux GUI journey **passes** on source candidate
`1f33b3f803762d9f5c5d0aa7d6837a68f5891bbc`. The work started from freshly fetched
`origin/main`, `5940c7286153def7715e6b5e5e4cd52c8c065706`. Normal CI/review and
any authorised promotion are tracked by the candidate pull request; this report
does not claim that main has been changed.

Worktree: `/home/davedap/emuwiz-job36-artwork`, branch
`qa/job36-artwork-metadata`. Evidence root:
`/home/davedap/emuwiz-qa-artifacts/job36-artwork/evidence/`.
The disposable profile is in the sibling `profile/` directory. No external
provider account, copyrighted download or playable commercial game was used.

The inventory found the existing per-field/per-kind Core resolver, Blue's
shared `MediaIndex`, generation-based picture workers, immutable local thumbnail
cache, provider cache and local-folder controls. Existing tests covered matching,
retry and basic caching, but no complete eight-point GUI journey report was
found. The clean local-artwork worktree contained an already-merged feature;
other active worktrees were left untouched. Details are in `inventory.md`.

Local folder enablement was exercised through the GUI. There is no configurable
provider-order preference. Existing source-class priority remains unchanged:
local LaunchBox artwork outranks ES-DE provider-cache artwork; metadata at equal
priority follows catalogue, ES-DE, RomM, then accepted ScreenScraper order, per
field. Provider metadata does not establish game identity. RomM in this run is
a synthetic, offline, locally written metadata-cache record with no artwork URL.

| Requested check | Result and executed evidence |
| --- | --- |
| 1. Selected cover, screenshots and metadata | PASS. `12-valid-details.png` shows the local cover and ES-DE description; `16-information.png` shows developer, publisher, release date, genre and players. `21-gallery-all.png` and `22-gallery-third.png` show all three selected screenshots in order. |
| 2. Provenance and conflicts | PASS. `17-alternatives-top.png` and `18-alternatives-lower.png` show selected and alternative values, providers and the existing priority explanation. `26-esde-provenance.png` and `31-local-cover-romm-metadata.png` identify the actual local source paths and provider evidence. |
| 3. Existing settings and precedence | PASS. Disabling LaunchBox selects the ES-DE cover and removes its gallery (`25-esde-fallback.png`). Disabling ES-DE as well removes artwork but retains cached RomM metadata (`29-both-local-disabled.png`, `30-romm-metadata-fallback.png`). Local cover plus RomM metadata coexist when only LaunchBox is re-enabled (`31`); enabling ES-DE restores its metadata without replacing the higher-priority local cover (`32-restored-precedence.png`). |
| 4. Source changes refresh evidence | PASS. After changing only two synthetic source files, Recheck displays the new cover pixels and description (`33-refreshed-source-evidence.png`). GUI source toggles also refresh automatically. A regression prevents settings from being saved during an active check without a corresponding refresh. |
| 5. Truthful missing-artwork fallback | PASS. `46-missing-artwork-details.png` shows the dimmed platform hardware labelled “No picture yet” and “No screenshots found for this game.” It does not borrow the other game's artwork. `30` also demonstrates missing artwork despite retained metadata and existing cached images. |
| 6. Restart and invalidation | PASS. A source replacement adds a derived cache entry, preserving all prior entries (`cache-before-source-change.json`, `cache-after-source-change.json`). Temporarily removing the selected source chooses ES-DE instead of the stale cached cover (`34-missing-source-fallback.png`). After normal close/restart, all seven thumbnail filenames and hashes persist unchanged; `48-after-restart.png` restores cover/metadata and `50-restart-cache-evidence.png` reports `cache_hit: true`, network time zero. A separate-process regression also verifies changed pixels, a cache miss after replacement and refusal after source removal. |
| 7. Narrow Blue layouts | PASS at the declared minimum 620×480. Artwork provenance and metadata remain reachable and wrap within the window (`36`–`38`); the compact details entry works (`40`), artwork actions remain reachable (`42`), and information rows remain readable (`45`). The final toolbar avoids overlap for empty and populated searches (`52`–`54`). Small windows require vertical scrolling. Captured window geometry is stored beside these screenshots. |
| 8. Preservation | PASS. All 19 real preference-file hashes are unchanged. All 11 valid-profile and 10 earlier fixture-source hashes match their originals after deliberate test replacements were restored. Native file-access traces show zero accesses to protected real profile/game/artwork roots, zero source-file write operations and zero internet connections. The disposable database has integrity `ok` and two game rows. See `preservation-audit.json`, the source/preference manifests, `database-integrity.json` and `session-exit.json`. |

The confirmed defects were corrected within existing modules:

- Game information bypassed resolved metadata and displayed only accepted
  ScreenScraper fields. It now uses the shared winning values with per-row
  source labels; metadata alternatives expose values and provenance rather
  than only a count. Accepted language values outside the neutral model remain
  labelled as saved ScreenScraper data.
- LaunchBox numbered images after `-01` were missed, and delivery collapsed a
  gallery to its single per-kind representative. Numbered images are indexed
  while exact catalogue-title filenames keep ownership; delivery retains up
  to eight distinct images from the selected source in provider order. Local
  screenshot diagnostics now count actual candidates.
- Source controls could save a change while an active index caused the refresh
  to be skipped. They are unavailable during that check, like Recheck itself.
- Expanded galleries displaced the details side column. A bounded grid keeps
  the gallery inside its column. Compact action labels wrap, and the Browse
  toolbar reserves space for Needs attention or uses its existing stacked row.
- Local-file provenance no longer calls the original artwork a cached copy;
  it describes the separate display-thumbnail cache. Full date values are
  labelled as release dates rather than years.

Baseline and intermediate failing regressions are preserved in
`regressions-before.log`, `gallery-parser-before.log`,
`refresh-and-title-before.log`, `layout-before.log` and `toolbar-before.log`.
The final source has **80 passing focused checks**: eight Job 36 regressions,
19 media-source checks, 23 artwork-worker checks, two thumbnail checks, 20 Blue
Home/Details checks and eight existing Blue usability checks. Two existing
real-provider diagnostics remain intentionally ignored. Formatting, scope and
GUI-root boundary checks pass. See `final-candidate-checks.json`,
`final-candidate-*.log`, `blue-usability.log`, `format.log` and `scope.log`.

The full local monolithic Core test compilation was cancelled through Cargo
under sustained swap pressure (45 GiB swap used on a 46 GiB host). It produced
no test result and is not counted as a pass. No other process was signalled and
no target was removed. Public Core discovery/matching is exercised by the GUI
integration regressions; full Core validation is required in normal CI. See
`core-build-cancelled.md` and `final-launchbox.log`.

The complete artwork/metadata native sequence ran at `070139a6`; the final
production delta at `1f33b3f8` changes only the Browse toolbar. Restart/cache and
both narrow toolbar cases were rerun on that final executable. The unchanged
artwork/metadata evidence is retained as recorded in `native-evidence-reuse.json`.
Executable paths, source SHAs and binary SHA-256 values are recorded in
`journey-native-provenance.json` and `native-provenance.json`.

The first ES-DE fixture used absolute references, which the existing safety
rules correctly refused. It was preserved, and the complete journey used a
fresh profile with relative gamelist paths, an explicit ROM directory and the
existing downloaded-media layout. No safety check was relaxed.

This run covers Linux/X11 with software rendering, local ES-DE/LaunchBox media
and offline synthetic RomM metadata. It does not certify online provider
credentials, network retrieval, other desktop platforms or emulator gameplay.
The native application, its private display and its controller exited. Cargo
ownership and worktree retention continue to follow the repository's lifecycle
gates while CI/review or authorised promotion remains outstanding.
