# Blue host and emulator compatibility readiness: blocker report

**Inspected base:** `origin/main` at `a3b76d7ddcc74fd911dc1898ff6fff5d9ad04a9b`

**Scope:** existing Core evidence and Blue reuse points for a read-only, per-game host/emulator compatibility card.

## Decision

Current `origin/main` does not contain enough trusted evidence to implement the requested host/emulator compatibility comparison without inventing compatibility data. Keep host compatibility, emulator hardware requirements, graphics API/backend support, and expected performance **Unknown** until those facts have authoritative producers. Do not derive them from platform names, emulator names, launchability, or installed state.

This is a concrete evidence blocker, not a claim that games cannot launch. Existing launch readiness remains useful and should be reused if the missing evidence contract is added later.

## Evidence already available

- `archivefs_core::ready_to_play::project_ready_to_play` is a pure projection over supplied evidence. `ReadyToPlayResult` distinguishes ready, warnings, attention, blocked, unsupported, and unknown states, and reasons preserve their provenance and fixability. It does not discover hardware or emulator requirements (`crates/archivefs-core/src/ready_to_play.rs`).
- `LaunchPlan` and `LaunchReadiness` expose per-game launch blockers and warnings. A shadPS4 warning explicitly says its bound inputs do not verify compatibility or host capability (`crates/archivefs-core/src/launch/readiness.rs`). This is evidence about the limits of that launch plan, not a negative or positive GPU result.
- Blue already has Game Details and per-game setup navigation, including a route to Emulator Setup (`crates/archivefs-gui/src/selected_game_readiness.rs`, `crates/archivefs-gui/src/gamer_view.rs`). The Ready-to-Play page consumes supplied results and performs no scan itself (`crates/archivefs-gui/src/ready_to_play_page.rs`).
- Emulator inventory and setup workflows can establish installation/configuration facts for their own supported checks (`crates/archivefs-core/src/emulator_inventory.rs`, `crates/archivefs-gui/src/emulator_readiness_state.rs`, `crates/archivefs-gui/src/emulator_setup/`). Those facts do not establish host hardware compatibility or graphics backend support.
- Existing static metadata such as MAME original-input requirements describes the original game controls, not PC hardware or emulator graphics requirements (`crates/archivefs-core/src/mame_input_requirements.rs`).

## Evidence missing on current `origin/main`

1. **Host snapshot contract:** no current Core host-capability model or bounded host snapshot API supplies architecture, CPU features, memory, graphics adapters, or graphics API/version/extension evidence to Blue.
2. **Trusted emulator requirements:** no current authoritative per-emulator requirements registry supplies supported host architectures, required CPU features, minimum memory, or graphics requirements with provenance and versioning.
3. **Configured backend evidence:** no current per-game/per-profile projection ties the selected emulator configuration to a graphics backend/API that can be compared with detected host API evidence.
4. **Freshness:** no shared evidence generation or invalidation contract connects emulator installation/configuration changes to a cached host compatibility result. A card could otherwise display stale state after setup changes.
5. **Coverage:** absent requirement metadata cannot be interpreted as a pass. In particular, current launchability and an installed emulator are not substitutes for these missing facts.

Historical commits `9eed96c1` and `26fa9dc8` are possible future recovery sources for the host-capability projection and launch-planning wiring. They are not ancestors of the inspected `origin/main`, so they are not proven compatible code for current main. Their presence in the local object database is not current product evidence or an active PR, and they do not supply authoritative per-emulator requirements or trusted graphics-backend evidence by themselves.

## Smallest safe next step

Before drawing a compatibility card, land a reviewed Core evidence contract with:

- bounded, read-only host facts and source/probe timestamps;
- trusted, versioned emulator requirement metadata (unknown when absent);
- selected emulator/profile configuration evidence, including the configured graphics backend where available;
- a deterministic comparison result that keeps installed, configured, compatible, unknown, and blocked as separate states;
- invalidation keyed to emulator/configuration changes;
- an explicit separation between technical launch readiness and unmeasured performance.

Blue can then render that projection within Game Details, reuse the existing Emulator Setup and Launch Readiness routes for actionable blockers, and show a concise Unknown state where a fact is unavailable. The view should not probe hardware while browsing. No FPS, game compatibility rating, or smooth-play promise follows from minimum requirements.

## Acceptance evidence still needed

When the Core contract exists, focused tests should cover supported and missing evidence, unknown requirements, an unsupported graphics backend, invalidation after emulator/configuration changes, wording that makes no performance promise, and the 1024×600 Game Details layout. No such compatibility-card tests can currently assert real host/backend behavior without first adding the evidence producers.
