# R2 practical safety contract (release-readiness target)

Owner decision, 2026-10-17: R2 is judged against this narrower contract. R2 is
an **optional, cooperative, crash-recoverable RenameMove transaction system**.
It is **not** an adversarial security boundary. R2 stays default-OFF. This
document does not approve production activation.

Earlier documents list exclusion of non-participating writers, an independent
anti-rollback witness, protected key custody and power-loss durability as
"production requirements". Under this contract those items are documented
**limitations**, not release prerequisites. Where an older R2 document disagrees,
this document governs release readiness. The engineering evidence in those
documents still stands.

## Scope

- Only the explicit `emuwiz-r2` operator executable (`r2-deployment.md`) can
  provision, enroll, approve or execute. Ordinary GUI and `emuwiz-cli`
  operations never enroll files, consult R2 or gain these guarantees.
- Each enrollment/execution needs `--enable-contract-1-i`. Provisioning needs
  `--accept-contract-1-i`. No persistent switch, environment variable, service,
  startup hook or GUI action exists.
- Supported envelope: one unresolved transaction per provisioned root. It covers 1–8
  fresh regular-file RenameMove entries, direct children of one canonical,
  private, owner-only local ext4 root with 4096-byte blocks. Enrollment passes
  actual syscall probes and needs 72 MiB free. Everything else refuses before
  admission or mutation.

## Guarantees kept

| # | Guarantee | Where it is enforced | Evidence |
| --- | --- | --- | --- |
| 1 | Reviewed plans and explicit signed authorisation | `plan`/`inspect-plan` digest; `review` digest bound to action, plan, issuer, recovery image and current observations; `approve` re-checks the review under admission, then signs (`deployment.rs` `approve`) | `deployment_tests.rs`, `crates/archivefs-cli/tests/r2_deployment.rs` |
| 2 | Exact filesystem identity and freshness | Plan captures (device, inode) and content identity. Execution re-verifies under admission. Grants bind an observations digest and the container image digest (`approval.rs` `check`/`consume`) | `enrolled_tests.rs`, `fd_contract_signed_tests.rs`, `restart_status_tests.rs` (`evidence_stale`) |
| 3 | No-clobber RenameMove | Existing executor/FD publication: link-without-overwrite into absent destinations; occupied destinations refuse | `r2-fd-publication.md`, `shared_engine_tests.rs`, `interrupted_apply_tests.rs` |
| 4 | Durable journaling and honest crash recovery | Preallocated compact container, commit-marker records, directory fsyncs. Short or torn records are retained and refused, never recreated. A complete but uncommitted record is finished only after a signed grant is consumed and the filesystem agrees with it | `enrolled_tests.rs`, `interrupted_apply_tests.rs`, `restart_status_tests.rs` |
| 5 | Fresh signed Undo authority | Undo needs its own reviewed, signed, one-shot grant. Spent grants never pass after restart. Changed evidence makes a grant `evidence_stale` | `restart_status_tests.rs`, `deployment_tests.rs` |
| 6 | Conservative refusal and retained evidence after interference | A substituted source is never published as the reviewed object (FD-pinned inode, private holding). Interference during Undo is detected and recorded as failure, and the uncertain step is not automatically reversed. Claims, anchors, holding and journals are retained. An enrolled transaction never falls back to the legacy executor/rollback: bare legacy Undo refuses before mutation | `fd_publication_probe_tests.rs`, `production_boundary_tests.rs`, `current_writer_tests.rs`, `shared_engine_tests.rs` |
| 7 | No silent automatic recovery of uncertain enrolled transactions | Automatic Apply resume is unsupported. Interrupted Apply recovers only through operator-signed Undo. Unrecorded link/unlink steps and contradictory evidence park as `review_required`. `status`/`inspect-grant` never write or consume | `interrupted_apply_tests.rs`, `restart_status_tests.rs` |

## Explicit limitations (not promised)

1. **Non-cooperating writers are not excluded.** Older builds, ordinary
   EmuWiz writers, other programs and shells running as the same user can
   rename, replace or write enrolled files. R2 detects this afterwards and
   refuses or retains evidence. It does not prevent it. **Not every foreign
   file stays unmoved:** if a writer substitutes a file in the Undo race
   window, R2 may move that foreign file before detecting the interference.
   It then reports `RollbackFailed`, retains ownership and evidence, and does
   not automatically reverse the uncertain step
   (`current_legacy_process_bypasses_enrolled_ownership_during_undo`).
2. **A deliberately restored authority snapshot is not detected.** The
   controlled-deployment tests demonstrated that restoring only `state/grants/`
   (including by file copy) can reactivate a consumed Apply grant that made no
   progress. They also demonstrated re-authorisation of a completed and undone
   Apply after consistently restoring authority and recovery state with holding
   anchors recreated as hardlinks to the live original inodes. The tested
   authority-only partial restore and a recovery restore with copied anchors
   (new inodes) refused as stale/inconsistent evidence. This is not general
   rollback detection. Do not restore authority/grant/tombstone/recovery storage
   over live state. Evidence: the controlled-deployment ledger's independent
   review, conclusion 6 (`repro-r1-copyrestore.json`, `scenarios-rerun-independent.log`,
   `repro-c5-r2c.json`); the earlier partial-rewind test remains valid
   (`older_authentic_snapshots_are_caught_where_evidence_allows_and_limits_are_explicit`).
3. **No protection from root or compromised privileged accounts**, or from
   hostile code running as the operator's own UID. Mode 0700/0600 is a
   cooperative hygiene check, not isolation.
4. **Writes through already-open writable descriptors or mappings are not
   prevented.** Such a write can change an object's contents after
   verification. Undo then records failure and retains ownership.
5. **Durability is bounded by the filesystem's fsync guarantees.** Process
   death, short writes and injected EIO/ENOSPC are tested. Hardware power loss
   and device cache behaviour are not claimed.

Also accepted and documented in `r2-deployment.md`: grants do not expire by
time; configuration/key rotation and transaction recycling are unsupported;
capacity exhaustion fails closed and needs operator review. Parked interrupted
Undo can leave other entries applied with ownership held. After reviewing the
current evidence, a fresh signed Undo can recover other eligible entries without
changing parked objects. A proven foreign-held entry can be settled only through
the distinct signed retention procedure below and in `r2-deployment.md`; ordinary
Undo cannot authorise it. Unrecorded link/unlink ambiguity, missing originals or
contradictory evidence still require review and preservation, not repeated grants
or manual filesystem reconciliation. Release requires every original restored
and every retained foreign object and signed receipt verified.

No GUI or CLI text may contradict these limitations. The GUI has no R2 surface.
The `emuwiz-r2` help and messages make no exclusion, tamper-proofing or
power-loss claim.

## Operational assumptions for controlled deployment testing

- **Signing-key custody.** `issuer-init` creates an Ed25519 PKCS#8 key
  (`ring`, OS RNG) as 0600 in a new 0700 directory, and never overwrites one.
  `approve` refuses a signer inside the collection root or state directory and
  refuses a key that differs from the configured issuer. `execute` never loads
  a key. The operator keeps the signer directory away from ordinary application
  use. Under this contract, that is enough.
- **Recovery authority.** Configuration, registration, admission database,
  compact container, anchors, holding, grants and tombstones are private
  trusted storage on the collection's filesystem. Back them up for evidence,
  but never restore them over live state (limitation 2). Never delete evidence
  to retry, and never create a second authority.
- **Service lifecycle.** There is no daemon. Each command is a short-lived
  process. After any interruption, run `status`/`inspect-grant` (read-only),
  then follow the single permitted next step. `busy` means a live participant
  holds admission.
- **Capacity.** Each container reserves `12 * entries + 32` slots within a
  64 MiB cap. Exhaustion retains the current state and claims and refuses
  further progress. Each registration is single-use, with no automatic reset. A new transaction on the same root requires explicit verified post-Undo succession; see `r2-deployment.md`.
- **Supported deployment.** Disposable or explicitly approved local ext4/4096
  roots only, and no reflink, network, FUSE or shared roots. During a test
  transaction the operator keeps other EmuWiz writers and tools away from the
  enrolled root (cooperation is the operator's job; see limitation 1).

## Identifier labels

`emuwiz-test-grant-v1` and `emuwiz-test-boundary-v1` are historical
serialization format labels, checked for equality on the execution and
inspection paths (`approval.rs`). They are **not** test-only security
provisioning. No production code path contains fixed keys, seeds or a
verification bypass. Keys come from `Ed25519KeyPair::generate_pkcs8` with
`SystemRandom`. Nonces are 32 bytes from `SystemRandom`. Signatures are
verified against the configured public key and directory identity. Renaming
the labels would break existing grants and is not required.

## Verdict basis

The guarantees above are implemented and tested on main. The limitations are
accepted owner decisions. The 2026-10-17 restart-continuity review concerns
(C2 FIFO non-blocking open, C4 inspect boundary-format parity, C6 hot-journal
reason) are closed on main in `e77d8d9e`. The 2026-10-16 seal-window defect is
fixed in `12942e07`. Validation results and the independent review for this
contract are in the audit ledger under `2026-10-17-r2-practical-contract/`.

## Owner-approved reviewed foreign retention

A distinct one-shot signed `retain-foreign` disposition may permanently retain
an identified foreign file parked during Undo, restore its proven original,
and record an identity-bound receipt in the existing compact journal. Ordinary
Apply/Undo grants cannot authorise it. Foreign holding and original continuity
anchors are preserved; no automatic return, disposal or garbage collection is
provided. A subsequent fresh ordinary signed Undo may release ownership only
after every original and every retained signed receipt verifies. Unresolved,
changed, missing or contradictory evidence remains REVIEW_REQUIRED. Retained
objects remain discoverable with `emuwiz-r2 retained STATE` after release. This
adds no power-loss, hostile-private-storage or authority-rollback guarantee;
all existing root eligibility, explicit enrollment and single-use registration limits apply. Explicit root succession never discards an actionable Undo or unresolved recovery.
