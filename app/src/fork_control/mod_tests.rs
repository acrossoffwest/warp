use warp_fork_control::protocol::ErrorCode;

use super::{is_enabled, validate_cwd};

#[test]
fn setting_and_env_both_gate_the_server() {
    assert!(is_enabled(true, None));
    assert!(is_enabled(true, Some("1")));
    assert!(!is_enabled(false, None));
    for off in ["0", "false", "off"] {
        assert!(!is_enabled(true, Some(off)), "{off}");
    }
}

#[test]
fn open_tab_cwd_must_be_an_existing_absolute_directory() {
    let dir = std::env::temp_dir();
    assert!(validate_cwd(dir.to_str().unwrap()).is_ok());
    for bad in ["relative/dir", "~/foo", ""] {
        assert_eq!(
            validate_cwd(bad).unwrap_err().code,
            ErrorCode::BadRequest,
            "{bad}"
        );
    }
    let file = dir.join(format!("wfc-cwd-{}", std::process::id()));
    std::fs::write(&file, b"").unwrap();
    let result = validate_cwd(file.to_str().unwrap());
    std::fs::remove_file(&file).unwrap();
    assert_eq!(result.unwrap_err().code, ErrorCode::BadRequest);
}
