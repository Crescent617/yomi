//! `yomi rpc` — send a raw wire-protocol request to the daemon.
//!
//! Debug/tooling escape hatch: takes a `snake_case` method name (any
//! `ReqMethod` variant) plus an optional JSON object of parameters, and
//! prints the daemon's `result` as JSON. Streaming methods (`subscribe`,
//! `subscribe_all`) only return an ack here — use `yomi events` to
//! follow the event stream.
//!
//! Discovery (no daemon needed): `yomi rpc --help` lists all methods,
//! `yomi rpc <method> --help` shows one method's parameter schema — both
//! derived from the `JsonSchema` of `ReqMethod`, so they can never drift
//! from the wire definition.

use anyhow::{Context, Result};
use kernel::wire::ReqMethod;

/// Build a `ReqMethod` from a method name and optional JSON params.
///
/// Two accepted forms:
/// - bare name + params object: `get_session` + `{"session_id": "…"}`
/// - full externally-tagged method JSON as `method`:
///   `{"get_session": {"session_id": "…"}}` (params must be absent)
fn build_method(method: &str, params: Option<&str>) -> Result<ReqMethod> {
    if method.trim_start().starts_with('{') {
        anyhow::ensure!(
            params.is_none(),
            "PARAMS not allowed when METHOD is a full JSON object"
        );
        let value: serde_json::Value =
            serde_json::from_str(method).context("Invalid method JSON")?;
        return serde_json::from_value(value)
            .map_err(|e| anyhow::anyhow!("Invalid wire method: {e}"));
    }

    let value = match params {
        None => serde_json::Value::String(method.to_string()),
        Some(raw) => {
            let params: serde_json::Value =
                serde_json::from_str(raw).context("Invalid PARAMS JSON")?;
            anyhow::ensure!(
                params.is_object(),
                "PARAMS must be a JSON object, e.g. '{{\"session_id\": \"sess_…\"}}'"
            );
            serde_json::json!({ method: params })
        }
    };
    serde_json::from_value(value).map_err(|e| {
        anyhow::anyhow!(
            "Unknown or malformed wire method `{method}`: {e}\n\
             Run `yomi rpc --help` to list methods, `yomi rpc <method> --help` for parameters."
        )
    })
}

/// The full `JsonSchema` of `ReqMethod` as a JSON value.
fn req_method_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(ReqMethod)).expect("ReqMethod schema must serialize")
}

/// All wire methods as `(name, params-schema)` pairs, extracted from the
/// `JsonSchema` of `ReqMethod`. Unit variants get a `Null` params schema.
fn method_entries(schema: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    let mut entries = Vec::new();
    let branches = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(serde_json::Value::as_array);
    for branch in branches.into_iter().flatten() {
        // Unit variant: {"type": "string", "const": "hello"} — or merged:
        // {"type": "string", "enum": ["hello", ...]} (schemars version-dependent).
        if let Some(name) = branch.get("const").and_then(serde_json::Value::as_str) {
            entries.push((name.to_string(), serde_json::Value::Null));
        } else if let Some(names) = branch.get("enum").and_then(serde_json::Value::as_array) {
            entries.extend(
                names
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(|n| (n.to_string(), serde_json::Value::Null)),
            );
        } else if let Some(props) = branch
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            // Variant with fields: {"properties": {"get_session": {...params...}}}
            entries.extend(props.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }
    entries
}

/// Recursively inline local `#/$defs/…` refs so help output is self-contained.
fn resolve_refs(value: &mut serde_json::Value, defs: &serde_json::Value) {
    if let Some(name) = value
        .get("$ref")
        .and_then(serde_json::Value::as_str)
        .and_then(|r| r.strip_prefix("#/$defs/"))
    {
        if let Some(def) = defs.get(name) {
            *value = def.clone();
        }
    }
    match value {
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                resolve_refs(v, defs);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                resolve_refs(v, defs);
            }
        }
        _ => {}
    }
}

/// Params schema for one method with local `$ref`s inlined.
/// `None` for unknown methods, `Some(Null)` for unit variants.
fn resolved_method_schema(name: &str) -> Option<serde_json::Value> {
    let schema = req_method_schema();
    let (_, mut fragment) = method_entries(&schema)
        .into_iter()
        .find(|(n, _)| n == name)?;
    if let Some(defs) = schema.get("$defs") {
        resolve_refs(&mut fragment, defs);
    }
    Some(fragment)
}

/// `yomi rpc --help` — clap usage followed by every wire method.
fn print_full_help() -> Result<()> {
    use clap::CommandFactory;
    crate::RpcArgs::command().name("yomi rpc").print_help()?;
    println!("\nWire methods (`yomi rpc <method> --help` shows parameters):");
    for (name, _) in method_entries(&req_method_schema()) {
        println!("  {name}");
    }
    Ok(())
}

/// `yomi rpc <method> --help` — show one method's parameter schema.
fn print_method_help(name: &str) -> Result<()> {
    let schema = resolved_method_schema(name).ok_or_else(|| {
        anyhow::anyhow!("Unknown method `{name}` — run `yomi rpc --help` to list methods")
    })?;
    if schema.is_null() {
        println!("{name}: no parameters");
    } else {
        println!("{}", serde_json::to_string_pretty(&schema)?);
    }
    Ok(())
}

pub async fn run(args: crate::RpcArgs) -> Result<()> {
    // Help short-circuits before anything touches stdin or the daemon.
    match (&args.method, args.help) {
        (Some(method), true) => return print_method_help(method),
        (None, _) => return print_full_help(),
        (Some(_), false) => {}
    }
    let method = args.method.as_deref().expect("method checked above");

    // Params fall back to piped stdin: `echo '{"job_id": "…"}' | yomi rpc get_cron_job`
    let params = match args.params {
        Some(p) => Some(p),
        None => crate::utils::read_piped_stdin().await,
    };
    let method = build_method(method, params.as_deref())?;

    if matches!(
        method,
        ReqMethod::Subscribe { .. } | ReqMethod::SubscribeAll
    ) {
        eprintln!("note: subscribe only returns an ack here; use `yomi events` to stream events");
    }

    let kernel = crate::daemon::connect_strict().await?;
    let result = kernel.call(method).await?;

    let out = if args.compact {
        serde_json::to_string(&result)?
    } else {
        serde_json::to_string_pretty(&result)?
    };
    println!("{out}");
    Ok(())
}

#[cfg(test)]
#[path = "rpc_test.rs"]
mod tests;
