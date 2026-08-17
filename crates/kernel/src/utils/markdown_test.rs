use super::map_outside_fences;

fn upper(segment: &str, out: &mut String) {
    out.push_str(&segment.to_uppercase());
}

#[test]
fn plain_text_maps_whole() {
    assert_eq!(map_outside_fences("hello", &upper), "HELLO");
}

#[test]
fn fenced_run_passes_verbatim() {
    let text = "before\n```\nkeep me\n```\nafter";
    assert_eq!(
        map_outside_fences(text, &upper),
        "BEFORE\n```\nkeep me\n```\nAFTER"
    );
}

#[test]
fn indented_marker_counts_as_fence() {
    let text = "a\n  ```rust\nkeep\n  ```\nb";
    assert_eq!(
        map_outside_fences(text, &upper),
        "A\n  ```rust\nkeep\n  ```\nB"
    );
}

#[test]
fn inline_backticks_are_not_fences() {
    assert_eq!(map_outside_fences("a `x` b", &upper), "A `X` B");
}

#[test]
fn unterminated_fence_swallows_rest_verbatim() {
    let text = "a\n```\nkeep\nkeep2";
    assert_eq!(map_outside_fences(text, &upper), "A\n```\nkeep\nkeep2");
}

#[test]
fn multiple_fences_split_runs() {
    let text = "```\nx\n```\nmid\n```\ny\n```\nend";
    assert_eq!(
        map_outside_fences(text, &upper),
        "```\nx\n```\nMID\n```\ny\n```\nEND"
    );
}

#[test]
fn empty_text_maps_to_empty() {
    assert_eq!(map_outside_fences("", &upper), "");
}
