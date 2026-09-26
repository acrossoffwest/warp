use std::path::{Path, PathBuf};

use super::agent_lookup_cwd;

#[test]
fn agent_lookup_cwd_strips_trailing_slash_of_missing_path() {
    assert_eq!(
        agent_lookup_cwd(Path::new("/definitely/not/here/project/")),
        PathBuf::from("/definitely/not/here/project")
    );
}

#[test]
fn agent_lookup_cwd_keeps_root() {
    assert_eq!(agent_lookup_cwd(Path::new("/")), PathBuf::from("/"));
}

#[test]
fn agent_lookup_cwd_expands_home() {
    let home = dirs::home_dir().expect("home dir should be known");
    assert_eq!(
        agent_lookup_cwd(Path::new("~/definitely-not-here-project")),
        home.join("definitely-not-here-project")
    );
}

#[cfg(unix)]
#[test]
fn agent_lookup_cwd_resolves_symlinks() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let real = tempdir.path().join("real");
    std::fs::create_dir(&real).expect("dir should be created");
    let link = tempdir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink should be created");

    assert_eq!(
        agent_lookup_cwd(&link),
        std::fs::canonicalize(&real).expect("real dir should canonicalize")
    );
}
