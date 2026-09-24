use super::ReqMethod;

#[test]
fn list_running_sessions_request_round_trips() {
    let json = serde_json::to_string(&ReqMethod::ListRunningSessions).unwrap();
    assert_eq!(json, "\"list_running_sessions\"");
    assert_eq!(
        serde_json::from_str::<ReqMethod>(&json).unwrap(),
        ReqMethod::ListRunningSessions
    );
}

#[test]
fn btw_request_round_trips() {
    let method = ReqMethod::Btw {
        session_id: "sess_1".to_string(),
        question: "刚才那个变量叫什么？".to_string(),
        request_id: None,
    };
    let json = serde_json::to_string(&method).unwrap();
    assert_eq!(serde_json::from_str::<ReqMethod>(&json).unwrap(), method);

    // request_id 缺省可省（客户端不带 id 的常见调用形状）。
    let parsed: ReqMethod =
        serde_json::from_str(r#"{"btw":{"session_id":"sess_1","question":"q"}}"#).unwrap();
    assert_eq!(
        parsed,
        ReqMethod::Btw {
            session_id: "sess_1".to_string(),
            question: "q".to_string(),
            request_id: None,
        }
    );
}
