//! Why a run could not be assembled, or could not carry on.

use crucible_config::ConfigError;
use crucible_credentials::CredentialError;
use crucible_provider::EndpointError;
use crucible_registry::RegistryError;
use crucible_sandbox::SandboxPolicyError;
use crucible_session::SessionError;
use crucible_tools::ToolsetError;
use crucible_workspace::PathError;

use crate::providers::ArmError;

/// Why a run could not be assembled, or could not carry on.
///
/// Every variant is a sentence somebody reads at a terminal, written here so
/// that the command line and any other front end say the same thing about the
/// same failure.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The working directory is not one that can be worked in.
    #[error(transparent)]
    Workspace(#[from] PathError),

    /// The session could not be recorded or continued.
    #[error(transparent)]
    Session(#[from] SessionError),

    /// The configured tool roster was invalid or could not be materialized.
    #[error(transparent)]
    Toolset(#[from] ToolsetError),

    /// The built-in commands could not be registered: two under one name, or
    /// one whose name will not fit a source identity. A wiring defect rather
    /// than anything the user did, and the sentence names the command.
    #[error("a built-in registry could not be assembled: {0}")]
    Registry(#[from] RegistryError),

    /// The built-in providers could not be assembled: two under one name, one
    /// whose name will not fit a source identity, or a model offered with no
    /// row in the generated table. A wiring defect rather than anything the
    /// user did, and the sentence names the provider.
    #[error("the built-in providers could not be assembled: {0}")]
    Providers(#[from] ArmError),

    /// `--resume` named a session this workspace has no record of.
    ///
    /// Its own sentence rather than the session crate's, because the id came
    /// from the command line a moment ago: what the user needs to hear is that
    /// the address is wrong here, not which file was looked for.
    #[error("no session {0} in this workspace")]
    NoSession(Box<str>),

    /// crucible's own files could not be found or read.
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// No confinement could be built for this directory at all.
    ///
    /// Fatal rather than a report saying so, because it is not an answer about
    /// a backend: the policy is refused before any backend is asked, and what
    /// is wrong is the directory the question was asked about.
    #[error("no confinement could be built for this directory: {0}")]
    Confinement(#[from] SandboxPolicyError),

    /// There is no key to authenticate with.
    #[error(transparent)]
    Credential(#[from] CredentialError),

    /// The sandbox would have had to wait to be asked what it can enforce,
    /// and the report is made by a caller that cannot wait yet.
    #[error(transparent)]
    Unready(#[from] crucible_runtime::Unready),

    /// Sensitive local state could not be made owner-only.
    #[error("{file} could not be protected: {source}")]
    Private {
        /// The directory or file whose boundary could not be established.
        file: Box<str>,
        /// What the platform refused.
        source: crucible_privacy::PrivacyError,
    },

    /// The command line named a provider this is not built with.
    #[error("no provider called {named}; this build has {has}")]
    Provider {
        /// What was asked for.
        named: Box<str>,
        /// The names the registry held when it was asked, comma-separated.
        has: Box<str>,
    },

    /// `--with-mcp` named a server no configuration file writes down.
    ///
    /// Fatal rather than a run without it: somebody who asked for a server by
    /// name and got a turn without its tools would be told nothing, and would
    /// read the silence as the model refusing the work.
    #[error("no mcp server called {named}; this configuration has {has}")]
    NoServer {
        /// What was asked for.
        named: Box<str>,
        /// The names `mcp.servers` held, comma-separated.
        has: Box<str>,
    },

    /// A written-down server cannot be turned into something startable.
    #[error("mcp.servers.{server}: {problem}")]
    Server {
        /// Whose record could not be resolved.
        server: Box<str>,
        /// What about it could not be, naming no value it read.
        problem: Box<str>,
    },

    /// `providers.<name>.baseUrl` is not an address requests can be sent to.
    ///
    /// Fatal rather than a warning that carries on at the vendor's address:
    /// somebody who set this has a reason not to reach the vendor, and sending
    /// there anyway would be a refusal that took the key with it.
    #[error("providers.{provider}.baseUrl: {source}")]
    Address {
        /// Which provider was pointed somewhere it could not go.
        provider: Box<str>,
        /// What was wrong with the address.
        source: EndpointError,
    },

    /// A renewable token was paired with an API-key endpoint setting.
    #[error(
        "providers.{provider}.baseUrl cannot be used with a subscription login; \
         export an API key to use that address"
    )]
    SubscriptionAddress {
        /// The provider whose fixed subscription audience was selected.
        provider: Box<str>,
    },

    /// Provider construction and source resolution disagreed.
    #[error("no credential is available for {provider}; use /login or set its API key variable")]
    Authentication {
        /// The provider that could not be authenticated.
        provider: Box<str>,
    },

    /// The run is over, and some of the work the application owned for it
    /// had not finished within its bound: renewals still in flight, or
    /// threads its work ran on. A cleanup that failed.
    #[error(transparent)]
    Unfinished(#[from] crate::services::Unfinished),

    /// The runtime a conversation waits for its turns on could not be
    /// started, so no conversation was assembled.
    #[error(transparent)]
    Unstarted(#[from] crate::runtime::Unstarted),
}
