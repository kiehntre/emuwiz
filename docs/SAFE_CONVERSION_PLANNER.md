# Safe conversion planner

EmuWiz separates storage conversion from content modification. A **lossless
recompress** changes the container representation while preserving the source
content. A **scrub/trim** or other **content-modified** operation may discard
partitions, files, or data and must never receive the lossless UI label.

The planner is read-only. It produces structured argv, collision and free-space
preflight results, a source-preservation contract, and the verification steps
required before an operation can be reported as trusted. Creating a plan does
not start a tool.

## WIT safety

WIT's default `copy` mode can scrub data. It is therefore classified as
`ContentModified`. Preservation-safe ISO → WIA planning must explicitly use
`wit copy --raw input.iso output.wia`. Partition selection such as
`--psel -update` or `--psel data` is also `ContentModified`.

The planner uses structured argv tokens rather than a shell command string, and
refuses an existing destination or in-place source/destination request.

## Other routes

- GameCube/Wii RVZ is classified as `LosslessRecompress` when the verified
  backend is available; this task does not invent a `rom-converto` command when
  the executable cannot be discovered.
- PSP ISO → CSO reuses the existing EmuWiz byte-exact restore contract.
- CHD planning requires explicit media topology. CD routes use semantic optical
  equivalence; DVD routes can require byte-exact ISO restoration.
- Content stripping, language deletion, update removal, and partition scrubs
  are not executed by this planner.

## Receipts and proof

Receipts retain source/output paths and sizes, source/output/restored hashes,
backend and tool version, structured argv, operation class, timestamp, savings,
content-change state, verification level, and source-untouched state. A receipt
cannot become `ByteExactRestoreVerified` unless the restored hash matches the
recorded source hash. Removal savings and later compression savings remain
separate.
