//! Shared bounded archive stream probe. No format-specific extraction or I/O.
use super::{ContentDetectionReport, ContentDetector, run_content_detectors};
use std::io::{self, Read};

/// Extra complete-input bytes across one archive, in addition to its existing
/// bounded prefixes. Exhaustion abstains; it never validates a prefix.
pub const MAX_COMPLETE_INPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_SINGLE_COMPLETE_INPUT_BYTES: usize = 8 * 1024 * 1024;

pub fn probe_content_stream(
    mut reader: impl Read,
    declared_size: u64,
    prefix_limit: usize,
    extra_budget: &mut usize,
    detectors: &[&dyn ContentDetector],
) -> io::Result<(usize, ContentDetectionReport)> {
    let mut bytes = Vec::with_capacity(prefix_limit.min(64 * 1024));
    (&mut reader)
        .take(prefix_limit as u64)
        .read_to_end(&mut bytes)?;
    let prefix_bytes = bytes.len();
    let complete_limit = detectors
        .iter()
        .filter_map(|d| d.complete_input_limit(&bytes))
        .max()
        .unwrap_or(0)
        .min(MAX_SINGLE_COMPLETE_INPUT_BYTES);
    let mut reached_eof = bytes.len() < prefix_limit;
    if declared_size >= bytes.len() as u64 && declared_size <= complete_limit as u64 && !reached_eof
    {
        let needed = declared_size as usize - bytes.len();
        if needed < *extra_budget {
            // One extra byte detects incorrect extents and lets the archive
            // decoder report its final CRC failure.
            let result = (&mut reader)
                .take(needed as u64 + 1)
                .read_to_end(&mut bytes);
            // Failed decoders still consumed budget. Repeated corrupt
            // members must not receive a fresh complete-input allowance.
            *extra_budget -= bytes.len() - prefix_bytes;
            result?;
            if bytes.len() as u64 != declared_size {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "member extent differs from declared size",
                ));
            }
            reached_eof = true;
        }
    }
    let complete = reached_eof && bytes.len() as u64 == declared_size;
    let report = run_content_detectors(
        detectors
            .iter()
            .copied()
            .filter(|d| complete || !d.requires_complete_input()),
        &bytes,
    );
    Ok((bytes.len(), report))
}
