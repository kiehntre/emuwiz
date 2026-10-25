# Limited R2 operator deployment (default OFF)

The release-readiness contract and its explicit limitations are in
`r2-practical-safety-contract.md`.

This workflow adds an explicit `emuwiz-r2` operator executable to the accepted
Contract 1-I controller, also reached by explicit Blue Advanced Protection actions
through the bundled `emuwiz-cli --r2-operator` child. Standard Rename and ordinary
`emuwiz-cli` operations are unchanged;
they do not silently enroll files or gain the new guarantees. The tool has no
persistent enable flag, environment-variable activation, service or startup hook.
Every enrollment/execution requires `--enable-contract-1-i`. Provisioning alone
never enables or enrolls anything. Promotion is not permission to provision a
real root or use real signing credentials.

## Preconditions

Obtain owner approval of Contract 1-I and the specific deployment first. Prepare
an already-private, canonical, owner-only local ext4 collection root with 4096-byte
blocks. No symlink components, nested moves, directories, mixed operations or
legacy-journal migration. Only one unresolved transaction per root, one to eight distinct
regular-file objects directly beneath that root. Existing recovery/holding data
prevents reprovisioning. A completed Apply alone does not permit reuse. After
verified Undo, complete retained-file settlement and ownership release, the
explicit successor procedure below permits another separately enrolled transaction.
No automatic reset, deletion or capacity recycling is provided.

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
succeed. Successful probes clean up only their own newly created, identity-verified files and directory. Changed, unknown or failed probe evidence is retained; pre-existing probes are never adopted for cleanup. Any partial provisioning,
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

A newly parked entry stops that Undo attempt. After reviewing the complete
current transaction, a fresh ordinary signed Undo can recover other eligible
entries while leaving all already-parked objects and receipts untouched. It
cannot settle a foreign holding file. A transaction stays `review_required`
with ownership held while any entry remains unresolved. If no ordinary Undo
can make progress, approval refuses before issuing a grant.

### Explicit signed foreign-file retention

Only a recorded **Undo/ForeignHeld** (or interrupted **RetentionPending**)
entry with its exact foreign object intact, a verified original continuity
anchor, an absent old destination and salvage slot, and a free original name
(or that same anchored original already there) is eligible. Changed, missing,
symlinked, occupied or contradictory evidence refuses. Other ambiguous parks
remain REVIEW_REQUIRED; no name guessing, legacy recovery or manual unlink is
prescribed.

This is a distinct operator disposition, **not ordinary Undo**:

1. `emuwiz-r2 review STATE retain-foreign` displays the identified foreign
   files, their holding locations and content/identity evidence, eligible entry
   indices, and the explicit permanent-retention effect. Require `actionable:
   true` and review the entire output. No file restoration happens here.
2. `emuwiz-r2 approve STATE SIGNER_DIR retain-foreign --confirm REVIEW_DIGEST NEW_PRIVATE_GRANT_JSON`
   issues the existing one-shot signed protocol with a distinct signed action.
   Ordinary Apply/Undo grants cannot authorise this disposition.
3. Require `usable` from `inspect-grant STATE retain-foreign NEW_PRIVATE_GRANT_JSON`.
   Then `execute STATE retain-foreign NEW_PRIVATE_GRANT_JSON --enable-contract-1-i`
   restores only the proven original through its pinned descriptor, never
   overwriting an occupied name. It does not recover other entries or release
   ownership. `completed: false` / exit 2 is expected until ordinary Undo finishes
   the transaction. Inspect each entry for `RetainedForeign`; a refusal is not a
   successful settlement.
4. The existing compact journal records the retained object's identity, the
   reviewed observations, and the nonce of its consumed signed decision. The
   foreign holding link and the original continuity anchor remain permanently;
   neither is garbage-collected. After interruption, review and approve a NEW
   `retain-foreign` grant only where still actionable. A completely written
   pending receipt is verified before signed repair; torn/unavailable evidence
   refuses and remains REVIEW_REQUIRED. Never retry a consumed grant.
5. Review and approve a fresh ordinary `undo` grant, then execute Undo. Require
   `completed: true`, all originals restored with the enrolled identities,
   `status STATE` = `settled` and ownership `released`. Release checks every
   retained identity, original anchor and original name, plus its consumed
   signed receipt. The retention action itself never releases ownership.

`status STATE` and **`retained STATE`** remain read-only and expose the exact
retained paths, identities, SHA-256 values and signed receipt nonces even after
release. For retrieval, first require a verified inventory (exit 0). Copy the
reported retained path to a **new** private operator-owned filename, for example
`cp --no-clobber -- EXACT_RETAINED_PATH NEW_PRIVATE_COPY`, then compare
`sha256sum -- NEW_PRIVATE_COPY` with the printed SHA-256. A copy is a new inode,
not the original object. Never move/delete the retained file, original anchor,
receipt or authority records. This is preservation and retrieval, not disposal.
The released registration remains single-use; explicit root succession is described
below. Hardlink anchors share existing data blocks; they
are not independent restored files or full extra copies. Retained foreign data
and directory metadata still consume space. No automatic cleanup is provided.

Empty, truncated or invalid grant documents produce an explicit refusal without
consumption: preserve them and authority evidence, review again, and use a NEW
grant filename. Do not overwrite the old document or reset authority to retry.

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
Blue exposes R2 History, fresh Undo and retained-file retrieval separately from
legacy History/Clean Logs. No cleanup of R2 evidence is provided.


## Explicit collection-root succession (still default-OFF)

Only **fully verified Undo with released ownership** permits this transition.
An Applied transaction still owns the root and retains Undo rights; neither
`reuse` nor a new enrollment may discard those rights. A parked object, pending
Undo, unfinished signed retention, uncertain journal, missing evidence or
unreleased/orphaned claims refuses succession. This is not an Apply/commit mode.

Use a **new sibling private state-directory path** beneath the same private
parent as the original state. Keep the collection root, original state, authority,
grants, signer and journals. No new collection or signing key is required.

```text
emuwiz-r2 status STATE
emuwiz-r2 preview-reuse STATE NEW_STATE
emuwiz-r2 reuse STATE NEW_STATE --confirm EXACT_REUSE_CONFIRMATION --enable-contract-1-i
emuwiz-r2 status NEW_STATE
emuwiz-r2 plan NEW_STATE NEW_PLAN_JSON SOURCE DEST
emuwiz-r2 enroll NEW_STATE NEW_PLAN_JSON --confirm NEW_PLAN_CONFIRMATION --enable-contract-1-i
```

Then use the existing review → approve → execute Apply and fresh signed Undo
commands with **NEW_STATE and new grant filenames**. No old grant is renewed.
The new state is OFF and unenrolled after `reuse`; its registration, plan binding,
transaction identity and container are distinct. It shares the original issuer
and admission authority so participating processes cannot branch or overlap.
Do not run `provision` again on the same collection.

The lifecycle boundary publishes an immutable settlement receipt only after the
originals, signed retention proofs, journal and released ownership verify. It
binds the exact final container and registration. Old containers and descriptors
stay in place; successor containers occupy separate generation directories in
the existing reserved recovery namespace. Old binaries refuse the new descriptor
format. A historical settled receipt describes restoration **at that boundary**;
it does not assert that a later transaction has left those old pathnames intact.

`history NEW_STATE` lists verified closed predecessor transactions and their
state paths, preserved journals and retained-file identities. `status OLD_STATE`
and `retained OLD_STATE` continue to verify the closed transaction and retained
objects after later transactions change library names. Retained foreign files
and original continuity anchors remain in place, with the same explicit no-clobber
copy-and-hash retrieval procedure above. They are never automatic cleanup targets.
Current, unclosed operations remain in ordinary `status` and journal History.

On interruption, run `status` on the predecessor and any prepared new state.
If activation is incomplete, obtain a **fresh `preview-reuse`** and retry the same
predecessor and NEW_STATE paths. Known private preparation temporaries are retained;
no partial configuration replaces a committed one. Activation is one append-only
transaction in the existing admission database. A committed predecessor can have
only one successor. Repeating a verified completed transition is idempotent;
a different successor, changed directory identity or conflicting record refuses.
Unknown or corrupted evidence remains REVIEW_REQUIRED; do not delete/reset it.

Each new enrollment rechecks eligibility and all predecessor settlement/retained
proofs under admission. Storage accumulates per transaction; capacity checks still
apply. No retention/garbage collector or production enablement is
introduced. Whole authority rollback, hostile private-storage modification and
power-loss guarantees remain outside the approved contract.

## Optional Blue Advanced Protection

Standard Rename remains the default and recommended everyday choice. Select
**Organisation → Rename/Move → Advanced Protection** explicitly for the limited
R2 envelope above. Blue shows the comparison and exact refusal reasons. An
ineligible operation stays refused; selecting Standard Rename instead requires
an explicit choice. There is no fallback. Do not switch collections or engines
while an enrolled transaction still owns files: recover it first.

Use the normal preview and selected files, check compatibility, then explicitly
prepare a private safety workspace. Preparation provisions keys and authority
but does not enroll or apply. Review exact filenames and original identity
before confirming enrollment; then confirm the separately displayed Apply.
Changing the normal preview invalidates its approval, including changes while a
worker is running. Signing and execution recheck the displayed digest and existing
freshness/identity checks; no replacement plan is silently approved.

Keys are created and read only by the short-lived bundled operator process.
Blue holds locators and public review evidence, never private key bytes. This
reuses the accepted single-user operator custody model: same-UID hostile code
is not excluded. There is no new privileged service or external witness. Keep
the private workspace and all recovery evidence; partial setup is preserved and
refused rather than overwritten. Missing/corrupt authority fails closed.

In **History → Advanced Protection recovery**, open the existing collection
workspace, refresh current state and explicitly review fresh Undo. Review may
finish interrupted admission bookkeeping; refresh itself is read-only. A fresh,
separately confirmed one-shot approval is required to act. REVIEW_REQUIRED and
interrupted states display current reasons and per-file evidence. Supported
recovery is exactly the interrupted-Undo and retained-file procedure above;
unknown/contradictory files are never repaired automatically. A failed worker
shows only a last verified snapshot and requires refresh before another action.

For foreign-held files, review the complete holding inventory, identity/hash and
preserved original anchors before the **separate retain-files decision**. This
retains evidence permanently; it does not release ownership. Afterwards prepare
a new signed Undo to recover eligible originals and verify settlement/release.
Retrieve current or historical retained files only to a new filename outside
the root/workspace. The child rechecks verified inventory and exact receipt,
copies without overwriting, hashes and syncs the copy, and preserves the held
original and anchors. Failure may leave a partial copy for review; never delete
holding as a substitute for settlement.

A settled, released root offers an explicitly reviewed successor transaction;
all predecessor state and retained evidence remain intact. Loading verified
earlier transactions is an explicit participating action because it may finish
admission bookkeeping. It never runs automatically while painting History.

Advanced Protection adds durable original anchors, transaction-specific approval
and conservative recovery evidence; it takes longer. Archived optimized backend
measurements (warm-cache median, two samples, complete plan/review/Apply/Undo
cycle) were 0.14 s Standard versus 0.88 s Advanced for one 1 MiB file, and 0.55 s
versus 4.00 s for eight 1 MiB files. These are measurements, not guarantees or GUI
timing promises. Evidence: `2026-10-17-r2-performance-optimisation/summary.json`
in the owner audit ledger. No performance campaign was rerun for integration.

The scope stays local ext4/4096, one to eight direct regular-file RenameMove
entries. Other writers are not excluded, and complete authority-store rollback
is not detected. Raw authority repair, unsupported filesystem reconciliation and
signer migration remain operator work, with no invented GUI recovery commands.
Production remains default-OFF; installing/promoting Blue does not authorize real
collection enrollment. A controlled GUI trial uses disposable collections,
isolated databases and credentials only.

One GUI recovery limitation remains: interruption while preparing a successor
can leave its highest `state-N` directory without committed configuration.
Discovery refuses that state and preserves both directories. The error identifies
the exact predecessor and next-state paths. Use the existing CLI fresh
`preview-reuse` / explicitly confirmed `reuse` procedure above with that same
next-state path. Blue does not delete, adopt or silently fall back from prepared
state. This narrow interrupted-succession recovery remains CLI-only.
