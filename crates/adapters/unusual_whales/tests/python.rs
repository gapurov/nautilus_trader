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

#![cfg(feature = "python")]

use std::{cell::RefCell, rc::Rc};

use nautilus_common::{
    cache::Cache, clock::TestClock, live::runner::replace_data_event_sender, messages::DataEvent,
};
use nautilus_core::UnixNanos;
use nautilus_model::identifiers::ClientId;
use nautilus_system::get_global_pyo3_registry;
use nautilus_unusual_whales::{
    UnusualWhalesDataClientConfig, UnusualWhalesDataClientFactory, UnusualWhalesWebSocketEvent,
    common::consts::UNUSUAL_WHALES, python,
};
use pyo3::{
    Py, Python,
    types::{PyAnyMethods, PyModule},
};
use rstest::rstest;

#[rstest]
fn python_module_registers_factory_config_identifiers_and_custom_data() {
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel::<DataEvent>();
    replace_data_event_sender(sender);
    Python::initialize();

    Python::attach(|py| {
        let module =
            PyModule::new(py, "unusual_whales").expect("Unusual Whales module should be created");
        python::unusual_whales(&module).expect("Unusual Whales Python module should register");

        assert_eq!(
            module
                .getattr("OPERATION_COUNT")
                .unwrap()
                .extract::<usize>()
                .unwrap(),
            215
        );
        assert_eq!(
            module
                .getattr("CHANNEL_FORM_COUNT")
                .unwrap()
                .extract::<usize>()
                .unwrap(),
            28
        );

        let factory = Py::new(py, UnusualWhalesDataClientFactory::new())
            .expect("factory should convert to Python")
            .into_any();
        let config = Py::new(
            py,
            UnusualWhalesDataClientConfig {
                api_key: Some("test-token".to_string()),
                dragonfly_url: Some("redis://127.0.0.1:6379/".to_string()),
                ..Default::default()
            },
        )
        .expect("config should convert to Python")
        .into_any();
        let registry = get_global_pyo3_registry();
        let extracted_factory = registry
            .extract_factory(py, factory)
            .expect("data factory should extract");
        let extracted_config = registry
            .extract_config(py, config)
            .expect("data config should extract");
        let cache = Rc::new(RefCell::new(Cache::default()));
        let clock = Rc::new(RefCell::new(TestClock::new()));
        let client = extracted_factory
            .create(
                "UW-EXTRACTED",
                extracted_config.as_ref(),
                cache.into(),
                clock,
            )
            .expect("extracted factory should create data client");

        assert_eq!(extracted_factory.name(), UNUSUAL_WHALES);
        assert_eq!(
            extracted_factory.config_type(),
            "UnusualWhalesDataClientConfig"
        );
        assert_eq!(client.client_id(), ClientId::from("UW-EXTRACTED"));
        assert_eq!(client.venue(), None);

        let event = Py::new(
            py,
            UnusualWhalesWebSocketEvent {
                channel: "price:AAPL".to_string(),
                connection_id: "connection-1".to_string(),
                frame_json: r#"["price:AAPL",{"price":"187.25"}]"#.to_string(),
                frame_body_base64: "WyJwcmljZTpBQVBMIix7InByaWNlIjoiMTg3LjI1In1d".to_string(),
                is_valid_json: true,
                received_at: UnixNanos::from(10),
                ts_event: UnixNanos::from(10),
                ts_init: UnixNanos::from(10),
            },
        )
        .expect("custom data should convert to Python");
        assert_eq!(
            event
                .getattr(py, "frame_json")
                .unwrap()
                .extract::<String>(py)
                .unwrap(),
            r#"["price:AAPL",{"price":"187.25"}]"#
        );
    });
}
