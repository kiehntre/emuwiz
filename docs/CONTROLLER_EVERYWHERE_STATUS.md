# Controller Everywhere: status, follow-ups and acceptance

## What is covered
Shared controller layer (`gui_v2/pad_shell.rs`) for ordinary Blue pages: focus
movement, A press, B back, LB/RB page, X/Y previous/next section (Tools
included), Start → Big Picture; on-screen keyboard for text fields (staged,
Cancel never changes the field, secrets masked); in-app file/folder picker;
open dialogs are modal for the controller (focus cannot reach the page behind);
unrecognised dialog buttons need A held 0.6 s.

## NOT yet covered: native file dialogs (targeted follow-up)
Converted to the controller-drivable picker (`ui/picker.rs`): Blue Add source,
Sources Add folder/Browse, emulator executable and configuration folder, core
game-support folder.

About 110 other `rfd::FileDialog` call sites in ~55 files still open the native,
blocking dialog, which a pad cannot drive. Convert in this priority order
(user-facing workflows first); each conversion is the same mechanical
`picker::request` / `picker::take` change:

1. Saves/cheats/mods used from a game: `pcsx2_page.rs` (4), `launch_readiness_page/{retroarch,pcsx2,melonds}_cheats.rs`,
   `cheats_mods/*` (mgba, melonds, flycast, ppsspp, controller, adf_trainer), `user_cheat_import_page.rs` (6),
   `cheatbase_page.rs`, `emulator_setup/{shadps4,cemu_keys}.rs`, `emulator_download_page/side_by_side.rs`.
2. Setup and verification data: `dat_sources_page.rs` (11), `gui_v2/native_workflows.rs` (remaining DAT choosers),
   `gui_v2/setup_portability.rs` (3), `identity_providers_page.rs`, `gui_v2/dat_name_review.rs`, `dat_duel_panel.rs`, `playing_library_page.rs`.
3. Artwork and media: `platform_artwork_manager.rs`, `gui_v2/local_artwork_folders.rs`, `gui_v2/bezel.rs`, `gui_v2/native_media_preview.rs`, `gui_v2/game_documents.rs`.
4. Organise/convert tools (desktop-leaning): `zip_converter_page.rs` (5), `optical_conversion_page.rs` (4),
   `rom_organisation_page.rs` (4), `exact_duplicate_review_page.rs` (3), `gui_v2/r2_rename/view.rs` (3),
   `gui_v2/hackhash.rs` (3), `local_mod_package_page.rs` (6), `dolphin_texture_mod_page.rs` (5), the rest.

Other known limits: the keyboard appends/deletes at the end only (no caret
movement, no newline key); multiline fields are replaced as a whole.
Feature coverage cannot be called complete until the list above is done.

## Real-controller acceptance (Saltbox)
Environment found 2026-11-05: `saltbox26`, a QEMU VM. Display: Xorg :0 (lightdm,
X11). The tty/ssh sessions have no DISPLAY; use `DISPLAY=:0 XAUTHORITY=~/.Xauthority`.
The only gamepad is `Sunshine X-Box One (virtual) pad` (`/dev/input/event14`,
group `input`, user is a member), i.e. the Moonlight client's controller. The
display is viewed through the Moonlight/Sunshine stream. Nobara is not involved
in this run.

Run (scratch HOME, never the live library):

    export HOME=$(mktemp -d); DISPLAY=:0 XAUTHORITY=/home/davedap/.Xauthority \
      <worktree>/target/debug/emuwiz-gui

Checklist, with the Moonlight pad only (no mouse/keyboard) unless stated:
1. Navigation: D-pad/stick move a visible focus ring; A presses; B goes back; LB/RB scroll; Y/X change section through all eight incl. Tools.
2. Games: Library → focus a game → A opens details → B back → Verify Games opens.
3. Virtual keyboard: Library search or a Sources path field → A opens keyboard; type with A, X delete, Y shift, Start done; B cancel leaves the field unchanged; a ScreenScraper password field is masked and starts empty.
4. Dialogs: open a confirmation (e.g. scan prompt / undo): stick cannot reach the page behind; tapping A on a non-Cancel button does nothing, holding A ~0.6 s shows the bar and activates; moving focus mid-hold cancels.
5. Picker: Sources → Add a games folder… opens the in-app picker; navigate to a USB/other folder, "Use this folder"; B cancels with nothing added.
6. Big Picture: Start from Library opens it; B/exit returns; launching an emulator suspends pad input in EmuWiz and it resumes on return without a stray press.
7. Reconnect: drop and rejoin the Moonlight session (or pad): input resumes, no stuck direction.
8. Mouse/keyboard still work alongside (click page behind a dialog works after using the mouse; Ctrl+C/V clipboard unaffected by controller text entry).
