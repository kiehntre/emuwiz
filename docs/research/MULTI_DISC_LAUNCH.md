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

## Current limits (not claimed)

- **No verified disc-set evidence is produced in real libraries yet.** Nothing in
  production writes the `media_topology_evidence` table, so sets come from the
  catalogue fallback, which rests on filename evidence. The composer never treats
  that as a verified order, so real libraries get the honest single-disc launch
  plus Game Details guidance today. A verified-topology producer (for example
  from trusted DAT matches) is the remaining blocker for playlist launches.
- **Not validated against real emulators.** That DuckStation accepts
  `-batch -- <playlist.m3u>` with absolute paths and `#` comment lines, and that
  each m3u-listing PS1 core accepts absolute paths, is unverified here.
- Other emulators and platforms keep single-disc launch and say so.
