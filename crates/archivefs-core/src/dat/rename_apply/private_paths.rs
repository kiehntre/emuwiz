//! Reserved recovery namespaces are not game inputs. This lexical exclusion is
//! scan hygiene only: it grants neither ownership nor deletion authority. It
//! also covers explicitly selected descendants when authoritative records are
//! missing, so retained data is never silently reclassified as ordinary games.
use std::path::Path;
pub(crate) fn is_private_recovery_path(path: &Path) -> bool {
    path.ancestors().any(|part| {
        part.file_name().is_some_and(|name| {
            name == ".emuwiz-r2-holding" || name == "emuwiz-enrolled-recovery-v3"
        })
    })
}
