use super::*;

/// `docs/config-schema.json` must be the verbatim output of
/// `yomi config schema`; regenerate with:
/// `cargo run -p cli -- config schema > docs/config-schema.json`
#[test]
fn docs_config_schema_in_sync() {
    let docs_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/config-schema.json");
    let on_disk = std::fs::read_to_string(docs_path).expect("docs/config-schema.json missing");
    let generated = format!("{}\n", schema_json_string());
    assert_eq!(
        on_disk, generated,
        "docs/config-schema.json is stale; regenerate with \
         `cargo run -p cli -- config schema > docs/config-schema.json`"
    );
}

#[test]
fn schema_is_deterministic_and_machine_independent() {
    let schema = schema_json_string();
    // Same output on every invocation.
    assert_eq!(schema, schema_json_string());
    // No machine-specific paths or inline default values leak in.
    assert!(!schema.contains("\"default\""));
    let parsed: serde_json::Value = serde_json::from_str(&schema).unwrap();
    let properties = parsed["properties"].as_object().unwrap();
    for key in ["agent", "models", "channels", "gc", "features"] {
        assert!(properties.contains_key(key), "missing top-level key {key}");
    }
}
