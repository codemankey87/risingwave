// Copyright 2024 RisingWave Labs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::Context;
use itertools::Itertools;
use pgwire::pg_protocol::ParameterStatus;
use pgwire::pg_response::{PgResponse, StatementType};
use pgwire::types::Row;
use risingwave_common::error::Result;
use risingwave_common::session_config::{ConfigReporter, SESSION_CONFIG_LIST_SEP};
use risingwave_common::system_param::is_mutable;
use risingwave_common::types::{DataType, ScalarRefImpl};
use risingwave_sqlparser::ast::{Ident, SetTimeZoneValue, SetVariableValue, Value};
use risingwave_sqlparser::keywords::Keyword;

use super::RwPgResponse;
use crate::handler::HandlerArgs;
use crate::utils::infer_stmt_row_desc::infer_show_variable;

/// convert `SetVariableValue` to string while remove the quotes on literals.
pub(crate) fn set_var_to_param_str(value: &SetVariableValue) -> Option<String> {
    match value {
        SetVariableValue::Single(var) => Some(var.to_string_unquoted()),
        SetVariableValue::List(list) => Some(
            list.iter()
                .map(|var| var.to_string_unquoted())
                .join(SESSION_CONFIG_LIST_SEP),
        ),
        SetVariableValue::Default => None,
    }
}

pub fn handle_set(
    handler_args: HandlerArgs,
    name: Ident,
    value: SetVariableValue,
) -> Result<RwPgResponse> {
    // Strip double and single quotes
    let mut string_val = set_var_to_param_str(&value);

    let mut status = ParameterStatus::default();

    struct Reporter<'a> {
        status: &'a mut ParameterStatus,
    }

    impl<'a> ConfigReporter for Reporter<'a> {
        fn report_status(&mut self, key: &str, new_val: String) {
            if key == "APPLICATION_NAME" {
                self.status.application_name = Some(new_val);
            }
        }
    }

    // special handle for streaming parallelism,
    if name
        .real_value()
        .eq_ignore_ascii_case("streaming_parallelism")
        && string_val
            .as_ref()
            .map(|val| val.eq_ignore_ascii_case(Keyword::ADAPTIVE.to_string().as_str()))
            .unwrap_or(false)
    {
        string_val = None;
    }

    // Currently store the config variable simply as String -> ConfigEntry(String).
    // In future we can add converter/parser to make the API more robust.
    // We remark that the name of session parameter is always case-insensitive.
    handler_args.session.set_config_report(
        &name.real_value().to_lowercase(),
        string_val,
        Reporter {
            status: &mut status,
        },
    )?;

    Ok(PgResponse::builder(StatementType::SET_VARIABLE)
        .status(status)
        .into())
}

pub(super) fn handle_set_time_zone(
    handler_args: HandlerArgs,
    value: SetTimeZoneValue,
) -> Result<RwPgResponse> {
    let tz_info = match value {
        SetTimeZoneValue::Local => {
            iana_time_zone::get_timezone().context("Failed to get local time zone")
        }
        SetTimeZoneValue::Default => Ok("UTC".to_string()),
        SetTimeZoneValue::Ident(ident) => Ok(ident.real_value()),
        SetTimeZoneValue::Literal(Value::DoubleQuotedString(s))
        | SetTimeZoneValue::Literal(Value::SingleQuotedString(s)) => Ok(s),
        _ => Ok(value.to_string()),
    }?;

    handler_args.session.set_config("timezone", tz_info)?;

    Ok(PgResponse::empty_result(StatementType::SET_VARIABLE))
}

pub(super) async fn handle_show(
    handler_args: HandlerArgs,
    variable: Vec<Ident>,
) -> Result<RwPgResponse> {
    // TODO: Verify that the name used in `show` command is indeed always case-insensitive.
    let name = variable.iter().map(|e| e.real_value()).join(" ");
    let row_desc = infer_show_variable(&name);
    let rows = if name.eq_ignore_ascii_case("PARAMETERS") {
        handle_show_system_params(handler_args).await?
    } else if name.eq_ignore_ascii_case("ALL") {
        handle_show_all(handler_args.clone())?
    } else {
        let config_reader = handler_args.session.config();
        vec![Row::new(vec![Some(config_reader.get(&name)?.into())])]
    };

    Ok(PgResponse::builder(StatementType::SHOW_VARIABLE)
        .values(rows.into(), row_desc)
        .into())
}

fn handle_show_all(handler_args: HandlerArgs) -> Result<Vec<Row>> {
    let config_reader = handler_args.session.config();

    let all_variables = config_reader.show_all();

    let rows = all_variables
        .iter()
        .map(|info| {
            Row::new(vec![
                Some(info.name.clone().into()),
                Some(info.setting.clone().into()),
                Some(info.description.clone().into()),
            ])
        })
        .collect_vec();
    Ok(rows)
}

async fn handle_show_system_params(handler_args: HandlerArgs) -> Result<Vec<Row>> {
    let params = handler_args
        .session
        .env()
        .meta_client()
        .get_system_params()
        .await?;
    let rows = params
        .to_kv()
        .into_iter()
        .map(|(k, v)| {
            let is_mutable_bytes = ScalarRefImpl::Bool(is_mutable(&k).unwrap())
                .text_format(&DataType::Boolean)
                .into();
            Row::new(vec![Some(k.into()), Some(v.into()), Some(is_mutable_bytes)])
        })
        .collect_vec();
    Ok(rows)
}
