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

//! Interactive Brokers execution diagnostics exposed as custom data.

use std::sync::Arc;

use nautilus_core::{Params, UnixNanos};
use nautilus_model::{
    data::{CustomData, CustomDataTrait, DataType},
    identifiers::{AccountId, ClientOrderId, InstrumentId, StrategyId},
};
use nautilus_persistence_macros::custom_data;
use rust_decimal::Decimal;

/// Non-mutating Interactive Brokers `whatIf` order preview result.
#[custom_data(
    pyo3,
    no_arrow,
    stub_module = "nautilus_trader.adapters.interactive_brokers"
)]
pub struct InteractiveBrokersOrderPreview {
    pub account_id: AccountId,
    #[custom_data_field(serde)]
    pub strategy_id: StrategyId,
    pub instrument_id: InstrumentId,
    #[custom_data_field(serde)]
    pub client_order_id: ClientOrderId,
    pub status: String,
    #[custom_data_field(serde)]
    pub initial_margin_before: Option<Decimal>,
    #[custom_data_field(serde)]
    pub initial_margin_change: Option<Decimal>,
    #[custom_data_field(serde)]
    pub initial_margin_after: Option<Decimal>,
    #[custom_data_field(serde)]
    pub maintenance_margin_before: Option<Decimal>,
    #[custom_data_field(serde)]
    pub maintenance_margin_change: Option<Decimal>,
    #[custom_data_field(serde)]
    pub maintenance_margin_after: Option<Decimal>,
    #[custom_data_field(serde)]
    pub equity_with_loan_before: Option<Decimal>,
    #[custom_data_field(serde)]
    pub equity_with_loan_change: Option<Decimal>,
    #[custom_data_field(serde)]
    pub equity_with_loan_after: Option<Decimal>,
    #[custom_data_field(serde)]
    pub initial_margin_before_outside_rth: Option<Decimal>,
    #[custom_data_field(serde)]
    pub initial_margin_change_outside_rth: Option<Decimal>,
    #[custom_data_field(serde)]
    pub initial_margin_after_outside_rth: Option<Decimal>,
    #[custom_data_field(serde)]
    pub maintenance_margin_before_outside_rth: Option<Decimal>,
    #[custom_data_field(serde)]
    pub maintenance_margin_change_outside_rth: Option<Decimal>,
    #[custom_data_field(serde)]
    pub maintenance_margin_after_outside_rth: Option<Decimal>,
    #[custom_data_field(serde)]
    pub equity_with_loan_before_outside_rth: Option<Decimal>,
    #[custom_data_field(serde)]
    pub equity_with_loan_change_outside_rth: Option<Decimal>,
    #[custom_data_field(serde)]
    pub equity_with_loan_after_outside_rth: Option<Decimal>,
    #[custom_data_field(serde)]
    pub commission: Option<Decimal>,
    #[custom_data_field(serde)]
    pub minimum_commission: Option<Decimal>,
    #[custom_data_field(serde)]
    pub maximum_commission: Option<Decimal>,
    pub commission_currency: String,
    pub margin_currency: String,
    #[custom_data_field(serde)]
    pub suggested_size: Option<Decimal>,
    pub reject_reason: String,
    pub warning_text: String,
    pub ts_event: UnixNanos,
    pub ts_init: UnixNanos,
}

impl InteractiveBrokersOrderPreview {
    pub(crate) fn unavailable(
        account_id: AccountId,
        strategy_id: StrategyId,
        instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        reason: String,
        ts_init: UnixNanos,
    ) -> Self {
        Self {
            account_id,
            strategy_id,
            instrument_id,
            client_order_id,
            status: "UNAVAILABLE".to_string(),
            initial_margin_before: None,
            initial_margin_change: None,
            initial_margin_after: None,
            maintenance_margin_before: None,
            maintenance_margin_change: None,
            maintenance_margin_after: None,
            equity_with_loan_before: None,
            equity_with_loan_change: None,
            equity_with_loan_after: None,
            initial_margin_before_outside_rth: None,
            initial_margin_change_outside_rth: None,
            initial_margin_after_outside_rth: None,
            maintenance_margin_before_outside_rth: None,
            maintenance_margin_change_outside_rth: None,
            maintenance_margin_after_outside_rth: None,
            equity_with_loan_before_outside_rth: None,
            equity_with_loan_change_outside_rth: None,
            equity_with_loan_after_outside_rth: None,
            commission: None,
            minimum_commission: None,
            maximum_commission: None,
            commission_currency: String::new(),
            margin_currency: String::new(),
            suggested_size: None,
            reject_reason: reason,
            warning_text: String::new(),
            ts_event: ts_init,
            ts_init,
        }
    }

    pub(crate) fn into_custom_data(self) -> CustomData {
        let instrument_id = self.instrument_id;
        let mut metadata = Params::new();
        metadata.insert(
            "instrument_id".to_string(),
            serde_json::Value::String(instrument_id.to_string()),
        );
        let value: Arc<dyn CustomDataTrait> = Arc::new(self);
        let data_type = DataType::new(
            value.type_name(),
            Some(metadata),
            Some(instrument_id.to_string()),
        );
        CustomData::new(value, data_type)
    }
}

/// Registers Interactive Brokers custom data codecs.
pub fn register_interactive_brokers_custom_data() {
    let _ = nautilus_model::data::ensure_custom_data_json_registered::<
        InteractiveBrokersOrderPreview,
    >();
}
