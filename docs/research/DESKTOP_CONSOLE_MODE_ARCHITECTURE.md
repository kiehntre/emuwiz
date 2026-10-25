# Desktop Console Mode architecture research

Status: research only. No production code, session configuration, compositor
configuration, emulator configuration, or desktop state is changed by this
document.

## Current EmuWiz boundary

The current launch layer already has an explicit candidate model, profile
discovery, identity/readiness blockers, direct argv construction, RetroArch
core candidates, and standalone adapters including VICE. Launch preflight
re-checks paths, executable identity, profile binding, and selected-game
identity. GUI-v2 already owns navigation and can open a separate window while
leaving the main application available.

The focused audit did not find a general controller-hotplug abstraction, a
frontend/game session supervisor, Gamescope integration, or a desktop-session
switcher. The architecture should add those as narrow seams rather than making
launch adapters own desktop policy.

## Research basis

- [Gamescope wiki](https://github.com/ValveSoftware/gamescope/wiki) and
  [README](https://github.com/ValveSoftware/gamescope/blob/master/README.md):
  Gamescope is a Wayland compositor and can run nested from an existing X11 or
  Wayland desktop; nested fullscreen/borderless/grab are explicit modes.
- [VICE monitor documentation](https://vice-emu.sourceforge.io/vice_6.html)
  is representative of why launch-time wrappers must remain adapter-owned:
  emulators have their own command/config semantics.
- [ES-DE FAQ](https://gitlab.com/es-de/emulationstation-de/-/blob/master/FAQ.md):
  ES-DE is MIT-licensed, desktop-oriented, and deliberately passes only the
  emulator options it needs.
- [RetroDECK desktop installation](https://retrodeck.readthedocs.io/en/latest/wiki_devices/linux_desktop/linux-install/)
  and [controller hotkeys](https://retrodeck.readthedocs.io/en/latest/wiki_rd_controls/hotkeys-retrodeck/):
  desktop deployment commonly relies on Steam Input and optional Gamescope,
  but that introduces another controller/configuration owner.
- [Batocera licensing](https://wiki.batocera.org/license): Batocera combines
  hundreds of upstream projects and assets with mixed licenses; Batocera code
  is generally LGPLv3, but bundled themes, decorations, fonts, and media must
  be reviewed independently.
- [Moonlight setup](https://github.com/moonlight-stream/moonlight-docs/wiki/Setup-Guide)
  documents fullscreen toggling, controller input, and a clean streaming-session
  quit action. [Sunshine documentation](https://github.com/LizardByte/Sunshine/blob/master/docs/getting_started.md)
  documents Linux support and application/session integration.
- [Nobara wiki](https://wiki.nobaraproject.org/) documents Gamescope availability
  and KDE/Wayland-oriented gaming setups, but does not make a separate gaming
  session mandatory.

## Approach comparison

| Approach | Desktop-safe | Controller-first | Wayland | Nvidia | Complexity | Maintenance | Recommended role |
|---|---|---|---|---|---|---|---|
| Existing desktop + fullscreen frontend window | Yes | Yes, if GUI-v2 adds input navigation | Strong; toolkit-dependent | Lowest risk | Low | Low | Baseline and MVP |
| Nested Gamescope | Yes | Yes, when input reaches the nested compositor | Strong where installed | Variable by driver/version | Medium | Medium | Optional per-session wrapper |
| Separate user graphical session | Riskier | Yes | DE/display-manager dependent | Hardware/session dependent | High | High | Avoid for MVP |
| Dedicated compositor/window mode | Usually | Yes | Complex across compositors | Variable | High | High | Later optimization only |
| ES-DE handoff | Yes | Mature controller UX | Good desktop support | Good, delegated | Medium | Medium/high integration | Optional external frontend |
| Batocera EmulationStation fork/config handoff | Not aligned with desktop goal | Mature | OS-oriented | OS/config dependent | High | High | Do not embed/copy |
| Native GUI-v2 Console Mode | Yes | Full control | Toolkit/platform work required | Depends on child windows | Medium | Medium | Recommended product direction |

## Recommended architecture

Implement an optional **nested Console Mode** inside the existing desktop
session:

```text
Desktop session
  -> EmuWiz GUI-v2 Console Mode window
      -> optional Gamescope nested wrapper
          -> existing validated emulator/core launch plan
          -> game process
      <- process exit/crash/session supervisor
  <- explicit Exit Console Mode
Desktop session unchanged
```

GUI-v2 remains the product surface. Gamescope is an optional launch wrapper,
not a required dependency and not a replacement compositor. A separate user
session is not needed and should not be started implicitly.

This preserves existing profile, identity, save, cheat, bezel, and launch
safety decisions because Console Mode consumes the already-authorized launch
candidate instead of reimplementing emulator discovery.

## Minimum viable Console Mode

1. A controller-first GUI-v2 mode with a clear opt-in and visible keyboard/mouse
   escape path.
2. SDL-backed controller discovery/hotplug state, with stable logical actions:
   Navigate, Confirm, Back, Search, Details, Launch, Quit Game, and Exit Mode.
3. Launch only existing `LaunchCandidate`/preflight results.
4. Child-process supervision that waits for normal exit, crash, signal, or
   explicit force-quit and always returns to the Console Mode window.
5. Fullscreen child-window policy delegated to the emulator first; optional
   Gamescope only after capability probing.
6. No media/config mutation beyond existing explicit EmuWiz workflows.

MVP should support one active game at a time, one selected display policy, no
session switching, no implicit Steam Input dependency, and no automatic HDR or
multi-monitor reconfiguration.

## Session lifecycle and failure policy

| Transition/event | Required behavior |
|---|---|
| Desktop -> Console Mode | Keep the original desktop session alive; create a managed frontend window. |
| Console Mode -> Game | Capture the launch receipt, selected profile, identity evidence, and child PID/process group. |
| Game exits normally | Reap the process, release wrapper resources, restore Console Mode focus. |
| Emulator crashes | Record exit/crash status, release the wrapper, keep Console Mode usable. |
| Frontend crashes | Do not kill or rewrite the desktop session; child process policy must be explicit and recoverable. |
| Controller disconnects | Show disconnected state; retain selection; accept reconnect without relaunching. |
| Display/sleep/wake changes | Re-query window/display state; never rewrite desktop display configuration automatically. |
| User force-quits | Send graceful termination, then bounded escalation; leave media and emulator config untouched. |
| Sunshine/Moonlight disconnects | Treat as input/display loss, not as authorization to kill the desktop session; provide configurable child cleanup. |
| Console Mode exit | Stop only EmuWiz-owned child/wrapper processes, then close the mode window. |

The supervisor needs a crash-safe journal/state marker so a restarted EmuWiz
can report an interrupted session without assuming that the emulator is still
alive.

## Controller model

Use a small logical-action layer, preferably SDL-compatible with the existing
GUI toolkit rather than a new database of controller layouts. Device discovery
and hotplug are separate from per-emulator mappings:

- frontend actions: D-pad/left stick navigation, A/Start confirm, B/Select
  back, shoulder buttons for page/search context, dedicated quit chord;
- game actions: delegated entirely to the emulator/profile;
- device identity: stable enough for display and diagnostics, never required to
  identify a game;
- reconnect: preserve focus and selection, expose “controller disconnected”;
- keyboard fallback: always retain Escape/Alt-F4 or a documented equivalent.

Avoid owning all emulator controller maps. EmuWiz should select a known profile
or launch context; the emulator should own obscure bindings and game-specific
input semantics.

## Gamescope role

Gamescope should be **optional and preferred only when explicitly enabled or
when a tested profile declares it safe**. It is useful for nested fullscreen,
scaling, frame pacing, and isolating child focus on both X11 and Wayland, but it
adds another compositor, Vulkan/driver dependency, and failure surface.

Do not require it for normal desktop mode. Probe the executable/version/backend,
display target, and child launch result. Prefer a direct fullscreen emulator
launch when Gamescope is unavailable or unproven. HDR, VRR, unusual scaling,
multi-monitor placement, and Nvidia-specific behavior should remain opt-in.

## Desktop/platform notes

### KDE / Wayland

Use a normal application window or nested Gamescope window. Do not manipulate
KWin workspace, global fullscreen, or display configuration. Test focus return,
multi-monitor placement, compositor shortcuts, and XWayland child emulators.

### Hyprland

Treat compositor rules as user-owned. Do not inject workspace/window rules or
assume a particular layer-shell policy. A direct window path should work; a
Gamescope path is an optional capability with explicit diagnostics.

### Nobara

Nobara is a strong target because KDE/Wayland, Gamescope, controller packages,
and Steam-oriented setups are common, but the desktop session remains the
authority. Support direct GUI-v2 first, then Gamescope where installed. Do not
require the Steam/Game Mode session or replace the user's login session.

### Nvidia

Do not promise identical behavior across proprietary-driver generations. Probe
Gamescope/Vulkan/Wayland support and retain direct-window fallback. Avoid
automatic HDR, VRR, DRM/KMS, or hardware-plane assumptions.

### Sunshine/Moonlight

Console Mode should look like one ordinary desktop application to Sunshine.
Keep the frontend visible before and after the game, preserve controller
forwarding, and provide an explicit remote quit action. A future virtual-display
integration is separate work and must not be inferred from desktop streaming.

## Configuration ownership

### EmuWiz owns

- selected game/media path and identity evidence;
- selected emulator/core and validated launch candidate;
- required BIOS/firmware path evidence;
- explicit fullscreen/session wrapper choice;
- controller-first frontend actions;
- per-game bezel/shader integration where already managed;
- cheat/save/document workflow handoffs and temporary runtime overrides;
- child-process supervision, return-to-frontend behavior, and diagnostics.

### Emulator owns

- renderer internals, audio internals, debugging, and obscure advanced options;
- game-specific controller mappings unless a profile explicitly delegates them;
- native save/config formats and emulator-specific runtime state.

Temporary overrides must be argv/environment/session-scoped, inspectable, and
removed after the child exits. EmuWiz must not silently rewrite emulator config
to make Console Mode work.

## Frontend choice and licensing

Native GUI-v2 is the recommended primary frontend because it preserves EmuWiz
identity, provenance, safety, cheats, bezels, and existing navigation. ES-DE is
a viable optional external integration because it is MIT-licensed and already
controller-oriented, but it would require a maintained export/launch boundary
and duplicate some library/config ownership.

Do not embed Batocera's fork, configuration generators, themes, decorations,
fonts, or media wholesale. Batocera's own license page documents a mixed
license set, including non-commercial assets. Integration with user-installed
ES-DE or user-provided themes is safer than redistribution.

Do not add cloud accounts or require Steam. Steam Input may be an optional user
choice, especially on RetroDECK-like setups, but SDL/controller support must
remain a first-class desktop path.

## Implementation phases

1. **Capability seam:** typed `ConsoleSessionPolicy`, logical controller action
   model, and child-process lifecycle states; pure tests only.
2. **GUI-v2 mode:** controller navigation, focus model, explicit enter/exit,
   keyboard fallback, and no emulator changes.
3. **Launch integration:** consume existing validated candidates and add a
   supervisor receipt without duplicating launch planning.
4. **Fullscreen policy:** direct emulator fullscreen first; add optional
   Gamescope wrapper with capability detection and fallback.
5. **Hotplug/recovery:** SDL device events, disconnect/reconnect handling,
   crash recovery, sleep/wake diagnostics.
6. **Streaming validation:** Sunshine/Moonlight tests, remote quit, focus,
   controller forwarding, and optional virtual-display research.
7. **Optional ES-DE bridge:** export/import only if the ownership boundary is
   demonstrably useful; no Batocera asset/config fork.

## Conclusion

The safest architecture is a normal desktop application with an optional
controller-first GUI-v2 mode and an optional nested Gamescope wrapper. It
provides the console-like flow without replacing the desktop session, adding a
second login session, or duplicating emulator configuration ownership.
