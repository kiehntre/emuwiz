# Upgrading EmuWiz

> **Version note.** This guide describes the current development
> version of EmuWiz (the workspace version is 0.10.0). Several features it
> mentions (for example the GUI v2 interface, the Wii U conversion queue and
> the preflight tool) were added after the `v0.9.0` git tag of 2026-09-13.

Upgrade carefully. EmuWiz keeps your data separate from the program, but an
upgrade can reveal old paths, an older database, drives that are not mounted,
or an operation that never finished.

## Before you start

Close EmuWiz, then back up the things that describe your library and choices:

- your EmuWiz config folder, including `config.toml`;
- `library.sqlite3`, your catalogue database;
- DAT selections and their snapshots, and any provider choices;
- emulator profiles and the programs you chose for each emulator;
- history and recovery records: rename, repair, mod, cheat and conversion
  records and the conversion queue;
- managed emulator install records;
- custom artwork and provider settings.

Thumbnails, scan fingerprints and temporary provider data can be rebuilt. They
are handy to keep, but they do not replace the catalogue, configuration or
history.

Do not edit SQLite files by hand or copy rows between databases. If a database
upgrade is needed, let EmuWiz do it, ideally on a copy first.

## Where your data lives

New installs use these folders:

```text
~/.config/emuwiz
~/.local/share/emuwiz
```

Earlier ArchiveFS versions used:

```text
~/.config/archivefs
~/.local/share/archivefs
```

If the EmuWiz folder does not exist and the ArchiveFS one does, EmuWiz keeps
using the ArchiveFS folder where it is. It does not copy or merge files. If
**both** folders contain real data, EmuWiz does not combine them; back up
first and decide which one is the real library.

`EMUWIZ_CONFIG_HOME` and `EMUWIZ_DATA_HOME` override these places when they are
absolute paths. A changed environment or an unmounted drive can make a healthy
library look empty without deleting anything.

## The database

The catalogue database upgrades forward only, in steps. The current schema is
**27**; the `v0.9.0` tag used schema 16, so upgrading from it applies several
steps. Later versions may use a higher number.

Starting EmuWiz does not upgrade the database. At schema 27 it can make one small
write to a saved Favourites item ([details](FAVOURITES_AND_COLLECTIONS.md)). If the
database is older than this version, the
app shows an **Outdated** state and does not change it. Upgrade it in the app
from **All Tools > Database status > Scan and upgrade library**. A plain
`emuwiz-cli library-scan` refuses an outdated database and does not upgrade it.
Without a desktop session, `emuwiz-cli library-upgrade` reports what would
happen and changes nothing; `emuwiz-cli library-upgrade --confirm` takes the
same verified backup and upgrades.
Before upgrading, EmuWiz makes a verified backup copy next to the database,
named like:

```text
library.sqlite3.schema-<old>-before-<new>.backup
```

and keeps it after the upgrade succeeds. If it cannot make that backup, it does
not touch the database. A database from a newer EmuWiz than the one you are
running is never downgraded; open it with a newer EmuWiz instead.

## Check first with the preflight tool

If you are not sure which folders are in use, run the read-only preflight from a
source checkout of EmuWiz:

```sh
python3 scripts/qa/upgrade_preflight.py
python3 scripts/qa/upgrade_preflight.py --backup-manifest /tmp/emuwiz-preflight.json
```

It reports the active EmuWiz and ArchiveFS folders and can write a backup
manifest (a list of what to back up). It does not copy files or change the
database. Add `--json PATH` for a machine-readable report, or use
`--config-root`, `--data-root`, `--legacy-config-root` and `--legacy-data-root`
to inspect other locations. Run it with `--help` to see every option.

### Schema 27: Favourites and collections

Schema 27 adds two tables and changes nothing that already exists. It is still
a one-way step, so choose when to take it:

- It needs the explicit upgrade above. A build at schema 27 will not open a
  schema 26 library until that has been done, and never does it by itself.
- After the upgrade, every build made for schema 26 refuses the library. That
  includes older installs and any build from a branch without this migration.
- `emuwiz-cli library-upgrade --confirm` upgrades without scanning.
  **Scan and upgrade library** also scans every game folder, so do not use it
  while a drive, mount or migration is in doubt. The checks to run before and
  after are in
  [Upgrading a live library without a scan](FAVOURITES_AND_COLLECTIONS.md#upgrading-a-live-library-without-a-scan).
- At schema 27, EmuWiz can make one small background write to a saved
  Favourites item after the library loads; see that page.

The backup holds the whole library, so going back loses every later change to
it, not only Favourites: scans, imports, metadata, verification results and more.
What that involves is in
[Favourites and collections](FAVOURITES_AND_COLLECTIONS.md#going-back-to-schema-26).

## Upgrade checklist

1. Close EmuWiz and make the backups above.
2. Confirm which config and data folders are active.
3. Keep the old install around until the new one opens your library.
4. Start the new version. If it shows **Outdated**, use **Scan and upgrade library**
   (or `emuwiz-cli library-upgrade --confirm`); it makes the backup and upgrades the
   database. Let it finish.
5. Check **Sources**, **Emulator Setup**, your DAT choices and **History**
   before you scan or organise anything.
6. Run **Check Games** only when your game folders are connected.

If you installed with `./install.sh`, close EmuWiz first and run `./install.sh`
again from the new folder. Your configuration is not touched.

Do not start from a blank profile to "fix" an upgrade unless you really want to
start over. An empty library usually means the wrong folder is being read.

## Troubleshooting

### "My game drive is unavailable"

Reconnect or mount the drive at the expected path. Do not scan into a new
location while the old one is missing. For drives under `/mnt` or `/media`,
**Setup & Doctor** may say the mount is unavailable: restore the mount first, or
review the path and choose a replacement on purpose.

### "Database upgrade required"

This is the **Outdated** state. Close other EmuWiz windows, back up
`library.sqlite3`, then start a scan and let EmuWiz upgrade it. If it fails,
keep the backup and the error message. Do not run SQL by hand.

### "Both ArchiveFS and EmuWiz data found"

EmuWiz will not merge them. Use the preflight tool to compare, decide which
folder holds the real catalogue and history, and keep the other until you are
sure.

### "Emulator path no longer exists"

Open **Emulator Setup** and reconnect the drive or choose the installed
emulator again. Another program with the same file name is not automatically the
same emulator.

### "DAT path missing"

Restore the DAT or select it again in **DAT Management**. Missing verification
data is not replaced by guessing from file names.

### "An interrupted operation needs attention"

Open **Problems & Repair** or **History**, read the details and use only the
recovery or undo action it offers. Do not delete journals or temporary files by
hand while an operation is unresolved. A Wii U conversion that was running when
EmuWiz closed shows as interrupted in **Game Details** and does not restart by
itself.
