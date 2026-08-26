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

use pyo3::prelude::*;

use crate::config::UnusualWhalesDataClientConfig;

#[pymethods]
#[pyo3_stub_gen::derive::gen_stub_pymethods]
impl UnusualWhalesDataClientConfig {
    /// Configuration for the Unusual Whales informational data client.
    #[new]
    #[pyo3(signature = (
        api_key = None,
        base_url = None,
        websocket_url = None,
        dragonfly_url = None,
        requests_per_minute = None,
        concurrent_requests = None,
        daily_request_limit = None,
        lease_ttl_secs = None,
        max_retries = None,
        retry_delay_initial_ms = None,
        retry_delay_max_ms = None,
        http_timeout_secs = None,
        reconnect_interval_ms = None,
        proxy_url = None,
    ))]
    #[expect(clippy::too_many_arguments)]
    fn py_new(
        api_key: Option<String>,
        base_url: Option<String>,
        websocket_url: Option<String>,
        dragonfly_url: Option<String>,
        requests_per_minute: Option<u32>,
        concurrent_requests: Option<u32>,
        daily_request_limit: Option<u32>,
        lease_ttl_secs: Option<u64>,
        max_retries: Option<u32>,
        retry_delay_initial_ms: Option<u64>,
        retry_delay_max_ms: Option<u64>,
        http_timeout_secs: Option<u64>,
        reconnect_interval_ms: Option<u64>,
        proxy_url: Option<String>,
    ) -> PyResult<Self> {
        let defaults = Self::default();
        let config = Self {
            api_key,
            base_url: base_url.unwrap_or(defaults.base_url),
            websocket_url: websocket_url.unwrap_or(defaults.websocket_url),
            dragonfly_url,
            requests_per_minute: requests_per_minute.unwrap_or(defaults.requests_per_minute),
            concurrent_requests: concurrent_requests.unwrap_or(defaults.concurrent_requests),
            daily_request_limit: daily_request_limit.unwrap_or(defaults.daily_request_limit),
            lease_ttl_secs: lease_ttl_secs.unwrap_or(defaults.lease_ttl_secs),
            max_retries: max_retries.unwrap_or(defaults.max_retries),
            retry_delay_initial_ms: retry_delay_initial_ms
                .unwrap_or(defaults.retry_delay_initial_ms),
            retry_delay_max_ms: retry_delay_max_ms.unwrap_or(defaults.retry_delay_max_ms),
            http_timeout_secs: http_timeout_secs.unwrap_or(defaults.http_timeout_secs),
            reconnect_interval_ms: reconnect_interval_ms.unwrap_or(defaults.reconnect_interval_ms),
            proxy_url,
        };
        config
            .validate()
            .map_err(nautilus_core::python::to_pyvalue_err)?;
        Ok(config)
    }

    #[getter]
    const fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    #[getter]
    const fn has_dragonfly_url(&self) -> bool {
        self.dragonfly_url.is_some()
    }

    fn __repr__(&self) -> String {
        stringify!(UnusualWhalesDataClientConfig).to_string()
    }
}

nautilus_core::impl_pyo3_config_getters!(UnusualWhalesDataClientConfig {
    base_url: String,
    websocket_url: String,
    requests_per_minute: u32,
    concurrent_requests: u32,
    daily_request_limit: u32,
    lease_ttl_secs: u64,
    max_retries: u32,
    retry_delay_initial_ms: u64,
    retry_delay_max_ms: u64,
    http_timeout_secs: u64,
    reconnect_interval_ms: u64,
});
