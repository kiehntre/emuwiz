# Read-only compact recovery groundwork

Extracted from `26c71a084cdc8180e68493d1fb03375d16d0d863` onto
`3bb213da4ef841c41c3970d07f457b68d6c5fd46`.

`dat::rename_apply::compact_recovery` accepts caller-provided bytes. It exposes
legacy JSON decoding, duplicate-key validation, exact original-byte preservation,
framed-record validation, observation materialization and checked budget estimates.
It performs no filesystem access and has no runtime callers.

The historical `Authority` name describes the original operation evidence only.
Neither that object, its unkeyed digest nor a successful observation authenticates
an issuer, proves filesystem state or authorizes Apply, Undo or migration.
Unknown metadata is preserved in original bytes. Unknown operation variants are
rejected by the existing transaction schema. Known operation variants can be
observed; observation does not assert executable support for them. Paths are
retained as evidence without canonicalization or filesystem validation.

Input byte limits are caller-supplied inspection bounds, not changes to production
journal admission. The budget function is an estimate, not an allocation or space
reservation. Legacy 75/100-entry evidence can be decoded under an adequate caller
limit without making those transactions recoverable.

Only the donor's pure module and its twelve tests were retained. Storage,
approval service, restart adapter, legacy continuation and runtime action
projection were excluded. Two added tests cover malformed/oversized/unknown
operation evidence and paths longer than filesystem path limits.

Existing Apply, Undo, journal writing and shared transaction code are unchanged.
This extraction does not include the donor's journal capacity or lease policy.
The accepted full candidate and its safety protections remain on their original
branch. Production integration of recovery still needs separate acceptance.
