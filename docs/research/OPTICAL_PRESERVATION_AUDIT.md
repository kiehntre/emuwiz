# Optical preservation support audit

This audit describes the current EmuWiz support surface for GameCube/Wii and Wii U disc containers. Inspection evidence is deliberately kept separate from proof of reconstruction. The Wii U queue is the only native conversion executor in this area; GameCube/Wii image routes elsewhere are inspection or provider/conversion-planning evidence.

## Supported behavior

| Format | Existing behavior | Preservation evidence and limit |
| --- | --- | --- |
| GameCube/Wii ISO/GCM | Bounded header/boot evidence and identity extraction. The `nod` provider can inspect ISO/GCM, RVZ, CISO and WBFS. | Header/identity evidence is not a full media hash or filesystem verification. No native GameCube/Wii executor was found in the audited paths. |
| NKit ISO | Bounded NKit v1 header inspection and read-only recovery preview. | A recognized header establishes only `NkitV1HeaderRecognizedBodyUnverified`. `Unknown` recoverability is not proof of recoverability; a declared missing update dependency is `DependenciesMissing`. A supplied recovery candidate is measured but does not enable reconstruction. No apply/queue implementation exists. |
| NKit GCZ / NKit GZ | GCZ wrapper signature can be recognized by the NKit inspector as unsupported. | No decompression, inner-image inspection, or recovery is performed. No separate NKit GZ path was found. |
| Wii U WUD | Structural/header inspection and native WUD-to-WUX conversion. | Conversion reads and hashes the complete logical WUD byte stream; successful output is verified against that stream. It does not claim to reproduce an input WUX physical layout. |
| Wii U WUX | Bounded header/table validation, sector-map inspection, native WUX-to-WUD conversion. | Conversion reconstructs the mapped logical sector stream and verifies its full SHA-256. Repeated references are expanded. Physical payload blocks not referenced by the map are omitted and now counted/warned. The input remains unchanged. |
| Wii U WUA | Detection as a separate representation. | Explicitly unsupported by the WUD/WUX conversion queue. |
| RVZ/WIA, CISO, WBFS | Read-only identity/boot evidence and/or storage conversion capability recommendations. | No native executor was found for these formats in this work area. The storage capability model marks ISO-to-RVZ and ISO-to-CSO as `PlayableNotOriginalReconstructable`; that is not byte-exact preservation. The generic RVZ planner classifier now returns `Unknown` until a concrete route and verifier exist. |

No code path found adds Wii U keys, decrypts partitions, or extracts game content. WUD/WUX operations preserve the source file and write a new target through the existing staged/journaled queue. The durable record keeps distinct physical source-container, reconstructed WUD stream, and output hashes; these hash domains must not be conflated.

## Preservation states and claims

NKit assessment has separate representation and recoverability fields. `NkitV1HeaderRecognizedBodyUnverified` means only that the bounded NKit v1 header was recognized. `Unknown` means the available header/recovery evidence does not establish reconstruction; `DependenciesMissing` means the header declares an update recovery dependency for which no matching user-supplied candidate was provided. Candidate metadata agreement is not content authentication. No state currently asserts exact reconstruction because no NKit reconstruction/verifier exists.

WUD/WUX verification proves equality of the complete logical disc byte stream represented by the source reader and the generated target reader. It does not prove physical container byte identity. WUX payload blocks absent from the sector map are outside that logical stream and are not copied to WUD; preview reports their count. Exact physical reconstruction is therefore not claimed. The source remains available unchanged.

The native Wii U verification steps compare full stream hashes, output geometry and WUD header evidence. This establishes logical stream equality for the conversion result, not the provenance or authenticity of encrypted disc contents. A successful conversion is not described as recovering original physical bytes.

## Boundaries and remaining limits

- The `nod` library provides format evidence, but those checks do not scan/hash every source byte unless a distinct conversion verifier does so.
- NKit recovery remains inspection-only. Recovery partition structure, body validity, reconstructed image checksums and an independent full-output witness are not established.
- WUD/WUX inspection validates container extents and the sector map, not encryption keys or partition semantics.
- WUX inspection retains the bounded table and a bounded seen-map proportional to table/stored-block count. Conversion streams sectors and hashes with fixed-size buffers; the mapping table is required to resolve logical order.
- ISO/GCM/RVZ/WIA/CISO/WBFS native conversion is not added here. Their external-tool/provider capabilities must not be presented as proof of lossless round-trip unless the concrete route supplies an appropriate verifier.
- Existing `WIIU_WUD_WUX_PRESERVATION.md` contains detailed format behavior; its statement that GUI projection is future work is stale because the queue now exists.
