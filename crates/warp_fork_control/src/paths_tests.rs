use super::*;

#[test]
fn dir_name_is_scoped_per_profile() {
    assert_eq!(socket_dir_name(None), "dev.warp.Warp");
    assert_eq!(socket_dir_name(Some("dev")), "dev.warp.Warp-dev");
}

#[test]
fn default_path_ends_with_socket_file() {
    let path = default_socket_path_without_env(None).expect("a data dir exists in tests");
    assert!(path.ends_with("control.sock"), "{}", path.display());
}
