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
