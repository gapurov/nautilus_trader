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

//! Instrument parsing utilities for converting IB ContractDetails to Nautilus instruments.

use std::str::FromStr;

use anyhow::Context;
use ibapi::contracts::SecurityType;
use jiff::{civil::DateTime, tz::AmbiguousOffset};
use nautilus_core::{UnixNanos, datetime::get_timezone, time::get_atomic_clock_realtime};
use nautilus_model::{
    enums::AssetClass,
    identifiers::{InstrumentId, Symbol},
    instruments::{
        Cfd, Commodity, CryptoPerpetual, CurrencyPair, Equity, FuturesContract, FuturesSpread,
        IndexInstrument, InstrumentAny, OptionContract, OptionSpread,
    },
    types::{Currency, Price, Quantity},
};
use rust_decimal::Decimal;
use ustr::Ustr;

use crate::common::{
    contract_to_params,
    enums::{IbOptionRight, IbSecurityType},
};

/// Convert tick size to precision value.
#[must_use]
pub fn tick_size_to_precision(tick_size: f64) -> u8 {
    if tick_size <= 0.0 {
        return 8; // Default precision for zero or negative tick sizes
    }

    // Count decimal places
    let s = format!("{:.10}", tick_size);
    let s = s.trim_end_matches('0');
    let parts: Vec<&str> = s.split('.').collect();

    if parts.len() == 2 {
        parts[1].len().min(8) as u8
    } else {
        0
    }
}

/// Converts IBKR derivative expiration facts to [`UnixNanos`].
///
/// Explicit timestamps use their timezone token, or `ContractDetails.time_zone_id` when omitted.
/// Date-only values require `ContractDetails.last_trade_time` and `time_zone_id`. Standard U.S.
/// equity options may omit `last_trade_time`; when the last-trade and real-expiration dates agree,
/// they use the OCC expiration time of 23:59 Eastern.
///
/// # Errors
///
/// Returns an error if the exact expiration or last-trade instant cannot be resolved.
pub fn expiry_timestring_to_unix_nanos(
    expiry: &str,
    details: Option<&ibapi::contracts::ContractDetails>,
) -> anyhow::Result<UnixNanos> {
    let expiry = expiry.trim();
    if expiry.is_empty() {
        anyhow::bail!("IBKR derivative contract is missing its last trade date or timestamp");
    }

    let mut parts = expiry.split_whitespace();
    let Some(date) = parts.next() else {
        anyhow::bail!("IBKR derivative contract is missing its last trade date or timestamp");
    };
    let explicit_time = parts.next();
    let explicit_timezone = parts.next();
    if parts.next().is_some() {
        anyhow::bail!("Invalid IBKR derivative expiration timestamp '{expiry}'");
    }

    let (date, time, timezone) = if let Some(time) = explicit_time {
        let timezone = explicit_timezone
            .or_else(|| {
                details
                    .map(|details| details.time_zone_id.trim())
                    .filter(|timezone| !timezone.is_empty())
            })
            .with_context(|| {
                format!("IBKR derivative timestamp '{expiry}' is missing its timezone")
            })?;
        (date, time, timezone)
    } else {
        let details = details.with_context(|| {
            format!("IBKR derivative last trade date '{date}' has no ContractDetails")
        })?;

        if date.len() != 8 {
            let real_expiration_date = details.real_expiration_date.trim();
            anyhow::bail!(
                "IBKR derivative last trade value '{date}' is not an exact date; real expiration date was '{real_expiration_date}'"
            );
        }

        let timezone = details.time_zone_id.trim();
        if timezone.is_empty() {
            anyhow::bail!(
                "IBKR derivative last trade date '{date}' is missing ContractDetails.time_zone_id"
            );
        }

        let time = details.last_trade_time.trim();
        if time.is_empty() {
            let real_expiration_date = details.real_expiration_date.trim();
            let is_standard_us_equity_option = details.contract.security_type
                == SecurityType::Option
                && details.under_security_type == "STK"
                && details.contract.currency.as_str() == "USD"
                && matches!(timezone, "US/Eastern" | "America/New_York");
            if is_standard_us_equity_option && real_expiration_date == date {
                (real_expiration_date, "23:59", timezone)
            } else {
                anyhow::bail!(
                    "IBKR derivative last trade date '{date}' is missing ContractDetails.last_trade_time"
                );
            }
        } else {
            (date, time, timezone)
        }
    };

    let format = match time.len() {
        5 => "%Y%m%d %H:%M",
        8 => "%Y%m%d %H:%M:%S",
        _ => anyhow::bail!("Invalid IBKR derivative last trade time '{time}' for date '{date}'"),
    };
    let local_label = format!("{date} {time}");
    let local = DateTime::strptime(format, &local_label)
        .with_context(|| format!("Invalid IBKR derivative expiration timestamp '{local_label}'"))?;
    let timezone_name = timezone;
    let timezone = get_timezone(timezone_name).with_context(|| {
        format!("Unknown IBKR derivative timezone '{timezone_name}' for '{local_label}'")
    })?;
    let ambiguous = timezone.to_ambiguous_timestamp(local);
    let timestamp = match ambiguous.offset() {
        AmbiguousOffset::Unambiguous { .. } => ambiguous.unambiguous()?,
        AmbiguousOffset::Fold { .. } => anyhow::bail!(
            "IBKR derivative expiration timestamp '{local_label}' is ambiguous in timezone '{timezone_name}'"
        ),
        AmbiguousOffset::Gap { .. } => anyhow::bail!(
            "IBKR derivative expiration timestamp '{local_label}' does not exist in timezone '{timezone_name}'"
        ),
    };
    let nanos = u64::try_from(timestamp.as_nanosecond()).with_context(|| {
        format!("IBKR derivative expiration timestamp '{local_label}' was before the Unix epoch")
    })?;

    Ok(UnixNanos::from(nanos))
}

/// Parse an IB ContractDetails to a Nautilus instrument.
///
/// # Errors
///
/// Returns an error if parsing fails.
pub fn parse_ib_contract_to_instrument(
    details: &ibapi::contracts::ContractDetails,
    instrument_id: InstrumentId,
) -> anyhow::Result<InstrumentAny> {
    let sec_type = &details.contract.security_type;

    match sec_type {
        SecurityType::Stock => Ok(parse_equity_contract(details, instrument_id)),
        SecurityType::ForexPair => Ok(parse_forex_contract(details, instrument_id)),
        SecurityType::Crypto => Ok(parse_crypto_contract(details, instrument_id)),
        SecurityType::Future | SecurityType::ContinuousFuture => {
            parse_futures_contract(details, instrument_id)
        }
        SecurityType::Option => parse_option_contract(details, instrument_id),
        SecurityType::FuturesOption => parse_option_contract(details, instrument_id), // FOP uses same parsing as OPT
        SecurityType::Index => Ok(parse_index_contract(details, instrument_id)),
        SecurityType::CFD => Ok(parse_cfd_contract(details, instrument_id)),
        SecurityType::Commodity => Ok(parse_commodity_contract(details, instrument_id)),
        SecurityType::Bond => Ok(parse_bond_contract(details, instrument_id)),
        _ => anyhow::bail!("Unsupported security type: {:?}", sec_type),
    }
}

fn ib_contract_info(details: &ibapi::contracts::ContractDetails) -> nautilus_core::Params {
    let mut info = nautilus_core::Params::new();
    let mut contract = serde_json::Map::new();

    let contract_params = contract_to_params(&details.contract);
    for (key, value) in &contract_params {
        contract.insert(key.clone(), value.clone());
    }

    info.insert("contract".to_string(), serde_json::Value::Object(contract));
    info.insert(
        "priceMagnifier".to_string(),
        serde_json::Value::from(details.price_magnifier),
    );
    info
}

fn ib_contract_info_for_contract(contract: &ibapi::contracts::Contract) -> nautilus_core::Params {
    let mut info = nautilus_core::Params::new();
    let mut contract_map = serde_json::Map::new();
    let contract_params = contract_to_params(contract);

    for (key, value) in &contract_params {
        contract_map.insert(key.clone(), value.clone());
    }

    info.insert(
        "contract".to_string(),
        serde_json::Value::Object(contract_map),
    );
    info
}

fn sec_type_to_asset_class(sec_type: &str) -> AssetClass {
    match IbSecurityType::from_str(sec_type).ok() {
        Some(IbSecurityType::Stock) => AssetClass::Equity,
        Some(IbSecurityType::Index) => AssetClass::Index,
        Some(IbSecurityType::ForexPair) => AssetClass::FX,
        Some(IbSecurityType::Bond) => AssetClass::Debt,
        Some(IbSecurityType::Commodity) => AssetClass::Commodity,
        Some(IbSecurityType::Future) => AssetClass::Index,
        _ => AssetClass::Equity,
    }
}

/// Parse equity contract (STK).
fn parse_equity_contract(
    details: &ibapi::contracts::ContractDetails,
    instrument_id: InstrumentId,
) -> InstrumentAny {
    let price_precision = tick_size_to_precision(details.min_tick);
    let timestamp = get_atomic_clock_realtime().get_time_ns();

    let instrument = Equity::new(
        instrument_id,
        Symbol::from(details.contract.local_symbol.as_str()),
        None, // isin
        Currency::from(details.contract.currency.to_string()),
        price_precision,
        Price::new(details.min_tick, price_precision),
        Some(Quantity::new(100.0, 0)),   // Standard lot size for stocks
        None,                            // max_quantity
        None,                            // min_quantity
        None,                            // max_price
        None,                            // min_price
        None,                            // margin_init
        None,                            // margin_maint
        None,                            // maker_fee
        None,                            // taker_fee
        None,                            // tick_scheme
        Some(ib_contract_info(details)), // info
        timestamp,
        timestamp,
    );

    InstrumentAny::from(instrument)
}

/// Parse forex contract (CASH).
fn parse_forex_contract(
    details: &ibapi::contracts::ContractDetails,
    instrument_id: InstrumentId,
) -> InstrumentAny {
    let price_precision = tick_size_to_precision(details.min_tick);
    let size_precision = tick_size_to_precision(details.min_size);
    let timestamp = get_atomic_clock_realtime().get_time_ns();

    let instrument = CurrencyPair::new(
        instrument_id,
        Symbol::from(details.contract.local_symbol.as_str()),
        Currency::from(details.contract.symbol.to_string()),
        Currency::from(details.contract.currency.to_string()),
        price_precision,
        size_precision,
        Price::new(details.min_tick, price_precision),
        Quantity::new(details.size_increment, size_precision),
        None,                            // multiplier
        None,                            // lot_size
        None,                            // max_quantity
        None,                            // min_quantity
        None,                            // max_notional
        None,                            // min_notional
        None,                            // max_price
        None,                            // min_price
        None,                            // margin_init
        None,                            // margin_maint
        None,                            // maker_fee
        None,                            // taker_fee
        None,                            // tick_scheme
        Some(ib_contract_info(details)), // info
        timestamp,
        timestamp,
    );

    InstrumentAny::from(instrument)
}

/// Parse crypto contract (CRYPTO).
fn parse_crypto_contract(
    details: &ibapi::contracts::ContractDetails,
    instrument_id: InstrumentId,
) -> InstrumentAny {
    let price_precision = tick_size_to_precision(details.min_tick);
    let size_precision = tick_size_to_precision(details.min_size);
    let timestamp = get_atomic_clock_realtime().get_time_ns();

    let instrument = CryptoPerpetual::new(
        instrument_id,
        Symbol::from(details.contract.local_symbol.as_str()),
        Currency::from(details.contract.symbol.to_string()),
        Currency::from(details.contract.currency.to_string()),
        Currency::from(details.contract.currency.to_string()),
        true, // is_inverse
        price_precision,
        size_precision,
        Price::new(details.min_tick, price_precision),
        Quantity::new(details.size_increment, size_precision),
        None, // multiplier
        None, // lot_size
        None, // max_quantity
        Some(Quantity::new(details.min_size, size_precision)),
        None,                            // max_notional
        None,                            // min_notional
        None,                            // max_price
        None,                            // min_price
        None,                            // margin_init
        None,                            // margin_maint
        None,                            // maker_fee
        None,                            // taker_fee
        None,                            // tick_scheme
        Some(ib_contract_info(details)), // info
        timestamp,
        timestamp,
    );

    InstrumentAny::from(instrument)
}

fn parse_contract_multiplier(multiplier: &str, default: f64) -> Quantity {
    if multiplier.is_empty() {
        return Quantity::new(default, 0);
    }

    Quantity::from_str(multiplier).unwrap_or_else(|e| {
        tracing::warn!(
            "Failed to parse IB contract multiplier '{multiplier}', using default {default}: {e}"
        );
        Quantity::new(default, 0)
    })
}

/// Parse futures contract (FUT).
fn parse_futures_contract(
    details: &ibapi::contracts::ContractDetails,
    instrument_id: InstrumentId,
) -> anyhow::Result<InstrumentAny> {
    let price_precision = tick_size_to_precision(details.min_tick);
    let timestamp = get_atomic_clock_realtime().get_time_ns();

    let expiration_ns = expiry_timestring_to_unix_nanos(
        &details.contract.last_trade_date_or_contract_month,
        Some(details),
    )
    .context("Failed to resolve IBKR futures contract expiration")?;

    let ninety_days_ns: u64 = 90 * 24 * 60 * 60 * 1_000_000_000;
    let activation_ns = expiration_ns
        .checked_sub(ninety_days_ns)
        .unwrap_or(UnixNanos::from(0)); // -90 days or 0 if underflow

    let multiplier = parse_contract_multiplier(&details.contract.multiplier, 1.0);

    let raw_symbol = if matches!(
        details.contract.security_type,
        SecurityType::ContinuousFuture
    ) && !details.contract.symbol.as_str().is_empty()
    {
        details.contract.symbol.as_str()
    } else {
        details.contract.local_symbol.as_str()
    };

    let instrument = FuturesContract::new(
        instrument_id,
        Symbol::from(raw_symbol),
        sec_type_to_asset_class(details.under_security_type.as_str()),
        None, // exchange
        Ustr::from(details.under_symbol.as_str()),
        activation_ns,
        expiration_ns,
        Currency::from(details.contract.currency.to_string()),
        price_precision,
        Price::new(details.min_tick, price_precision),
        multiplier,
        Quantity::new(1.0, 0),
        None,                            // max_quantity
        None,                            // min_quantity
        None,                            // max_price
        None,                            // min_price
        None,                            // margin_init
        None,                            // margin_maint
        None,                            // maker_fee
        None,                            // taker_fee
        None,                            // tick_scheme
        Some(ib_contract_info(details)), // info
        timestamp,
        timestamp,
    );

    Ok(InstrumentAny::from(instrument))
}

/// Parse option contract (OPT).
fn parse_option_contract(
    details: &ibapi::contracts::ContractDetails,
    instrument_id: InstrumentId,
) -> anyhow::Result<InstrumentAny> {
    let price_precision = tick_size_to_precision(details.min_tick);
    let timestamp = get_atomic_clock_realtime().get_time_ns();

    let expiration_ns = expiry_timestring_to_unix_nanos(
        &details.contract.last_trade_date_or_contract_month,
        Some(details),
    )
    .context("Failed to resolve IBKR option contract expiration")?;

    let ninety_days_ns: u64 = 90 * 24 * 60 * 60 * 1_000_000_000;
    let activation_ns = expiration_ns
        .checked_sub(ninety_days_ns)
        .unwrap_or(UnixNanos::from(0)); // -90 days or 0 if underflow

    // Parse option kind (CALL or PUT)
    let option_kind = details
        .contract
        .right
        .map(|right| IbOptionRight::from_str(right.as_str()))
        .transpose()?
        .context("Option contract missing right")?
        .option_kind();

    let multiplier = parse_contract_multiplier(&details.contract.multiplier, 100.0);
    let asset_class = sec_type_to_asset_class(details.under_security_type.as_str());
    let underlying =
        if details.under_security_type == "IND" && !details.under_symbol.starts_with('^') {
            format!("^{}", details.under_symbol)
        } else {
            details.under_symbol.clone()
        };

    let instrument = OptionContract::new(
        instrument_id,
        Symbol::from(details.contract.local_symbol.as_str()),
        asset_class,
        None, // exchange
        Ustr::from(underlying.as_str()),
        option_kind,
        Price::new(details.contract.strike, price_precision),
        Currency::from(details.contract.currency.to_string()),
        activation_ns,
        expiration_ns,
        price_precision,
        Price::new(details.min_tick, price_precision),
        multiplier,
        multiplier,
        None,                            // max_quantity
        None,                            // min_quantity
        None,                            // max_price
        None,                            // min_price
        None,                            // margin_init
        None,                            // margin_maint
        None,                            // maker_fee
        None,                            // taker_fee
        None,                            // tick_scheme
        Some(ib_contract_info(details)), // info
        timestamp,
        timestamp,
    );

    Ok(InstrumentAny::from(instrument))
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use ibapi::contracts::{
        Contract, ContractDetails, Currency, Exchange, OptionRight, SecurityType, Symbol,
    };
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        enums::AssetClass,
        identifiers::{InstrumentId, Symbol as NautilusSymbol, Venue},
        instruments::{Instrument, InstrumentAny},
        types::{Price, Quantity},
    };
    use rstest::rstest;
    use ustr::Ustr;

    use super::{
        expiry_timestring_to_unix_nanos, parse_contract_multiplier,
        parse_ib_contract_to_instrument, parse_option_spread_instrument_id,
    };

    fn derivative_expiration_details(
        real_expiration_date: &str,
        last_trade_time: &str,
        time_zone_id: &str,
    ) -> ContractDetails {
        ContractDetails {
            real_expiration_date: real_expiration_date.to_string(),
            last_trade_time: last_trade_time.to_string(),
            time_zone_id: time_zone_id.to_string(),
            trading_hours: vec!["20261127:CLOSED".to_string()],
            liquid_hours: vec!["20261127:CLOSED".to_string()],
            ..Default::default()
        }
    }

    #[rstest]
    #[case(
        "20260717",
        "20260717",
        "16:00:00",
        "US/Eastern",
        1_784_318_400_000_000_000
    )]
    #[case(
        "20261127",
        "20261127",
        "13:00:00",
        "US/Eastern",
        1_795_802_400_000_000_000
    )]
    #[case("20261127 13:00:00 US/Eastern", "", "", "", 1_795_802_400_000_000_000)]
    #[case(
        "20260319",
        "20260320",
        "17:00",
        "US/Eastern",
        1_773_954_000_000_000_000
    )]
    fn test_parse_derivative_expiration_uses_authoritative_fields(
        #[case] expiry: &str,
        #[case] real_expiration_date: &str,
        #[case] last_trade_time: &str,
        #[case] time_zone_id: &str,
        #[case] expected_ns: u64,
    ) {
        let details =
            derivative_expiration_details(real_expiration_date, last_trade_time, time_zone_id);
        let result = expiry_timestring_to_unix_nanos(expiry, Some(&details)).unwrap();

        assert_eq!(result, UnixNanos::from(expected_ns));
    }

    #[rstest]
    #[case("20260717", "", "US/Eastern", "last_trade_time")]
    #[case("20260717", "16:00:00", "", "time_zone_id")]
    #[case("20260717", "25:00:00", "US/Eastern", "Invalid IBKR derivative")]
    #[case("20261101", "01:30:00", "US/Eastern", "ambiguous")]
    fn test_parse_derivative_expiration_rejects_inexact_facts(
        #[case] expiry: &str,
        #[case] last_trade_time: &str,
        #[case] time_zone_id: &str,
        #[case] expected_error: &str,
    ) {
        let details = derivative_expiration_details(expiry, last_trade_time, time_zone_id);
        let error = expiry_timestring_to_unix_nanos(expiry, Some(&details)).unwrap_err();

        assert!(error.to_string().contains(expected_error));
    }

    #[rstest]
    fn test_parse_option_contract_prefixes_index_underlying() {
        let details = ContractDetails {
            contract: Contract {
                symbol: Symbol::from("SPXW"),
                security_type: SecurityType::Option,
                exchange: Exchange::from("SMART"),
                currency: Currency::from("USD"),
                local_symbol: "SPXW  260313P06630000".to_string(),
                last_trade_date_or_contract_month: "20260313".to_string(),
                right: Some(OptionRight::Put),
                strike: 6630.0,
                multiplier: "100".to_string(),
                ..Default::default()
            },
            min_tick: 0.05,
            under_symbol: "SPX".to_string(),
            under_security_type: "IND".to_string(),
            real_expiration_date: "20260313".to_string(),
            last_trade_time: "16:00:00".to_string(),
            time_zone_id: "US/Eastern".to_string(),
            ..Default::default()
        };
        let instrument_id = InstrumentId::new(
            NautilusSymbol::from("SPXW  260313P06630000"),
            Venue::from("SMART"),
        );

        let instrument = parse_ib_contract_to_instrument(&details, instrument_id).unwrap();

        let InstrumentAny::OptionContract(option) = instrument else {
            panic!("expected option contract");
        };

        assert_eq!(option.asset_class(), AssetClass::Index);
        assert_eq!(
            option.expiration_ns(),
            Some(UnixNanos::from(1_773_432_000_000_000_000))
        );
        assert_eq!(option.underlying(), Some(Ustr::from("^SPX")));
    }

    #[rstest]
    fn test_parse_date_only_us_equity_option_expiration_boundaries() {
        let details = ContractDetails {
            contract: Contract {
                contract_id: 851_284_071,
                symbol: Symbol::from("MU"),
                security_type: SecurityType::Option,
                exchange: Exchange::from("AMEX"),
                currency: Currency::from("USD"),
                local_symbol: "MU    260918C00880000".to_string(),
                last_trade_date_or_contract_month: "20260918".to_string(),
                right: Some(OptionRight::Call),
                strike: 880.0,
                multiplier: "100".to_string(),
                ..Default::default()
            },
            min_tick: 0.01,
            under_symbol: "MU".to_string(),
            under_security_type: "STK".to_string(),
            real_expiration_date: "20260918".to_string(),
            last_trade_time: String::new(),
            time_zone_id: "US/Eastern".to_string(),
            ..Default::default()
        };
        let instrument_id = InstrumentId::new(
            NautilusSymbol::from("MU    260918C00880000"),
            Venue::from("AMEX"),
        );

        let instrument = parse_ib_contract_to_instrument(&details, instrument_id).unwrap();
        let mut index_option = details.clone();
        index_option.under_security_type = "IND".to_string();
        let index_error =
            parse_ib_contract_to_instrument(&index_option, instrument_id).unwrap_err();
        let mut mismatched_expiration = details;
        mismatched_expiration.real_expiration_date = "20260919".to_string();
        let mismatch_error =
            parse_ib_contract_to_instrument(&mismatched_expiration, instrument_id).unwrap_err();

        assert_eq!(
            instrument.expiration_ns(),
            Some(UnixNanos::from(1_789_790_340_000_000_000))
        );
        assert!(format!("{index_error:#}").contains("missing ContractDetails.last_trade_time"));
        assert!(format!("{mismatch_error:#}").contains("missing ContractDetails.last_trade_time"));
    }

    #[rstest]
    fn test_parse_contract_preserves_price_magnifier_in_info() {
        let details = ContractDetails {
            contract: Contract {
                symbol: Symbol::from("AAPL"),
                security_type: SecurityType::Stock,
                exchange: Exchange::from("SMART"),
                primary_exchange: Exchange::from("NASDAQ"),
                currency: Currency::from("USD"),
                local_symbol: String::from("AAPL"),
                ..Default::default()
            },
            min_tick: 0.01,
            price_magnifier: 100,
            ..Default::default()
        };
        let instrument_id = InstrumentId::new(NautilusSymbol::from("AAPL"), Venue::from("XNAS"));

        let instrument = parse_ib_contract_to_instrument(&details, instrument_id).unwrap();
        let InstrumentAny::Equity(equity) = instrument else {
            panic!("expected equity");
        };

        assert_eq!(
            equity.info.unwrap().get("priceMagnifier"),
            Some(&serde_json::Value::from(100))
        );
    }

    #[rstest]
    #[case("100", 100.0)]
    #[case("", 1.0)]
    #[case("not-a-number", 1.0)]
    fn test_parse_contract_multiplier_uses_quantity_parser(
        #[case] multiplier: &str,
        #[case] expected: f64,
    ) {
        assert_eq!(
            parse_contract_multiplier(multiplier, 1.0),
            Quantity::new(expected, 0)
        );
    }

    #[rstest]
    fn test_parse_continuous_future_contract_uses_symbol_as_raw_symbol() {
        let details = ContractDetails {
            contract: Contract {
                symbol: Symbol::from("ES"),
                security_type: SecurityType::ContinuousFuture,
                exchange: Exchange::from("CME"),
                currency: Currency::from("USD"),
                local_symbol: String::new(),
                last_trade_date_or_contract_month: "20260320".to_string(),
                multiplier: "50".to_string(),
                ..Default::default()
            },
            min_tick: 0.25,
            under_symbol: "ES".to_string(),
            under_security_type: "IND".to_string(),
            real_expiration_date: "20260320".to_string(),
            last_trade_time: "16:00:00".to_string(),
            time_zone_id: "US/Central".to_string(),
            ..Default::default()
        };
        let instrument_id = InstrumentId::new(NautilusSymbol::from("ES"), Venue::from("CME"));

        let instrument = parse_ib_contract_to_instrument(&details, instrument_id).unwrap();

        let InstrumentAny::FuturesContract(future) = instrument else {
            panic!("expected futures contract");
        };

        assert_eq!(future.raw_symbol().as_str(), "ES");
    }

    #[rstest]
    fn test_parse_option_spread_uses_minimum_leg_tick() {
        let leg1 = ContractDetails {
            contract: Contract {
                symbol: Symbol::from("SPY"),
                security_type: SecurityType::Option,
                exchange: Exchange::from("SMART"),
                currency: Currency::from("USD"),
                local_symbol: "SPY   260120C00400000".to_string(),
                multiplier: "100".to_string(),
                ..Default::default()
            },
            min_tick: 0.05,
            under_symbol: "SPY".to_string(),
            ..Default::default()
        };
        let leg2 = ContractDetails {
            contract: Contract {
                symbol: Symbol::from("SPY"),
                security_type: SecurityType::Option,
                exchange: Exchange::from("SMART"),
                currency: Currency::from("USD"),
                local_symbol: "SPY   260120C00410000".to_string(),
                multiplier: "100".to_string(),
                ..Default::default()
            },
            min_tick: 0.01,
            under_symbol: "SPY".to_string(),
            ..Default::default()
        };
        let instrument_id =
            InstrumentId::from("(1)SPY   260120C00400000_((-1))SPY   260120C00410000.SMART");

        let spread = parse_option_spread_instrument_id(
            instrument_id,
            &[(&leg1, 1), (&leg2, -1)],
            None,
            None,
        )
        .unwrap();

        assert_eq!(spread.price_precision(), 2);
        assert_eq!(spread.price_increment(), Price::from("0.01"));
    }
}

/// Parse index contract (IND).
///
/// Note: Indices are typically not directly tradable. This creates a CurrencyPair
/// representation as a placeholder until IndexInstrument type is available.
fn parse_index_contract(
    details: &ibapi::contracts::ContractDetails,
    instrument_id: InstrumentId,
) -> InstrumentAny {
    let price_precision = tick_size_to_precision(details.min_tick);
    let size_precision = tick_size_to_precision(details.min_size);
    let timestamp = get_atomic_clock_realtime().get_time_ns();

    let instrument = IndexInstrument::new(
        instrument_id,
        Symbol::from(details.contract.local_symbol.as_str()),
        Currency::from(details.contract.currency.to_string()),
        price_precision,
        size_precision,
        Price::new(details.min_tick, price_precision),
        Quantity::new(details.size_increment, size_precision),
        None,
        Some(ib_contract_info(details)), // info
        timestamp,
        timestamp,
    );

    InstrumentAny::from(instrument)
}

/// Parse a spread instrument ID into an OptionSpread instrument.
///
/// This implements the same logic as Python's `parse_spread_instrument_id`.
/// Uses contract details from the first leg to determine spread properties.
///
/// # Errors
///
/// Returns an error if parsing fails.
pub fn parse_spread_instrument_id(
    instrument_id: InstrumentId,
    leg_contract_details: &[(&ibapi::contracts::ContractDetails, i32)],
    timestamp_ns: Option<UnixNanos>,
) -> anyhow::Result<OptionSpread> {
    if leg_contract_details.is_empty() {
        anyhow::bail!("leg_contract_details must be provided");
    }

    // Use contract details from first leg
    let (first_details, _) = leg_contract_details[0];
    let first_contract = &first_details.contract;

    // Extract properties from the first leg contract details
    let currency = Currency::from(first_contract.currency.to_string());
    let underlying = if !first_details.under_symbol.is_empty() {
        Ustr::from(first_details.under_symbol.as_str())
    } else {
        Ustr::from(first_contract.symbol.as_str())
    };

    // Parse multiplier
    let multiplier_str = first_contract.multiplier.to_string();
    let multiplier =
        Quantity::from_str(&multiplier_str).unwrap_or_else(|_| Quantity::new(100.0, 0)); // Default to 100 for options

    // Determine asset class based on security type
    let asset_class = match first_contract.security_type {
        ibapi::contracts::SecurityType::FuturesOption => AssetClass::Index, // Futures options
        _ => AssetClass::Equity,                                            // Equity options
    };

    // Calculate price precision and increment from the finest leg tick.
    let min_tick = leg_contract_details
        .iter()
        .map(|(details, _)| details.min_tick)
        .fold(first_details.min_tick, f64::min);
    let price_precision = tick_size_to_precision(min_tick);
    let price_increment = Price::new(min_tick, price_precision);

    // Use provided timestamp or current time
    let timestamp = timestamp_ns.unwrap_or_else(|| get_atomic_clock_realtime().get_time_ns());

    // For options spreads, lot size equals multiplier (same as individual option contracts)
    let lot_size = multiplier;

    // Create the spread instrument
    let spread = OptionSpread::new_checked(
        instrument_id,
        Symbol::from(instrument_id.symbol.as_str()), // raw_symbol
        asset_class,
        None, // exchange (optional)
        underlying,
        Ustr::from("SPREAD"), // strategy_type
        UnixNanos::new(0),    // activation_ns (spreads don't have single activation dates)
        UnixNanos::new(0),    // expiration_ns (spreads don't have single expiration dates)
        currency,
        price_precision,
        price_increment,
        multiplier,
        lot_size,
        None,                // max_quantity
        None,                // min_quantity
        None,                // max_price
        None,                // min_price
        Some(Decimal::ZERO), // margin_init
        Some(Decimal::ZERO), // margin_maint
        Some(Decimal::ZERO), // maker_fee
        Some(Decimal::ZERO), // taker_fee
        None,                // tick_scheme
        None,                // info
        timestamp,
        timestamp,
    )?;

    Ok(spread)
}

pub fn parse_option_spread_instrument_id(
    instrument_id: InstrumentId,
    leg_contract_details: &[(&ibapi::contracts::ContractDetails, i32)],
    bag_contract: Option<&ibapi::contracts::Contract>,
    timestamp_ns: Option<UnixNanos>,
) -> anyhow::Result<OptionSpread> {
    let mut spread = parse_spread_instrument_id(instrument_id, leg_contract_details, timestamp_ns)?;
    spread.info = bag_contract.map(ib_contract_info_for_contract);
    Ok(spread)
}

pub fn parse_futures_spread_instrument_id(
    instrument_id: InstrumentId,
    leg_contract_details: &[(&ibapi::contracts::ContractDetails, i32)],
    bag_contract: Option<&ibapi::contracts::Contract>,
    timestamp_ns: Option<UnixNanos>,
) -> anyhow::Result<FuturesSpread> {
    if leg_contract_details.is_empty() {
        anyhow::bail!("leg_contract_details must be provided");
    }

    let (first_details, _) = leg_contract_details[0];
    let first_contract = &first_details.contract;
    let currency = Currency::from(first_contract.currency.to_string());
    let underlying = if !first_details.under_symbol.is_empty() {
        Ustr::from(first_details.under_symbol.as_str())
    } else {
        Ustr::from(first_contract.symbol.as_str())
    };
    let multiplier = Quantity::from_str(&first_contract.multiplier.to_string())
        .unwrap_or_else(|_| Quantity::new(1.0, 0));
    let min_tick = leg_contract_details
        .iter()
        .map(|(details, _)| details.min_tick)
        .fold(first_details.min_tick, f64::min);
    let price_precision = tick_size_to_precision(min_tick);
    let price_increment = Price::new(min_tick, price_precision);
    let timestamp = timestamp_ns.unwrap_or_else(|| get_atomic_clock_realtime().get_time_ns());

    Ok(FuturesSpread::new_checked(
        instrument_id,
        Symbol::from(instrument_id.symbol.as_str()),
        AssetClass::Index,
        None,
        underlying,
        Ustr::from("SPREAD"),
        UnixNanos::new(0),
        UnixNanos::new(0),
        currency,
        price_precision,
        price_increment,
        multiplier,
        Quantity::new(1.0, 0),
        None,
        None,
        None,
        None,
        Some(Decimal::ZERO),
        Some(Decimal::ZERO),
        Some(Decimal::ZERO),
        Some(Decimal::ZERO),
        None,
        bag_contract.map(ib_contract_info_for_contract),
        timestamp,
        timestamp,
    )?)
}

pub fn parse_spread_instrument_any(
    instrument_id: InstrumentId,
    leg_contract_details: &[(&ibapi::contracts::ContractDetails, i32)],
    bag_contract: Option<&ibapi::contracts::Contract>,
    timestamp_ns: Option<UnixNanos>,
) -> anyhow::Result<InstrumentAny> {
    let has_future = leg_contract_details.iter().any(|(details, _)| {
        matches!(
            details.contract.security_type,
            SecurityType::Future | SecurityType::ContinuousFuture
        )
    });

    if has_future {
        Ok(InstrumentAny::from(parse_futures_spread_instrument_id(
            instrument_id,
            leg_contract_details,
            bag_contract,
            timestamp_ns,
        )?))
    } else {
        Ok(InstrumentAny::from(parse_option_spread_instrument_id(
            instrument_id,
            leg_contract_details,
            bag_contract,
            timestamp_ns,
        )?))
    }
}

/// Parse CFD contract (CFD).
fn parse_cfd_contract(
    details: &ibapi::contracts::ContractDetails,
    instrument_id: InstrumentId,
) -> InstrumentAny {
    let price_precision = tick_size_to_precision(details.min_tick);
    let size_precision = tick_size_to_precision(details.min_size);
    let timestamp = get_atomic_clock_realtime().get_time_ns();

    let base_currency = details
        .contract
        .local_symbol
        .contains('.')
        .then(|| Currency::from(details.contract.symbol.to_string()));

    let instrument = Cfd::new(
        instrument_id,
        Symbol::from(details.contract.local_symbol.as_str()),
        sec_type_to_asset_class(details.under_security_type.as_str()),
        base_currency,
        Currency::from(details.contract.currency.to_string()),
        price_precision,
        size_precision,
        Price::new(details.min_tick, price_precision),
        Quantity::new(details.size_increment, size_precision),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(ib_contract_info(details)),
        timestamp,
        timestamp,
    );

    InstrumentAny::from(instrument)
}

/// Parse commodity contract (CMDTY).
fn parse_commodity_contract(
    details: &ibapi::contracts::ContractDetails,
    instrument_id: InstrumentId,
) -> InstrumentAny {
    let price_precision = tick_size_to_precision(details.min_tick);
    let size_precision = tick_size_to_precision(details.min_size);
    let timestamp = get_atomic_clock_realtime().get_time_ns();

    let instrument = Commodity::new(
        instrument_id,
        Symbol::from(details.contract.local_symbol.as_str()),
        AssetClass::Commodity,
        Currency::from(details.contract.currency.to_string()),
        price_precision,
        size_precision,
        Price::new(details.min_tick, price_precision),
        Quantity::new(details.size_increment, size_precision),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(ib_contract_info(details)),
        timestamp,
        timestamp,
    );

    InstrumentAny::from(instrument)
}

/// Parse bond contract (BOND).
fn parse_bond_contract(
    details: &ibapi::contracts::ContractDetails,
    instrument_id: InstrumentId,
) -> InstrumentAny {
    // Use Equity as a placeholder until Bond type is available in Rust model
    // Note: This is a limitation of the current Nautilus Rust model, not the IB adapter
    let price_precision = tick_size_to_precision(details.min_tick);
    let timestamp = get_atomic_clock_realtime().get_time_ns();

    let instrument = Equity::new(
        instrument_id,
        Symbol::from(details.contract.local_symbol.as_str()),
        None, // isin - could extract from security_id if available
        Currency::from(details.contract.currency.to_string()),
        price_precision,
        Price::new(details.min_tick, price_precision),
        Some(Quantity::new(1.0, 0)),     // Standard lot size for bonds
        None,                            // max_quantity
        None,                            // min_quantity
        None,                            // max_price
        None,                            // min_price
        None,                            // margin_init
        None,                            // margin_maint
        None,                            // maker_fee
        None,                            // taker_fee
        None,                            // tick_scheme
        Some(ib_contract_info(details)), // info
        timestamp,
        timestamp,
    );

    InstrumentAny::from(instrument)
}
