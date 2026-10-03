# EmuWiz quickstart

> **Version note.** These instructions describe the current development
> version of EmuWiz (the workspace version is 0.9.0). The packaged-release
> workflow they rely on (the `.tar.xz` archive, `install.sh`, checksums and
> optional SBOM and signature) was added after the `v0.9.0` git tag of
> 2026-09-13, so file names below use `<version>`. Replace it with the version
> in the file name you downloaded.

EmuWiz is a local-first game-library tool. It catalogues the folders you
choose, checks game identity and launch prerequisites, and shows a preview
before it changes anything.

This is a Linux-only, pre-1.0 release. The download contains the application
only: no ROMs, BIOS or firmware files, saves or configuration.

## 1. Verify, extract and start

Keep the downloaded archive and its checksum file together, and check the
archive before you extract it:

```sh
sha256sum -c emuwiz-<version>-linux-x86_64.tar.xz.sha256
tar -xf emuwiz-<version>-linux-x86_64.tar.xz
cd emuwiz-<version>-linux-x86_64
./bin/emuwiz
```

`bin/emuwiz` is the EmuWiz application (the GUI v2 interface). `bin/emuwiz-cli`
is the command-line tool; `./bin/emuwiz-cli --version` prints its version. You
can run both straight from the extracted folder with no installation.

To install for your own user instead, run `./install.sh` from the extracted
folder. It needs no `sudo`, puts the programs in `~/.local/bin`, never touches
an existing configuration, and can be undone with `./install.sh --uninstall`.
Close EmuWiz before re-running it to upgrade. Run `./install.sh --help` for the
options.

See [Verify a release](VERIFY_RELEASE.md) for the optional signature and
software-inventory (SBOM) checks.

## System requirements

- A Linux desktop with an X11 or Wayland session and a working graphics stack
  (OpenGL/EGL).
- Write access to your normal user config and data folders (XDG locations).
- Use the download whose file name matches your machine, for example
  `x86_64`. Do not assume an x86_64 download runs on another architecture.
- Windows and macOS are not supported by this release.

Some features use optional tools: `ratarmount`/FUSE, `7z`, `unrar`, a document
viewer, your emulators, Flatpak or AppImage emulators, RomM, and online DAT or
artwork services. A missing optional tool does not stop EmuWiz starting; the
related feature just reports what is missing.

## 2. Follow Setup & Doctor

On a fresh install, open **Setup & Doctor** from the sidebar. It explains what
is set up and what still needs attention, and it is useful even with an empty
library.

A sensible order:

1. Open **Sources** and add the folder or folders that contain your games.
   Adding a folder does not move or rename anything in it.
2. Let EmuWiz scan. It builds its own catalogue and leaves your files alone.
3. Open **Emulator Setup** and choose the emulator you want to use. Finding a
   program is not proof that a game will start.
4. Add the system software the emulator needs on **BIOS / Firmware**. EmuWiz
   reports what it finds, but it does not download BIOS or firmware. Get those
   from a source you are allowed to use.
5. Use **Check Games** to review unknown, damaged or mismatched items.
6. Select a game and use **Launch**. EmuWiz re-checks the game, its files, the
   emulator and the required firmware first.

## 3. Find your way around

The sidebar groups the main areas:

- **Home**, **Games**, **Platforms** and **Museum** to browse your library.
- **Setup & Doctor**, **Emulator Setup** and **BIOS / Firmware** for setup.
- **Check Games** and **Problems & Repair** to find and fix problems. Repairs
  show a preview first, and **Problems & Repair** also has a review of games
  that EmuWiz can no longer find.
- **Duplicates** for exact duplicates and for the same game stored in two
  formats (for example CUE/BIN and CHD, or different N64 byte orders). Extra
  copies go to a recoverable holding folder, never straight to deletion.
- **Multi-disc games** shows which disc sets look complete. It only reads;
  nothing is changed or launched from it.
- **Storage** shows where your space is going. It only looks.
- **Organisation** plans a separate, tidy library (see below).
- **Converter**, **Tape Inspector** and **Mods & Cheats** are specialist tools.
- **Sources**, **DAT Management** and **RomM Library** manage where information
  comes from. **RomM Library** is a read-only browser for a RomM server you
  already run.
- **Saves & States**, **Activity**, **History** and **Settings**. **Advanced**
  holds specialist tools and opens the specialist interface in a separate
  window; opening it changes nothing.

Mr Wiz tips and plain-language messages explain results as you go. Technical
detail is kept under **Details** or **Why** sections.

## Organisation and linked libraries

EmuWiz keeps your original collection separate from any output library:

- **Rename** changes names in the folder you chose, so it changes your
  originals. Preview it first. It is journaled so it can be reviewed in
  **History**.
- **Move** changes where original files live. Treat it as a real filesystem
  change.
- **Linked library** makes a separate view using links. Originals are not moved
  or renamed, but the destination must be reachable by the program using it.
- **RomM**, **ES-DE** and **RetroDECK** outputs are separate, explicit
  workflows. They create links or published views; they do not rewrite your
  original files and do not change a RomM server.

Do not run an organisation action just to try a folder. Start with a source and
a read-only check.

## Converting a Wii U disc image

For a Wii U `.wud` or `.wux` file, open the game's **Game Details**, expand
**Disc & ROM evidence**, and use **Convert this disc image**. You preview the
result, confirm, and EmuWiz converts it in the background through a queue that
survives a restart. Your original file is never changed or overwritten, and the
result can be reviewed in **History & Undo**.

## Mods and cheats

In **Mods & Cheats**, pick a game, look at the compatibility notes and preview
the proposed change. Apply it only when the preview shows the exact files and
action. Changes use the same history and rollback paths as other operations
where the emulator supports them. A mod that only matches by file name is never
treated as proof it fits your game.

## Saves & States

Keep these apart:

- a **game save** is data written by the game;
- a **memory card** is emulator storage that holds saves;
- a **savestate** is a snapshot of an emulator session;
- **system storage** is emulator configuration or virtual storage.

Savestates can depend on the emulator, its version and settings, so they are not
guaranteed to work after an emulator update or in a different emulator. The
**Saves & States** page looks and explains; it does not change saves. Export and
restore for supported PS1/PS2 memory cards live in the PS1/PS2 Save Vault in the
specialist interface. EmuWiz does not offer a universal save restore.

## MAME and arcade data

Arcade verification data can come from a DAT you import or from a MAME you
choose. Imported data works without an installed MAME, and EmuWiz marks it as
imported rather than official. Import it from **DAT Management** under **Import
verification data**.

## If something is not ready

Trust the wording in **Problems & Repair**, **Setup & Doctor** and **Emulator
Setup**. In particular:

- **BIOS needed** means get and add the required system software;
- **selected path missing** means reconnect the drive or choose the emulator
  again;
- **DAT path missing** means restore or reselect the DAT, not rescan blindly;
- an interrupted operation should be reviewed in **History** before you start
  another one.

Your files are not changed just because a check reports a problem.

Upgrading from an earlier version? Read [Upgrading](UPGRADING.md) first.
