# Preservation media identity audit (V1)

**Scope.** This is a read-only capability and policy audit of preservation
containers. It records what the authoritative EmuWiz tree can actually prove,
not what an extension convention or an emulator might support. No parser,
converter, repair action, or protection bypass is proposed here.

**Authority.** Audited at `eb1ea221bf6cfe31afe3dd176f7e1ca39b8fe3d8` on
`feature/archivefs-unified-platform`. Existing dirty work was deliberately not
used as proof and is outside this document's scope.

## Rules used by this audit

1. A valid preservation container proves its container and, only where its
   format is platform-specific, its media family. It does **not** prove a game
   title, revision, or a good dump.
2. Software identity requires an applicable, independently verified DAT/hash
   match or stronger internal identity evidence. Preservation metadata is
   corroboration, never a title guess.
3. A sector-image conversion is **lossy** whenever it discards flux timing,
   weak/fuzzy bits, deliberate CRC failures, non-standard geometry, custom
   sector layout, or CD subchannel data. EmuWiz must never offer that conversion
   automatically.
4. `PARTIAL` means exactly the documented bounded fields are inspected; it
   never implies track reconstruction, protection emulation, or full media
   fidelity.

## Current implementation evidence

- `disk_format/atari_stx.rs` validates Pasti/STX signature, version, bounded
  track-record chain and declared sector count, but deliberately does not read
  sector descriptors, fuzzy masks, timing, or track data.
- `apple2_disk.rs` validates WOZ1/WOZ2 chunk boundaries and TMAP metadata, and
  classifies NIB only as a nibble-stream container. It explicitly does not
  emulate GCR, reconstruct sectors, or flatten either format.
- `raw_cd_sector.rs` and `raw_cd_logical_media.rs` recognise raw 2352-byte
  sector structure and safely extract Mode 1 / Mode 2 user data. They do not
  expose 96-byte CD subchannels or protection semantics.
- The authoritative media ledger and `docs/research/SONY_PLAYSTATION_SUPPORT_AUDIT.md`
  record no CCD/SUB or SBI parser. `docs/research/ATARI_FAMILY_SUPPORT_AUDIT.md`
  records ATX and IPF as intentionally deferred.

## Capability matrix

`Yes` means current code proves it; `No` means it is absent; `Partial` is
strictly bounded metadata/layout inspection. DAT means the role a DAT may play,
not a claim that a suitable DAT is currently installed.

| Format | Detect | Parse | Timing | Weak bits | Bad sectors | Software ID | DAT role | Conversion loss | Recommended EmuWiz scope |
|---|---|---|---|---|---|---|---|---|---|
| Amiga IPF | Extension / launch-format evidence only | No | No | No | No | No | Whole-file hash only if a compatible preservation DAT exists | Raw-sector conversion is lossy; timing/protection semantics may be lost | **Defer / detect-only.** Preserve file unchanged; require explicit CAPS/SPS backend evidence for launch. |
| SCP flux | No production detector/parser | No | No | No | No | No | Future format-aware or whole-file preservation DAT only | **Lossy** to sectors: flux timing, repeated revolutions, weak regions and custom layout can be lost | **Defer.** A future read-only header/track-table inspector is feasible only after format and fidelity policy work. |
| KryoFlux stream | No production detector/parser | No | No | No | No | No | Whole-file provenance/hash only, if an appropriate dataset exists | **Lossy** to sectors; stream data is capture/protocol-oriented and may preserve flux timing | **Detect-only/defer.** Do not treat as a stable logical disk format or shell out to DTC. |
| Apple WOZ | Yes (`WOZ1`/`WOZ2`, marker, bounded chunks) | Partial (chunk bounds, TMAP) | Partial (bitstream container is retained; EmuWiz does not interpret timing) | No semantic interpretation | No semantic interpretation | No | TOSEC-style whole-image matching remains external corroboration | Usually **lossy** to DO/PO/DSK for protection/non-standard layout; never auto-convert | **Safe next incremental task:** read-only INFO/TRKS structural summary and explicit fidelity limitations. |
| Apple NIB | Yes (bounded geometry / container classification) | Partial (nibble-stream classification only) | No | No semantic interpretation | No semantic interpretation | No | Whole-image hash only | **Lossy** to sectors: GCR layout, address/data-field irregularities and protection may be lost | **Keep bounded detect-only** until GCR/bitstream work has a proven preservation model. |
| C64 G64 | Extension/launch support; no core structural parser proven | No | No | No | No | No | Whole-image hash only | **Lossy** to D64: GCR track stream, half-tracks and speed zones are not retained by logical sectors | **Safe next research/parser task:** read-only G64 header/offset-table validation; do not decode to D64 automatically. |
| C64 NIB | No production parser proven | No | No | No | No | No | Whole-image hash only | **Lossy** to D64 for raw nibble/GCR and non-standard format data | **Defer** pending an authoritative NIB dialect decision; extension alone is not enough. |
| Atari ST STX/Pasti | Yes (`RSY\\0`, supported version) | Partial bounded track-record-header walk | No | Not read | Not read | Container settles Atari ST media family, not title | DAT/TOSEC hash can corroborate title only | **Lossy** to ST/MSA: fuzzy masks, timing, sector descriptors and track data can be lost | **Current safe scope is correct:** retain header-level evidence and refuse reconstruction. |
| Atari 8-bit ATX/VAPI | No production parser | No | No | No | No | No | Whole-image hash only | **Lossy** to ATR/XFD: timing, weak sectors and deliberate error behaviour may be lost | **Defer.** First obtain a second independent format/implementation review, then consider bounded header/table inspection. |
| Raw CD 2352 sectors | Yes in raw-sector/logical-media paths | Partial (Mode 1 and Mode 2 user-data extraction) | N/A (optical sector timing absent) | No | Partial only where raw sector/header structure remains; not protection semantics | No by itself | Redump track/disc data can corroborate only with exact matching byte-domain semantics | Cooked ISO conversion is **lossy** for raw headers/ECC/EDC, Mode 2 details, layout and all subchannel | Keep read-only logical extraction; add per-track parity only after exact raw/cooked hash-domain contract is specified. |
| SUB / CD subchannel | No production parser | No | N/A | Potentially relevant but not interpreted | Potentially relevant but not interpreted | No | Separate companion evidence; never imply match from BIN/ISO alone | Dropping `.sub` is **lossy**, including Q-channel and protection-related data | **Intentionally deferred.** Detect companion relationships first; no interpretation or bypass. |
| SBI-style external protection evidence | No production parser | No | N/A | May encode correction/protection metadata, not media bytes | May describe intentional read anomalies | No | Separate preservation/protection provenance only; not a Redump substitute | Omitting SBI is **lossy** for the represented drive-read/protection evidence | **Intentionally deferred.** Never apply patches or synthesize sectors automatically. |

## Format notes and evidence boundaries

### IPF / SPS

IPF is a preservation format tied to the SPS/CAPS library ecosystem. The SPS
download page says its User and Developer libraries are bound by SPS Freeware
terms and that commercial distribution requires the separately licensed Access
API arrangement. It also recommends dynamic linking for third-party use.

**Decision:** EmuWiz should not bundle, reimplement, or shell out to CAPS/SPS
as part of core identity work. Keep IPF read-only/detect-only and retain the
existing explicit launch readiness gate (`CAPS/SPS backend evidence required`).
Its opaque/controlled decoding ecosystem is a licensing and reproducibility
risk, not merely a missing parser.

### SCP and KryoFlux streams

SCP has a published format and an explicit `SCP` magic; it is a flux container,
so a future bounded header/track-index reader is plausible. It must remain
distinct from interpreting flux into sectors. KryoFlux streams are documented
as a persisted USB stream protocol rather than a long-term logical disk
format; their documentation warns that the protocol/file format may change.

**Decision:** no external-tool shelling. A future Rust library evaluation may
consider a permissively licensed flux reader (for example, FluxFox only after a
separate dependency/license/security review), but its output must stay
read-only and preserve capture/fidelity limitations.

### WOZ and Apple NIB

WOZ is openly documented by Applesauce and stores Apple II encoded/nibblized
track bitstreams. The Library of Congress format description notes its purpose
is to retain the encoded layout rather than reduce a disk to ordinary logical
sectors. Current EmuWiz therefore makes the right bounded claim: valid chunked
WOZ plus TMAP evidence, no GCR decoding. NIB is even less self-describing in
the current implementation and remains a nibble-stream classification.

**Decision:** a bounded WOZ structural enrichment is the lowest-risk next
preservation task. It must not equate a valid WOZ/NIB image with a specific
release and must clearly label a logical export as potentially lossy.

### G64 / C64 nibble images

VICE documents G64 as a GCR-encoded 1541 raw track-stream format with a header,
track offsets and speed-zone data; it was designed for non-standard and
copy-protected disk layouts that D64 cannot represent. That makes a G64-to-D64
conversion intrinsically conditional and often lossy. The repository currently
launches documented G64 content through VICE but does not prove a core G64/NIB
structural parser.

**Decision:** implement header/table validation before any GCR decoding, and
never derive title identity from GCR shape or launchability.

### STX / Pasti and ATX / VAPI

Current STX inspection proves the Pasti header and a bounded, internally
consistent record chain only. It intentionally avoids fuzzy masks, timing and
track/sector payloads; these are precisely the fields that make preservation
images different from ordinary sector dumps. The VAPI/ATX documentation
describes a structured header followed by track, sector and extended-sector
records, but the existing Atari audit correctly requires independent review
before a parser is adopted.

**Decision:** retain the current STX boundary. Defer ATX until there is a
tested table-walk design that reports, rather than repairs, sector anomalies.
Neither format supplies software identity on its own.

### Raw CD sectors, SUB, and SBI

Raw 2352-byte sectors have more information than an ISO/user-data projection,
but do not include the separate 96-byte per-sector subchannel stream. Current
code safely distinguishes/extracts supported Mode 1 and Mode 2 user data, and
the authoritative ledger labels subchannel/weak-sector fidelity incomplete.
SBI and companion SUB data must stay independent evidence: the absence of a
companion means **unavailable**, not a clean disc; the presence of one never
authorizes applying a patch or bypassing copy protection.

**Decision:** first add a non-invasive companion-file relationship model and
explicit fidelity status. Only then evaluate read-only Q-subchannel parsing.
No code should claim LibCrypt or other protection equivalence from cooked or
raw user data alone.

## Licensing and dependency assessment

| Area | Evidence | Recommended choice |
|---|---|---|
| IPF/CAPS | SPS library distribution and Access API terms constrain redistribution/integration | **E — intentionally defer** in core; retain explicit external-backend readiness. |
| STX/Pasti | Current bounded parser is self-contained; full semantics are specialised and protection-sensitive | **A — small internal read-only extension only** after independent format validation. |
| ATX/VAPI | Publicly described but current repository audit says no independent implementation review | **E — defer** until that review is complete. |
| SCP | Published specification; flux semantics still require careful preservation modelling | **B — evaluate a library** only in a dedicated, license-reviewed lane; otherwise detect-only. |
| KryoFlux stream | Protocol/capture origin and changing-format caveat; DTC is proprietary tooling | **D — read-only/detect-only**; do not shell out. |
| WOZ | Openly documented chunked format | **A — internal bounded parser/enrichment** is suitable. |
| G64 | VICE documents its header/track representation | **A — internal header/index parser** is suitable; GCR recovery is a separate lane. |
| SUB/SBI | Companion/protection formats require exact optical and protection semantics | **D/E — detect relationship only, otherwise defer.** |

## Lossy-conversion policy

All of the following must be classified **LOSSY** when converted to a plain
logical-sector image without a separately demonstrated round-trip contract:

- IPF, SCP, KryoFlux stream, STX, ATX: flux/timing, fuzzy/weak bits,
  non-standard sectors, deliberate CRC/read failures, and non-standard
  geometry may be discarded.
- WOZ, NIB, G64: encoded/nibble/GCR stream and layout-specific protection
  information may be discarded.
- raw CD 2352 to ISO: sync/header, EDC/ECC and Mode 2 distinctions may be
  discarded; SUB/SBI information is not represented at all.
- SUB/SBI omission: channel/protection/read-behaviour evidence is discarded.

Therefore EmuWiz should only ever offer a future conversion as an explicit,
new-output, user-confirmed operation that reports exactly what cannot be
preserved. This audit does not recommend adding one.

## Intentionally unsupported / deferred

- Full IPF decode/reconstruction: SPS licensing and opaque semantics.
- Flux-to-sector decoding for SCP/KryoFlux: fidelity policy and multi-revolution
  semantics are not yet modelled.
- ATX sector/weak-bit reconstruction: independent VAPI validation still needed.
- STX fuzzy-mask/timing/track-data interpretation: not required for current
  safe container validation and should not be guessed.
- G64/NIB GCR recovery: needs a dedicated preservation-aware decoder.
- SUB, CCD/SUB and SBI interpretation: no current companion model, raw
  subchannel reader, or safe protection-evidence policy.
- Any automated repair, protection patching, conversion, or title inference
  from preservation structure.

## Top three recommended preservation tasks

1. **WOZ structural enrichment (small, low risk):** retain current bounded
   chunk parsing and add a clear INFO/TRKS/track-map summary with no GCR
   decoding, no export, and explicit fidelity limits.
2. **G64 header and offset-table validation (small/medium):** a read-only
   bounded parser using the documented VICE layout, exposing only track/half-
   track and speed-zone presence; defer GCR-to-sector conversion.
3. **Optical companion-fidelity model (medium):** associate CUE/BIN/CCD with
   optional SUB/SBI companions and surface `present`, `absent`, or `unavailable`
   without parsing/providing protection workarounds. It should unblock honest
   Redump/logical-media reporting without claiming protected-disc equivalence.

## Sources consulted

- Software Preservation Society, [IPF library download and Access API terms](https://www.softpres.org/download).
- Software Preservation Society, [KryoFlux stream description](https://www.softpres.org/kryoflux%3Astream).
- Library of Congress, [WOZ Disk Image](https://www.loc.gov/preservation/digital/formats/fdd/fdd000642.shtml), and Applesauce, [WOZ reference](https://applesaucefdc.com/woz/reference1/).
- CBMSTUFF, [SuperCard Pro](https://www.cbmstuff.com/index.php?product_id=52) and the IETF media-type registration citing its published SCP specification.
- VICE, [G64 GCR disk image format](https://vice-emu.sourceforge.io/vice_17.html).
- Whizzo Software, [VAPI / ATX Disk Image Format](https://www.whizzosoftware.com/sio2arduino/vapi.html).

These external references inform future feasibility only. The matrix's current
capability claims are grounded in the authoritative repository files named
above.
