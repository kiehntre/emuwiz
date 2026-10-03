# GUI v2: "Why can't I play this?" launch diagnosis

Game Details shows one launch card (`launch_readiness_summary`). When a game is
not ready, the card now lists **every** blocker the launch plan already knows
(most actionable first, "N things need attention"), each with a plain-language
reason and a direct fix route, and says the game was not changed. Raw blocker
names and messages stay under "Readiness details". When the blockers are
resolved, the same card turns into "Ready to play" and names the emulator.

Presentation only: nothing here plans, probes or launches. Code:
`gui_v2/launch_readiness_summary.rs` plus `launch_readiness_summary/diagnosis.rs`.

## Cause to action

| Situation (from launch-plan evidence) | Fix route |
| --- | --- |
| Game file unreachable | Sources (check games folder) |
| Disc set blocked / needs review | Multi-disc games |
| Identity not confirmed | Check |
| No emulator candidate | Emulator Setup |
| Profile ineligible, core missing, executable missing, binding unavailable | Emulator Setup |
| Firmware / BIOS missing | BIOS / Firmware Setup |
| Content format unsupported by the adapter | Problems |
| Platform not supported by the adapter | none (explained only) |
| Several emulators possible | Choose emulator |
| Anything else the plan refused | Problems, listed honestly as "can't determine" |
| Stale readiness | Recheck (in place, no duplicate, generation-guarded) |
| Last Play: could not start / closed within 5 s | Emulator Setup / Activity |

## Backend evidence gaps (recorded, not invented)

1. The plan has no adapter support level (ready / readiness-only / unproven /
   unsupported), so the card never claims one; it only reports blockers.
2. "Installed but not detected", "executable moved" and "not installed" share
   `NoInstallationCandidate`; the card says it cannot tell them apart.
   RetroArch has a distinct executable-missing blocker and is described as an
   incomplete setup.
3. No blocker kind exists for "emulator version unsupported", "required key or
   metadata missing" (beyond per-adapter serial/ID blockers), or "BIOS present
   but unsupported".
4. Disc-set blockers do not say incomplete vs conflicting; the card says there
   is a problem or that review is needed, and the detail text carries the rest.
5. Launch workers return a rendered error `String`, not a typed spawn error
   (`launch_readiness_page.rs`, owned by another lane at the time). Start
   failures are classified only from the standard OS error kinds and the
   preflight refusal. A typed `ProcessExitReport` would replace the "closed
   within 5 s" timing heuristic with the real exit status.
6. The full Launch page still prints its own failure text; only the Game
   Details card and Activity use the new wording.
7. `scripts/qa/synthetic_library.py` cannot yet create emulator/BIOS fixtures,
   so only "no emulator", "game file moved" and multi-disc are reachable live.
