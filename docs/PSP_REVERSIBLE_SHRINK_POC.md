# PSP reversible shrink POC

This converter writes PSP ISOs as standard CSO v1 (raw deflate blocks; index bit 31 marks a block stored plain; byte offset = index << align, align 2), the format PPSSPP, PCSX2 and maxcso read. Files written before the ALCHEMY Q1 correction (2026-11) used a private variant (zlib-wrapped blocks, bit 31 on compressed blocks, align 0 with offsets divided by four) that no standard reader opens; `classify_cso_dialect` gives a header-level hint for such files, `confirm_legacy_emuwiz_cso` confirms one by decoding every block, and `restore_legacy_emuwiz_cso` recovers it into a new ISO without modifying it.

Conversion is refused before anything is written unless all of these are available: an independent validator (maxcso at a pinned absolute path), a safe diagnostics location on the output's own filesystem, and free space for the CSO plus one transient restored ISO plus a margin.

The diagnostics location is the first of these that passes every check (same filesystem as the output, a real directory owned by the user and not writable by others, no symbolic links, neither inside nor containing a configured game source folder, enabled or disabled):

1. a folder the user configured, one absolute path per line in `cso-diagnostics-dirs.txt` in the EmuWiz configuration folder (it must already exist, be owned by you and not be writable by other users or your group, for example after `chmod 700`; EmuWiz never creates it);
2. `diagnostics/` in EmuWiz's user data folder;
3. `<mount point>/.emuwiz-diagnostics/`, only where the user can already create it.

EmuWiz never needs root or sudo and never changes mount permissions. If no candidate passes, the conversion is refused and the message lists why each candidate was skipped and how to configure a folder.

The work happens in `staging/<id>/` inside that location. Four results are kept separate:

1. Format validity: standard CSO v1 with align 2, every block decodable by EmuWiz's standard reader.
2. Content integrity: EmuWiz's restore has the source SHA-256.
3. Independent reversibility: maxcso's restore has the source SHA-256. EmuWiz reading its own output is never called independent.
4. Emulator compatibility: never tested by conversion; always reported as not tested.

Only when 1-3 pass is the CSO published at the output path, by a same-filesystem hard link that never replaces an existing file. Any failure publishes nothing and keeps the failed CSO, `intent.json` and `diagnostic.json` under `failed/<id>/`, listed in `manifest.jsonl`; if the manifest cannot be written the evidence is still kept and its location reported. An interrupted run leaves its stage and `intent.json` in `staging/`. Retained evidence is never deleted automatically; cleanup controls are a later, separately confirmed action. The source ISO is only ever opened for reading.

The current GUI surface is intentionally deferred from this isolated core POC because there is no existing non-conflicting converter panel to extend while the provider and managed-snapshot work proceeds. The intended panel contract is `Converter > Reversible Shrink > PSP ISO -> CSO`, showing original size, compressed size, saved space, CSO readability, restored-hash equality, and `REVERSIBLE VERIFIED`, with `Keep both`, `Keep compressed copy`, and `Cancel`. `Delete Original` is deliberately absent. The core result is sufficient for that panel to remain a presentation layer over a verified operation receipt.

The PSP boot evidence module remains the identity integration point. The shrink operation preserves source hash and conservative PSP ISO evidence in provenance; it does not infer a game identity from compression or permit metadata to replace identity evidence.
