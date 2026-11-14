//! The versioned rule set and the diagnostic engine.
//!
//! A rule is a pure function from evidence to findings. It has an
//! identifier, a version that changes whenever its meaning changes, and a
//! one-line statement of what it concludes and from what. The engine runs
//! every rule, then applies limits that no rule can opt out of:
//!
//! - a finding must cite observed, current evidence; evidence that is
//!   stale, unavailable or merely asserted is not support;
//! - a finding that rests on untrusted text needs at least one firm
//!   observation other than the process exit to rise above
//!   [`Confidence::Possible`];
//! - a finding that names a cause from nothing but an exit status and an
//!   unspecific error message is rejected outright;
//! - a finding with contradicting evidence, or one that contradicts
//!   another finding, is held at [`Confidence::Possible`] and the report
//!   says so.
//!
//! What happened (a crash, an exit status) is reported as a *symptom*; why
//! it happened is a *cause*. A report with symptoms and no cause has the
//! verdict [`Verdict::Unknown`], and stays that way until evidence, not
//! inference, says otherwise.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::evidence::{
    AreaCoverage, BlockerFamily, ConfigLayer, EVIDENCE_SCHEMA_VERSION, Evidence, EvidenceArea,
    EvidenceId, EvidenceSet, EvidenceState, ExitOutcome, Fact, FirmwareStatus, GraphicsApi,
    LogEvent, LogMeaning, MediaIntegrityStatus, PathAccess, PathPresence, PathRole, ProbeScope,
    ProcessExit, SpawnFailure, Subject, Trust,
};
use super::process::{SignalKind, possibly_relayed_signal, signal_info};
use super::signatures::{HOST_EMULATOR, HOST_SIGNATURES};
use super::{limitations, retroarch};

/// Changes when the engine's own limits or verdict logic change.
pub const ENGINE_VERSION: u32 = 1;

/// The kinds of problem the rules can tell apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisCategory {
    MissingFirmware,
    InvalidOrUnsupportedMedia,
    UnsupportedEmulatorVersion,
    GraphicsInitialisation,
    MissingDependency,
    SandboxFailure,
    PermissionError,
    AudioDevice,
    ControllerConfiguration,
    ConflictingModifications,
    /// The launch planner refused for a reason outside the other
    /// categories (for example an unconfirmed game identity).
    LaunchBlocked,
    Crash,
    UnexplainedExit,
}

impl DiagnosisCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::MissingFirmware => "Missing firmware",
            Self::InvalidOrUnsupportedMedia => "Invalid or unsupported game file",
            Self::UnsupportedEmulatorVersion => "Unsupported emulator version",
            Self::GraphicsInitialisation => "Graphics could not start",
            Self::MissingDependency => "Missing program or component",
            Self::SandboxFailure => "Sandbox problem",
            Self::PermissionError => "Permission problem",
            Self::AudioDevice => "Sound problem",
            Self::ControllerConfiguration => "Controller setup",
            Self::ConflictingModifications => "Conflicting patches or mods",
            Self::LaunchBlocked => "Stopped before launch",
            Self::Crash => "Crash",
            Self::UnexplainedExit => "Unexplained exit",
        }
    }
}

/// How firmly the evidence supports a finding. Earned, never assumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Firm observations show it directly.
    Established,
    /// Firm observations and the emulator's own account agree, but no
    /// direct check of the cause was made.
    Probable,
    /// Consistent with what was seen; not shown.
    Possible,
}

impl Confidence {
    fn rank(self) -> u8 {
        match self {
            Self::Established => 2,
            Self::Probable => 1,
            Self::Possible => 0,
        }
    }

    fn at_most(self, cap: Self) -> Self {
        if self.rank() > cap.rank() { cap } else { self }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Established => "established",
            Self::Probable => "probable",
            Self::Possible => "possible",
        }
    }
}

/// What the problem does to the launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    PreventsLaunch,
    /// The game can run, with something missing or at risk.
    Degrades,
    Unknown,
}

/// A statement a finding makes that another finding could contradict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "subject", content = "of", rename_all = "snake_case")]
pub enum ClaimSubject {
    GraphicsApiUsable(GraphicsApi),
    GameFileReadable,
    GameLoaded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    #[serde(flatten)]
    pub subject: ClaimSubject,
    pub holds: bool,
}

/// A further check that would narrow the diagnosis. Descriptive only:
/// nothing here is run by the engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "check", rename_all = "snake_case")]
pub enum NextCheck {
    /// Gather evidence in an area that has none.
    CollectEvidence { area: EvidenceArea, why: String },
    /// Start the emulator again in an isolated session with detailed
    /// logging.
    VerboseDiagnosticRun { why: String },
    /// Check whether a graphics API can be used in the launch environment.
    ProbeGraphics { api: GraphicsApi, why: String },
    /// Compare the current value of a setting with an alternative, in an
    /// isolated session that leaves the real settings untouched.
    CompareSetting {
        emulator: String,
        key: String,
        alternative: String,
        why: String,
    },
    /// Something only the person at the computer can do or check.
    UserAction { what: String },
}

impl NextCheck {
    /// The check in plain words.
    pub fn describe(&self) -> String {
        match self {
            Self::CollectEvidence { area, why } => format!("Check {}: {why}", area.label()),
            Self::VerboseDiagnosticRun { why } => format!(
                "Start the emulator once more with detailed logging, in an isolated session: \
                 {why}"
            ),
            Self::ProbeGraphics { api, why } => {
                format!(
                    "Check whether {} works where the emulator runs: {why}",
                    api.label()
                )
            }
            Self::CompareSetting {
                key,
                alternative,
                why,
                ..
            } => format!(
                "Compare with {key} = \"{alternative}\" in an isolated session, leaving the \
                 real settings untouched: {why}"
            ),
            Self::UserAction { what } => what.clone(),
        }
    }
}

/// A pointer to a repair the repair planner knows how to prepare. It is
/// not a repair and authorises nothing: the planner decides independently
/// whether the repair is valid, previews it and asks for consent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairHint {
    pub template: String,
    pub summary: String,
}

/// One conclusion, with the evidence for and against it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnosis {
    pub rule: String,
    pub rule_version: u32,
    pub category: DiagnosisCategory,
    pub confidence: Confidence,
    /// True for a cause; false for a symptom, which says what happened
    /// without saying why.
    pub explains_cause: bool,
    /// True when this restates an error the emulator itself printed. That
    /// the emulator reported it is established; that it is the underlying
    /// cause is not, and a cause is reported separately when the evidence
    /// supports one.
    #[serde(default)]
    pub reported: bool,
    pub effect: Effect,
    pub title: String,
    pub explanation: String,
    pub supporting: Vec<EvidenceId>,
    pub contradicting: Vec<EvidenceId>,
    /// What keeps the confidence from being higher, in plain words.
    pub limitations: Vec<String>,
    pub next_checks: Vec<NextCheck>,
    pub repairs: Vec<RepairHint>,
    pub claims: Vec<Claim>,
}

impl Diagnosis {
    /// How the finding stands, for display next to its title: its
    /// confidence, or that it is the emulator's own report.
    pub fn standing(&self) -> &'static str {
        if self.reported {
            "reported by the emulator"
        } else {
            self.confidence.label()
        }
    }
}

/// Two findings that cannot both be right.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    #[serde(flatten)]
    pub subject: ClaimSubject,
    pub rules: Vec<String>,
}

/// A finding a rule produced and the engine refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedFinding {
    pub rule: String,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Nothing in the evidence shows a problem. This is not a statement
    /// that the game works.
    NoProblemObserved,
    /// At least one established cause, and nothing contradicts it.
    Explained,
    /// Causes were found, none of them established.
    PartlyExplained,
    /// Findings or evidence contradict each other.
    Conflicting,
    /// A problem was observed, or the evidence could not be interpreted,
    /// and no rule could explain it.
    Unknown,
}

impl Verdict {
    /// The verdict as a sentence fragment for a report heading.
    pub fn headline(self) -> &'static str {
        match self {
            Self::NoProblemObserved => {
                "no problem observed in the evidence supplied (this does not show that the \
                 game works)"
            }
            Self::Explained => "explained",
            Self::PartlyExplained => "partly explained: a cause is suggested but not established",
            Self::Conflicting => "conflicting: the evidence or the findings disagree",
            Self::Unknown => "unknown: something went wrong and the evidence does not show why",
        }
    }
}

/// Identifies the rule set a report was produced with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSetVersion {
    pub engine_version: u32,
    /// Lower-case hex SHA-256 over every rule, signature and table entry.
    pub digest: String,
}

/// One rule, as shown to people.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RuleInfo {
    pub id: &'static str,
    pub version: u32,
    pub category: DiagnosisCategory,
    pub summary: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RuleSetInfo {
    #[serde(flatten)]
    pub version: RuleSetVersion,
    pub rules: Vec<RuleInfo>,
}

/// The result of one diagnosis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosisReport {
    pub schema_version: u32,
    pub rule_set: RuleSetVersion,
    pub subject: Subject,
    pub verdict: Verdict,
    /// Why, most firmly supported first.
    pub causes: Vec<Diagnosis>,
    /// What happened.
    pub symptoms: Vec<Diagnosis>,
    pub conflicts: Vec<Conflict>,
    pub rejected: Vec<RejectedFinding>,
    /// Every evidence area, including those with nothing observed.
    pub coverage: Vec<AreaCoverage>,
    /// Limits that apply to the report as a whole.
    pub notes: Vec<String>,
    pub evidence: EvidenceSet,
}

/// A rule's raw output, before the engine's limits are applied.
pub(crate) struct Finding {
    category: Option<DiagnosisCategory>,
    confidence: Confidence,
    explains_cause: bool,
    reported: bool,
    effect: Effect,
    title: String,
    explanation: String,
    supporting: Vec<EvidenceId>,
    contradicting: Vec<EvidenceId>,
    limitations: Vec<String>,
    next_checks: Vec<NextCheck>,
    repairs: Vec<RepairHint>,
    claims: Vec<Claim>,
}

impl Finding {
    pub(crate) fn cause(
        confidence: Confidence,
        effect: Effect,
        title: impl Into<String>,
        explanation: impl Into<String>,
    ) -> Self {
        Self {
            category: None,
            confidence,
            explains_cause: true,
            reported: false,
            effect,
            title: title.into(),
            explanation: explanation.into(),
            supporting: Vec::new(),
            contradicting: Vec::new(),
            limitations: Vec::new(),
            next_checks: Vec::new(),
            repairs: Vec::new(),
            claims: Vec::new(),
        }
    }

    pub(crate) fn symptom(
        confidence: Confidence,
        title: impl Into<String>,
        explanation: impl Into<String>,
    ) -> Self {
        Self {
            explains_cause: false,
            ..Self::cause(confidence, Effect::Unknown, title, explanation)
        }
    }

    /// A symptom that restates what the emulator itself printed.
    pub(crate) fn reported(title: impl Into<String>, explanation: impl Into<String>) -> Self {
        Self {
            reported: true,
            ..Self::symptom(Confidence::Established, title, explanation)
        }
    }

    pub(crate) fn with_effect(mut self, effect: Effect) -> Self {
        self.effect = effect;
        self
    }

    pub(crate) fn in_category(mut self, category: DiagnosisCategory) -> Self {
        self.category = Some(category);
        self
    }

    pub(crate) fn support(mut self, evidence: &Evidence) -> Self {
        if !self.supporting.contains(&evidence.id) {
            self.supporting.push(evidence.id.clone());
        }
        self
    }

    pub(crate) fn support_all<'a>(mut self, items: impl IntoIterator<Item = &'a Evidence>) -> Self {
        for item in items {
            self = self.support(item);
        }
        self
    }

    pub(crate) fn contradict(mut self, evidence: &Evidence) -> Self {
        if !self.contradicting.contains(&evidence.id) {
            self.contradicting.push(evidence.id.clone());
        }
        self
    }

    pub(crate) fn limit(mut self, limitation: impl Into<String>) -> Self {
        self.limitations.push(limitation.into());
        self
    }

    pub(crate) fn next(mut self, check: NextCheck) -> Self {
        self.next_checks.push(check);
        self
    }

    pub(crate) fn repair(mut self, template: &str, summary: impl Into<String>) -> Self {
        self.repairs.push(RepairHint {
            template: template.to_string(),
            summary: summary.into(),
        });
        self
    }

    pub(crate) fn claim(mut self, subject: ClaimSubject, holds: bool) -> Self {
        self.claims.push(Claim { subject, holds });
        self
    }
}

/// One rule.
pub(crate) struct Rule {
    pub(crate) id: &'static str,
    pub(crate) version: u32,
    pub(crate) category: DiagnosisCategory,
    pub(crate) summary: &'static str,
    pub(crate) evaluate: fn(&View<'_>) -> Vec<Finding>,
}

/// A setting as the emulator will apply it: the value from the
/// highest-precedence settings file that sets the key.
pub(crate) struct Setting<'a> {
    pub(crate) evidence: &'a Evidence,
    pub(crate) value: &'a str,
    pub(crate) layer: ConfigLayer,
}

/// Read access to the observed, current evidence. Rules see nothing else.
pub(crate) struct View<'a> {
    set: &'a EvidenceSet,
}

impl<'a> View<'a> {
    pub(crate) fn new(set: &'a EvidenceSet) -> Self {
        Self { set }
    }

    /// Observed, current items that were actually observed by something:
    /// a statement that was only asserted is never shown to a rule.
    pub(crate) fn observed(&self, area: EvidenceArea) -> impl Iterator<Item = &'a Evidence> {
        self.set
            .observed(area)
            .filter(|item| item.trust != Trust::Asserted)
    }

    /// The process exit, when exactly one was recorded. Evidence that
    /// mixes several runs is not interpreted.
    pub(crate) fn exit(&self) -> Option<(&'a Evidence, &'a ProcessExit)> {
        let mut exits =
            self.observed(EvidenceArea::ProcessExit)
                .filter_map(|item| match &item.fact {
                    Fact::ProcessExit(exit) => Some((item, exit)),
                    _ => None,
                });
        let first = exits.next()?;
        exits.next().is_none().then_some(first)
    }

    pub(crate) fn failed_exit(&self) -> Option<(&'a Evidence, &'a ProcessExit)> {
        self.exit().filter(|(_, exit)| exit.is_failure())
    }

    pub(crate) fn events(&self, meaning: LogMeaning) -> Vec<(&'a Evidence, &'a LogEvent)> {
        self.observed(EvidenceArea::LogEvents)
            .filter_map(|item| match &item.fact {
                Fact::LogEvent(event) if event.meaning == meaning => Some((item, event)),
                _ => None,
            })
            .collect()
    }

    /// Whether any examined log of `emulator` was at informational level.
    pub(crate) fn verbose_log(&self, emulator: &str) -> bool {
        self.observed(EvidenceArea::LogEvents).any(|item| {
            matches!(&item.fact, Fact::LogCoverage(coverage)
                if coverage.emulator == emulator && coverage.verbose)
        })
    }

    pub(crate) fn setting(&self, emulator: &str, key: &str) -> Option<Setting<'a>> {
        self.observed(EvidenceArea::EmulatorConfiguration)
            .filter_map(|item| match &item.fact {
                Fact::ConfigValue {
                    emulator: found_emulator,
                    layer,
                    key: found_key,
                    value: Some(value),
                    ..
                } if found_emulator == emulator && found_key == key => Some(Setting {
                    evidence: item,
                    value: value.as_str(),
                    layer: *layer,
                }),
                _ => None,
            })
            .max_by_key(|setting| setting.layer.precedence())
    }

    /// Reasons the settings files of `emulator` may not show the values it
    /// will really use.
    pub(crate) fn setting_caveats(&self, emulator: &str) -> Vec<String> {
        let mut caveats = Vec::new();
        for item in self.observed(EvidenceArea::EmulatorConfiguration) {
            let Fact::ConfigStructure {
                emulator: found_emulator,
                layer,
                includes,
                repeated_keys,
                ..
            } = &item.fact
            else {
                continue;
            };
            if found_emulator != emulator {
                continue;
            }
            if *includes > 0 {
                caveats.push(format!(
                    "The {} file includes other settings files, which were not read.",
                    layer.label()
                ));
            }
            if !repeated_keys.is_empty() {
                caveats.push(format!(
                    "The {} file sets {} more than once; the first value is the one used.",
                    layer.label(),
                    repeated_keys.join(", ")
                ));
            }
        }
        caveats
    }

    /// Whether any examined log, of any program, was at informational level.
    pub(crate) fn any_verbose_log(&self) -> bool {
        self.observed(EvidenceArea::LogEvents)
            .any(|item| matches!(&item.fact, Fact::LogCoverage(coverage) if coverage.verbose))
    }

    /// Qualify a finding that rests on a setting's value.
    ///
    /// A settings file that could not be read, or that changed after it was
    /// read, may hold the value the emulator really uses: the finding is
    /// then held at possible. Features of the files that were read
    /// (included files, repeated keys) and layers nobody looked at become
    /// limitations.
    pub(crate) fn qualify_setting_finding(&self, mut finding: Finding, emulator: &str) -> Finding {
        for caveat in self.setting_caveats(emulator) {
            finding = finding.limit(caveat);
        }
        let mut unreadable = 0usize;
        let mut layers_examined = BTreeSet::new();
        for item in self.set.items() {
            if item.area != EvidenceArea::EmulatorConfiguration || item.trust == Trust::Asserted {
                continue;
            }
            match (&item.state, &item.fact) {
                (EvidenceState::Stale { .. } | EvidenceState::Unavailable { .. }, _) => {
                    unreadable += 1;
                }
                (
                    EvidenceState::Observed,
                    Fact::ConfigValue { layer, .. }
                    | Fact::ConfigStructure { layer, .. }
                    | Fact::ConfigLayerAbsent { layer, .. },
                ) => {
                    layers_examined.insert(*layer);
                }
                _ => {}
            }
        }
        if unreadable > 0 {
            finding.confidence = finding.confidence.at_most(Confidence::Possible);
            finding = finding.limit(
                "A settings file could not be read, or changed while it was being \
                 examined, so the value the emulator really uses may be different.",
            );
        }
        let overrides = [
            ConfigLayer::Core,
            ConfigLayer::ContentDirectory,
            ConfigLayer::Game,
        ];
        if !overrides
            .iter()
            .any(|layer| layers_examined.contains(layer))
        {
            finding = finding.limit(
                "Only the main settings file was examined; an override for this core, \
                 folder or game could set a different value.",
            );
        }
        finding
    }

    pub(crate) fn environment(&self, name: &str) -> Option<(&'a Evidence, Option<&'a str>)> {
        self.observed(EvidenceArea::LaunchEnvironment)
            .find_map(|item| match &item.fact {
                Fact::EnvironmentVariable { name: found, value } if found == name => {
                    Some((item, value.as_deref()))
                }
                _ => None,
            })
    }

    pub(crate) fn graphics(
        &self,
        api: GraphicsApi,
        scope: ProbeScope,
    ) -> Option<(&'a Evidence, bool)> {
        self.observed(EvidenceArea::Graphics)
            .find_map(|item| match &item.fact {
                Fact::GraphicsApi {
                    api: found_api,
                    scope: found_scope,
                    usable,
                    ..
                } if *found_api == api && *found_scope == scope => Some((item, *usable)),
                _ => None,
            })
    }

    pub(crate) fn path(&self, role: PathRole) -> Option<(&'a Evidence, &'a PathAccess)> {
        self.observed(EvidenceArea::Filesystem)
            .find_map(|item| match &item.fact {
                Fact::PathAccess(access) if access.role == role => Some((item, access)),
                _ => None,
            })
    }

    /// The emulator program evidence and how it is installed.
    pub(crate) fn installation(&self) -> Option<(&'a Evidence, &'a str)> {
        self.observed(EvidenceArea::EmulatorExecutable)
            .find_map(|item| match &item.fact {
                Fact::EmulatorExecutable {
                    installation: Some(installation),
                    ..
                } => Some((item, installation.as_str())),
                _ => None,
            })
    }
}

fn seconds(exit: &ProcessExit) -> String {
    match exit.duration_ms {
        Some(ms) if ms < 1_000 => " in under a second".to_string(),
        Some(ms) => format!(" after {} s", ms / 1_000),
        None => String::new(),
    }
}

fn launcher_caveat(exit: &ProcessExit) -> Option<String> {
    exit.launcher.as_ref().map(|launcher| {
        format!(
            "The emulator was started through {launcher}, so the exit status is what \
             {launcher} reported, not necessarily what the emulator itself returned."
        )
    })
}

const RETROARCH: &str = retroarch::EMULATOR;

fn firmware_missing(view: &View<'_>) -> Vec<Finding> {
    view.observed(EvidenceArea::Firmware)
        .filter_map(|item| match &item.fact {
            Fact::Firmware {
                requirement,
                status: FirmwareStatus::Missing,
            } => Some(
                Finding::cause(
                    Confidence::Established,
                    Effect::PreventsLaunch,
                    format!("Required firmware is missing: {requirement}"),
                    format!(
                        "EmuWiz looked for {requirement} before launch and did not find it. \
                         The emulator needs it to start this game."
                    ),
                )
                .support(item)
                .next(NextCheck::UserAction {
                    what: "Add the firmware file in Firmware setup, then check again.".into(),
                }),
            ),
            _ => None,
        })
        .collect()
}

fn game_file_missing(view: &View<'_>) -> Vec<Finding> {
    let Some((item, access)) = view.path(PathRole::GameFile) else {
        return Vec::new();
    };
    if access.presence != PathPresence::Missing {
        return Vec::new();
    }
    let events = view.events(LogMeaning::ContentReadFailed);
    vec![
        Finding::cause(
            Confidence::Established,
            Effect::PreventsLaunch,
            "The game file is missing",
            format!(
                "Nothing exists at {}. The file may have been moved, renamed or its drive \
                 may not be connected.",
                access.path
            ),
        )
        .support(item)
        .support_all(events.iter().map(|(event, _)| *event))
        .claim(ClaimSubject::GameFileReadable, false)
        .next(NextCheck::UserAction {
            what: "Check that the drive holding the game is connected, then scan the \
                   library again."
                .into(),
        }),
    ]
}

fn game_file_unreadable(view: &View<'_>) -> Vec<Finding> {
    let Some((item, access)) = view.path(PathRole::GameFile) else {
        return Vec::new();
    };
    if access.presence != PathPresence::RegularFile || access.readable != Some(false) {
        return Vec::new();
    }
    let events = view.events(LogMeaning::ContentReadFailed);
    vec![
        Finding::cause(
            Confidence::Established,
            Effect::PreventsLaunch,
            "The game file cannot be read",
            format!(
                "{} exists, but this user account is not allowed to read it.",
                access.path
            ),
        )
        .support(item)
        .support_all(events.iter().map(|(event, _)| *event))
        .claim(ClaimSubject::GameFileReadable, false)
        .next(NextCheck::UserAction {
            what: "Check the file's permissions and owner.".into(),
        }),
    ]
}

fn media_integrity_failed(view: &View<'_>) -> Vec<Finding> {
    view.observed(EvidenceArea::MediaIntegrity)
        .filter_map(|item| match &item.fact {
            Fact::MediaIntegrity {
                status: MediaIntegrityStatus::Failed,
                detail,
            } => Some(
                Finding::cause(
                    Confidence::Established,
                    Effect::PreventsLaunch,
                    "The game file failed its integrity check",
                    format!("EmuWiz checked the game file and found a problem: {detail}"),
                )
                .support(item),
            ),
            _ => None,
        })
        .collect()
}

fn retroarch_content_rejected(view: &View<'_>) -> Vec<Finding> {
    let events = view.events(LogMeaning::ContentRejected);
    let (Some((event, _)), Some((exit_item, _))) = (events.first(), view.failed_exit()) else {
        return Vec::new();
    };
    // Held at possible whatever else is known: a readable file rules out a
    // read problem, but it is true of every healthy setup and says nothing
    // about whether the core refused the file.
    let mut finding = Finding::cause(
        Confidence::Possible,
        Effect::PreventsLaunch,
        "The emulator core refused the game file",
        "RetroArch opened the game file and its core would not load it. The file may be \
         damaged, be a different kind of file than this core expects, or need a different \
         core.",
    )
    .support(event)
    .support(exit_item)
    .limit("RetroArch does not say why the core refused the file.");
    match view.path(PathRole::GameFile) {
        Some((path_item, access))
            if access.presence == PathPresence::RegularFile && access.readable == Some(true) =>
        {
            finding = finding
                .support(path_item)
                .limit("The file is present and readable, so this is not a problem reading it.");
        }
        Some(_) => {}
        None => {
            finding = finding.next(NextCheck::CollectEvidence {
                area: EvidenceArea::Filesystem,
                why: "Confirm the game file is present and readable.".into(),
            });
        }
    }
    if view.observed(EvidenceArea::MediaIntegrity).next().is_none() {
        finding = finding.next(NextCheck::CollectEvidence {
            area: EvidenceArea::MediaIntegrity,
            why: "Verify the game file against a trusted record.".into(),
        });
    }
    vec![finding]
}

fn retroarch_content_read_failed(view: &View<'_>) -> Vec<Finding> {
    let events = view.events(LogMeaning::ContentReadFailed);
    let Some((event, _)) = events.first() else {
        return Vec::new();
    };
    let exit = view.failed_exit();
    let path = view.path(PathRole::GameFile);
    match path {
        // Missing and unreadable files have their own rules.
        Some((_, access))
            if access.presence == PathPresence::Missing || access.readable == Some(false) =>
        {
            Vec::new()
        }
        Some((path_item, access))
            if access.presence == PathPresence::RegularFile && access.readable == Some(true) =>
        {
            // Readable on the host, yet RetroArch could not read it.
            let mut finding = Finding::cause(
                Confidence::Possible,
                Effect::PreventsLaunch,
                "RetroArch could not see the game file",
                format!(
                    "RetroArch reported that it could not read {}, but the file is present \
                     and readable from EmuWiz. RetroArch may be running somewhere the file \
                     is not visible.",
                    access.path
                ),
            )
            .in_category(DiagnosisCategory::SandboxFailure)
            .support(event)
            .contradict(path_item)
            .limit("The file may also have changed between the launch and this check.");
            if let Some((exit_item, _)) = exit {
                finding = finding.support(exit_item);
            }
            if let Some((install_item, installation)) = view.installation()
                && installation.eq_ignore_ascii_case("flatpak")
            {
                finding = finding.support(install_item).limit(
                    "RetroArch is installed as a Flatpak, which only sees folders it has \
                     been granted.",
                );
            }
            vec![finding]
        }
        _ => {
            let mut finding = Finding::cause(
                Confidence::Possible,
                Effect::PreventsLaunch,
                "RetroArch could not read the game file",
                "RetroArch reported that it could not read the game file. It prints the \
                 same message for a missing file and for one it is not allowed to read.",
            )
            .support(event);
            finding = match path {
                // Looked at, and what is there is not a plain readable file.
                Some((path_item, access)) => finding.support(path_item).limit(format!(
                    "EmuWiz looked at the path and it {}.",
                    access.presence.describe()
                )),
                None => finding
                    .limit("EmuWiz has not looked at the file itself.")
                    .next(NextCheck::CollectEvidence {
                        area: EvidenceArea::Filesystem,
                        why: "Check whether the game file exists and can be read.".into(),
                    }),
            };
            if let Some((exit_item, _)) = exit {
                finding = finding.support(exit_item);
            }
            vec![finding]
        }
    }
}

fn emulator_version_unsupported(view: &View<'_>) -> Vec<Finding> {
    view.observed(EvidenceArea::KnownLimitations)
        .filter_map(|item| match &item.fact {
            // Only an entry of the compiled-in table, evaluated by EmuWiz,
            // counts: a limit that merely claims to be one does not.
            Fact::KnownLimitation {
                id,
                summary,
                violated: true,
            } if item.trust == Trust::Measured && limitations::is_table_entry(id, summary) => Some(
                Finding::cause(
                    Confidence::Established,
                    Effect::PreventsLaunch,
                    "This emulator version is not supported",
                    summary.clone(),
                )
                .support(item)
                .next(NextCheck::UserAction {
                    what: "Update the emulator, then check again.".into(),
                }),
            ),
            _ => None,
        })
        .collect()
}

fn retroarch_vulkan_unavailable(view: &View<'_>) -> Vec<Finding> {
    let Some(setting) = view.setting(RETROARCH, "video_driver") else {
        return Vec::new();
    };
    if setting.value != "vulkan" {
        return Vec::new();
    }
    let Some((exit_item, _)) = view.failed_exit() else {
        return Vec::new();
    };
    let vulkan_events = view.events(LogMeaning::VulkanUnavailable);
    let init_events = view.events(LogMeaning::VideoDriverInitFailed);
    let probe = view.graphics(GraphicsApi::Vulkan, ProbeScope::LaunchEnvironment);
    let host_probe = view.graphics(GraphicsApi::Vulkan, ProbeScope::Host);
    let probe_says_unusable = matches!(probe, Some((_, false)));
    let host_says_unusable = matches!(host_probe, Some((_, false)));
    if vulkan_events.is_empty()
        && init_events.is_empty()
        && !probe_says_unusable
        && !host_says_unusable
    {
        // Set to Vulkan and failed: that alone points at nothing.
        return Vec::new();
    }
    // What RetroArch printed is never more than a possibility on its own:
    // being set to Vulkan is true of every healthy Vulkan setup, so it does
    // not corroborate the message. Only a check of Vulkan itself does.
    let (confidence, explanation) = if probe_says_unusable {
        (
            Confidence::Established,
            "RetroArch is set to use the Vulkan video driver, and a check made in the same \
             environment it was started in found that Vulkan cannot be used there.",
        )
    } else if host_says_unusable {
        (
            Confidence::Probable,
            "RetroArch is set to use the Vulkan video driver, and a check of this computer \
             found that Vulkan cannot be used on it.",
        )
    } else if !vulkan_events.is_empty() {
        (
            Confidence::Possible,
            "RetroArch is set to use the Vulkan video driver and reported that it could \
             not set Vulkan up before it stopped.",
        )
    } else {
        (
            Confidence::Possible,
            "RetroArch is set to use the Vulkan video driver and stopped while starting \
             its video driver. Whether Vulkan itself is the problem is not shown.",
        )
    };
    let mut finding = Finding::cause(
        confidence,
        Effect::PreventsLaunch,
        "Vulkan graphics could not start",
        explanation,
    )
    .support(setting.evidence)
    .support(exit_item)
    .support_all(vulkan_events.iter().map(|(event, _)| *event))
    .support_all(init_events.iter().map(|(event, _)| *event))
    .claim(ClaimSubject::GraphicsApiUsable(GraphicsApi::Vulkan), false);
    match probe {
        Some((probe_item, false)) => finding = finding.support(probe_item),
        Some((probe_item, true)) => finding = finding.contradict(probe_item),
        None => {
            finding = finding
                .limit("Vulkan was not checked independently in the launch environment.")
                .next(NextCheck::ProbeGraphics {
                    api: GraphicsApi::Vulkan,
                    why: "Confirm whether Vulkan can be used where RetroArch runs.".into(),
                });
        }
    }
    match host_probe {
        Some((host_item, false)) => finding = finding.support(host_item),
        Some((_, true)) if !matches!(probe, Some((_, true))) => {
            finding = finding.limit(
                "Vulkan works on this computer outside the launch environment, so something \
                 about how RetroArch is started (for example a sandbox) may be what blocks it.",
            );
        }
        _ => {}
    }
    if !view.verbose_log(RETROARCH) {
        finding = finding
            .limit(
                "RetroArch was not logging in detail, so it did not say why the video \
                 driver failed.",
            )
            .next(NextCheck::VerboseDiagnosticRun {
                why: "Get RetroArch's own account of the video driver failure.".into(),
            });
    }
    finding = view.qualify_setting_finding(finding, RETROARCH);
    match view.graphics(GraphicsApi::OpenGl, ProbeScope::LaunchEnvironment) {
        Some((_, false)) => {
            finding = finding.limit(
                "OpenGL was also found unusable in the launch environment, so changing \
                 the video driver to OpenGL is not expected to help.",
            );
        }
        opengl => {
            finding = finding
                .next(NextCheck::CompareSetting {
                    emulator: RETROARCH.into(),
                    key: "video_driver".into(),
                    alternative: "gl".into(),
                    why: "See whether the game starts with the OpenGL video driver.".into(),
                })
                .repair(
                    "retroarch.video_driver",
                    format!(
                        "Use the OpenGL video driver instead of Vulkan (currently set in: \
                         {}).",
                        setting.layer.label()
                    ),
                );
            if opengl.is_none() {
                finding = finding.limit(
                    "Whether OpenGL can be used in the launch environment has not been \
                     checked.",
                );
            }
        }
    }
    vec![finding]
}

fn retroarch_video_driver_failed(view: &View<'_>) -> Vec<Finding> {
    let init_events = view.events(LogMeaning::VideoDriverInitFailed);
    let (Some((event, _)), Some((exit_item, _))) = (init_events.first(), view.failed_exit()) else {
        return Vec::new();
    };
    let setting = view.setting(RETROARCH, "video_driver");
    if setting
        .as_ref()
        .is_some_and(|setting| setting.value == "vulkan")
    {
        // The Vulkan rule covers this case with more to say.
        return Vec::new();
    }
    let mut finding = Finding::cause(
        Confidence::Possible,
        Effect::PreventsLaunch,
        "The video driver could not start",
        match &setting {
            Some(setting) => format!(
                "RetroArch stopped while starting its video driver, which is set to \
                 \"{}\".",
                setting.value
            ),
            None => "RetroArch stopped while starting its video driver.".to_string(),
        },
    )
    .support(event)
    .support(exit_item)
    .limit("Why the video driver failed is not shown.");
    match setting {
        Some(setting) => {
            finding = finding.support(setting.evidence);
            finding = view.qualify_setting_finding(finding, RETROARCH);
        }
        None => {
            finding = finding.next(NextCheck::CollectEvidence {
                area: EvidenceArea::EmulatorConfiguration,
                why: "Find out which video driver RetroArch is set to use.".into(),
            });
        }
    }
    if !view.verbose_log(RETROARCH) {
        finding = finding.next(NextCheck::VerboseDiagnosticRun {
            why: "Get RetroArch's own account of the video driver failure.".into(),
        });
    }
    vec![finding]
}

fn no_display(view: &View<'_>) -> Vec<Finding> {
    let (Some((x11_item, None)), Some((wayland_item, None))) = (
        view.environment("DISPLAY"),
        view.environment("WAYLAND_DISPLAY"),
    ) else {
        return Vec::new();
    };
    let Some((exit_item, _)) = view.failed_exit() else {
        return Vec::new();
    };
    let mut finding = Finding::cause(
        Confidence::Probable,
        Effect::PreventsLaunch,
        "The emulator had no display to open a window on",
        "Neither DISPLAY nor WAYLAND_DISPLAY was set where the emulator was started, so \
         it had no desktop session to draw on.",
    )
    .in_category(DiagnosisCategory::GraphicsInitialisation)
    .support(x11_item)
    .support(wayland_item)
    .support(exit_item);
    if !view.any_verbose_log() {
        finding = finding.limit("The emulator's own account of the failure was not available.");
    }
    // A video driver that reports itself ready did find a display.
    for (event, _) in view.events(LogMeaning::VideoDriverReady) {
        finding = finding.contradict(event);
    }
    vec![finding]
}

fn shared_library_missing(view: &View<'_>) -> Vec<Finding> {
    let events = view.events(LogMeaning::SharedLibraryMissing);
    let (Some((event, log)), Some((exit_item, exit))) = (events.first(), view.failed_exit()) else {
        return Vec::new();
    };
    if !matches!(exit.outcome, ExitOutcome::Exited { code: 127 }) {
        return Vec::new();
    }
    let library = log.value.as_deref().unwrap_or("a shared library");
    vec![
        Finding::cause(
            Confidence::Probable,
            Effect::PreventsLaunch,
            format!("A library the emulator needs is not installed: {library}"),
            format!(
                "The system's program loader reported that it could not find {library}, \
                 and the launch ended with status 127, which is what the loader returns \
                 in that case."
            ),
        )
        .support(event)
        .support(exit_item)
        .limit("EmuWiz has not checked for the library itself.")
        .next(NextCheck::UserAction {
            what: format!(
                "Install the package that provides {library}, or reinstall the emulator."
            ),
        }),
    ]
}

fn emulator_program_missing(view: &View<'_>) -> Vec<Finding> {
    let Some((exit_item, exit)) = view.exit() else {
        return Vec::new();
    };
    let ExitOutcome::SpawnFailed { reason, .. } = exit.outcome else {
        return Vec::new();
    };
    let mut finding = match reason {
        SpawnFailure::ProgramNotFound => Finding::cause(
            Confidence::Established,
            Effect::PreventsLaunch,
            "The emulator program could not be started: not found",
            "The operating system reported that the emulator program, or something it \
             needs in order to start (such as the interpreter a launcher script names), \
             does not exist. It may have been moved, removed or updated to a different \
             location.",
        ),
        SpawnFailure::PermissionDenied => Finding::cause(
            Confidence::Established,
            Effect::PreventsLaunch,
            "The emulator program is not allowed to run",
            "The operating system refused to run the emulator program: this user account \
             may not execute it.",
        )
        .in_category(DiagnosisCategory::PermissionError),
        SpawnFailure::Other => return Vec::new(),
    }
    .support(exit_item)
    .next(NextCheck::UserAction {
        what: "Open Emulator setup and check the emulator's location.".into(),
    });
    // The path check is cited only when it says the same thing.
    if let Some((path_item, access)) = view.path(PathRole::EmulatorExecutable) {
        match (reason, access.presence) {
            (SpawnFailure::ProgramNotFound, PathPresence::Missing) => {
                finding = finding.support(path_item);
            }
            (SpawnFailure::ProgramNotFound, PathPresence::RegularFile) => {
                finding = finding.limit(
                    "The program file itself exists, so what is missing is something it \
                     needs in order to start.",
                );
            }
            _ => {}
        }
    }
    vec![finding]
}

fn retroarch_core_not_loaded(view: &View<'_>) -> Vec<Finding> {
    let ignored = view.events(LogMeaning::CoreRequestIgnored);
    let core = view.path(PathRole::EmulatorCore);
    let core_missing =
        matches!(core, Some((_, access)) if access.presence == PathPresence::Missing);
    if ignored.is_empty() && !core_missing {
        return Vec::new();
    }
    let clean_exit_note = "RetroArch can run with no core and still exit normally, so a \
                           clean exit here does not mean the game ran.";
    let mut finding = if core_missing {
        Finding::cause(
            Confidence::Established,
            Effect::PreventsLaunch,
            "The emulator core is missing",
            format!(
                "The core file EmuWiz asked RetroArch to use does not exist. {clean_exit_note}"
            ),
        )
    } else {
        Finding::cause(
            Confidence::Possible,
            Effect::PreventsLaunch,
            "RetroArch did not load the requested core",
            format!(
                "RetroArch reported that it ignored the core it was asked to use. \
                 {clean_exit_note}"
            ),
        )
    }
    .support_all(ignored.iter().map(|(event, _)| *event))
    .claim(ClaimSubject::GameLoaded, false);
    match core {
        Some((path_item, access)) if access.presence == PathPresence::Missing => {
            finding = finding.support(path_item);
        }
        Some((path_item, access)) if access.presence == PathPresence::RegularFile => {
            finding = finding.contradict(path_item).limit(
                "A file is present at the core's location now, so RetroArch may be \
                 running somewhere it is not visible.",
            );
        }
        Some((_, access)) => {
            finding = finding.limit(format!(
                "EmuWiz looked at the core's location and it {}.",
                access.presence.describe()
            ));
        }
        None => {
            finding = finding
                .limit("EmuWiz has not looked at the core file itself.")
                .next(NextCheck::CollectEvidence {
                    area: EvidenceArea::Filesystem,
                    why: "Check whether the core file exists.".into(),
                });
        }
    }
    vec![finding.next(NextCheck::UserAction {
        what: "Open Emulator setup and install or reselect the core.".into(),
    })]
}

fn retroarch_core_open_failed(view: &View<'_>) -> Vec<Finding> {
    let events = view.events(LogMeaning::CoreOpenFailed);
    let (Some((event, _)), Some((exit_item, _))) = (events.first(), view.failed_exit()) else {
        return Vec::new();
    };
    let base = |confidence, title: &str, explanation: &str| {
        Finding::cause(confidence, Effect::PreventsLaunch, title, explanation)
            .support(event)
            .support(exit_item)
    };
    let finding = match view.path(PathRole::EmulatorCore) {
        // The missing-core rule reports this.
        Some((_, access)) if access.presence == PathPresence::Missing => return Vec::new(),
        // Established by the look at the file, not by the message.
        Some((path_item, access))
            if access.presence == PathPresence::RegularFile && access.readable == Some(false) =>
        {
            base(
                Confidence::Established,
                "The emulator core cannot be read",
                "The core file exists, but this user account is not allowed to read it.",
            )
            .in_category(DiagnosisCategory::PermissionError)
            .support(path_item)
        }
        // A readable core file is true of every healthy setup: it rules out
        // two other explanations without supporting this one.
        Some((path_item, access))
            if access.presence == PathPresence::RegularFile && access.readable == Some(true) =>
        {
            base(
                Confidence::Possible,
                "The emulator core could not be loaded",
                "RetroArch reported that it could not open the core, and the core file \
                 exists and can be read. It may be damaged, built for a different kind of \
                 computer, or need a library that is not installed.",
            )
            .support(path_item)
            .limit("The system's own reason for refusing the core was not captured.")
        }
        Some((path_item, access)) => base(
            Confidence::Possible,
            "The emulator core could not be opened",
            "RetroArch reported that it could not open the core it was asked to use.",
        )
        .support(path_item)
        .limit(format!(
            "EmuWiz looked at the core's location and it {}.",
            access.presence.describe()
        )),
        None => base(
            Confidence::Possible,
            "The emulator core could not be opened",
            "RetroArch reported that it could not open the core it was asked to use.",
        )
        .limit("EmuWiz has not looked at the core file itself.")
        .next(NextCheck::CollectEvidence {
            area: EvidenceArea::Filesystem,
            why: "Check whether the core file exists and can be read.".into(),
        }),
    };
    vec![finding.next(NextCheck::UserAction {
        what: "Reinstall the core from Emulator setup.".into(),
    })]
}

fn sandbox_refused(view: &View<'_>) -> Vec<Finding> {
    let events = view.events(LogMeaning::SandboxRefused);
    let (Some((event, log)), Some((exit_item, exit))) = (events.first(), view.failed_exit()) else {
        return Vec::new();
    };
    let detail = log.value.as_deref().unwrap_or_default();
    let flatpak = log.signature == "host.flatpak.app_not_installed";
    let (title, explanation) = if flatpak {
        (
            "The Flatpak emulator may not be installed",
            format!("Flatpak reported that the application \"{detail}\" is not installed."),
        )
    } else {
        (
            "The sandbox may not have started",
            format!("The sandbox the emulator runs in reported: \"{detail}\"."),
        )
    };
    // Held at possible: the message is text, and how the emulator is
    // installed does not show that the sandbox refused it.
    let mut finding = Finding::cause(
        Confidence::Possible,
        Effect::PreventsLaunch,
        title,
        explanation,
    )
    .support(event)
    .support(exit_item);
    match view.installation() {
        Some((install_item, installation))
            if flatpak && installation.eq_ignore_ascii_case("flatpak") =>
        {
            finding = finding.support(install_item);
        }
        Some(_) => {}
        None => {
            finding = finding.next(NextCheck::CollectEvidence {
                area: EvidenceArea::EmulatorExecutable,
                why: "Confirm how the emulator is installed.".into(),
            });
        }
    }
    if let Some(caveat) = launcher_caveat(exit) {
        finding = finding.limit(caveat);
    }
    vec![finding]
}

fn save_location_unwritable(view: &View<'_>) -> Vec<Finding> {
    let events = view.events(LogMeaning::SaveWriteFailed);
    let directory = view.path(PathRole::SaveDirectory);
    let unwritable = matches!(directory, Some((_, access)) if access.writable == Some(false));
    if events.is_empty() && !unwritable {
        return Vec::new();
    }
    let mut finding = if unwritable {
        Finding::cause(
            Confidence::Established,
            Effect::Degrades,
            "Saves cannot be written",
            "The folder the emulator saves to is not writable by this user account. The \
             game can run, but progress will not be saved there.",
        )
    } else {
        Finding::cause(
            Confidence::Possible,
            Effect::Degrades,
            "A save could not be written",
            "RetroArch reported that it failed to write save data during this session.",
        )
        .limit("EmuWiz has not checked the save folder itself.")
        .next(NextCheck::CollectEvidence {
            area: EvidenceArea::Filesystem,
            why: "Check whether the save folder is writable.".into(),
        })
    }
    .in_category(DiagnosisCategory::PermissionError)
    .support_all(events.iter().map(|(event, _)| *event));
    if let Some((path_item, access)) = directory {
        finding = if access.writable == Some(false) {
            finding.support(path_item)
        } else if access.writable == Some(true) && !events.is_empty() {
            finding.contradict(path_item)
        } else {
            finding
        };
    }
    vec![finding]
}

fn retroarch_audio_failed(view: &View<'_>) -> Vec<Finding> {
    let failures = view.events(LogMeaning::AudioDriverInitFailed);
    let Some((failure, _)) = failures.first() else {
        return Vec::new();
    };
    // Cores that produce no sound report a sample rate of zero, and
    // RetroArch then prints the same failure line on a healthy run.
    let outputs = view.events(LogMeaning::CoreReportedOutput);
    let Some((output, _)) = outputs.iter().find(|(_, event)| {
        event
            .value
            .as_deref()
            .and_then(|rate| rate.parse::<f64>().ok())
            .is_some_and(|rate| rate.is_finite() && rate > 0.0)
    }) else {
        return Vec::new();
    };
    let setting = view.setting(RETROARCH, "audio_driver");
    if setting
        .as_ref()
        .is_some_and(|setting| setting.value == "null")
    {
        // Sound is switched off on purpose.
        return Vec::new();
    }
    let mut finding = Finding::cause(
        Confidence::Possible,
        Effect::Degrades,
        "Sound could not start",
        "RetroArch reported that its audio driver failed to start and carried on without \
         sound, for a core that does produce sound.",
    )
    .support(failure)
    .support(output)
    .limit("Why the audio driver failed is not shown.")
    .next(NextCheck::UserAction {
        what: "Check that sound works in other applications, then check the audio driver \
               chosen in RetroArch's settings."
            .into(),
    });
    if let Some(setting) = setting {
        finding = finding.support(setting.evidence);
        finding = view.qualify_setting_finding(finding, RETROARCH);
    }
    vec![finding]
}

fn controller_unbound(view: &View<'_>) -> Vec<Finding> {
    view.observed(EvidenceArea::EmulatorConfiguration)
        .filter_map(|item| match &item.fact {
            Fact::ControllerBinding {
                emulator,
                player: 1,
                bound: false,
            } => Some(
                Finding::cause(
                    Confidence::Established,
                    Effect::Degrades,
                    "No controller is set up for player 1",
                    format!(
                        "The settings of {emulator} do not assign any controller to \
                         player 1. The game can start, but will not respond to a controller."
                    ),
                )
                .support(item)
                .next(NextCheck::UserAction {
                    what: "Assign a controller in the emulator's own controller settings.".into(),
                }),
            ),
            _ => None,
        })
        .collect()
}

fn modification_conflict(view: &View<'_>) -> Vec<Finding> {
    view.observed(EvidenceArea::Modifications)
        .filter_map(|item| match &item.fact {
            Fact::ModificationConflict {
                target,
                first,
                second,
            } => Some(
                Finding::cause(
                    Confidence::Possible,
                    Effect::Unknown,
                    "Two active modifications change the same thing",
                    format!(
                        "{first} and {second} both change {target}. They may not work \
                         together."
                    ),
                )
                .support(item)
                .limit(
                    "That the two overlap is established; that the overlap causes this \
                     problem is not.",
                )
                .next(NextCheck::UserAction {
                    what: "Try the game with one of the two turned off.".into(),
                }),
            ),
            _ => None,
        })
        .collect()
}

fn readiness_blocked(view: &View<'_>) -> Vec<Finding> {
    view.observed(EvidenceArea::LaunchReadiness)
        .filter_map(|item| match &item.fact {
            Fact::ReadinessBlocker { family, detail, .. } => {
                let category = match family {
                    BlockerFamily::Firmware => DiagnosisCategory::MissingFirmware,
                    BlockerFamily::Media | BlockerFamily::Unsupported => {
                        DiagnosisCategory::InvalidOrUnsupportedMedia
                    }
                    BlockerFamily::Emulator | BlockerFamily::Dependency => {
                        DiagnosisCategory::MissingDependency
                    }
                    BlockerFamily::Controller => DiagnosisCategory::ControllerConfiguration,
                    BlockerFamily::Modification => DiagnosisCategory::ConflictingModifications,
                    BlockerFamily::Identity
                    | BlockerFamily::Configuration
                    | BlockerFamily::Other => DiagnosisCategory::LaunchBlocked,
                };
                Some(
                    Finding::cause(
                        Confidence::Established,
                        Effect::PreventsLaunch,
                        "EmuWiz stopped this launch before starting the emulator",
                        detail.clone(),
                    )
                    .in_category(category)
                    .support(item),
                )
            }
            _ => None,
        })
        .collect()
}

/// Errors a program states outright, as symptoms. Audio is absent on
/// purpose: RetroArch prints its audio failure on healthy runs of cores that
/// make no sound, so on its own that line reports nothing.
const REPORTED_ERRORS: &[(LogMeaning, DiagnosisCategory, Effect, &str)] = &[
    (
        LogMeaning::VulkanUnavailable,
        DiagnosisCategory::GraphicsInitialisation,
        Effect::PreventsLaunch,
        "it could not set up Vulkan",
    ),
    (
        LogMeaning::VideoDriverInitFailed,
        DiagnosisCategory::GraphicsInitialisation,
        Effect::PreventsLaunch,
        "it could not start its video driver",
    ),
    (
        LogMeaning::CoreRequestIgnored,
        DiagnosisCategory::MissingDependency,
        Effect::PreventsLaunch,
        "it ignored the emulator core it was asked to use",
    ),
    (
        LogMeaning::CoreOpenFailed,
        DiagnosisCategory::MissingDependency,
        Effect::PreventsLaunch,
        "it could not open the emulator core",
    ),
    (
        LogMeaning::ContentReadFailed,
        DiagnosisCategory::InvalidOrUnsupportedMedia,
        Effect::PreventsLaunch,
        "it could not read the game file",
    ),
    (
        LogMeaning::ContentRejected,
        DiagnosisCategory::InvalidOrUnsupportedMedia,
        Effect::PreventsLaunch,
        "its core refused the game file",
    ),
    (
        LogMeaning::SaveWriteFailed,
        DiagnosisCategory::PermissionError,
        Effect::Degrades,
        "it could not write save data",
    ),
    (
        LogMeaning::SharedLibraryMissing,
        DiagnosisCategory::MissingDependency,
        Effect::PreventsLaunch,
        "a library the program needs could not be found",
    ),
    (
        LogMeaning::SandboxRefused,
        DiagnosisCategory::SandboxFailure,
        Effect::PreventsLaunch,
        "the sandbox or application runtime refused to start the program",
    ),
    (
        LogMeaning::FatalErrorUnspecific,
        DiagnosisCategory::UnexplainedExit,
        Effect::Unknown,
        "it hit a fatal error, without saying what caused it",
    ),
];

fn reported_errors(view: &View<'_>) -> Vec<Finding> {
    REPORTED_ERRORS
        .iter()
        .flat_map(|(meaning, category, effect, what)| {
            // One report per program that said it, so a host message and an
            // emulator message of the same kind are not merged.
            let events = view.events(*meaning);
            let mut speakers: Vec<&str> = events
                .iter()
                .map(|(_, event)| event.emulator.as_str())
                .collect();
            speakers.sort_unstable();
            speakers.dedup();
            speakers
                .into_iter()
                .filter_map(|speaker| {
                    let said: Vec<&(&Evidence, &LogEvent)> = events
                        .iter()
                        .filter(|(_, event)| event.emulator == speaker)
                        .collect();
                    let (_, first) = said.first()?;
                    let name = match speaker {
                        RETROARCH => "RetroArch",
                        HOST_EMULATOR => "The system",
                        _ => "The emulator",
                    };
                    let times = match said.len() {
                        1 => String::new(),
                        count => format!(" This kind of message was printed {count} times."),
                    };
                    Some(
                        Finding::reported(
                            format!("{name} reported that {what}"),
                            format!("The output contains: \"{}\".{times}", first.excerpt),
                        )
                        .in_category(*category)
                        .with_effect(*effect)
                        .support_all(said.iter().map(|(item, _)| *item)),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn process_symptoms(view: &View<'_>) -> Vec<Finding> {
    let Some((exit_item, exit)) = view.exit() else {
        return Vec::new();
    };
    let after = seconds(exit);
    let mut findings = Vec::new();
    match exit.outcome {
        ExitOutcome::Signaled {
            signal,
            core_dumped,
        } => {
            let info = signal_info(signal);
            let name = info.map_or_else(|| format!("signal {signal}"), |info| info.name.into());
            let dumped = if core_dumped {
                " A core dump was written."
            } else {
                ""
            };
            match info.map(|info| info.kind) {
                Some(SignalKind::StopRequest) => {}
                Some(SignalKind::Fault) => findings.push(
                    Finding::symptom(
                        Confidence::Established,
                        format!("The emulator crashed ({name})"),
                        format!(
                            "The operating system ended the emulator process with {name}\
                             {after}: the program itself faulted.{dumped}"
                        ),
                    )
                    .in_category(DiagnosisCategory::Crash),
                ),
                Some(SignalKind::ForcedKill) => findings.push(Finding::symptom(
                    Confidence::Established,
                    format!("The emulator was forcibly stopped ({name})"),
                    format!(
                        "The emulator process was ended with {name}{after}. The system \
                         does this when memory runs out, and a person can do it too; the \
                         signal does not say which."
                    ),
                )),
                Some(SignalKind::Other) | None => findings.push(Finding::symptom(
                    Confidence::Established,
                    format!("The emulator was ended by {name}"),
                    format!("The emulator process was ended by {name}{after}.{dumped}"),
                )),
            }
        }
        // A status that only relays a request to stop is not a failure and
        // is reported as a note, not here.
        ExitOutcome::Exited { code } if exit.is_failure() => {
            findings.push(Finding::symptom(
                Confidence::Established,
                format!("The emulator exited with status {code}"),
                format!("The emulator process ended with exit status {code}{after}."),
            ));
            let relayed = possibly_relayed_signal(code)
                .and_then(|signal| signal_info(signal).map(|info| (signal, info)));
            match relayed {
                Some((_, info)) if info.kind == SignalKind::Fault => findings.push(
                    Finding::symptom(
                        Confidence::Possible,
                        format!("The emulator may have crashed ({})", info.name),
                        format!(
                            "Exit status {code} is what a launcher reports when the program \
                             it started was ended by {}. A program can also choose this \
                             status itself, so this is not certain.",
                            info.name
                        ),
                    )
                    .in_category(DiagnosisCategory::Crash),
                ),
                Some((_, info)) if info.kind == SignalKind::ForcedKill => {
                    findings.push(Finding::symptom(
                        Confidence::Possible,
                        format!(
                            "The emulator may have been forcibly stopped ({})",
                            info.name
                        ),
                        format!(
                            "Exit status {code} is what a launcher reports when the program \
                             it started was ended by {}. The system does that when memory \
                             runs out, and a person can do it too. A program can also choose \
                             this status itself, so this is not certain.",
                            info.name
                        ),
                    ));
                }
                _ => {}
            }
        }
        ExitOutcome::SpawnFailed {
            reason: SpawnFailure::Other,
            os_error,
        } => findings.push(Finding::symptom(
            Confidence::Established,
            "The emulator could not be started",
            match os_error {
                Some(code) => format!(
                    "The operating system refused to start the emulator program (error {code})."
                ),
                None => "The operating system refused to start the emulator program.".into(),
            },
        )),
        _ => {}
    }
    findings
        .into_iter()
        .map(|mut finding| {
            finding = finding.support(exit_item);
            if let Some(caveat) = launcher_caveat(exit) {
                finding = finding.limit(caveat);
            }
            finding
        })
        .collect()
}

/// The built-in rules, in identifier order.
pub(crate) const RULES: &[Rule] = &[
    Rule {
        id: "controller.player_one_unbound",
        version: 1,
        category: DiagnosisCategory::ControllerConfiguration,
        summary: "The emulator's settings bind no controller to player 1.",
        evaluate: controller_unbound,
    },
    Rule {
        id: "dependency.emulator_program_unavailable",
        version: 1,
        category: DiagnosisCategory::MissingDependency,
        summary: "The operating system could not find, or refused to run, the emulator \
                  program.",
        evaluate: emulator_program_missing,
    },
    Rule {
        id: "dependency.shared_library_missing",
        version: 1,
        category: DiagnosisCategory::MissingDependency,
        summary: "The program loader reported a missing shared library and the launch \
                  ended with status 127.",
        evaluate: shared_library_missing,
    },
    Rule {
        id: "emulator.reported_error",
        version: 1,
        category: DiagnosisCategory::UnexplainedExit,
        summary: "Restates an error the emulator, the program loader or the sandbox printed. \
                  Established as a report; a symptom, not a cause.",
        evaluate: reported_errors,
    },
    Rule {
        id: "emulator.version_unsupported",
        version: 1,
        category: DiagnosisCategory::UnsupportedEmulatorVersion,
        summary: "The installed emulator version is below a minimum in the built-in \
                  compatibility table.",
        evaluate: emulator_version_unsupported,
    },
    Rule {
        id: "environment.no_display",
        version: 1,
        category: DiagnosisCategory::GraphicsInitialisation,
        summary: "The launch failed and neither DISPLAY nor WAYLAND_DISPLAY was set in \
                  the launch environment.",
        evaluate: no_display,
    },
    Rule {
        id: "firmware.required_missing",
        version: 1,
        category: DiagnosisCategory::MissingFirmware,
        summary: "The launch check found required firmware missing.",
        evaluate: firmware_missing,
    },
    Rule {
        id: "media.game_file_missing",
        version: 1,
        category: DiagnosisCategory::InvalidOrUnsupportedMedia,
        summary: "Nothing exists at the game file's location.",
        evaluate: game_file_missing,
    },
    Rule {
        id: "media.integrity_failed",
        version: 1,
        category: DiagnosisCategory::InvalidOrUnsupportedMedia,
        summary: "An integrity check of the game file failed.",
        evaluate: media_integrity_failed,
    },
    Rule {
        id: "modifications.conflict",
        version: 1,
        category: DiagnosisCategory::ConflictingModifications,
        summary: "Two active modifications managed by EmuWiz change the same target.",
        evaluate: modification_conflict,
    },
    Rule {
        id: "permission.game_file_unreadable",
        version: 1,
        category: DiagnosisCategory::PermissionError,
        summary: "The game file exists and this user may not read it.",
        evaluate: game_file_unreadable,
    },
    Rule {
        id: "permission.save_location_unwritable",
        version: 1,
        category: DiagnosisCategory::PermissionError,
        summary: "The save folder is not writable, or the emulator reported a failed save.",
        evaluate: save_location_unwritable,
    },
    Rule {
        id: "process.exit",
        version: 1,
        category: DiagnosisCategory::UnexplainedExit,
        summary: "Reports how the emulator process ended: a crash signal, a forced stop \
                  or a non-zero exit status. A symptom, not a cause.",
        evaluate: process_symptoms,
    },
    Rule {
        id: "readiness.blocked",
        version: 1,
        category: DiagnosisCategory::LaunchBlocked,
        summary: "The launch planner refused to start the game and gave a reason.",
        evaluate: readiness_blocked,
    },
    Rule {
        id: "retroarch.audio.driver_failed",
        version: 1,
        category: DiagnosisCategory::AudioDevice,
        summary: "RetroArch reported an audio driver failure for a core that produces \
                  sound, with sound not switched off.",
        evaluate: retroarch_audio_failed,
    },
    Rule {
        id: "retroarch.content.read_failed",
        version: 1,
        category: DiagnosisCategory::InvalidOrUnsupportedMedia,
        summary: "RetroArch reported it could not read the game file and the file is \
                  neither known missing nor known unreadable.",
        evaluate: retroarch_content_read_failed,
    },
    Rule {
        id: "retroarch.content.rejected",
        version: 1,
        category: DiagnosisCategory::InvalidOrUnsupportedMedia,
        summary: "RetroArch's core read the game file and refused to load it.",
        evaluate: retroarch_content_rejected,
    },
    Rule {
        id: "retroarch.core.not_loaded",
        version: 1,
        category: DiagnosisCategory::MissingDependency,
        summary: "The core file is missing, or RetroArch reported ignoring the requested \
                  core.",
        evaluate: retroarch_core_not_loaded,
    },
    Rule {
        id: "retroarch.core.open_failed",
        version: 1,
        category: DiagnosisCategory::MissingDependency,
        summary: "RetroArch reported it could not open the core file, which is not known to \
                  be missing.",
        evaluate: retroarch_core_open_failed,
    },
    Rule {
        id: "retroarch.video.driver_failed",
        version: 1,
        category: DiagnosisCategory::GraphicsInitialisation,
        summary: "RetroArch stopped while starting its video driver, which is not known to \
                  be set to Vulkan.",
        evaluate: retroarch_video_driver_failed,
    },
    Rule {
        id: "retroarch.video.vulkan_unavailable",
        version: 1,
        category: DiagnosisCategory::GraphicsInitialisation,
        summary: "RetroArch is set to Vulkan and failed, with a Vulkan probe, a Vulkan \
                  error or a video driver failure to show for it. Established only by a \
                  probe of the launch environment.",
        evaluate: retroarch_vulkan_unavailable,
    },
    Rule {
        id: "sandbox.refused",
        version: 1,
        category: DiagnosisCategory::SandboxFailure,
        summary: "Flatpak or bubblewrap refused to start the emulator.",
        evaluate: sandbox_refused,
    },
];

fn rule_set_version(rules: &[Rule]) -> RuleSetVersion {
    let mut hasher = Sha256::new();
    let mut feed = |text: String| {
        hasher.update(text.as_bytes());
        hasher.update([0x1e]);
    };
    feed(format!("engine\u{1f}{ENGINE_VERSION}"));
    for rule in rules {
        feed(format!(
            "rule\u{1f}{}\u{1f}{}\u{1f}{:?}",
            rule.id, rule.version, rule.category
        ));
    }
    for signature in HOST_SIGNATURES.iter().chain(retroarch::SIGNATURES) {
        feed(format!("signature\u{1f}{}", signature.digest_text()));
    }
    feed(format!("limitations\u{1f}{}", limitations::digest_text()));
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    RuleSetVersion {
        engine_version: ENGINE_VERSION,
        digest: hex,
    }
}

/// The built-in rule set: its version and every rule in plain words.
pub fn rule_set() -> RuleSetInfo {
    RuleSetInfo {
        version: rule_set_version(RULES),
        rules: RULES
            .iter()
            .map(|rule| RuleInfo {
                id: rule.id,
                version: rule.version,
                category: rule.category,
                summary: rule.summary,
            })
            .collect(),
    }
}

/// Diagnose with the built-in rules.
pub fn diagnose(evidence: &EvidenceSet) -> DiagnosisReport {
    diagnose_with(RULES, evidence)
}

/// Whether an evidence item can carry a cause on its own: anything except
/// an ordinary process exit and an unspecific error message.
fn is_cause_specific(item: &Evidence) -> bool {
    match &item.fact {
        Fact::ProcessExit(exit) => matches!(exit.outcome, ExitOutcome::SpawnFailed { .. }),
        Fact::LogEvent(event) => event.meaning.is_specific(),
        _ => true,
    }
}

/// Apply the engine's limits to one finding. `Err` is the reason it was
/// refused.
fn finalise(rule: &Rule, finding: Finding, evidence: &EvidenceSet) -> Result<Diagnosis, String> {
    let mut limitations = finding.limitations;
    let mut supporting = Vec::new();
    let mut unusable = 0usize;
    let mut asserted = 0usize;
    for id in finding.supporting {
        match evidence.get(&id) {
            Some(item) if !item.is_observed() => unusable += 1,
            Some(item) if item.trust == Trust::Asserted => asserted += 1,
            Some(_) => supporting.push(id),
            None => unusable += 1,
        }
    }
    if supporting.is_empty() {
        return Err("no observed, current evidence supports it".into());
    }
    if unusable > 0 {
        limitations.push(format!(
            "{unusable} cited observation(s) were out of date or unavailable and were not \
             counted."
        ));
    }
    if asserted > 0 {
        limitations.push(format!(
            "{asserted} cited statement(s) were only asserted, not observed, and were not \
             counted."
        ));
    }
    let items: Vec<&Evidence> = supporting
        .iter()
        .filter_map(|id| evidence.get(id))
        .collect();
    if finding.explains_cause && !items.iter().any(|item| is_cause_specific(item)) {
        return Err(
            "it names a cause from an exit status and an unspecific error message alone".into(),
        );
    }
    if finding.reported {
        // A report is a statement about the emulator's output and nothing
        // else, so it may cite only that output and may not name a cause.
        if finding.explains_cause
            || !items
                .iter()
                .all(|item| matches!(item.fact, Fact::LogEvent(_)))
        {
            return Err("it presents something other than emulator output as a report".into());
        }
        limitations.push(
            "This is what the emulator printed. That it reported this is established; \
             that it is the underlying cause is not."
                .into(),
        );
        return Ok(Diagnosis {
            rule: rule.id.to_string(),
            rule_version: rule.version,
            category: finding.category.unwrap_or(rule.category),
            confidence: Confidence::Established,
            explains_cause: false,
            reported: true,
            effect: finding.effect,
            title: finding.title,
            explanation: finding.explanation,
            supporting,
            contradicting: Vec::new(),
            limitations,
            next_checks: finding.next_checks,
            repairs: Vec::new(),
            claims: Vec::new(),
        });
    }
    let mut confidence = finding.confidence;
    let has_untrusted_text = items.iter().any(|item| item.trust == Trust::UntrustedText);
    let has_firm = items.iter().any(|item| item.trust.is_firm());
    let has_firm_beyond_exit = items.iter().any(|item| {
        item.trust.is_firm()
            && !matches!(&item.fact, Fact::ProcessExit(exit)
                if !matches!(exit.outcome, ExitOutcome::SpawnFailed { .. }))
    });
    if !has_firm {
        confidence = confidence.at_most(Confidence::Possible);
        limitations.push(
            "It rests only on text the emulator printed, which a game or a damaged file \
             could also have produced."
                .into(),
        );
    } else if has_untrusted_text
        && !has_firm_beyond_exit
        && confidence.rank() > Confidence::Possible.rank()
    {
        confidence = Confidence::Possible;
        limitations.push(
            "Apart from the fact that the launch failed, it rests on text the emulator \
             printed; nothing else confirms it."
                .into(),
        );
    }
    let contradicting: Vec<EvidenceId> = finding
        .contradicting
        .into_iter()
        .filter(|id| {
            evidence
                .get(id)
                .is_some_and(|item| item.is_observed() && item.trust != Trust::Asserted)
        })
        .collect();
    if !contradicting.is_empty() {
        confidence = confidence.at_most(Confidence::Possible);
        limitations.push("Some of the evidence points the other way.".into());
    }
    Ok(Diagnosis {
        rule: rule.id.to_string(),
        rule_version: rule.version,
        category: finding.category.unwrap_or(rule.category),
        confidence,
        explains_cause: finding.explains_cause,
        reported: false,
        effect: finding.effect,
        title: finding.title,
        explanation: finding.explanation,
        supporting,
        contradicting,
        limitations,
        next_checks: finding.next_checks,
        repairs: finding.repairs,
        claims: finding.claims,
    })
}

/// Diagnose with a given rule slice. The built-in rules are the only ones
/// used outside tests; the parameter exists so the engine's limits can be
/// tested against rules that try to break them.
pub(crate) fn diagnose_with(rules: &[Rule], evidence: &EvidenceSet) -> DiagnosisReport {
    let view = View::new(evidence);
    let mut diagnoses = Vec::new();
    let mut rejected = Vec::new();
    for rule in rules {
        for finding in (rule.evaluate)(&view) {
            match finalise(rule, finding, evidence) {
                Ok(diagnosis) => diagnoses.push(diagnosis),
                Err(reason) => rejected.push(RejectedFinding {
                    rule: rule.id.to_string(),
                    reason,
                }),
            }
        }
    }

    // Findings that make opposite claims about the same thing cannot both
    // be right: record the conflict and hold both at `Possible`.
    let mut conflicts: Vec<Conflict> = Vec::new();
    let mut conflicted: BTreeSet<usize> = BTreeSet::new();
    for (first_index, first) in diagnoses.iter().enumerate() {
        for (offset, second) in diagnoses[first_index + 1..].iter().enumerate() {
            let second_index = first_index + 1 + offset;
            for claim in &first.claims {
                let opposed = second
                    .claims
                    .iter()
                    .any(|other| other.subject == claim.subject && other.holds != claim.holds);
                if !opposed {
                    continue;
                }
                conflicted.insert(first_index);
                conflicted.insert(second_index);
                let mut pair = vec![first.rule.clone(), second.rule.clone()];
                pair.sort();
                let conflict = Conflict {
                    subject: claim.subject,
                    rules: pair,
                };
                if !conflicts.contains(&conflict) {
                    conflicts.push(conflict);
                }
            }
        }
    }
    for index in conflicted {
        let diagnosis = &mut diagnoses[index];
        diagnosis.confidence = diagnosis.confidence.at_most(Confidence::Possible);
        diagnosis
            .limitations
            .push("Another finding contradicts this one.".into());
    }

    let (mut causes, mut symptoms): (Vec<Diagnosis>, Vec<Diagnosis>) = diagnoses
        .into_iter()
        .partition(|diagnosis| diagnosis.explains_cause);
    let order = |diagnosis: &Diagnosis| {
        (
            std::cmp::Reverse(diagnosis.confidence.rank()),
            diagnosis.rule.clone(),
            diagnosis.title.clone(),
        )
    };
    causes.sort_by_key(order);
    symptoms.sort_by_key(order);

    // Several process exits mean the evidence mixes several runs. Which
    // log line belongs to which run cannot be known, so no rule reads any
    // of them (see `View::exit`) and the report cannot call the picture
    // clean. Asserted exits are not evidence and are not counted.
    let exits: Vec<&ProcessExit> = view
        .observed(EvidenceArea::ProcessExit)
        .filter_map(|item| match &item.fact {
            Fact::ProcessExit(exit) => Some(exit),
            _ => None,
        })
        .collect();
    let mixed_runs = exits.len() > 1;
    // A failure is something that stopped the launch: the process failing,
    // or the emulator saying so. Once there is one, only a cause that can
    // prevent a launch explains it; a cause that merely degrades the game
    // (no sound, saves not written) is still reported but does not.
    let failure_observed = symptoms
        .iter()
        .any(|symptom| symptom.effect != Effect::Degrades);
    let explains = |cause: &&Diagnosis| !failure_observed || cause.effect != Effect::Degrades;
    let has_contradicted_cause = causes.iter().any(|cause| !cause.contradicting.is_empty());
    let verdict = if !conflicts.is_empty() || has_contradicted_cause {
        Verdict::Conflicting
    } else if causes
        .iter()
        .filter(explains)
        .any(|cause| cause.confidence == Confidence::Established)
    {
        Verdict::Explained
    } else if causes.iter().any(|cause| explains(&cause)) {
        Verdict::PartlyExplained
    } else if failure_observed || mixed_runs {
        Verdict::Unknown
    } else {
        Verdict::NoProblemObserved
    };

    let mut notes = Vec::new();
    for exit in &exits {
        if let ExitOutcome::Exited { code } = exit.outcome
            && !exit.is_failure()
            && code != 0
        {
            notes.push(format!(
                "Exit status {code} is what a launcher reports after the program was asked \
                 to stop. It is treated as a stop request, not as a failure."
            ));
        }
    }
    if evidence.items().iter().any(|item| {
        item.is_observed()
            && item.trust == Trust::Reported
            && matches!(
                item.area,
                EvidenceArea::ProcessExit | EvidenceArea::LaunchEnvironment
            )
    }) {
        notes.push(
            "How the process ended, or the environment it ran in, was supplied by whoever \
             asked for this diagnosis. EmuWiz did not observe it."
                .to_string(),
        );
    }
    if mixed_runs {
        notes.push(
            "The evidence holds more than one process exit, so it mixes several runs. None \
             of the exits was interpreted."
                .to_string(),
        );
    }
    if evidence.is_truncated() {
        notes.push(
            "More evidence was offered than a report can hold; the rest was not examined."
                .to_string(),
        );
    }
    let stale = evidence
        .items()
        .iter()
        .filter(|item| matches!(item.state, EvidenceState::Stale { .. }))
        .count();
    if stale > 0 {
        notes.push(format!(
            "{stale} observation(s) were out of date because their source changed, and \
             were not used."
        ));
    }
    let log_cut = evidence.items().iter().any(|item| {
        item.is_observed()
            && matches!(&item.fact, Fact::LogCoverage(coverage) if coverage.truncated)
    });
    if log_cut {
        notes.push(
            "Part of the emulator's output was not examined: it was longer than the \
             limit, or only its beginning was captured."
                .to_string(),
        );
    }
    if verdict == Verdict::NoProblemObserved {
        notes.push(
            "No problem was found in the evidence. That is not a statement that the game \
             works: nothing here shows a game loading or being played."
                .to_string(),
        );
    }

    DiagnosisReport {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        rule_set: rule_set_version(rules),
        subject: evidence.subject.clone(),
        verdict,
        causes,
        symptoms,
        conflicts,
        rejected,
        coverage: evidence.coverage(),
        notes,
        evidence: evidence.clone(),
    }
}

#[cfg(test)]
#[path = "rules/tests.rs"]
mod tests;
