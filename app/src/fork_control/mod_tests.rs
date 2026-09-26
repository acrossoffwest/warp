use super::is_enabled;

#[test]
fn setting_and_env_both_gate_the_server() {
    assert!(is_enabled(true, None));
    assert!(is_enabled(true, Some("1")));
    assert!(!is_enabled(false, None));
    for off in ["0", "false", "off"] {
        assert!(!is_enabled(true, Some(off)), "{off}");
    }
}
