# Xbox / XISO reversible shrink research

## Result

This slice adds read-only XISO analysis and an explicit reversible-state model.
It does not write, rebuild, trim, delete, replace, or mutate an Xbox image.
The result is:

**NO BYTE-EXACT XISO SHRINK PATH PROVEN**

An image that boots after an extraction/rebuild is only evidence of
playability. It is not evidence that the original raw bytes can be restored.

## Formats investigated

The relevant target is the original Xbox XDVDFS filesystem, commonly carried
in either a raw/stripped XISO or a full Redump-style dump with leading and
other physical/security-sector regions. XDVDFS is also used by Xbox 360, so a
volume signature alone cannot identify the console generation. Xbox 360 XEX,
STFS/GOD and Xenia packaging remain outside this original-Xbox shrink task.

EmuWiz already has bounded XDVDFS inspection. Its file-backed traversal tries
the upstream `xdvdfs` crate's fixed offsets for raw/stripped and XGD1/XGD2/XGD3
layouts, and never materializes a multi-gigabyte image merely to inspect
`default.xbe`. Directory traversal has a read-call budget and bounded file
prefix reads.

## Why a rebuilt XISO is not reversible

An XDVDFS rebuild can preserve the logical files and produce a playable image,
but it may change directory-table placement, file-sector placement, alignment,
leading/trailing padding, and physical dump regions. Unused-looking bytes are
not generally reconstructible from the filesystem tree. A byte-for-byte
restore would therefore need to retain every omitted byte plus its original
offset and layout, which is a lossless container/backup representation rather
than a proven XISO shrink.

Removing an outer region is safe only when the exact region boundaries and all
removed bytes are retained as restoration data. No such representation is
currently implemented or established for arbitrary Xbox images. A smaller
playable XISO must remain `PlayableButNotByteExact`.

## Tool findings

`extract-xiso` was not installed on the host during this bounded audit. The
existing source contains no XISO writer or shell-based XISO conversion path.
`xemu` is installed at `/home/davedap/.local/bin/xemu` and has existing launch
readiness support. Its version probe could not connect to the restricted
display environment, and no real image was launched by this POC. A future
bounded boot check would be reported separately as `IMAGE ACCEPTED` or `BOOT
CHECK PASSED`; it could never upgrade a result to byte-exact reversible.

No external tool is invoked by the current analyzer, so there is no tool
version/options or shell-command provenance to record. If a future tool is
added, it must use structured `Command::new` plus `args`, explicit output
paths, collision refusal, bounded execution, and a restore/hash verification
before any reversible claim.

## Analyzer states

`ReversibleExact` is reserved for an actual derived representation whose
restored bytes equal the source SHA-256. The current analyzer never produces
that state.

`PlayableButNotByteExact` means bounded XDVDFS structure is readable, while
exact raw restoration is unproven. `Unsupported` means a signature or
structure check failed. `Unknown` means the input is insufficient to classify.

The analyzer records source size and SHA-256, does not alter source bytes, and
refuses a proposed output collision before returning the no-byte-exact-path
result. It does not calculate a shrinkable amount because no derived output is
authorized.

## Future Converter contract

The eventual location remains:

`Organise & Export → Converter → Reversible Shrink → Xbox / XISO`

The normal card can show source size, a derived size only after a real
transformation, savings, verification state, source/restored hashes, and
“original unchanged”. Advanced details can show layout and provenance. It
must not offer Delete Original, and Game Slimmer/content deletion must remain
a separate feature.

Verification levels must remain distinct:

1. `STRUCTURAL VALID`
2. `IMAGE ACCEPTED`
3. `BOOT CHECK PASSED`
4. `BYTE-EXACT RESTORE VERIFIED`
5. `PLAYABILITY VERIFIED`

Only level 4 may be labelled reversible.

## Next step

If a future lossless container is designed, it must preserve the complete
source hash, all omitted bytes, exact offsets/layout metadata, tool/version,
options, output hash, restored hash, and a collision-safe derived path. It
must be tested against raw XISO and Redump-style specimens before any GUI or
automatic conversion is added.
