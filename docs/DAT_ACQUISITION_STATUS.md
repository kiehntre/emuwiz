# Current DAT acquisition capability

DAT acquisition is complete in the current Sources → DATs workflow.

- No-Intro: official browser-assisted DAT-o-MATIC download, followed by
  bounded import, validation, revision comparison, explicit activation,
  history, and rollback.
- TOSEC: official browser-assisted release-pack download, followed by bounded
  import, validation, revision comparison, explicit activation, history, and
  rollback.
- MAME: existing managed source and update machinery.
- Redump: existing managed BIOS and game/disc sources and update machinery.
- GitHub: explicit user-selected raw or release-asset source with provenance
  and reviewed/user-provided trust classification.
- Custom HTTPS: explicit user-provided source with HTTPS, size, redirect,
  timeout, SSRF, staging, hashing, validation, snapshot, and rollback
  safeguards.
- Local: DAT file, folder, No-Intro pack, and TOSEC release-pack imports.

DAT-o-MATIC and TOSEC browser-assisted acquisition are intentional provider
constraints: their official browser flows are supported, while EmuWiz does not
scrape forms, automate anti-bot challenges, or invent undocumented download
endpoints.
