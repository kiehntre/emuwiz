//! Conservative local MAME cheat XML support.
//!
//! MAME cheat XML is a native debugger-expression format, not a generic
//! memory-cheat interchange format.  This module therefore preserves native
//! expressions and only recognises the deliberately narrow `address = value`
//! form.  It never evaluates an expression.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::shared_preview::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedPreviewReport, SharedPreviewRequest,
    build_shared_preview,
};
use super::shared_transaction::{SharedTransactionPlan, build_shared_transaction_plan};

pub const MAME_CHEAT_MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
pub const MAME_CHEAT_MAX_DEPTH: usize = 32;
pub const MAME_CHEAT_MAX_ENTRIES: usize = 2048;
pub const MAME_CHEAT_MAX_PARAMETERS: usize = 128;
pub const MAME_CHEAT_MAX_SCRIPTS: usize = 256;
pub const MAME_CHEAT_MAX_ACTIONS: usize = 4096;
pub const MAME_CHEAT_MAX_TEXT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MameCheatProvenance {
    LocalImport,
    UserCreated,
    ExistingMameFile,
    UnknownExternal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MameScriptState {
    On,
    Off,
    Run,
    Change,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameNativeExpression {
    pub original: String,
    pub context: String,
    pub opaque: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameCheatAction {
    pub condition: Option<String>,
    pub expression: MameNativeExpression,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameCheatOutput {
    pub condition: Option<String>,
    pub format: Option<String>,
    pub line: Option<String>,
    pub align: Option<String>,
    pub arguments: Vec<MameNativeExpression>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameCheatScript {
    pub state: MameScriptState,
    pub actions: Vec<MameCheatAction>,
    pub outputs: Vec<MameCheatOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameCheatParameterItem {
    pub value: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameCheatParameter {
    pub min: Option<String>,
    pub max: Option<String>,
    pub step: Option<String>,
    pub items: Vec<MameCheatParameterItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameCheatEntry {
    pub description: String,
    pub comment: Option<String>,
    pub parameters: Vec<MameCheatParameter>,
    pub scripts: Vec<MameCheatScript>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameCheatFile {
    pub machine: String,
    pub version: u32,
    pub mame_version: Option<String>,
    pub cheats: Vec<MameCheatEntry>,
    pub provenance: MameCheatProvenance,
    pub source_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MameCheatParseIssue {
    Empty,
    TooLarge,
    MalformedXml(String),
    DoctypeOrExternalEntity,
    UnsupportedVersion(u32),
    ExcessiveDepth,
    TooManyCheats,
    TooManyParameters,
    TooManyScripts,
    TooManyActions,
    TextTooLong,
    MissingDescription,
    InvalidMachine(String),
    InvalidScriptState(String),
    UnexpectedElement(String),
}

impl fmt::Display for MameCheatParseIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MameCheatReadiness {
    ReadyNative,
    ReadyWithOpaqueNativeOps,
    PreviewOnly,
    WrongMachine,
    UnsupportedExpression,
    Malformed,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MameCheatRuntimeState {
    DefinitionInstalled,
    RuntimeEnableRequired,
    PersistentStateSupported,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameNormalizedWrite {
    pub address: u64,
    pub value: u64,
    pub width_bits: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameCheatInspection {
    pub readiness: MameCheatReadiness,
    pub machine: String,
    pub normalized_writes: Vec<MameNormalizedWrite>,
    pub opaque_expression_count: usize,
    pub runtime_state: MameCheatRuntimeState,
    pub issues: Vec<MameCheatParseIssue>,
}

#[derive(Debug, Clone)]
pub struct MameCheatPreviewRequest {
    pub machine: String,
    pub source_file: std::path::PathBuf,
    pub destination_root: std::path::PathBuf,
    /// A caller-owned staging directory.  It is never the MAME cheat root.
    pub staging_root: std::path::PathBuf,
    pub profile_id: String,
    pub mame_version: Option<String>,
}

#[derive(Debug)]
pub struct MameCheatPreview {
    pub file: MameCheatFile,
    pub inspection: MameCheatInspection,
    pub report: SharedPreviewReport,
    pub transaction_plan: SharedTransactionPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MameCheatPreviewError {
    Parse(MameCheatParseError),
    Io(String),
    WrongMachine,
    Shared(String),
}

impl fmt::Display for MameCheatPreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MameCheatPreviewError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MameCheatParseError(pub MameCheatParseIssue);

impl fmt::Display for MameCheatParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for MameCheatParseError {}

fn valid_machine(machine: &str) -> bool {
    !machine.is_empty()
        && machine.len() <= 64
        && machine
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn attr(start: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Option<String> {
    start
        .attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name)
        .and_then(|a| String::from_utf8(a.value.into_owned()).ok())
}

fn text(event: quick_xml::events::BytesText<'_>) -> Result<String, MameCheatParseError> {
    let raw = String::from_utf8_lossy(event.as_ref());
    let value = raw
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    if value.len() > MAME_CHEAT_MAX_TEXT_BYTES {
        return Err(MameCheatParseError(MameCheatParseIssue::TextTooLong));
    }
    Ok(value)
}

fn script_state(value: &str) -> Result<MameScriptState, MameCheatParseError> {
    match value {
        "on" => Ok(MameScriptState::On),
        "off" => Ok(MameScriptState::Off),
        "run" => Ok(MameScriptState::Run),
        value if value.starts_with("change") => Ok(MameScriptState::Change),
        other => Err(MameCheatParseError(
            MameCheatParseIssue::InvalidScriptState(other.into()),
        )),
    }
}

/// Parse a MAME XML file for a caller-supplied machine shortname.  MAME uses
/// the filename selected by `-cheatpath` for machine routing; the XML itself
/// does not provide a trustworthy target attribute.
pub fn parse_mame_cheat_xml(
    bytes: &[u8],
    machine: &str,
    provenance: MameCheatProvenance,
) -> Result<MameCheatFile, MameCheatParseError> {
    if bytes.is_empty() {
        return Err(MameCheatParseError(MameCheatParseIssue::Empty));
    }
    if bytes.len() > MAME_CHEAT_MAX_FILE_BYTES {
        return Err(MameCheatParseError(MameCheatParseIssue::TooLarge));
    }
    if !valid_machine(machine) {
        return Err(MameCheatParseError(MameCheatParseIssue::InvalidMachine(
            machine.into(),
        )));
    }
    let upper = String::from_utf8_lossy(bytes).to_ascii_uppercase();
    if upper.contains("<!DOCTYPE")
        || upper.contains("<!ENTITY")
        || upper.contains("SYSTEM ")
        || upper.contains("PUBLIC ")
    {
        return Err(MameCheatParseError(
            MameCheatParseIssue::DoctypeOrExternalEntity,
        ));
    }
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut file_version = None;
    let mut cheats = Vec::new();
    let mut cheat: Option<MameCheatEntry> = None;
    let mut parameter: Option<MameCheatParameter> = None;
    let mut script: Option<MameCheatScript> = None;
    let mut output: Option<MameCheatOutput> = None;
    let mut current_condition = None;
    let mut current_attrs: Option<(String, String)> = None;
    let mut text_target: Option<String> = None;
    let mut action_count = 0usize;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(start)) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                if stack.len() >= MAME_CHEAT_MAX_DEPTH {
                    return Err(MameCheatParseError(MameCheatParseIssue::ExcessiveDepth));
                }
                if name == "mamecheat" {
                    file_version = attr(&start, b"version").and_then(|v| v.parse().ok());
                } else if name == "cheat" {
                    if cheats.len() >= MAME_CHEAT_MAX_ENTRIES {
                        return Err(MameCheatParseError(MameCheatParseIssue::TooManyCheats));
                    }
                    cheat = Some(MameCheatEntry {
                        description: attr(&start, b"desc").unwrap_or_default(),
                        comment: None,
                        parameters: Vec::new(),
                        scripts: Vec::new(),
                    });
                } else if name == "parameter" {
                    if parameter.is_some() {
                        return Err(MameCheatParseError(MameCheatParseIssue::UnexpectedElement(
                            name,
                        )));
                    }
                    parameter = Some(MameCheatParameter {
                        min: attr(&start, b"min"),
                        max: attr(&start, b"max"),
                        step: attr(&start, b"step"),
                        items: Vec::new(),
                    });
                } else if name == "script" {
                    script = Some(MameCheatScript {
                        state: script_state(
                            &attr(&start, b"state").unwrap_or_else(|| "on".into()),
                        )?,
                        actions: Vec::new(),
                        outputs: Vec::new(),
                    });
                } else if name == "action" {
                    action_count += 1;
                    if action_count > MAME_CHEAT_MAX_ACTIONS {
                        return Err(MameCheatParseError(MameCheatParseIssue::TooManyActions));
                    }
                    current_condition = attr(&start, b"condition");
                    text_target = Some("action".into());
                } else if name == "output" {
                    output = Some(MameCheatOutput {
                        condition: attr(&start, b"condition"),
                        format: attr(&start, b"format"),
                        line: attr(&start, b"line"),
                        align: attr(&start, b"align"),
                        arguments: Vec::new(),
                    });
                } else if name == "argument" {
                    current_attrs = Some((
                        "argument".into(),
                        attr(&start, b"count").unwrap_or_default(),
                    ));
                    text_target = Some("argument".into());
                } else if name == "comment" || name == "item" {
                    text_target = Some(name.clone());
                    if name == "item" {
                        current_attrs =
                            Some(("item".into(), attr(&start, b"value").unwrap_or_default()));
                    }
                } else if name != "mamecheat" {
                    return Err(MameCheatParseError(MameCheatParseIssue::UnexpectedElement(
                        name,
                    )));
                }
                stack.push(name);
            }
            Ok(Event::Empty(start)) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                if name == "action" {
                    if let Some(s) = script.as_mut() {
                        s.actions.push(MameCheatAction {
                            condition: attr(&start, b"condition"),
                            expression: MameNativeExpression {
                                original: String::new(),
                                context: "action".into(),
                                opaque: true,
                            },
                        });
                    }
                }
            }
            Ok(Event::Text(value)) => {
                let value = text(value)?;
                if let Some(target) = text_target.as_deref() {
                    if target == "action" {
                        if let Some(s) = script.as_mut() {
                            s.actions.push(MameCheatAction {
                                condition: current_condition.take(),
                                expression: MameNativeExpression {
                                    original: value,
                                    context: "action".into(),
                                    opaque: true,
                                },
                            });
                        }
                    } else if target == "argument" {
                        if let Some(o) = output.as_mut() {
                            o.arguments.push(MameNativeExpression {
                                original: value,
                                context: "argument".into(),
                                opaque: true,
                            });
                        }
                    } else if target == "comment" {
                        if let Some(c) = cheat.as_mut() {
                            c.comment = Some(value);
                        }
                    } else if target == "item" {
                        if let Some(p) = parameter.as_mut() {
                            let val = current_attrs.take().map(|(_, v)| v).unwrap_or_default();
                            p.items.push(MameCheatParameterItem {
                                value: val,
                                text: value,
                            });
                        }
                    }
                }
            }
            Ok(Event::End(end)) => {
                let name = String::from_utf8_lossy(end.name().as_ref()).into_owned();
                text_target = None;
                match name.as_str() {
                    "parameter" => {
                        if let (Some(c), Some(p)) = (cheat.as_mut(), parameter.take()) {
                            if c.parameters.len() >= MAME_CHEAT_MAX_PARAMETERS {
                                return Err(MameCheatParseError(
                                    MameCheatParseIssue::TooManyParameters,
                                ));
                            }
                            c.parameters.push(p);
                        }
                    }
                    "output" => {
                        if let (Some(s), Some(o)) = (script.as_mut(), output.take()) {
                            s.outputs.push(o);
                        }
                    }
                    "script" => {
                        if let (Some(c), Some(s)) = (cheat.as_mut(), script.take()) {
                            if c.scripts.len() >= MAME_CHEAT_MAX_SCRIPTS {
                                return Err(MameCheatParseError(
                                    MameCheatParseIssue::TooManyScripts,
                                ));
                            }
                            c.scripts.push(s);
                        }
                    }
                    "cheat" => {
                        if let Some(c) = cheat.take() {
                            if c.description.is_empty() {
                                return Err(MameCheatParseError(
                                    MameCheatParseIssue::MissingDescription,
                                ));
                            }
                            cheats.push(c);
                        }
                    }
                    _ => {}
                }
                stack.pop();
            }
            Ok(Event::Eof) => break,
            Ok(Event::Decl(_) | Event::Comment(_) | Event::CData(_) | Event::PI(_)) => {}
            Ok(Event::DocType(_)) => {
                return Err(MameCheatParseError(
                    MameCheatParseIssue::DoctypeOrExternalEntity,
                ));
            }
            Ok(Event::GeneralRef(_)) => {
                return Err(MameCheatParseError(
                    MameCheatParseIssue::DoctypeOrExternalEntity,
                ));
            }
            Err(error) => {
                return Err(MameCheatParseError(MameCheatParseIssue::MalformedXml(
                    error.to_string(),
                )));
            }
        }
        buf.clear();
    }
    if !stack.is_empty()
        || cheat.is_some()
        || script.is_some()
        || parameter.is_some()
        || output.is_some()
    {
        return Err(MameCheatParseError(MameCheatParseIssue::MalformedXml(
            "unclosed element".into(),
        )));
    }
    let version = file_version.ok_or_else(|| {
        MameCheatParseError(MameCheatParseIssue::MalformedXml(
            "missing or invalid version".into(),
        ))
    })?;
    if version != 1 {
        return Err(MameCheatParseError(
            MameCheatParseIssue::UnsupportedVersion(version),
        ));
    }
    let digest = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(MameCheatFile {
        machine: machine.into(),
        version,
        mame_version: None,
        cheats,
        provenance,
        source_sha256: Some(digest),
    })
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn render_mame_cheat_xml(file: &MameCheatFile) -> String {
    let mut out = String::from("<?xml version=\"1.0\"?>\n<mamecheat version=\"1\">\n");
    for cheat in &file.cheats {
        out.push_str(&format!(
            "  <cheat desc=\"{}\">\n",
            xml_escape(&cheat.description)
        ));
        if let Some(comment) = &cheat.comment {
            out.push_str(&format!("    <comment>{}</comment>\n", xml_escape(comment)));
        }
        for p in &cheat.parameters {
            out.push_str("    <parameter");
            for (k, v) in [("min", &p.min), ("max", &p.max), ("step", &p.step)] {
                if let Some(v) = v {
                    out.push_str(&format!(" {k}=\"{}\"", xml_escape(v)));
                }
            }
            out.push_str(">\n");
            for item in &p.items {
                out.push_str(&format!(
                    "      <item value=\"{}\">{}</item>\n",
                    xml_escape(&item.value),
                    xml_escape(&item.text)
                ));
            }
            out.push_str("    </parameter>\n");
        }
        for s in &cheat.scripts {
            let state = match s.state {
                MameScriptState::On => "on",
                MameScriptState::Off => "off",
                MameScriptState::Run => "run",
                MameScriptState::Change => "change(run)",
            };
            out.push_str(&format!("    <script state=\"{state}\">\n"));
            for a in &s.actions {
                out.push_str("      <action");
                if let Some(c) = &a.condition {
                    out.push_str(&format!(" condition=\"{}\"", xml_escape(c)));
                }
                out.push_str(&format!(
                    ">{}</action>\n",
                    xml_escape(&a.expression.original)
                ));
            }
            for o in &s.outputs {
                out.push_str("      <output");
                for (k, v) in [
                    ("condition", &o.condition),
                    ("format", &o.format),
                    ("line", &o.line),
                    ("align", &o.align),
                ] {
                    if let Some(v) = v {
                        out.push_str(&format!(" {k}=\"{}\"", xml_escape(v)));
                    }
                }
                if o.arguments.is_empty() {
                    out.push_str("/>\n");
                } else {
                    out.push_str(">\n");
                    for (i, a) in o.arguments.iter().enumerate() {
                        out.push_str(&format!(
                            "        <argument count=\"{}\">{}</argument>\n",
                            i,
                            xml_escape(&a.original)
                        ));
                    }
                    out.push_str("      </output>\n");
                }
            }
            out.push_str("    </script>\n");
        }
        out.push_str("  </cheat>\n");
    }
    out.push_str("</mamecheat>\n");
    out
}

fn direct_write(expression: &str) -> Option<MameNormalizedWrite> {
    let (left, right) = expression.split_once('=')?;
    let left = left.trim();
    let right = right.trim();
    if left.is_empty()
        || right.is_empty()
        || left.contains(|c: char| c.is_ascii_whitespace())
        || right.contains(|c: char| c.is_ascii_whitespace())
    {
        return None;
    }
    let address = u64::from_str_radix(
        left.trim_start_matches("0x")
            .trim_start_matches("0X")
            .trim_start_matches('$'),
        16,
    )
    .ok()?;
    let value = if let Some(v) = right
        .strip_prefix("0x")
        .or_else(|| right.strip_prefix("0X"))
    {
        u64::from_str_radix(v, 16).ok()?
    } else {
        right.parse().ok()?
    };
    let width_bits = if value <= u8::MAX as u64 {
        8
    } else if value <= u16::MAX as u64 {
        16
    } else if value <= u32::MAX as u64 {
        32
    } else {
        64
    };
    Some(MameNormalizedWrite {
        address,
        value,
        width_bits,
    })
}

pub fn inspect_mame_cheat(file: &MameCheatFile, selected_machine: &str) -> MameCheatInspection {
    let mut normalized_writes = Vec::new();
    let mut opaque = 0;
    let mut issues = Vec::new();
    if file.machine != selected_machine {
        return MameCheatInspection {
            readiness: MameCheatReadiness::WrongMachine,
            machine: file.machine.clone(),
            normalized_writes,
            opaque_expression_count: 0,
            runtime_state: MameCheatRuntimeState::Unknown,
            issues: vec![MameCheatParseIssue::InvalidMachine(file.machine.clone())],
        };
    }
    for cheat in &file.cheats {
        for script in &cheat.scripts {
            for action in &script.actions {
                if let Some(w) = direct_write(&action.expression.original) {
                    normalized_writes.push(w);
                } else {
                    opaque += 1;
                }
            }
            for output in &script.outputs {
                opaque += output.arguments.len();
            }
        }
    }
    if opaque > 0 {
        issues.push(MameCheatParseIssue::UnexpectedElement(
            "opaque native expressions preserved".into(),
        ));
    }
    MameCheatInspection {
        readiness: if opaque > 0 {
            MameCheatReadiness::ReadyWithOpaqueNativeOps
        } else {
            MameCheatReadiness::ReadyNative
        },
        machine: selected_machine.into(),
        normalized_writes,
        opaque_expression_count: opaque,
        runtime_state: MameCheatRuntimeState::RuntimeEnableRequired,
        issues,
    }
}

pub fn merge_mame_cheat(existing: &MameCheatFile, incoming: &MameCheatEntry) -> MameCheatFile {
    let mut merged = existing.clone();
    if !merged.cheats.iter().any(|c| c == incoming) {
        merged.cheats.push(incoming.clone());
    }
    merged.source_sha256 = None;
    merged
}

pub fn set_mame_cheat_enabled(
    _file: &MameCheatFile,
    _description: &str,
    _enabled: bool,
) -> MameCheatRuntimeState {
    MameCheatRuntimeState::RuntimeEnableRequired
}

/// Build the exact native-file preview and shared transaction plan.  The only
/// write performed here is to the caller-owned staging root; the destination
/// is untouched until the normal shared confirmation/apply path is invoked.
pub fn build_mame_cheat_preview(
    request: &MameCheatPreviewRequest,
) -> Result<MameCheatPreview, MameCheatPreviewError> {
    let mut file = load_mame_cheat(
        &request.source_file,
        &request.machine,
        MameCheatProvenance::LocalImport,
    )
    .map_err(MameCheatPreviewError::Parse)?;
    file.mame_version = request.mame_version.clone();
    let inspection = inspect_mame_cheat(&file, &request.machine);
    if inspection.readiness == MameCheatReadiness::WrongMachine {
        return Err(MameCheatPreviewError::WrongMachine);
    }
    fs::create_dir_all(&request.staging_root)
        .map_err(|e| MameCheatPreviewError::Io(e.to_string()))?;
    let staged = request
        .staging_root
        .join(format!("{}.xml", request.machine));
    fs::write(&staged, render_mame_cheat_xml(&file))
        .map_err(|e| MameCheatPreviewError::Io(e.to_string()))?;
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Mame,
        selected_archive: request.source_file.clone(),
        platform: Some("mame".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::MameMachineShortname,
            state: PreviewIdentityState::Verified,
            value: Some(request.machine.clone()),
            archive_path: request.source_file.clone(),
            revision: None,
        },
        destination_root: request.destination_root.clone(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::Mame,
            source_path: staged,
            expected_source_digest: None,
            destination_relative_paths: vec![PathBuf::from(format!("{}.xml", request.machine))],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|e| MameCheatPreviewError::Shared(format!("{e:?}")))?;
    let plan = build_shared_transaction_plan(
        &report,
        &request.profile_id,
        "mame-native-cheat",
        &request.staging_root,
    )
    .map_err(|e| MameCheatPreviewError::Shared(format!("{e:?}")))?;
    Ok(MameCheatPreview {
        file,
        inspection,
        report,
        transaction_plan: plan,
    })
}

pub fn load_mame_cheat(
    path: &Path,
    machine: &str,
    provenance: MameCheatProvenance,
) -> Result<MameCheatFile, MameCheatParseError> {
    let bytes = fs::read(path)
        .map_err(|e| MameCheatParseError(MameCheatParseIssue::MalformedXml(e.to_string())))?;
    parse_mame_cheat_xml(&bytes, machine, provenance)
}

#[cfg(test)]
mod tests {
    use super::*;
    const XML: &str = r#"<?xml version="1.0"?><mamecheat version="1"><cheat desc="Infinite Lives"><comment>safe</comment><parameter min="0" max="2" step="1"><item value="0">Off</item><item value="1">On</item></parameter><script state="on"><action>0x1234 = 99</action><action condition="frame &gt; 2">foo = bar</action></script></cheat></mamecheat>"#;
    #[test]
    fn parses_and_inspects_without_execution() {
        let file = parse_mame_cheat_xml(XML.as_bytes(), "pacman", MameCheatProvenance::LocalImport)
            .unwrap();
        let report = inspect_mame_cheat(&file, "pacman");
        assert_eq!(report.normalized_writes[0].address, 0x1234);
        assert_eq!(report.opaque_expression_count, 1);
        assert_eq!(
            report.readiness,
            MameCheatReadiness::ReadyWithOpaqueNativeOps
        );
    }
    #[test]
    fn machine_is_authoritative() {
        let file = parse_mame_cheat_xml(XML.as_bytes(), "pacman", MameCheatProvenance::LocalImport)
            .unwrap();
        assert_eq!(
            inspect_mame_cheat(&file, "galaga").readiness,
            MameCheatReadiness::WrongMachine
        );
    }
    #[test]
    fn doctype_is_refused() {
        assert!(matches!(
            parse_mame_cheat_xml(
                b"<!DOCTYPE x SYSTEM 'x'><mamecheat version='1'/>",
                "pacman",
                MameCheatProvenance::LocalImport
            ),
            Err(MameCheatParseError(
                MameCheatParseIssue::DoctypeOrExternalEntity
            ))
        ));
    }
    #[test]
    fn rendering_is_deterministic_and_merging_deduplicates() {
        let f = parse_mame_cheat_xml(XML.as_bytes(), "pacman", MameCheatProvenance::LocalImport)
            .unwrap();
        assert_eq!(render_mame_cheat_xml(&f), render_mame_cheat_xml(&f));
        let m = merge_mame_cheat(&f, &f.cheats[0]);
        assert_eq!(m.cheats.len(), 1);
    }
    #[test]
    fn enablement_is_runtime_only() {
        let f = parse_mame_cheat_xml(XML.as_bytes(), "pacman", MameCheatProvenance::LocalImport)
            .unwrap();
        assert_eq!(
            set_mame_cheat_enabled(&f, "Infinite Lives", true),
            MameCheatRuntimeState::RuntimeEnableRequired
        );
    }
}
