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

//! Persistable Unusual Whales custom data envelopes.

use nautilus_core::UnixNanos;
use nautilus_persistence_macros::custom_data;
use serde::{Deserialize, Serialize};

/// Typed result of an Unusual Whales REST request.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader.adapters.unusual_whales", eq, from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass_enum(
        module = "nautilus_trader.adapters.unusual_whales"
    )
)]
pub enum UnusualWhalesOutcome {
    /// The provider returned a valid successful JSON response.
    Success,
    /// Account-wide admission or provider rate limiting prevented success.
    RateLimited,
    /// The provider denied the account entitlement.
    EntitlementDenied,
    /// The provider rejected the request.
    ProviderRejected,
    /// The provider response was not valid JSON.
    MalformedResponse,
    /// Dragonfly coordination was unavailable or its state reset.
    CoordinationUnavailable,
    /// The HTTP transport was unavailable after retry handling.
    TransportUnavailable,
}

/// Rate-related response headers preserved without reinterpretation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UnusualWhalesRateLimitHeaders {
    pub retry_after: Option<String>,
    pub minute_request_counter: Option<String>,
    pub requests_per_minute_remaining: Option<String>,
    pub requests_per_minute_reset: Option<String>,
    pub rate_limit_reset: Option<String>,
}

/// Result of one generated Unusual Whales REST operation.
///
/// `ts_event` is the local receive timestamp required by the Nautilus custom-data contract. It is
/// not a provider event time. Provider timestamps remain unchanged inside `response_json`.
#[cfg_attr(
    feature = "arrow",
    custom_data(pyo3, stub_module = "nautilus_trader.adapters.unusual_whales")
)]
#[cfg_attr(
    not(feature = "arrow"),
    custom_data(
        pyo3,
        no_arrow,
        stub_module = "nautilus_trader.adapters.unusual_whales"
    )
)]
pub struct UnusualWhalesRestResult {
    pub operation_id: String,
    #[custom_data_field(serde)]
    pub outcome: UnusualWhalesOutcome,
    #[custom_data_field(serde)]
    pub http_status: Option<u16>,
    pub request_id: String,
    pub attempts: u32,
    #[custom_data_field(serde)]
    pub rate_limit_headers: UnusualWhalesRateLimitHeaders,
    #[custom_data_field(serde)]
    pub response_json: Option<String>,
    pub response_body_base64: String,
    #[custom_data_field(serde)]
    pub message: Option<String>,
    pub received_at: UnixNanos,
    pub ts_event: UnixNanos,
    pub ts_init: UnixNanos,
}

/// Exact JSON frame received from an Unusual Whales WebSocket channel.
///
/// `ts_event` is the local receive timestamp required by the Nautilus custom-data contract. It is
/// not a provider event time. Provider timestamps remain unchanged inside `frame_json`.
#[cfg_attr(
    feature = "arrow",
    custom_data(pyo3, stub_module = "nautilus_trader.adapters.unusual_whales")
)]
#[cfg_attr(
    not(feature = "arrow"),
    custom_data(
        pyo3,
        no_arrow,
        stub_module = "nautilus_trader.adapters.unusual_whales"
    )
)]
pub struct UnusualWhalesWebSocketEvent {
    pub channel: String,
    pub connection_id: String,
    pub frame_json: String,
    pub frame_body_base64: String,
    pub is_valid_json: bool,
    pub received_at: UnixNanos,
    pub ts_event: UnixNanos,
    pub ts_init: UnixNanos,
}

/// State transition emitted by the Unusual Whales provider connection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader.adapters.unusual_whales", eq, from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass_enum(
        module = "nautilus_trader.adapters.unusual_whales"
    )
)]
pub enum UnusualWhalesProviderStateKind {
    Connecting,
    Connected,
    Reconnecting,
    ContinuityLost,
    SubscriptionPending,
    SubscriptionReady,
    SubscriptionRejected,
    MalformedFrame,
    CoordinationUnavailable,
    Disconnected,
}

/// Explicit provider, connection, subscription, and continuity state.
#[cfg_attr(
    feature = "arrow",
    custom_data(pyo3, stub_module = "nautilus_trader.adapters.unusual_whales")
)]
#[cfg_attr(
    not(feature = "arrow"),
    custom_data(
        pyo3,
        no_arrow,
        stub_module = "nautilus_trader.adapters.unusual_whales"
    )
)]
pub struct UnusualWhalesProviderState {
    #[custom_data_field(serde)]
    pub state: UnusualWhalesProviderStateKind,
    #[custom_data_field(serde)]
    pub connection_id: Option<String>,
    #[custom_data_field(serde)]
    pub channel: Option<String>,
    #[custom_data_field(serde)]
    pub detail: Option<String>,
    pub continuity_sequence: u64,
    pub received_at: UnixNanos,
    pub ts_event: UnixNanos,
    pub ts_init: UnixNanos,
}

/// Registers all persistable UW custom data types.
pub fn register_unusual_whales_custom_data() {
    #[cfg(feature = "arrow")]
    {
        nautilus_serialization::ensure_custom_data_registered::<UnusualWhalesRestResult>();
        nautilus_serialization::ensure_custom_data_registered::<UnusualWhalesWebSocketEvent>();
        nautilus_serialization::ensure_custom_data_registered::<UnusualWhalesProviderState>();
    }
}

#[cfg(all(test, feature = "arrow"))]
mod tests {
    use arrow::datatypes::DataType;
    use nautilus_serialization::arrow::{
        ArrowSchemaProvider, DecodeDataFromRecordBatch, EncodeToRecordBatch,
    };
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn rest_result_has_fixed_arrow_schema_and_round_trips_exact_json() {
        let original = UnusualWhalesRestResult {
            operation_id: "PublicApi.MarketController.market_tide".to_string(),
            outcome: UnusualWhalesOutcome::Success,
            http_status: Some(200),
            request_id: "request-1".to_string(),
            attempts: 1,
            rate_limit_headers: UnusualWhalesRateLimitHeaders {
                retry_after: None,
                minute_request_counter: Some("1".to_string()),
                requests_per_minute_remaining: Some("119".to_string()),
                requests_per_minute_reset: Some("30".to_string()),
                rate_limit_reset: None,
            },
            response_json: Some(r#"{ "provider_ts": 123, "value": "1.25" }"#.to_string()),
            response_body_base64: "eyJ2YWx1ZSI6IjEuMjUifQ==".to_string(),
            message: None,
            received_at: UnixNanos::from(10),
            ts_event: UnixNanos::from(10),
            ts_init: UnixNanos::from(10),
        };
        let schema = UnusualWhalesRestResult::get_schema(None);
        assert_eq!(
            schema.field_with_name("response_json").unwrap().data_type(),
            &DataType::Utf8
        );
        assert_eq!(
            schema
                .field_with_name("rate_limit_headers")
                .unwrap()
                .data_type(),
            &DataType::Utf8
        );

        let metadata = original.metadata();
        let batch =
            UnusualWhalesRestResult::encode_batch(&metadata, std::slice::from_ref(&original))
                .unwrap();
        let decoded = UnusualWhalesRestResult::decode_data_batch(&metadata, batch).unwrap();
        let decoded =
            UnusualWhalesRestResult::try_from(decoded.into_iter().next().unwrap()).unwrap();
        assert_eq!(decoded, original);
    }

    #[rstest]
    fn websocket_event_and_provider_state_round_trip() {
        let event = UnusualWhalesWebSocketEvent {
            channel: "price:AAPL".to_string(),
            connection_id: "connection-1".to_string(),
            frame_json: r#"["price:AAPL",{"provider_ts":123}]"#.to_string(),
            frame_body_base64: "WyJwcmljZTpBQVBMIix7InByb3ZpZGVyX3RzIjoxMjN9XQ==".to_string(),
            is_valid_json: true,
            received_at: UnixNanos::from(20),
            ts_event: UnixNanos::from(20),
            ts_init: UnixNanos::from(20),
        };
        let metadata = event.metadata();
        let batch =
            UnusualWhalesWebSocketEvent::encode_batch(&metadata, std::slice::from_ref(&event))
                .unwrap();
        let decoded = UnusualWhalesWebSocketEvent::decode_data_batch(&metadata, batch).unwrap();
        let decoded =
            UnusualWhalesWebSocketEvent::try_from(decoded.into_iter().next().unwrap()).unwrap();
        assert_eq!(decoded, event);

        let state = UnusualWhalesProviderState {
            state: UnusualWhalesProviderStateKind::ContinuityLost,
            connection_id: Some("connection-2".to_string()),
            channel: None,
            detail: Some("Data completeness is not implied".to_string()),
            continuity_sequence: 1,
            received_at: UnixNanos::from(21),
            ts_event: UnixNanos::from(21),
            ts_init: UnixNanos::from(21),
        };
        let metadata = state.metadata();
        let batch =
            UnusualWhalesProviderState::encode_batch(&metadata, std::slice::from_ref(&state))
                .unwrap();
        let decoded = UnusualWhalesProviderState::decode_data_batch(&metadata, batch).unwrap();
        let decoded =
            UnusualWhalesProviderState::try_from(decoded.into_iter().next().unwrap()).unwrap();
        assert_eq!(decoded, state);
    }
}
