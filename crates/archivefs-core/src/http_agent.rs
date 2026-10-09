//! Internal construction boundary for production HTTP agents.
//!
//! EmuWiz does not inherit proxy configuration from the process environment.
//! Transports keep their TLS, redirect, timeout and address policies at their
//! call sites; this module applies the proxy opt-out after that configuration.
//! The sealed configuration cannot be changed before it becomes an agent.
//!
//! Any future proxy support requires an explicit EmuWiz user setting, must be
//! transport-scoped and disabled by default, and must document its privacy
//! implications. This module deliberately provides no proxy support.
//!
//! Reviewed production inventory (14 sites in 13 files):
//! - `dat::updates::HttpsManagedDatTransport::new`
//! - `emulator_download::HttpsEmulatorDownloadTransport::new`
//! - `emulator_update::HttpsUpdateDownloader::download`
//! - `emulator_update::OfficialMetadataProvider::default`
//! - `homebrew_github::UreqGithubTransport::new`
//! - `identity_source::hasheous::client::UreqTransport::new`
//! - `identity_source::managed_snapshot::HttpsManagedSourceTransport::default`
//! - `identity_source::romm::client::UreqTransport::new`
//! - `identity_source::screenscraper::UreqTransport::new`
//! - `mod_download_transport::UreqModDownloadBackend::with_resolver`
//! - `moddb::UreqModDbTransport::default`
//! - `patch_manager::cheat_sources::HttpsCheatSourceTransport::new`
//! - `patch_manager::gamehacking_shared::UreqGameHackingTransport::new`
//! - `patch_manager::retrieval::HttpsMetadataFetcher::new`
//!
//! This is an architectural boundary, not a compiler-enforced ban on ureq.
//! Review new clients, imports, helpers, macros, dependencies and request-level
//! overrides for bypasses. There is no source-text scanner: literal searches
//! cannot reliably establish which Rust code constructs an agent.

use ureq::config::{Config, ConfigBuilder};
use ureq::typestate::AgentScope;
use ureq::unversioned::resolver::Resolver;
use ureq::unversioned::transport::Connector;

type Builder = ConfigBuilder<AgentScope>;

/// A transport's configuration with the shared proxy policy already applied.
pub(crate) struct AgentConfig(Config);

pub(crate) fn config(configure: impl FnOnce(Builder) -> Builder) -> AgentConfig {
    AgentConfig(configure(ureq::Agent::config_builder()).proxy(None).build())
}

impl AgentConfig {
    pub(crate) fn new_agent(self) -> ureq::Agent {
        self.0.new_agent()
    }

    pub(crate) fn with_parts(
        self,
        connector: impl Connector,
        resolver: impl Resolver,
    ) -> ureq::Agent {
        ureq::Agent::with_parts(self.0, connector, resolver)
    }
}

impl From<AgentConfig> for ureq::Agent {
    fn from(config: AgentConfig) -> Self {
        config.0.into()
    }
}

#[cfg(test)]
mod tests;
