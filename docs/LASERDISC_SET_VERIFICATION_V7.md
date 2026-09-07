# LaserDisc game-set verification (V7)

LaserDisc titles are verified as sets rather than as one image. The bounded
observer in `archivefs_core::laserdisc_set` looks for a concrete framefile,
parses its `start-frame media` mappings (the common Daphne/Hypseus form), and
checks that each referenced asset is a safe relative path to a readable,
non-empty file.

The report preserves mapping order/line provenance, duplicate or conflicting
frame starts, missing/empty media, and competing framefiles. It also records
ROM-like, Singe script, and configuration companions found directly in the
set root. Family detection is deliberately structural: Daphne requires a
framefile plus ROM component; Hypseus/Singe requires a Singe/script companion;
MAME requires an explicit `mame.ini`/`mame.cfg`. A directory name alone never
identifies an emulator family.

Readiness is `Ready`, `Partial`, `Broken`, or `Unknown`. Missing media,
malformed frame numbers, unsafe absolute/traversal references, and empty
assets are reported without repair or automatic winner selection. Video files
are checked with bounded filesystem metadata only; no full-video hash,
decode, seek validation, or transcoding is performed in V7. Duration,
resolution, codec, multi-video semantics, and emulator-specific script
validation remain follow-up work.

The verifier is read-only and does not download ROMs/videos, mutate user
files, execute scripts, or perform raw analogue LaserDisc RF capture/decoding.
DAT/hash identity for ROM components remains independent corroboration rather
than an inferred title.
