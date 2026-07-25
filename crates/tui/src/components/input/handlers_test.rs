use super::InputComponent;
use crate::msg::Msg;

#[test]
fn test_parse_models_command_without_key() {
    assert_eq!(
        InputComponent::parse_command("/models"),
        Some(Msg::CommandModels(None))
    );
    assert_eq!(
        InputComponent::parse_command("/model"),
        Some(Msg::CommandModels(None))
    );
}

#[test]
fn test_parse_models_command_with_key() {
    assert_eq!(
        InputComponent::parse_command("/models k2"),
        Some(Msg::CommandModels(Some("k2".to_string())))
    );
    assert_eq!(
        InputComponent::parse_command("/model claude-sonnet"),
        Some(Msg::CommandModels(Some("claude-sonnet".to_string())))
    );
    // Extra arguments beyond the key are ignored
    assert_eq!(
        InputComponent::parse_command("/models k2 extra"),
        Some(Msg::CommandModels(Some("k2".to_string())))
    );
}

#[test]
fn test_parse_non_command_returns_none() {
    assert_eq!(InputComponent::parse_command("hello world"), None);
    assert_eq!(InputComponent::parse_command("/unknown"), None);
}

#[test]
fn test_parse_steer_command_with_content() {
    assert_eq!(
        InputComponent::parse_command("/steer focus on tests"),
        Some(Msg::CommandSteer(vec![kernel::types::ContentBlock::Text {
            text: "focus on tests".to_string(),
        },]))
    );
}

#[test]
fn test_parse_bare_steer_promotes_queued_message() {
    assert_eq!(
        InputComponent::parse_command("/steer"),
        Some(Msg::SteerQueuedMessage)
    );
    // Whitespace-only content is also treated as bare /steer
    assert_eq!(
        InputComponent::parse_command("/steer   "),
        Some(Msg::SteerQueuedMessage)
    );
}

#[test]
fn test_parse_other_commands_still_work() {
    assert_eq!(
        InputComponent::parse_command("/sessions"),
        Some(Msg::CommandSessions)
    );
    assert_eq!(InputComponent::parse_command("/new"), Some(Msg::CommandNew));
}
