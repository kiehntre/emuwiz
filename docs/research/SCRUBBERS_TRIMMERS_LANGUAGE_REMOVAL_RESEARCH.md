# Console scrubbers, trimmers and language-removal tools research

Research-only note. No production code, converters, downloaders, GUI or tests were changed.

Question asked: *discover and research the best console rippers/scrubbers to remove updates and
unwanted languages from game ROMs / ISOs / PKGs so they compress smaller, safely.*

## Scope and method

This note covers tools that do four different things, which are frequently conflated:

1. **Scrub** — rewrite an image so unused/dummy/padding regions are zeroed, which makes later
   compression dramatically better.
2. **Trim** — drop the padding *after* the end of the filesystem / partition table so the file is
   shorter (not just more compressible).
3. **Recompress** — move the payload into a smaller container (CHD, CSO/ZSO, RVZ/GCZ, WIA, WBFS,
   NSZ/XCZ, WUA) without deleting content.
4. **Content-strip** — delete real files (update partitions, `$SystemUpdate`, `PS3_UPDATE`, unwanted
   language audio/subtitle assets) and rebuild the filesystem.

Only (1)–(3) are reliably **reversible/lossless**. (4) is a deliberate modification and the only
operation that can silently break a game, so it must be treated as a *mod*, not as compression.

Sources were the upstream repositories/readmes, the Wiimms ISO Tools documentation site, the MAME
`chdman` shipped locally at `/usr/bin/chdman` (verified `0.264`), and the user's existing survey in
`docs/research/PS_MULTI_TOOLS_AUDIT.md`. Where a claim is from community consensus rather than an
upstream doc it is marked **CONSENSUS**; machine observations are **VERIFIED-LOCAL** and upstream
documentation/repository evidence is **VERIFIED-UPSTREAM**. Items without trustworthy evidence are
**UNRESOLVED**.

## Part 1 — The recompression layer (safe, reversible)

These are the "compress smaller safely" formats. Prefer these; they do not alter game content.

| Container | Tool | Platforms | Lossless? | Notable consumers | Confidence |
|---|---|---|---|---|---|
| **CHD** | `chdman createcd` / `createdvd` | PS1, PS2 (CD/DVD), Saturn, Dreamcast (GDI), Sega CD, PSP (UMD as DVD), arcade | Yes (hunked, per-sector; verify round-trip) | RetroArch, DuckStation, PCSX2, PPSSPP, Flycast, MAME | High — local `chdman 0.264` present |
| **CSO / ZSO** | `maxcso` (ISC, v1.13.0) | PSP, PS2 | Yes when decoder is correct; CSO→ISO must be byte-identical | PPSSPP, OPL (ZSO), PCSX2 (CSO support is limited; prefer CHD) | **High — built locally, round-trip verified byte-identical** |
| **RVZ** | Dolphin / `DolphinTool` / **rom-converto `dol`+`rvl`** | GameCube, Wii | Yes (zstd, block-based) | Dolphin, rom-converto | **High — round-trip verified byte-identical** |
| **GCZ** | Dolphin | GameCube, Wii | Yes (legacy zlib) | Dolphin, some Wii homebrew | High |
| **WIA** | Wiimms ISO Tools (`wit`/`wdf`) | Wii, GameCube | Yes **only with `--raw`**; the default scrubs and changes the image | WIT ecosystem, Dolphin (via conversion) | **High — round-trip verified (see Part 7)** |
| **NSZ / XCZ** | `nsz` (nicoboss) / **rom-converto `nx`** | Switch | Yes (zstd), installable by homebrew installers | Switch CFW installers | High (rom-converto `nx compress` verified) |
| **WUA** | **rom-converto `wup compress`** (v0.15.0, installed here) | Wii U | Yes, bundles base + update + DLC with dedup | Cemu | **High — confirmed on this machine** |
| **WBFS** | `wwt` / WIT | Wii | Yes, but no compression (container only) | Wii USB loaders | High |
| **XISO** | `extract-xiso` | Xbox (OG) | Structural; `-r` rewrite | xemu, Xbox | High (upstream readme) |
| **7z / ZIP / RAR** | generic | storage only | Yes | any archive browser | High |

Key implementation points already captured in `PS_MULTI_TOOLS_AUDIT.md` and worth repeating:

- CSO/ZSO correctness is a **header + block size + size-table** property, not just "it compressed".
  A valid round trip (CSO→ISO byte-identical to the source) is the only proof; `maxcso` supports
  zlib, 7z-deflate, Zopfli, libdeflate and experimental CSO v2 / ZSO (LZ4), and warns that larger
  block sizes and libdeflate output are incompatible with some PSP CFW. Do not enable them by
  default.
- CHD is *not* a filesystem change. `chdman extractcd/extractdvd` must reproduce the source image,
  including padding; that is the reversible-shrink property EmuWiz already uses for the Xbox path
  (`emuwiz-xbox-xiso-reversible-shrink`).
- **Verified on this box:** `chdman 0.264` at `/usr/bin/chdman` accepts `-c lzma`, `-c zlib`,
  `-c huff` and `-c flac` for `createdvd` (tested with a 4 MiB input on 2026-09-18). `cdlz` is
  rejected for DVD because the `cd*` codecs are CD-only filters. `-c` takes up to four codecs tried
  in order. **Practical default: `-c lzma,zlib`.** Do not use `flac` for DVD data (it is an
  audio-oriented codec; acceptance by the CLI is not a suitability check).

## Part 2 — Per-platform scrubbers, trimmers and content-strippers

### Nintendo GameCube / Wii — best in class: **Wiimms ISO Tools (WIT)**

- Upstream: <https://wit.wiimm.de/> and <https://github.com/Wiimm/wiimms-iso-tools>
- Release: **v3.05a, 2022-08-27**; Linux x86_64/i386, Cygwin (Windows), macOS universal.
- Toolset: `wit` (ISO manipulation), `wwt` (WBFS), `wdf` (WDF/WIA/CISO/GCZ pack + unpack),
  `wfuse` (mount a Wii/GC image or WBFS over FUSE).
- Capability: list, analyse, verify, convert, split, join, patch, mix, extract, compose, rename,
  compare; plus scrubbing during copy and **partition selection** so the update partition can be
  dropped.
- **Flags confirmed locally** (Part 7): scrubbing is a `COPY`/`CONVERT`/`EXTRACT` behaviour
  controlled by `--psel`. There is **no** standalone `wit scrub` command (the `SCRUB` command exists
  in `wwt`, the WBFS tool, not `wit`).
  - `--psel data` — copy only the DATA partition (drops UPDATE and CHANNEL).
  - `--psel -update` — copy every partition **except** UPDATE.
  - `--psel whole` — do not analyse the partition filesystem for unused sectors (disables scrub
    method 3 only).
  - `--raw` — shortcut for `--psel raw`: copy the whole disc with **no scrubbing at all**.
  - `--neek` — shortcut for `--psel data --pmode none --files :neek --copy-gc`.
- **Scrubbing is ON by default.** Per the upstream guide, three methods are combined: (1) ignore
  disc space not claimed by any partition — on by default; (2) remove unwanted partitions — off by
  default, enabled via `--psel data` / `--psel -update`; (3) zero unused sectors *inside* the
  partition filesystem — on by default, disabled via `--psel whole`. Scrubbed files are written
  **sparse**, and WIA compresses them well.
- **Version note:** Ubuntu 24.04 `universe` ships **3.01a** (`/usr/bin/wit`, plus `wwt`, `wdf`,
  `wfuse`); upstream current is **3.05a r8638 (2022-08-27)**, downloadable from
  `https://wit.wiimm.de/download/wit-v3.05a-r8638-x86_64.tar.gz`. Both binaries were probed here and
  expose identical `--psel` / `--raw` semantics.
- Why it is "best": it understands the Wii partition table, which means it removes the update
  partition *by rebuilding the partition table*, rather than zero-filling bytes and hoping. That is
  the safe form of scrubbing.
- GUIs on top of WIT: QtWitGui, Wii Backup Fusion, Witgui (macOS).

**Dolphin (RVZ/GCZ)** — the other safe path. Dolphin's Convert-to-RVZ is lossless and its disc
conversion dialog exposes discard options for the Wii update partition. Confidence **High** on
RVZ/GCZ being lossless. The exact wording and availability of the
"remove update partition" toggle across Dolphin versions.

**WiiBackupManager** — Windows GUI: scrub/trim, drop update partition, convert to WBFS/CISO.
**UNRESOLVED:** the historical SourceForge project page returns 404 and no canonical source
repository, CLI/API, or redistribution licence was found. `wiibackupmanager.com` now presents itself
as an archive of Build 78, not a maintained source project. Treat Wii Backup Manager as
historical/reference tooling and prefer WIT + Dolphin on Linux. TinyWiiBackupManager is a separate,
current GPL-3.0 Linux/macOS/Windows alternative, but adds no required capability beyond WIT/Dolphin.

**WiiScrubber** — legacy Windows scrubber (zero-fills unused disc area). Confidence **High** that it
is superseded and **CONSENSUS** that it is the tool most likely to produce images that boot badly or
fail verification, because it is byte-oriented rather than partition-aware. Do not recommend it when
WIT is available.

### Nintendo Wii U

- **Cemu WUA** is documented by Cemu as lossless and convertible back to the original files. The
  original standalone upstream utility is **Exzap/ZArchive**; its README defines `.wua` as a Wii
  U-specific ZArchive layout and its library as MIT-0. Current cross-platform CLI tooling is
  **DevYukine/rom-converto**: `rom-converto wup compress -o output.wua input...`, with `wup decrypt`,
  `wup verify` and `wup info`.
- **rom-converto `wup compress`** (v0.15.0, **already installed here** at `~/bin/rom-converto-cli`)
  is the lossless archival path: it writes a Cemu-compatible **`.wua`** and can bundle base + update
  + DLC into one archive. Verified from its own `--help`; the user's config already has
  `[presets."wiiu to compress"] wup = { level = 0, on_conflict = "overwrite" }` and `.wua` files
  exist under `~/Desktop/WII-U/`.
- Inputs accepted: loadiine directory (`meta/`, `code/`, `content/`), **NUS directory**
  (`title.tmd`, `title.tik`, `*.app`, auto-decrypted), or a `.wud`/`.wux` disc image (needs the
  16-byte master key via `--key`, a sibling `<input>.key`, or `game.key`). `wup decrypt` turns a NUS
  directory into a loadiine tree; `wup verify` re-checks each content's SHA-1 against the TMD.
- Other extraction/repack tools: `NUSPacker`, `JNUSTool`, `UWUVCI` (injection), `Wii U USB Helper`
  (NUS downloader/decrypt). None is a language remover; Wii U language assets live inside content
  archives and removing them is game-specific work, not a generic tool feature.

### Nintendo Switch — best in class for language/update stripping: **NSC_Builder**

- Upstream: <https://github.com/julesontheroad/NSC_BUILDER> (README notes the repo was temporarily
  archived by the author, then reopened for commits).
- What it is: "Nintendo Switch Cleaner and Builder" — a Switch-army-knife that removes title-rights,
  merges multi-content NSP/XCI, and performs batch file information/edit operations. This is the
  tool the community actually uses for **removing unwanted languages / audio tracks** and for
  building clean game-only, update-only or DLC-only content. Confidence: **High** that it is the
  category leader; **UNRESOLVED** for the exact current language-addon flag; do not script it from
  memory or forum posts.
- Requires a full `prod.keys` (from the user's own console via Lockpick). It does not ship keys.
- Compression companions: **`nsz`** (<https://github.com/nicoboss/nsz>, MIT, zstd, homebrew-installable;
  keys supplied by the user; explicitly does not remove DRM), **rom-converto `nx`** (installed here:
  `nx compress` converts NSP→NSZ and XCI→XCZ, `solid` vs `block` mode, `--keys` defaults to
  `$HOME/.switch/prod.keys`, `nx verify` re-hashes every NCA), and **SAK / Switch Army Knife**
  (<https://github.com/dezem/SAK>, now archived; wraps `squirrel`/NSC_Builder, `hactool`,
  `hacBrewPack`, 4NXCI, and does XCI↔NSP and NSZ/XCZ conversion).
- **rom-converto `nx` does not remove languages or updates** — it is a lossless recompressor. For the
  language/update strip you still need NSC_Builder (its `--help` exposes no language option; the
  container is decrypted, zstd-compressed and repackaged as-is).
- Low level: `hactool` (SciresM) and `LibHac`/`hactoolnet` (TheAlexBarney) for extract/repack.

### Nintendo 3DS and DS

- **3DS:** `ctrtool` (read/extract) and `makerom` (build CXI/CFA/CCI/CIA) from
  <https://github.com/3DSGuy/Project_CTR>; on-device `GodMode9` can copy/trim a game image. There is
  no mainstream generic 3DS *language* stripper — language selection is inside NCCH/romfs and is
  game-specific. Confidence **High** on ctrtool/makerom, **CONSENSUS** on GodMode9 trimming.
- **NDS:** `ndstool` (<https://github.com/devkitPro/ndstool>) to extract/build a file system; the
  classic Windows trimmer is `NDSTokyoTrim` (removes the trailing `0xFF` padding safely); `Tinke`
  and `DSLazy` can also repack. NDS trim is the clearest "safe trim" case: the pad is literally past
  the end of the ROM and no game reads it.

### Game Boy Advance / other cartridge ROMs

`GBATA` (GBA) and the various No-Intro/GoodTools "trim" utilities remove end-of-ROM padding. Safe for
plain ROMs, but it **changes the file hash**, so a trimmed ROM no longer matches No-Intro DATs.

### PlayStation 1

- **`chdman createcd`** (lossless CD→CHD) is the recommended recompression. Verified locally:
  `chdman 0.264` is installed and exposes `createcd`, `extractcd`, `copy`, `verify`, `dumpmeta`.
- `CDmage` (Windows) for track/file inspection and CD image conversion.
- `PSX2PSP` for PBP packaging (PSP/PS3). Content-stripping is not a normal operation for PS1;
  undub/relanguage work is done as per-game patches.

### PlayStation 2

- **Safe default: recompress, don't strip.** `chdman createdvd` → CHD (PCSX2/RetroArch) or `maxcso`
  → ZSO (Open PS2 Loader). The PS2 file system is shared and many "language" assets are inside common
  archives, so blind deletion is the highest-risk operation on this platform.
- File-level editing requires a PS2 ISO extractor/rebuilder that preserves the ISO9660 layout
  (e.g. `ExPERT`, `CDVDGEN`-era workflows, or extract → `mkisofs`/`genisoimage` → rebuild). This
  remains **CONSENSUS**, not a verified EmuWiz backend; every rebuild is a mod that must be boot-tested.

### PlayStation Portable — best in class for content-strip: **UMDGen**

- **UMDGen 4.00** (Windows) is the standard PSP ISO editor: extract, delete/replace files (dummy
  files, update data, non-native-language assets), and rebuild a valid ISO. This is the tool the PSP
  community uses for removing unused language/data before compression. Confidence: **High**
  (long-standing community standard). **UNRESOLVED:** no verifiable maintained upstream source,
  release home, or clear redistribution licence was found; UMDGen is a 2010-era Windows-only binary
  distributed via mirrors. `quickstraw/UMDGenCLI` is a separate .NET recreation with a scriptable
  CLI and Windows-only published binaries, explicitly not the original project. Treat original UMDGen
  as research/reference only; treat UMDGenCLI as a possible user-installed helper only after a
  separate licence and compatibility review.
- Recompression: `maxcso` (preferred, maintained, multi-threaded, Zopfli optional) → CSO/ZSO;
  older `mCiso`, `CisoPlus`, `PSP ISO Compressor` also exist. PPSSPP reads CSO/ZSO and CHD.
- Compatibility note: keep the default block size; `maxcso`'s libdeflate output breaks some PSP CFW.

### PlayStation 3

- Dump source: **`ps3-disc-dumper`** (<https://github.com/13xforever/ps3-disc-dumper>) makes a
  decrypted disc copy using a matching disc key from Redump/IRD; it needs a compatible Blu-ray drive.
- Rebuild/repack: PS3 game ISOs are UDF images; the community flattens them to a "JB folder"
  (`PS3_GAME`…) or rebuilds an ISO. `PS3 ISO Tools` is bundled inside **PS Multi Tools**
  (SvenGDK) — already surveyed in `PS_MULTI_TOOLS_AUDIT.md` — and `ManaGunZ` / `multiMAN` file
  managers can copy/manipulate mounted games on console. Confidence: **High** for JB-folder/ISO
  concepts and for PS Multi Tools. Named rebuilders do exist and were located:
  - **`bucanero/ps3iso-utils`** (<https://github.com/bucanero/ps3iso-utils>) — "Windows, Linux, and
    macOS builds of Estwald's PS3ISO utilities". Its documented commands are `extractps3iso <ISO>
    <folder>` and `makeps3iso <folder> <ISO-or-folder>`, with Docker/Podman support. This is the
    practical ISO↔JB-folder rebuild path on Linux, but no byte-exact round trip was proven here.
  - **`ifcaro/PS3-ISO-Rebuilder`** (<https://github.com/ifcaro/PS3-ISO-Rebuilder>) — a dedicated
    rebuilder with only two commits and no maintained-release evidence; treat as unmaintained.
  - **`13xforever/ird-iso-patcher`** — IRD/key patching for redump ISOs (adjacent, not a rebuilder).
- `PS3_UPDATE` / `$SystemUpdate` directories can be deleted from a JB-folder dump and are not needed
  by RPCS3; this is the PS3 equivalent of the Wii update-partition strip. Confidence **High**.

### Xbox (OG) — already partly implemented here

- **`extract-xiso`** (<https://github.com/XboxDev/extract-xiso>, v2.7.1): `create`, `list`,
  `rewrite`, `extract`. Critically it has **`-s` (skip `$SystemUpdate`)** and `-m` (disable automatic
  `.xbe` media-enable patching). `-r` rewrite + `-D` is the reversible-shrink workflow already being
  modelled in `emuwiz-xbox-xiso-reversible-shrink`. Verified from upstream readme; binary built
  locally at `/home/davedap/extract-xiso/extract-xiso`.
- Older GUI equivalents: **Xbox Image Browser**, **wxPirs** (also used for 360).

### Xbox 360

- **wxPirs** / **Xbox Image Browser** / **Velocity** to extract and rebuild XISO; **`ISO2GOD` /
  `GOD2ISO`** to move between ISO and the Games-on-Demand layout. Deleting `$SystemUpdate` before
  rebuild is the usual strip; title updates (TUs) live outside the disc image, so "rip updates"
  on 360 mostly means *don't* repack the update partition and keep TU handling separate.
  Confidence: **High** on tool names; **UNRESOLVED** for signatures/hosts before any download.

### Dreamcast / Saturn / Sega CD

`chdman createcd` handles GDI/CDI/CUE-BIN; Dreamcast GD-ROM metadata is detected by `chdman` and by
EmuWiz's `chd_rs_probe` (GD-ROM old/track tags). `gditools.py` / GD-ROM Explorer can extract and
rebuild GDI. Trimming GDIs is not standard because the high-density area matters.

### PlayStation Vita / PS4 / PS5 (the "PKG" family)

- **Vita:** `pkg2zip` (unpack retail PKG), `VitaShell`, `NoPayStation` tooling (already local at
  `/home/davedap/Applications/nopaystation`). No generic language strip.
- **PS4:** local `PS4PkgExtractor`; `orbis-pub`-era fake-PKG tooling. Not a scrubber category.
- **PS5:** PKG viewer/manifest editing only (see `PS_MULTI_TOOLS_AUDIT.md`).

Note on the question's "**pjks**": the only common console package extension in this space is
**PKG** (PSP/PS3/Vita/PS4/PS5). If a different container was meant, the PKG-family tools above are
still the right starting point, but please confirm the extension.

## Part 3 — Where language removal is actually safe

"Remove unwanted languages to compress smaller" is only a *generic* feature on two platforms:

| Platform | Generic language strip? | Tool | Why it works |
|---|---|---|---|
| Switch | **Yes** | NSC_Builder | NCA/romfs layout separates language audio/subtitle add-ons; tool rebuilds the container |
| PSP | **Yes** | UMDGen | Plain ISO9660 file list; delete files then rebuild |
| Wii | **Partly** | WIT (`--psel -update`, `--psel data`) / WiiBackupManager / Dolphin | Partition-level granularity (e.g. update), file-level language data is game-specific |
| Xbox / Xbox 360 | **Partly** | `extract-xiso -s` / wxPirs | Whole `$SystemUpdate` directory, plus any plainly-named files |
| Wii U, 3DS, PS2, PS3, Vita | **No (per-game)** | extract → delete → rebuild | Languages are inside package archives; deleting needs format knowledge and boot testing |

Practical rule: if the platform exposes its asset tree as ordinary files (PSP, Xbox, PC-style
containers), a generic strip is safe-ish. If it is a signed/encrypted/compiled container (3DS NCCH,
Wii U, PS2 archives, PS3 UDF+EBOOT), language removal is a **romhack**, not a compression step.

## Part 4 — Safety checklist (must precede any scrub/trim/strip)

1. **Never operate in place.** Copy, transform to a new path, verify, then replace. This matches the
   no-clobber transaction policy already used for CHD/CUE-BIN work.
2. **Classify the operation.** Recompress (CHD/CSO/RVZ/WIA/NSZ/WUA) = reversible; scrub/trim = 
   reversible if only trailing pad; strip = irreversible mod.
3. **Round-trip proof for recompressors.** CSO→ISO and CHD→source and RVZ→ISO must reproduce the
   original bytes; store SHA-256 of input and the reconstructed output.
4. **DAT expectation.** Any trim/scrub/strip changes the hash, so the result will **not** match
   Redump / No-Intro / TOSEC. Keep the DAT-verifiable original and treat the compacted file as a
   separate "play" artifact (mirrors the existing "preservation copy vs play copy" split).
5. **Boot test to a fixed point** after every strip: title screen → main menu → language selection →
   first in-game scene. A game that boots is not proof; force the *removed* language to be requested
   if the game has a selector.
6. **Prefer partition-aware over byte-aware.** WIT and Dolphin rebuild the Wii partition table;
   WiiScrubber zero-fills. Prefer the former.
7. **Keep block sizes and codecs conservative.** Default CSO block size, default CHD codec, no
   libdeflate for PSP CFW, Zopfli only when time allows.
8. **Respect the update/update-split boundary.** Stripping `$SystemUpdate` / `PS3_UPDATE` / Wii
   update partition is generally safe for emulators and loaders; keep consoles' on-system update
   paths out of scope.
9. **Log the tool + version + flags** for every transformed file, so the operation is auditable and
   reversible from the preserved original.

## Part 5 — Recommended toolchain for this workspace

Ordered by reversibility, mapping onto the existing `StorageFormatClass` set. **The first choice is
already installed:** `rom-converto` (`~/bin/rom-converto-cli`, v0.15.0) covers almost the whole
lossless layer with `--dry-run`, `--preset`, hashing, verify and a space preflight.

1. **Recompress, never delete, by default.**
   - **`rom-converto`** — `dol`/`rvl` → RVZ, `wup` → .wua, `nx` → NSZ/XCZ, `chd` → CHD,
     `cso` → CSO/ZSO, `cue` → CUE/BIN, `ctr` → 3DS. Also `chd to-cso` and `cso to-chd`.
   - PS1 / PS2 / PSP / Saturn / Dreamcast / Sega CD → `chdman` (local 0.264, `-c lzma,zlib`).
   - PSP / PS2 (loader path) → `maxcso` (CSO/ZSO, now at `/usr/local/bin/maxcso`), conservative
     options (no `--use-libdeflate`, default block size).
   - GameCube / Wii → RVZ (rom-converto `dol`/`rvl`, verified byte-identical) **or** WIT WIA
     **only with `--raw`**; WIT's default WIA scrubs and changes the image.
   - Xbox → `extract-xiso -r` rewrite / reversible shrink (already implemented here).
2. **Partition-level strip (safe, reversible-ish).**
   - Wii update partition: `wit copy --psel -update` (or `--psel data`); Dolphin convert option.
   - `$SystemUpdate`: `extract-xiso -s` (OG Xbox), wxPirs for 360.
   - `PS3_UPDATE`: remove from JB-folder dump.
3. **Content strip (mod, opt-in, preview + boot test required).**
   - Switch languages/updates/DLC: NSC_Builder (best in class; rom-converto `nx` cannot do this).
   - PSP files/update/dummy: UMDGen.
   - Anything else: extract → delete → rebuild with a format-correct builder, then boot test.

Suggested EmuWiz-facing distinction (consistent with the existing "preview before destructive"
policy): expose **"Recompress (lossless)"** and **"Trim/strip (modifies content)"** as two separate
actions with different confirmation weightings and different verification receipts.

## Part 6 — Evidence and links

- Wiimms ISO Tools — <https://wit.wiimm.de/> (v3.05a, 2022-08-27; tool list and GUIs verified)
  and <https://github.com/Wiimm/wiimms-iso-tools>
  - Scrubbing guide: <https://wit.wiimm.de/opt/psel> (three scrub methods, keywords, `--psel data`,
    `--psel -update`, `--raw` examples)
  - Upstream tarball: <https://wit.wiimm.de/download/wit-v3.05a-r8638-x86_64.tar.gz>
  - Local docs: `zcat /usr/share/doc/wit/wit.txt.gz` (Debian 3.01a option reference)
- **`rom-converto`** — <https://github.com/DevYukine/rom-converto> (v0.15.0 installed at
  `~/bin/rom-converto-cli`; families `ctr`/`dol`/`rvl`/`wup`/`nx`/`chd`/`cso`/`cue`; verified on this
  machine, last upstream push 2026-09-15)
- `maxcso` — <https://github.com/unknownbrackets/maxcso> (ISC; algorithms, CSO v2/ZSO, PSP CFW
  caveat verified from readme)
- `extract-xiso` — <https://github.com/XboxDev/extract-xiso> (modes `-c/-l/-r/-x`, `-s` verified)
- `nsz` — <https://github.com/nicoboss/nsz> (MIT; zstd; keys user-supplied; no DRM removal)
- NSC_Builder — <https://github.com/julesontheroad/NSC_BUILDER>
- SAK (archived) — <https://github.com/dezem/SAK> (bundles squirrel/NSC_Builder, hactool, etc.)
- `ndstool` — <https://github.com/devkitPro/ndstool>
- Project_CTR (`ctrtool`, `makerom`) — <https://github.com/3DSGuy/Project_CTR>
- `ps3-disc-dumper` — <https://github.com/13xforever/ps3-disc-dumper>
- MAME `chdman` — local `/usr/bin/chdman` 0.264 (`--help` verified)
- Internal: `docs/research/PS_MULTI_TOOLS_AUDIT.md` (SvenGDK PS Multi Tools; CSO/CHD reasoning)
- Internal: `docs/research/READY_TO_PLAY_ARCHITECTURE_AUDIT.md` (`StorageFormatClass` set)
- Internal: `emuwiz-xbox-xiso-reversible-shrink` (reversible-shrink precedent)

## Part 7 — Verified locally, 2026-09-18

Environment: Ubuntu 24.04.5 (noble), passwordless `sudo`, pre-existing `chdman 0.264` and
`rom-converto 0.15.0` at `~/bin/rom-converto-cli`.

### Tooling obtained

| Tool | Source | Version | Location |
|---|---|---|---|
| `wit`, `wwt`, `wdf`, `wfuse` | Ubuntu `universe` (`apt install wit`) | 3.01a-4.1build2 | `/usr/bin` |
| Wiimms ISO Tools upstream | `https://wit.wiimm.de/download/wit-v3.05a-r8638-x86_64.tar.gz` | 3.05a r8638 | tarball kept at `~/Applications/`, extracted at `/tmp/wit-v3.05a-r8638-x86_64` |
| `maxcso` | built from `unknownbrackets/maxcso` | v1.13.0 | `/usr/local/bin/maxcso` |
| `rom-converto` | pre-existing | 0.15.0 | `~/bin/rom-converto-cli` |
| `chdman` | pre-existing | 0.264 | `/usr/bin/chdman` |

**maxcso packaging caveat:** its GitHub releases publish **Windows-only** binaries
(`maxcso_v1.13.0_windows.7z`); Linux and macOS must build from source. Build deps used here:
`build-essential liblz4-dev libdeflate-dev libuv1-dev zlib1g-dev pkgconf git`. The build completed
cleanly and the binary reports `maxcso v1.13.0`.

### Test 1 — maxcso CSO round trip: PASS

Synthetic 12 MiB input (8 MiB random + 4 MiB zeros): `rt.iso` 12,582,912 → `rt.cso` 8,445,980 →
`maxcso --decompress` → 12,582,912. SHA-256 before/after both
`ed9ba507261cc3ea92494cb1f05a977263be7b999f4c8c1b014ad21f285afa4d` — **byte-identical round trip**.

### Test 2 — real GameCube disc: WIT scrub vs lossless paths

Input: `~/.config/retroarch/downloads/GameCube-240pSuite-1.10b.iso`, 1,507,328 bytes, disc id
`GBLPGL`, title "GAMECUBE HOMEBREW BOOTLOADER".

| Operation | Output size | Round-trip vs source SHA-256 |
|---|---|---|
| `wit copy` (default = scrubbed) → WIA | **101,772** | **changed** (`a62b9d…`); 1,280,899 bytes differ |
| `wit copy --raw` → WIA | 1,403,978 | **identical** (`d8c13c…`) |
| `rom-converto-cli dol compress --level 22` → RVZ | 1,390,652 | **identical** (`d8c13c…`) |

Non-zero byte count fell from 1,390,436 in the source to 109,537 in the scrubbed reconstruction — the
source carried ~1.28 MiB of non-zero but unused data that WIT's default scrub replaces with zeroes.

**This is the central safety result of the whole note:**
- WIT's default WIA is **13.8× smaller** than the raw WIA *because it discards data*, and the result
  no longer matches the original hash (so it will fail every DAT).
- `--raw` WIA and RVZ both preserve every byte. RVZ saved only 7.7 % here *precisely because* it kept
  the junk. Losslessness and maximum shrinkage are in direct tension.
- Practical rule confirmed: **scrub is a modification, not a compression setting.** Apply reversible
  formats (RVZ, CHD, CSO, `--raw` WIA) to a preservation master; apply `--psel`/`--raw`-less scrub
  only to a play copy you can regenerate.

### Test 3 — `chdman` DVD codecs: PASS

`chdman 0.264` accepts `-c lzma`, `-c zlib`, `-c huff` and `-c flac` for `createdvd`; `cdlz` is
rejected as CD-only. `-c` accepts up to four codecs tried in order. Recommended default: `lzma,zlib`.

### Test 4 — rom-converto capability surface

Families confirmed from its own `--help`: `ctr`, `dol`, `rvl`, `wup`, `nx`, `chd`, `cso`, `cue`,
`dat`, `hash`, `playlist`. Every disc family exposes compress / decompress / migrate / verify / info,
plus cross-conversions (`chd to-cso`, `cso to-chd`). Safety affordances: `--dry-run`, `--preset`,
`--report`, free-space preflight, persistent hash/verify cache, `--output-template` with
`{title}/{titleId}/{region}/{console}/{serial}` tokens.

**No family exposes language removal, audio-track removal, or update-partition removal.** That
confirms the tool is the *safe* layer only; the strip layer still needs WIT (`--psel -update`),
NSC_Builder, UMDGen and `extract-xiso -s`.

## Historical open-item log (superseded by Part 8)

**Resolved**
- ~~Exact WIT scrub/partition flags~~ — `--psel data` / `--psel -update` / `--psel whole` / `--raw`;
  scrubbing is on by default; `wwt` has `SCRUB`, `wit` does not. Verified on 3.01a and 3.05a.
- ~~`chdman` DVD codec support~~ — lzma/zlib/huff/flac accepted; use `-c lzma,zlib`.
- ~~Wii U WUA creator~~ — `rom-converto wup compress` writes Cemu-compatible `.wua`; already in use here.
- ~~Whether a maintained PS3 ISO rebuilder exists~~ — `bucanero/ps3iso-utils` (2022) is the practical
  path; `ifcaro/PS3-ISO-Rebuilder` exists but is unmaintained since 2019.

**Previously open; current disposition follows in Part 8**
- Canonical upstream of **WiiBackupManager** (SourceForge page 404, no GitHub repo found).
- Canonical upstream of **UMDGen** (psdevwiki 403, no repo found); Windows-only mirror binary.
- **NSC_Builder** language-addon flag: the repo README documents the tool's role but the exact
  current flag list should be read from its own release docs before scripting it.
- Whether upstream **WIT 3.05a** should replace the distro 3.01a in `/usr/local` here. Only the
  distro build is installed; 3.05a was run from `/tmp` and its `--psel` surface matches. The tarball
  is parked at `~/Applications/wit-v3.05a-r8638-x86_64.tar.gz` if you want it installed.
- Confirm the meaning of **"PJKS"** with the requester.

## Part 8 — VERIFY closure and capability matrix, 2026-09-18

This section supersedes the older “Still open” bullets where it provides a more precise result.

### WIT/WWT exact command contract

Local `/usr/bin/wit` is `3.01a-4.1build2` (`wit: Wiimms ISO Tool v3.01a r0 x86_64`), installed from
Ubuntu `universe`; Ubuntu 22.04 Jammy also publishes package `wit` in `universe`. Upstream current
documentation is WIT 3.05a. No package was installed during this pass. The exact non-destructive
forms are:

```text
wit copy input.iso output.wia --wia
wit copy input.iso output.wbfs --wbfs
wit copy input.iso output.wbfs --psel data --wbfs
wit copy input.iso output.wia --psel=-update --wia
wit copy input.iso output.wia --psel=whole --wia
wit copy input.iso output.wia --raw --wia
wit verify input.iso
wwt check wbfs-partition-or-file
wwt verify --part wbfs-partition-or-file
```

`COPY` writes a new destination; `--dest`/`--DEST` select it, `--overwrite` permits replacing an
existing destination, `--test` performs a no-write dry run, and `--diff` compares after copying.
`CONVERT` replaces the source after success and is not appropriate for the research-safe workflow.
`EXTRACT` accepts the same `--psel`/`--raw` controls; `EDIT` patches an existing image in place.
`--psel data` keeps only DATA; `--psel=-update` denies UPDATE while retaining other selected
partitions; `--psel whole` disables filesystem-unused-sector scrubbing; `--raw` is `--psel RAW` and
disables scrubbing. `wwt scrub --psel ...` is a WBFS operation, not the WIT ISO command.

WIA and WBFS output are new containers, not byte-identical source files. Raw WIA can reconstruct the
source bytes; default WIA and partition-filtered output cannot restore discarded source bytes.
Status: **VERIFIED-UPSTREAM** for command semantics and **VERIFIED-LOCAL** for installed help/version
output and the matching 3.05a probe.

### maxcso exact result

Local output: `maxcso v1.13.0`. It accepts ISO and CSO inputs, `--decompress` writes a raw ISO,
`--format=cso1|cso2|zso|dax`, `--block=N`, `--output-path`, and `-o output`. Upstream documents zlib
and 7-Zip deflate by default, optional Zopfli/libdeflate deflate trials, and LZ4 for ZSO/experimental
CSO v2. It explicitly targets PSP and PS2 emulators; that is not proof every PS2 loader accepts
every format/block size. Larger blocks may be incompatible with older readers and LZ4 is experimental.
Existing output was overwritten successfully in the local probe (return code 0); EmuWiz should still
use a new destination.

Local 4 MiB synthetic ISO SHA-256:
`79518e710c8b076e5acaa0d88258093b92bba20455ba7ab8ac01abf104036a72`.
CSO1→ISO and ZSO→ISO both produced the same SHA-256 and passed `cmp`. This proves playable
decompression plus byte-exact reconstruction for the tested files, not arbitrary malformed inputs or
all PS2 readers. Licence: ISC. Linux is available by building from source; upstream release assets
are Windows binaries. Status: **VERIFIED-UPSTREAM** plus **VERIFIED-LOCAL**.

### UMDGen and Wii Backup Manager disposition

Original UMDGen remains **UNRESOLVED** for a trustworthy current upstream home, maintenance status,
licence, and redistribution rights. Its historical Windows-only GUI/editor behavior is **CONSENSUS**.
`quickstraw/UMDGenCLI` is a separate .NET recreation with CLI commands (`create`, `open`, `delete`,
`dummy`, `optimize`, `convert`, `extract`, `save`) and Windows-only published binaries; it is not
the original UMDGen. Role: original UMDGen = **research/reference only**; UMDGenCLI = possible
**user-installed helper** only after separate licence/compatibility review.

Wii Backup Manager Build 78 remains **UNRESOLVED** for canonical source, maintenance, licence and
CLI/API. The current archive site supports historical binaries only. Role = **historical/reference
only**. TinyWiiBackupManager is a separate GPL-3.0, cross-platform GUI with partition stripping and
RVZ archiving, but WIT/Dolphin already cover the required safe operations.

### WUA conclusion

The upstream-supported format is **Cemu WUA**. The original standalone source is **Exzap/ZArchive**;
its README defines `.wua` as a Wii U-specific ZArchive layout and its library as MIT-0. Current
cross-platform CLI tooling is **DevYukine/rom-converto** (current main reports 0.17.0; local is
0.15.0): `rom-converto wup compress -o output.wua input...`, plus `wup decrypt`, `wup verify` and
`wup info`. Inputs include NUS/loadiine folders and WUD/WUX with keys; output is `.wua`. Cemu calls
WUA lossless and convertible back to original files. This is file/content-tree reconstruction, not
byte-exact restoration of source WUD/WUX or folder bytes. CLI and Linux availability are
**VERIFIED-UPSTREAM**; local command surface is **VERIFIED-LOCAL**.

### PS3 conclusion

`bucanero/ps3iso-utils` is the cross-platform ISO↔folder utility found: `extractps3iso <ISO> <folder>`
and `makeps3iso <folder> <ISO-or-folder>`, with Docker/Podman support. No byte-exact round trip was
proven. `ifcaro/PS3-ISO-Rebuilder` is stale. PS Multi Tools is an existing GUI wrapper/catalogue,
but its audit shows no round-trip hash proof and AGPL-3.0 covers the application, not all bundled
binaries. RPCS3 workflows are safest around a decrypted JB folder containing `PS3_GAME` and normally
`PS3_DISC.SFB`; `PS3_UPDATE` is firmware/update material, not a generic language-strip target.

EmuWiz classification: **reversible compression/decryption only where separately verified;
profile-driven JB-folder slimming only as an explicit per-game mod; update cleanup only with a
documented profile; unsupported for generic byte-preserving ISO rebuild**. `ps3iso-utils` is a
user-installed/research backend candidate pending licence and fixture tests, not an approved
destructive slimmer.

### CHD DVD confirmation

Exact local help is `chdman help createdvd` and `chdman help extractdvd`. `createdvd` accepts
`-c lzma,zlib,huff,flac` (up to four codecs tried in order); `cdlz` is rejected for DVD. Safe commands
are `chdman createdvd -i source -o output.chd -c lzma,zlib` and
`chdman extractdvd -i output.chd -o restored`. Synthetic source SHA-256:
`d608930215e849e76d869b6896de3f5546c196e08705036e798ea27d4ce0bcd6`; restored SHA-256 is identical
and `cmp` passed. This proves byte-exact restoration for the tested DVD path, not every CHD workflow.

### Compact capability matrix

| Tool | Platform | Operation | Source immutable? | Reversible? | Byte-exact restore proven? | Content stripping? | CLI? | Linux? | Licence | EmuWiz role |
|---|---|---|---|---|---|---|---|---|---|---|
| WIT/WWT | Wii/GC | ISO↔WIA/WBFS; partition scrub | COPY yes; CONVERT/EDIT no | raw WIA yes; scrub/filter no | raw WIA sample: yes | partition only | yes | yes | upstream review | safe backend candidate |
| maxcso | PSP/PS2 | ISO↔CSO/ZSO/DAX | yes with new output | yes | CSO1/ZSO synthetic: yes | no | yes | build source | ISC | safe backend candidate |
| chdman | CD/DVD/PS2/PSP | image↔CHD | yes with new output | yes | DVD synthetic: yes | no | yes | yes | MAME licence | safe backend candidate |
| ZArchive/rom-converto | Wii U | folders/WUD/WUX↔WUA | yes with new output | file-tree lossless | no source-byte proof | no | yes | yes | MIT-0/MIT | safe backend candidate |
| UMDGen 4.00 | PSP | ISO edit/rebuild | normally new output | no after delete | no | yes, game-specific | original GUI; CLI unresolved | Windows | unresolved | research/reference only |
| UMDGenCLI | PSP | scripted ISO/CSO/DAX edit/rebuild | supports output paths | no after delete | not tested | yes | yes | Windows binaries | project-specific review | user-installed helper candidate |
| Wii Backup Manager | Wii/GC | scrub/trim/WBFS/CISO GUI | unclear | no for strip | no | partition/update | no verified API | Windows | unresolved | historical/reference only |
| ps3iso-utils | PS3 | JB folder↔ISO | new output | structural rebuild only | no | no generic strip | yes | yes | inspect before redistribution | user-installed/research candidate |
| PS Multi Tools | PS3/PSP/etc. | GUI wrappers | child-tool dependent | not proven | no | not safely generic | app/child tools | yes | AGPL app; bundled tools vary | existing reference only |

### Remaining unresolved items

- Original UMDGen upstream/licence and original Wii Backup Manager source/licence remain genuinely
  unresolved; neither is approved for redistribution.
- No PS3 byte-exact rebuild proof was performed; generic PS3 ISO rebuild remains unsupported for the
  safe backend classification.
- The exact NSC_Builder language-addon flag remains outside this pass and must be read from its own
  upstream release documentation before scripting it.
- “PJKS” remains ambiguous in the earlier note and is not used in this classification.

All requested VERIFY items are either resolved with upstream/local evidence or explicitly recorded as
UNRESOLVED where trustworthy evidence does not exist. No production code or destructive writer was
added.
