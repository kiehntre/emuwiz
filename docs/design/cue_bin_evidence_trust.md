# CUE/BIN observation and custody contract

This additive core adapter implements design A from the approved 2026-10-11
*CUE/BIN content-stability trust model*. The existing preservation engine,
original regression assertions and conversion gates are unchanged.

`ingestion::cue_evidence_trust::assess_cue_observations` accepts references to
already collected `Result<DiscLayoutEvidence, CuePreservationError>` operands.
It calls the existing in-memory comparator. It does not collect, reopen, hash,
copy, monitor or mutate any file. It does not accept a public, caller-fabricated
`CuePreservationComparison` as proof.

The returned `CueObservationAssessment` has private fields and read-only access:

| Dimension | Typed result | Meaning |
|---|---|---|
| Complete observed physical bytes/layout | `MatchingBytesObserved`, `DifferentBytesObserved`, `NotCompared` | Relationship of previously read streams and supported layout facts. A layout difference can coexist with matching program bytes. |
| Per-operand representation | `MutableReadObservation` | No retained protected payload, atomic acquisition or custody guarantee. |
| Per-operand source | `ChangedSource(error)` or `IndeterminateSource(reason)` | Positive detected change, collection failure, or unproven continuing stability. |
| Per-operand layout | `SupportedLayout { contract }`, `UnsupportedLayout(error)`, `NotEstablished` | Existing supported schema or the exact refusal; an I/O failure does not establish layout support. |

Every successful ordinary observation has source assessment
`IndeterminateSource(ContinuousStabilityUnproven)`. This applies even to files
that happen to be unchanged, repeated identical reads, read-only descriptors,
readonly paths, apparently precise timestamps, private directories, and all
filesystem types. SHA-256 binds returned byte sequences; it does not exclude a
writer or prove those sequences existed as one complete file or coherent set at
one instant. Concurrent writes can assemble a stream from several versions.

The operand retains a borrowed reference to the original opaque evidence or
error. Original CUE text/digest, paths, source identities, FILE/track membership,
alias relationships, timeline, full-file hashes and range hashes remain
accessible without alteration. This adapter invents neither an acquisition time
nor a capture identifier. It does not replace an earlier baseline with current
bytes. The caller retains the actual evidence record; it is not a BIN capture.

`refusals()` keeps both errors and their left/right attribution in deterministic
order. Unsupported left and changed right remain separate facts. The adapter
uses the existing comparator's unsupported classification for failed operands;
these error-only calls inspect no successful evidence. `observed_programs_match`
is absent for refused/incomplete comparisons and is always a historical physical
program-byte/mode fact, never a way to override layout or custody status.

## Reserved captured representation

`RepresentationTrust::VerifiedStableCapturedRepresentation(CaptureGuard)` is
reserved for a future, separately implemented and independently reviewed custody
backend. `CaptureGuard` currently has a private uninhabited field, no constructor,
no deserialization and no cloning or conversion from observations. It cannot be
manufactured by safe production code. `require_verified_capture()` always returns
`CustodyBackendUnavailable`. No filesystem or platform currently qualifies.

A future backend must enforce and bind all-member protected bytes, membership,
manifest identity, schema and generation through actual use. It must distinguish
an assembled read stream from per-file atomic acquisition or a whole-set
snapshot, and maintain namespace/lifecycle bindings. Deserializing a receipt or
finding matching hashes cannot supply that live capability. Future guard support
requires an independently reviewed implementation; changing the uninhabited
reservation is not an admission mechanism in this slice.

Neither ordinary observations nor a future guarded capture alone authorize
quarantine, source deletion, permanent live-source equivalence or CHD conversion.
No permission token, consumer, transaction hook or operation entry point is added.
A destructive consumer would require a distinct, independently accepted authority
covering live writers, namespace identity through execution, preimage and recovery.
Existing CHD conversion restrictions remain independently applicable.

## Legacy meaning and limitations

Legacy `CueComparisonOutcome::Equivalent` compares stored evidence in memory. It
is mapped only to `MatchingBytesObserved`, never to preservation equivalence of
current originals. Legacy `Different` maps to `DifferentBytesObserved`, also a
historical fact. Legacy refusals map to `NotCompared` with explicit dimensions.
Mutation after collection may leave legacy matching records intact: the adapter
makes their unprotected nature explicit rather than claiming freshness detection.

The original intermittent restored-mtime regression remains intact and enabled.
This adapter does not repair its acquisition race, the failed sequential-reread
candidates, concurrent mmap writes, or the interval after any last observation.
There is no timeout, sleep, metadata version assumption, source copy or new reread
solution. Existing limits remain 256 KiB CUE text, 99 FILEs, 99 tracks, 8 GiB of
declared source bytes (including aliases), and the engine's 128 KiB read buffer.
Non-Unix collection remains unsupported. The adapter itself adds only bounded
record comparison work and no payload reads or image-sized allocation.

Direct tests exercise physical/layout distinctions, stored/synthetic pregaps,
positive changes, mixed refusals, stale records, restored mtime, file/symlink
replacement, multi-FILE hardlink aliases, missing/unreadable data, and real Linux
writable mappings with genuinely equal metadata. Compile-fail tests reject guard
construction, deserialization, fabricated comparison input and upgrading an
observation. Payload-independent tests compare pre-collected 8/64 MiB streams
and 99-member alias layouts after their paths disappear. Platform-specific
reproduction limits must be reported honestly; metadata is never fabricated.
