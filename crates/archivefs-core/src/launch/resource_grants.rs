//! Typed, declarative filesystem resources for a future isolated launch.
//!
//! This module deliberately does not project paths, change permissions, or
//! enforce access at the operating-system boundary.  It describes intent for
//! a later launch executor and rejects internally contradictory contracts.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LaunchResourceRole {
    GameMedia,
    BiosFirmware,
    DependencyRom,
    DeviceRom,
    Config,
    Profile,
    SaveData,
    MemoryCard,
    Nvram,
    Nand,
    HddImage,
    Cache,
    TemporaryRuntime,
    Metadata,
    Artwork,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LaunchResourceAccess {
    ReadOnly,
    ReadWrite,
    CreateOnly,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LaunchResourceLifetime {
    LaunchOnly,
    Session,
    Persistent,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LaunchProjectionMethod {
    DirectPath,
    SymlinkFile,
    SymlinkDirectory,
    ReadOnlyBind,
    BindMount,
    Reflink,
    ScratchCopy,
    TempCopy,
    GeneratedFile,
    GeneratedDirectory,
    ConfigOverride,
    NoProjection,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LaunchAccessScope {
    StrictMinimal,
    NarrowDirectory,
    EmulatorProfileRoot,
    LegacyBroadAccess,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LaunchResourceGrant {
    pub launch_id: String,
    pub role: LaunchResourceRole,
    pub source_path: Option<PathBuf>,
    pub presented_path: Option<PathBuf>,
    pub access: LaunchResourceAccess,
    pub projection: LaunchProjectionMethod,
    pub lifetime: LaunchResourceLifetime,
    pub scope: LaunchAccessScope,
    pub provenance: String,
    pub reason: String,
}

impl LaunchResourceGrant {
    pub fn validate(&self) -> Result<(), LaunchResourceGrantError> {
        let source_required = matches!(
            self.projection,
            LaunchProjectionMethod::DirectPath
                | LaunchProjectionMethod::SymlinkFile
                | LaunchProjectionMethod::SymlinkDirectory
                | LaunchProjectionMethod::ReadOnlyBind
                | LaunchProjectionMethod::BindMount
                | LaunchProjectionMethod::Reflink
                | LaunchProjectionMethod::ScratchCopy
                | LaunchProjectionMethod::TempCopy
        );
        let destination_required = !matches!(self.projection, LaunchProjectionMethod::NoProjection);

        if source_required && self.source_path.is_none() {
            return Err(LaunchResourceGrantError::MissingSource);
        }
        if destination_required && self.presented_path.is_none() {
            return Err(LaunchResourceGrantError::MissingPresentedPath);
        }
        if !source_required
            && self.source_path.is_some()
            && matches!(
                self.projection,
                LaunchProjectionMethod::GeneratedFile
                    | LaunchProjectionMethod::GeneratedDirectory
                    | LaunchProjectionMethod::ConfigOverride
                    | LaunchProjectionMethod::NoProjection
            )
        {
            return Err(LaunchResourceGrantError::UnexpectedSource);
        }

        for (kind, path) in [
            (LaunchPathKind::Source, self.source_path.as_ref()),
            (LaunchPathKind::Presented, self.presented_path.as_ref()),
        ] {
            if let Some(path) = path {
                if !path.is_absolute()
                    || path
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
                {
                    return Err(LaunchResourceGrantError::UnsafePath(kind));
                }
            }
        }

        if matches!(
            self.projection,
            LaunchProjectionMethod::SymlinkFile
                | LaunchProjectionMethod::SymlinkDirectory
                | LaunchProjectionMethod::ReadOnlyBind
                | LaunchProjectionMethod::BindMount
                | LaunchProjectionMethod::Reflink
                | LaunchProjectionMethod::ScratchCopy
                | LaunchProjectionMethod::TempCopy
        ) && self.source_path == self.presented_path
        {
            return Err(LaunchResourceGrantError::AliasedProjection);
        }

        if matches!(self.projection, LaunchProjectionMethod::GeneratedFile)
            && self.access == LaunchResourceAccess::ReadWrite
        {
            return Err(LaunchResourceGrantError::InvalidCombination(
                "generated files cannot be declared READ_WRITE before creation",
            ));
        }
        if matches!(self.projection, LaunchProjectionMethod::GeneratedDirectory)
            && self.access == LaunchResourceAccess::ReadOnly
        {
            return Err(LaunchResourceGrantError::InvalidCombination(
                "generated directories must allow creation or writing",
            ));
        }
        if self.lifetime == LaunchResourceLifetime::LaunchOnly
            && matches!(
                self.role,
                LaunchResourceRole::SaveData
                    | LaunchResourceRole::MemoryCard
                    | LaunchResourceRole::Nvram
                    | LaunchResourceRole::Nand
                    | LaunchResourceRole::HddImage
            )
            && self.access != LaunchResourceAccess::ReadOnly
        {
            return Err(LaunchResourceGrantError::InvalidCombination(
                "persistent user state cannot be writable for LAUNCH_ONLY lifetime",
            ));
        }
        if self.scope == LaunchAccessScope::LegacyBroadAccess
            && self.projection == LaunchProjectionMethod::NoProjection
        {
            return Err(LaunchResourceGrantError::InvalidCombination(
                "LEGACY_BROAD_ACCESS requires an explicit projection",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LaunchResourceGrantSet {
    pub grants: Vec<LaunchResourceGrant>,
}

impl LaunchResourceGrantSet {
    pub fn validate(&self) -> Result<(), LaunchResourceGrantError> {
        let mut by_presented: BTreeMap<PathBuf, &LaunchResourceGrant> = BTreeMap::new();
        for grant in &self.grants {
            grant.validate()?;
            let Some(path) = grant.presented_path.as_ref() else {
                continue;
            };
            if let Some(existing) = by_presented.insert(path.clone(), grant) {
                if existing == grant {
                    continue;
                }
                return Err(LaunchResourceGrantError::PresentedPathConflict(
                    path.clone(),
                ));
            }
        }
        Ok(())
    }

    pub fn try_insert(
        &mut self,
        grant: LaunchResourceGrant,
    ) -> Result<(), LaunchResourceGrantError> {
        grant.validate()?;
        if let Some(existing) = self
            .grants
            .iter()
            .find(|existing| existing.presented_path == grant.presented_path)
        {
            if existing == &grant {
                return Ok(());
            }
            if let Some(path) = grant.presented_path {
                return Err(LaunchResourceGrantError::PresentedPathConflict(path));
            }
        }
        self.grants.push(grant);
        self.grants.sort_by(|left, right| {
            left.presented_path
                .cmp(&right.presented_path)
                .then(left.role.cmp(&right.role))
                .then(left.projection.cmp(&right.projection))
                .then(left.reason.cmp(&right.reason))
        });
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchPathKind {
    Source,
    Presented,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchResourceGrantError {
    MissingSource,
    MissingPresentedPath,
    UnexpectedSource,
    UnsafePath(LaunchPathKind),
    AliasedProjection,
    PresentedPathConflict(PathBuf),
    InvalidCombination(&'static str),
}

impl fmt::Display for LaunchResourceGrantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSource => write!(f, "projection requires a source path"),
            Self::MissingPresentedPath => write!(f, "projection requires a presented path"),
            Self::UnexpectedSource => write!(f, "projection does not accept a source path"),
            Self::UnsafePath(kind) => {
                write!(f, "{kind:?} path must be absolute and traversal-free")
            }
            Self::AliasedProjection => write!(f, "projection source and destination must differ"),
            Self::PresentedPathConflict(path) => {
                write!(f, "conflicting grants target presented path {path:?}")
            }
            Self::InvalidCombination(detail) => {
                write!(f, "invalid launch resource grant: {detail}")
            }
        }
    }
}

impl std::error::Error for LaunchResourceGrantError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(
        role: LaunchResourceRole,
        source: Option<&str>,
        presented: Option<&str>,
        access: LaunchResourceAccess,
        projection: LaunchProjectionMethod,
        lifetime: LaunchResourceLifetime,
    ) -> LaunchResourceGrant {
        LaunchResourceGrant {
            launch_id: "test-launch".into(),
            role,
            source_path: source.map(PathBuf::from),
            presented_path: presented.map(PathBuf::from),
            access,
            projection,
            lifetime,
            scope: LaunchAccessScope::StrictMinimal,
            provenance: "synthetic fixture".into(),
            reason: "test resource".into(),
        }
    }

    #[test]
    fn representative_retroarch_mame_xemu_and_dolphin_grants_validate() {
        let mut set = LaunchResourceGrantSet::default();
        for item in [
            grant(
                LaunchResourceRole::GameMedia,
                Some("/source/game.chd"),
                Some("/run/game.chd"),
                LaunchResourceAccess::ReadOnly,
                LaunchProjectionMethod::DirectPath,
                LaunchResourceLifetime::Session,
            ),
            grant(
                LaunchResourceRole::BiosFirmware,
                Some("/source/bios.bin"),
                Some("/run/system/bios.bin"),
                LaunchResourceAccess::ReadOnly,
                LaunchProjectionMethod::SymlinkFile,
                LaunchResourceLifetime::Session,
            ),
            grant(
                LaunchResourceRole::SaveData,
                Some("/state/saves"),
                Some("/run/saves"),
                LaunchResourceAccess::ReadWrite,
                LaunchProjectionMethod::TempCopy,
                LaunchResourceLifetime::Persistent,
            ),
            grant(
                LaunchResourceRole::DependencyRom,
                Some("/source/parent.zip"),
                Some("/run/parent.zip"),
                LaunchResourceAccess::ReadOnly,
                LaunchProjectionMethod::DirectPath,
                LaunchResourceLifetime::Session,
            ),
            grant(
                LaunchResourceRole::DeviceRom,
                Some("/source/qsound.zip"),
                Some("/run/qsound.zip"),
                LaunchResourceAccess::ReadOnly,
                LaunchProjectionMethod::DirectPath,
                LaunchResourceLifetime::Session,
            ),
            grant(
                LaunchResourceRole::HddImage,
                Some("/state/hdd.img"),
                Some("/run/hdd.img"),
                LaunchResourceAccess::ReadWrite,
                LaunchProjectionMethod::ScratchCopy,
                LaunchResourceLifetime::Persistent,
            ),
            grant(
                LaunchResourceRole::Nand,
                Some("/state/nand"),
                Some("/run/nand"),
                LaunchResourceAccess::ReadWrite,
                LaunchProjectionMethod::TempCopy,
                LaunchResourceLifetime::Persistent,
            ),
            grant(
                LaunchResourceRole::MemoryCard,
                Some("/state/card.mcr"),
                Some("/run/card.mcr"),
                LaunchResourceAccess::ReadWrite,
                LaunchProjectionMethod::ScratchCopy,
                LaunchResourceLifetime::Persistent,
            ),
        ] {
            set.try_insert(item).unwrap();
        }
        assert!(set.validate().is_ok());
        assert_eq!(
            set.grants.first().unwrap().presented_path.as_deref(),
            Some(std::path::Path::new("/run/bios.bin"))
        );
    }

    #[test]
    fn missing_paths_and_relative_paths_fail_closed() {
        let missing = grant(
            LaunchResourceRole::GameMedia,
            None,
            Some("/run/game"),
            LaunchResourceAccess::ReadOnly,
            LaunchProjectionMethod::DirectPath,
            LaunchResourceLifetime::Session,
        );
        assert_eq!(
            missing.validate(),
            Err(LaunchResourceGrantError::MissingSource)
        );
        let relative = grant(
            LaunchResourceRole::GameMedia,
            Some("game.iso"),
            Some("/run/game.iso"),
            LaunchResourceAccess::ReadOnly,
            LaunchProjectionMethod::DirectPath,
            LaunchResourceLifetime::Session,
        );
        assert_eq!(
            relative.validate(),
            Err(LaunchResourceGrantError::UnsafePath(LaunchPathKind::Source))
        );
        let traversal = grant(
            LaunchResourceRole::GameMedia,
            Some("/source/game.iso"),
            Some("/run/../game.iso"),
            LaunchResourceAccess::ReadOnly,
            LaunchProjectionMethod::DirectPath,
            LaunchResourceLifetime::Session,
        );
        assert_eq!(
            traversal.validate(),
            Err(LaunchResourceGrantError::UnsafePath(
                LaunchPathKind::Presented
            ))
        );
    }

    #[test]
    fn duplicate_is_idempotent_but_access_conflict_is_rejected() {
        let item = grant(
            LaunchResourceRole::SaveData,
            None,
            Some("/run/save"),
            LaunchResourceAccess::CreateOnly,
            LaunchProjectionMethod::GeneratedDirectory,
            LaunchResourceLifetime::Session,
        );
        let mut set = LaunchResourceGrantSet::default();
        set.try_insert(item.clone()).unwrap();
        set.try_insert(item).unwrap();
        assert_eq!(set.grants.len(), 1);
        let conflict = grant(
            LaunchResourceRole::SaveData,
            None,
            Some("/run/save"),
            LaunchResourceAccess::ReadWrite,
            LaunchProjectionMethod::GeneratedDirectory,
            LaunchResourceLifetime::Persistent,
        );
        assert!(matches!(
            set.try_insert(conflict),
            Err(LaunchResourceGrantError::PresentedPathConflict(_))
        ));
    }

    #[test]
    fn invalid_alias_and_lifetime_combinations_are_rejected() {
        let alias = grant(
            LaunchResourceRole::MemoryCard,
            Some("/state/card"),
            Some("/state/card"),
            LaunchResourceAccess::ReadWrite,
            LaunchProjectionMethod::ScratchCopy,
            LaunchResourceLifetime::Persistent,
        );
        assert_eq!(
            alias.validate(),
            Err(LaunchResourceGrantError::AliasedProjection)
        );
        let transient_save = grant(
            LaunchResourceRole::SaveData,
            Some("/state/save"),
            Some("/run/save"),
            LaunchResourceAccess::ReadWrite,
            LaunchProjectionMethod::TempCopy,
            LaunchResourceLifetime::LaunchOnly,
        );
        assert!(matches!(
            transient_save.validate(),
            Err(LaunchResourceGrantError::InvalidCombination(_))
        ));
    }
}
