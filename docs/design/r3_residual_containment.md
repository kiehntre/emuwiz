# Residual R3 containment checks

Recovery retains its parent and root bindings through completion publication.
A detected late binding failure restores unresolved state and attempts one durable
correction. Correction failure returns an error, never successful recovery; the
old journal may still contain historical Applied bytes. No ROM is moved by this
reconciliation step.

Reload refuses demonstrably redirected Applied RenameMove paths. The same check
runs before reconciliation (including Undo), exact resume and cleanup
classification. Cleanup returns Uncertain, preserving evidence. Missing objects
still go through existing identity checks; journal state is not proof of current
location.

Roots must be real canonical directories, opened without following a final
symlink. Their descriptors and identities remain held through verification.
Recovery cannot prove an originally trusted symlink identity from a journal path
alone and refuses that case. A configured alias can supply its previously resolved
canonical root; changing the alias does not change that retained binding.

These checks detect failures; they do not exclude writers. An external writer can
act after the final check or reverse a temporary swap before it. Crash or I/O
failure can prevent correction. An atomic guarantee requires an enforced
filesystem/credential boundary excluding competing namespace writers through
mutation, publication and subsequent use. Advisory locks and repeated checks are
not such a boundary. Old journals also lack the original root identity needed to
detect arbitrary real-directory replacement before binding. This patch neither
authenticates journal contents nor manufactures that missing proof.

No R2 writer, capacity policy, approval service or migration is enabled.
