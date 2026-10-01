//! Bounded DCP replacement of a reviewed extracted Dreamcast tree.
//! No image reconstruction or launch. Callers must supply an independently
//! reviewed package/source binding: DCP itself has no cryptographic base claim.
//! A returned shared receipt is staged, not published; use tree::{publish,inspect,undo}.
use crate::dreamcast_boot_evidence::{
    DreamcastIpBinValidationStatus, IP_BIN_META_BYTES, inspect_ip_bin_meta,
};
use crate::dreamcast_patch_readiness::{
    DreamcastPatchEntryKind, DreamcastPatchPackage, MAX_DCP_EXPANDED_BYTES, inspect_dreamcast_dcp,
    safe_relative_path,
};
use crate::optical_patch_tree::{Content, Contents, refuse};
use crate::patch_output_recovery::tree::{self, PreparedTreePatch, TreePatchPlan};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_STAGING_BYTES: u64 = MAX_SOURCE_BYTES + MAX_DCP_EXPANDED_BYTES;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DreamcastIdentity {
    pub product_code: String,
    pub revision: String,
    pub region: String,
}
/// Expected values supplied by a trusted catalogue or explicit package/source
/// review. Merely computing two hashes does not establish their relationship.
#[derive(Clone, Debug)]
pub struct DreamcastDcpBinding {
    pub package_sha256: String,
    pub source_tree_sha256: String,
    pub identity: DreamcastIdentity,
}
#[derive(Clone, Debug)]
pub struct DreamcastDcpPlan {
    tree: TreePatchPlan,
    source: PathBuf,
    package: DreamcastPatchPackage,
    original: Contents,
    expected: Contents,
    identity: DreamcastIdentity,
    boot_filename: String,
    allowance: u64,
}
/// Read-only review evidence, never an inferred package/source association.
pub fn source_tree_sha256(source: &Path) -> io::Result<String> {
    Contents::read(source, MAX_SOURCE_BYTES)?.fingerprint()
}
fn verify_ip(root: &Path, identity: &DreamcastIdentity) -> io::Result<String> {
    let mut bytes = [0u8; IP_BIN_META_BYTES];
    File::open(root.join("bootsector/IP.BIN"))?.read_exact(&mut bytes)?;
    let ip = inspect_ip_bin_meta(&bytes).map_err(|e| refuse(e.message))?;
    if matches!(
        ip.validation_status,
        DreamcastIpBinValidationStatus::Invalid | DreamcastIpBinValidationStatus::Truncated
    ) || !matches!(
        ip.hardware_id.value.as_str(),
        "SEGA SEGAKATANA" | "SEGA SEGAMARIO"
    ) || !ip.device_information.value.contains("GD-ROM")
        || identity.product_code.is_empty()
        || identity.revision.is_empty()
        || identity.region.is_empty()
        || ip.product_number.value != identity.product_code
        || ip.product_version.value != identity.revision
        || ip.area_symbols.value != identity.region
    {
        return Err(refuse(
            "Dreamcast IP.BIN domain/product/revision/region mismatch",
        ));
    }
    safe_relative_path(&ip.boot_filename.value).map_err(refuse)?;
    if !fs::symlink_metadata(root.join(&ip.boot_filename.value))?.is_file() {
        return Err(refuse("missing regular Dreamcast boot member"));
    }
    Ok(ip.boot_filename.value)
}
/// Only a directory with bootsector/IP.BIN and reviewed existing replacement
/// targets is accepted. Passing GDI/CHD/CDI/raw media fails before patch decoding.
pub fn review_dreamcast_dcp(
    source: &Path,
    package_path: &Path,
    destination: &Path,
    binding: &DreamcastDcpBinding,
) -> io::Result<DreamcastDcpPlan> {
    if package_path.starts_with(source) {
        return Err(refuse("DCP must be outside the source tree"));
    }
    let original = Contents::read(source, MAX_SOURCE_BYTES)?;
    if original.fingerprint()? != binding.source_tree_sha256 {
        return Err(refuse("exact extracted source binding mismatch"));
    }
    // Shared validation also rejects patch symlink ancestors, hardlinks, input
    // overlap and destinations inside source before any package decoding.
    TreePatchPlan::review_with_max_total_bytes(
        &[source.to_owned(), package_path.to_owned()],
        destination,
        MAX_STAGING_BYTES,
    )?;
    let package = inspect_dreamcast_dcp(package_path).map_err(refuse)?;
    if package.package_sha256 != binding.package_sha256 {
        return Err(refuse("exact package binding mismatch"));
    }
    let boot_filename = verify_ip(source, &binding.identity)?;
    let mut expected = original.clone();
    let mut replacements = 0;
    for entry in &package.entries {
        match entry.kind {
            DreamcastPatchEntryKind::Metadata => continue,
            DreamcastPatchEntryKind::FileReplacement | DreamcastPatchEntryKind::IpBin => {}
            _ => return Err(refuse("opaque DCP delta/unsupported patch operation")),
        }
        let path = Path::new(&entry.relative_path);
        let before = original.file(path)?; // No new/outside targets.
        if entry.kind == DreamcastPatchEntryKind::IpBin && entry.size_bytes != before.size {
            return Err(refuse(
                "IP.BIN replacement must preserve the reviewed boot-sector length",
            ));
        }
        expected.0.insert(
            path.to_owned(),
            Some(Content {
                size: entry.size_bytes,
                sha256: entry.sha256.clone(),
            }),
        );
        replacements += 1;
    }
    if replacements == 0 {
        return Err(refuse("DCP has no supported replacements"));
    }
    let input_size = original
        .size()?
        .checked_add(fs::metadata(package_path)?.len())
        .ok_or_else(|| refuse("input size overflow"))?;
    let allowance = input_size.max(expected.size()?).max(1);
    if allowance > MAX_STAGING_BYTES {
        return Err(refuse("DCP staging byte policy exceeded"));
    }
    // Bind freshness, then recheck the independently reviewed content.
    let tree = TreePatchPlan::review_with_max_total_bytes(
        &[source.to_owned(), package_path.to_owned()],
        destination,
        allowance,
    )?;
    original.verify(source, MAX_SOURCE_BYTES)?;
    if inspect_dreamcast_dcp(package_path).map_err(refuse)? != package {
        return Err(refuse("package changed during review"));
    }
    Ok(DreamcastDcpPlan {
        tree,
        source: source.to_owned(),
        package,
        original,
        expected,
        identity: binding.identity.clone(),
        boot_filename,
        allowance,
    })
}
impl DreamcastDcpPlan {
    pub fn max_total_bytes(&self) -> u64 {
        self.allowance
    }
    fn produce(&self, staging: &Path) -> io::Result<()> {
        if inspect_dreamcast_dcp(&self.package.path).map_err(refuse)? != self.package {
            return Err(refuse("DCP changed after review"));
        }
        self.original.verify(&self.source, MAX_SOURCE_BYTES)?;
        self.original.copy(&self.source, staging)?;
        let mut archive = zip::ZipArchive::new(File::open(&self.package.path)?).map_err(refuse)?;
        for entry in &self.package.entries {
            if entry.kind == DreamcastPatchEntryKind::Metadata {
                continue;
            }
            let mut bytes = Vec::new();
            archive
                .by_name(&entry.relative_path)
                .map_err(refuse)?
                .take(entry.size_bytes + 1)
                .read_to_end(&mut bytes)?;
            if Content::bytes(&bytes) != *self.expected.file(Path::new(&entry.relative_path))? {
                return Err(refuse("DCP member changed"));
            }
            fs::write(staging.join(&entry.relative_path), bytes)?;
        }
        Ok(())
    }
    fn verify(&self, staging: &Path) -> io::Result<()> {
        self.expected.verify(staging, self.allowance)?;
        if verify_ip(staging, &self.identity)? != self.boot_filename {
            return Err(refuse("Dreamcast boot mapping changed"));
        }
        Ok(())
    }
    pub fn prepare(&self) -> io::Result<PreparedTreePatch> {
        tree::prepare(
            &self.tree,
            |staging| self.produce(staging),
            |staging| self.verify(staging),
        )
    }
}

#[cfg(test)]
#[path = "dreamcast_dcp_apply_tests.rs"]
pub(crate) mod tests;
