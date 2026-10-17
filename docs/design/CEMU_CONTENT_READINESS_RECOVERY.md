# Wii U / Cemu content readiness recovery

Source: `feature/cemu-content-readiness-wiring` at
`c83fc5d32daa2ac999dba686d70fdb34e776745f`, including predecessors
`cfb4e1e9` (projection) and `5b845b1d` (composition). Starting main:
`35e08ff17d66d8b26213e1c9491b55d0263fa9e7`, freshly fetched 2026-10-17.
This is a behaviour port, not a cherry-pick or another Wii U backend.

## Historical/current capability comparison

| Capability | Initial classification | Result |
| --- | --- | --- |
| Base layout and single RPX selection | ALREADY IMPLEMENTED | Reuses `cemu_local::inspect_extracted_layout`; recovered projection adds missing/ambiguous/conflicting evidence handling. |
| Base/update/DLC role classification | ALREADY IMPLEMENTED, defective mapping | Corrected main's reversed update/DLC prefixes: `00050000` base, `0005000E` update, `0005000C` DLC. Historical classifier agrees with Cemu. |
| Update/DLC relationships and versions | MISSING | Recovered structural family checks; exact configured MLC locations, title-ID/slot agreement, declared versions, duplicate/conflicting-version refusal. No automatic version choice. |
| Extracted versus packaged representation | ALREADY IMPLEMENTED | Existing extracted-layout adapter and bounded `wiiu_disc` inspector are reused. Recovered projector exposes them together with NUS structure. |
| Encrypted/unsupported classification | ALREADY IMPLEMENTED / MISSING NUS projection | WUD/WUX/WUA remain refused by Cemu; NUS evidence is displayed and refused. No keys are read or acquired. |
| Structural title identity | ALREADY IMPLEMENTED | Keeps main's self-reported metadata evidence and recovered confidence/provenance. It is not cryptographic verification. |
| Canonical verified Wii U identity bridge | BLOCKED | Current main has no Wii U identity-conferring kind. This recovery does not fabricate one or promote metadata to canonical identity. The dedicated read-only preview works independently; shared candidates still require resolved upstream identity. |
| Cemu executable/profile selection | ALREADY IMPLEMENTED | Existing profile discovery, exact native binding, installation eligibility and command/preflight paths. No additional resolver or selection framework. |
| Historical launch matrix wiring | SUPERSEDED | Additive composition over current candidates and current Cemu command/preflight, preserving prior blockers and remembered choices. |
| Content readiness in Game Details | MISSING | Selection-bound background inspection and explicit Cemu preview/refusals. |
| Stale-evidence warning downgrade | SUPERSEDED | Nested base/related content and binding stamps; changed previews are refused before execution rather than merely warned. |

Prefix evidence: Cemu's title-ID definitions at
https://github.com/cemu-project/Cemu/blob/main/src/Cafe/TitleList/TitleId.h
and its update title checksum manifests, for example
https://github.com/cemu-project/title-checksums/blob/master/0005000e10102000_v32.json .

## Supported layouts and presentation

The existing native Cemu backend accepts an absolute extracted base-title root
with `code/` containing exactly one regular `.rpx`, `content/`, and
`meta/meta.xml` declaring a unique, valid base TITLE_ID. Metadata must be a
complete menu XML document. A declared version, when supplied, must fit a u16.
Missing identity, malformed/duplicate metadata, symlinks encountered in the
inspected tree, missing structure, multiple RPXs, and contradictory structured
IDs refuse readiness. A directory name alone never authorizes launch.

Related extracted update/DLC structures can have `meta/meta.xml` and
`content/` without executable code. Inspection checks only
`<configured MLC>/usr/title/0005000e/<lowercase low title ID>` and the analogous
`0005000c` path. The metadata must agree with that exact slot and base family.
Presence is not a claim that an update was applied or DLC is usable. Other
library folders are not searched and content is never installed or modified.

Game Details displays representation, role, encryption state, structural
readiness, TITLE_ID and its evidence confidence, declared version, exact Cemu
profile/executable, related content paths/versions/relationships, and either
the existing backend's command or explicit refusal reasons. The result is
labelled as an inspection at refresh time and has a refresh button. Inspection
and profile/preflight work run off the GUI thread. Changing selection drops
late results. No filesystem traversal occurs while composing cached shared
candidates.

Cemu remains **preview-only in Game Details**. This task does not introduce a
new Cemu GUI Launch action. The core helper `launch_previewed_cemu` validates a
preview's base, related and binding stamps, rebuilds and compares the command,
and invokes the existing fresh preflight/executor. Existing direct preflight
also checks structural readiness and nested content freshness. Cached preview
readiness is never execution authority.

## Limits and unsupported inputs

- WUD/WUX, WUA internals, raw encrypted NUS packages, arbitrary RPX files,
  update/DLC as a base game, and unknown representations are not native launch
  inputs in this build. Recognizing a container is not verifying its game.
- No decryption, keys inspection/acquisition, downloads, update/DLC installation,
  WUD/WUX/NKit conversion work, configuration writes, R2 or journal changes.
- The historical projection's structural walk is capped at 4,096 entries and
  examines root/code/meta; it does not traverse payloads to infer identity.
  Metadata reads are capped at 64 KiB and NUS header probes at 1 KiB.
- Freshness is an inode/device/size/mtime observation over at most 8,192 tree
  entries and depth 32. It is not a payload hash or an atomic filesystem
  snapshot. Exceeding limits, unreadable entries, and symlinks fail closed.
- Metadata TITLE_ID is editable structural evidence. Canonical identity and
  cryptographic authenticity are never inferred from filenames or this preview.
- Platform launch support, emulator compatibility, and actual application of
  installed content remain distinct from structural presence.

## Historical preservation

The historical branch and all three unique commits remain preserved. The
projection was recovered and hardened; relevant composition behaviour was
adapted to current main. Historical planner APIs and the warning-only stale
policy were deliberately superseded. Branch deletion is outside this task;
retirement can be considered only after the reviewed port is on GitHub main.
