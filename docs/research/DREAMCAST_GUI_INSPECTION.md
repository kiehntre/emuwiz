# Dreamcast IP.BIN and DCP GUI inspection

The selected Dreamcast game's **Mods & Cheats → Dreamcast IP.BIN & DCP
inspection** and **Disc & ROM evidence** panels now project actual bounded
IP.BIN metadata instead of placeholder descriptions. Inspection starts only
when the user clicks **Inspect / refresh IP.BIN**. Existing emulator cheat
controls remain available. No original images or launch settings are changed.

Supported image routes:

- GDI: the existing parser's complete multi-track topology and high-density
  data-track selection, cooked 2048 or validated raw 2352 sectors. All files
  must exist; ambiguous tracks, overlaps, non-whole sectors, nonzero file
  offsets, unsafe references and unsupported layouts are refused.
- ISO: a whole-sector cooked 2048-byte stream with at least 32 KiB available.
  Boot structure status distinguishes malformed/unsupported/suspicious headers.
- CDI, CHD, CUE and loose BIN are explicitly unsupported in this panel. Their
  existing identity/optical backends are unchanged; they are not guessed to be
  equivalent to a supported GDI representation.

The inspector reuses `dreamcast_boot_evidence::ip_bin`, exposes product and
revision, positional region declaration, boot filename, date, title, hardware,
peripherals, product CRC, unknown/raw metadata and issues. Stronger library
product evidence conflicts remain visible. A valid header is not proof of a
bootable image, runtime compatibility, or existence of the boot executable.

**Preview DCP contents & compatibility** reads a user-selected local package
using the existing bounded DCP ZIP inspector. It shows source path, package
SHA-256, every member's path/hash/size and replacement/delta/IP.BIN classification,
plus boot/track impact and warnings. Package publisher authenticity is not
established. Editing the package path clears the prior preview. No content is
downloaded, extracted, applied or executed.

Format evidence is the upstream [Universal Dreamcast Patcher manual](https://github.com/DerekPascarella/UniversalDreamcastPatcher#manual-method),
which describes ZIP filesystem members and the special `bootsector/IP.BIN`
member, with xdelta support in its automatic builder. Metadata offsets and
product CRC come from [Comstedt's primary documentation](https://mc.pp.se/dc/ip0000.bin.html).
Only documentation and synthetic, freely created fixture bytes were used.
Existing backend research and extracted-tree replacement/xdelta support are
retained; this does not add a second DCP implementation.

Standard DCP does not embed an exact cryptographic game binding. This GUI does
not accept a title, filename, product ID, or self-entered hash as compatibility
proof. Readiness stays **NotReady / NoExactMatch** and **Apply DCP to disc image**
is disabled. Existing extracted-tree DCP publication does not rebuild a disc.
The separate `dreamcast_extent_patch` backend can publish verified new GDI sets
for reviewed exact-length file replacements and supported IP.BIN edits, retaining
audio and untouched sectors. This panel does not adapt DCP packages into those
extent plans, establish an authenticated package/game binding or implement
arbitrary filesystem rebuilding. No output is produced or claimed verified by
this panel; existing image publishers and their recovery rules remain unchanged.

Snapshots hash the complete 32 KiB boot area and GDI descriptor, retaining
observed file sizes, modification times and Unix object identities for all
tracks. Before and after DCP inspection the image snapshot is reread and
compared; changed evidence refuses preview. The displayed results are historical
snapshots. Unread track content is not hashed and no complete-image freshness,
atomic snapshot, or exact target-binding claim is made. Refresh explicitly to
update evidence. Existing optical verification and recovery authority are
unchanged.

Validation uses synthetic Dreamcast headers, sector streams and DCP ZIPs.
Focused tests cover actual metadata rendering, native Mods & Cheats reachability,
invalid/truncated headers, unsupported containers, contradictory IDs, stale
source/package evidence, replaced inodes/symlinks, missing audio, unsafe track
references and unsupported offsets. Existing Dreamcast backend tests cover
incorrect exact targets, delta refusal, output verification and publication/Undo.
