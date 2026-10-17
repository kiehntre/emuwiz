# ZIP Reviewed Apply

The Converter → ZIP Converter page recovers the reviewed-preview binding and
background worker from `49808e1b` / `aa9eedc4`. Their named staging publication
and cleanup are **not** restored. The historical FAIL report and uncommitted
work remain preserved in their original worktrees and the task audit ledger.

## Supported workflow

1. Select **Compress** (a regular file or folder), or **Extract one file**.
2. Choose the source and a **new output filename**. Extraction's destination is
   one regular file, not a directory. It does not reproduce file metadata.
3. Select **Preview**. Review the source, complete member list, destination,
   sizes and archive SHA-256 where applicable. Large lists are visually capped;
   the backend checks every member.
4. Select **Confirm and create ZIP** or **Confirm and extract file**. Changing
   input fields invalidates the preview. The background worker consumes that
   exact review, locks inputs and reports actual progress. Cancel requests a stop;
   the final result determines whether publication had already begun.

Only Linux with a local ext4 destination, functional `O_TMPFILE`, hardlinks and
kernel `/proc/self/fd` publication is supported. Filesystem identity is probed,
not inferred from a directory name. Failure never falls back to named staging,
ordinary rename, overwriting or external extraction tools. The original legacy
ZIP Apply APIs stay unconditionally blocked; the new page uses reviewed APIs.

Extraction Apply supports exactly one top-level regular member and no directory
entries. Multi-file/nested/directory layouts can be previewed but Apply remains
disabled. No member is silently selected from an unsupported archive.
Unencrypted stored/deflated ZIPs and UTF-8 names are supported. Traversal, dot or
empty path components, backslashes, ASCII case aliases, duplicate/conflicting paths,
symlinks and special entries refuse. Bounds: 4096 members, depth 32 and 128 MiB
for the source ZIP and total uncompressed data. Existing central-directory
bounds also apply. ZIP bytes are bounded-snapshotted before parsing so mutable
input cannot invalidate allocation preflight. CRC and streamed SHA-256 checks
cover every ordinary member. A single ordinary end-record and zero archive-offset are required. Additional
end-record markers anywhere in the ZIP bytes (including comments or embedded
payloads) and ZIP64 end-records are explicitly refused before upstream indexing;
this conservatively excludes some otherwise valid archives. Per-entry ZIP64
size declarations remain subject to checked bounds.

## Commit and recovery boundary

The existing ZIP copy transaction remains stage → verify → publish. Staging is
an unnamed regular inode in the destination filesystem. It stays descriptor
bound during writing, readback verification and publication. Closing an
unpublished descriptor reclaims it; no cleanup selects a mutable pathname.
The final `linkat` creates the new name for that exact inode and atomically
refuses occupied names. See the Linux [open](https://man7.org/linux/man-pages/man2/open.2.html)
and [linkat](https://man7.org/linux/man-pages/man2/linkat.2.html) documentation.
No directory tree is renamed, recursively removed or automatically cleaned.

Source identities/content and destination directory identity/absence are checked
again before commit. Published identity/bytes and directory identity are checked
before success. Files and parent directories are synced, but no hardware
power-loss durability claim is made.

Before publication, cancellation/I/O failure leaves no destination output.
After publication, a sync/verification failure or missing acknowledgement is
**uncertain**, never "nothing published" or a successful operation. Preserve the
source and inspect the named destination. No automatic retry, deletion or Undo
occurs. An occupied destination blocks a new Preview/Apply. If another attempt
is needed, choose another new filename and obtain a fresh Preview.

Process death before commit drops the unnamed inode. Death after commit can
leave a complete output without acknowledgement. No resumable extraction or
persistent recovery journal is added: ZIP operations copy sources and have no
existing journal-based Undo contract. Original archives/files remain untouched,
so source-based recovery is available. The existing rename, R2 and other archive
recovery/Undo contracts are unchanged. This feature never deletes a final output
as rollback and never invents a completed recovery record.

## Explicit limits

Checks are observations, not exclusion of external writers. A later external
rename/delete/write may change an output after the operation's checks. Deliberate
modification of EmuWiz memory or private open descriptors, privileged namespace
changes and kernel compromise are not covered. File permissions are not claimed
to exclude hostile same-UID code. No source image/ROM preservation or playability
claim follows from ZIP integrity alone.
