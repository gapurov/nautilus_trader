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
    fmt::Debug,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use nautilus_common::{
    clients::DataClient,
    live::{runner::get_data_event_sender, runtime::get_runtime, task::TaskHandles},
    messages::{
        DataEvent,
        data::{
            CustomDataResponse, DataResponse, RequestBars, RequestBookDeltas, RequestBookDepth,
            RequestBookSnapshot, RequestCustomData, RequestForwardPrices, RequestFundingRates,
            RequestInstrument, RequestInstruments, RequestQuotes, RequestTrades, SubscribeBars,
            SubscribeBookDeltas, SubscribeBookDepth10, SubscribeCustomData, SubscribeFundingRates,
            SubscribeIndexPrices, SubscribeInstrument, SubscribeInstrumentClose,
            SubscribeInstrumentStatus, SubscribeInstruments, SubscribeMarkPrices,
            SubscribeOptionGreeks, SubscribeQuotes, SubscribeTrades, UnsubscribeBars,
            UnsubscribeBookDeltas, UnsubscribeBookDepth10, UnsubscribeCustomData,
            UnsubscribeFundingRates, UnsubscribeIndexPrices, UnsubscribeInstrument,
            UnsubscribeInstrumentClose, UnsubscribeInstrumentStatus, UnsubscribeInstruments,
            UnsubscribeMarkPrices, UnsubscribeOptionGreeks, UnsubscribeQuotes, UnsubscribeTrades,
        },
    },
};
use nautilus_core::datetime::datetime_to_unix_nanos;
use nautilus_live::SocketControl;
use nautilus_model::{
    data::{CustomData, DataType},
    identifiers::{ClientId, Venue},
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    common::{
        consts::{
            CHANNEL_METADATA_KEY, OPERATION_ID_METADATA_KEY, REST_RESULT_TYPE_NAME,
            UNUSUAL_WHALES_DRAGONFLY_URL, WEBSOCKET_ENDPOINT, WEBSOCKET_EVENT_TYPE_NAME,
        },
        credential::Credential,
    },
    config::UnusualWhalesDataClientConfig,
    contract::{validate_channel, validate_rest_request},
    data_types::register_unusual_whales_custom_data,
    dragonfly::DragonflyGate,
    http::UnusualWhalesHttpClient,
    websocket::UnusualWhalesWebSocketClient,
};

type DataEventSender = tokio::sync::mpsc::UnboundedSender<DataEvent>;

macro_rules! reject_subscribe {
    ($name:ident, $command:ty) => {
        fn $name(&mut self, _cmd: $command) -> anyhow::Result<()> {
            anyhow::bail!(
                "Unusual Whales is informational custom data only; native data subscription '{}' is unsupported",
                stringify!($name)
            )
        }
    };
}

macro_rules! reject_unsubscribe {
    ($name:ident, $command:ty) => {
        fn $name(&mut self, _cmd: &$command) -> anyhow::Result<()> {
            anyhow::bail!(
                "Unusual Whales is informational custom data only; native data unsubscription '{}' is unsupported",
                stringify!($name)
            )
        }
    };
}

macro_rules! reject_request {
    ($name:ident, $request:ty) => {
        fn $name(&self, _request: $request) -> anyhow::Result<()> {
            anyhow::bail!(
                "Unusual Whales is informational custom data only; native data request '{}' is unsupported",
                stringify!($name)
            )
        }
    };
}

/// Native Unusual Whales informational data client.
pub struct UnusualWhalesDataClient {
    client_id: ClientId,
    config: UnusualWhalesDataClientConfig,
    credential: Credential,
    dragonfly_url: Zeroizing<String>,
    data_sender: DataEventSender,
    is_connected: AtomicBool,
    cancellation: CancellationToken,
    tasks: TaskHandles,
    gate: Option<DragonflyGate>,
    http_client: Option<UnusualWhalesHttpClient>,
    websocket_client: Option<UnusualWhalesWebSocketClient>,
    socket_control: SocketControl,
}

impl Debug for UnusualWhalesDataClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(UnusualWhalesDataClient))
            .field("client_id", &self.client_id)
            .field("config", &self.config)
            .field("credential", &self.credential)
            .field("dragonfly_url", &"<redacted>")
            .field("is_connected", &self.is_connected)
            .field("tasks", &self.tasks)
            .finish_non_exhaustive()
    }
}

impl UnusualWhalesDataClient {
    /// Creates an Unusual Whales informational data client.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid configuration, missing credentials, or missing Dragonfly URL.
    pub fn new(client_id: ClientId, config: UnusualWhalesDataClientConfig) -> anyhow::Result<Self> {
        let data_sender = get_data_event_sender();
        Self::new_with_sender(client_id, config, data_sender)
    }

    fn new_with_sender(
        client_id: ClientId,
        mut config: UnusualWhalesDataClientConfig,
        data_sender: DataEventSender,
    ) -> anyhow::Result<Self> {
        config.validate()?;
        let credential = Credential::resolve(config.api_key.as_deref())?;
        if let Some(api_key) = config.api_key.as_mut() {
            api_key.zeroize();
        }
        config.api_key = None;

        let mut dragonfly_url = config.dragonfly_url.clone().or_else(|| {
            std::env::var(UNUSUAL_WHALES_DRAGONFLY_URL)
                .ok()
                .filter(|value| !value.trim().is_empty())
        });
        let dragonfly_url = dragonfly_url.take().ok_or_else(|| {
            anyhow::anyhow!("Dragonfly URL is required; set {UNUSUAL_WHALES_DRAGONFLY_URL}")
        })?;

        if let Some(value) = config.dragonfly_url.as_mut() {
            value.zeroize();
        }
        config.dragonfly_url = None;
        let socket_control = SocketControl::new(client_id, None, WEBSOCKET_ENDPOINT);

        Ok(Self {
            client_id,
            config,
            credential,
            dragonfly_url: Zeroizing::new(dragonfly_url),
            data_sender,
            is_connected: AtomicBool::new(false),
            cancellation: CancellationToken::new(),
            tasks: TaskHandles::default(),
            gate: None,
            http_client: None,
            websocket_client: None,
            socket_control,
        })
    }

    fn custom_channel(data_type: &DataType) -> anyhow::Result<String> {
        anyhow::ensure!(
            data_type.type_name() == WEBSOCKET_EVENT_TYPE_NAME,
            "Unsupported Unusual Whales custom subscription type: {}",
            data_type.type_name()
        );
        let channel = data_type
            .metadata()
            .and_then(|metadata| metadata.get(CHANNEL_METADATA_KEY))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Unusual Whales subscription metadata requires string '{CHANNEL_METADATA_KEY}'"
                )
            })?;
        Ok(validate_channel(channel)?.channel)
    }

    fn custom_operation(data_type: &DataType) -> anyhow::Result<&str> {
        anyhow::ensure!(
            data_type.type_name() == REST_RESULT_TYPE_NAME,
            "Unsupported Unusual Whales custom request type: {}",
            data_type.type_name()
        );
        data_type
            .metadata()
            .and_then(|metadata| metadata.get(OPERATION_ID_METADATA_KEY))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Unusual Whales request metadata requires string '{OPERATION_ID_METADATA_KEY}'"
                )
            })
    }

    fn shutdown_now(&mut self) {
        self.cancellation.cancel();
        self.tasks.abort_all();

        if let Some(websocket) = self.websocket_client.as_ref() {
            websocket.shutdown_now();
        }
        self.is_connected.store(false, Ordering::Release);
    }
}

#[async_trait(?Send)]
impl DataClient for UnusualWhalesDataClient {
    fn client_id(&self) -> ClientId {
        self.client_id
    }

    fn venue(&self) -> Option<Venue> {
        None
    }

    fn start(&mut self) -> anyhow::Result<()> {
        register_unusual_whales_custom_data();
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.shutdown_now();
        Ok(())
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        self.shutdown_now();
        self.cancellation = CancellationToken::new();
        self.gate = None;
        self.http_client = None;
        self.websocket_client = None;
        Ok(())
    }

    fn dispose(&mut self) -> anyhow::Result<()> {
        self.stop()
    }

    fn is_connected(&self) -> bool {
        self.is_connected.load(Ordering::Acquire)
    }

    fn is_disconnected(&self) -> bool {
        !self.is_connected()
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        if self.is_connected() {
            return Ok(());
        }
        register_unusual_whales_custom_data();
        let scope_hash = self.credential.scope_hash(&self.config.base_url);
        let gate = DragonflyGate::connect(
            &self.dragonfly_url,
            &scope_hash,
            self.config.requests_per_minute,
            self.config.concurrent_requests,
            self.config.daily_request_limit,
            Duration::from_secs(self.config.lease_ttl_secs),
            Duration::from_millis(self.config.reconnect_interval_ms),
        )
        .await?;
        let http_client =
            UnusualWhalesHttpClient::new(&self.config, &self.credential, gate.clone())?;
        let websocket_client = UnusualWhalesWebSocketClient::new(
            self.config.websocket_url.clone(),
            self.config.proxy_url.clone(),
            self.credential.clone(),
            gate.clone(),
            self.socket_control.clone(),
            self.data_sender.clone(),
        );
        self.gate = Some(gate);
        self.http_client = Some(http_client);
        self.websocket_client = Some(websocket_client);
        self.is_connected.store(true, Ordering::Release);
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        if self.is_disconnected() {
            return Ok(());
        }
        self.cancellation.cancel();
        self.tasks.abort_all_retained();

        if let Some(websocket) = self.websocket_client.as_mut() {
            websocket.shutdown().await;
        }

        for handle in self.tasks.take_all() {
            match handle.await {
                Ok(()) => {}
                Err(e) if e.is_cancelled() => {}
                Err(e) => log::warn!("Unusual Whales REST task failed during shutdown: {e}"),
            }
        }
        self.websocket_client = None;
        self.http_client = None;
        self.gate = None;
        self.cancellation = CancellationToken::new();
        self.is_connected.store(false, Ordering::Release);
        Ok(())
    }

    fn subscribe(&mut self, cmd: SubscribeCustomData) -> anyhow::Result<()> {
        let channel = Self::custom_channel(&cmd.data_type)?;
        let validated = validate_channel(&channel)?;
        let websocket = self
            .websocket_client
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Unusual Whales client is not connected"))?;
        websocket.subscribe(validated)
    }

    fn unsubscribe(&mut self, cmd: &UnsubscribeCustomData) -> anyhow::Result<()> {
        let channel = Self::custom_channel(&cmd.data_type)?;
        let validated = validate_channel(&channel)?;
        let websocket = self
            .websocket_client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Unusual Whales client is not connected"))?;
        websocket.unsubscribe(&validated)
    }

    fn request_data(&self, request: RequestCustomData) -> anyhow::Result<()> {
        let operation_id = Self::custom_operation(&request.data_type)?;
        let validated = validate_rest_request(operation_id, request.params.as_ref())?;
        let http = self
            .http_client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Unusual Whales client is not connected"))?
            .clone();
        let sender = self.data_sender.clone();
        let data_type = request.data_type;
        let request_id = request.request_id;
        let client_id = request.client_id;
        let params = request.params;
        let start = datetime_to_unix_nanos(request.start);
        let end = datetime_to_unix_nanos(request.end);
        let handle = get_runtime().spawn(async move {
            let result = http.request(validated, request_id.to_string()).await;
            let timestamp = result.ts_init;
            let custom = CustomData::new(Arc::new(result), data_type.clone());
            let response = DataResponse::Data(CustomDataResponse::new(
                request_id,
                client_id,
                None,
                data_type,
                vec![custom],
                start,
                end,
                timestamp,
                params,
            ));

            if let Err(e) = sender.send(DataEvent::Response(response)) {
                log::debug!("Failed to publish Unusual Whales REST result: {e}");
            }
        });
        self.tasks.push(handle);
        Ok(())
    }

    reject_subscribe!(subscribe_instruments, SubscribeInstruments);
    reject_subscribe!(subscribe_instrument, SubscribeInstrument);
    reject_subscribe!(subscribe_book_deltas, SubscribeBookDeltas);
    reject_subscribe!(subscribe_book_depth10, SubscribeBookDepth10);
    reject_subscribe!(subscribe_quotes, SubscribeQuotes);
    reject_subscribe!(subscribe_trades, SubscribeTrades);
    reject_subscribe!(subscribe_mark_prices, SubscribeMarkPrices);
    reject_subscribe!(subscribe_index_prices, SubscribeIndexPrices);
    reject_subscribe!(subscribe_funding_rates, SubscribeFundingRates);
    reject_subscribe!(subscribe_bars, SubscribeBars);
    reject_subscribe!(subscribe_instrument_status, SubscribeInstrumentStatus);
    reject_subscribe!(subscribe_instrument_close, SubscribeInstrumentClose);
    reject_subscribe!(subscribe_option_greeks, SubscribeOptionGreeks);

    reject_unsubscribe!(unsubscribe_instruments, UnsubscribeInstruments);
    reject_unsubscribe!(unsubscribe_instrument, UnsubscribeInstrument);
    reject_unsubscribe!(unsubscribe_book_deltas, UnsubscribeBookDeltas);
    reject_unsubscribe!(unsubscribe_book_depth10, UnsubscribeBookDepth10);
    reject_unsubscribe!(unsubscribe_quotes, UnsubscribeQuotes);
    reject_unsubscribe!(unsubscribe_trades, UnsubscribeTrades);
    reject_unsubscribe!(unsubscribe_mark_prices, UnsubscribeMarkPrices);
    reject_unsubscribe!(unsubscribe_index_prices, UnsubscribeIndexPrices);
    reject_unsubscribe!(unsubscribe_funding_rates, UnsubscribeFundingRates);
    reject_unsubscribe!(unsubscribe_bars, UnsubscribeBars);
    reject_unsubscribe!(unsubscribe_instrument_status, UnsubscribeInstrumentStatus);
    reject_unsubscribe!(unsubscribe_instrument_close, UnsubscribeInstrumentClose);
    reject_unsubscribe!(unsubscribe_option_greeks, UnsubscribeOptionGreeks);

    reject_request!(request_instruments, RequestInstruments);
    reject_request!(request_instrument, RequestInstrument);
    reject_request!(request_book_snapshot, RequestBookSnapshot);
    reject_request!(request_quotes, RequestQuotes);
    reject_request!(request_trades, RequestTrades);
    reject_request!(request_funding_rates, RequestFundingRates);
    reject_request!(request_forward_prices, RequestForwardPrices);
    reject_request!(request_bars, RequestBars);
    reject_request!(request_book_depth, RequestBookDepth);
    reject_request!(request_book_deltas, RequestBookDeltas);
}

#[cfg(test)]
mod tests {
    use nautilus_common::clients::DataClient;
    use nautilus_core::{UUID4, time::get_atomic_clock_realtime};
    use nautilus_model::identifiers::{ClientId, InstrumentId};
    use rstest::rstest;

    use super::*;

    fn test_client() -> UnusualWhalesDataClient {
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        UnusualWhalesDataClient::new_with_sender(
            ClientId::from("UW-TEST"),
            UnusualWhalesDataClientConfig {
                api_key: Some("test-token".to_string()),
                dragonfly_url: Some("redis://127.0.0.1:6379/".to_string()),
                ..Default::default()
            },
            sender,
        )
        .unwrap()
    }

    #[rstest]
    fn venue_is_none() {
        let client = test_client();
        assert_eq!(client.venue(), None);
    }

    #[rstest]
    fn native_data_requests_are_rejected() {
        let client = test_client();
        let request = RequestInstrument::new(
            InstrumentId::from("AAPL.XNAS"),
            None,
            None,
            Some(client.client_id()),
            UUID4::new(),
            get_atomic_clock_realtime().get_time_ns(),
            None,
        );
        assert!(client.request_instrument(request).is_err());
    }

    #[rstest]
    fn debug_output_redacts_all_secrets() {
        let client = test_client();
        let debug = format!("{client:?}");
        assert!(!debug.contains("test-token"));
        assert!(!debug.contains("redis://"));
        assert!(debug.contains("<redacted>"));
    }

    #[rstest]
    fn stop_reset_and_dispose_are_idempotent_without_network_resources() {
        let mut client = test_client();
        client.stop().unwrap();
        client.stop().unwrap();
        client.reset().unwrap();
        client.dispose().unwrap();
        assert!(client.is_disconnected());
        assert!(client.tasks.is_empty());
        assert!(client.http_client.is_none());
        assert!(client.websocket_client.is_none());
        assert!(client.gate.is_none());
    }
}
