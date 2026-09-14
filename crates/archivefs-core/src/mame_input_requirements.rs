//! Conservative normalization of preserved MAME static input metadata.
//!
//! The output describes original machine controls only.  It is not a probe of
//! connected hardware, a controller requirement, a launch blocker, or a
//! Ready-to-Play input decision.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::dat::{MameControlMetadata, MameInputMetadata};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ArcadeInputRequirementFamily {
    DigitalDirections,
    Digital2Way,
    Digital4Way,
    Digital8Way,
    DirectionalOther,
    Buttons,
    DualStick,
    AnalogAxis,
    Pedal,
    RelativePointer,
    AbsolutePointer,
    LightGun,
    Keyboard,
    Mouse,
    Trackball,
    DialSpinner,
    PositionalControl,
    SpecialPanel,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ArcadeInputEvidenceStrength {
    AuthoritativeMachineMetadata,
    NormalizedFromMachineMetadata,
    Heuristic,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArcadeInputRequirement {
    pub family: ArcadeInputRequirementFamily,
    pub raw_control_type: Option<String>,
    pub player: Option<String>,
    pub buttons: Option<String>,
    pub reqbuttons: Option<String>,
    pub ways: Option<String>,
    pub ways2: Option<String>,
    pub ways3: Option<String>,
    pub minimum: Option<String>,
    pub maximum: Option<String>,
    pub sensitivity: Option<String>,
    pub keydelta: Option<String>,
    pub reverse: Option<String>,
    pub raw_attributes: BTreeMap<String, String>,
    pub evidence_strength: ArcadeInputEvidenceStrength,
    pub provenance: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArcadeInputRequirements {
    /// MAME's supported-player declaration, not a connected-controller count.
    pub supported_players: Option<String>,
    pub requirements: Vec<ArcadeInputRequirement>,
    pub provenance: String,
}

impl ArcadeInputRequirements {
    pub fn from_mame(input: &MameInputMetadata) -> Self {
        let mut requirements = Vec::new();
        for control in &input.controls {
            let base = requirement_base(control);
            let family = family_for(control);
            requirements.push(ArcadeInputRequirement {
                family,
                ..base.clone()
            });

            if control.buttons.is_some() && family != ArcadeInputRequirementFamily::Buttons {
                requirements.push(ArcadeInputRequirement {
                    family: ArcadeInputRequirementFamily::Buttons,
                    ..base
                });
            }
        }
        Self {
            supported_players: input.players.clone(),
            requirements,
            provenance: "MAME listxml <input>/<control> metadata".into(),
        }
    }
}

fn requirement_base(control: &MameControlMetadata) -> ArcadeInputRequirement {
    ArcadeInputRequirement {
        family: ArcadeInputRequirementFamily::Unknown,
        raw_control_type: control.control_type.clone(),
        player: control.player.clone(),
        buttons: control.buttons.clone(),
        reqbuttons: control.reqbuttons.clone(),
        ways: control.ways.clone(),
        ways2: control.ways2.clone(),
        ways3: control.ways3.clone(),
        minimum: control.minimum.clone(),
        maximum: control.maximum.clone(),
        sensitivity: control.sensitivity.clone(),
        keydelta: control.keydelta.clone(),
        reverse: control.reverse.clone(),
        raw_attributes: control.raw_attributes.clone(),
        evidence_strength: ArcadeInputEvidenceStrength::NormalizedFromMachineMetadata,
        provenance: "MAME listxml <control>".into(),
    }
}

fn family_for(control: &MameControlMetadata) -> ArcadeInputRequirementFamily {
    let control_type = control
        .control_type
        .as_deref()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match control_type.as_str() {
        "joy" | "stick" => direction_family(control.ways.as_deref()),
        "doublejoy" => ArcadeInputRequirementFamily::DualStick,
        "paddle" => ArcadeInputRequirementFamily::AnalogAxis,
        "pedal" => ArcadeInputRequirementFamily::Pedal,
        "trackball" => ArcadeInputRequirementFamily::Trackball,
        "dial" => ArcadeInputRequirementFamily::DialSpinner,
        "mouse" => ArcadeInputRequirementFamily::Mouse,
        "lightgun" => ArcadeInputRequirementFamily::LightGun,
        "positional" => ArcadeInputRequirementFamily::PositionalControl,
        "keyboard" => ArcadeInputRequirementFamily::Keyboard,
        "only_buttons" => ArcadeInputRequirementFamily::Buttons,
        "mahjong" | "hanafuda" | "gambling" | "keypad" => {
            ArcadeInputRequirementFamily::SpecialPanel
        }
        _ => ArcadeInputRequirementFamily::Unknown,
    }
}

fn direction_family(ways: Option<&str>) -> ArcadeInputRequirementFamily {
    match ways.map(str::to_ascii_lowercase).as_deref() {
        Some("2") => ArcadeInputRequirementFamily::Digital2Way,
        Some("4") => ArcadeInputRequirementFamily::Digital4Way,
        Some("8") => ArcadeInputRequirementFamily::Digital8Way,
        Some(_) => ArcadeInputRequirementFamily::DirectionalOther,
        None => ArcadeInputRequirementFamily::DigitalDirections,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(
        control_type: &str,
        ways: Option<&str>,
        buttons: Option<&str>,
    ) -> MameControlMetadata {
        MameControlMetadata {
            control_type: Some(control_type.into()),
            ways: ways.map(str::to_owned),
            buttons: buttons.map(str::to_owned),
            raw_attributes: [("type".into(), control_type.into())].into_iter().collect(),
            ..Default::default()
        }
    }

    fn input(controls: Vec<MameControlMetadata>) -> MameInputMetadata {
        MameInputMetadata {
            players: Some("4".into()),
            controls,
            ..Default::default()
        }
    }

    #[test]
    fn joystick_buttons_and_supported_players_are_separate_facts() {
        let normalized =
            ArcadeInputRequirements::from_mame(&input(vec![control("joy", Some("8"), Some("3"))]));
        assert_eq!(normalized.supported_players.as_deref(), Some("4"));
        assert_eq!(normalized.requirements.len(), 2);
        assert_eq!(
            normalized.requirements[0].family,
            ArcadeInputRequirementFamily::Digital8Way
        );
        assert_eq!(
            normalized.requirements[1].family,
            ArcadeInputRequirementFamily::Buttons
        );
        assert_eq!(normalized.requirements[1].buttons.as_deref(), Some("3"));
    }

    #[test]
    fn analog_and_special_families_are_conservative() {
        let normalized = ArcadeInputRequirements::from_mame(&input(vec![
            control("trackball", None, Some("2")),
            control("lightgun", None, None),
            control("pedal", None, None),
            control("dial", None, None),
            control("only_buttons", None, Some("6")),
        ]));
        let families: Vec<_> = normalized.requirements.iter().map(|r| r.family).collect();
        assert!(families.contains(&ArcadeInputRequirementFamily::Trackball));
        assert!(families.contains(&ArcadeInputRequirementFamily::LightGun));
        assert!(families.contains(&ArcadeInputRequirementFamily::Pedal));
        assert!(families.contains(&ArcadeInputRequirementFamily::DialSpinner));
        assert!(families.contains(&ArcadeInputRequirementFamily::Buttons));
    }

    #[test]
    fn version_drift_and_nonnumeric_ways_are_preserved() {
        let mut raw = control("joy", Some("vertical2"), None);
        raw.reqbuttons = Some("2".into());
        raw.ways2 = Some("horizontal2".into());
        raw.raw_attributes.insert("future".into(), "value".into());
        let normalized = ArcadeInputRequirements::from_mame(&input(vec![raw]));
        let requirement = &normalized.requirements[0];
        assert_eq!(
            requirement.family,
            ArcadeInputRequirementFamily::DirectionalOther
        );
        assert_eq!(requirement.ways.as_deref(), Some("vertical2"));
        assert_eq!(requirement.ways2.as_deref(), Some("horizontal2"));
        assert_eq!(requirement.reqbuttons.as_deref(), Some("2"));
        assert_eq!(requirement.raw_attributes["future"], "value");
    }

    #[test]
    fn unknown_control_type_is_not_rejected_or_promoted() {
        let normalized =
            ArcadeInputRequirements::from_mame(&input(vec![control("future-control", None, None)]));
        assert_eq!(
            normalized.requirements[0].family,
            ArcadeInputRequirementFamily::Unknown
        );
        assert_eq!(
            normalized.requirements[0].evidence_strength,
            ArcadeInputEvidenceStrength::NormalizedFromMachineMetadata
        );
    }

    #[test]
    fn dual_stick_keyboard_and_mouse_remain_static_metadata() {
        let normalized = ArcadeInputRequirements::from_mame(&input(vec![
            control("doublejoy", None, None),
            control("keyboard", None, None),
            control("mouse", None, None),
        ]));
        assert_eq!(
            normalized.requirements[0].family,
            ArcadeInputRequirementFamily::DualStick
        );
        assert_eq!(
            normalized.requirements[1].family,
            ArcadeInputRequirementFamily::Keyboard
        );
        assert_eq!(
            normalized.requirements[2].family,
            ArcadeInputRequirementFamily::Mouse
        );
    }
}
