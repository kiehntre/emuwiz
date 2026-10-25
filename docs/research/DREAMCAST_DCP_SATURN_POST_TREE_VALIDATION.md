# Dreamcast DCP and Saturn component backend validation

Branch: `integration/dreamcast-dcp-saturn-patch-post-tree`.
Starting authoritative main: `e306d7c293c179660800d62c23e3e6013e414861`; HEAD/main/origin/main matched and tracked files were clean before work began. Main advanced independently through four CLI-only commits to `1a24276a1270114193490201eb4e542f7ef93f08`. Those commits were inspected; core source, workspace manifest and lockfile are unchanged. The candidate was rebased cleanly onto that verified main after testing. The post-rebase workspace check passed; a Git comparison proves the tested core source, workspace manifest and lockfile are unchanged across the rebase.

The two candidate inventories were committed before implementation. Dreamcast salvages the intent of 566f8fcc/b43256c4; Saturn salvages ffcbd909/1f6ce22d. Neither branch was cherry-picked wholesale. All private transaction IDs, staging/rename publication, alternate receipts, recursive-delete cleanup and rollback were discarded. The old Saturn EOF grow-only edit was not ported; standalone_patch.rs is unchanged.

Both adapters return `patch_output_recovery::tree::PreparedTreePatch`. Their private plans call `TreePatchPlan::review_with_max_total_bytes` and `tree::prepare`; callers publish, inspect and undo using `tree::{publish,inspect,undo}`. No alternative receipt, journal, publisher or rollback model was added. Shared tree code is unchanged.

The cross-backend test exercises exactly this contract for both adapters. Backend tests cover retained preparation/failure, staged/published inspection (restart boundaries), undo, repeated undo refusal, republish, corrupt output refusal and no-clobber collisions. Stale dependency tests assert the producer callback is never entered. All fixtures are synthetic temporary trees; no real collection was patched.

## Supported scope

| | Dreamcast DCP | Saturn |
| --- | --- | --- |
| Source | Reviewed extracted tree with bootsector/IP.BIN and declared boot member | Complete dedicated BINARY CUE/component tree; one reviewed data component/track, separate audio components |
| Operations | Existing-file replacement and same-length IP.BIN replacement; metadata ignored; stored/deflated unencrypted ZIP | Canonical IPS, BPS, UPS and PPF3 application; same component length; MODE1/2048, MODE1/2352, MODE2/2352 |
| Authority | Explicit reviewed package SHA-256 + complete source-tree SHA-256 binding; exact product/revision/region and recognized hardware/GD-ROM evidence | Reviewed exact manifest, component/patch hashes, component/track mapping and native disc ordinal |
| Verification | Every expected member/directory and unchanged content, exact replacement hashes, IP.BIN identity/length and boot mapping | Exact CUE, all unchanged members/audio, all track/component/layout facts and native System ID; raw sync/address/mode/XA headers unchanged |
| Size | Source <=512 MiB, compressed/expanded DCP <=512 MiB each, member <=256 MiB; staging <=1 GiB | Source/staging <=1 GiB, target <=512 MiB, patch <=128 MiB |
| Per-plan shared allowance | max(actual combined input bytes, exact output bytes), <=1 GiB | actual combined input bytes, <=1152 MiB |
| Refusals | Direct GDI/CHD/CDI/raw images, arbitrary image rebuild, new/missing targets, opaque deltas, traversal/absolute/alias paths, duplicate/special entries | SSP, CHD, lone BIN, xdelta/unsupported formats, logical-track/filesystem targets, audio/shared-track targets, resizing/rebuilds, unrepresented CUE directives |

Size accounting uses checked arithmetic and logical lengths, including sparse files. Neither adapter requests the shared 8 GiB ceiling blindly. Structural verification is not a claim that arbitrary patched game code will boot successfully. DCP has no embedded source hash; its package/source relationship requires an explicitly reviewed caller binding. No GUI/provider binding route or emulator launch integration was added.

## Validation

Isolated target: `/tmp/emuwiz-dcp-saturn-post-tree-target`; two Cargo build jobs and two test threads. Cargo commands use `--offline --locked`. Full core runs used local mock-server access outside the network-restricted sandbox.

- Final focused run: **269 passed, 0 failed**. This includes 11 DCP backend tests, 18 Dreamcast boot/IP.BIN tests, 11 Saturn backend tests, 18 canonical CUE parser tests, 36 optical preservation tests, 32 standalone patch tests, 25 patch-output recovery tests (20 shared tree, 5 file-output), and one cross-backend test, plus related platform/readiness tests.
- Pre-final full core: **10,128 passed, 0 failed, 3 ignored**.
- Final full core: **10,131 passed, 0 failed, 3 ignored** (three additional Saturn preservation tests; 23 new tests overall).
- Offline locked workspace check: **passed before and after rebase**.
- Full formatting, complete diff whitespace and scope checks: **passed**.
- GUI tests: not applicable; GUI files are unchanged.

Five existing GUI warnings remain: unused LaunchWarningKind import in launch_readiness_summary.rs, unused show_with_playing_library_plan in mame_collection_health.rs, unused dat_health_label in native_workflows.rs, unused Informational variant in problems.rs, and unused portability_label in saves_states.rs. No new backend warnings were reported. No prohibited file or active-work collision occurred.

Main was not modified by this task. Nothing was pushed or promoted.

Validated core tree: `075bf0a71d45fd7233945e061c2bee17a4ee47bf` (identical before and after rebase).

Reproduction commands (from this worktree):

```sh
export CARGO_TARGET_DIR=/tmp/emuwiz-dcp-saturn-post-tree-target
export CARGO_BUILD_JOBS=2
cargo test --offline --locked -p archivefs-core --lib -- dreamcast saturn cue_bin optical_preservation standalone_patch patch_output_recovery optical_patch_tree --test-threads=2
cargo test --offline --locked -p archivefs-core --lib -- --test-threads=2
cargo check --offline --locked --workspace
cargo fmt --all -- --check
git diff --check
git diff 1a24276a..HEAD --check
```

Safe for focused review: **YES**. GUI wiring and an explicit provider/user review route for source-package bindings remain deferred. Unsupported disc representations and rebuilds remain refusals, not implied capabilities. No launch integration was added.

## Final focused review changes

- **Dreamcast source bound 512 MiB -> 1 GiB.** `MAX_SOURCE_BYTES` bounds the
  logical size of the COMPLETE reviewed extracted tree. A GD-ROM high-density
  area is LBA 45000..=549149, 504,150 x 2048 B = 984.7 MiB, so 512 MiB refused
  most discs that fill more than half the area. `MAX_STAGING_BYTES` becomes
  1.5 GiB (source + 512 MiB package), compile-time checked against the shared
  helper's 8 GiB ceiling. A 900 MiB sparse tree passed review, prepare, publish,
  inspect and undo at 46 MB peak RSS; hashing is the cost (about 9 minutes in a
  debug build, because the tree is hashed several times).
- **Saturn bounds unchanged.** Source/staging 1 GiB, patch 128 MiB, allowance
  <= 1152 MiB are passed explicitly to the shared helper. The 512 MiB component
  cap equals the canonical engine's in-memory `MAX_APPLY_BYTES`; a Saturn data
  track above 512 MiB (a full raw 2352-byte disc can reach about 750 MiB) is
  refused. That limit belongs to the canonical engine, not this adapter, and
  would need a streaming applier to lift.
- **Shared helper journal lock.** `tree::load` used a single non-blocking
  `flock`. A fork in another thread briefly holds an inherited copy of the
  descriptor until its exec, so a lock the caller had just released reported
  `WouldBlock`. In the full suite this failed the Saturn recovery tests in 3 of
  4 runs; module-only runs never failed. `load` now retries for up to 2 s; a
  real concurrent publish/undo still refuses. 4 of 4 full runs pass after.
- **Added coverage:** a backend-independent contract run by both adapters
  (published-tree mutations never gain undo authority, post-rename recovery,
  repeated undo, collision, stale plans, input immutability), IP.BIN
  boot-mapping/product/revision/region changes, and IPS growth past EOF.
