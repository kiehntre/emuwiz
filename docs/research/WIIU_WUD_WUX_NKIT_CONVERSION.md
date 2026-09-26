# Wii U WUD/WUX/NKit conversion research and gap audit

Status: research-only. No keys, copyrighted images, external downloads,
conversion backend, or production mutation were added.

## Executive result

WUD is the raw/encrypted Wii U optical-disc representation. WUX is a
compressed WUD representation: compression/decompression is intended to retain
the logical WUD bytes, but byte identity must be verified for the exact tool
version and input rather than assumed from the extension. JWUDTool explicitly
provides WUD/WUX compression, decompression, image comparison, hash extraction,
and optional decryption; it also documents common-key/title-key requirements
for decrypted partition/file access.

WUD→WUX is therefore a candidate lossless storage conversion. WUX→WUD should
be byte-identical when a conforming decompressor reconstructs the original
WUD block stream, but EmuWiz has no local implementation or tool-version
verification yet. It must not label that result byte-exact today.

NKit is a separate normalization/recovery family, primarily documented for
GameCube/Wii. It can remove or reconstruct junk, hashes, scrubbed regions, and
padding according to NKit/DAT rules. NKit output is not a preservation-equivalent
WUD/WUX representation and must not be treated as an original-disc hash.
NKit2's DataStore documentation mentions Wii U support, but that does not by
itself establish a safe WUD/WUX round-trip or an identity-preserving converter.

Sources:

- [JWUDTool documentation](https://github.com/Maschell/JWUDTool/blob/master/README.md)
- [Cemu WudCompress](https://github.com/cemu-project/WudCompress)
- [NKit README](https://github.com/Nanook/NKit/blob/main/README.md)
- [NKit format notes](https://wiki.gbatemp.net/wiki/NKit/NKitFormat)
- [NKit disc/recovery notes](https://wiki.gbatemp.net/wiki/NKit/Discs)
- [Cemu setup/documentation summary of WUD/WUX](https://wiki.cemu.info/wiki/Serfrosts_Cemu_Setup_Guide)

## Format matrix

| Representation | Structure/semantics | Keys | Identity evidence | Preservation decision |
|---|---|---|---|---|
| WUD | Raw/encrypted Wii U disc image, including partition/layout data and encrypted content | Not needed merely to hash or compress; required for decryption/extraction | Full-file/DAT hash first; structural header only after a bounded parser exists | Preservation candidate; currently inspect/hash only |
| WUX | Compressed WUD container/block stream | Same distinction as WUD; decryption still needs keys | WUX hash identifies the container, not automatically the decompressed WUD | Lossless-storage candidate only after round-trip byte verification |
| WUD parts | Split raw WUD segments, commonly FAT32-oriented | Same as WUD | Ordered part set plus aggregate hash | Must reject missing, duplicate, reordered, or ambiguous parts |
| Extracted Wii U title | `code/`, `content/`, `meta/`; decrypted/install-style files | Usually already decrypted; title-key handling occurs before extraction | `meta.xml` title ID/product/version are self-reported; file hashes and DAT/provider evidence are stronger | Playback/install convenience representation, not disc-byte equivalent |
| NKit/NKit2 | Normalized/recovered/scrub-aware disc or datastore representation | Tool/DAT dependent | NKit/DAT recovery evidence, not original raw hash by default | Convenience/recovery; never silently called preservation-equivalent |
| Raw/decrypted partitions | Decrypted content extracted from WUD/WUX | Common key/title key as applicable | Partition/file hashes and title metadata | Useful derived evidence; not reversible to original encrypted bytes without a proven writer |

## Conversion semantics

### WUD and WUX

WUX compression is intended to be a reversible storage transformation of WUD
data, not a filesystem conversion. A safe implementation would require:

1. bounded WUD/WUX header and block parser;
2. checked block offsets, lengths, sparse/padding rules, and overflow handling;
3. local-only key handling for any operation beyond compression;
4. source and reconstructed WUD hashes;
5. tool/version/options recorded in provenance; and
6. repeated conversion plus WUX→WUD comparison before claiming byte identity.

Compression itself should not require title keys. Decrypting partitions,
extracting files, or building installable content may require the Wii U common
key and title/game key. EmuWiz must never log, upload, or commit those values.

### NKit and Wii overlap

NKit's documented purpose is recovery/storage efficiency for GameCube/Wii
images. Its processing can normalize scrubbed data and discard or reconstruct
disc junk/hash regions. Recovery may rely on DAT data and can produce bytes
that are valid or reconstructable without being the original source bytes.
NKit2's broader datastore support is not evidence that every NKit/NKDS output
has a reversible WUD/WUX mapping. Treat Wii and Wii U pipelines as separate
until a format-specific contract is verified.

### Cemu and extracted directories

The current Cemu adapter recognizes WUD, WUX, and WUA but marks WUD/WUX as
key-dependent and refuses them because EmuWiz has no safe container parser.
Only an extracted `code/content/meta` directory reaches launch planning.
`meta.xml` is useful self-reported evidence, not a cryptographic identity
anchor. The directory can be hashed and launched, but converting it back to
the original encrypted disc is not presumed reversible.

## Identity and verification policy

Identity precedence should be:

1. exact Redump/DAT full-image hash;
2. exact validated WUD/WUX reconstructed-image hash;
3. exact partition/component hashes bound to a verified title/disc structure;
4. validated title/disc metadata and partition topology;
5. extracted `meta.xml` title ID/product code;
6. filename or folder name only as a weak hint.

Before conversion, record the source representation, path set, sizes, hashes,
WUD/WUX structural facts, title/disc identifiers, key availability state
(never the key), tool/version/options, and whether the source is encrypted,
decrypted, scrubbed, split, or reconstructed.

After conversion, verify the output hash and size, container structure, ordered
part set, partition boundaries, title/disc identity, and all available DAT or
component evidence. For WUD↔WUX, only a reconstructed WUD hash equal to the
source WUD hash earns `ByteExactRestoreVerified`. For NKit, use
`SemanticEquivalent` or `StructuralVerified` only when the NKit/DAT contract
supports it; never upgrade to raw-byte equivalence from matching title ID.

## Gap matrix

| Area | Current EmuWiz state | Gap | Safe next step |
|---|---|---|---|
| Wii U platform registration | WUD/WUX/RPX strong extensions; no structural magic | Extension is not identity | Add bounded container inspection |
| WUD identity | No WUD parser | No title/disc/partition evidence | Parse headers/layout without decrypting |
| WUX identity | No WUX parser | Compression validity and logical size unknown | Parse blocks and verify reconstruction |
| Keys | Cemu readiness models `keys.txt` as setup evidence | No local key-aware Wii U container reader | Key-presence state only; never expose key material |
| Extracted title | `code/content/meta` layout and metadata extraction exist | Self-reported metadata is not source identity | Bind hashes and DAT evidence to directory records |
| Conversion | Planner supports unrelated ISO/WIA/CSO/CHD paths; no WUD/WUX backend | No safe execution, verification, or provenance | Add WUD↔WUX plan only after parser/round-trip proof |
| NKit | No native NKit/WUD support | Normalization/recovery semantics unmodeled | Add explicit convenience/recovery classification, not generic conversion |
| DAT | Wii U DAT source configuration exists | No WUD/WUX structural join | Join only exact local hashes after parser work |
| Provenance | Existing conversion/patch provenance primitives exist | No Wii U representation chain | Record source/output hashes, tool/version, options, key-state, and verification level |
| Launch | Extracted titles can reach Cemu; WUD/WUX/WUA are refused | No bounded media reader | Keep refusal until parser and local key semantics are proven |

## Refusals

EmuWiz should refuse missing or reordered WUD parts, malformed headers,
overflowing block offsets, unsupported compression variants, missing required
keys for decryption, ambiguous title/partition selection, unverified NKit
recovery claims, WUD/WUX-to-extracted conversion presented as reversible, and
any conversion that overwrites the source or silently discards padding,
scrubbed data, hashes, partitions, updates, or DLC.

Keys remain local-only. No key is printed in diagnostics, persisted in
provenance, sent to providers, or committed to the repository.

## Recommended implementation order

1. Add a bounded read-only WUD/WUX container parser and split-part resolver.
2. Add structural identity facts and deterministic local hashing.
3. Add local key-state/readiness without key logging or network access.
4. Add WUD↔WUX read-only conversion planning with exact reconstructed-WUD
   verification.
5. Add transaction/history/provenance only after repeated round-trip proofs.
6. Add extracted-title and NKit classification as separate semantic/recovery
   paths; do not merge them with preservation-equivalent conversion.
7. Add GUI preview only after backend verification states are real.

POC: not implemented. The missing primitive is a format parser plus a
versioned, independently verified WUD/WUX block conversion contract; a
synthetic parser without that contract would create false confidence.
