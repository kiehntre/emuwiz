# Launch-Option Trainer Family Research

## Scope

This follow-up extends the documented WHDLoad custom-option model without
turning arbitrary emulator settings, DOS commands, or scripts into cheats.
Only options whose documentation describes gameplay behavior are eligible for
the trainer surface.

## Classification matrix

| System | Candidate | Scope | Typed/documented | Identity | Config/apply | Decision |
| --- | --- | --- | --- | --- | --- | --- |
| ScummVM | Hypno: `cheats`, `infiniteHealth`, `infiniteAmmo`, `unlockAllLevels` | per-game target | boolean; official engine Game Options | verified `engine:game` plus selected folder/profile | INI target, alternate `--config`, shared transaction | **ImplementNow** |
| ScummVM | other engine options (enhancements, debug, graphics/audio) | per-game or global | often typed/documented | available, but not trainer semantics | technically configurable | **ResearchOnly** for Cheats/Trainers; route to normal settings where appropriate |
| DOSBox Staging | `[autoexec]`, config overrides, machine/video/audio settings | profile/session/global | deterministic, but commands/settings are not a trainer declaration model | profile identity only | possible to write a config, but arbitrary commands are unsafe | **Reject** as trainer adapter |
| DOSBox-X | `[autoexec]`, `-c`, `-set`, `-conf` | profile/session/global | documented config and command channels | profile identity only | `-c` can execute arbitrary DOS commands | **Reject** for trainer UX; no script execution |
| upstream DOSBox | configuration and autoexec | profile/session/global | deterministic configuration, no typed gameplay trainer metadata | profile identity only | no EmuWiz-owned game trainer layer | **ResearchOnly** |
| RetroArch/libretro | core options and game `.opt` files | per-core/per-game | typed by cores, documented individually | game/content/core path | per-game option files are deterministic and reversible | **ResearchOnly** unless a core explicitly documents gameplay trainer semantics |

## Sources

- [ScummVM Game settings](https://docs.scummvm.org/en/latest/settings/game.html)
- [ScummVM configuration file](https://docs.scummvm.org/en/latest/advanced_topics/configuration_file.html)
- [ScummVM command line](https://docs.scummvm.org/en/latest/advanced_topics/command_line.html)
- [DOSBox-X wiki](https://dosbox-x.com/wiki/Home)
- [DOSBox-X command-line options](https://dosbox-x.com/wiki/DOSBox%E2%80%90X%E2%80%99s-Command%E2%80%90Line-Options)
- [Libretro content/folder/core overrides](https://docs.libretro.com/guides/overrides/)
- [WHDLoad usage and options](https://www.whdload.de/docs/en/opt.html)

## ScummVM findings

ScummVM has a documented per-target INI model. The official engine settings
page labels Hypno's four gameplay toggles as cheats: enabling original cheats,
infinite health, infinite ammo, and unlocking all levels. These are not memory
addresses and do not modify game data. The adapter therefore accepts only the
exact verified `hypno:<game>` identity and projects only those four boolean
keys.

The generated profile uses ScummVM's documented alternate configuration
(`--config`) and target-name mechanism. The target section contains the exact
game ID, engine ID, game path, and selected typed values. The launch command
uses the target name, so ScummVM consumes the same deterministic per-game
profile that EmuWiz previews and transactions.

Other engine-specific options are deliberately not all trainers. For example,
`enable_enhancements`, `debug`, audio/video switches, restored content, and
quality-of-life options remain outside this adapter. A future subsystem may
model them as game settings or enhancements, but classifying them as cheats
would blur provenance and user intent.

## DOSBox findings

DOSBox-family configuration files are useful launch profiles, but `[autoexec]`,
DOSBox-X `-c`, and DOSBox-X `-o`/`-set` channels are command/configuration
mechanisms rather than a machine-readable trainer declaration system. Treating
arbitrary commands as trainer options would introduce script execution and
would make identity, rollback, and safety claims unprovable. EmuWiz therefore
does not implement a DOSBox trainer adapter in this task.

Typed known DOSBox settings may be appropriate for a future emulator-profile
subsystem. They are not Cheats/Trainers unless a specific game-facing feature
is documented and can be bound to an exact profile.

## RetroArch findings

RetroArch supports per-game `.opt` files and layered overrides. The files are
deterministic and reversible, but core options are not inherently cheats:
most control rendering, latency, audio, input, or emulator behavior. A core
could expose a gameplay option, but EmuWiz must first establish an explicit
core/game declaration and provenance. No generic RetroArch adapter is added.

## Shared safety model

The implemented ScummVM adapter uses the existing shared preview and
transaction primitives. It requires:

1. verified `engine:game` identity;
2. a safe absolute game folder and safe EmuWiz-owned configuration path;
3. a bounded UTF-8 INI source;
4. typed boolean values from a fixed documented declaration set;
5. deterministic rendering preserving unrelated sections and keys;
6. atomic publication, backup, journal, and exact rollback.

Unknown keys, duplicate selections, unsupported engines, unsafe paths, stale
configuration fingerprints, and arbitrary/free-form values are refused.

## Legal and licensing notes

No trainer databases, game binaries, scripts, or external assets are bundled.
The adapter uses locally installed ScummVM identity/profile information and
public documentation. DOSBox and RetroArch are referenced as documented
external configuration systems only.
