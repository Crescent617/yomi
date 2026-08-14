use super::*;

#[test]
fn unit_method_without_params() {
    let m = build_method("get_config", None).unwrap();
    assert_eq!(m, ReqMethod::GetConfig);
}

#[test]
fn method_with_params() {
    let m = build_method("get_session", Some(r#"{"session_id": "sess_123"}"#)).unwrap();
    assert_eq!(
        m,
        ReqMethod::GetSession {
            session_id: "sess_123".to_string()
        }
    );
}

#[test]
fn full_method_json_form() {
    let m = build_method(r#"{"get_session": {"session_id": "sess_123"}}"#, None).unwrap();
    assert_eq!(
        m,
        ReqMethod::GetSession {
            session_id: "sess_123".to_string()
        }
    );
}

#[test]
fn full_method_json_rejects_extra_params() {
    let err = build_method(r#"{"get_config": null}"#, Some("{}")).unwrap_err();
    assert!(err.to_string().contains("PARAMS not allowed"));
}

#[test]
fn unknown_method_is_rejected() {
    let err = build_method("not_a_method", None).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Unknown or malformed wire method"));
    assert!(msg.contains("yomi rpc --help"));
}

#[test]
fn missing_required_params_is_rejected() {
    assert!(build_method("get_session", None).is_err());
}

#[test]
fn non_object_params_are_rejected() {
    let err = build_method("get_config", Some("[1, 2]")).unwrap_err();
    assert!(err.to_string().contains("must be a JSON object"));
}

#[test]
fn invalid_params_json_is_rejected() {
    let err = build_method("get_config", Some("{nope")).unwrap_err();
    assert!(err.to_string().contains("Invalid PARAMS JSON"));
}

#[test]
fn params_with_wrong_field_type_are_rejected() {
    // `limit` is usize; a string must fail.
    assert!(build_method("list_cron_jobs", Some(r#"{"limit": "five"}"#)).is_err());
}

#[test]
fn params_with_defaults_fill_in() {
    // `ListSessions.limit` is required, optional fields may be omitted.
    let m = build_method("list_cron_jobs", Some(r#"{"limit": 5}"#)).unwrap();
    assert_eq!(
        m,
        ReqMethod::ListCronJobs {
            status: None,
            limit: 5
        }
    );
}

// ── `yomi rpc help` (schema-driven discovery) ────────────────────────────

#[test]
fn help_lists_all_wire_methods() {
    let entries = method_entries(&req_method_schema());
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    for expected in [
        "hello",
        "get_config",
        "send_message",
        "subscribe",
        "trigger_cron_job",
    ] {
        assert!(names.contains(&expected), "missing method: {expected}");
    }
    // One entry per ReqMethod variant — guard against extraction silently
    // dropping a branch shape (schemars may merge unit variants).
    assert!(entries.len() > 50, "got {} entries", entries.len());
    // No duplicates.
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len());
}

#[test]
fn help_unit_method_has_null_params() {
    assert!(resolved_method_schema("hello").unwrap().is_null());
}

#[test]
fn help_param_method_shows_fields() {
    let schema = resolved_method_schema("get_session").unwrap();
    assert_eq!(schema["properties"]["session_id"]["type"], "string");
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("session_id")));
}

#[test]
fn help_optional_fields_are_not_required() {
    let schema = resolved_method_schema("list_cron_jobs").unwrap();
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("limit")));
    assert!(!required.contains(&serde_json::json!("status")));
}

#[test]
fn create_session_auto_approve_level_is_optional() {
    // schema 里不再必填（缺省时内核回落配置 auto_approve）；
    // create_session 的所有参数都可选，required 可能整个缺省
    let schema = resolved_method_schema("create_session").unwrap();
    let required = schema["required"].as_array();
    assert!(
        !required.is_some_and(|r| r.contains(&serde_json::json!("auto_approve_level"))),
        "auto_approve_level should not be required: {schema}"
    );

    // 缺省 JSON 能正常反序列化
    let m = build_method("create_session", Some(r#"{"working_dir":"/tmp"}"#)).unwrap();
    let ReqMethod::CreateSession {
        auto_approve_level, ..
    } = m
    else {
        panic!("expected create_session");
    };
    assert!(auto_approve_level.is_none());
}

#[test]
fn help_enum_params_show_variants() {
    // `$ref`s to shared types (Level) are inlined for self-contained output.
    // (Doc-commented enums render as oneOf/const branches, not a flat enum.)
    let schema = resolved_method_schema("fork_session").unwrap();
    let level = serde_json::to_string(&schema["properties"]["auto_approve_level"]).unwrap();
    for variant in ["safe", "caution", "dangerous"] {
        assert!(
            level.contains(&format!("\"{variant}\"")),
            "missing {variant}"
        );
    }
}

#[test]
fn help_unknown_method_is_none() {
    assert!(resolved_method_schema("not_a_method").is_none());
}

// ── clap parsing (`--help` convention) ───────────────────────────────────

#[test]
fn parse_help_alone() {
    use clap::Parser;
    let args = crate::RpcArgs::try_parse_from(["yomi-rpc", "--help"]).unwrap();
    assert!(args.method.is_none());
    assert!(args.help);
}

#[test]
fn parse_method_with_help() {
    use clap::Parser;
    let args = crate::RpcArgs::try_parse_from(["yomi-rpc", "get_session", "--help"]).unwrap();
    assert_eq!(args.method.as_deref(), Some("get_session"));
    assert!(args.help);
}

#[test]
fn parse_normal_call() {
    use clap::Parser;
    let args = crate::RpcArgs::try_parse_from(["yomi-rpc", "get_config", "--compact"]).unwrap();
    assert_eq!(args.method.as_deref(), Some("get_config"));
    assert!(!args.help);
    assert!(args.compact);
}
