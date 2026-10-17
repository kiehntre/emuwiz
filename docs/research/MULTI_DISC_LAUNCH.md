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

## Loose PlayStation CUE/BIN discs

A loose `.cue` in a folder whose existing folder evidence says PlayStation
(or a PlayStation source root) is one library entry; the files it references
are its companions and never entries of their own. Other platforms' sheets are
still not catalogued.

| Step | Rule |
| --- | --- |
| Accepted layouts | Every `FILE` is a quoted, `BINARY` reference inside the sheet's folder, present, a regular file and not a symlink. Track 1 is the only data track, `MODE2/2352` or `MODE1/2352`, whole sectors; any further tracks are `AUDIO`. INDEX 00 (in-file pregap) and PREGAP (synthetic) are accepted where the existing data-track parser places them. Two sheets in one folder sharing a referenced file are both refused |
| Refused | Missing or unsafe references, symlinks, `MODE1/2048`, audio first, several data tracks, non-BINARY files, partial sectors, malformed sheets. In a PlayStation folder these count as invalid media structure |
| DAT verification | A sheet's own match never stands for the disc. The audit judges the sheet plus every referenced file with the existing set classifier: verified only when one catalogue entry is `Complete` and every file matched that entry exactly by hash. Otherwise any positive sheet verdict is withdrawn. This applies to every `.cue` an audit sees, so it can only remove a claim |
| Freshness | The fingerprint each file had while it was hashed (device, inode, size, modified and change time) is saved with the disc's identity. Any later difference in any file - rewritten, replaced even with an equal size and restored timestamp, removed - makes the identity stale everywhere it is read: Game Details, list badges, per-source counts, the playing-library export, platform evidence, disc order, Preview and Launch. Checks read metadata only. Files must sit beside their sheet |
| Remounts | A file with the same path, size and modification time but another device or inode (what a remounted network or FUSE drive reports) is "identity uncertain", not "changed": still not verified, no disc order and no playlist, and Game Details says the drive may have been remounted. **Re-check this disc** re-hashes only that sheet and its files, with the hash cache off, against the configured catalogue that verified it. Verification is renewed only if the same catalogue version verifies the whole disc as the same entry; otherwise the saved evidence is left as it was. Nothing re-checks automatically |
| Launch | A catalogued sheet that still passes the inspector is CUE/BIN content, which only DuckStation accepts (RetroArch is never shown a playlist for it); DuckStation's preflight re-inspects the release and re-reads the data track's serial. A multi-disc playlist carries the audited fingerprints of every disc file: Preview and Launch refuse if any is no longer that object, and Launch also refuses any change since Preview |

## Current limits (not claimed)

- **CHD discs cannot be verified against Redump yet.** Redump hashes raw
  2352-byte tracks; nothing in EmuWiz extracts raw tracks from a CHD (the
  existing readers return track 1 as cooked 2048-byte sectors), and a CHD
  header hash is not content identity. That needs a separately approved
  track-extraction design.
- **A sheet that becomes incomplete after it was catalogued keeps its entry**,
  like any existing file the scanner no longer classifies; its identity goes
  stale and Launch refuses it with the reason.
- **A same-inode rewrite inside one timestamp tick with both times restored**
  is not detected without re-hashing - the same residual limit the audit cache
  documents.
- **Order is catalogue-proven for floppies and tapes, but playlists are PS1
  only.** Their order shows in Game Details; Launch stays single-media.
- **Not validated against real emulators.** That DuckStation accepts
  `-batch -- <playlist.m3u>` with absolute paths and `#` comment lines, and that
  each m3u-listing PS1 core accepts absolute paths, is unverified here.
- Other emulators and platforms keep single-disc launch and say so.
