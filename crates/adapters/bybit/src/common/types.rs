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

//! Result types for Bybit margin operations.
//!
//! These types are used for strategy-level communication of margin operation results.

use rust_decimal::Decimal;

use super::enums::BybitMarginMode;

/// Raw provider values for one coin in a unified account wallet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BybitAccountCoinInfo {
    pub coin: String,
    pub wallet_balance: String,
    pub equity: String,
    pub usd_value: String,
}

/// Exact decimal inputs and derived rate for converting one coin into account USD value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BybitAccountUsdConversionRate {
    /// The source coin.
    pub coin: String,
    /// Raw coin equity used as the conversion denominator.
    pub coin_equity: Decimal,
    /// Raw account USD value used as the conversion numerator.
    pub account_usd_value: Decimal,
    /// Account USD per one unit of the source coin.
    pub rate: Decimal,
}

impl BybitAccountUsdConversionRate {
    /// Derives a positive account USD conversion rate from exact provider decimal strings.
    ///
    /// # Errors
    ///
    /// Returns an error when either value is invalid or zero, division fails, or the rate is not
    /// positive.
    pub fn try_new(coin: &str, coin_equity: &str, account_usd_value: &str) -> anyhow::Result<Self> {
        let coin_info = BybitAccountCoinInfo {
            coin: coin.to_string(),
            wallet_balance: String::new(),
            equity: coin_equity.to_string(),
            usd_value: account_usd_value.to_string(),
        };
        Self::try_from_coin(&coin_info)
    }

    pub(crate) fn try_from_coin(coin: &BybitAccountCoinInfo) -> anyhow::Result<Self> {
        let coin_equity = Decimal::from_str_exact(&coin.equity).map_err(|e| {
            anyhow::anyhow!(
                "invalid {} equity '{}' for account USD conversion: {e}",
                coin.coin,
                coin.equity
            )
        })?;
        let account_usd_value = Decimal::from_str_exact(&coin.usd_value).map_err(|e| {
            anyhow::anyhow!(
                "invalid {} USD value '{}' for account USD conversion: {e}",
                coin.coin,
                coin.usd_value
            )
        })?;
        anyhow::ensure!(
            !coin_equity.is_zero(),
            "{} equity is zero; account USD conversion is unavailable",
            coin.coin
        );
        anyhow::ensure!(
            !account_usd_value.is_zero(),
            "{} USD value is zero; account USD conversion is unavailable",
            coin.coin
        );
        let rate = account_usd_value
            .checked_div(coin_equity)
            .ok_or_else(|| anyhow::anyhow!("{} account USD conversion overflow", coin.coin))?;
        anyhow::ensure!(
            rate > Decimal::ZERO,
            "{} account USD conversion rate must be positive, was {rate}",
            coin.coin
        );

        Ok(Self {
            coin: coin.coin.clone(),
            coin_equity,
            account_usd_value,
            rate,
        })
    }
}

/// Explicit result when an exact proposed option collateral effect is unavailable.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader.adapters.bybit", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.adapters.bybit")
)]
pub struct BybitOptionCollateralUnavailable {
    /// The verified account margin mode.
    pub margin_mode: BybitMarginMode,
    /// Authoritative inputs that the public V5 API does not provide.
    pub missing_authoritative_inputs: Vec<String>,
    /// The provider limitation which prevents an exact result.
    pub reason: String,
}

impl BybitOptionCollateralUnavailable {
    /// Returns the current provider-owned unavailability result.
    #[must_use]
    pub fn new(margin_mode: BybitMarginMode) -> Self {
        Self {
            margin_mode,
            missing_authoritative_inputs: vec![
                "option_margin_factors".to_string(),
                "order_open_close_allocation".to_string(),
                "post_order_account_collateral".to_string(),
            ],
            reason: "Bybit V5 provides no non-mutating option order-margin pre-check or versioned option margin-factor endpoint".to_string(),
        }
    }
}

/// Result from a Bybit borrow operation for strategy consumption.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader.adapters.bybit", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.adapters.bybit")
)]
pub struct BybitMarginBorrowResult {
    /// The coin that was borrowed.
    pub coin: String,
    /// The amount that was borrowed.
    pub amount: String,
    /// Whether the borrow operation was successful.
    pub success: bool,
    /// Error message if the operation failed.
    pub message: String,
    /// UNIX timestamp (nanoseconds) when the event occurred.
    pub ts_event: u64,
    /// UNIX timestamp (nanoseconds) when the object was initialized.
    pub ts_init: u64,
}

/// Result from a Bybit repay operation for strategy consumption.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader.adapters.bybit", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.adapters.bybit")
)]
pub struct BybitMarginRepayResult {
    /// The coin that was repaid.
    pub coin: String,
    /// The amount that was repaid (None if repaying all).
    pub amount: Option<String>,
    /// Whether the repay operation was successful.
    pub success: bool,
    /// The result status from Bybit API.
    pub result_status: String,
    /// Error message if the operation failed.
    pub message: String,
    /// UNIX timestamp (nanoseconds) when the event occurred.
    pub ts_event: u64,
    /// UNIX timestamp (nanoseconds) when the object was initialized.
    pub ts_init: u64,
}

/// Result with current borrowed amount on Bybit.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader.adapters.bybit", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.adapters.bybit")
)]
pub struct BybitMarginStatusResult {
    /// The coin being queried.
    pub coin: String,
    /// The current borrowed amount.
    pub borrow_amount: String,
    /// UNIX timestamp (nanoseconds) when the event occurred.
    pub ts_event: u64,
    /// UNIX timestamp (nanoseconds) when the object was initialized.
    pub ts_init: u64,
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use rust_decimal::Decimal;

    use super::*;

    #[rstest]
    fn account_usd_conversion_uses_provider_values_without_parity_assumption() {
        let conversion = BybitAccountUsdConversionRate::try_new("USDT", "100", "99.5").unwrap();

        assert_eq!(conversion.rate, Decimal::new(995, 3));
    }

    #[rstest]
    #[case::missing("", "100")]
    #[case::zero_equity("0", "100")]
    #[case::zero_usd_value("100", "0")]
    fn account_usd_conversion_rejects_missing_or_zero_values(
        #[case] equity: &str,
        #[case] usd_value: &str,
    ) {
        assert!(BybitAccountUsdConversionRate::try_new("USDT", equity, usd_value).is_err());
    }
}
