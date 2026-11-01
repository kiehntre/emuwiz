# Favourites and collections

Favourites and user-created collections keep a person's own lists of games.
Membership is user data: it never changes a game file or a catalogue row, and it
is not evidence of what a game is.

## Using them

- Select the star on any game in Browse & Play to favourite it, or again to
  remove it. The selected-game panel and Game Details have the same star, plus a
  **Collections** menu.
- Above the shelf, choose **All games**, **Favourites** or a collection. Search,
  the system chips and the readiness filters then work inside that list, and a
  sort control appears: title, system or recently added.
- A collection can be renamed or deleted. Deleting one cannot be undone, but its
  games stay in the library. Favourites cannot be renamed or deleted.

## What is saved

Schema 27 adds `game_collections` and `game_collection_items` to
`library.sqlite3`, so database backups cover them. Migration 27 changes no
existing table, row or index.

A favourite belongs to one catalogue entry, not to a title, so two copies with
the same title are separate. Hover over a card to see which copy it is.

An item finds its game in this order:

1. the same catalogue row (same id and creation time), wherever it now lives;
2. the row at the same source and relative path, with the same size when both
   sizes are known.

If the row found has been replaced by another, the page shows the replacement.
A title match is never enough, and a reused id is never accepted: a saved game
and a current game can even hold the same id and stay separate items. An item
with no match stays saved and appears under **no longer in the library** with
its saved title, system and file name. It stays there until a scan finds the
file again or you remove it. A game whose row remains but whose file is missing
stays in the list, marked as needing attention.

Most replacement links are worked out afresh each time the library loads and
can stop applying, so they only change what is shown. An item is saved against a
different row only when that is a fact: EmuWiz's own rename journal records the
move, or a new row sits at the item's own place and size. That small write
happens in the background after the library loads, and only when such an item
exists. It touches only the item. If it follows a Review restore, it can make
that restore's Rollback refuse, because the database has changed since.

Discovery import Undo refuses a game whose saved item points at the imported
row, and says so. Remove it there first, then Undo. An item that has not yet been
saved against that row does not block Undo; it shows as no longer in the library
afterwards. Forget and missing cleanup also remove the row and leave the item
showing that way.

## Upgrade and compatibility

**Older builds cannot open an upgraded library.** After the upgrade, every build
made for schema 26 refuses it, and the only way back restores the whole library
to its state before the upgrade.

Schema 27 is one-way; see [Upgrading](UPGRADING.md#schema-27-favourites-and-collections).

- A schema 27 build does not open a schema 26 library until you run the explicit
  upgrade. There is no automatic upgrade.
- A schema 26 build refuses a library that has been upgraded.
- The upgrade writes and verifies a backup before changing anything, and keeps
  it afterwards. From schema 26 it is normally
  `library.sqlite3.schema-26-before-27.backup`. If that name is taken it gets a
  numeric suffix, so always use the path the upgrade printed.
- If the upgrade stops or fails, the migration rolls back whole. The library
  stays at schema 26, the backup stays, and the upgrade can be run again.

Install this after any build you still intend to run against schema 26, and move
other builds to a base that includes migration 27 before pointing them at the
upgraded library. Another branch that adds its own migration must take number 28.

## Upgrading a live library without a scan

Use `emuwiz-cli library-upgrade --confirm` from the schema 27 build. Do not use
**Scan and upgrade library** while any source drive, mount or migration is in
doubt: that action also scans every game folder, and a scan with a drive absent
can mark games missing.

The CLI command opens only the database and its backup. It reads no game folder
and changes no mount. Migration 27 changes no existing row, so paths, missing
flags and platforms are as they were. A later scan is optional and is not needed
for Favourites.

Before upgrading:

1. Confirm the release, library and any pending database work are ready, and
   that nothing else is using the library: no EmuWiz window, no worker, no scan.
2. Use the schema 27 build by its full path, and record its SHA-256. An
   installed `emuwiz-cli` is a different program, and `--version` does not tell
   them apart: schema 26 and 27 builds can both print `0.10.0`. Run the upgrade
   from an ordinary login shell. A different `XDG_DATA_HOME` or `HOME` selects a
   different database folder.
3. Run `<build>/emuwiz-cli database-check`. Stop unless it is healthy.
4. Check that no `library.sqlite3-wal`, `-shm` or `-journal` file exists, and
   that the database's folder has room for one more copy of it.
5. Run `<build>/emuwiz-cli library-upgrade` without `--confirm`. It leaves the
   library unchanged. Stop unless it names the expected database and says schema 26 would
   become schema 27.
6. Record the counts in [Checking the result](#checking-the-result).

Then run `<build>/emuwiz-cli library-upgrade --confirm` once and keep its output.
It prints the backup path and SHA-256.

This command, and the preview in step 5, create small working files: an
upgrade lock and `undo-seals.sqlite3` beside the database, and a writer entry
under `$XDG_RUNTIME_DIR/emuwiz`. It leaves `library.sqlite3.upgrade.lock` behind;
an empty lock file is normal.

If it fails or is stopped, run `database-check` before anything else.

- If it reports schema 26 and a clean quick check, with no live rollback journal,
  run the same command again. In testing, a kill during the backup and a kill
  during the migration both ended this way, with every record unchanged.
- If it reports a hot rollback journal, **stop**. EmuWiz refuses that database
  rather than recover it silently, so retrying will not help. Leave the files as
  they are, copy the database and journal together, and follow
  [database recovery](DATABASE_RECOVERY.md).

A killed run can leave a partial `.backup` file, a receipt, and a
`library.sqlite3-journal` that `database-check` reports as not hot. The retry
writes its backup under a new name such as `.backup.1`, so **only the path printed
by the upgrade that finished is a backup you can restore from**. Keep the partial
file until you have checked the finished one.

### Checking the result

None of this scans, and only the first two commands use EmuWiz:

```sh
<build>/emuwiz-cli database-check
<build>/emuwiz-cli library-status
sqlite3 -readonly library.sqlite3 "PRAGMA user_version; PRAGMA integrity_check;
  SELECT count(*) FROM archives;
  SELECT count(*) FROM archives WHERE last_verified_missing_at IS NOT NULL;
  SELECT count(*) FROM scan_runs;
  SELECT count(*) FROM game_collections;
  SELECT count(*) FROM game_collection_items;"
sha256sum "<backup path the upgrade printed>"
```

Expect schema 27, integrity `ok`, the archive, missing and scan-run counts
unchanged, one collection and no items. The SHA-256 must match the upgrade's
output, and the same counts run against the backup must report schema 26. Leave
`library-scan` for a time when every source drive is ready.

## Going back to schema 26

**The backup is the whole library, not just Favourites.** It is every table of
`library.sqlite3` as it stood just before the upgrade. Restoring it removes
every change made to the database since: scans, imported games, metadata edits,
verification results, platform changes, the catalogue's record of organisation
moves, and all Favourites and collections. Nothing is merged back. Treat it as a
last resort, and only after working out what it would cost.

Going back also means going back to a schema 26 build. A schema 26 build refuses
a schema 27 library, and a schema 27 build refuses the restored schema 26 library
until it is upgraded again. EmuWiz has no downgrade: there is no automatic route
either way, and Review restore in the app refuses this backup because it accepts
only backups at the running build's schema.

Rename and organisation journals, and game files, live outside the database and
are not rewound. Anything moved after the upgrade stays moved, and its journal
stays, while the restored catalogue still expects the old place. That mismatch
has to be reconciled by a later scan, when your drives are ready.

### Before deciding, compare

Keep the output of these read-only counts from just after the upgrade. Then, with
EmuWiz closed, run the same query against the current library and against the
printed backup, and compare them. Every difference is something a restore loses.

```sh
cat > /tmp/emuwiz-impact.sql <<'SQL'
SELECT 'catalogue entries',count(*) FROM archives;
SELECT 'marked missing',count(*) FROM archives WHERE last_verified_missing_at IS NOT NULL;
SELECT 'newest catalogue entry',max(id) FROM archives;
SELECT 'scans',count(*) FROM scan_runs;
SELECT 'platform assignments',count(*) FROM platform_assignments;
SELECT 'source folders',count(*) FROM source_folders;
SELECT 'import receipts',count(*) FROM discovery_import_operations;
SELECT 'DAT identity results',count(*) FROM library_dat_identities;
SELECT 'set audit results',count(*) FROM dat_set_audit_results;
SELECT 'verified facts',count(*) FROM verified_identity_facts;
SELECT 'metadata enrichments',count(*) FROM screenscraper_enrichments;
SQL
sqlite3 -readonly "<backup path>" < /tmp/emuwiz-impact.sql > before.txt
sqlite3 -readonly library.sqlite3 < /tmp/emuwiz-impact.sql > now.txt
diff before.txt now.txt
sqlite3 -readonly library.sqlite3 "SELECT count(*) FROM game_collection_items"
find . -maxdepth 2 -type f -newer "<backup path>" ! -name 'library.sqlite3*' ! -name 'undo-seals*'
```

The last two show the Favourites and collection entries that would go, and the
journals and receipts written since. Counts can hide a change that keeps a total
the same, such as an edited title, so treat an unchanged count as a lower bound.

Prefer fixing forward: a defect in Favourites does not need the database put
back, because the feature's tables stand apart from the rest of the library.

### Keeping your Favourites

The set-aside database in step 3 below holds them. To also keep a readable list,
export it first. This only reads:

```sh
sqlite3 -readonly -json library.sqlite3 "SELECT c.kind, c.name, i.title, i.platform,
  i.archive_id, i.archive_created_at, i.source_folder_id, hex(i.relative_path) AS rel,
  i.size_bytes, i.added_at
  FROM game_collection_items i JOIN game_collections c ON c.id = i.collection_id" \
  > collections-export.json
```

There is no supported import. Each item carries its own identity, so copying the
two tables into a re-upgraded library works in testing without touching any other
table, and items whose games still exist reattach. Items for games added after
the upgrade stay under **no longer in the library** until those games return. It
still means writing to the database by hand, which [Upgrading](UPGRADING.md) rules
out, so ask before doing it.

### Restoring by hand

1. Close every EmuWiz window and process, and stop anything that uses the
   library.
2. Check that no `library.sqlite3-wal`, `-shm` or `-journal` file exists beside
   the database. If one does, stop and follow
   [database recovery](DATABASE_RECOVERY.md).
3. Set aside the current database, not deleting it:
   `mv library.sqlite3 library.sqlite3.schema-27-set-aside`
4. Copy the backup into place, keeping the original:
   `cp -p "<backup path the upgrade printed>" library.sqlite3`
5. Compare `sha256sum` of `library.sqlite3` with the SHA-256 the upgrade printed.
   The backup is a fresh SQLite copy, so that value is not the hash of the
   library from before the upgrade.
6. Run `database-check` and `library-status` from the schema 26 build by its full
   path. Expect schema 26, a clean quick check and the counts from before the
   upgrade.
7. Do not scan until your source drives are ready.

The set-aside copy is then the only record of what the restore removed. Taking
schema 27 again starts with empty Favourites.
