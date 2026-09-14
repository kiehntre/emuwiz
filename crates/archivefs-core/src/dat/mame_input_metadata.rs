//! Raw, MAME-specific static input metadata.
//!
//! These values describe what a MAME driver declares, not what hardware the
//! user owns or what a launch requires.  Values intentionally remain strings:
//! MAME has version-dependent attributes and values such as `vertical2` for
//! `ways`, so MI0 does not normalize or interpret them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameInputMetadata {
    pub players: Option<String>,
    pub coins: Option<String>,
    pub service: Option<String>,
    pub tilt: Option<String>,
    #[serde(default)]
    pub controls: Vec<MameControlMetadata>,
    /// All attributes from the `<input>` element, including known attributes.
    /// Keeping the complete bounded map preserves version-specific evidence.
    #[serde(default)]
    pub raw_attributes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameControlMetadata {
    pub control_type: Option<String>,
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
    /// All attributes from the `<control>` element, including attributes not
    /// known to this parser version.
    #[serde(default)]
    pub raw_attributes: BTreeMap<String, String>,
}

impl MameInputMetadata {
    pub(crate) fn from_raw(raw_attributes: BTreeMap<String, String>) -> Self {
        Self {
            players: raw_attributes.get("players").cloned(),
            coins: raw_attributes.get("coins").cloned(),
            service: raw_attributes.get("service").cloned(),
            tilt: raw_attributes.get("tilt").cloned(),
            controls: Vec::new(),
            raw_attributes,
        }
    }
}

impl MameControlMetadata {
    pub(crate) fn from_raw(raw_attributes: BTreeMap<String, String>) -> Self {
        Self {
            control_type: raw_attributes.get("type").cloned(),
            player: raw_attributes.get("player").cloned(),
            buttons: raw_attributes.get("buttons").cloned(),
            reqbuttons: raw_attributes.get("reqbuttons").cloned(),
            ways: raw_attributes.get("ways").cloned(),
            ways2: raw_attributes.get("ways2").cloned(),
            ways3: raw_attributes.get("ways3").cloned(),
            minimum: raw_attributes.get("minimum").cloned(),
            maximum: raw_attributes.get("maximum").cloned(),
            sensitivity: raw_attributes.get("sensitivity").cloned(),
            keydelta: raw_attributes.get("keydelta").cloned(),
            reverse: raw_attributes.get("reverse").cloned(),
            raw_attributes,
        }
    }
}
