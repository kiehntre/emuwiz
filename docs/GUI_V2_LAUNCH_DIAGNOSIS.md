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

## Game Doctor (Part 1, read-only)

Game Details has a collapsed **Game Doctor** section (`gui_v2/game_doctor.rs`),
drawn last in the page body so it never moves anything above it, that explains, in one place, what EmuWiz already knows about the selected game.
It is a projection only: no I/O, probing, hashing, planning, launching,
persistence or actions.

- **Identity tier** comes from Game Details' own identity answer
  (`detail_identity::decide`, which applies the DAT current-proof rule), the
  canonical identity review and the saved catalogue state: *Verified* (current
  proof only), *Probable* (one likely identity, not verified), *Ambiguous*
  (several candidates, conflicting evidence, or a saved ambiguous DAT
  comparison), *Unknown* (file name only, or no trusted system) and *Still
  being checked* (a saved DAT match whose proof is being re-read).
- **Launch readiness** is the exact summary the readiness card last displayed
  for this game and path (a repaint is requested whenever that answer changes,
  so a Doctor drawn earlier in the frame catches up). Its blockers are listed as
  *Missing*; a blocked answer with no listed blocker shows the card's own
  headline and explanation; firmware is *Missing* only when the plan says so
  (present-but-unverified or unknown firmware is never called missing; the
  card's own warning is repeated instead). Warnings, a stale answer and the last
  in-session launch attempt are listed as *Warning*. Until the card has
  answered, the Doctor says readiness has not been answered yet.
- Passing checks reads "Launch checks passed", never "Ready to play"; the section always states that
  EmuWiz keeps no record of gameplay and that recognition and passing launch
  checks do not prove the game plays.

Not covered yet: repair offers or links (Next Frontier M31 GD-3/GD-4, deferred
in the Midnight Handoff), a persisted launch-outcome record (NF-38-01), the
core `diagnostics::Finding` projection (NF-31-01) and a CLI `doctor --game`.

## Setup & Doctor: selected-game launch check

The first card on Setup & Doctor (`gui_v2/doctor_launch_check.rs`) restates
the launch-readiness answer Blue already holds for the game selected in
Browse & Play. It runs no check of its own and changes nothing.

- **Ready**: a current answer whose launch checks passed with no findings.
  Doctor never offers Play.
- **Unsupported**: every finding is `FormatUnsupported` or
  `PlatformUnsupported`. If anything else is also wrong, that fixable item
  leads, as **Needs attention**.
- **Needs attention**: the first (most actionable) finding's title, reason
  and existing fix. A stale answer, or a blocked answer with no finding,
  keeps the readiness card's own headline and action.
- **Not checked**: no game selected, or its check is still running. This is
  never reported as Ready.

The one button opens an existing screen through `route_for_action`.
Play and "Try anyway" open the Launch page, whose preflight and
confirmations still decide. Recheck and "Look up online" open Game Details.

### Gap: no controller configuration destination

The follow-up asked for an optional "Configure controller" action. It is
**omitted**, because Blue has no controller configuration screen. The only
controller surfaces are read-only:

- the Launch page's controller settings evidence, which is PCSX2 only
  (`launch_readiness_page/controller_readiness.rs`, #101);
- Hatari's report in Emulator Setup.

Both only say whether settings exist. They bind nothing, and they say so.
Launch readiness carries no controller fact. When a real controller
configuration workflow exists, add the button as a secondary action in
`doctor_launch_check::show`. It must not change the verdict.
