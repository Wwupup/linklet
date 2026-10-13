//! Actual account observation and OS-enforced file access, with no sudo inference.
#[test]
fn identity_records_the_current_process_authority() {
    let cwd = std::env::current_dir().expect("cwd");
    let identity = linklet_adapters::identity::observe(&cwd, "test");
    assert_eq!(identity.os.as_deref(), Some(std::env::consts::OS));
    assert!(identity.observed_at_ms.is_some());
    #[cfg(target_os = "linux")]
    {
        let uid = std::process::Command::new("id")
            .arg("-u")
            .output()
            .expect("uid");
        let elevated = String::from_utf8(uid.stdout).expect("uid").trim() == "0";
        assert_eq!(identity.elevated, Some(elevated));
        let account = std::process::Command::new("id")
            .arg("-un")
            .output()
            .expect("account");
        assert_eq!(
            identity.account.as_deref(),
            Some(String::from_utf8(account.stdout).expect("account").trim())
        );
    }
    println!(
        "AUTHORITY os={} account={:?} elevated={:?} warnings={:?}",
        std::env::consts::OS,
        identity.account,
        identity.elevated,
        identity.warnings
    );
}

#[cfg(target_os = "linux")]
#[test]
fn file_access_uses_the_current_effective_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::temp_dir().join(format!("linklet-permission-{}", std::process::id()));
    std::fs::write(&path, b"protected-content").expect("fixture");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
        .expect("remove read permission");
    let paths = linklet_core::transfer::Destination::new("/tmp").expect("base");
    let reply = linklet_adapters::tail::read(
        &paths,
        &linklet_core::wire::TailRequest {
            path: path.to_string_lossy().into_owned(),
            count: 20,
            cursor: None,
            content_bytes: 1024,
        },
    );
    let identity = linklet_adapters::identity::observe(std::path::Path::new("/tmp"), "test");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("restore");
    std::fs::remove_file(&path).expect("cleanup");
    if identity.elevated == Some(true) {
        assert_eq!(reply.expect("root can read").text, "protected-content");
    } else {
        assert_eq!(
            reply.expect_err("ordinary account denied").code,
            linklet_core::result::ErrorCode::PermissionDenied
        );
    }
    println!(
        "PERMISSION elevated={:?} denied_without_elevation=true",
        identity.elevated
    );
}
