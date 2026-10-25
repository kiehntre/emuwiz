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
| Last Play: could not start / closed shortly after starting | Emulator Setup / Activity |

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
5. **Current limitation: launch workers return rendered error strings, not
   typed spawn/exit outcomes** (`launch_readiness_page.rs`, owned by another
   lane). So the card says only what it can know:
   - A failed start is shown as "The emulator could not be started." Any reason
     (program may be missing, permission may be denied, final check may have
     declined) is a hedged *hint read from the error text*, labelled as not a
     confirmed diagnosis; the original rendered error is kept only under
     Technical details.
   - An emulator that stops within a 5 s *startup observation window* is shown
     as "closed shortly after it was started. EmuWiz cannot yet tell why it
     closed." The window is an EmuWiz display rule, not a backend boundary, and
     the card never says the emulator crashed, failed internally, has an exit
     reason, or is an unsupported version.
6. The full Launch page still prints its own failure text; only the Game
   Details card and Activity use the new wording.
7. `scripts/qa/synthetic_library.py` cannot yet create emulator/BIOS fixtures,
   so only "no emulator", "game file moved" and multi-disc are reachable live.

## Future improvements

- A typed spawn result and typed exit status from the launch workers.
- Structured adapter support/readiness in the plan.
- Distinct emulator-detection blockers (not installed / not detected / moved /
  unsupported version).
