# Media topology and launch planning

This integration is a read-only projection from the existing `MediaSet` and
`MediaSwapPlan` models into the existing launch planner. The topology engine
continues to own grouping, ordinals, sides, roles, completeness, conflicts,
and representation evidence. The launch adapter does not inspect files,
scan the catalogue, contact the network, or write state.

## Boundary

`project_media_set_for_launch` consumes an already-resolved `MediaSet` and
optionally the already-selected `MediaProfile`. It returns the declarative
swap plan, ordered media, the start member (when proven), typed missing-media
requirements, conflicts, an explanation, and the shared evidence
`ActionSafety` value. `build_launch_plan_with_media_set` attaches this result
to the existing `LaunchPlan` and adds a typed fail-closed launch blocker when
the topology is blocked or still requires review.

Plans with no topology input remain unchanged (`media_topology: None`). The
existing canonical identity, launch-input projection, emulator profile,
command planning, and firmware/readiness primitives remain authoritative.

## Start media and states

The adapter selects a start member only when the topology plan supplies an
explicit ordinal for every ordered step, the first step has the minimum
ordinal, and its role is boot, game, or play media. It never chooses by
filename, directory order, or lexical order. Unknown start media is
`REVIEW_REQUIRED`.

`COMPLETE_SET` can be `SAFE_TO_ACT` when the start member and representation
are resolved. Incomplete, ambiguous, conflicting, and unsupported sets are
`BLOCKED`; unverified sets are `REVIEW_REQUIRED`. Missing members retain the
expected ordinal/unit in a typed projection. Competing candidates and
representation conflicts remain blockers; alternatives are not treated as
additional media.

Floppy side semantics and tape load order remain in the existing
`MediaSwapStep`/`MediaTransition` values. Optical, floppy, and tape are not
flattened into one generic disc sequence.

## Readiness separation

Topology safety is separate from emulator and BIOS readiness. A complete,
verified media set can still have a launch candidate blocked by a missing BIOS
or an unavailable emulator profile. Conversely, a ready emulator cannot make
an incomplete or conflicting media set launch-safe.

The current command planners receive only the proven start content through
their existing inputs. This task does not execute swaps, create playlists,
write emulator configuration, or launch a process.

## Future boundary

A future emulator adapter may consume the declarative `MediaSwapPlan` to
present explicit actions such as change disc, flip side, or load next tape.
That adapter must revalidate the current plan and remain separate from this
projection; automatic hotkeys, playlists, scripts, and media execution are
not part of this integration.

## Real collection smoke

The current catalogue was inspected read-only (`102,343` rows; SQLite
`quick_check` was `ok`). It contains media-like formats including CHD, DSK,
IPF, TAP, and TZX. Persisted topology evidence is not available for every
catalogue row, so this projection honestly remains absent or unverified when
the caller has not supplied a resolved `MediaSet`; it does not manufacture a
real complete multi-media launch set from filenames alone.

## Performance and side effects

Projection is linear in the supplied set and swap steps. It performs no
filesystem or network work. Large-catalogue page assembly remains the
responsibility of callers that already provide bounded topology results; this
adapter does not scan the whole catalogue.
