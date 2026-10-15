# Separate GUI job: Rename History, Holding Area and completed-log cleanup

This is backlog only. No GUI implementation or activation is included in the
Contract 1-I filesystem experiment. Require separate owner approval and
coordinate with Organisation/GUI owners before editing their modules.

Use the existing authoritative operation journals and attention projections;
do not create a second rename log. Rename History should show the game title
when known, original and proposed filenames/locations, date, transaction ID,
recorded identity/checksum, operation state, Undo eligibility and the reason
for every unresolved file. Distinguish original preserved, correct destination
published, original relocated, bytes salvaged into a new inode, interference
detected, original possibly unrecoverable, Undo incomplete and Undo completed.
A partial or failed Undo must never receive a success banner.

A Holding Area panel should associate each entry with its authoritative
transaction and reason for retention. Show operator attention and safe cleanup
eligibility; unidentified or contradictory entries require review. A future
Clear action must use an independently accepted cleanup mechanism, never
unlink files merely because they look temporary or old.

Count entries separately from objects. Group identities by device/inode and
identify hardlink aliases; do not sum apparent sizes as additional storage.
Allocated blocks describe allocation on an inode, not ownership of those blocks
by a particular link. Reflink/shared extents require filesystem-specific evidence
or an explicit unknown estimate. Display logical size, known allocated storage,
shared allocation and uncertainty separately, without claiming exact reclaimable
space from link counts alone.

Clean completed logs is a separate action from holding-data cleanup. Preview
eligible completed rename and other operation records, and exclude unresolved,
interrupted or recovery-required records. Holding game data is never an ordinary
log. Explain when deleting each selected record removes Undo or recovery rights;
require explicit confirmation for loss of Undo and offer export beforehand.
Revalidate eligibility at deletion time and fail closed on changed evidence.

Scanner/duplicate-detector integration must exclude the reserved private holding
area before traversal and hashing, using the accepted root/holding binding rather
than a broad extension filter. An exclusion must not grant cleanup authority or
hide unresolved entries from the dedicated attention/holding view. This remains
unimplemented pending Gate 1 acceptance and a separately scoped integration.
