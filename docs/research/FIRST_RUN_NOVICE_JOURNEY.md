# EmuWiz 0.9 first-run novice journey

## Journey model

The first-run overlay gives a new user a short, resumable route through the
existing application:

1. **Scan** — choose a source folder and let EmuWiz read it.
2. **Understand** — see what identification and DAT evidence means.
3. **Fix** — review attention items and use existing preview-first repairs.
4. **Organise** — preview a separate playing or published library.
5. **Play** — inspect emulator readiness and explicitly launch a verified item.

The overlay displays these five concepts as orientation. Its implementation
steps remain the existing Sources, DAT Sources, Emulator Setup, and Verify
pages, so the onboarding path does not create a second scanner, catalogue,
repair engine, organisation planner, or launch resolver.

## First-run detection and persistence

Automatic opening uses the existing `missing_config_is_first_run` predicate.
An existing confirmed configuration is never presented as a fresh install.
Progress is stored in the GUI-only `onboarding_state.txt` sidecar under the
normal config directory. Missing, malformed, or unreadable state is treated as
not started. The user can skip the journey, finish it, or reopen it later from
Settings; reopening does not clear configuration or library data.

## Safety boundaries

Navigation and explanation are read-only. Choosing a source invokes the same
source-page action as normal navigation. DAT setup only registers or reads the
existing DAT state. Emulator setup reports readiness and does not configure an
emulator. Repairs and organisation remain preview-first, explicitly confirmed
actions on their existing pages; game launch remains an explicit user action.

The source library and any playing/published library are described as
different things. A source is where files already live. A playing library is a
separate planned output and is never implied by scanning.

## Plain-language states

The journey keeps detailed evidence available on the underlying pages while
using short explanations for new users: verified evidence is “Verified”, a
probable result is a “Likely match”, ambiguity asks for review, and conflicting
or stale evidence is described as needing attention or a re-check. The
technical details remain available through the existing advanced surfaces.

## Degraded states

Empty or inaccessible folders, missing catalogues, interrupted scans, absent
emulators, and incomplete media sets are shown as facts with a next action;
they are not represented as successful readiness. No DAT authority or
recognised game is not an error in itself. Existing Needs Attention,
organisation, Media Sets, and launch-readiness pages remain the authoritative
places for their detailed findings.

## Walkthrough boundary

The deterministic GUI coverage exercises fresh state, an existing configured
state, skip/reopen, empty source state, and the handoffs to the existing
Sources, DAT, Emulator Setup, Verify, Needs Attention, organisation, Media
Sets, and launch-readiness surfaces. A realistic library walkthrough should
use disposable/read-only fixtures for ready, missing-BIOS, incomplete-media,
ambiguous, and unverified examples; it must not fabricate backend evidence or
mutate a user's collection.

The 0.9 coherence pass ends at guidance and handoff. It does not add new
repair execution, publication, emulator setup, launch, DAT download, or
network-provider behavior.
