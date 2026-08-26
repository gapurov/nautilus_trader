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

//! Unusual Whales informational data adapter.
//!
//! The adapter emits provider JSON only as UW-specific custom data. It never acts as a venue,
//! instrument provider, execution client, or native market-data authority.

#![warn(rustc::all)]
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(nonstandard_style)]
#![deny(missing_debug_implementations)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_panics_doc)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod common;
pub mod config;
pub mod contract;
pub mod data;
pub mod data_types;
pub mod dragonfly;
pub mod factories;
pub mod generated;
pub mod http;
pub mod websocket;

#[cfg(feature = "python")]
pub mod python;

pub use config::UnusualWhalesDataClientConfig;
pub use contract::{ValidatedChannel, ValidatedRestRequest};
pub use data::UnusualWhalesDataClient;
pub use data_types::{
    UnusualWhalesOutcome, UnusualWhalesProviderState, UnusualWhalesProviderStateKind,
    UnusualWhalesRateLimitHeaders, UnusualWhalesRestResult, UnusualWhalesWebSocketEvent,
    register_unusual_whales_custom_data,
};
pub use factories::UnusualWhalesDataClientFactory;
pub use generated::{CHANNELS, OPERATIONS, UnusualWhalesChannelForm, UnusualWhalesOperationId};
