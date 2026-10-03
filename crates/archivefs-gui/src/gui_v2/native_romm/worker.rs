//! Glue to the promoted core browser. Runs only on the existing GUI worker.
use super::{PAGE_SIZE, Problem, Reply, Request};
use archivefs_core::identity_source::{
    artwork::{ArtworkCache, ArtworkRequest},
    model::IdentityProvider,
    net_policy::{EndpointRefusal, HostResolver, SystemResolver},
    romm::{
        browser::{RommBrowseError, RommBrowser, RommServerStatus},
        client::{RommTransport, UreqTransport},
        config::{ConfigRefusal, ValidatedRommSource},
    },
    settings::{SettingsLocation, load_token_file},
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) fn run(
    request: Request,
    roots: &[PathBuf],
    cancel: &AtomicBool,
) -> Result<Reply, Problem> {
    let root = archivefs_core::identity_source::settings::default_identity_root()
        .map_err(|_| Problem::Settings)?;
    run_with(
        &root,
        request,
        roots,
        &SystemResolver,
        &UreqTransport::new(),
        cancel,
    )
}

pub(super) fn run_with<T: RommTransport>(
    root: &Path,
    request: Request,
    roots: &[PathBuf],
    resolver: &impl HostResolver,
    transport: &T,
    cancel: &AtomicBool,
) -> Result<Reply, Problem> {
    if cancel.load(Ordering::Acquire) {
        return Err(Problem::Backend(RommBrowseError::Cancelled));
    }
    let location = SettingsLocation::new(root, IdentityProvider::Romm);
    if std::fs::metadata(location.config_path()).is_ok_and(|m| m.len() > 1024 * 1024) {
        return Err(Problem::Settings);
    }
    let settings = location.load().map_err(|_| Problem::Settings)?;
    if matches!(request, Request::Settings) {
        let token = load_token_file(settings.source.token_path.as_deref()).ok();
        return Ok(Reply::Settings(Box::new(settings), token));
    }
    if settings.source.url.trim().is_empty() {
        return Err(Problem::NotConfigured);
    }
    if !settings.source.enabled {
        return Err(Problem::Disabled);
    }
    let token =
        load_token_file(settings.source.token_path.as_deref()).map_err(|_| Problem::Credentials)?;
    let source = ValidatedRommSource::validate(&settings.source, &token, roots, resolver)
        .map_err(config_problem)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| i64::try_from(t.as_secs()).unwrap_or(i64::MAX));
    if let Request::Cover(game) = request {
        // The established core cache owns fetch policy, byte/image bounds and storage.
        let cache = ArtworkCache::new(root, IdentityProvider::Romm);
        let mut request = ArtworkRequest::from_record(&game.identity);
        if request.small_reference.is_some() {
            request.large_reference = None;
        }
        let image = cache
            .fetch(&source, transport, &request, now, Some(cancel))
            .ok()
            .and_then(|thumbnail| crate::romm_game::decode_thumbnail(&thumbnail, true).ok())
            .map(|cover| cover.image);
        return Ok(Reply::Cover { id: game.id, image });
    }
    let mut browser = RommBrowser::new(&source, transport, now);
    let info = browser.discover(Some(cancel));
    if matches!(request, Request::Connect) {
        if !matches!(
            info.status,
            RommServerStatus::Supported | RommServerStatus::PartiallySupported
        ) {
            return Ok(Reply::Connection {
                info,
                platforms: vec![],
                page: None,
                token: source.token().clone(),
            });
        }
        let platforms = if info.capabilities.platforms {
            browser.platforms(Some(cancel)).map_err(Problem::Backend)?
        } else {
            vec![]
        };
        let page = if info.capabilities.games {
            Some(
                browser
                    .games(0, PAGE_SIZE, &Default::default(), Some(cancel))
                    .map_err(Problem::Backend)?,
            )
        } else {
            None
        };
        return Ok(Reply::Connection {
            info,
            platforms,
            page,
            token: source.token().clone(),
        });
    }
    if !matches!(
        info.status,
        RommServerStatus::Supported | RommServerStatus::PartiallySupported
    ) {
        return Err(Problem::Backend(
            info.error.unwrap_or(RommBrowseError::UnsupportedCapability),
        ));
    }
    match request {
        Request::Games { offset, filter } => browser
            .games(offset, PAGE_SIZE, &filter, Some(cancel))
            .map(|page| Reply::Page(page, source.token().clone()))
            .map_err(Problem::Backend),
        Request::Detail(id) => browser
            .game_detail(id, Some(cancel))
            .map(|detail| Reply::Detail(detail, source.token().clone()))
            .map_err(Problem::Backend),
        Request::Settings | Request::Connect | Request::Cover(_) => unreachable!(),
    }
}

pub(super) fn config_problem(error: ConfigRefusal) -> Problem {
    match error {
        ConfigRefusal::Endpoint(
            EndpointRefusal::UnresolvableHost { .. } | EndpointRefusal::NoAddresses,
        ) => Problem::NameLookup,
        ConfigRefusal::Endpoint(_) => Problem::Backend(RommBrowseError::EndpointRefused),
        ConfigRefusal::Token(_) => Problem::Credentials,
        ConfigRefusal::Disabled => Problem::Disabled,
        _ => Problem::Settings,
    }
}
