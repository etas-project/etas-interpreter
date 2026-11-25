use super::std_call::nominal_representation_ref;
use super::*;
use crate::control::ExecutionFault;

impl EvalContext<'_> {
    pub(super) fn source_tcp_options(
        &self,
        value: &InterpValue,
        span: Span,
    ) -> Result<etas_host::TcpConnectOptions, ExecutionFault> {
        let (fields, timeout) =
            self.source_network_config(value, &["std", "net", "tcp", "TcpOptions"], span)?;
        let nodelay = match fields.get_ref("nodelay") {
            Some(InterpValue::Bool(value)) => value.to_owned(),
            _ => {
                return Err(network_config_fault(
                    span,
                    "TcpOptions.nodelay must be bool",
                ));
            }
        };
        Ok(etas_host::TcpConnectOptions {
            timeout_ms: timeout,
            nodelay,
        })
    }

    pub(super) fn source_tls_options(
        &self,
        value: &InterpValue,
        span: Span,
    ) -> Result<etas_host::TlsConnectOptions, ExecutionFault> {
        let (fields, timeout_ms) =
            self.source_network_config(value, &["std", "tls", "TlsConfig"], span)?;
        let minimum = match fields.get_ref("minimum").map(nominal_representation_ref) {
            Some(InterpValue::Variant { name, fields }) if fields.is_empty() => match name.as_str()
            {
                "V12" => etas_host::TlsVersion::V12,
                "V13" => etas_host::TlsVersion::V13,
                _ => return Err(network_config_fault(span, "unknown checked TLS version")),
            },
            _ => {
                return Err(network_config_fault(
                    span,
                    "TlsConfig.minimum must be TlsVersion",
                ));
            }
        };
        let trust = match fields.get_ref("trust").map(nominal_representation_ref) {
            Some(InterpValue::Variant { name, fields })
                if name.as_str() == "Platform" && fields.is_empty() =>
            {
                etas_host::TlsTrust::Platform
            }
            Some(InterpValue::Variant { name, fields })
                if name.as_str() == "Bundle" && fields.len() == 1 =>
            {
                let InterpValue::Bytes(bytes) = &fields[0] else {
                    return Err(network_config_fault(
                        span,
                        "TlsTrust.Bundle must contain bytes",
                    ));
                };
                etas_host::TlsTrust::Bundle(bytes.to_vec())
            }
            _ => {
                return Err(network_config_fault(
                    span,
                    "TlsConfig.trust must be TlsTrust",
                ));
            }
        };
        let Some(InterpValue::Array(protocols)) = fields.get_ref("alpn") else {
            return Err(network_config_fault(
                span,
                "TlsConfig.alpn must contain strings",
            ));
        };
        let mut alpn = Vec::new();
        for protocol in protocols.snapshot().iter() {
            let InterpValue::String(protocol) = protocol else {
                return Err(network_config_fault(span, "ALPN identifier must be string"));
            };
            alpn.push(protocol.to_string());
        }
        Ok(etas_host::TlsConnectOptions {
            timeout_ms,
            minimum,
            trust,
            alpn,
        })
    }

    fn source_network_config(
        &self,
        value: &InterpValue,
        path: &[&str],
        span: Span,
    ) -> Result<(crate::value::RecordValue, Option<u128>), ExecutionFault> {
        let config_type = super::resolve_std_type(self.checked, path).ok_or_else(|| {
            network_config_fault(span, "network configuration has no checked type")
        })?;
        let record_type =
            etas_types::record_fields_with_applied_params(&self.checked.type_store, config_type)
                .map_err(|error| network_config_fault(span, &error.to_string()))?
                .ok_or_else(|| {
                    network_config_fault(span, "network configuration has no record layout")
                })?;
        let timeout_type = record_type
            .fields
            .iter()
            .find(|field| field.name == "timeout")
            .ok_or_else(|| {
                network_config_fault(span, "network configuration has no timeout field")
            })?
            .ty;
        let InterpValue::Record(fields) = nominal_representation_ref(value) else {
            return Err(network_config_fault(
                span,
                "network configuration must be a checked record",
            ));
        };
        let timeout = fields
            .get_ref("timeout")
            .ok_or_else(|| network_config_fault(span, "network configuration requires timeout"))?;
        let timeout_ms = match self.checked_option_payload(timeout, timeout_type) {
            Some(None) => None,
            Some(Some(value)) => {
                let InterpValue::Record(duration) = nominal_representation_ref(value) else {
                    return Err(network_config_fault(
                        span,
                        "network timeout must be Duration",
                    ));
                };
                Some(
                    duration
                        .get_ref("millis")
                        .and_then(InterpValue::as_number)
                        .and_then(|number| number.as_u128())
                        .ok_or_else(|| {
                            network_config_fault(span, "Duration.millis must be u128")
                        })?,
                )
            }
            None => {
                return Err(network_config_fault(
                    span,
                    "network timeout must be its checked Option",
                ));
            }
        };
        Ok((fields.clone(), timeout_ms))
    }
}

fn network_config_fault(span: Span, message: &str) -> ExecutionFault {
    ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
}
