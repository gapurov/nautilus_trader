// -------------------------------------------------------------------------------------------------
//  Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved.
//  https://nautechsystems.io
//
//  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
//  You may not use this file except in compliance with the License.
//  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
// -------------------------------------------------------------------------------------------------

use std::fmt::Debug;

use serde::{Deserialize, Serialize};

use crate::common::consts::{DEFAULT_HTTP_BASE_URL, DEFAULT_WEBSOCKET_BASE_URL};

/// Configuration for the Unusual Whales informational data client.
#[derive(Clone, Serialize, Deserialize, bon::Builder)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader.adapters.unusual_whales", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.adapters.unusual_whales")
)]
pub struct UnusualWhalesDataClientConfig {
    /// API token. Falls back to `UNUSUAL_WHALES_API_TOKEN`.
    pub api_key: Option<String>,
    /// REST API base URL.
    #[builder(default = DEFAULT_HTTP_BASE_URL.to_string())]
    pub base_url: String,
    /// WebSocket URL without the token query parameter.
    #[builder(default = DEFAULT_WEBSOCKET_BASE_URL.to_string())]
    pub websocket_url: String,
    /// Dragonfly URL. Falls back to `UNUSUAL_WHALES_DRAGONFLY_URL`.
    pub dragonfly_url: Option<String>,
    /// Configured account-wide rolling-minute request limit.
    #[builder(default = 120)]
    pub requests_per_minute: u32,
    /// Configured account-wide concurrent request limit.
    #[builder(default = 1)]
    pub concurrent_requests: u32,
    /// Configured UTC daily request budget.
    #[builder(default = 30_000)]
    pub daily_request_limit: u32,
    /// HTTP lease expiry used for crash recovery.
    #[builder(default = 60)]
    pub lease_ttl_secs: u64,
    /// Maximum retry count after the initial HTTP attempt.
    #[builder(default = 3)]
    pub max_retries: u32,
    /// Initial retry delay in milliseconds.
    #[builder(default = 1_000)]
    pub retry_delay_initial_ms: u64,
    /// Maximum retry delay in milliseconds.
    #[builder(default = 10_000)]
    pub retry_delay_max_ms: u64,
    /// HTTP timeout in seconds.
    #[builder(default = 30)]
    pub http_timeout_secs: u64,
    /// Account-wide minimum interval between WebSocket connection starts.
    #[builder(default = 5_000)]
    pub reconnect_interval_ms: u64,
    /// Optional forward proxy URL for HTTP and WebSocket traffic.
    pub proxy_url: Option<String>,
}

impl Default for UnusualWhalesDataClientConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl Debug for UnusualWhalesDataClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(UnusualWhalesDataClientConfig))
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .field("websocket_url", &"<redacted>")
            .field(
                "dragonfly_url",
                &self.dragonfly_url.as_ref().map(|_| "<redacted>"),
            )
            .field("requests_per_minute", &self.requests_per_minute)
            .field("concurrent_requests", &self.concurrent_requests)
            .field("daily_request_limit", &self.daily_request_limit)
            .field("lease_ttl_secs", &self.lease_ttl_secs)
            .field("max_retries", &self.max_retries)
            .field("retry_delay_initial_ms", &self.retry_delay_initial_ms)
            .field("retry_delay_max_ms", &self.retry_delay_max_ms)
            .field("http_timeout_secs", &self.http_timeout_secs)
            .field("reconnect_interval_ms", &self.reconnect_interval_ms)
            .field("proxy_url", &self.proxy_url.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl UnusualWhalesDataClientConfig {
    /// Validates deterministic local configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when a limit, timeout, or endpoint is invalid.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.base_url.trim().is_empty(), "base_url cannot be empty");
        anyhow::ensure!(
            !self.websocket_url.trim().is_empty(),
            "websocket_url cannot be empty"
        );
        anyhow::ensure!(
            self.requests_per_minute > 0,
            "requests_per_minute must be positive"
        );
        anyhow::ensure!(
            self.concurrent_requests > 0,
            "concurrent_requests must be positive"
        );
        anyhow::ensure!(
            self.daily_request_limit > 0,
            "daily_request_limit must be positive"
        );
        anyhow::ensure!(self.lease_ttl_secs > 0, "lease_ttl_secs must be positive");
        anyhow::ensure!(
            self.http_timeout_secs > 0,
            "http_timeout_secs must be positive"
        );
        anyhow::ensure!(
            self.reconnect_interval_ms > 0,
            "reconnect_interval_ms must be positive"
        );
        anyhow::ensure!(
            self.retry_delay_initial_ms <= self.retry_delay_max_ms,
            "retry_delay_initial_ms cannot exceed retry_delay_max_ms"
        );
        Ok(())
    }
}
