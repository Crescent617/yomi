use super::*;

#[test]
fn test_completion_list_basic() {
    let mut list: CompletionList<String> = CompletionList::new();
    assert!(!list.is_visible());
    assert!(list.is_empty());

    list.show(vec!["a".to_string(), "b".to_string()]);
    assert!(list.is_visible());
    assert_eq!(list.len(), 2);

    assert_eq!(list.get_selected(), Some(&"a".to_string()));
    list.next();
    assert_eq!(list.get_selected(), Some(&"b".to_string()));
    list.next();
    assert_eq!(list.get_selected(), Some(&"a".to_string())); // wraps

    list.hide();
    assert!(!list.is_visible());
    assert!(list.is_empty());
}

#[test]
fn test_completion_list_prev() {
    let mut list: CompletionList<i32> = CompletionList::new();
    list.show(vec![1, 2, 3]);

    assert_eq!(list.get_selected(), Some(&1));
    list.prev();
    assert_eq!(list.get_selected(), Some(&3)); // wraps to end
    list.prev();
    assert_eq!(list.get_selected(), Some(&2));
}

#[test]
fn test_empty_list() {
    let mut list: CompletionList<String> = CompletionList::new();
    list.show(vec![]);
    assert!(!list.is_visible()); // empty list doesn't show

    list.next(); // no panic
    list.prev(); // no panic
    assert_eq!(list.get_selected(), None);
}
