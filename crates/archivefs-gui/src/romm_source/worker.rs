//! RomM background operation execution.
//!
//! Owns settings/cache I/O, provider requests, explicit file verification and
//! artwork/manual operations. The app supplies immutable request inputs,
//! cancellation and a progress callback; only redacted feature outcomes return.
//!
//! Scheduling, generation checks, selection guards and UI state remain in the
//! app shell. This module must not depend on `ArchiveFsApp` or root-scope helpers.
//! Source validation and mutation policy continue to be delegated to core.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use archivefs_core::Database;

use super::{
    RommConnectionSummary, RommImportSummary, RommOperation, RommOperationOutcome,
    RommProgressEvent, RommSnapshot, VerifyRommSummary,
};
use crate::{romm_browse, romm_config};

/// Reads authoritative RomM state: settings, cache status and artwork stats.
///
/// Contacts nothing. `reachable: false` is passed deliberately - the card must be
/// able to say what it knows without a network round trip, which is also why
/// opening the Sources page cannot start one.
pub(crate) fn load_romm_snapshot() -> Result<RommSnapshot, String> {
    load_romm_snapshot_at(&archivefs_core::identity_source::settings::default_identity_root()?)
}

/// Explicit storage scope for hermetic worker tests; production resolves it above.
fn load_romm_snapshot_at(identity_root: &Path) -> Result<RommSnapshot, String> {
    use archivefs_core::identity_source::artwork::ArtworkCache;
    use archivefs_core::identity_source::model::IdentityProvider;
    use archivefs_core::identity_source::settings::{SettingsLocation, load_token_file};
    use archivefs_core::identity_source::status::IdentitySourceApi;

    let settings = SettingsLocation::new(identity_root, IdentityProvider::Romm)
        .load()
        .map_err(|error| error.detail())?;
    let api = IdentitySourceApi::new(identity_root, IdentityProvider::Romm);
    // Explicit verifications, so a file that was hashed reads as Confirmed here and
    // not only in the panel that hashed it.
    let hashes = archivefs_core::identity_source::verification::VerificationStore::new(
        identity_root,
        IdentityProvider::Romm,
    )
    .load();
    let status = api.status(&settings.source, &hashes, false);
    let cache = api.open_cache(None).ok();
    let cache_format_version = cache.as_ref().map(|cache| cache.format_version);
    let (verify_summary, media_coverage, platform_media_coverage) = cache
        .as_ref()
        .map(|cache| {
            let counts = cache.counts();
            let verify_summary = VerifyRommSummary::from_counts(&counts);
            let media_coverage =
                archivefs_core::identity_source::model::MediaCoverage::from_counts(&counts);
            let platform_media_coverage =
                archivefs_core::identity_source::model::IdentityImportCounts::grouped_by_platform(
                    &cache.records,
                )
                .into_iter()
                .filter_map(|(platform, counts)| {
                    platform.map(|name| {
                        (
                            name,
                            archivefs_core::identity_source::model::MediaCoverage::from_counts(
                                &counts,
                            ),
                        )
                    })
                })
                .collect();
            (
                Some(verify_summary),
                Some(media_coverage),
                platform_media_coverage,
            )
        })
        .unwrap_or((None, None, std::collections::BTreeMap::new()));
    let server_id = status
        .server_id
        .clone()
        .unwrap_or_else(|| settings.source.url.clone());
    let artwork = ArtworkCache::new(identity_root, IdentityProvider::Romm).stats(&server_id);
    let token = load_token_file(settings.source.token_path.as_deref());
    Ok(RommSnapshot {
        settings,
        status,
        artwork,
        token_available: token.is_ok(),
        // The core's own refusal text, which never quotes the token.
        token_problem: token.err().map(|refusal| refusal.detail()),
        cache_format_version,
        verify_summary,
        media_coverage,
        platform_media_coverage,
    })
}

/// Runs one RomM operation to completion.
///
/// The error type is a plain `String` because every core refusal already renders
/// itself redacted; passing the refusal type up would only invite a caller to
/// format it some other way.
pub(crate) fn run_romm_operation(
    operation: &RommOperation,
    trusted_roots: &Result<Vec<PathBuf>, String>,
    database_path: Option<&Path>,
    generation: u64,
    cancellation: &Arc<AtomicBool>,
    report: &dyn Fn(RommProgressEvent),
) -> Result<RommOperationOutcome, String> {
    run_romm_operation_at(
        &archivefs_core::identity_source::settings::default_identity_root()?,
        operation,
        trusted_roots,
        database_path,
        generation,
        cancellation,
        report,
    )
}

/// Execute against an explicit store without consulting app state or changing process environment.
fn run_romm_operation_at(
    identity_root: &Path,
    operation: &RommOperation,
    trusted_roots: &Result<Vec<PathBuf>, String>,
    database_path: Option<&Path>,
    generation: u64,
    cancellation: &Arc<AtomicBool>,
    report: &dyn Fn(RommProgressEvent),
) -> Result<RommOperationOutcome, String> {
    use archivefs_core::identity_source::artwork::ArtworkCache;
    use archivefs_core::identity_source::hashing::LocalHashCache;
    use archivefs_core::identity_source::model::IdentityProvider;
    use archivefs_core::identity_source::romm::client::UreqTransport;
    use archivefs_core::identity_source::romm::import::ImportScope;
    use archivefs_core::identity_source::settings::{SettingsLocation, load_token_file};
    use archivefs_core::identity_source::status::{IdentitySourceApi, RefreshRequest};
    use archivefs_core::identity_source::verification::VerificationStore;

    let location = SettingsLocation::new(identity_root, IdentityProvider::Romm);
    let mut settings = location.load().map_err(|error| error.detail())?;
    let api = IdentitySourceApi::new(identity_root, IdentityProvider::Romm);

    // Enable and disable need no network and no token, so they are handled before
    // anything is validated.
    if let RommOperation::SetEnabled(enabled) = operation {
        if settings.source.url.trim().is_empty() {
            return Err(
                "Configure the RomM URL before enabling this source. The command line's \
                 `identity source romm configure` does this today; the dialog arrives in the next \
                 slice."
                    .to_string(),
            );
        }
        settings.source.enabled = *enabled;
        location.save(&settings).map_err(|error| error.detail())?;
        return Ok(RommOperationOutcome::Enabled(*enabled));
    }

    if let RommOperation::SaveConfiguration(proposed) = operation {
        // Validated again here, not merely in the dialog: the dialog's pass cannot
        // resolve a hostname, and the token file may have changed since it was
        // typed. This is the pass that decides.
        let mut proposed_settings = (**proposed).clone();
        proposed_settings.source.url = proposed_settings.source.url.trim().to_string();
        // A no-op Save is answered entirely from the already-loaded settings. It
        // neither rewrites the file nor resolves a hostname, reads a token, starts
        // an import, or constructs a transport.
        if proposed_settings == settings {
            return Ok(RommOperationOutcome::Saved(Box::new(proposed_settings)));
        }
        if proposed_settings.source.url.is_empty() {
            return Err("A RomM address is required.".to_string());
        }
        // The full local-only policy, with real name resolution.
        let approved = archivefs_core::identity_source::net_policy::validate_endpoint(
            &proposed_settings.source.url,
            &archivefs_core::identity_source::net_policy::SystemResolver,
        )
        .map_err(|refusal| refusal.detail())?;
        proposed_settings.source.url = approved.origin().to_string();
        if let Some(size) = proposed_settings.page_size
            && !(archivefs_core::identity_source::settings::MIN_CONFIGURED_PAGE_SIZE
                ..=archivefs_core::identity_source::settings::MAX_CONFIGURED_PAGE_SIZE)
                .contains(&size)
        {
            return Err(format!(
                "{size} records per request is outside the safe range."
            ));
        }
        // The token file is re-read, and only its verdict is kept.
        if let Some(path) = proposed_settings.source.token_path.clone() {
            load_token_file(Some(&path)).map_err(|refusal| refusal.detail())?;
        }
        let trusted_roots = trusted_roots.as_deref().map_err(Clone::clone)?;
        archivefs_core::identity_source::path_map::PathMappings::validate(
            &proposed_settings.source.mappings,
            trusted_roots,
            proposed_settings.source.provider_path_kind,
        )
        .map_err(|refusal| refusal.detail())?;
        if let Some(media_mapping) = proposed_settings.source.media_mapping.as_ref() {
            let validated =
                archivefs_core::identity_source::romm::media_mapping::validate_romm_media_mapping(
                    media_mapping,
                )
                .map_err(|error| error.to_string())?;
            proposed_settings.source.media_mapping = Some(
                archivefs_core::identity_source::romm::media_mapping::RommMediaMapping {
                    provider_prefix: validated.provider_prefix().to_string(),
                    local_root: validated.local_root().to_path_buf(),
                },
            );
        }
        // Atomic, and only after everything above agreed - so a refused save leaves
        // the previous configuration byte-identical.
        location
            .save(&proposed_settings)
            .map_err(|error| error.detail())?;
        return Ok(RommOperationOutcome::Saved(Box::new(proposed_settings)));
    }

    if let RommOperation::ClearArtwork = operation {
        let status = api.status(&settings.source, &LocalHashCache::new(), false);
        let server_id = status
            .server_id
            .clone()
            .unwrap_or_else(|| settings.source.url.clone());
        let cache = ArtworkCache::new(identity_root, IdentityProvider::Romm);
        let outcome = cache
            .clear(&server_id, true)
            .map_err(|refusal| refusal.detail())?;
        return Ok(RommOperationOutcome::ArtworkCleared {
            items: outcome.removed_items,
            bytes: outcome.removed_bytes,
        });
    }

    // Browsing the published cache needs no token and no network, so it is served
    // before anything is validated - which is what makes "no request is made merely
    // by browsing" structural rather than a promise.
    match operation {
        RommOperation::PlanMappings => {
            let cache = api.open_cache(None).map_err(|refusal| refusal.detail())?;
            let current = archivefs_core::identity_source::path_map::PathMappings::validate(
                &settings.source.mappings,
                &[],
                settings.source.provider_path_kind,
            )
            .map_err(|refusal| refusal.detail())?;
            let roots = trusted_roots.as_deref().map_err(Clone::clone)?;
            let plan =
                archivefs_core::identity_source::romm::mapping_plan::plan_mapping_reconciliation(
                    &cache, &current, roots,
                );
            return Ok(RommOperationOutcome::MappingPlan(Box::new(plan)));
        }
        RommOperation::CheckLinks { local_paths } => {
            let cache = api.open_cache(None).ok();
            let trusted_roots = trusted_roots.as_deref().map_err(Clone::clone)?;
            let mappings = archivefs_core::identity_source::path_map::PathMappings::validate(
                if cache.is_some() {
                    &settings.source.mappings
                } else {
                    &[]
                },
                trusted_roots,
                settings.source.provider_path_kind,
            )
            .map_err(|refusal| refusal.detail())?;
            let report = archivefs_core::identity_source::romm::linkage::inspect_local_paths(
                cache.as_ref(),
                &mappings,
                local_paths,
            );
            return Ok(RommOperationOutcome::Linkage(Box::new(report)));
        }
        RommOperation::LoadRecords {
            filters,
            offset,
            limit,
        } => {
            let cache = api.open_cache(None).map_err(|refusal| refusal.detail())?;
            let presence_for = |path: &Path| {
                archivefs_core::identity_source::matching::LocalPresence::observe(path)
            };
            return Ok(RommOperationOutcome::Records(Box::new(
                romm_browse::build_record_page(&cache, filters, *offset, *limit, &presence_for),
            )));
        }
        RommOperation::LoadRecordDetail { romm_game_id } => {
            let cache = api.open_cache(None).map_err(|refusal| refusal.detail())?;
            let presence_for = |path: &Path| {
                archivefs_core::identity_source::matching::LocalPresence::observe(path)
            };
            return Ok(RommOperationOutcome::RecordDetail(Box::new(
                romm_browse::build_record_detail(&cache, romm_game_id, &presence_for),
            )));
        }
        RommOperation::LoadConflicts { offset } => {
            let cache = api.open_cache(None).map_err(|refusal| refusal.detail())?;
            return Ok(RommOperationOutcome::Conflicts(Box::new(
                romm_browse::build_conflict_page(&cache, *offset, romm_browse::CONFLICT_PAGE_SIZE),
            )));
        }
        RommOperation::StaleSummary => {
            let cache = api.open_cache(None).map_err(|refusal| refusal.detail())?;
            let identity = romm_browse::CacheIdentity::of(&cache);
            let mappings: Vec<(String, String)> = settings
                .source
                .mappings
                .iter()
                .map(|mapping| {
                    (
                        mapping.provider_prefix.clone(),
                        mapping.archivefs_prefix.display().to_string(),
                    )
                })
                .collect();
            // Probing 10,081 paths takes noticeable time, so progress is reported and
            // cancellation is checked as it goes.
            let stale_total = cache
                .records
                .iter()
                .filter(|record| {
                    record.verification
                        == archivefs_core::identity_source::model::ExternalVerification::Stale
                })
                .count();
            let probed = std::cell::Cell::new(0usize);
            let cancelled = std::cell::Cell::new(false);
            let presence_for = |path: &Path| {
                if cancellation.load(Ordering::Acquire) {
                    cancelled.set(true);
                }
                let seen = archivefs_core::identity_source::matching::LocalPresence::observe(path);
                let done = probed.get() + 1;
                probed.set(done);
                // Reported in batches: one event per path would flood the channel for
                // no benefit at this scale.
                if done.is_multiple_of(250) || done == stale_total {
                    report(RommProgressEvent::StaleProgress {
                        probed: done,
                        total: stale_total,
                    });
                }
                seen
            };
            let summary = archivefs_core::identity_source::stale::StaleSummary::build(
                &cache,
                &mappings,
                archivefs_core::identity_source::stale::DEFAULT_EXAMPLES,
                presence_for,
            );
            if cancelled.get() || cancellation.load(Ordering::Acquire) {
                // A half-probed partition would read as a finding, so nothing is
                // returned rather than a partial one.
                return Err("The stale summary was cancelled. Nothing was changed.".to_string());
            }
            return Ok(RommOperationOutcome::Stale(Box::new(
                romm_browse::StaleSummaryView {
                    cache: identity,
                    summary,
                },
            )));
        }
        RommOperation::ResolveGame {
            local_path,
            local_platform,
            chosen_game_id,
        } => {
            let cache = api.open_cache(None).map_err(|refusal| refusal.detail())?;
            let verified = VerificationStore::new(identity_root, IdentityProvider::Romm).load();
            // Metadata only: no read, no hash. `observe` is the same call the import
            // makes, so the panel and the catalogue agree about what is at the path.
            let facts_for = |path: &Path| {
                archivefs_core::identity_source::matching::LocalFileFacts::observe(path)
            };
            return Ok(RommOperationOutcome::GameIdentity(Box::new(
                crate::romm_game::resolve_selected_game(
                    &cache,
                    local_path,
                    &verified,
                    local_platform,
                    chosen_game_id.as_deref(),
                    &facts_for,
                ),
            )));
        }
        RommOperation::VerifyLocalFile {
            local_path,
            romm_game_id,
            local_platform,
            chosen_game_id,
        } => {
            return verify_local_file(
                &api,
                identity_root,
                trusted_roots.as_deref().map_err(Clone::clone)?,
                local_path,
                romm_game_id,
                local_platform,
                chosen_game_id.as_deref(),
                cancellation,
                report,
            );
        }
        RommOperation::LoadCover {
            local_path,
            romm_game_id,
        } => {
            // A cover already in the cache needs no token and no request, so that case
            // is answered here, before anything is validated.
            if let Some(outcome) =
                cover_from_cache(&api, identity_root, &settings, local_path, romm_game_id)?
            {
                return Ok(RommOperationOutcome::Cover(Box::new(outcome)));
            }
        }
        RommOperation::LoadScreenshot {
            local_path,
            romm_game_id,
        } => {
            if let Some(outcome) =
                screenshot_from_cache(&api, identity_root, &settings, local_path, romm_game_id)?
            {
                return Ok(RommOperationOutcome::Screenshot(Box::new(outcome)));
            }
        }
        RommOperation::OpenManual {
            local_path,
            romm_game_id,
        } => {
            let path = open_romm_manual(&api, &settings, local_path, romm_game_id)?;
            return Ok(RommOperationOutcome::ManualOpened { path });
        }
        _ => {}
    }

    // Everything below talks to RomM, so it needs a validated source.
    let token = load_token_file(settings.source.token_path.as_deref())
        .map_err(|refusal| refusal.detail())?;
    let trusted_roots = trusted_roots.as_deref().map_err(Clone::clone)?;
    let source = archivefs_core::identity_source::romm::config::ValidatedRommSource::validate(
        &settings.source,
        &token,
        trusted_roots,
        &archivefs_core::identity_source::net_policy::SystemResolver,
    )
    .map_err(|refusal| refusal.detail())?;
    let transport = UreqTransport::new();

    // Placed before the connection pre-flight deliberately: fetching one cover should
    // cost one request, not two.
    if let RommOperation::LoadCover {
        local_path,
        romm_game_id,
    } = operation
    {
        return Ok(RommOperationOutcome::Cover(Box::new(fetch_cover(
            &api,
            identity_root,
            &source,
            &transport,
            local_path,
            romm_game_id,
            cancellation,
        )?)));
    }
    if let RommOperation::LoadScreenshot {
        local_path,
        romm_game_id,
    } = operation
    {
        return Ok(RommOperationOutcome::Screenshot(Box::new(
            fetch_screenshot(
                &api,
                identity_root,
                &source,
                &transport,
                local_path,
                romm_game_id,
                cancellation,
            )?,
        )));
    }

    let capability = api
        .test_connection(&source, &transport, Some(cancellation))
        .map_err(|error| error.detail())?;

    if let RommOperation::TestConnection = operation {
        // One record: enough to prove the token reads, and to see which path shape
        // this instance reports.
        let client =
            archivefs_core::identity_source::romm::client::RommClient::new(&source, &transport);
        let first_page = client.roms_page(1, 0, Some(cancellation));
        let observed = first_page
            .as_ref()
            .ok()
            .and_then(|page| page.items.first())
            .map(archivefs_core::identity_source::romm::normalise::provider_path_of)
            .filter(|path| !path.is_empty())
            .map(|path| {
                archivefs_core::identity_source::path_map::ProviderPathKind::observed_in(&path)
            });
        let reads = vec![
            (
                "/api/platforms".to_string(),
                client.platforms(Some(cancellation)).is_ok(),
            ),
            ("/api/roms".to_string(), first_page.is_ok()),
        ];
        return Ok(RommOperationOutcome::Connection(Box::new(
            RommConnectionSummary::from_report(
                &capability,
                settings.source.provider_path_kind.slug(),
                observed.map(|kind| kind.slug()),
                reads,
            ),
        )));
    }

    // A re-import must not undo a verification, so the stored hashes are fed into
    // matching exactly as a freshly computed one would be.
    let hashes = VerificationStore::new(identity_root, IdentityProvider::Romm).load();
    let trusted = archivefs_core::safe_read::TrustedRoots::from_paths(trusted_roots);
    let facts_for = |record: &archivefs_core::identity_source::model::ExternalIdentityRecord| {
        romm_local_facts(record, &trusted)
    };
    let on_progress = |progress| report(RommProgressEvent::Import(progress));
    let started = std::time::Instant::now();

    match operation {
        RommOperation::SampleImport { records } => {
            // A sample never publishes, so it is imported and matched here and then
            // simply reported. Nothing touches the live cache.
            let mut outcome = archivefs_core::identity_source::romm::import::import_identity(
                &source,
                &transport,
                ImportScope::Sample {
                    max_records: *records,
                },
                &capability,
                settings.effective_page_size(),
                on_progress,
                Some(cancellation),
            )
            .map_err(|failure| failure.detail())?;
            archivefs_core::identity_source::matching::match_all(
                &mut outcome.cache.records,
                &hashes,
                facts_for,
                Some(cancellation),
            )
            .map_err(|_| "The sample import was cancelled.".to_string())?;
            let counts = outcome.cache.counts();
            let groups =
                archivefs_core::identity_source::matching::build_groups(&outcome.cache.records);
            report_file_detail_omissions(report, &outcome.adaptive);
            Ok(RommOperationOutcome::Sample(Box::new(RommImportSummary {
                published: false,
                cache_path: None,
                cache_bytes: None,
                records: outcome.cache.records.len(),
                platforms: outcome.cache.platforms.len(),
                confirmed: counts.confirmed,
                strong: counts.strong,
                probable: counts.probable,
                ambiguous: counts.ambiguous,
                stale: counts.stale,
                unmatched: counts.unmatched,
                unknown_platforms: outcome.normalisation.unknown_platforms.len(),
                invalid_hashes: outcome.normalisation.rejected_hashes.len(),
                multi_file_groups: groups.len(),
                with_game_information: counts.with_game_information,
                game_information_failed: outcome.normalisation.skipped_records,
                pages_fetched: outcome.progress.pages_fetched,
                elapsed_milliseconds: started.elapsed().as_millis(),
                adaptive: Some(outcome.adaptive),
                failure: None,
                failure_code: None,
                previous_cache_usable: api.open_cache(None).is_ok(),
                platform_enrichment: None,
            })))
        }
        RommOperation::FullImport | RommOperation::Refresh => {
            let summary = api.refresh(
                RefreshRequest {
                    source: &source,
                    transport: &transport,
                    scope: ImportScope::Full,
                    capability: &capability,
                    hashes: &hashes,
                    page_size: settings.effective_page_size(),
                    cancel: Some(cancellation),
                    import_timeout: settings.effective_import_timeout(),
                },
                facts_for,
                on_progress,
            );
            match summary {
                Ok(summary) => {
                    if cancellation.load(Ordering::Acquire) {
                        return Err(
                            "The import was cancelled before platform metadata was updated."
                                .to_string(),
                        );
                    }
                    let platform_enrichment = if let Some(database_path) = database_path
                        && database_path.is_file()
                    {
                        let enrichment = api
                            .open_cache(None)
                            .map_err(|error| error.detail())
                            .and_then(|cache| {
                                let mut database = Database::open_or_create(database_path)
                                    .map_err(|error| error.to_string())?;
                                database
                                    .enrich_platforms_from_romm_cache(&cache, generation)
                                    .map_err(|error| error.to_string())
                            });
                        match enrichment {
                            Ok(enrichment) => {
                                report(RommProgressEvent::Note(format!(
                                    "Platform identity enrichment: {} applied, {} already current, {} manual assignment(s) preserved, {} conflict(s) require review.",
                                    enrichment.applied,
                                    enrichment.unchanged,
                                    enrichment.manual_preserved,
                                    enrichment.conflicts,
                                )));
                                Some(Box::new(enrichment))
                            }
                            Err(error) => {
                                report(RommProgressEvent::Note(format!(
                                    "RomM identity was published, but platform metadata could not be updated: {error}"
                                )));
                                None
                            }
                        }
                    } else {
                        None
                    };
                    report_file_detail_omissions(report, &summary.adaptive);
                    let cache_bytes = std::fs::metadata(&summary.cache_path)
                        .ok()
                        .map(|metadata| metadata.len());
                    Ok(RommOperationOutcome::Import(Box::new(RommImportSummary {
                        published: true,
                        cache_path: Some(summary.cache_path.clone()),
                        cache_bytes,
                        records: summary.records,
                        platforms: summary.platforms,
                        confirmed: summary.counts.confirmed,
                        strong: summary.counts.strong,
                        probable: summary.counts.probable,
                        ambiguous: summary.counts.ambiguous,
                        stale: summary.counts.stale,
                        unmatched: summary.counts.unmatched,
                        unknown_platforms: summary.unknown_platforms,
                        invalid_hashes: summary.invalid_hashes,
                        multi_file_groups: summary.groups.len(),
                        with_game_information: summary.counts.with_game_information,
                        game_information_failed: summary.game_information_failed,
                        pages_fetched: summary.progress.pages_fetched,
                        elapsed_milliseconds: started.elapsed().as_millis(),
                        adaptive: Some(summary.adaptive),
                        failure: None,
                        failure_code: None,
                        previous_cache_usable: true,
                        platform_enrichment,
                    })))
                }
                Err(failure) => Err(failure.detail()),
            }
        }
        RommOperation::Preview { limit } => {
            let summary = run_romm_preview(
                &api,
                &source,
                &transport,
                &settings,
                trusted_roots,
                *limit,
                cancellation,
            )?;
            Ok(RommOperationOutcome::Preview(Box::new(summary)))
        }
        // Handled above.
        RommOperation::LoadStatus
        | RommOperation::TestConnection
        | RommOperation::SetEnabled(_)
        | RommOperation::ClearArtwork
        | RommOperation::SaveConfiguration(_)
        | RommOperation::LoadRecords { .. }
        | RommOperation::LoadRecordDetail { .. }
        | RommOperation::LoadConflicts { .. }
        | RommOperation::StaleSummary
        | RommOperation::ResolveGame { .. }
        | RommOperation::VerifyLocalFile { .. }
        | RommOperation::LoadCover { .. }
        | RommOperation::CheckLinks { .. }
        | RommOperation::PlanMappings => unreachable!("handled before this match"),
        RommOperation::LoadScreenshot { .. } => unreachable!("handled before this match"),
        RommOperation::OpenManual { .. } => unreachable!("handled before this match"),
    }
}

/// Turns a file-detail omission into a sentence a person can act on.
fn report_file_detail_omissions(
    report: &dyn Fn(RommProgressEvent),
    adaptive: &archivefs_core::identity_source::romm::import::AdaptivePagination,
) {
    if adaptive.records_without_file_detail.is_empty() {
        return;
    }
    report(RommProgressEvent::Note(format!(
        "Game identity imported. Detailed file list omitted for RomM id {} because the provider \
         response exceeded the safety limit.",
        adaptive
            .records_without_file_detail
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    )));
}

/// Local facts for one record: metadata only, and never a hash.
///
/// The same shape the CLI uses, so the GUI and the command line reach the same
/// verdicts from the same evidence.
/// Refuses a path that is not a regular file inside a configured source folder.
///
/// `TrustedRoots` governs what a symlink may point *at*, not which path may be named,
/// so this is the check that stops an explicit verification reading a file outside the
/// library. Both the named path and its resolved form must be inside a root.
fn confine_to_source_roots(path: &Path, roots: &[PathBuf]) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!(
            "{} is not an absolute path, so which file is meant is not certain.",
            path.display()
        ));
    }
    // Canonical roots, so a symlinked source folder does not defeat the comparison. A
    // root that cannot be resolved is dropped rather than trusted.
    let canonical_roots: Vec<PathBuf> = roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .collect();
    let inside = |candidate: &Path| {
        canonical_roots
            .iter()
            .any(|root| candidate.starts_with(root))
    };
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("{} cannot be examined: {error}", path.display()))?;
    // Checked before resolution, so the verdict describes the path that was named.
    let lexical = path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .map(|parent| parent.join(path.file_name().unwrap_or_default()));
    if !lexical.as_deref().is_some_and(&inside) {
        return Err(format!(
            "{} is not inside a configured source folder, so EmuWiz will not read it.",
            path.display()
        ));
    }
    let resolved = path.canonicalize().map_err(|error| {
        if metadata.file_type().is_symlink() {
            format!(
                "{} is a symbolic link whose target cannot be resolved: {error}",
                path.display()
            )
        } else {
            format!("{} cannot be resolved: {error}", path.display())
        }
    })?;
    if !inside(&resolved) {
        return Err(format!(
            "{} leads out of your configured source folders; EmuWiz will not follow it.",
            path.display()
        ));
    }
    if !resolved.is_file() {
        return Err(format!(
            "{} is not a regular file, so there are no bytes to hash.",
            path.display()
        ));
    }
    Ok(resolved)
}

/// Hashes one local file, compares it with one RomM record, and records the result.
///
/// The verdict is recomputed by the same matcher the import uses, before and after the
/// hash is stored - so Confirmed is something the comparison earned rather than a
/// label this function applies.
#[allow(clippy::too_many_arguments)]
fn verify_local_file(
    api: &archivefs_core::identity_source::status::IdentitySourceApi,
    identity_root: &Path,
    roots: &[PathBuf],
    local_path: &Path,
    romm_game_id: &str,
    local_platform: &crate::romm_game::LocalPlatformClaim,
    chosen_game_id: Option<&str>,
    cancellation: &Arc<AtomicBool>,
    report: &dyn Fn(RommProgressEvent),
) -> Result<RommOperationOutcome, String> {
    use archivefs_core::identity_source::hashing::hash_file_reporting;
    use archivefs_core::identity_source::model::IdentityProvider;
    use archivefs_core::identity_source::verification::VerificationStore;
    use archivefs_core::safe_read::TrustedRoots;

    let cache = api.open_cache(None).map_err(|refusal| refusal.detail())?;
    let record = cache
        .records
        .iter()
        .find(|record| {
            record.provider_game_id == romm_game_id
                && record.archivefs_path.as_deref() == Some(local_path)
        })
        .ok_or_else(|| {
            "That RomM record no longer maps to this file. Look the game up again.".to_string()
        })?
        .clone();
    if record.hashes.is_empty() {
        return Err(
            "RomM published no hash for this game, so hashing the file would produce nothing to \
             compare it against."
                .to_string(),
        );
    }

    // Both checks: this one decides which path may be named, `TrustedRoots` below
    // decides what a symlink may point at.
    confine_to_source_roots(local_path, roots)?;
    let trusted = TrustedRoots::from_paths(roots);

    let store = VerificationStore::new(identity_root, IdentityProvider::Romm);
    let before_hashes = store.load();
    let facts_for =
        |path: &Path| archivefs_core::identity_source::matching::LocalFileFacts::observe(path);
    let before = crate::romm_game::resolve_selected_game(
        &cache,
        local_path,
        &before_hashes,
        local_platform,
        chosen_game_id.or(Some(romm_game_id)),
        &facts_for,
    );
    let verdict_before = before
        .chosen_candidate()
        .map(|candidate| candidate.verdict)
        .unwrap_or(before.verdict);

    let started = std::time::Instant::now();
    let file_label = local_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| local_path.display().to_string());
    let progress_label = file_label.clone();
    let on_progress = |progress: archivefs_core::identity_source::hashing::HashProgress| {
        report(RommProgressEvent::Hashing(
            crate::romm_game::HashProgressView {
                file_label: progress_label.clone(),
                bytes_read: progress.bytes_read,
                total_bytes: progress.total_bytes,
                elapsed_seconds: started.elapsed().as_secs(),
                cancellation_requested: cancellation.load(Ordering::Acquire),
            },
        ));
    };
    let hashes = hash_file_reporting(local_path, &trusted, Some(cancellation), &on_progress)
        .map_err(|refusal| refusal.detail())?;
    let elapsed_seconds = started.elapsed().as_secs();

    let comparisons = crate::romm_game::compare_hashes(&record, &hashes);
    let all_agree = !comparisons.is_empty() && comparisons.iter().all(|line| line.agrees);
    let any_disagree = comparisons.iter().any(|line| !line.agrees);

    // Stored whether or not it agreed: the hash is a fact about the file, and storing a
    // disagreement is what keeps it visible instead of inviting a second read.
    let after_hashes = store
        .record(&record.server_id, hashes.clone())
        .map_err(|error| error.detail())?;
    let stored_at = Some(store.path());

    let after = crate::romm_game::resolve_selected_game(
        &cache,
        local_path,
        &after_hashes,
        local_platform,
        chosen_game_id.or(Some(romm_game_id)),
        &facts_for,
    );
    let verdict_after = after
        .chosen_candidate()
        .map(|candidate| candidate.verdict)
        .unwrap_or(after.verdict);
    let compact_label = after
        .chosen_candidate()
        .map(crate::romm_game::CandidateView::compact_label)
        .unwrap_or_else(|| file_label.clone());

    Ok(RommOperationOutcome::Verified(Box::new(
        crate::romm_game::VerificationOutcomeView {
            local_path: local_path.to_path_buf(),
            file_label,
            compact_label,
            romm_game_id: romm_game_id.to_string(),
            comparisons,
            all_agree,
            any_disagree,
            verdict_before,
            verdict_after,
            bytes_hashed: hashes.bytes_hashed,
            elapsed_seconds,
            stored_at,
            panel: Box::new(after),
        },
    )))
}

/// The record one cover request is about, and its artwork request.
fn cover_record(
    api: &archivefs_core::identity_source::status::IdentitySourceApi,
    local_path: &Path,
    romm_game_id: &str,
) -> Result<archivefs_core::identity_source::model::ExternalIdentityRecord, String> {
    let cache = api.open_cache(None).map_err(|refusal| refusal.detail())?;
    cache
        .records
        .iter()
        .find(|record| {
            record.provider_game_id == romm_game_id
                && record.archivefs_path.as_deref() == Some(local_path)
        })
        .cloned()
        .ok_or_else(|| {
            "That RomM record no longer maps to this file. Look the game up again.".to_string()
        })
}

fn open_romm_manual(
    api: &archivefs_core::identity_source::status::IdentitySourceApi,
    settings: &archivefs_core::identity_source::settings::ProviderSettings,
    local_path: &Path,
    romm_game_id: &str,
) -> Result<PathBuf, String> {
    let record = cover_record(api, local_path, romm_game_id)?;
    let manual = record
        .artwork
        .as_ref()
        .and_then(|artwork| artwork.manual.as_ref())
        .ok_or_else(|| "No manual is available for this RomM record.".to_string())?;
    let mapping = settings
        .source
        .media_mapping
        .as_ref()
        .map(archivefs_core::identity_source::romm::media_mapping::validate_romm_media_mapping)
        .transpose()
        .map_err(|error| error.to_string())?;
    archivefs_core::identity_source::romm::manual::open_local_romm_manual(
        mapping.as_ref(),
        manual,
        &archivefs_core::identity_source::romm::manual::DesktopManualOpener,
    )
    .map_err(|error| error.to_string())
}

/// Answers a cover request without contacting anything, when it can.
///
/// Returns `None` only when a real fetch is needed.
fn cover_from_cache(
    api: &archivefs_core::identity_source::status::IdentitySourceApi,
    identity_root: &Path,
    settings: &archivefs_core::identity_source::settings::ProviderSettings,
    local_path: &Path,
    romm_game_id: &str,
) -> Result<Option<crate::romm_game::CoverOutcome>, String> {
    use archivefs_core::identity_source::artwork::{ArtworkCache, ArtworkRequest};
    use archivefs_core::identity_source::hashing::LocalHashCache;
    use archivefs_core::identity_source::model::IdentityProvider;

    let record = cover_record(api, local_path, romm_game_id)?;
    let availability = crate::romm_game::availability_of(&record);
    let cache = ArtworkCache::new(identity_root, IdentityProvider::Romm);
    let status = api.status(&settings.source, &LocalHashCache::new(), false);
    let server_id = status
        .server_id
        .clone()
        .unwrap_or_else(|| settings.source.url.clone());
    let stats = cache.stats(&server_id);
    let finish = |state: crate::romm_game::CoverState| crate::romm_game::CoverOutcome {
        local_path: local_path.to_path_buf(),
        romm_game_id: romm_game_id.to_string(),
        state,
        cached_items: stats.items as u64,
        cached_bytes: stats.bytes,
    };

    if availability != crate::romm_game::ArtworkAvailability::Fetchable {
        // RomM recorded no cover of its own. `url_cover` points at IGDB or
        // RetroAchievements, and this build does not fetch from public hosts.
        return Ok(Some(finish(crate::romm_game::CoverState::Unavailable(
            availability,
        ))));
    }
    let request = ArtworkRequest::from_record(&record);
    match cache.lookup(&server_id, &request) {
        Some(thumbnail) => {
            let state = match crate::romm_game::decode_thumbnail(&thumbnail, true) {
                Ok(image) => crate::romm_game::CoverState::Ready(Box::new(image)),
                Err(detail) => crate::romm_game::CoverState::Failed(detail),
            };
            Ok(Some(finish(state)))
        }
        None => Ok(None),
    }
}

/// Fetches one cover from RomM's own small-cover path.
fn fetch_cover(
    api: &archivefs_core::identity_source::status::IdentitySourceApi,
    identity_root: &Path,
    source: &archivefs_core::identity_source::romm::config::ValidatedRommSource,
    transport: &archivefs_core::identity_source::romm::client::UreqTransport,
    local_path: &Path,
    romm_game_id: &str,
    cancellation: &Arc<AtomicBool>,
) -> Result<crate::romm_game::CoverOutcome, String> {
    use archivefs_core::identity_source::artwork::{ArtworkCache, ArtworkRefusal, ArtworkRequest};
    use archivefs_core::identity_source::model::IdentityProvider;

    let record = cover_record(api, local_path, romm_game_id)?;
    let cache = ArtworkCache::new(identity_root, IdentityProvider::Romm);
    let request = ArtworkRequest::from_record(&record);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default();
    let state = match cache.fetch(source, transport, &request, now, Some(cancellation)) {
        Ok(thumbnail) => match crate::romm_game::decode_thumbnail(&thumbnail, false) {
            Ok(image) => crate::romm_game::CoverState::Ready(Box::new(image)),
            Err(detail) => crate::romm_game::CoverState::Failed(detail),
        },
        Err(ArtworkRefusal::Cancelled) => crate::romm_game::CoverState::Cancelled,
        Err(ArtworkRefusal::Request(
            archivefs_core::identity_source::romm::client::RommRequestError::Transport { detail },
        )) => crate::romm_game::CoverState::Offline(detail),
        Err(ArtworkRefusal::Request(
            archivefs_core::identity_source::romm::client::RommRequestError::Timeout,
        )) => crate::romm_game::CoverState::Offline("RomM did not answer in time".to_string()),
        Err(
            refusal @ (ArtworkRefusal::TooLarge { .. }
            | ArtworkRefusal::NotAnImage { .. }
            | ArtworkRefusal::DimensionsTooLarge { .. }
            | ArtworkRefusal::DecodeFailed
            | ArtworkRefusal::WriteFailed { .. }
            | ArtworkRefusal::CacheUnusable { .. }),
        ) => crate::romm_game::CoverState::Failed(refusal.detail()),
        // The core's own wording, which never contains a URL or a token.
        Err(refusal) => crate::romm_game::CoverState::Refused(refusal.detail()),
    };
    // Read after the fetch rather than incremented, so a clear that ran alongside it
    // cannot leave a figure on screen that was never true.
    let stats = cache.stats(source.server_id());
    Ok(crate::romm_game::CoverOutcome {
        local_path: local_path.to_path_buf(),
        romm_game_id: romm_game_id.to_string(),
        state,
        cached_items: stats.items as u64,
        cached_bytes: stats.bytes,
    })
}

fn screenshot_from_cache(
    api: &archivefs_core::identity_source::status::IdentitySourceApi,
    identity_root: &Path,
    settings: &archivefs_core::identity_source::settings::ProviderSettings,
    local_path: &Path,
    romm_game_id: &str,
) -> Result<Option<crate::romm_game::CoverOutcome>, String> {
    use archivefs_core::identity_source::artwork::{ArtworkCache, ArtworkRequest};
    use archivefs_core::identity_source::hashing::LocalHashCache;
    use archivefs_core::identity_source::model::IdentityProvider;

    let record = cover_record(api, local_path, romm_game_id)?;
    let Some(media) = record
        .artwork
        .as_ref()
        .and_then(|artwork| artwork.screenshots.first())
    else {
        return Ok(Some(crate::romm_game::CoverOutcome {
            local_path: local_path.to_path_buf(),
            romm_game_id: romm_game_id.to_string(),
            state: crate::romm_game::CoverState::Failed("No screenshot is available.".to_string()),
            cached_items: 0,
            cached_bytes: 0,
        }));
    };
    let cache = ArtworkCache::new(identity_root, IdentityProvider::Romm);
    let status = api.status(&settings.source, &LocalHashCache::new(), false);
    let server_id = status
        .server_id
        .clone()
        .unwrap_or_else(|| settings.source.url.clone());
    let stats = cache.stats(&server_id);
    let request = ArtworkRequest::from_media(&record.provider_game_id, media);
    let Some(thumbnail) = cache.lookup(&server_id, &request) else {
        return Ok(None);
    };
    let state = crate::romm_game::decode_thumbnail(&thumbnail, true)
        .map(|image| crate::romm_game::CoverState::Ready(Box::new(image)))
        .unwrap_or_else(crate::romm_game::CoverState::Failed);
    Ok(Some(crate::romm_game::CoverOutcome {
        local_path: local_path.to_path_buf(),
        romm_game_id: romm_game_id.to_string(),
        state,
        cached_items: stats.items as u64,
        cached_bytes: stats.bytes,
    }))
}

fn fetch_screenshot(
    api: &archivefs_core::identity_source::status::IdentitySourceApi,
    identity_root: &Path,
    source: &archivefs_core::identity_source::romm::config::ValidatedRommSource,
    transport: &archivefs_core::identity_source::romm::client::UreqTransport,
    local_path: &Path,
    romm_game_id: &str,
    cancellation: &Arc<AtomicBool>,
) -> Result<crate::romm_game::CoverOutcome, String> {
    use archivefs_core::identity_source::artwork::{ArtworkCache, ArtworkRefusal, ArtworkRequest};
    use archivefs_core::identity_source::model::IdentityProvider;
    let record = cover_record(api, local_path, romm_game_id)?;
    let media = record
        .artwork
        .as_ref()
        .and_then(|artwork| artwork.screenshots.first())
        .ok_or_else(|| "No screenshot is available.".to_string())?;
    let cache = ArtworkCache::new(identity_root, IdentityProvider::Romm);
    let request = ArtworkRequest::from_media(&record.provider_game_id, media);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default();
    let state = match cache.fetch(source, transport, &request, now, Some(cancellation)) {
        Ok(thumbnail) => crate::romm_game::decode_thumbnail(&thumbnail, false)
            .map(|image| crate::romm_game::CoverState::Ready(Box::new(image)))
            .unwrap_or_else(crate::romm_game::CoverState::Failed),
        Err(ArtworkRefusal::Cancelled) => crate::romm_game::CoverState::Cancelled,
        Err(refusal) => crate::romm_game::CoverState::Failed(refusal.detail()),
    };
    let stats = cache.stats(source.server_id());
    Ok(crate::romm_game::CoverOutcome {
        local_path: local_path.to_path_buf(),
        romm_game_id: romm_game_id.to_string(),
        state,
        cached_items: stats.items as u64,
        cached_bytes: stats.bytes,
    })
}

fn romm_local_facts(
    record: &archivefs_core::identity_source::model::ExternalIdentityRecord,
    _trusted: &archivefs_core::safe_read::TrustedRoots,
) -> archivefs_core::identity_source::matching::LocalFileFacts {
    use archivefs_core::identity_source::matching::LocalFileFacts;
    use archivefs_core::identity_source::model::LocalEvidenceStrength;
    match record.archivefs_path.as_deref() {
        Some(path) => {
            let local = archivefs_core::platform::detect::platform_for_folder_name(
                path.parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    .unwrap_or(""),
            )
            .map(|platform| platform.id);
            LocalFileFacts::observe(path).with_local_platform(
                local,
                if local.is_some() {
                    LocalEvidenceStrength::Weak
                } else {
                    LocalEvidenceStrength::None
                },
            )
        }
        None => LocalFileFacts::default(),
    }
}

/// Translates a bounded sample of provider paths and reports what each becomes.
///
/// Prefers the published cache, because previewing against records that were really
/// imported costs nothing and needs no network. Only when there is no cache does it
/// ask RomM, and then for one bounded page.
///
/// Publishes nothing, writes nothing, and reads only file *metadata* - the presence
/// probe never opens a file.
fn run_romm_preview(
    api: &archivefs_core::identity_source::status::IdentitySourceApi,
    source: &archivefs_core::identity_source::romm::config::ValidatedRommSource,
    transport: &archivefs_core::identity_source::romm::client::UreqTransport,
    settings: &archivefs_core::identity_source::settings::ProviderSettings,
    trusted_roots: &[PathBuf],
    limit: usize,
    cancellation: &Arc<AtomicBool>,
) -> Result<romm_config::RommPreviewSummary, String> {
    use archivefs_core::identity_source::matching::LocalPresence;
    use archivefs_core::identity_source::path_map::{MappingPreview, PathMappings};

    let limit = limit.clamp(1, romm_config::MAX_PREVIEW_LIMIT);
    let engine = PathMappings::validate(
        &settings.source.mappings,
        trusted_roots,
        settings.source.provider_path_kind,
    )
    .map_err(|refusal| refusal.detail())?;

    let (samples, platforms, sample_source) = match api.open_cache(None) {
        Ok(cache) => {
            let samples: Vec<String> = cache
                .records
                .iter()
                .take(limit)
                .map(|record| record.provider_path.clone())
                .collect();
            let platforms: Vec<Option<String>> = cache
                .records
                .iter()
                .take(limit)
                .map(|record| record.platform_candidate.clone())
                .collect();
            (samples, platforms, "the published identity cache")
        }
        Err(_) => {
            let client =
                archivefs_core::identity_source::romm::client::RommClient::new(source, transport);
            let page = client
                .roms_page(u32::try_from(limit).unwrap_or(20), 0, Some(cancellation))
                .map_err(|error| error.detail())?;
            let samples: Vec<String> = page
                .items
                .iter()
                .map(archivefs_core::identity_source::romm::normalise::provider_path_of)
                .filter(|path| !path.is_empty())
                .collect();
            let platforms: Vec<Option<String>> = page
                .items
                .iter()
                .map(|item| {
                    item.get("platform_slug")
                        .and_then(|value| value.as_str())
                        .and_then(
                            archivefs_core::identity_source::romm::normalise::canonical_platform_for_romm_slug,
                        )
                        .map(str::to_string)
                })
                .collect();
            (samples, platforms, "a bounded RomM sample")
        }
    };
    if cancellation.load(Ordering::Acquire) {
        return Err("The preview was cancelled.".to_string());
    }

    let preview = MappingPreview::build(&engine, &samples);
    let presence_for = |path: &Path| LocalPresence::observe(path).code();
    let examples: Vec<romm_config::PreviewExampleView> = preview
        .translations
        .iter()
        .enumerate()
        .map(|(index, translation)| {
            romm_config::preview_example(
                translation,
                platforms.get(index).cloned().flatten(),
                &presence_for,
            )
        })
        .collect();
    Ok(romm_config::summarise_preview(
        examples,
        settings.source.provider_path_kind,
        preview.observed_relative,
        preview.observed_absolute,
        sample_source,
    ))
}

#[cfg(test)]
mod tests;
