# Dreamcast DCP semantic reconciliation

Reviewed before implementation against authoritative main e306d7c293c179660800d62c23e3e6013e414861. Candidate commits: 566f8fcc, b43256c4. No wholesale cherry-pick.

| Candidate hunk / responsibility | Classification | Integration decision |
| --- | --- | --- |
| lib.rs export | still useful | Export the adapted backend. |
| DCP ZIP inspection, IP.BIN parsing, readiness types used by candidate | already on main | Reuse canonical modules; no second package or IP parser. |
| Representation enum and explicit image refusals | still useful | Extracted directory only; reject GDI/CHD/CDI/raw image input. |
| Request/preview types and identity requirements | needs adaptation | Immutable reviewed plan; exact package + complete source digest binding supplied by reviewed caller, plus mandatory product/revision/region and valid IP.BIN. Readiness labels are not execution authority. |
| check_identity / readiness flag acceptance | unsafe / discard | Caller-constructed Ready/PossiblyReady flags and unrelated image hashes cannot prove extracted-tree identity. |
| source_tree_hash / collect_files / safe_relative | needs adaptation | Bounded complete membership and streaming content hashes; include directories; reject symlinks/special files and path aliases. Shared transaction snapshots bind source freshness. |
| package_file_hash and package recheck (b43256c4) | needs adaptation | Retain revalidation intent; replace hex-encoded entire bytes with actual SHA-256 in canonical inspector; bound compressed and expanded reads. |
| package_member / replacement loop | needs adaptation | Reject duplicate/case-conflicting targets, noncanonical paths and special entries. Only replace reviewed existing members; opaque deltas and additions refused. Explicitly include IP.BIN, which the old filesystem_member filter skipped. |
| copy_tree / expected-member verification | needs adaptation | Copy into shared helper-owned staging; verify every expected member and directory, including unchanged content; no independent publication code. |
| validate_ip_bin | still useful | Reuse canonical inspection, exact identity and boot member checks before and after patching. |
| timestamp transaction ID, private staging path, rename publication, own receipt/state, recursive-delete rollback and cleanup | superseded / unsafe, discard | Exclusively TreePatchPlan, tree::prepare, tree::publish, tree::inspect, tree::undo. Failed staging is retained by the shared helper. |
| success, stale source, collision, image refusal, modified-output rollback tests | test-only | Adapt synthetic fixtures to shared recovery and extend package, stale membership, domain and immutability coverage. |

Size policy: supported source tree at most 512 MiB; compressed DCP at most 512 MiB; expanded package at most 512 MiB, individual entry at most 256 MiB. Existing-member replacement staging therefore needs at most 1 GiB. Each shared plan uses the larger of actual combined input bytes and exact planned output bytes (minimum one byte), never over 1 GiB. Logical sizes count sparse files. Shared 8 GiB ceiling remains unchanged.

GUI integration is deferred: no existing DCP apply caller exists, and provider source/package bindings and recovery ownership need an explicit route. This backend does not rebuild or launch disc images.
