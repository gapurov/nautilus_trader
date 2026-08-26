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

//! Python bindings for the Unusual Whales informational data adapter.

mod config;
mod factories;
mod identifiers;

use nautilus_common::factories::{ClientConfig, DataClientFactory};
use nautilus_core::python::{to_pyruntime_err, to_pyvalue_err};
use nautilus_model::data::ensure_rust_extractor_registered;
use nautilus_system::get_global_pyo3_registry;
use pyo3::prelude::*;

use crate::{
    common::consts::UNUSUAL_WHALES,
    config::UnusualWhalesDataClientConfig,
    data_types::{
        UnusualWhalesOutcome, UnusualWhalesProviderState, UnusualWhalesProviderStateKind,
        UnusualWhalesRestResult, UnusualWhalesWebSocketEvent, register_unusual_whales_custom_data,
    },
    factories::UnusualWhalesDataClientFactory,
    generated::{
        CHANNEL_FORM_COUNT, CHANNELS, GET_OPERATION_COUNT, OPERATION_COUNT, OPERATIONS, PATH_COUNT,
        POST_OPERATION_COUNT, SOURCE_SHA256, SOURCE_URL, UnusualWhalesChannelForm,
        UnusualWhalesOperationId,
    },
};

#[pyfunction]
#[pyo3_stub_gen::derive::gen_stub_pyfunction(module = "nautilus_trader.adapters.unusual_whales")]
fn unusual_whales_operation_ids() -> Vec<&'static str> {
    OPERATIONS
        .iter()
        .map(|operation| operation.operation_id)
        .collect()
}

#[pyfunction]
#[pyo3_stub_gen::derive::gen_stub_pyfunction(module = "nautilus_trader.adapters.unusual_whales")]
fn unusual_whales_channel_forms() -> Vec<&'static str> {
    CHANNELS.iter().map(|channel| channel.form).collect()
}

#[expect(clippy::needless_pass_by_value)]
fn extract_unusual_whales_data_factory(
    py: Python<'_>,
    factory: Py<PyAny>,
) -> PyResult<Box<dyn DataClientFactory>> {
    factory
        .extract::<UnusualWhalesDataClientFactory>(py)
        .map(|factory| Box::new(factory) as Box<dyn DataClientFactory>)
        .map_err(|e| {
            to_pyvalue_err(format!(
                "Failed to extract UnusualWhalesDataClientFactory: {e}"
            ))
        })
}

#[expect(clippy::needless_pass_by_value)]
fn extract_unusual_whales_data_config(
    py: Python<'_>,
    config: Py<PyAny>,
) -> PyResult<Box<dyn ClientConfig>> {
    config
        .extract::<UnusualWhalesDataClientConfig>(py)
        .map(|config| Box::new(config) as Box<dyn ClientConfig>)
        .map_err(|e| {
            to_pyvalue_err(format!(
                "Failed to extract UnusualWhalesDataClientConfig: {e}"
            ))
        })
}

/// Exposed through nautilus_trader.adapters.unusual_whales.
///
/// # Errors
///
/// Returns an error if a class, function, custom-data extractor, factory extractor, or config
/// extractor cannot be registered.
#[pymodule]
pub fn unusual_whales(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add(stringify!(UNUSUAL_WHALES), UNUSUAL_WHALES)?;
    m.add(stringify!(SOURCE_URL), SOURCE_URL)?;
    m.add(stringify!(SOURCE_SHA256), SOURCE_SHA256)?;
    m.add(stringify!(PATH_COUNT), PATH_COUNT)?;
    m.add(stringify!(OPERATION_COUNT), OPERATION_COUNT)?;
    m.add(stringify!(GET_OPERATION_COUNT), GET_OPERATION_COUNT)?;
    m.add(stringify!(POST_OPERATION_COUNT), POST_OPERATION_COUNT)?;
    m.add(stringify!(CHANNEL_FORM_COUNT), CHANNEL_FORM_COUNT)?;
    m.add_class::<UnusualWhalesDataClientConfig>()?;
    m.add_class::<UnusualWhalesDataClientFactory>()?;
    m.add_class::<UnusualWhalesOperationId>()?;
    m.add_class::<UnusualWhalesChannelForm>()?;
    m.add_class::<UnusualWhalesOutcome>()?;
    m.add_class::<UnusualWhalesProviderStateKind>()?;
    m.add_class::<UnusualWhalesRestResult>()?;
    m.add_class::<UnusualWhalesWebSocketEvent>()?;
    m.add_class::<UnusualWhalesProviderState>()?;
    m.add_function(wrap_pyfunction!(unusual_whales_operation_ids, m)?)?;
    m.add_function(wrap_pyfunction!(unusual_whales_channel_forms, m)?)?;

    register_unusual_whales_custom_data();
    let _result = ensure_rust_extractor_registered::<UnusualWhalesRestResult>();
    let _result = ensure_rust_extractor_registered::<UnusualWhalesWebSocketEvent>();
    let _result = ensure_rust_extractor_registered::<UnusualWhalesProviderState>();

    let registry = get_global_pyo3_registry();
    registry
        .register_factory_extractor(
            UNUSUAL_WHALES.to_string(),
            extract_unusual_whales_data_factory,
        )
        .map_err(|e| {
            to_pyruntime_err(format!(
                "Failed to register Unusual Whales data factory extractor: {e}"
            ))
        })?;
    registry
        .register_config_extractor(
            "UnusualWhalesDataClientConfig".to_string(),
            extract_unusual_whales_data_config,
        )
        .map_err(|e| {
            to_pyruntime_err(format!(
                "Failed to register Unusual Whales data config extractor: {e}"
            ))
        })?;
    Ok(())
}
