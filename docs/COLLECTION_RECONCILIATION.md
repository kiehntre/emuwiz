# Library files check: reconciliation

How EmuWiz explains the difference between its library and the files on disk.
There is one workflow and one Blue page for it, **Library files check**
(Problems & Repair). This document covers the evidence engine added to that
workflow. Everything described is read-only.

Sections 1 to 5 describe what is implemented. Sections 6 to 9 are designs and
proposals that are **not** implemented and need their own approval.

## 1. The problem

A library row is its path: `archives` is unique on `(source_folder_id,
relative_path)` and a scan looks a file up by path only. No content hash is
stored for a library file (`content_hash`, `archive_hash` and
`internal_listing_hash` are never written), and a scan keeps no inode. So when
a file moves outside EmuWiz, a scan adds a row for the new path and flags the
old row "file not found". Nothing connects the two. The flag clears only if the
same path comes back.

A library that has been reorganised by hand therefore carries rows that look
like lost games but may be old path records of files that still exist. This
module works out, from evidence, which is which.

## 2. One workflow, two kinds of evidence

Library files check has two parts on one page.

| Part | Question | Evidence | Engine |
|---|---|---|---|
| Files and entries | Which files are not in the library, which entries have no file, which files probably moved | A cryptographic hash stored for the entry that equals the hash of exactly one file; otherwise name and size as a hint only | `catalogue_reconciliation` (unchanged) |
| Where missing entries went | What became of each entry without a file, and which links work | EmuWiz's own rename journal with the inode it recorded, the filesystem's record of each file, scan history, matching name, size and date, and independently recorded hashes | `collection_intelligence` (new) |

The first part keeps its rule: it proposes a move only on a stored checksum.
EmuWiz stores no checksum for a library file, so on a real library that part
ends at "possibly moved". The second part does not require a checksum that was
never recorded. It uses the evidence that exists and states how strong each
piece is. Where a checksum does exist, the first part's matching still applies
and a person can still start its explicit checksum run.

Neither part changes anything, and the two do not share counts: the first
counts files and entries, the second counts entries without a file by evidence
level, and links are counted on their own in both.

Related code that is reused, not replaced:

| Existing | Used for |
|---|---|
| The rename journal files (`rename-transactions` and `rename-transactions-sealed-v1`) | EmuWiz's own applied moves. They are read directly, as history: the journal module's own reader exists to authorise Undo and refuses journals whose folders have since been redirected, which are the ones needed here. Nothing acts on a journal; a move counts only when the file it recorded is found on disk |
| `Database::open_read_only` | Reading the library |
| `identity_source::hashing`, `dat::archive::zip` | Bounded content comparison |
| `database::forget_missing` exports | The records it removed. Read directly, because that module's reader is private and accepts only exports written for the database being opened; Undo is never run |
| `catalogue_supersession` | Still hides replaced entries in the game list; it is display only and is not changed. It calls a name-and-size match strong; this document calls it weak. Aligning the two is a follow-up |
| `catalogue_reconciliation::walk` | Not reused: it lists regular files only and never records links, which this needs. Its guards are kept: private recovery folders are skipped, other filesystems are not entered and their rows are not judged |

## 3. Architecture

```
library.sqlite3 (read-only) ──┐
rename journals ──────────────┤
Forget Missing exports ───────┼─> sources ─> reconcile (records) ─┐
RomM hash cache (optional) ───┤                                    ├─> ReconciliationReport
library folders ─> inventory ─┴─> links (every symbolic link) ────┘      │
                                                                          ├─ emuwiz-cli collection reconcile
                                                                          └─ Blue "Library files check", second part
```

`crates/archivefs-core/src/collection_intelligence/`:

| File | Role |
|---|---|
| `model.rs` | Typed results: record sets, evidence tiers and classes, link grades and causes, the report |
| `inventory.rs` | Metadata-only folder listing with a completeness account |
| `reconcile.rs` | Pure classification of records, strongest rule first |
| `links.rs` | Audit of every symbolic link |
| `sources.rs` | Loads the facts, applies the safety guards, assembles the report |
| `database/collection_records.rs` | Two read-only queries the general loader does not offer |

### Read-only guarantees

- The library database is opened with `Database::open_read_only`.
- Folders are read with `readdir`, `lstat` and `readlink`. No game file is opened unless
  the caller asks for a content comparison, and then only files that have an independently
  recorded hash for their old path, up to 64 MiB each.
- Nothing is offered, in the CLI or in Blue, that changes a record, a flag, a link or a file.
- The CLI registers as a reader. Neither it nor Blue writes anything for this check.

### Guards against a misleading answer

- **Incomplete listing.** A folder that cannot be read, a limit or a cancel makes the listing
  incomplete, and reconciliation then refuses to report at all.
- **Unattached drive.** A row is judged only when the nearest folder that still exists above
  its path has something in it. An empty folder with library rows under it is far more likely
  a drive that is not attached than a deleted collection, so those rows, flagged or not, are
  counted as "not checked" and get no finding. The same holds for rows under a folder on another
  filesystem, which the listing does not enter, and the report names those folders.
- **Reached through a link.** The library scan follows no link. A recorded path that now leads
  through a linked folder is therefore not counted as present, even when a file answers at
  that path: what is there may be a copy kept elsewhere. The same goes for the wrong kind of
  thing at a path, such as a folder where a file was recorded.
- **Changed while looking.** Every file a record is tied to is checked on disk again (size,
  date, inode) before the finding is reported. Candidates and leads are not claims and are
  not re-checked.
- **Double counting.** A Forget Missing row whose path is in the library again, under any id,
  or that is listed in two exports, is counted once. An export written for another database is
  ignored unless named. A link is never a record: only a regular file, or a folder for a game
  that is a folder, counts as present at a recorded path.
- **One folder, one listing.** No folder is entered twice, and linked folders are looked into
  only after every real folder, so a link back into the library can neither list a game twice
  nor take a real folder's place.

### Nothing is kept between runs

Every run reads every folder and looks up every entry afresh. An earlier draft kept the previous
folder listing in a cache file. Measured on a large library, with the files recently read by the
system, it was slower than no cache: it spared the folder reads, but every entry still had to be
looked up, as the rule that no stale fact is ever reported requires, and the saved listing had to
be read and rewritten on every run. It was removed, together with its settings. The module now
writes nothing at all.

## 4. Evidence model

Records come in three sets that never overlap: flagged missing, absent but never flagged, and
removed by Forget Missing. Each record gets exactly one tier.

| Code | Tier | Class | What was shown |
|---|---|---|---|
| T1 | `confirmed_content` | Confirmed | Everything in T2, and a fresh hash of the file equal to one recorded for the old path |
| T2 | `confirmed_object` | Confirmed | EmuWiz's move record is about this record (the first move applied after the record was last seen, of a file with the record's size and date), and the file it recorded is on disk: same inode, size and date, created before the move, on the disk the record's library folder is on |
| T3 | `same_file_object` | Strong | One file with the name, size and date, on the same disk, whose inode was created before the old path was last seen. The scan that last saw the old path completed and covered this file's library folder without finding it, and its row first appears in a later scan |
| T4 | `metadata_match` | Metadata | One file matches on recorded facts only: exactly one file with the same name, size and date; or a file EmuWiz's move record points to that could not be confirmed (no creation time on that filesystem, or contents that differ from a recorded hash) |
| T5 | `name_and_size` | Weak | Same name and size; date differs, or several files qualify (then no single file is named) |
| T5L | `name_and_size_via_link` | Weak | A broken link of that name, and exactly one file, in the folder the link's target was moved to, that has the record's size |
| T6 | `present_at_recorded_path` | Present | The file is there; the flag is stale |
| T7 | `size_only_candidate` | Candidate | One file of identical byte size and type under another name |
| T8 | `name_only_lead` | Candidate | Something shares the name. Shown, never accepted |
| T9L | `broken_link_only` | Unresolved | Only a broken link remains, to storage that is gone |
| T9 | `no_trace` | Unresolved | No move record and no file of that name |

Rules that must not be weakened:

- Rename history alone never confirms a record. A journal entry from a record's path may be
  about another file that had that path earlier or later, so the record's own size, date and
  last sighting must agree with the entry.
- A name alone never ties a record to a file. Two systems often have a game of the same name
  (`system-a/Shared Title (Japan).zip` against `system-b/Shared Title (Japan).zip`). The
  tests cover it.
- Only T1 says that file contents were compared. T2 identifies the same file object; that its
  contents are unchanged is inferred from the object, not hashed. T3 to T5L are filesystem and
  metadata evidence and say nothing about contents.
- T3 is an inference, which is why it is "strong" and never "confirmed": the file existed
  when the old path was last seen, yet a completed scan of its library folder did not find it,
  so it was most likely the same file under the old name. A copy made earlier and kept on the
  same disk outside the library folders, then moved in, cannot be told apart from that. Where
  the scan that last saw the old path was cut short, or is not known to have covered the
  file's library folder, the record stays a metadata match.
- Two records are not both one file, unless they were one file seen at two paths (a hard link,
  or a folder that was visible twice). When several records qualify as T3 for the same file and
  the library's sightings do not show them there at different times (their spans from first to
  last sighting, widened to whole scans, overlap, or a sighting cannot be read), at most one of
  them is the file found now and nothing recorded says which, so all of them are metadata
  matches. Records for one file that never overlapped are one file moved more than once and
  stay T3. Only records that each qualify as T3 are compared this way.
- Contents that differ from a hash recorded for the old path contradict the move record. Such
  a record is reported as a metadata match with a warning, not as confirmed.
- An inode number is reused after a file is deleted. A file created after the move is never
  the file that was moved, and where the filesystem does not record creation times the record
  is graded no higher than a metadata match.
- Device numbers are never compared across runs for identity: a removable disk can get a
  different device number after a reboot. The inode and size are compared and the difference
  is stated.
- Artwork, documents and saves are never candidates.

Symbolic links are not library records. They are audited and counted separately: a link works,
or it is broken for one of a small set of causes, and its target is identified, a plausible
candidate, or unresolved. A target counts as identified only when the folder the link points
into was moved (the missing part of the path reappears under a parent of the deepest folder
that still exists) and the file found there is older than the link. A file beside the link is
a candidate at most, whatever its age: it was never shown to be what the link pointed at.

## 5. Interfaces

- **Blue:** Library files check, second part "Where missing entries went". It has a summary
  with the separate counts, "Entries to review" with level, source and search filters, and
  "Links". Levels are worded Confirmed move, Strong match, Metadata match, Weak match, Stale
  missing mark, Possible match and Unresolved. Links are shown as working, broken because the
  storage they point at is not there, or broken because the file is gone from a folder that is
  there. The check runs on a worker thread with progress and Cancel. Opening the page, or
  switching part, starts nothing. There is no separate page and no entry in All Tools.
- **CLI:** `emuwiz-cli collection reconcile [--json] [--records] [--links] [--class <level>]
  [--limit <n>] [--verify-content] [--forget-receipt <file>] [--database <file>]`. JSON schema: `docs/json-api.md`.

## 6. Design: scan-time file identity and automatic re-pathing (not implemented)

Goal: a moved or renamed file keeps its library row, so stale records stop accumulating.

### Identity hierarchy

| Level | Identity | Survives | Does not survive |
|---|---|---|---|
| 0 | Path (today) | Nothing | Any move |
| 1 | File object: filesystem identifier, inode, size, modification time, creation time | Rename or move within one filesystem | Copy, move across filesystems, restore from backup |
| 2 | Content fingerprint: size plus a sampled hash as a candidate key, then a full SHA-256 on demand | Any move or copy | A change to the bytes |
| 3 | Verified identity: an exact DAT match | Any representation the catalogue covers | n/a |

The filesystem identifier must be stable across boots (the filesystem UUID or the stable part
of the mount binding already captured for nested mounts), never the device number.

### Scan rule

When a scan finds a path gone and a new path present:

1. Level 1 equal (same filesystem, inode, size and times): re-path the existing row in the same
   transaction that records the scan, exactly as `database/organisation_paths.rs` already does
   for EmuWiz's own moves. The row keeps its id, history, DAT result, manual platform,
   collections and save links.
2. Level 2 equal with a stored fingerprint for the old row: re-path, recording the evidence.
3. Anything weaker: insert the new row as today and record a proposed successor (section 7).
   Never re-path on a name, a size or a date.

### Schema (needs approval and a number from the coordination ledger)

```sql
CREATE TABLE archive_file_identity (
  archive_id     INTEGER PRIMARY KEY REFERENCES archives(id),
  filesystem_id  TEXT NOT NULL,      -- stable across boots
  inode          INTEGER NOT NULL,
  size_bytes     INTEGER NOT NULL,
  modified_ns    INTEGER NOT NULL,
  changed_ns     INTEGER,
  born_ns        INTEGER,
  captured_scan  INTEGER NOT NULL REFERENCES scan_runs(id)
);
CREATE INDEX archive_file_identity_object ON archive_file_identity(filesystem_id, inode);

CREATE TABLE archive_content_fingerprint (
  archive_id   INTEGER NOT NULL REFERENCES archives(id),
  kind         TEXT NOT NULL,        -- 'sample-v1' | 'sha256'
  value        TEXT NOT NULL,
  size_bytes   INTEGER NOT NULL,
  modified_ns  INTEGER NOT NULL,     -- the file state the value was computed from
  computed_at  TEXT NOT NULL,
  PRIMARY KEY (archive_id, kind)
);
```

`archives` itself does not change. Both tables are derived and can be rebuilt by a scan.

### Risks to settle before implementing

- Inode reuse after deletion: size, modification time and creation time must also match.
- Hard links: two paths, one object. Both rows stay; neither is a move.
- Filesystems without stable inodes (some network and FUSE mounts): level 1 is disabled there.
- The re-path must run under the same writer lock and missing-authority checks as today's
  mark-missing step.

### File identity that already exists in the code (verified, 2026-11)

Seven places already answer "is this still the same file?", each with its own struct:

| Where | Type | Fields | Used for |
|---|---|---|---|
| `launch/process_spawn.rs` | `CapturedFileIdentity` | device, inode, size, modified, changed | The file handed to an emulator is the file that was checked |
| `game_identity.rs` | `StableFileMetadata` | length, modified, changed, device, inode | A file did not change while its identity was read |
| `catalogue_reconciliation/mod.rs` | `FileIdentity` | device, inode, size, modified, modified_ns, changed | Library files check listings |
| `dat/sources/audit_cache.rs` | `FileFingerprint` | size, modified, device, inode, changed | Key of the loose-file hash cache |
| `dat/rename_apply/model.rs` | `ObjectIdentity` (+ `ObjectFreshness` with a SHA-256) | size, modified (seconds), kind, inode, device | What a rename journal entry moved |
| `verified_identity_cache.rs` | columns of `verified_identity_facts` | device, inode, size, modified (seconds) | Freshness of stored identity facts |

All of them use the raw device number. Only `catalogue_health::SourceRootBinding` carries a
filesystem identifier that is stable across boots, and it is used for library folders and nested
mounts, not for files.

### Other work that keeps a file identity

A game recovery engine is proposed separately (pull request #525, not merged when this was
written). It keeps its own identity for the archives it prepares copies from, in its own store.
This design neither uses nor changes it, and neither needs the other's format. Two points apply
to any code that keeps a file identity across runs, that one included:

- a device number must not be compared across boots (see above);
- a content identity is specific to what it describes. A fingerprint of a disc set and a
  fingerprint of a whole file are different domains and must not share a column or a type.

### What can be shared, and what should not

Share one small value type, owned by core as a leaf module with no dependency on launch,
recovery, the DAT code or this module:

```rust
/// One file as the filesystem identifies it. Not a claim about its contents.
pub struct FileObjectIdentity {
    pub filesystem: FilesystemId, // stable across boots; falls back to the device number
    pub inode: u64,
    pub size: u64,
    pub modified_ns: i64,
    pub changed_ns: Option<i64>,
    pub born_ns: Option<i64>,
}

impl FileObjectIdentity {
    pub fn of(metadata: &std::fs::Metadata, filesystem: FilesystemId) -> Self;
    /// Every recorded field equal: the file was not touched.
    pub fn unchanged(&self, now: &Self) -> bool;
    /// Same filesystem, inode, size and modification time. The change time is
    /// ignored because a rename updates it.
    pub fn same_object_after_rename(&self, now: &Self) -> bool;
}
```

- Each existing struct keeps its own serialised form and gains a conversion to this type, so no
  stored format changes and adoption can be one caller at a time.
- The two comparison methods name the two questions that the seven structs answer today with
  hand-written field comparisons.
- Stay feature-specific: recovery's disc-set fingerprint and receipts, the journal's
  `ObjectFreshness`, launch's spawn-time checks, and this module's evidence tiers.
- Not proposed: a shared content hash. The domains differ (whole file, archive member, disc set).

This is a proposal. It is not implemented, and nothing in this slice depends on it.

## 7. Design: verified replacement links for old rows (not implemented)

Old rows stay hidden as today. This adds a durable, explained link from an old row to the row
that replaced it, so history and choices are kept without a second game appearing.

```sql
CREATE TABLE archive_successions (
  old_archive_id  INTEGER PRIMARY KEY REFERENCES archives(id),
  new_archive_id  INTEGER NOT NULL REFERENCES archives(id),
  evidence_tier   TEXT NOT NULL,     -- the tier names in section 4
  evidence_json   BLOB NOT NULL,     -- the statements shown to the person
  established_at  TEXT NOT NULL,
  established_by  TEXT NOT NULL      -- 'scan' | 'person' | 'reconciliation'
);
```

- One successor per old row; the successor must be a current row; chains are flattened and
  cycles refused.
- Written automatically only at Confirmed and Strong. A metadata match or weaker needs a person
  to accept it, one entry or one explained group at a time.
- User choices (manual platform, collections, favourites, save links) move to the successor
  only through an explicit, journalled step with Undo, and only where the successor has no
  choice of its own.
- Nothing is deleted. Counts read "current games" and "historical records" separately.

## 8. Design: symlink-aware discovery (not implemented)

### What the code does today

- Both walkers skip every symbolic link, deliberately and without counting it:
  `lib.rs` (the library scan, `if file_type.is_symlink() { continue; }`) and
  `ingestion/discovery.rs` (`continue; // never follow symlinks during discovery`).
  It is not a scan mode: there is no option that includes them.
- The closed list of scan skip reasons has no entry for links, so a scan result does not
  mention them.
- The rest of the code is consistent with that: `safe_read` refuses a link unless it points
  into a trusted root, a library folder may not contain a linked path component, and the
  existing reconciliation walk never follows a link.
- Effect: every link in a library folder is invisible to the library. That includes working
  links to games kept on other storage and whole folders that are linked in from another disk.

The exclusion is a safety policy (link races, escapes from the library folder), not an
accident. The design keeps that policy as the default.

### Proposed behaviour

| Topic | Rule |
|---|---|
| Opt-in | Per library folder: `links = "none"` (default), `"files"`, or `"files-and-listed-folders"` |
| Library path | The row is the link's own path. The link text and the resolved target are stored beside it, never instead of it |
| Target identity | The target's file identity (section 6, level 1) and, when computed, its fingerprint. Verification and hashes belong to the target identity, so retargeting a link invalidates them |
| One game, several links | Links resolving to the same target identity form one game: the first path in sort order is the entry, the others are recorded as "also linked from" and not counted. A person can mark a second link as an intentional separate entry; it then shares the target's verification |
| Link to a catalogued file | A link whose target is already a library row is an alias of that row, not a new game |
| Broken link | Recorded with its cause. Never flagged "file not found" as if a game had been deleted |
| Storage unavailable | When the deepest existing folder on the target's path is an absent or empty mount point, or an approved storage root is not attached, the state is "storage not attached". The row is kept and nothing is flagged |
| Loop | Resolution stops at the kernel's limit; the link is recorded as a loop |
| Escaping the library | A target outside the library folder is followed only when it lies under an approved storage root listed in the configuration. Otherwise the link is recorded as "points outside approved storage" |
| Linked folders | Never followed recursively by default. Only folders named in the configuration, with a visited set of file objects so no folder is entered twice |
| Target changes | A changed target identity makes the row's evidence stale, the same as a changed file |
| Launching | The emulator is given the resolved real path, opened without following links and compared with the recorded target identity first |
| DAT verification | Hashes the target; refuses when the target is on storage marked "do not read in bulk" unless asked |
| Rename and organise | Act on the link. The target is never moved or renamed unless that is the explicit operation |
| Remote storage | Metadata calls are rate-limited and cached; contents are never read without being asked |

### Is a schema change needed?

Yes, for the feature itself: the link text, the resolved target and its identity, the link
state and the alias relation have nowhere to live today.

```sql
CREATE TABLE archive_link_targets (
  archive_id       INTEGER PRIMARY KEY REFERENCES archives(id),
  stored_target    BLOB NOT NULL,
  resolved_target  BLOB,
  target_state     TEXT NOT NULL,   -- resolves | broken | storage_unavailable | loop | outside_approved | denied
  filesystem_id    TEXT,
  inode            INTEGER,
  size_bytes       INTEGER,
  modified_ns      INTEGER,
  checked_at       TEXT NOT NULL,
  alias_of         INTEGER REFERENCES archives(id)
);
```

A new skip reason for links also needs a migration, because the reason column has a closed list.

### What can ship first, in order

1. **No schema change (this slice):** the read-only link audit, in the CLI and Blue. It already
   reports every link, why a broken one is broken and, where that could be worked out, the
   folder its target was moved to.
2. **No schema change:** count links during a scan and say so in the scan summary in memory
   ("N links were not catalogued"), without persisting the count.
3. **Schema change, opt-in:** linked files as rows with `archive_link_targets`, target states,
   and "storage not attached" instead of "file not found".
4. **Later:** aliases and intentional duplicates, launching and verification through links,
   listed linked folders.

Tests for steps 1 and 2 use synthetic files and links only: a working link to another folder,
a linked folder, a link broken by a moved folder, by a vanished location and by a missing file,
a loop, and a link out of the library folder.

## 9. Design: repairing broken links (not implemented)

Repairing a link changes the filesystem, so it is a separate, explicit operation and never part
of the read-only check. Any repair needs a preview, a record of every link's original text, a
verification step, and an Undo that writes the original text back.

Two situations come up, and they call for different answers.

| Situation | Evidence | Proposal |
|---|---|---|
| A folder was moved or renamed, breaking every link into it. The files exist at the same relative path under the new folder and are older than the links | Target identified | Either put one link at the old folder name, which repairs every link at once and touches no library folder, or rewrite each link to the new location. Verify that each link resolves to the inode recorded by the check |
| The storage a link pointed at is gone, and a newer file with the same relative path sits beside the link | Plausible candidate only | Do not retarget. The newer file is a later copy and nothing was recorded about the original target, so retargeting would present a path pattern as verified content. If the links are unwanted, removing them is a deletion and needs its own approval |

Repairing links does not add their games to the library. That needs symlink-aware discovery
(section 8).

## 10. Known limits

- A match by name, size and date does not compare systems. Two different games with the same
  file name, byte size and date would be reported as a metadata match, which is why that
  level is never called confirmed.
- Content was never recorded for an old path except where RomM hashed it, so most records
  rest on filesystem and metadata evidence.
- "No trace" means no file of that name and size under the listed library folders. Other
  disks are not searched.
- The listing does not enter other filesystems inside a library folder, except linked folders
  directly inside it.
- One folder that cannot be read stops the whole report. That is safe, and it means a folder
  such as a root-owned `lost+found` inside a library folder has to be readable or excluded.
- A link target on a dead network mount can make a metadata call wait; there is no timeout.
- The check in Blue runs on its own worker thread, and its result is collected while the page
  is open.
- The two parts of Library files check run as separate checks and each lists the folders
  itself. Sharing one listing between them is a follow-up.
