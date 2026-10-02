//! Bevy Remote Protocol call pass-through.
//!
//! Both tools delegate transport to `crate::brp_post`, which owns the
//! security policy: loopback-only endpoints unless
//! `LUMEN_ALLOW_REMOTE_BRP=1`, http only, 5 s timeout, no retries. Nothing
//! here re-implements or relaxes any of that. Neither tool touches the
//! filesystem, so neither takes a project root.

#![forbid(unsafe_code)]

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::{BrpCallResult, BrpDiscovery, DEFAULT_BRP_ENDPOINT, LumenError};

/// Longest accepted BRP method name, in characters.
pub const BRP_METHOD_MAX_CHARS: usize = 128;
/// Largest accepted serialized `params` payload, in bytes.
pub const BRP_PARAMS_MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BevyCallRequest {
    /// BRP endpoint URL. Defaults to http://127.0.0.1:15702.
    /// Non-loopback hosts are rejected unless LUMEN_ALLOW_REMOTE_BRP=1.
    pub endpoint: Option<String>,
    /// JSON-RPC method, e.g. "bevy/list" or "world.query". 1..=128 chars.
    pub method: String,
    /// JSON-RPC params. Omitted means JSON `null`.
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BevyStatusRequest {
    /// BRP endpoint URL. Defaults to http://127.0.0.1:15702.
    /// Non-loopback hosts are rejected unless LUMEN_ALLOW_REMOTE_BRP=1.
    pub endpoint: Option<String>,
}

/// Send one JSON-RPC call to a BRP endpoint and return the classified reply.
///
/// A JSON-RPC `error` object in the reply is a RESULT (`ok: false`, `error`
/// populated), never an `Err`: the server answered, the method said no.
/// `Err` is reserved for: bad parameters (`BadParam`: method empty, blank,
/// or over 128 chars; params over 1 MiB serialized), endpoint policy or
/// transport failures (`Brp`), and refused/timed-out connections
/// (`BrpUnreachable`).
pub async fn bevy_call(req: BevyCallRequest) -> Result<BrpCallResult, LumenError> {
    if req.method.trim().is_empty() {
        return Err(LumenError::BadParam("method is empty".to_string()));
    }
    if req.method.chars().count() > BRP_METHOD_MAX_CHARS {
        return Err(LumenError::BadParam(format!(
            "method exceeds {BRP_METHOD_MAX_CHARS} characters"
        )));
    }
    let params = req.params.unwrap_or(serde_json::Value::Null);
    let params_bytes = serde_json::to_vec(&params)
        .map_err(|e| LumenError::BadParam(format!("params not serializable: {e}")))?
        .len();
    if params_bytes > BRP_PARAMS_MAX_BYTES {
        return Err(LumenError::BadParam(format!(
            "params are {params_bytes} bytes; limit is {BRP_PARAMS_MAX_BYTES}"
        )));
    }
    let endpoint = req.endpoint.as_deref().unwrap_or(DEFAULT_BRP_ENDPOINT);
    crate::brp_post(endpoint, &req.method, params).await
}

/// Report whether a BRP endpoint is reachable and speaks BRP.
///
/// Probes `rpc.discover`, which Bevy 0.18 serves as an OpenRPC document
/// (`bevy/list` no longer exists there). Classification:
/// - any JSON-RPC `result` -> `speaks_brp: true`; the detail reports the
///   method count when the result is an OpenRPC document;
/// - a JSON-RPC `error` -> `speaks_brp: false`: every JSON-RPC server answers
///   an unknown method with an error, so an error proves nothing about Bevy;
/// - non-JSON-RPC body -> `speaks_brp: false`;
/// - refused/timed-out connection -> `reachable: false` (a report, not `Err`).
///
/// `Err` only for endpoint-policy and other transport failures. The detail
/// never echoes server text, so its size is bounded.
pub async fn bevy_status(req: BevyStatusRequest) -> Result<BrpDiscovery, LumenError> {
    let endpoint = req.endpoint.as_deref().unwrap_or(DEFAULT_BRP_ENDPOINT);
    let call = match crate::brp_post(endpoint, BRP_STATUS_PROBE_METHOD, Value::Null).await {
        Ok(call) => call,
        Err(LumenError::BrpUnreachable(detail)) => {
            return Ok(status_report(endpoint, false, false, detail));
        }
        Err(e) => return Err(e),
    };
    if let Some(result) = &call.result {
        let detail = match openrpc_method_count(result) {
            Some(count) => {
                format!("BRP confirmed: {BRP_STATUS_PROBE_METHOD} listed {count} methods")
            }
            None => format!(
                "BRP assumed: {BRP_STATUS_PROBE_METHOD} returned a result that is not \
                 an OpenRPC document"
            ),
        };
        return Ok(status_report(endpoint, true, true, detail));
    }
    // JSON-RPC error codes are integers; anything else is not JSON-RPC.
    let code = call.error.as_ref().and_then(|e| e.get("code")?.as_i64());
    let detail = match code {
        Some(code) => format!(
            "JSON-RPC server answered {BRP_STATUS_PROBE_METHOD} with error code {code}: \
             not a Bevy 0.18 BRP endpoint"
        ),
        None => format!(
            "http {} with a non-JSON-RPC body: not a BRP endpoint",
            call.http_status
        ),
    };
    Ok(status_report(endpoint, true, false, detail))
}

/// Method `bevy_status` probes. Bevy 0.18 registers it in `bevy_remote`.
const BRP_STATUS_PROBE_METHOD: &str = "rpc.discover";

/// Number of methods in an OpenRPC document, or `None` if `result` is not
/// shaped like one (`openrpc` string plus `methods` array).
fn openrpc_method_count(result: &Value) -> Option<usize> {
    result.get("openrpc")?.as_str()?;
    Some(result.get("methods")?.as_array()?.len())
}

fn status_report(
    endpoint: &str,
    reachable: bool,
    speaks_brp: bool,
    detail: String,
) -> BrpDiscovery {
    BrpDiscovery {
        endpoint: endpoint.to_string(),
        reachable,
        speaks_brp,
        detail,
    }
}
