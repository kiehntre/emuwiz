# Multi-disc launch: playlist handoff and Game Details guidance

EmuWiz launches a multi-disc game with an EmuWiz-managed M3U playlist only when
every condition below holds; otherwise the ordinary single-disc launch is kept
and Preview, Game Details and Gamer say why. A playlist only lets the emulator
offer its own disc-change controls. EmuWiz never changes discs in a running
emulator.

| Condition | Rule |
| --- | --- |
| Emulator | DuckStation (standalone), or a RetroArch core whose own installed `.info` lists `m3u` and the format of every disc |
| Installation | Native or AppImage. Flatpak keeps single-disc: its sandbox only sees the selected disc's folder |
| Platform | PS1 only (the reused `duckstation_multimedia_profile` / `retroarch_multimedia_profile` are PSX) |
| Disc set | Verified by the existing `media_handoff` composer: complete set, explicit disc numbers, no duplicates or conflicts, regular non-symlink files, every CUE's referenced files present |
| Freshness | Every disc matches the size and modification time of the last library scan; Launch re-captures every disc and refuses any change since Preview; every disc and the exact playlist bytes are re-checked just before spawn |
| Playlist file | Deterministic path under `<data dir>/launch-playlists`, staged by the existing managed writer (exclusive temp file, fsync, rename); never replaces a non-managed file; a symlinked or relative root refuses the launch instead of starting one disc |

## Where verified disc order comes from

After every saved DAT audit (DAT Sources) and every library verification run,
`Database::refresh_dat_disc_order` rebuilds catalogue-proven order from saved
identities and writes it through the existing `media_topology_evidence` path.
It reads only the database and file metadata.

| Step | Rule |
| --- | --- |
| Which files | Catalogued, present, loose images whose type has a media family: optical (`.iso`, `.chd`, legacy `.cue` rows), floppy (`.adf`, `.dsk`, ...) or tape (`.tzx`, `.tap`, ...). `.bin` track files are never media here |
| Identity | The image's own bytes matched exactly one DAT entry by SHA-256, SHA-1 or MD5; the identity is current (size unchanged, revision not marked stale); not an archive-member identity. Images whose sources disagree on the entry are refused |
| Position | Read only from that entry's title: exactly one `(Disc N)`, `(Disk N)` or `(Tape N)`, optionally `of M`. The unit must fit the image type. A title that names a side is not read. Local file names are never read |
| Total | The title's own `of M`, or the same installed catalogue revision listing exactly entries 1..M of that unit under the same base title. Otherwise the total is unknown: Game Details says so and Launch keeps one disc |
| Freshness | The entry must be in the saved inventory of the same catalogue revision; each image must match its last library scan. When the library loads, saved order is kept only while the source is still configured, its saved inventory is still that revision, every image still has that same current, verified identity, and no other current identity for the image is ambiguous or names a different entry. Everything that reads saved topology (Game Details, Preview, Launch, Media Sets, DuckStation cheat disc checks) sees only what passed |
| Conflicts | Gaps, repeats, two images claiming one position, any current identity naming another entry, or different releases or regions never complete a set. A saved order is never replaced by a disagreeing one while its own files are unchanged |

The release key is the catalogue title without the position token, so region,
revision and edition must match exactly. A disc catalogued only under a variant
title (for example `(Disc 3) (Rev 1)`) belongs to a different release and does
not count towards this one's total.

## Current limits (not claimed)

- **Real PS1 libraries mostly cannot be verified yet.** The scanner does not
  catalogue `.cue` sheets (they are disc companions), and a `.chd` never matches
  a Redump entry because Redump hashes the raw tracks. A typical CUE/BIN or CHD
  PS1 collection therefore gets no DAT identity per disc and keeps the honest
  single-disc launch. Loose images whose own bytes are in the catalogue (for
  example ISO-based catalogues, and TOSEC floppy or tape sets) do get verified
  order. Closing the PS1 gap needs either CUE cataloguing or a CHD track-hash
  bridge, both separate designs.
- **Order is catalogue-proven for floppies and tapes, but playlists are PS1
  only.** Their order shows in Game Details; Launch stays single-media.
- **Not validated against real emulators.** That DuckStation accepts
  `-batch -- <playlist.m3u>` with absolute paths and `#` comment lines, and that
  each m3u-listing PS1 core accepts absolute paths, is unverified here.
- Other emulators and platforms keep single-disc launch and say so.
