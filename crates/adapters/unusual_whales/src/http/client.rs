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

use std::{
    collections::HashMap,
    fmt::{Debug, Display},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use nautilus_core::{UnixNanos, time::get_atomic_clock_realtime};
use nautilus_network::{
    http::{HttpClient, HttpResponse},
    retry::{RetryConfig, RetryError, RetryManager},
};
use reqwest::Method;
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc2822};

use crate::{
    common::credential::Credential,
    config::UnusualWhalesDataClientConfig,
    contract::ValidatedRestRequest,
    data_types::{UnusualWhalesOutcome, UnusualWhalesRateLimitHeaders, UnusualWhalesRestResult},
    dragonfly::{
        AdmissionDecision, AdmissionDeniedKind, CoordinationError, DragonflyGate,
        ResponseObservation,
    },
};

const RESPONSE_HEADER_NAMES: &[&str] = &[
    "retry-after",
    "x-uw-minute-req-counter",
    "x-uw-req-per-minute-remaining",
    "x-uw-req-per-minute-reset",
    "x-ratelimit-reset",
    "x-rate-limit-reset",
    "ratelimit-reset",
];

/// Shared HTTP client for generated Unusual Whales read operations.
#[derive(Clone)]
pub struct UnusualWhalesHttpClient {
    base_url: String,
    client: HttpClient,
    gate: DragonflyGate,
    retry_manager: Arc<RetryManager<AttemptFailure>>,
}

impl Debug for UnusualWhalesHttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(UnusualWhalesHttpClient))
            .field("base_url", &self.base_url)
            .field("gate", &self.gate)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
struct AttemptPayload {
    outcome: UnusualWhalesOutcome,
    http_status: Option<u16>,
    headers: UnusualWhalesRateLimitHeaders,
    response_json: Option<String>,
    response_body_base64: String,
    message: Option<String>,
    received_at: UnixNanos,
}

#[derive(Clone, Debug)]
struct AttemptFailure {
    payload: Box<AttemptPayload>,
    retryable: bool,
    retry_after: Option<Duration>,
}

impl Display for AttemptFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.payload.outcome)
    }
}

impl std::error::Error for AttemptFailure {}

impl UnusualWhalesHttpClient {
    /// Creates a shared HTTP client without any process-local rate limiter.
    ///
    /// # Errors
    ///
    /// Returns an error if the base URL, proxy, headers, or retry configuration is invalid.
    pub fn new(
        config: &UnusualWhalesDataClientConfig,
        credential: &Credential,
        gate: DragonflyGate,
    ) -> anyhow::Result<Self> {
        let parsed = url::Url::parse(&config.base_url)?;
        anyhow::ensure!(
            matches!(parsed.scheme(), "http" | "https"),
            "Unusual Whales base_url must use http or https"
        );
        anyhow::ensure!(
            parsed.query().is_none() && parsed.fragment().is_none(),
            "Unusual Whales base_url cannot contain a query or fragment"
        );
        anyhow::ensure!(
            parsed.username().is_empty() && parsed.password().is_none(),
            "Unusual Whales base_url cannot contain credentials"
        );

        let mut headers = HashMap::new();
        headers.insert(
            "Authorization".to_string(),
            format!("Bearer {}", credential.token()),
        );
        headers.insert("Accept".to_string(), "application/json".to_string());
        let client = HttpClient::new(
            headers,
            RESPONSE_HEADER_NAMES
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            Vec::new(),
            None,
            Some(config.http_timeout_secs),
            config.proxy_url.clone(),
        )?;
        let retry_manager = RetryManager::new(RetryConfig {
            max_retries: config.max_retries,
            initial_delay_ms: config.retry_delay_initial_ms,
            max_delay_ms: config.retry_delay_max_ms,
            backoff_factor: 2.0,
            jitter_ms: 100,
            operation_timeout_ms: Some(config.http_timeout_secs.saturating_mul(1_000)),
            immediate_first: false,
            max_elapsed_ms: None,
        });

        Ok(Self {
            base_url: config.base_url.trim_end_matches('/').to_string(),
            client,
            gate,
            retry_manager: Arc::new(retry_manager),
        })
    }

    /// Executes one validated generated read operation.
    pub async fn request(
        &self,
        request: ValidatedRestRequest,
        request_id: String,
    ) -> UnusualWhalesRestResult {
        let attempts = Arc::new(AtomicU32::new(0));
        let operation_id = request.operation.operation_id;
        let result = self
            .retry_manager
            .execute_with_retry_with_delay(
                operation_id,
                || {
                    let attempts = Arc::clone(&attempts);
                    let request = request.clone();
                    async move {
                        attempts.fetch_add(1, Ordering::Relaxed);
                        self.attempt(&request).await
                    }
                },
                |failure| failure.retryable,
                |failure| failure.retry_after,
                control_failure,
            )
            .await;
        let payload = match result {
            Ok(payload) => payload,
            Err(failure) => *failure.payload,
        };

        UnusualWhalesRestResult {
            operation_id: operation_id.to_string(),
            outcome: payload.outcome,
            http_status: payload.http_status,
            request_id,
            attempts: attempts.load(Ordering::Relaxed),
            rate_limit_headers: payload.headers,
            response_json: payload.response_json,
            response_body_base64: payload.response_body_base64,
            message: payload.message,
            received_at: payload.received_at,
            ts_event: payload.received_at,
            ts_init: payload.received_at,
        }
    }

    async fn attempt(
        &self,
        request: &ValidatedRestRequest,
    ) -> Result<AttemptPayload, AttemptFailure> {
        let lease = match self.gate.admit_http().await {
            Ok(AdmissionDecision::Admitted(lease)) => lease,
            Ok(AdmissionDecision::Denied { kind, retry_after }) => {
                return Err(admission_failure(kind, retry_after));
            }
            Err(e) => return Err(coordination_failure(e)),
        };

        let url = format!("{}{}", self.base_url, request.relative_url);
        let response = self
            .client
            .request(Method::GET, url, None, None, None, None, None)
            .await;

        match response {
            Ok(response) => {
                let observation = response_observation(&response, lease.admitted_at_ms());
                let coordination = self.gate.reconcile_response(observation).await;
                let release = lease.release().await;
                if let Err(e) = coordination.and(release) {
                    let payload = response_payload(
                        &response,
                        UnusualWhalesOutcome::CoordinationUnavailable,
                        Some(e.to_string()),
                    );
                    return Err(failure_from_payload(payload, false, None));
                }
                classify_response(&response)
            }
            Err(_) => {
                let release = lease.release().await;
                if release.is_err() {
                    return Err(coordination_failure(CoordinationError::Unavailable));
                }
                Err(transport_failure())
            }
        }
    }
}

fn classify_response(response: &HttpResponse) -> Result<AttemptPayload, AttemptFailure> {
    let status = response.status.as_u16();
    let raw = String::from_utf8(response.body.to_vec()).ok();
    let json = raw
        .as_deref()
        .and_then(|body| serde_json::from_str::<Value>(body).ok());
    let concurrent_limit_exceeded = json.as_ref().is_some_and(contains_concurrency_error);

    let (outcome, retryable) = if status == 429 || concurrent_limit_exceeded {
        (UnusualWhalesOutcome::RateLimited, true)
    } else if response.status.is_server_error() {
        (UnusualWhalesOutcome::TransportUnavailable, true)
    } else if matches!(status, 401 | 403) {
        (UnusualWhalesOutcome::EntitlementDenied, false)
    } else if !response.status.is_success() {
        (UnusualWhalesOutcome::ProviderRejected, false)
    } else if raw.is_none() || json.is_none() {
        (UnusualWhalesOutcome::MalformedResponse, false)
    } else {
        (UnusualWhalesOutcome::Success, false)
    };
    let payload = response_payload(response, outcome, None);

    if outcome == UnusualWhalesOutcome::Success {
        Ok(payload)
    } else {
        let retry_after = if concurrent_limit_exceeded {
            None
        } else {
            payload
                .headers
                .retry_after
                .as_deref()
                .and_then(|value| parse_retry_after(value, unix_time_ms()))
        };
        Err(failure_from_payload(payload, retryable, retry_after))
    }
}

fn response_payload(
    response: &HttpResponse,
    outcome: UnusualWhalesOutcome,
    message: Option<String>,
) -> AttemptPayload {
    let raw = String::from_utf8(response.body.to_vec()).ok();
    let response_json = raw.filter(|body| serde_json::from_str::<Value>(body).is_ok());
    AttemptPayload {
        outcome,
        http_status: Some(response.status.as_u16()),
        headers: rate_headers(&response.headers),
        response_json,
        response_body_base64: BASE64.encode(&response.body),
        message,
        received_at: get_atomic_clock_realtime().get_time_ns(),
    }
}

fn response_observation(response: &HttpResponse, admitted_at_ms: i64) -> ResponseObservation {
    let headers = rate_headers(&response.headers);
    let json = serde_json::from_slice::<Value>(&response.body).ok();
    let concurrent_limit_exceeded = json.as_ref().is_some_and(contains_concurrency_error);
    let minute_request_counter = headers
        .minute_request_counter
        .as_deref()
        .and_then(parse_counter);
    let requests_per_minute_remaining = headers
        .requests_per_minute_remaining
        .as_deref()
        .and_then(parse_counter);
    let requests_per_minute_reset_ms = headers
        .requests_per_minute_reset
        .as_deref()
        .or(headers.rate_limit_reset.as_deref())
        .and_then(|value| parse_reset_ms(value, admitted_at_ms))
        .or_else(|| {
            (minute_request_counter.is_some() || requests_per_minute_remaining.is_some())
                .then(|| admitted_at_ms.saturating_add(60_000))
        });
    ResponseObservation {
        retry_after_until_ms: if concurrent_limit_exceeded {
            None
        } else {
            headers
                .retry_after
                .as_deref()
                .and_then(|value| retry_after_until_ms(value, admitted_at_ms))
        },
        minute_request_counter,
        requests_per_minute_remaining,
        requests_per_minute_reset_ms,
        success: response.status.is_success(),
        concurrent_limit_exceeded,
    }
}

fn rate_headers(headers: &HashMap<String, String>) -> UnusualWhalesRateLimitHeaders {
    UnusualWhalesRateLimitHeaders {
        retry_after: header(headers, "retry-after"),
        minute_request_counter: header(headers, "x-uw-minute-req-counter"),
        requests_per_minute_remaining: header(headers, "x-uw-req-per-minute-remaining"),
        requests_per_minute_reset: header(headers, "x-uw-req-per-minute-reset"),
        rate_limit_reset: ["x-ratelimit-reset", "x-rate-limit-reset", "ratelimit-reset"]
            .iter()
            .find_map(|name| header(headers, name)),
    }
}

fn header(headers: &HashMap<String, String>, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn contains_concurrency_error(value: &Value) -> bool {
    match value {
        Value::String(value) => value
            .to_ascii_lowercase()
            .contains("concurrent_limit_exceeded"),
        Value::Array(values) => values.iter().any(contains_concurrency_error),
        Value::Object(values) => values.values().any(contains_concurrency_error),
        _ => false,
    }
}

fn parse_counter(value: &str) -> Option<i64> {
    value
        .trim()
        .split('/')
        .next()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value >= 0)
}

fn retry_after_until_ms(value: &str, now_ms: i64) -> Option<i64> {
    parse_retry_after(value, now_ms)
        .and_then(|delay| i64::try_from(delay.as_millis()).ok())
        .and_then(|delay| now_ms.checked_add(delay))
}

fn parse_retry_after(value: &str, now_ms: i64) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let timestamp = OffsetDateTime::parse(value.trim(), &Rfc2822)
        .ok()?
        .unix_timestamp_nanos()
        / 1_000_000;
    let timestamp = i64::try_from(timestamp).ok()?;
    Some(Duration::from_millis(
        u64::try_from(timestamp.saturating_sub(now_ms).max(0)).ok()?,
    ))
}

fn parse_reset_ms(value: &str, now_ms: i64) -> Option<i64> {
    let raw = value.trim().parse::<i64>().ok()?;
    if raw <= 0 {
        return None;
    }

    if raw >= 100_000_000_000 {
        Some(raw)
    } else if raw >= 1_000_000_000 {
        raw.checked_mul(1_000)
    } else {
        now_ms.checked_add(raw.checked_mul(1_000)?)
    }
}

fn admission_failure(kind: AdmissionDeniedKind, retry_after: Duration) -> AttemptFailure {
    let retryable = kind != AdmissionDeniedKind::DailyBudget;
    failure(
        UnusualWhalesOutcome::RateLimited,
        Some(format!("{kind:?}")),
        retryable,
        Some(retry_after),
    )
}

fn failure(
    outcome: UnusualWhalesOutcome,
    message: Option<String>,
    retryable: bool,
    retry_after: Option<Duration>,
) -> AttemptFailure {
    failure_from_payload(
        AttemptPayload {
            outcome,
            http_status: None,
            headers: UnusualWhalesRateLimitHeaders::default(),
            response_json: None,
            response_body_base64: String::new(),
            message,
            received_at: get_atomic_clock_realtime().get_time_ns(),
        },
        retryable,
        retry_after,
    )
}

fn failure_from_payload(
    payload: AttemptPayload,
    retryable: bool,
    retry_after: Option<Duration>,
) -> AttemptFailure {
    AttemptFailure {
        payload: Box::new(payload),
        retryable,
        retry_after,
    }
}

fn coordination_failure(error: CoordinationError) -> AttemptFailure {
    failure(
        UnusualWhalesOutcome::CoordinationUnavailable,
        Some(error.to_string()),
        false,
        None,
    )
}

fn transport_failure() -> AttemptFailure {
    failure(
        UnusualWhalesOutcome::TransportUnavailable,
        Some("Unusual Whales HTTP transport is unavailable".to_string()),
        true,
        None,
    )
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "RetryManager requires an owned RetryError callback"
)]
fn control_failure(error: RetryError) -> AttemptFailure {
    failure(
        UnusualWhalesOutcome::TransportUnavailable,
        Some(error.to_string()),
        false,
        None,
    )
}

fn unix_time_ms() -> i64 {
    i64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use bytes::Bytes;
    use nautilus_network::http::HttpStatus;
    use rstest::rstest;

    use super::*;

    fn response(status: u16, body: &[u8], headers: &[(&str, &str)]) -> HttpResponse {
        HttpResponse {
            status: HttpStatus::try_from(status).unwrap(),
            headers: headers
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect::<HashMap<_, _>>(),
            body: Bytes::copy_from_slice(body),
        }
    }

    #[rstest]
    fn successful_response_preserves_exact_json() {
        let raw = include_bytes!("../../test_data/market_tide_response.json");
        let payload = classify_response(&response(200, raw, &[])).unwrap();
        assert_eq!(payload.outcome, UnusualWhalesOutcome::Success);
        assert_eq!(
            payload.response_json.as_deref(),
            std::str::from_utf8(raw).ok()
        );
        assert_eq!(BASE64.decode(payload.response_body_base64).unwrap(), raw);
    }

    #[rstest]
    fn malformed_response_is_a_value_and_preserves_bytes() {
        let raw = include_bytes!("../../test_data/malformed_response.json");
        let failure = classify_response(&response(200, raw, &[])).unwrap_err();
        assert_eq!(
            failure.payload.outcome,
            UnusualWhalesOutcome::MalformedResponse
        );
        assert!(failure.payload.response_json.is_none());
        assert_eq!(
            BASE64.decode(failure.payload.response_body_base64).unwrap(),
            raw
        );
    }

    #[rstest]
    fn provider_failures_are_typed_values() {
        assert_eq!(
            classify_response(&response(429, b"rate limit", &[]))
                .unwrap_err()
                .payload
                .outcome,
            UnusualWhalesOutcome::RateLimited
        );
        assert_eq!(
            classify_response(&response(403, b"{}", &[]))
                .unwrap_err()
                .payload
                .outcome,
            UnusualWhalesOutcome::EntitlementDenied
        );
        assert_eq!(
            classify_response(&response(400, b"{}", &[]))
                .unwrap_err()
                .payload
                .outcome,
            UnusualWhalesOutcome::ProviderRejected
        );
    }

    #[rstest]
    fn concurrency_error_uses_normal_retry_delay() {
        let failure = classify_response(&response(
            429,
            br#"{"error":"concurrent_limit_exceeded"}"#,
            &[("retry-after", "60")],
        ))
        .unwrap_err();
        assert!(failure.retryable);
        assert_eq!(failure.retry_after, None);
    }

    #[rstest]
    fn zero_remaining_success_blocks_until_reset_observation() {
        let response = response(
            200,
            b"{}",
            &[
                ("x-uw-minute-req-counter", "120"),
                ("x-uw-req-per-minute-remaining", "0"),
                ("x-uw-req-per-minute-reset", "30"),
            ],
        );
        let observation = response_observation(&response, 1_000_000);
        assert!(observation.success);
        assert_eq!(observation.minute_request_counter, Some(120));
        assert_eq!(observation.requests_per_minute_remaining, Some(0));
        assert_eq!(observation.requests_per_minute_reset_ms, Some(1_030_000));
    }

    #[rstest]
    fn counters_without_reset_use_one_rolling_window() {
        let response = response(
            200,
            b"{}",
            &[
                ("x-uw-minute-req-counter", "1"),
                ("x-uw-req-per-minute-remaining", "119"),
            ],
        );
        let observation = response_observation(&response, 1_000_000);
        assert_eq!(observation.requests_per_minute_reset_ms, Some(1_060_000));
    }

    #[rstest]
    fn coordination_failure_payload_preserves_provider_response() {
        let raw = include_bytes!("../../test_data/market_tide_response.json");
        let response = response(200, raw, &[]);
        let payload = response_payload(
            &response,
            UnusualWhalesOutcome::CoordinationUnavailable,
            Some(CoordinationError::Unavailable.to_string()),
        );
        assert_eq!(payload.http_status, Some(200));
        assert_eq!(
            payload.response_json.as_deref(),
            std::str::from_utf8(raw).ok()
        );
        assert_eq!(BASE64.decode(payload.response_body_base64).unwrap(), raw);
    }
}
