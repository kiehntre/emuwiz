//! Conversion queue planning and durable, serial execution.
//!
//! Converter-specific inspection, readiness and execution remain in their
//! existing backends; this module owns only queue vocabulary and conservative
//! space arithmetic. [`durable`] adds persisted reviewed jobs and restart
//! recovery while reusing converter execution and Repair publication journals.

pub mod durable;

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaceEstimate {
    Exact(u64),
    Range { minimum: u64, maximum: u64 },
    Unknown,
}

impl SpaceEstimate {
    pub fn maximum(&self) -> Option<u64> {
        match self {
            Self::Exact(value) => Some(*value),
            Self::Range { maximum, .. } => Some(*maximum),
            Self::Unknown => None,
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::Exact(value) => format_bytes(*value),
            Self::Range { minimum, maximum } => {
                format!("{}–{}", format_bytes(*minimum), format_bytes(*maximum))
            }
            Self::Unknown => "Unknown".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionEstimate {
    pub source_size: u64,
    pub destination_size: SpaceEstimate,
    pub temporary_space: SpaceEstimate,
    pub reclaimable_space: SpaceEstimate,
    pub available_space: Option<u64>,
    pub safety_margin: u64,
    pub same_filesystem: Option<bool>,
    pub atomic_publication_requires_duplicate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionPlanningInput {
    pub source_path: PathBuf,
    pub source_format: String,
    pub destination_path: PathBuf,
    pub destination_format: String,
    pub platform: Option<String>,
    pub source_size: u64,
    pub destination_size: SpaceEstimate,
    pub temporary_space: SpaceEstimate,
    pub reclaimable_space: SpaceEstimate,
    pub converter: String,
    pub verification_plan: String,
    pub provenance: String,
    pub readiness_reason: Option<String>,
    pub available_space_override: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionReadiness {
    Ready,
    Waiting,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionQueueState {
    Planned,
    Ready,
    Waiting,
    Running,
    Verifying,
    Completed,
    Failed,
    Refused,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressionPreview {
    pub expected_type: String,
    pub space_saving: SpaceEstimate,
    pub lossless: bool,
    pub preservation_equivalent: bool,
    pub round_trip_identity_expected: bool,
    pub retain_original: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionQueueItem {
    pub id: u64,
    pub source_path: PathBuf,
    pub source_format: String,
    pub destination_path: PathBuf,
    pub destination_format: String,
    pub platform: Option<String>,
    pub estimate: ConversionEstimate,
    pub readiness: ConversionReadiness,
    pub converter: String,
    pub verification_plan: String,
    pub provenance: String,
    pub state: ConversionQueueState,
    pub refusal_or_warning: Option<String>,
    pub compression: Option<CompressionPreview>,
}

impl ConversionQueueItem {
    pub fn plan(id: u64, input: ConversionPlanningInput) -> Self {
        let available_space = input
            .available_space_override
            .or_else(|| available_space(&input.destination_path));
        let same_filesystem = same_filesystem(&input.source_path, &input.destination_path);
        let duplicate = same_filesystem == Some(true);
        let safety_margin = input.source_size / 10;
        let temporary_space = input.temporary_space.clone();
        let required_space = required_space(
            &input.destination_size,
            &temporary_space,
            safety_margin,
            duplicate,
        );
        let (readiness, warning) = if let Some(reason) = input.readiness_reason.clone() {
            (ConversionReadiness::Refused, Some(reason))
        } else if let (Some(available), Some(required)) = (available_space, required_space) {
            if available < required {
                (
                    ConversionReadiness::Refused,
                    Some(format!(
                        "Not enough free space: {} available, {} required conservatively.",
                        format_bytes(available),
                        format_bytes(required)
                    )),
                )
            } else {
                (ConversionReadiness::Ready, None)
            }
        } else {
            (
                ConversionReadiness::Waiting,
                Some("The destination size cannot be estimated safely yet.".into()),
            )
        };
        let state = match readiness {
            ConversionReadiness::Ready => ConversionQueueState::Ready,
            ConversionReadiness::Waiting => ConversionQueueState::Waiting,
            ConversionReadiness::Refused => ConversionQueueState::Refused,
        };
        Self {
            id,
            source_path: input.source_path,
            source_format: input.source_format,
            destination_path: input.destination_path,
            destination_format: input.destination_format,
            platform: input.platform,
            estimate: ConversionEstimate {
                source_size: input.source_size,
                destination_size: input.destination_size,
                temporary_space,
                reclaimable_space: input.reclaimable_space,
                available_space,
                safety_margin,
                same_filesystem,
                atomic_publication_requires_duplicate: duplicate,
            },
            readiness,
            converter: input.converter,
            verification_plan: input.verification_plan,
            provenance: input.provenance,
            state,
            refusal_or_warning: warning,
            compression: None,
        }
    }

    pub fn with_compression(mut self, compression: CompressionPreview) -> Self {
        self.compression = Some(compression);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionQueueResult {
    Completed { verified: bool },
    Failed { reason: String },
    Refused { reason: String },
    Cancelled,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversionQueue {
    pub items: Vec<ConversionQueueItem>,
    next_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionQueueSummary {
    pub ready: usize,
    pub refused: usize,
    pub waiting: usize,
    pub total_input_size: u64,
    pub estimated_output: SpaceEstimate,
    pub estimated_temporary_space: SpaceEstimate,
    pub available_space: Option<u64>,
    pub likely_space_saved: SpaceEstimate,
}

impl ConversionQueue {
    pub fn add(&mut self, input: ConversionPlanningInput) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        self.items.push(ConversionQueueItem::plan(id, input));
        id
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let old_len = self.items.len();
        self.items.retain(|item| item.id != id);
        old_len != self.items.len()
    }

    pub fn cancel_pending(&mut self) {
        for item in &mut self.items {
            if matches!(
                item.state,
                ConversionQueueState::Planned
                    | ConversionQueueState::Ready
                    | ConversionQueueState::Waiting
            ) {
                item.state = ConversionQueueState::Cancelled;
            }
        }
    }

    pub fn summary(&self) -> ConversionQueueSummary {
        let mut output_min = 0;
        let mut output_max = 0;
        let mut output_unknown = false;
        let mut temp_min = 0;
        let mut temp_max = 0;
        let mut temp_unknown = false;
        let mut saved_min = 0;
        let mut saved_max = 0;
        let mut saved_unknown = false;
        let mut available = None;
        for item in &self.items {
            add_estimate(
                &item.estimate.destination_size,
                &mut output_min,
                &mut output_max,
                &mut output_unknown,
            );
            add_estimate(
                &item.estimate.temporary_space,
                &mut temp_min,
                &mut temp_max,
                &mut temp_unknown,
            );
            add_estimate(
                &item.estimate.reclaimable_space,
                &mut saved_min,
                &mut saved_max,
                &mut saved_unknown,
            );
            available = match (available, item.estimate.available_space) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (None, value) | (value, None) => value,
            };
        }
        ConversionQueueSummary {
            ready: self
                .items
                .iter()
                .filter(|item| item.readiness == ConversionReadiness::Ready)
                .count(),
            refused: self
                .items
                .iter()
                .filter(|item| item.readiness == ConversionReadiness::Refused)
                .count(),
            waiting: self
                .items
                .iter()
                .filter(|item| item.readiness == ConversionReadiness::Waiting)
                .count(),
            total_input_size: self
                .items
                .iter()
                .map(|item| item.estimate.source_size)
                .sum(),
            estimated_output: finish_estimate(output_min, output_max, output_unknown),
            estimated_temporary_space: finish_estimate(temp_min, temp_max, temp_unknown),
            available_space: available,
            likely_space_saved: finish_estimate(saved_min, saved_max, saved_unknown),
        }
    }
}

fn required_space(
    destination: &SpaceEstimate,
    temporary: &SpaceEstimate,
    safety_margin: u64,
    duplicate: bool,
) -> Option<u64> {
    let destination = destination.maximum()?;
    let temporary = temporary.maximum()?;
    let duplicate = duplicate.then_some(destination).unwrap_or(0);
    temporary
        .checked_add(destination)?
        .checked_add(duplicate)?
        .checked_add(safety_margin)
}

fn add_estimate(value: &SpaceEstimate, minimum: &mut u64, maximum: &mut u64, unknown: &mut bool) {
    match value {
        SpaceEstimate::Exact(value) => {
            *minimum = minimum.saturating_add(*value);
            *maximum = maximum.saturating_add(*value);
        }
        SpaceEstimate::Range {
            minimum: low,
            maximum: high,
        } => {
            *minimum = minimum.saturating_add(*low);
            *maximum = maximum.saturating_add(*high);
        }
        SpaceEstimate::Unknown => *unknown = true,
    }
}

fn finish_estimate(minimum: u64, maximum: u64, unknown: bool) -> SpaceEstimate {
    if unknown {
        SpaceEstimate::Unknown
    } else if minimum == maximum {
        SpaceEstimate::Exact(minimum)
    } else {
        SpaceEstimate::Range { minimum, maximum }
    }
}

fn same_filesystem(source: &Path, destination: &Path) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let source_device = fs::metadata(source).ok()?.dev();
        let destination_device =
            fs::metadata(destination.parent().unwrap_or_else(|| Path::new(".")))
                .ok()?
                .dev();
        Some(source_device == destination_device)
    }
    #[cfg(not(unix))]
    {
        let _ = (source, destination);
        None
    }
}

fn available_space(destination: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        let path = CString::new(parent.as_os_str().as_bytes()).ok()?;
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: statvfs fills its fixed-size output structure.
        if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
            return None;
        }
        let stats = unsafe { stats.assume_init() };
        Some((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
    }
    #[cfg(not(unix))]
    {
        let _ = destination;
        None
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(platform: &str, destination_size: SpaceEstimate) -> ConversionPlanningInput {
        ConversionPlanningInput {
            source_path: PathBuf::from(format!("/source/{platform}.cue")),
            source_format: "CUE/BIN".into(),
            destination_path: PathBuf::from(format!("/destination/{platform}.chd")),
            destination_format: "CHD".into(),
            platform: Some(platform.into()),
            source_size: 100,
            destination_size,
            temporary_space: SpaceEstimate::Exact(100),
            reclaimable_space: SpaceEstimate::Exact(100),
            converter: "verified CHD backend".into(),
            verification_plan: "canonical optical fingerprint".into(),
            provenance: "existing CHD conversion plan".into(),
            readiness_reason: None,
            available_space_override: Some(10_000),
        }
    }

    #[test]
    fn mixed_platforms_keep_order_and_summarise_unknown_output() {
        let mut queue = ConversionQueue::default();
        queue.add(input("Saturn", SpaceEstimate::Unknown));
        queue.add(input(
            "PlayStation",
            SpaceEstimate::Range {
                minimum: 40,
                maximum: 60,
            },
        ));
        assert_eq!(queue.items[0].platform.as_deref(), Some("Saturn"));
        assert_eq!(queue.items[1].platform.as_deref(), Some("PlayStation"));
        let summary = queue.summary();
        assert_eq!(summary.ready, 1);
        assert_eq!(summary.waiting, 1);
        assert_eq!(summary.estimated_output, SpaceEstimate::Unknown);
    }

    #[test]
    fn insufficient_space_refuses_without_touching_files() {
        let mut item = input("Dreamcast", SpaceEstimate::Exact(500));
        item.available_space_override = Some(100);
        let planned = ConversionQueueItem::plan(1, item);
        assert_eq!(planned.readiness, ConversionReadiness::Refused);
        assert_eq!(planned.state, ConversionQueueState::Refused);
        assert!(
            planned
                .refusal_or_warning
                .unwrap()
                .contains("Not enough free space")
        );
    }

    #[test]
    fn explicit_refusal_and_cancellation_are_distinct_states() {
        let mut queue = ConversionQueue::default();
        let mut planned = input("PSP", SpaceEstimate::Exact(50));
        planned.readiness_reason =
            Some("PSP execution remains on its existing direct workflow.".into());
        queue.add(planned);
        assert_eq!(queue.items[0].state, ConversionQueueState::Refused);
        queue.cancel_pending();
        assert_eq!(queue.items[0].state, ConversionQueueState::Refused);
    }

    #[test]
    fn planning_is_deterministic_and_does_not_create_output() {
        let input = input("Saturn", SpaceEstimate::Unknown);
        let left = ConversionQueueItem::plan(7, input.clone());
        let right = ConversionQueueItem::plan(7, input);
        assert_eq!(left, right);
        assert!(!left.destination_path.exists());
    }
}
