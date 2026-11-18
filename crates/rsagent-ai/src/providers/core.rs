//! The fields and helpers every HTTP provider shares.
//!
//! `OpenAiProvider` and `AnthropicProvider` differ only in their wire protocol;
//! their identity, model catalog, API-key lookup, and base-URL override are
//! identical. That common skeleton lives here so the two providers stay small
//! and a third one is trivial to add.

use std::time::Duration;

use crate::provider::Auth;
use crate::types::Model;

/// How long to wait for a TCP connection to the provider before giving up.
///
/// Only the *connect* phase is bounded; a request that has already connected is left
/// alone, because streaming responses are long-lived and must not be cut off mid-stream.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Identity, model catalog, and key/base-URL resolution for a provider.
#[derive(Debug)]
pub(super) struct ProviderCore {
    id: String,
    models: Vec<Model>,
    env_keys: Vec<String>,
    base_url_override: Option<String>,
    /// One HTTP client per provider, so its connection pool is reused across requests
    /// instead of rebuilt on every call. Deliberately built with no *request* timeout:
    /// streaming responses are long-lived and must not be cut off mid-stream. A short
    /// *connect* timeout is set instead, so a server that accepts TCP but never answers
    /// cannot hang a turn forever.
    client: reqwest::Client,
}

impl ProviderCore {
    /// Build a core from a provider id, its models, and its API-key env vars.
    pub(super) fn new(id: String, models: Vec<Model>, env_keys: Vec<String>) -> Self {
        ProviderCore {
            id,
            models,
            env_keys,
            base_url_override: None,
            // A builder with only a connect timeout cannot fail to build; fall back to the
            // default client rather than panicking if it ever does.
            client: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .unwrap_or_default(),
        }
    }

    /// The provider's shared HTTP client (cheap to clone; clones share the pool).
    pub(super) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Point every model at `base_url` instead of its catalog value.
    pub(super) fn set_base_url(&mut self, base_url: String) {
        self.base_url_override = Some(base_url);
    }

    /// The provider id.
    pub(super) fn id(&self) -> &str {
        &self.id
    }

    /// The provider's model catalog.
    pub(super) fn models(&self) -> Vec<Model> {
        self.models.clone()
    }

    /// The first non-empty API key found among the configured env vars.
    pub(super) fn resolve_api_key(&self) -> Option<String> {
        self.env_keys
            .iter()
            .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
    }

    /// The base URL to use for `model`: the override if set, else the model's own.
    pub(super) fn effective_base_url(&self, model: &Model) -> String {
        self.base_url_override
            .clone()
            .unwrap_or_else(|| model.base_url.clone())
    }

    /// Resolve the authentication a request should carry.
    pub(super) fn resolve_auth(&self) -> Auth {
        Auth {
            api_key: self.resolve_api_key(),
            headers: Vec::new(),
            base_url: self.base_url_override.clone(),
        }
    }
}
