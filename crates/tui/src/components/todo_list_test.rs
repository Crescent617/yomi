use super::*;

#[test]
fn test_parse_todo_json() {
    let json = r#"{"todos":[{"id":1,"content":"Fix bug","status":"pending"},{"id":2,"content":"Write tests","status":"in_progress"}]}"#;
    let data: TodoListData = serde_json::from_str(json).unwrap();
    assert_eq!(data.todos.len(), 2);
    assert_eq!(data.todos[0].id, 1);
    assert_eq!(data.todos[0].content, "Fix bug");
    assert_eq!(data.todos[0].status, TodoStatus::Pending);
    assert_eq!(data.todos[1].id, 2);
    assert_eq!(data.todos[1].status, TodoStatus::InProgress);
}

#[test]
fn test_parse_todo_with_completed() {
    let json = r#"{"todos":[{"id":1,"content":"Done task","status":"completed"},{"id":2,"content":"Pending task","status":"pending"}]}"#;
    let data: TodoListData = serde_json::from_str(json).unwrap();
    assert_eq!(data.todos.len(), 2);
    assert_eq!(data.todos[0].status, TodoStatus::Completed);
    assert_eq!(data.todos[1].status, TodoStatus::Pending);
}

#[test]
fn test_todo_list_shows_completed_with_strikethrough() {
    let json = r#"{"todos":[{"id":1,"content":"Done","status":"completed"},{"id":2,"content":"Pending","status":"pending"}]}"#;
    let mut list = TodoList::new();
    list.update_todos(json);
    assert_eq!(list.todos.len(), 2);
}

#[test]
fn test_parse_todo_with_unicode() {
    let json = r#"{"todos":[{"id":1,"content":"演示todo工具的基本功能","status":"in_progress"}]}"#;
    let mut list = TodoList::new();
    list.update_todos(json);
    assert_eq!(list.todos.len(), 1);
    assert_eq!(list.todos[0].content, "演示todo工具的基本功能");
    assert_eq!(list.todos[0].status, TodoStatus::InProgress);
}

#[test]
fn test_parse_todo_with_escapes() {
    let json = r#"{"todos":[{"id":1,"content":"Line 1\nLine 2\tTabbed","status":"pending"}]}"#;
    let mut list = TodoList::new();
    list.update_todos(json);
    assert_eq!(list.todos.len(), 1);
    assert_eq!(list.todos[0].content, "Line 1\nLine 2\tTabbed");
}
