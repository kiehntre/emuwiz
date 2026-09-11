# Next EmuWiz alpha — draft release notes

> Draft only. This is a user-facing summary of changes reachable from
> `v0.8.2` to the current development head; it does not create a release,
> version bump, tag, or artifact.

## Library / Home

- Home now has a clearer first-use path from onboarding into the library, plus
  more useful Sources & Discovery guidance for empty and newly configured
  libraries.
- Playing Library and organisation workflows have continued safety coverage
  for selection, confirmation, apply, recovery, and rollback.

## DAT verification and identity

- Game Details and verification views explain DAT identity, structural
  identity, and verification status more plainly, including catalogue and
  variant provenance where it is available.
- Collection verification remains local-first: evidence is inspected and
  presented before any user-approved action; ambiguous or insufficient
  identity fails closed rather than being guessed.

## Emulator setup and launching

- Emulator Setup and Doctor now present readiness more clearly, with native
  standalone candidates kept separate from RetroArch candidates.
- Added reviewed standalone adapters for **RMG** (Nintendo 64), **Mesen 2**
  (NES, SNES, Game Boy family, PC Engine, WonderSwan family), **Snes9x**,
  **Stella**, and **VICE** where their supported direct content and local
  discovery evidence are safe.
- Mesen 2, RMG, and the other candidates do not become automatic winners:
  users retain a separate choice, and missing, stale, ambiguous, unsupported,
  or non-executable installations are refused.

## Cheats & Mods

- Cheats & Mods has clearer safety/readiness presentation and completed
  regression coverage for local, provider-backed, preview, apply, history,
  and rollback journeys.
- The normal workflow remains read-only/planning by default. Changes require
  explicit selection and confirmation, and managed rollback never treats
  unrelated user files as EmuWiz-owned.

## Firmware / BIOS

- Firmware and BIOS readiness is presented in plainer language in Game
  Details, Doctor, and Emulator Setup.
- Required firmware remains platform-specific and fail-closed; optional or
  built-in firmware is not incorrectly reported as a requirement.

## Playing Library / organisation

- The Playing Library’s reviewed organisation, one-game/one-region choice,
  and reversible filesystem workflows retain explicit preview and
  confirmation boundaries.
- Recovery and rollback continue to use the journalled, no-clobber safety
  model rather than silently moving, overwriting, or deleting user content.

### Known DAT rename limitation

DAT rename/apply includes durable identity, freshness, no-clobber, journaling,
recovery, and rollback protections. A last-mile Linux regular-file race remains
open: an external source replacement between final preflight and pathname-based
rename can cause mutation of the replacement object. This is documented and not
fully fixed in this release. The deterministic known-gap regression covers this
sequence; planning and preview are unaffected. Users requiring strict source
immutability should avoid regular-file apply while another process may modify
the source tree.

## ES-DE integration

- ES-DE export/publication coverage now includes additional safe canonical
  platform mappings, including TurboGrafx-16 and PC-98/NEC PC-9801.
- Unsupported mappings remain explicit refusals instead of guessed system
  names or launch commands.

## RomM integration

- Large-library RomM planning and browser projection received performance
  validation and guard coverage.
- RomM layout work remains local-first: plans surface missing, unsafe, or
  conflicting paths before an explicit apply; no destination is overwritten.

## AppImage / installation

- EmuWiz AppImage packaging now records pinned toolchain provenance and has a
  FUSE-less extract-and-run fresh-install QA path.
- RetroArch AppImage profiles can be discovered only through the reviewed
  adapter path; EmuWiz does not grant generic AppImage execute permission.

## Reliability / safety

- Recent release-readiness work closes regressions in Home, Doctor, Emulator
  Setup, DAT presentation, firmware presentation, and RMG candidate wording.
- Across library, launch, DAT, emulator, and organisation features, EmuWiz
  remains **local-first**, **read-only/planning by default**, and uses
  **explicit apply/confirmation** for mutations. Identity, firmware, content,
  and executable readiness failures are designed to **fail closed**.

## Known deferred / not in this release

- **openMSX** remains deferred: its machine-profile integration is greenfield
  work, not a safe small adapter addition.
- ES-DE intentionally still does not map Atari 8-bit, Commodore 128,
  NeoGeo64, or generic PC; MegaDrive remains intentionally partial rather
  than guessing a region.
- Optional physical launches for newer standalone adapters, desktop
  click-through QA, and the current-artifact interactive fresh-user AppImage
  acceptance remain manual follow-up. In particular, this draft does **not**
  claim that manual AppImage acceptance has passed.
