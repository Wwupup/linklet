//! Account-local persistence, reset exclusivity and per-target controller records.
use linklet_adapters::credentials::{AgentCredentials, save_token, saved_token};
#[test]
fn credentials_survive_restart_reset_under_lock_and_keep_targets_separate() {
    let dir = std::env::temp_dir().join(format!("linklet-credentials-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("existing directory");
    let credentials = AgentCredentials::acquire(&dir, false).expect("initialize");
    let code = credentials.code().to_string();
    let token = credentials.token().clone();
    assert!(
        AgentCredentials::acquire(&dir, true).is_err(),
        "running agent prevents reset"
    );
    save_token(&dir, "one:8787", &token).expect("save");
    assert_eq!(
        saved_token(&dir, "ONE:8787").expect("read"),
        Some(token.clone())
    );
    assert_eq!(saved_token(&dir, "two:8787").expect("read"), None);
    drop(credentials);
    let credentials = AgentCredentials::acquire(&dir, false).expect("restart");
    assert_eq!(credentials.code(), code);
    assert_eq!(credentials.token(), &token);
    drop(credentials);
    let credentials = AgentCredentials::acquire(&dir, true).expect("reset");
    assert_ne!(credentials.token(), &token);
    save_token(&dir, "one:8787", credentials.token()).expect("replace");
    assert_eq!(
        saved_token(&dir, "one:8787").expect("read"),
        Some(credentials.token().clone())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&dir)
                .expect("directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(dir.join("agent.json"))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    #[cfg(windows)]
    {
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=[System.IO.Directory]::GetAccessControl($env:LINKLET_TEST_DIRECTORY); if(-not $acl.AreAccessRulesProtected){exit 1}; foreach($path in @($env:LINKLET_TEST_DIRECTORY,($env:LINKLET_TEST_DIRECTORY+'/agent.json'))){$acl=Get-Acl -LiteralPath $path; $rules=$acl.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier]); if($rules.Count -ne 1 -or $rules[0].IdentityReference -ne $sid -or $rules[0].AccessControlType -ne 'Allow'){exit 2}}"])
            .env("LINKLET_TEST_DIRECTORY", &dir).output().expect("read Windows ACL");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    drop(credentials);
    std::fs::write(dir.join("agent.json"), b"broken").expect("corrupt");
    assert!(
        AgentCredentials::acquire(&dir, false).is_err(),
        "never silently rotate corrupted state"
    );
    drop(AgentCredentials::acquire(&dir, true).expect("explicit recovery"));
    std::fs::remove_dir_all(dir).expect("cleanup");
}
