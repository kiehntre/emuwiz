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

## Local file rules

- A local DAT is Logiqx XML or ClrMamePro text. A ZIP, gzip, 7-Zip, RAR or
  xz file chosen as a DAT is refused by its container signature, with the
  advice to extract the `.dat`/`.xml` inside it or, for a DAT-o-MATIC
  download, to import it as a No-Intro pack ZIP. Nothing is decompressed in
  that path. A file with no `<datafile>` root, ClrMamePro header or `game (`
  block is refused as "not recognised as a DAT catalogue" rather than
  accepted as a valid empty catalogue.
- No-Intro and Redump identity is read from the same header fields (`name`,
  `author`, `description`, `version`, `homepage`, `url`) in both Logiqx and
  ClrMamePro form, so a DAT-o-MATIC "clrmamepro" download imports as
  No-Intro like its XML twin.
- A managed MAME software-list update whose downloaded bytes equal the
  installed snapshot reports "Up to date" and records the compared commit; it
  never publishes a new snapshot or invalidates saved verification results.
  The check itself still compares against the newest commit of the whole MAME
  repository, so "Update available" can appear after a commit that did not
  touch the list.

DAT-o-MATIC and TOSEC browser-assisted acquisition are intentional provider
constraints: their official browser flows are supported, while EmuWiz does not
scrape forms, automate anti-bot challenges, or invent undocumented download
endpoints.
