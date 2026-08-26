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

//! Unusual Whales adapter constants.

use std::sync::LazyLock;

use nautilus_model::identifiers::ClientId;
use ustr::Ustr;

/// The adapter identifier string.
pub const UNUSUAL_WHALES: &str = "UNUSUAL_WHALES";

/// The default REST API base URL.
pub const DEFAULT_HTTP_BASE_URL: &str = "https://api.unusualwhales.com";

/// The default WebSocket base URL without credentials.
pub const DEFAULT_WEBSOCKET_BASE_URL: &str = "wss://api.unusualwhales.com/socket";

/// The API token environment variable.
pub const UNUSUAL_WHALES_API_TOKEN: &str = "UNUSUAL_WHALES_API_TOKEN";

/// The Dragonfly connection URL environment variable.
pub const UNUSUAL_WHALES_DRAGONFLY_URL: &str = "UNUSUAL_WHALES_DRAGONFLY_URL";

/// Custom data type name for REST results.
pub const REST_RESULT_TYPE_NAME: &str = "UnusualWhalesRestResult";

/// Custom data type name for WebSocket events.
pub const WEBSOCKET_EVENT_TYPE_NAME: &str = "UnusualWhalesWebSocketEvent";

/// Custom data type name for provider state.
pub const PROVIDER_STATE_TYPE_NAME: &str = "UnusualWhalesProviderState";

/// Metadata key selecting a REST operation.
pub const OPERATION_ID_METADATA_KEY: &str = "operation_id";

/// Metadata key selecting a WebSocket channel.
pub const CHANNEL_METADATA_KEY: &str = "channel";

/// Socket registry endpoint name.
pub const WEBSOCKET_ENDPOINT: &str = "unusual_whales_stream";

/// Static client ID instance.
pub static UNUSUAL_WHALES_CLIENT_ID: LazyLock<ClientId> =
    LazyLock::new(|| ClientId::new(Ustr::from(UNUSUAL_WHALES)));
