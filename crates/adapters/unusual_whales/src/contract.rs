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

//! Generated-contract validation and exact request serialization.

use std::{collections::HashSet, str::FromStr};

use anyhow::Context;
use nautilus_core::{Params, string::urlencoding};
use regex::Regex;
use rust_decimal::Decimal;
use serde_json::{Map, Value};

use crate::generated::{
    CHANNELS, ChannelSpec, OperationClassification, OperationSpec, ParameterSpec, find_operation,
};

/// A generated read operation after all local input validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedRestRequest {
    pub operation: &'static OperationSpec,
    pub relative_url: String,
}

/// A documented WebSocket channel after local validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedChannel {
    pub spec: &'static ChannelSpec,
    pub channel: String,
}

/// Validates and serializes one generated REST operation.
///
/// # Errors
///
/// Returns an error for unknown or mutating operations, missing parameters, extra parameters,
/// invalid values, or unsupported serialization shapes. No task or network work starts on error.
pub fn validate_rest_request(
    operation_id: &str,
    params: Option<&Params>,
) -> anyhow::Result<ValidatedRestRequest> {
    let operation = find_operation(operation_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown Unusual Whales operation ID: {operation_id}"))?;
    anyhow::ensure!(
        operation.classification == OperationClassification::Read,
        "Unusual Whales operation {operation_id} is an account mutation and is not callable"
    );
    anyhow::ensure!(
        operation.method == "GET",
        "Unusual Whales read operation {operation_id} has unsupported method {}",
        operation.method
    );

    let empty = Params::new();
    let params = params.unwrap_or(&empty);
    let expected: HashSet<&str> = operation
        .parameters
        .iter()
        .map(|parameter| parameter.name)
        .collect();
    let unexpected: Vec<&str> = params
        .keys()
        .map(String::as_str)
        .filter(|name| !expected.contains(name))
        .collect();
    anyhow::ensure!(
        unexpected.is_empty(),
        "Unexpected parameters for {operation_id}: {}",
        unexpected.join(", ")
    );

    let mut path = operation.path.to_string();
    let mut query = Vec::<(String, String)>::new();

    for parameter in operation.parameters {
        let schema: Value = serde_json::from_str(parameter.resolved_schema_json)
            .with_context(|| format!("Invalid generated schema for {}", parameter.name))?;
        let value = params
            .get(parameter.name)
            .cloned()
            .or_else(|| schema.get("default").cloned());

        let Some(value) = value else {
            anyhow::ensure!(
                !parameter.required,
                "Missing required parameter '{}' for {operation_id}",
                parameter.name
            );
            continue;
        };

        if value.is_null() && !parameter.required {
            continue;
        }
        validate_value(parameter.name, &value, &schema)?;

        match parameter.location {
            "path" => {
                anyhow::ensure!(
                    parameter.style == "simple" && !parameter.explode,
                    "Unsupported path serialization for parameter '{}'",
                    parameter.name
                );
                let wire = scalar_wire_value(parameter.name, &value)?;
                let placeholder = format!("{{{}}}", parameter.name);
                anyhow::ensure!(
                    path.contains(&placeholder),
                    "Generated path is missing placeholder {placeholder}"
                );
                path = path.replace(&placeholder, &urlencoding::encode(&wire));
            }
            "query" => serialize_query_parameter(parameter, &value, &mut query)?,
            other => anyhow::bail!(
                "Unsupported generated parameter location '{other}' for '{}'",
                parameter.name
            ),
        }
    }

    if !query.is_empty() {
        path.push('?');

        for (index, (name, value)) in query.into_iter().enumerate() {
            if index > 0 {
                path.push('&');
            }
            path.push_str(&urlencoding::encode(&name));
            path.push('=');
            path.push_str(&value);
        }
    }

    Ok(ValidatedRestRequest {
        operation,
        relative_url: path,
    })
}

fn serialize_query_parameter(
    parameter: &ParameterSpec,
    value: &Value,
    output: &mut Vec<(String, String)>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        parameter.style == "form",
        "Unsupported query style '{}' for '{}'",
        parameter.style,
        parameter.name
    );
    anyhow::ensure!(
        !parameter.allow_reserved,
        "allowReserved query serialization is not supported for '{}'",
        parameter.name
    );

    if let Some(values) = value.as_array() {
        if parameter.explode {
            for item in values {
                output.push((
                    parameter.name.to_string(),
                    urlencoding::encode(&scalar_wire_value(parameter.name, item)?).into_owned(),
                ));
            }
        } else {
            let encoded = values
                .iter()
                .map(|item| {
                    scalar_wire_value(parameter.name, item)
                        .map(|wire| urlencoding::encode(&wire).into_owned())
                })
                .collect::<anyhow::Result<Vec<_>>>()?
                .join(",");
            output.push((parameter.name.to_string(), encoded));
        }
        return Ok(());
    }

    output.push((
        parameter.name.to_string(),
        urlencoding::encode(&scalar_wire_value(parameter.name, value)?).into_owned(),
    ));
    Ok(())
}

fn scalar_wire_value(name: &str, value: &Value) -> anyhow::Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        _ => anyhow::bail!("Parameter '{name}' must be a scalar value"),
    }
}

fn validate_value(name: &str, value: &Value, schema: &Value) -> anyhow::Result<()> {
    let schema = schema
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Generated schema for '{name}' is not an object"))?;
    if value.is_null() && schema.get("nullable").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }

    if let Some(options) = schema.get("allOf").and_then(Value::as_array) {
        for option in options {
            validate_value(name, value, option)?;
        }
    }

    if let Some(options) = schema.get("oneOf").and_then(Value::as_array) {
        let matches = options
            .iter()
            .filter(|option| validate_value(name, value, option).is_ok())
            .count();
        anyhow::ensure!(
            matches == 1,
            "Parameter '{name}' must match exactly one schema"
        );
    }

    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        anyhow::ensure!(
            options
                .iter()
                .any(|option| validate_value(name, value, option).is_ok()),
            "Parameter '{name}' does not match any allowed schema"
        );
    }

    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        anyhow::ensure!(
            values.contains(value),
            "Parameter '{name}' is not an allowed enum value"
        );
    }

    match schema.get("type").and_then(Value::as_str) {
        None => {}
        Some("string") => validate_string(name, value, schema)?,
        Some("date") => validate_date(name, value)?,
        Some("integer") => {
            anyhow::ensure!(
                value.as_i64().is_some() || value.as_u64().is_some(),
                "Parameter '{name}' must be an integer"
            );
            validate_number_bounds(name, value, schema)?;
        }
        Some("number") => {
            anyhow::ensure!(value.is_number(), "Parameter '{name}' must be a number");
            validate_number_bounds(name, value, schema)?;
        }
        Some("boolean") => {
            anyhow::ensure!(value.is_boolean(), "Parameter '{name}' must be a boolean");
        }
        Some("array") => validate_array(name, value, schema)?,
        Some("null") => anyhow::ensure!(value.is_null(), "Parameter '{name}' must be null"),
        Some(other) => {
            anyhow::bail!("Unclassified generated parameter type '{other}' for '{name}'")
        }
    }
    Ok(())
}

fn validate_string(name: &str, value: &Value, schema: &Map<String, Value>) -> anyhow::Result<()> {
    let value = value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Parameter '{name}' must be a string"))?;
    validate_length(name, value.chars().count(), schema, "Length")?;
    if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
        let regex = Regex::new(pattern)
            .with_context(|| format!("Generated pattern for '{name}' is invalid"))?;
        anyhow::ensure!(
            regex.is_match(value),
            "Parameter '{name}' does not match its pattern"
        );
    }
    Ok(())
}

fn validate_date(name: &str, value: &Value) -> anyhow::Result<()> {
    let value = value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Parameter '{name}' must be a YYYY-MM-DD string"))?;
    time::Date::parse(
        value,
        &time::macros::format_description!("[year]-[month]-[day]"),
    )
    .with_context(|| format!("Parameter '{name}' must be a valid YYYY-MM-DD date"))?;
    Ok(())
}

fn validate_array(name: &str, value: &Value, schema: &Map<String, Value>) -> anyhow::Result<()> {
    let values = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Parameter '{name}' must be an array"))?;
    validate_length(name, values.len(), schema, "Items")?;
    if schema.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
        let unique: HashSet<String> = values.iter().map(Value::to_string).collect();
        anyhow::ensure!(
            unique.len() == values.len(),
            "Parameter '{name}' must contain unique items"
        );
    }

    if let Some(items) = schema.get("items") {
        for item in values {
            validate_value(name, item, items)?;
        }
    }
    Ok(())
}

fn validate_length(
    name: &str,
    length: usize,
    schema: &Map<String, Value>,
    suffix: &str,
) -> anyhow::Result<()> {
    let minimum = schema.get(&format!("min{suffix}")).and_then(Value::as_u64);
    let maximum = schema.get(&format!("max{suffix}")).and_then(Value::as_u64);

    if let Some(minimum) = minimum {
        anyhow::ensure!(
            length as u64 >= minimum,
            "Parameter '{name}' is shorter than {minimum}"
        );
    }

    if let Some(maximum) = maximum {
        anyhow::ensure!(
            length as u64 <= maximum,
            "Parameter '{name}' is longer than {maximum}"
        );
    }
    Ok(())
}

fn validate_number_bounds(
    name: &str,
    value: &Value,
    schema: &Map<String, Value>,
) -> anyhow::Result<()> {
    let number = Decimal::from_str(&value.to_string())
        .with_context(|| format!("Parameter '{name}' is not a finite decimal"))?;

    if let Some(minimum) = schema.get("minimum") {
        let minimum = Decimal::from_str(&minimum.to_string())?;
        let exclusive = schema
            .get("exclusiveMinimum")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        anyhow::ensure!(
            if exclusive {
                number > minimum
            } else {
                number >= minimum
            },
            "Parameter '{name}' is below its minimum"
        );
    }

    if let Some(maximum) = schema.get("maximum") {
        let maximum = Decimal::from_str(&maximum.to_string())?;
        let exclusive = schema
            .get("exclusiveMaximum")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        anyhow::ensure!(
            if exclusive {
                number < maximum
            } else {
                number <= maximum
            },
            "Parameter '{name}' is above its maximum"
        );
    }
    Ok(())
}

/// Validates an exact channel name against the generated channel catalog.
///
/// # Errors
///
/// Returns an error when the channel form or ticker component is invalid.
pub fn validate_channel(channel: &str) -> anyhow::Result<ValidatedChannel> {
    anyhow::ensure!(
        channel == channel.trim(),
        "WebSocket channel has surrounding whitespace"
    );

    if let Some(spec) = CHANNELS
        .iter()
        .find(|spec| !spec.requires_ticker && spec.form == channel)
    {
        return Ok(ValidatedChannel {
            spec,
            channel: channel.to_string(),
        });
    }

    for spec in CHANNELS.iter().filter(|spec| spec.requires_ticker) {
        let Some(ticker) = channel
            .strip_prefix(spec.prefix)
            .and_then(|value| value.strip_prefix(':'))
        else {
            continue;
        };
        anyhow::ensure!(
            !ticker.is_empty() && ticker.len() <= 32,
            "WebSocket ticker must contain 1 to 32 characters"
        );
        anyhow::ensure!(
            ticker.bytes().all(|byte| {
                byte.is_ascii_uppercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
            }),
            "WebSocket ticker must use uppercase ASCII letters, digits, '.' or '-'"
        );
        return Ok(ValidatedChannel {
            spec,
            channel: channel.to_string(),
        });
    }

    anyhow::bail!("Unknown Unusual Whales WebSocket channel: {channel}")
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use nautilus_core::Params;
    use rstest::rstest;
    use serde_json::json;

    use super::*;
    use crate::generated::{
        CHANNEL_FORM_COUNT, GET_OPERATION_COUNT, OPERATION_COUNT, OPERATIONS, PATH_COUNT,
        POST_OPERATION_COUNT, SOURCE_SHA256, SOURCE_URL,
    };

    #[rstest]
    fn generated_catalog_has_full_source_coverage() {
        assert_eq!(PATH_COUNT, 214);
        assert_eq!(OPERATION_COUNT, 215);
        assert_eq!(GET_OPERATION_COUNT, 214);
        assert_eq!(POST_OPERATION_COUNT, 1);
        assert_eq!(CHANNEL_FORM_COUNT, 28);
        assert_eq!(OPERATIONS.len(), OPERATION_COUNT);
        assert_eq!(CHANNELS.len(), CHANNEL_FORM_COUNT);
        assert_eq!(SOURCE_URL, "https://api.unusualwhales.com/api/openapi");
        assert_eq!(
            SOURCE_SHA256,
            "38ca3116dcbcc941c002154c02dfde4c96e9c79a4d2462e73c768fb6a2f7d43f"
        );
    }

    #[rstest]
    fn generated_operations_are_sorted_and_unique() {
        let ids: Vec<_> = OPERATIONS
            .iter()
            .map(|operation| operation.operation_id)
            .collect();
        let mut expected = ids.clone();
        expected.sort_unstable();
        expected.dedup();
        assert_eq!(ids, expected);
    }

    #[rstest]
    fn account_mutation_is_rejected_before_serialization() {
        let error =
            validate_rest_request("PublicApi.AlertsController.create_config", None).unwrap_err();
        assert!(error.to_string().contains("account mutation"));
    }

    #[rstest]
    fn every_documented_channel_form_validates() {
        for spec in CHANNELS {
            let channel = if spec.requires_ticker {
                spec.form.replace("TICKER", "AAPL")
            } else {
                spec.form.to_string()
            };
            let validated = validate_channel(&channel).unwrap();
            assert_eq!(validated.spec.form, spec.form);
            assert_eq!(validated.channel, channel);
        }
    }

    #[rstest]
    fn invalid_channel_fails_synchronously() {
        assert!(validate_channel("price:aapl").is_err());
        assert!(validate_channel("unknown").is_err());
        assert!(validate_channel("price:AAPL\nother").is_err());
    }

    #[rstest]
    fn request_serialization_preserves_path_and_query_values() {
        let mut params = Params::new();
        params.insert("ticker".to_string(), json!("BRK B"));
        params.insert("limit".to_string(), json!(25));
        let request = validate_rest_request(
            "PublicApi.DarkpoolController.darkpool_ticker",
            Some(&params),
        )
        .unwrap();

        assert!(request.relative_url.starts_with("/api/darkpool/BRK%20B?"));
        assert!(request.relative_url.contains("limit=25"));
    }

    #[rstest]
    fn array_serialization_repeats_form_keys_and_applies_defaults() {
        let mut params = Params::new();
        params.insert("config_ids[]".to_string(), json!(["first-id", "second-id"]));
        let request =
            validate_rest_request("PublicApi.AlertsController.alerts", Some(&params)).unwrap();

        assert!(request.relative_url.contains("config_ids%5B%5D=first-id"));
        assert!(request.relative_url.contains("config_ids%5B%5D=second-id"));
        assert!(request.relative_url.contains("limit=50"));
    }

    #[rstest]
    fn enums_bounds_and_unknown_parameters_fail_before_transport() {
        let mut enum_params = Params::new();
        enum_params.insert("target".to_string(), json!("unknown"));
        assert!(
            validate_rest_request("PublicApi.AlertsController.dsl_grammar", Some(&enum_params))
                .is_err()
        );

        let mut bound_params = Params::new();
        bound_params.insert("limit".to_string(), json!(501));
        assert!(
            validate_rest_request("PublicApi.AlertsController.alerts", Some(&bound_params))
                .is_err()
        );

        let mut unknown_params = Params::new();
        unknown_params.insert("not_in_contract".to_string(), json!(true));
        assert!(
            validate_rest_request("PublicApi.AlertsController.alerts", Some(&unknown_params))
                .is_err()
        );
        assert!(validate_rest_request("not-an-operation", None).is_err());
    }

    #[rstest]
    fn every_generated_json_fragment_is_valid() {
        for operation in OPERATIONS {
            serde_json::from_str::<Value>(operation.responses_json).unwrap();
            serde_json::from_str::<Value>(operation.security_json).unwrap();

            if let Some(body) = operation.request_body_json {
                serde_json::from_str::<Value>(body).unwrap();
            }

            for parameter in operation.parameters {
                serde_json::from_str::<Value>(parameter.schema_json).unwrap();
                serde_json::from_str::<Value>(parameter.resolved_schema_json).unwrap();
            }
        }
    }

    #[rstest]
    fn every_generated_read_operation_accepts_a_schema_valid_request() {
        for operation in OPERATIONS
            .iter()
            .filter(|operation| operation.classification == OperationClassification::Read)
        {
            let mut params = Params::new();

            for parameter in operation
                .parameters
                .iter()
                .filter(|parameter| parameter.required)
            {
                let schema: Value = serde_json::from_str(parameter.resolved_schema_json).unwrap();
                params.insert(parameter.name.to_string(), valid_example(&schema));
            }
            let request = validate_rest_request(operation.operation_id, Some(&params))
                .unwrap_or_else(|e| panic!("{} did not validate: {e}", operation.operation_id));
            assert!(!request.relative_url.contains('{'));
            assert!(!request.relative_url.contains('}'));
        }
    }

    fn valid_example(schema: &Value) -> Value {
        if let Some(value) = schema.get("default") {
            return value.clone();
        }

        if let Some(value) = schema.get("example") {
            return value.clone();
        }

        if let Some(value) = schema
            .get("enum")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
        {
            return value.clone();
        }

        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
            return match pattern {
                "^[0-9]{4}Q[1-4]$" => json!("2025Q1"),
                r"^\d{4}-(Q[1-4]|H[1-2]|MID|LATE|\d{2}-\d{2})$" => {
                    json!("2025-Q1")
                }
                _ => panic!("unclassified generated test pattern: {pattern}"),
            };
        }

        match schema.get("type").and_then(Value::as_str) {
            Some("date") => json!("2025-01-02"),
            Some("integer") => schema.get("minimum").cloned().unwrap_or_else(|| json!(1)),
            Some("number") => schema.get("minimum").cloned().unwrap_or_else(|| json!(1)),
            Some("boolean") => json!(true),
            Some("array") => json!([]),
            _ => json!("AAPL"),
        }
    }

    #[rstest]
    fn committed_generation_is_deterministic() {
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let status = Command::new("ruby")
            .arg(crate_dir.join("scripts/generate_contract.rb"))
            .arg("--source")
            .arg(crate_dir.join("resources/openapi.yaml"))
            .arg("--check")
            .status()
            .expect("Ruby is required only for the development-time contract drift test");
        assert!(status.success());
    }
}
