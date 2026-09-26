const MAX_DEPTH: usize = 64;

/// Walks `pid`'s ancestors and returns the key of the first shell pid found.
pub fn find_pane_for_pid<K: Copy>(
    pid: u32,
    shells: &[(K, u32)],
    parent_of: impl Fn(u32) -> Option<u32>,
) -> Option<K> {
    let mut current = pid;
    for _ in 0..MAX_DEPTH {
        if let Some((key, _)) = shells.iter().find(|(_, shell_pid)| *shell_pid == current) {
            return Some(*key);
        }
        match parent_of(current) {
            Some(parent) if parent != current && parent != 0 => current = parent,
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
#[path = "pids_tests.rs"]
mod tests;
