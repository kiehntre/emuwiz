# Limited R2 operator deployment (default OFF)

The release-readiness contract and its explicit limitations are in
`r2-practical-safety-contract.md`.

This workflow adds an explicit `emuwiz-r2` operator executable to the accepted
Contract 1-I controller. Ordinary GUI and `emuwiz-cli` operations are unchanged;
they do not silently enroll files or gain the new guarantees. The tool has no
persistent enable flag, environment-variable activation, service or startup hook.
Every enrollment/execution requires `--enable-contract-1-i`. Provisioning alone
never enables or enrolls anything. Promotion is not permission to provision a
real root or use real signing credentials.

## Preconditions

Obtain owner approval of Contract 1-I and the specific deployment first. Prepare
an already-private, canonical, owner-only local ext4 collection root with 4096-byte
blocks. No symlink components, nested moves, directories, mixed operations or
legacy-journal migration. Only one NEW transaction per root, one to eight distinct
regular-file objects directly beneath that root. Existing recovery/holding data
prevents reprovisioning. A completed transaction does not make this root reusable:
no automatic reset, deletion or capacity recycling is provided.

Prepare a private parent for a NEW state directory on that same filesystem, outside
the collection, and a separate NEW operator signer directory. The tool creates
its own new directories/files as 0700/0600 and never fixes existing permissions.
All path arguments for private documents are absolute. Configuration/registration,
keys and grants must remain in trusted private storage. Backups must preserve
recovery evidence, but restoring a whole old authority store is NOT detected by
this protocol. Do not roll back authority/grant/tombstone storage.

Provisioning performs actual filesystem capability probes. Enrollment repeats
these checks and requires 72 MiB available for the existing bounded container and
configuration allowance; actual reservation still uses the existing allocation
checks. This is not a guarantee that space remains available or that salvage can
succeed. Probe evidence remains in private holding. Any partial provisioning,
enrollment, missing registration or inconsistent evidence requires review; never
retry by deleting the evidence or creating a second authority.

Signing material belongs to the operator process; execution receives only the
public verifier and a signed one-shot grant. Protect the signer directory from
ordinary application access operationally. This single-user implementation does
not isolate it from hostile same-UID code, and directory modes do not provide
that guarantee. No new custody/security boundary is claimed. The independently
registered immutable original plan and store ticket are trust configuration;
transaction progress remains exclusively in the existing compact journal.

## Explicit workflow

Build the reviewed tree with the existing isolated Cargo workflow:
`scripts/cargo-iso --low-debug --incremental build --locked -p archivefs-cli --bin emuwiz-r2`.
Use the resulting binary (abbreviated `emuwiz-r2` below). These are operator
instructions, not commands the agent has run against real user data.

1. `emuwiz-r2 issuer-init /private/operator/signer`
   creates a new Ed25519 PKCS8 key and `public.json`, never overwriting a key.
2. `emuwiz-r2 provision /private/games /private/state /private/operator/signer/public.json OPERATOR_NAME --accept-contract-1-i`
   binds the root to exactly one authority/public key. It remains OFF.
3. `emuwiz-r2 plan /private/state /private/plan.json old.bin new.bin`
   captures the exact reviewed identities. More source/destination pairs may be
   supplied within the eight-object limit. Inspect all paths, identities and
   bytes as appropriate. `inspect-plan /private/plan.json` prints the exact plan
   and its confirmation digest without enrollment.
4. `emuwiz-r2 enroll /private/state /private/plan.json --confirm PLAN_DIGEST --enable-contract-1-i`
   creates admission, existing compact authority and continuity anchors, then
   stores the immutable registration. It does NOT Apply. An interrupted setup
   with no complete registration refuses instead of reconstructing authority.
5. `emuwiz-r2 review /private/state apply`
   shows current authoritative state, holding inventory, checkpoint uncertainty,
   operator and a digest bound to the action, original plan, issuer, exact recovery
   image and current file observations. Review this output before signing.
   This is a participating command: it may recover the admission database
   (including rolling back a hot SQLite journal). It is not a read-only
   substitute for `status` or `inspect-grant`.
6. `emuwiz-r2 approve /private/state /private/operator/signer apply --confirm REVIEW_DIGEST /private/apply-grant.json`
   rechecks that review under admission, checks the configured key, then issues
   the EXISTING signed protocol's durable one-shot grant. It does not execute.
7. `emuwiz-r2 execute /private/state apply /private/apply-grant.json --enable-contract-1-i`
   reopens through signed verification/consumption and the reviewed controller.
   It never loads a private key. A stale or spent grant cannot be retried; inspect
   current evidence and obtain a new reviewed grant. Do not automatically loop.
8. For Undo, repeat review, approve and execute with action `undo` and a NEW grant
   file. Inspect the JSON `completed` field and exit status: partial/failed Undo
   exits 2, not success. Missing/malformed/ambiguous evidence refuses. Automatic
   interrupted Apply resume remains unsupported. An Apply that died or failed
   part-way is recovered by this same signed Undo: never-mutated entries are
   recorded `Skipped` and their anchors retired, held originals are returned
   through trusted holding, and a recorded publication has its recorded
   cleanup finished before it is reversed (see "Interrupted Apply recovery" in
   `r2-contract1i.md`). A link or unlink that happened without its record,
   and any foreign, missing or contradictory evidence, parks the entry for
   review.

The v1 signature protocol retains its historical `emuwiz-test-*` wire labels for
compatibility; labels confer no authority. Trusted configured keys, independently
registered original bytes/ticket, exact observations and durable consumption do.
Never feed a recovery journal to provisioning as replacement trusted registration.
Configuration/key rotation and transaction recycling are intentionally unsupported.

## Restart: read-only status and grant inspection

After any controller or service termination, start with the read-only commands.
Neither ever starts recovery, consumes, renews or recreates a grant, repairs a
checkpoint or writes an authority record:

- `emuwiz-r2 status STATE` interprets the transaction from trusted evidence and
  prints one state with its reasons and the only permitted next step. Exit 0 for
  a recognised state, 2 for `review_required` or `busy`.
- `emuwiz-r2 inspect-grant STATE apply|undo GRANT_JSON` reports whether a grant
  would pass the signed checks today, without burning it.

| State | Meaning | Next step |
| --- | --- | --- |
| `not_enrolled` | Provisioned; nothing enrolled, no partial enrollment | Enroll a reviewed plan |
| `awaiting_apply` | Enrolled; no checkpoint published | Review and approve Apply |
| `apply_incomplete` | Apply began and stopped; Apply is spent | Review and approve Undo (recovery re-verifies per entry and may park entries) |
| `applied` | Every entry applied and verified against the files | Review and approve Undo |
| `undo_incomplete` | Undo began and stopped | Review and approve a fresh Undo |
| `settled_awaiting_release` | Verified settlement recorded, ownership still held | A fresh Undo performs only the verified release |
| `settled` | Verified settlement recorded and ownership released | None |
| `review_required` | Missing, corrupt, rebound, stale or contradictory evidence; incomplete enrollment; Apply or Undo parked for review; or unreadable admission authority after interruption | Follow the reason-specific procedure below; retain evidence and ownership. A fresh signed Undo does not clear a parked entry |
| `busy` | A live participating process holds admission or the store | Retry when it ends |

Status reads the configuration and registration, takes the participants' admission
lock without writing the database, opens the compact store with a shared lock,
and accepts a journal claim only when the files agree with it: a record that says
nothing moved, while the files say otherwise, is refused as stale or rewound
evidence rather than believed. Grant verdicts are `usable`, `consumed`,
`evidence_stale` (files, transaction or authority evidence changed after issue),
`wrong_action`, `foreign_or_invalid` and `not_applicable`. Execution still burns a
grant before acting and re-verifies everything under admission; `usable` is not
an authorisation by itself.

**Grant lifetime limitation (accepted under `r2-practical-safety-contract.md`).** Grants are
one-shot and evidence-bound, not time-limited. Time-based expiry is NOT supported
and is not claimed. An unused grant over unchanged evidence stays usable
indefinitely, across any number of restarts; treat unused grant files like keys
and keep them only as long as needed. Nothing deletes them automatically. A
consumed grant is never reusable after restart, and evidence changes make an
unused one `evidence_stale` while current authority remains intact. Deliberately
restoring older local authority can defeat consumption: the controlled-deployment
tests demonstrated re-use after restoring only `state/grants/` for a consumed
Apply that made no progress. A consistent restore of authority and recovery state
also re-authorised an Apply already completed and undone when holding anchors
were recreated as hardlinks to the live original inodes. An authority-only
partial restore, or a recovery restore using copied anchors with new inodes,
refused as stale/inconsistent evidence in the tested cases. These refusals do not
provide general rollback protection. Never restore grants, tombstones, admission
or recovery authority over live state; retain backups as evidence only. See
limitation 2 of `r2-practical-safety-contract.md`.

## Interrupted Undo: reason-specific operator procedure

Keep other writers away from the enrolled root. Start with `status STATE` and,
if relevant, `inspect-grant STATE undo GRANT_JSON`; preserve their output and the
original plan, registration, admission database and journal, compact journal,
grants/tombstones, anchors and holding. Do not edit or delete evidence to retry,
reset ownership, create another authority, or use legacy Undo on enrolled files.
A `review_required` reason must be classified before another execute attempt.

### Admission database hot journal

The tested case is interruption during admission release after file restoration:
`status` exits 2 with "admission authority unreadable" and says that only the
participating path may recover it. Read-only status leaves
`STATE/admission/owners.sqlite-journal` untouched. A journal's presence alone is
not permission to delete it or proof of this recoverable case.

1. Preserve the existing evidence, confirm no participant is live (`busy` means
   wait), and run `emuwiz-r2 review STATE undo`. This participating review can
   roll back the hot admission journal; it does not execute file Undo or sign a
   grant. Do not perform an external SQLite repair or remove the journal.
2. Run `emuwiz-r2 status STATE` again. Continue this release-only procedure only
   if the result is `settled_awaiting_release` and the observed files agree with
   the original plan. If review fails, the reason changes, or an entry is parked,
   stop for operator review instead of assuming settlement.
3. Inspect the review's Undo action and current evidence. Using its confirmation
   digest, issue a NEW grant at a new private absolute filename:
   `emuwiz-r2 approve STATE SIGNER_DIR undo --confirm REVIEW_DIGEST NEW_GRANT_JSON`.
   Inspect it with `emuwiz-r2 inspect-grant STATE undo NEW_GRANT_JSON`; proceed
   only with `usable` for the intended action and unchanged evidence.
4. `emuwiz-r2 execute STATE undo NEW_GRANT_JSON --enable-contract-1-i`
   performs only the verified ownership release in this settled state. Require
   exit 0, `completed: true`, and final `status STATE` = `settled` with ownership
   `released`. Never reuse a consumed grant or claim success from signing alone.

The controlled-deployment campaign recovered all seven hot-journal cases this
way; independent reproductions confirmed the release-only transition. This is
not a repair procedure for arbitrary admission corruption.

### Parked Undo and files that remain applied

Interrupted Undo can park as `Undo/DestinationOccupied` or
`Undo/PossiblyUnrecoverable`, as well as foreign/missing/contradictory evidence.
Inspect every plan entry separately, including the current source and destination
names, holding and anchor locations. Compare device/inode and recorded freshness
and content identity with the trusted original plan; matching names or bytes
alone do not establish the original object. Retain any foreign object unchanged.

In every reproduced `Undo/PossiblyUnrecoverable` cleanup-window case, the original
inode was already back at its source name: the kill occurred after anchor/holding
unlink but before the completion record. Check that source-name identity before
treating the warning as lost content. This observation is not proof for a new
case. In reproduced `Undo/DestinationOccupied` cases the original was at its
source name and still linked from holding and its anchor; do not remove those
links to make the state look settled.

A parked entry stops Undo for the transaction. Another entry can still be at its
applied destination with ownership held even though the parked entry's original
is already restored. Inventory all entries and retain the whole transaction;
restoring one file's name does not mean Undo completed. The tested fresh Undo
refused to continue a parked transaction (and consumed the newly issued grant);
`approve` can sign a non-actionable grant, so signing success is not recovery.
Repeated refused execute attempts also accumulate `probe-*` evidence in holding.

**Operational blocker:** there is no supported CLI procedure demonstrated to
clear the park, complete the remaining entries and reconcile/release ownership.
The verified operator action is to stop, retain all objects and authority, record
which original inodes are restored, held or still applied, and escalate for a
case-specific reviewed recovery procedure. The campaign did not validate manual
rename/unlink, journal editing or ownership-reset commands; none is prescribed
here. Such a case remains unresolved until a separate safe procedure is approved
and verified. This limitation must be accepted for an optional deployment.

Evidence: `2026-10-17-r2-controlled-deployment/RESULT.md` (F2–F4),
`results/hotjournal-recovery.json`, and the independent review's
`independent/VERDICT.md` conclusions 3–6/L3 with
`logs/repro-park-hotjournal.json`, `logs/repro-r1-copyrestore.json` and
`logs/repro-c5-r2c.json` in the existing audit ledger. No full campaign rerun is
needed to establish these documented outcomes.

## Recovery and limits

Success is only the actual nested completed outcome. Foreign objects are never
automatically returned or deleted, but a file substituted during the Undo race
window may be moved before the interference is detected; Undo then reports
failure and retains evidence (see `r2-practical-safety-contract.md`). Salvaged bytes are a new inode
and remain incomplete. Partial copies, lost-last-link uncertainty, pending or
contradictory records require operator review. Keep all unresolved journals,
registration, admission claims, grants/tombstones and holding entries. The existing
History/recovery projections remain authoritative; this adds no second event log.
After verified Undo release, the execute receipt reports completion; attempting to
reuse the released registration refuses. Existing read-only journal/History readers
can still inspect the terminal transaction.

This is the accepted bounded baseline-equivalence contract, not exclusion of other
applications. No claim covers power loss, hostile private-directory modification,
full authority-store rollback, existing writable-FD content changes or external
changes after completion. Reflink filesystems remain ineligible. Hardlink anchors
retain existing allocation, not full extra copies; salvage needs extra storage.
The separate GUI History/Holding/Clean Logs backlog remains separate.
