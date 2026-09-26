# GUI-v2 Conversion Queue and Space Preview

## Scope

This feature adds orchestration and preview only. It does not add a codec and
does not replace the converter-specific safety checks already in EmuWiz.

The first adapter is the existing verified CUE/BIN → CHD lane. It can already
build a `ChdConversionPlan`, stage output transactionally, fingerprint the
source and output, and publish only after verification. The queue carries
that plan's facts without reimplementing its eligibility rules.

The existing PSP ISO → CSO reversible-shrink workflow and verified ZIP workflow
remain available in their existing direct pages. They do not yet expose one
common readiness/execution contract, so the queue does not claim to schedule
them.

## Queue model

`archivefs_core::conversion_queue` provides:

- `ConversionQueue` with stable insertion order and cancellable pending items.
- `ConversionQueueItem` containing source/destination paths and formats,
  platform, backend, verification plan, provenance, readiness, state and
  refusal/warning text.
- `ConversionEstimate` with source size, destination estimate, temporary
  space, reclaimable space, free space, safety margin, filesystem relationship
  and atomic-publication duplication requirements.
- `SpaceEstimate::{Exact, Range, Unknown}` so unknown compression is never
  represented as a fabricated ratio.
- queue summary counts and aggregate ranges.

Preview planning performs metadata/statvfs reads only. It creates no output,
does not delete sources, and does not mutate queue-owned files.

## Space rules

The planner uses a 10% safety margin of source size. When destination and
temporary estimates are known, conservative required space is:

```text
temporary space + destination upper bound + atomic duplicate space + margin
```

Atomic duplicate space is included when source and destination are detected on
the same filesystem. Different filesystems are reported as not requiring that
same-filesystem atomic duplicate; unknown filesystem relationships remain
unknown rather than being guessed.

If output size is unknown, the item remains `Waiting` with an explicit reason.
The UI shows `Unknown`, not an invented compressed-size estimate.

## Compression and preservation

The CHD adapter reports the existing backend's known facts: CHD compression
policy, canonical optical fingerprint verification, preservation-equivalent
round-trip expectation, and “retain original”. Its space-saving estimate is
unknown until conversion, so the UI does not claim a ratio.

Queue execution is intentionally disabled in this first slice. The existing
single-item CHD executor remains the authority for staging, cancellation,
source freshness, verification and transactional publication. A future queue
executor must call that API rather than bypass it.

## Concurrency and cancellation

The product language promises one-at-a-time conversion for heavy optical work.
The queue currently plans and orders items only; it does not introduce unsafe
parallelism. Cancellation marks pending items `Cancelled`. Running and
verifying states are reserved for a future executor that can propagate the
backend's safe cancellation signal and retain incomplete temporary outputs as
incomplete rather than presenting them as complete.

## GUI-v2 surface

The existing Converter page now contains a Conversion Queue panel. After an
existing CUE/BIN conversion preview succeeds, the user can add it to the
queue. The panel shows:

- platform and source → target;
- ready, waiting or refused state;
- input, output and temporary-space estimates;
- free-space and likely-saved summaries;
- preservation/verification details under Advanced details;
- refusal or warning reasons;
- remove and cancel-pending controls.

The plain-language explanation is: “EmuWiz will convert these one at a time
and verify each result.” Advanced details remain available without making the
queue execution appear more capable than it is.

## Explicit non-goals

- no new conversion formats or codecs;
- no source deletion or quarantine from the queue;
- no invented output ratios;
- no generic execution wrapper around converters with incompatible safety
  contracts;
- no partially published output presented as complete;
- no replacement of existing converter readiness or verification checks.
