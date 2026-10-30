# PS1 multi-disc PBP export, Phase 1

Core exposes an explicit export plan and executor. It accepts only two to five
current, DAT-verified PS1 CUE/BIN discs in one release. Phase 1 requires one
`MODE2/2352` BINARY track per disc, `INDEX 01 00:00:00`, no gap or extra-index
declarations, a verified PS1 serial, and an unambiguous USA, Europe, or Japan
region agreed by the DAT title and serial. ISO, CHD, audio tracks, other raw
track modes, and subcode-dependent discs are outside this contract.

The external writer is the `psxpackager-linux-x64.tar.gz` asset from upstream
release `v1.6.3`, archive SHA-256
`378ecb4d2310c8f72fc77ec092e50797427de79d6e001ab0b56eaad4699890da`.
The extracted executable SHA-256 is
`2ab8477755b6307dbf4f1c5af496245b720a61c289f9056a7d1d58375a0b000f`.
The executable reports `psxpackager 1.6.2`; the exporter verifies that exact
string and the hashes of the executable and required resources. The upstream
project's source is MIT-licensed. The release archive also contains `BASE.PBP`
and `KEYS.BIN`, whose redistribution rights were not established here. EmuWiz
does not bundle or redistribute the archive; the user supplies the pinned
upstream installation. The test script downloads it into temporary storage.

Export copies the verified source files into private staging, creates and
extracts a PBP there, and compares every extracted raw BIN byte and serial
with its original. The PBP and receipt are published by a journaled no-clobber
move, receipt first. An interrupted transaction retains its stage for Repair
history and rollback. Original discs and their DAT identity remain in place.
The receipt says **Derived playing copy: retain original discs**. PBP output
does not become a DAT-verified source or automatic launch target.

For admitted discs, a successful receipt proves full raw BIN bytes survived
this writer's compress-and-extract round trip. It does not certify every PBP
consumer, retain the original CUE text, or cover audio, subchannel, and gap
layouts rejected above. Keep the original DAT-verified CUE/BIN set as the
preservation copy. After publication, temporary input and extraction copies
are removed; the journal and minimal recovery stage remain for Undo.

Run the real synthetic two- and five-disc acceptance suite with
`bash scripts/test-ps1-pbp-export.sh`; the ordinary Core test suite marks those
external-writer cases ignored and reports them as such.
