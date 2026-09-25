# GUI-v2 novice first-run audit

Audited at `f89a0a4586541c3d4cc7c922cd0ed40a44306107`.

| Area | Finding | Foundation response |
|---|---|---|
| First launch | Setup/Doctor explains only “find/check games”, not the product stages. | Expand the existing dismissible welcome into a compact product map and suggested next steps. |
| Empty library | Sources and Games can be empty before a folder is configured; “add a Games folder” does not explain read-only scanning or separate identification. | Explain add → scan/inspect → identify, with direct routes. |
| Jargon | DAT, BIOS/Firmware, CHD, parent/clone, provenance, verified/external evidence, and MAME preservation vs playing-library concepts require prior knowledge. | Add glossary-style “What is this?” definitions to welcome and Settings. |
| Workflow choice | Organisation, emulator setup, metadata/artwork, and cheats/mods are discoverable but their order and optionality are unclear. | Show a suggested path while keeping every sidebar route available. |
| Advanced access | Advanced tools are reachable, but novices may not know dismissal never hides them. | State this explicitly. |
| Preferences | Welcome dismissal is persisted; beginner guidance itself cannot be disabled. | Add a persisted “Show beginner hints” setting, enabled by default. |

The skeleton is not a wizard. It is a dismissible guide with direct navigation to
Sources, Games, Problems, Emulator Setup, Artwork & Metadata, Mods & Cheats,
Organisation, and Advanced. Showing it changes no data. Help is deterministic and
local: existing Mr Wiz guidance remains the page-level hint mechanism, while the
welcome and Settings provide plain-language definitions. Technical detail remains
available in Advanced Details and specialist pages.
