# LaserDisc media metadata and frame-range verification (V2)

V2 extends the existing bounded LaserDisc set verifier. It still treats a
Daphne/Hypseus set as a collection of a framefile, media and companion files;
it does not decode, transcode, hash, rewrite or execute any of them.

## Optional media metadata

When `ffprobe` is available, each referenced non-empty video is probed with a
small JSON summary. The probe records the container format, first video codec,
width, height, reported frame rate, duration, reported frame count, and audio
and video stream counts. Output is bounded and the subprocess is killed after a
short timeout. No packet dump, frame decode or seek operation is performed.

`ffprobe` is deliberately optional. If it is absent, fails, times out, or
returns malformed output, the set retains its existing structural readiness and
the report says that metadata is unavailable. A complete set is not downgraded
just because a workstation lacks ffprobe.

## Framefile ranges

For every referenced media name, V2 collects the first and last frame *start*
listed in the framefile. This is intentionally conservative: a framefile's
last mapping does not by itself prove an end frame. A range is therefore:

- `RangeValid` when a trustworthy reported frame count contains a single
  mapped start, or contains each bounded segment before the next mapping;
- `RangeExceedsMedia` when a mapped start is at or beyond that count;
- `RangeUnverified` when probing worked but no trustworthy frame count was
  reported;
- `MetadataUnavailable` when the media could not be probed; or
- `MalformedMapping` when the framefile contains malformed or unsafe mapping
  lines.

An observed range beyond the media is a broken set. Unverified or unavailable
metadata is a limitation, not proof that the set is broken. Multiple media
files are checked independently; V2 does not pretend that they form one
continuous video. Singe/Hypseus script interpretation remains shallow: only
media references already surfaced by the existing set verifier are considered.

The verifier remains read-only and preserves missing files, duplicate starts,
conflicting mappings and DAT/hash evidence as separate facts. Unusual codecs,
resolutions and stream layouts are warnings rather than universal rejection
rules because preservation variants are expected.
