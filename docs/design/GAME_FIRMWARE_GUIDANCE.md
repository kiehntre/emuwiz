# Per-game firmware guidance

Blue Game Details reuses the existing Launch-selected candidate, discovered
profile, BIOS inventory and adapter inspectors. Expand **BIOS / firmware
evidence for this game**, then choose **Check BIOS / firmware evidence**.
Inspection runs on a worker. The panel reports the exact assessed target,
requirements, observed evidence and uncertainty. It cannot download firmware,
change settings, execute an emulator or change Launch decisions.

## Evidence by adapter

| Adapter | Guidance evidence | Limits |
| --- | --- | --- |
| PCSX2 | Selected profile's PS2 BIOS; existing verifier matches local authoritative DAT system, size, CRC32, MD5 and SHA-1. | An unknown hash is unverified, not proof of corruption. No inferred game region/version requirement; existing global-only config policy is retained. |
| DuckStation | Selected profile and existing game-settings resolver; PlayStation BIOS exact local DAT hash evidence, resolved slot and conflicts. | A configured region slot does not prove ROM region or title compatibility. Ambiguous/conflicting settings remain Unknown. |
| Flycast | Selected Dreamcast profile's existing BIOS and flash inspection. | Both must be verified for complete evidence. Dreamcast proof is not transferable to NAOMI/arcade firmware. |
| PC Engine CD | Existing System Card policy/readiness types are projected by the shared adapter helper. Selected RetroArch core declarations and recorded System-path file presence can be displayed. | Unknown emulator policy or unresolved per-title System Card sufficiency stays Unknown. Blue has no established standalone Mesen firmware-policy binding. HuCard games must not inherit CD requirements. |
| Hatari | Existing TOS inspector facts can be projected by the shared adapter helper. Selected RetroArch core declarations and recorded System-path file presence can be displayed. | TOS region/version observations do not establish a game's machine/TOS requirements. Blue's current Launch bridge does not contribute a standalone Hatari profile; guidance never selects one independently. |

RetroArch declarations are discovery snapshots. Presence under their recorded
System path is unverified evidence and always remains Unknown; standalone proof
is never transferred to a core. Unsafe relative paths and symlink observations
are refused. A global BIOS inventory is context, not proof that the selected
profile is configured to use an entry.

## Freshness and preservation

The worker brackets inspection with file/directory generation observations,
including effective configuration and firmware paths. It checks the existing
selected-game file fingerprint without hashing ROM content. Changed inputs
produce Stale. Missing game identity prevents Ready and leaves freshness
Unknown; unreadable configuration produces Unknown. A displayed snapshot expires after 30 seconds measured from worker
completion, including time queued while Game Details was hidden. Changing the
selected target discards results for the old request.

PCSX2 guidance bounds directory inspection to 64 entries and 128 MiB, in
addition to existing verifier limits. Firmware/configuration reads happen off
the GUI thread. Results are observations, not continuing filesystem monitors;
recheck after changing files or settings. No guidance path writes files or
modifies database records, library identity, BIOS inventory, profile selection
or Launch decisions.

## Historical recovery

Main already supplied the shared readiness model, BIOS inventory, adapter
inspectors and emulator resolver. Historical `dd2e4a18` supplied adapter mapping
without a Game Details consumer. This recovery adapts those missing mappings
and supplies the focused Blue consumer. It supersedes historical unbound
profile evidence and coarse-Verified fallbacks, which could misleadingly mask
missing or conflicting inspection evidence. It does not wholesale cherry-pick
the historical add/add conflict.

Tests use synthetic firmware bytes, local synthetic DAT records and disposable
profiles. They cover exact evidence, missing/ambiguous/conflicting facts,
profile/platform binding, freshness, partial Flycast evidence, System Card
policy uncertainty, core declarations and the read-only egui journey.
