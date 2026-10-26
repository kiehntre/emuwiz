//! Read-only setup facts from the existing catalogue inventory. No matching,
//! parsing, trust assignment or verification is performed by this projection.
use super::{CatalogueAvailability, InstalledCatalogueSummary};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DatSetupStatus {
    pub configured: usize,
    pub enabled: usize,
    pub ready: usize,
    pub needs_validation: usize,
    pub folders: usize,
}

impl DatSetupStatus {
    pub fn from_catalogues(rows: &[InstalledCatalogueSummary]) -> Self {
        let mut status = Self {
            configured: rows.len(),
            ..Self::default()
        };
        for row in rows.iter().filter(|row| row.enabled) {
            status.enabled += 1;
            match row.availability {
                CatalogueAvailability::Ready if row.capabilities.verify => status.ready += 1,
                CatalogueAvailability::NeedsValidation { .. } if row.capabilities.verify => {
                    status.needs_validation += 1
                }
                CatalogueAvailability::AggregateFolder { .. } => status.folders += 1,
                _ => {}
            }
        }
        status
    }

    pub fn label(&self) -> &'static str {
        if self.configured == 0 {
            "No trusted DAT configured"
        } else if self.enabled == 0 {
            "Configured DAT unavailable or disabled"
        } else if self.ready > 0 {
            "Game catalogue available"
        } else if self.needs_validation > 0 {
            "Catalogue needs validation"
        } else if self.folders > 0 {
            "Catalogue folder configured"
        } else {
            "No game catalogue available"
        }
    }

    pub fn detail(&self) -> String {
        format!(
            "{} configured · {} enabled · {} ready for game checks · {} need validation · {} folders. Catalogue availability does not verify any game. Import local data explicitly, then Preview and confirm a check.",
            self.configured, self.enabled, self.ready, self.needs_validation, self.folders
        )
    }
}
