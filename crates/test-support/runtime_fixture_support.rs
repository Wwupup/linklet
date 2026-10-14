//! Build a native interpreter stand-in, including executable paths with spaces.
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn build(directory: &Path) -> PathBuf {
    std::fs::create_dir_all(directory).expect("fixture directory");
    let program = directory.join(format!("python{}", std::env::consts::EXE_SUFFIX));
    let source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-support/runtime_fixture.rs");
    assert!(
        Command::new("rustc")
            .arg(source)
            .arg("-o")
            .arg(&program)
            .status()
            .expect("compile interpreter fixture")
            .success()
    );
    for name in ["python3", "powershell", "pwsh"] {
        std::fs::copy(
            &program,
            directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)),
        )
        .expect("interpreter alias");
    }
    program
}
