use archivefs_core::game_identity::IdentityPlatform;
use archivefs_core::platform::{
    platform_by_id, platform_for_alias, platforms_with_strong_extension,
};

#[test]
fn enterprise_identity_uses_one_family_with_explicit_model_aliases() {
    for alias in [
        "Enterprise",
        "Enterprise 64",
        "EP64",
        "Enterprise 128",
        "EP128",
    ] {
        assert_eq!(
            IdentityPlatform::from_catalogue(Some(alias)),
            IdentityPlatform::Enterprise,
            "{alias} must resolve to the Enterprise family"
        );
    }
}

#[test]
fn enterprise_folder_aliases_are_safe_context_not_media_proof() {
    for alias in [
        "enterprise",
        "enterprise64",
        "enterprise128",
        "ep64",
        "ep128",
    ] {
        assert_eq!(
            platform_for_alias(alias).map(|platform| platform.id),
            Some("Enterprise"),
            "{alias} must resolve to the canonical Enterprise registry row"
        );
    }

    let platform = platform_by_id("Enterprise").expect("Enterprise registry row");
    assert!(platform.strong_extensions.is_empty());
    for extension in ["tap", "dtf", "dsk", "img", "rom"] {
        assert!(
            platform.weak_extensions.contains(&extension),
            ".{extension} remains a weak Enterprise candidate only"
        );
        assert!(
            platforms_with_strong_extension(extension).is_empty(),
            ".{extension} must not establish Enterprise identity by itself"
        );
    }
}

#[test]
fn enterprise_media_formats_remain_deferred_without_structural_or_dat_evidence() {
    let platform = platform_by_id("Enterprise").expect("Enterprise registry row");
    assert!(platform.magic.is_empty());
    assert!(!platform.weak_extensions.contains(&"cas"));
    assert!(!platform.weak_extensions.contains(&"sna"));
    assert!(!platform.weak_extensions.contains(&"vhd"));
}
