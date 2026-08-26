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
    collections::HashSet,
    fmt::Debug,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;
use nautilus_common::{
    clients::{SocketReconnectHandle, SocketReconnectRequestOutcome},
    live::{runtime::get_runtime, task::TaskHandles},
    messages::DataEvent,
};
use nautilus_core::{Params, UUID4, time::get_atomic_clock_realtime};
use nautilus_model::data::{CustomData, Data, DataType};
use nautilus_network::{
    transport::Message,
    websocket::{MessageReader, SubscriptionState, WebSocketClient, WebSocketConfig},
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{
    common::{
        consts::{CHANNEL_METADATA_KEY, PROVIDER_STATE_TYPE_NAME, WEBSOCKET_EVENT_TYPE_NAME},
        credential::Credential,
    },
    contract::{ValidatedChannel, validate_channel},
    data_types::{
        UnusualWhalesProviderState, UnusualWhalesProviderStateKind, UnusualWhalesWebSocketEvent,
    },
    dragonfly::{CoordinationError, DragonflyGate},
};

type DataEventSender = tokio::sync::mpsc::UnboundedSender<DataEvent>;

#[derive(Debug)]
enum WebSocketCommand {
    Join(String),
    Reconnect,
    Shutdown,
}

struct Shared {
    credential: Credential,
    websocket_url: String,
    proxy_url: Option<String>,
    gate: DragonflyGate,
    subscriptions: SubscriptionState,
    commands: tokio::sync::mpsc::UnboundedSender<WebSocketCommand>,
    data_sender: DataEventSender,
    cancellation: CancellationToken,
    reconnecting: AtomicBool,
    active: AtomicBool,
    started: AtomicBool,
    continuity_sequence: AtomicU64,
}

impl Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(Shared))
            .field("credential", &self.credential)
            .field("websocket_url", &"<redacted>")
            .field("proxy_url", &self.proxy_url.as_ref().map(|_| "<redacted>"))
            .field("gate", &self.gate)
            .field("subscriptions", &self.subscriptions)
            .field("reconnecting", &self.reconnecting)
            .field("active", &self.active)
            .field("started", &self.started)
            .field("continuity_sequence", &self.continuity_sequence)
            .finish_non_exhaustive()
    }
}

/// Lazy Unusual Whales WebSocket client with adapter-owned coordinated reconnects.
pub struct UnusualWhalesWebSocketClient {
    shared: Arc<Shared>,
    receiver: Option<tokio::sync::mpsc::UnboundedReceiver<WebSocketCommand>>,
    tasks: TaskHandles,
}

impl Debug for UnusualWhalesWebSocketClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(UnusualWhalesWebSocketClient))
            .field("shared", &self.shared)
            .field("tasks", &self.tasks)
            .finish()
    }
}

impl UnusualWhalesWebSocketClient {
    /// Creates a lazy WebSocket client. No network task starts until the first subscription.
    #[must_use]
    pub fn new(
        websocket_url: String,
        proxy_url: Option<String>,
        credential: Credential,
        gate: DragonflyGate,
        data_sender: DataEventSender,
    ) -> Self {
        let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            shared: Arc::new(Shared {
                credential,
                websocket_url,
                proxy_url,
                gate,
                subscriptions: SubscriptionState::new(':'),
                commands,
                data_sender,
                cancellation: CancellationToken::new(),
                reconnecting: AtomicBool::new(false),
                active: AtomicBool::new(false),
                started: AtomicBool::new(false),
                continuity_sequence: AtomicU64::new(0),
            }),
            receiver: Some(receiver),
            tasks: TaskHandles::default(),
        }
    }

    /// Subscribes to one already validated channel.
    ///
    /// # Errors
    ///
    /// Returns an error if the manager cannot start or its command channel is closed.
    pub fn subscribe(&mut self, channel: ValidatedChannel) -> anyhow::Result<()> {
        self.start_manager()?;

        if !self.shared.subscriptions.add_reference(&channel.channel) {
            return Ok(());
        }
        self.shared.subscriptions.mark_subscribe(&channel.channel);
        publish_state(
            &self.shared,
            UnusualWhalesProviderStateKind::SubscriptionPending,
            None,
            Some(channel.channel.clone()),
            None,
        );
        self.shared
            .commands
            .send(WebSocketCommand::Join(channel.channel))
            .map_err(|_e| anyhow::anyhow!("Unusual Whales WebSocket manager is closed"))
    }

    /// Removes local subscription intent.
    ///
    /// UW documents no leave frame, so the active connection is replaced with only the remaining
    /// desired channels.
    ///
    /// # Errors
    ///
    /// Returns an error if the manager command channel is closed.
    pub fn unsubscribe(&self, channel: &ValidatedChannel) -> anyhow::Result<()> {
        if !self.shared.subscriptions.remove_reference(&channel.channel) {
            return Ok(());
        }
        self.shared.subscriptions.mark_unsubscribe(&channel.channel);
        self.request_reconnect()
            .map(|_| ())
            .map_err(|()| anyhow::anyhow!("Unusual Whales WebSocket manager is closed"))
    }

    /// Returns a synchronous reconnect control for the socket registry.
    #[must_use]
    pub fn reconnect_handle(&self) -> SocketReconnectHandle {
        let shared = Arc::clone(&self.shared);
        SocketReconnectHandle::new(move || {
            if shared.cancellation.is_cancelled() {
                return SocketReconnectRequestOutcome::Closed;
            }

            if !shared.started.load(Ordering::Acquire) {
                return SocketReconnectRequestOutcome::Unsupported;
            }

            if shared.reconnecting.swap(true, Ordering::AcqRel) {
                return SocketReconnectRequestOutcome::AlreadyReconnecting;
            }

            if shared.commands.send(WebSocketCommand::Reconnect).is_err() {
                shared.reconnecting.store(false, Ordering::Release);
                return SocketReconnectRequestOutcome::Closed;
            }
            SocketReconnectRequestOutcome::Accepted
        })
    }

    /// Returns whether a current WebSocket transport is active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.shared.active.load(Ordering::Acquire)
    }

    /// Returns whether the lazy manager has started.
    #[must_use]
    pub fn is_started(&self) -> bool {
        self.shared.started.load(Ordering::Acquire)
    }

    /// Stops the manager, disconnects the current transport, and joins owned tasks.
    pub async fn shutdown(&mut self) {
        self.shared.cancellation.cancel();
        let _ = self.shared.commands.send(WebSocketCommand::Shutdown);

        for handle in self.tasks.take_all() {
            match handle.await {
                Ok(()) => {}
                Err(e) if e.is_cancelled() => {}
                Err(e) => log::warn!("Unusual Whales WebSocket task failed: {e}"),
            }
        }
        self.shared.active.store(false, Ordering::Release);
        self.shared.started.store(false, Ordering::Release);
    }

    /// Signals shutdown and aborts owned tasks without blocking.
    pub fn shutdown_now(&self) {
        self.shared.cancellation.cancel();
        let _ = self.shared.commands.send(WebSocketCommand::Shutdown);
        self.tasks.abort_all_retained();
        self.shared.active.store(false, Ordering::Release);
    }

    fn start_manager(&mut self) -> anyhow::Result<()> {
        if self.shared.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let receiver = self.receiver.take().ok_or_else(|| {
            self.shared.started.store(false, Ordering::Release);
            anyhow::anyhow!("Unusual Whales WebSocket manager cannot be restarted")
        })?;
        let shared = Arc::clone(&self.shared);
        let handle = get_runtime().spawn(run_manager(shared, receiver));
        self.tasks.push(handle);
        Ok(())
    }

    fn request_reconnect(&self) -> Result<SocketReconnectRequestOutcome, ()> {
        let outcome = self.reconnect_handle().request_reconnect();
        if outcome == SocketReconnectRequestOutcome::Closed {
            Err(())
        } else {
            Ok(outcome)
        }
    }
}

async fn run_manager(
    shared: Arc<Shared>,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<WebSocketCommand>,
) {
    let mut has_connected = false;

    loop {
        if shared.cancellation.is_cancelled() {
            break;
        }

        while shared.subscriptions.all_topics().is_empty() {
            match commands.recv().await {
                Some(WebSocketCommand::Shutdown) | None => return,
                Some(WebSocketCommand::Join(_)) => break,
                Some(WebSocketCommand::Reconnect) => {
                    shared.reconnecting.store(false, Ordering::Release);
                }
            }
        }

        shared.reconnecting.store(has_connected, Ordering::Release);
        publish_state(
            &shared,
            if has_connected {
                UnusualWhalesProviderStateKind::Reconnecting
            } else {
                UnusualWhalesProviderStateKind::Connecting
            },
            None,
            None,
            None,
        );

        if let Err(e) = wait_for_reconnect_admission(&shared).await {
            publish_state(
                &shared,
                UnusualWhalesProviderStateKind::CoordinationUnavailable,
                None,
                None,
                Some(e.to_string()),
            );

            if e == CoordinationError::StateReset {
                break;
            }

            if wait_or_cancel(&shared.cancellation, Duration::from_secs(5)).await {
                break;
            }
            continue;
        }

        let connection_id = UUID4::new().to_string();
        let Some((mut reader, client)) = connect(&shared).await else {
            if wait_or_cancel(&shared.cancellation, Duration::from_secs(1)).await {
                break;
            }
            continue;
        };

        if has_connected {
            let sequence = shared
                .continuity_sequence
                .fetch_add(1, Ordering::AcqRel)
                .saturating_add(1);
            publish_state_with_sequence(
                &shared,
                UnusualWhalesProviderStateKind::ContinuityLost,
                Some(connection_id.clone()),
                None,
                Some(
                    "Connection replaced; acknowledged subscriptions do not prove data completeness"
                        .to_string(),
                ),
                sequence,
            );
        }
        has_connected = true;
        shared.active.store(true, Ordering::Release);
        shared.reconnecting.store(false, Ordering::Release);
        publish_state(
            &shared,
            UnusualWhalesProviderStateKind::Connected,
            Some(connection_id.clone()),
            None,
            None,
        );

        let replay = prepare_replay(&shared.subscriptions);
        let mut replay_failed = false;

        for topic in &replay {
            if send_join(&client, topic).await.is_err() {
                replay_failed = true;
                break;
            }
        }

        if replay_failed {
            shared.active.store(false, Ordering::Release);
            client.disconnect().await;
            continue;
        }

        let mut sent_topics: HashSet<String> = replay.into_iter().collect();
        let mut replace_connection = false;
        while !replace_connection && !shared.cancellation.is_cancelled() {
            tokio::select! {
                biased;
                command = commands.recv() => {
                    match command {
                        Some(WebSocketCommand::Join(channel)) => {
                            if should_send_join(&mut sent_topics, &channel)
                                && send_join(&client, &channel).await.is_err()
                            {
                                replace_connection = true;
                            }
                        }
                        Some(WebSocketCommand::Reconnect) => {
                            replace_connection = true;
                        }
                        Some(WebSocketCommand::Shutdown) | None => {
                            client.disconnect().await;
                            return;
                        }
                    }
                }
                message = reader.next() => {
                    match message {
                        Some(Ok(Message::Text(bytes) | Message::Binary(bytes))) => {
                            handle_frame(&shared, &connection_id, &bytes);
                        }
                        Some(Ok(Message::Close(_)) | Err(_)) | None => {
                            replace_connection = true;
                        }
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    }
                }
                () = shared.cancellation.cancelled() => {
                    client.disconnect().await;
                    return;
                }
            }
        }

        shared.active.store(false, Ordering::Release);
        shared.reconnecting.store(true, Ordering::Release);
        prepare_replay(&shared.subscriptions);
        complete_connection_bound_unsubscribes(&shared.subscriptions);
        client.disconnect().await;
    }

    shared.active.store(false, Ordering::Release);
}

async fn wait_for_reconnect_admission(shared: &Shared) -> Result<(), CoordinationError> {
    loop {
        match shared.gate.admit_reconnect().await? {
            None => return Ok(()),
            Some(delay) => {
                if wait_or_cancel(&shared.cancellation, delay).await {
                    return Err(CoordinationError::Unavailable);
                }
            }
        }
    }
}

async fn connect(shared: &Shared) -> Option<(MessageReader, WebSocketClient)> {
    let mut url = match url::Url::parse(&shared.websocket_url) {
        Ok(url) => url,
        Err(_) => {
            publish_state(
                shared,
                UnusualWhalesProviderStateKind::Disconnected,
                None,
                None,
                Some("WebSocket URL is invalid".to_string()),
            );
            return None;
        }
    };
    url.query_pairs_mut()
        .append_pair("token", shared.credential.token());
    let config = match WebSocketConfig::builder()
        .url(url.to_string())
        .maybe_proxy_url(shared.proxy_url.clone())
        .build()
    {
        Ok(config) => config,
        Err(_) => {
            publish_state(
                shared,
                UnusualWhalesProviderStateKind::Disconnected,
                None,
                None,
                Some("WebSocket configuration is invalid".to_string()),
            );
            return None;
        }
    };

    match WebSocketClient::connect_stream(config, Vec::new(), None).await {
        Ok(session) => Some(session),
        Err(_) => {
            publish_state(
                shared,
                UnusualWhalesProviderStateKind::Disconnected,
                None,
                None,
                Some("WebSocket transport is unavailable".to_string()),
            );
            None
        }
    }
}

async fn send_join(client: &WebSocketClient, channel: &str) -> anyhow::Result<()> {
    let payload = serde_json::to_string(&json!({
        "channel": channel,
        "msg_type": "join",
    }))?;
    client
        .send_text(payload, None)
        .await
        .map_err(|_| anyhow::anyhow!("WebSocket join send failed"))
}

fn handle_frame(shared: &Shared, connection_id: &str, bytes: &[u8]) {
    let decoded = decode_frame(bytes);
    let channel = decoded
        .value
        .as_ref()
        .and_then(frame_parts)
        .map(|(channel, _)| channel);
    publish_event(
        shared,
        channel.unwrap_or_default(),
        connection_id,
        decoded.frame_json.as_deref().unwrap_or_default(),
        &decoded.frame_body_base64,
        decoded.value.is_some(),
    );

    if decoded.frame_json.is_none() {
        publish_state(
            shared,
            UnusualWhalesProviderStateKind::MalformedFrame,
            Some(connection_id.to_string()),
            None,
            Some("WebSocket frame is not UTF-8 JSON".to_string()),
        );
        return;
    }
    let Some(value) = decoded.value else {
        publish_state(
            shared,
            UnusualWhalesProviderStateKind::MalformedFrame,
            Some(connection_id.to_string()),
            None,
            Some("WebSocket frame is malformed JSON".to_string()),
        );
        return;
    };
    let Some((channel, payload)) = frame_parts(&value) else {
        publish_state(
            shared,
            UnusualWhalesProviderStateKind::MalformedFrame,
            Some(connection_id.to_string()),
            None,
            Some("WebSocket frame does not contain a channel envelope".to_string()),
        );
        return;
    };

    if validate_channel(channel).is_err() {
        publish_state(
            shared,
            UnusualWhalesProviderStateKind::MalformedFrame,
            Some(connection_id.to_string()),
            None,
            Some("WebSocket frame has an unknown channel".to_string()),
        );
        return;
    }

    match acknowledgement(payload) {
        Some(true) if shared.subscriptions.get_reference_count(channel) > 0 => {
            shared.subscriptions.confirm_subscribe(channel);
            publish_state(
                shared,
                UnusualWhalesProviderStateKind::SubscriptionReady,
                Some(connection_id.to_string()),
                Some(channel.to_string()),
                Some(
                    "Join acknowledged on current connection; data completeness is not implied"
                        .to_string(),
                ),
            );
        }
        Some(false) => {
            shared.subscriptions.mark_failure(channel);
            publish_state(
                shared,
                UnusualWhalesProviderStateKind::SubscriptionRejected,
                Some(connection_id.to_string()),
                Some(channel.to_string()),
                None,
            );
        }
        _ => {}
    }
}

#[derive(Debug)]
struct DecodedFrame {
    frame_json: Option<String>,
    frame_body_base64: String,
    value: Option<Value>,
}

fn decode_frame(bytes: &[u8]) -> DecodedFrame {
    let frame_json = std::str::from_utf8(bytes).ok().map(str::to_string);
    let value = frame_json
        .as_deref()
        .and_then(|frame| serde_json::from_str::<Value>(frame).ok());
    DecodedFrame {
        frame_json,
        frame_body_base64: BASE64.encode(bytes),
        value,
    }
}

fn frame_parts(value: &Value) -> Option<(&str, &Value)> {
    let values = value.as_array()?;
    if values.len() != 2 {
        return None;
    }
    Some((values[0].as_str()?, &values[1]))
}

fn acknowledgement(payload: &Value) -> Option<bool> {
    let payload = payload.as_object()?;
    let status = payload.get("status")?.as_str()?;
    if !payload.contains_key("response") && status == "ok" {
        return None;
    }
    Some(status == "ok")
}

fn prepare_replay(subscriptions: &SubscriptionState) -> Vec<String> {
    let topics = subscriptions.all_topics();
    for topic in &topics {
        subscriptions.mark_failure(topic);
    }
    topics
}

fn complete_connection_bound_unsubscribes(subscriptions: &SubscriptionState) {
    for topic in subscriptions.pending_unsubscribe_topics() {
        subscriptions.confirm_unsubscribe(&topic);
    }
}

fn should_send_join(sent_topics: &mut HashSet<String>, channel: &str) -> bool {
    sent_topics.insert(channel.to_string())
}

fn publish_event(
    shared: &Shared,
    channel: &str,
    connection_id: &str,
    frame: &str,
    frame_body_base64: &str,
    is_valid_json: bool,
) {
    let timestamp = get_atomic_clock_realtime().get_time_ns();
    let event = UnusualWhalesWebSocketEvent {
        channel: channel.to_string(),
        connection_id: connection_id.to_string(),
        frame_json: frame.to_string(),
        frame_body_base64: frame_body_base64.to_string(),
        is_valid_json,
        received_at: timestamp,
        ts_event: timestamp,
        ts_init: timestamp,
    };
    let mut metadata = Params::new();
    metadata.insert(
        CHANNEL_METADATA_KEY.to_string(),
        Value::String(channel.to_string()),
    );
    let data_type = DataType::new(WEBSOCKET_EVENT_TYPE_NAME, Some(metadata), None);
    let custom = CustomData::new(Arc::new(event), data_type);

    if let Err(e) = shared
        .data_sender
        .send(DataEvent::Data(Data::Custom(custom)))
    {
        log::debug!("Failed to publish Unusual Whales WebSocket event: {e}");
    }
}

fn publish_state(
    shared: &Shared,
    state: UnusualWhalesProviderStateKind,
    connection_id: Option<String>,
    channel: Option<String>,
    detail: Option<String>,
) {
    let sequence = shared.continuity_sequence.load(Ordering::Acquire);
    publish_state_with_sequence(shared, state, connection_id, channel, detail, sequence);
}

fn publish_state_with_sequence(
    shared: &Shared,
    state: UnusualWhalesProviderStateKind,
    connection_id: Option<String>,
    channel: Option<String>,
    detail: Option<String>,
    continuity_sequence: u64,
) {
    let timestamp = get_atomic_clock_realtime().get_time_ns();
    let state = UnusualWhalesProviderState {
        state,
        connection_id,
        channel,
        detail,
        continuity_sequence,
        received_at: timestamp,
        ts_event: timestamp,
        ts_init: timestamp,
    };
    let data_type = DataType::new(PROVIDER_STATE_TYPE_NAME, None, None);
    let custom = CustomData::new(Arc::new(state), data_type);

    if let Err(e) = shared
        .data_sender
        .send(DataEvent::Data(Data::Custom(custom)))
    {
        log::debug!("Failed to publish Unusual Whales provider state: {e}");
    }
}

async fn wait_or_cancel(cancellation: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        () = cancellation.cancelled() => true,
        () = tokio::time::sleep(duration) => false,
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn join_message_uses_documented_shape() {
        let value = json!({"channel": "price:AAPL", "msg_type": "join"});
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            r#"{"channel":"price:AAPL","msg_type":"join"}"#
        );
    }

    #[rstest]
    fn acknowledgement_requires_response_and_status() {
        let documented: Value =
            serde_json::from_str(include_str!("../../test_data/join_ack.json")).unwrap();
        let (_, documented_payload) = frame_parts(&documented).unwrap();
        assert_eq!(acknowledgement(documented_payload), Some(true));
        assert_eq!(
            acknowledgement(&json!({"response": {}, "status": "ok"})),
            Some(true)
        );
        assert_eq!(
            acknowledgement(&json!({"response": {}, "status": "error"})),
            Some(false)
        );
        assert_eq!(acknowledgement(&json!({"status": "ok"})), None);
    }

    #[rstest]
    fn data_frame_is_not_misclassified_as_acknowledgement() {
        let value: Value =
            serde_json::from_str(include_str!("../../test_data/price_frame.json")).unwrap();
        let (channel, payload) = frame_parts(&value).unwrap();
        assert_eq!(channel, "price:SPY");
        assert_eq!(acknowledgement(payload), None);
    }

    #[rstest]
    fn malformed_frame_fixture_is_rejected_as_json() {
        let raw = include_bytes!("../../test_data/malformed_frame.json");
        let decoded = decode_frame(raw);
        assert!(decoded.frame_json.is_some());
        assert!(decoded.value.is_none());
        assert_eq!(BASE64.decode(decoded.frame_body_base64).unwrap(), raw);
    }

    #[rstest]
    fn reconnect_clears_confirmation_and_replays_desired_topics() {
        let subscriptions = SubscriptionState::new(':');
        assert!(subscriptions.add_reference("price:AAPL"));
        subscriptions.mark_subscribe("price:AAPL");
        subscriptions.confirm_subscribe("price:AAPL");
        assert_eq!(subscriptions.len(), 1);

        let replay = prepare_replay(&subscriptions);
        assert_eq!(replay, ["price:AAPL"]);
        assert_eq!(subscriptions.len(), 0);
        assert_eq!(subscriptions.pending_subscribe_topics(), ["price:AAPL"]);
    }

    #[rstest]
    fn unsubscribe_reconnect_replays_only_remaining_intent() {
        let subscriptions = SubscriptionState::new(':');
        for topic in ["price:AAPL", "option_trades:AAPL"] {
            assert!(subscriptions.add_reference(topic));
            subscriptions.mark_subscribe(topic);
            subscriptions.confirm_subscribe(topic);
        }
        assert!(subscriptions.remove_reference("option_trades:AAPL"));
        subscriptions.mark_unsubscribe("option_trades:AAPL");

        let replay = prepare_replay(&subscriptions);
        assert_eq!(replay, ["price:AAPL"]);
        complete_connection_bound_unsubscribes(&subscriptions);
        assert!(subscriptions.pending_unsubscribe_topics().is_empty());
        assert!(
            !subscriptions
                .all_topics()
                .contains(&"option_trades:AAPL".to_string())
        );
    }

    #[rstest]
    fn queued_first_subscription_does_not_send_a_duplicate_join() {
        let mut sent_topics = HashSet::from(["price:AAPL".to_string()]);
        assert!(!should_send_join(&mut sent_topics, "price:AAPL"));
        assert!(should_send_join(&mut sent_topics, "price:MSFT"));
    }
}
