use std::collections::HashMap;

use super::*;

fn parents(pairs: &[(u32, u32)]) -> impl Fn(u32) -> Option<u32> {
    let map: HashMap<u32, u32> = pairs.iter().copied().collect();
    move |pid| map.get(&pid).copied()
}

#[test]
fn finds_pane_through_ancestors() {
    let shells = [("a", 100), ("b", 200)];
    let parent_of = parents(&[(300, 250), (250, 200), (200, 1)]);
    assert_eq!(find_pane_for_pid(300, &shells, parent_of), Some("b"));
}

#[test]
fn shell_pid_itself_matches() {
    let shells = [("a", 100)];
    assert_eq!(find_pane_for_pid(100, &shells, parents(&[])), Some("a"));
}

#[test]
fn unrelated_pid_is_none() {
    let shells = [("a", 100)];
    assert_eq!(find_pane_for_pid(42, &shells, parents(&[(42, 1)])), None);
}

#[test]
fn parent_cycle_terminates() {
    let shells = [("a", 100)];
    assert_eq!(
        find_pane_for_pid(5, &shells, parents(&[(5, 6), (6, 5)])),
        None
    );
}
