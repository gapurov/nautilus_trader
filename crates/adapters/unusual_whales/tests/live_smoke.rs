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

use std::time::Duration;

use nautilus_common::{
    clients::DataClient,
    live::runner::replace_data_event_sender,
    messages::{
        DataEvent,
        data::{DataResponse, RequestCustomData, SubscribeCustomData},
    },
};
use nautilus_core::{Params, UUID4, time::get_atomic_clock_realtime};
use nautilus_model::{
    data::{CustomData, Data, DataType},
    identifiers::ClientId,
};
use nautilus_unusual_whales::{
    UnusualWhalesDataClient, UnusualWhalesDataClientConfig, UnusualWhalesOutcome,
    UnusualWhalesProviderState, UnusualWhalesProviderStateKind, UnusualWhalesRestResult,
    UnusualWhalesWebSocketEvent,
};
use serde_json::Value;

#[tokio::test]
#[ignore = "requires a controlled UW account and a dedicated Dragonfly test service"]
async fn controlled_account_rest_and_websocket_smoke() {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<DataEvent>();
    replace_data_event_sender(sender);
    let client_id = ClientId::from("UW-SMOKE");
    let mut client =
        UnusualWhalesDataClient::new(client_id, UnusualWhalesDataClientConfig::default())
            .expect("environment must contain UW and Dragonfly credentials");
    client.connect().await.expect("client should connect");

    let mut operation_metadata = Params::new();
    operation_metadata.insert(
        "operation_id".to_string(),
        Value::String("PublicApi.MarketController.market_tide".to_string()),
    );
    client
        .request_data(RequestCustomData::new(
            client_id,
            DataType::new("UnusualWhalesRestResult", Some(operation_metadata), None),
            None,
            None,
            None,
            UUID4::new(),
            get_atomic_clock_realtime().get_time_ns(),
            None,
        ))
        .expect("REST request should validate");
    let rest_event = tokio::time::timeout(Duration::from_secs(30), receiver.recv())
        .await
        .expect("REST smoke timed out")
        .expect("data event channel closed");
    let DataEvent::Response(DataResponse::Data(response)) = rest_event else {
        panic!("expected custom REST response");
    };
    let results = response
        .data
        .downcast_ref::<Vec<CustomData>>()
        .expect("REST response should contain custom data");
    let result = results[0]
        .data
        .as_any()
        .downcast_ref::<UnusualWhalesRestResult>()
        .expect("custom data should be an UnusualWhalesRestResult");
    assert_eq!(result.outcome, UnusualWhalesOutcome::Success);
    let rest_payload: Value = serde_json::from_str(
        result
            .response_json
            .as_deref()
            .expect("successful REST result should contain JSON"),
    )
    .expect("REST result should contain valid JSON");
    assert!(
        rest_payload.get("data").is_some_and(|data| !data.is_null()),
        "market_tide response should contain provider data"
    );

    let mut channel_metadata = Params::new();
    channel_metadata.insert(
        "channel".to_string(),
        Value::String("price:AAPL".to_string()),
    );
    client
        .subscribe(SubscribeCustomData::new(
            Some(client_id),
            None,
            DataType::new("UnusualWhalesWebSocketEvent", Some(channel_metadata), None),
            UUID4::new(),
            get_atomic_clock_realtime().get_time_ns(),
            None,
            None,
        ))
        .expect("WebSocket subscription should validate");
    let (ready, websocket_payload) = tokio::time::timeout(Duration::from_secs(30), async {
        let mut ready = false;
        let mut websocket_payload = None;

        while let Some(event) = receiver.recv().await {
            let DataEvent::Data(Data::Custom(custom)) = event else {
                continue;
            };

            if let Some(state) = custom
                .data
                .as_any()
                .downcast_ref::<UnusualWhalesProviderState>()
            {
                if state.state == UnusualWhalesProviderStateKind::SubscriptionReady {
                    ready = true;
                }
            } else if let Some(frame) = custom
                .data
                .as_any()
                .downcast_ref::<UnusualWhalesWebSocketEvent>()
            {
                if frame.channel == "price:AAPL" {
                    let payload = serde_json::from_str::<Value>(&frame.frame_json)
                        .expect("WebSocket event should contain valid JSON");
                    let is_acknowledgement = payload
                        .as_array()
                        .and_then(|parts| parts.get(1))
                        .and_then(Value::as_object)
                        .is_some_and(|value| value.contains_key("status"));
                    if !is_acknowledgement {
                        websocket_payload = Some(payload);
                    }
                }
            }

            if ready && websocket_payload.is_some() {
                return (ready, websocket_payload);
            }
        }
        (ready, websocket_payload)
    })
    .await
    .expect("WebSocket acknowledgement or data frame timed out");
    assert!(ready);
    let frame = websocket_payload.expect("price:AAPL should publish a data frame");
    let frame = frame
        .as_array()
        .expect("price:AAPL frame should use the documented array envelope");
    assert_eq!(frame.first().and_then(Value::as_str), Some("price:AAPL"));
    let price = frame
        .get(1)
        .and_then(Value::as_object)
        .expect("price:AAPL frame should contain an object payload");
    assert_eq!(price.get("ticker").and_then(Value::as_str), Some("AAPL"));
    assert!(price.get("close").is_some());
    assert!(price.get("time").is_some());

    client.disconnect().await.expect("client should disconnect");
}
