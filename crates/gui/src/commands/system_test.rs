use super::*;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let suffix = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "yomi-gui-system-test-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn typed_config_error_retains_parser_location() {
    let error = validate_config_toml("# comment\nauto_approve = \"unsupported\"\n")
        .expect_err("typed config should be rejected");

    assert!(error.message.starts_with("Invalid TOML:"));
    assert!(error.message.contains("line 2, column"));
}

#[test]
fn duplicate_model_names_match_startup_validation() {
    let content = r#"
[[models]]
name = "duplicate"

[[models]]
name = "duplicate"
"#;

    let error = validate_config_toml(content).expect_err("duplicate names should be rejected");

    assert_eq!(
        error.message,
        "Invalid config: duplicate model name in [[models]]"
    );
}

#[test]
fn missing_default_model_is_rejected() {
    let content = r#"
[agent]
default_model = "missing"

[[models]]
name = "available"
"#;

    let error = validate_config_toml(content).expect_err("missing default model should fail");

    assert_eq!(
        error.message,
        "Invalid config: agent.default_model must match a [[models]] name"
    );
}

#[test]
fn save_preserves_original_toml_text() {
    let dir = TestDir::new();
    let path = dir.path().join("config.toml");
    let content = "# keep this comment\nmax_checkpoints = 7  # and spacing\n";

    save_config_toml_to_path(&path, content).expect("save config");

    assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(files.len(), 1, "temporary file should be renamed");
}

#[test]
fn save_replaces_existing_config() {
    let dir = TestDir::new();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "# old\n").unwrap();

    save_config_toml_to_path(&path, "# new\n").expect("replace config");

    assert_eq!(std::fs::read_to_string(path).unwrap(), "# new\n");
}

#[test]
fn invalid_config_does_not_replace_existing_file() {
    let dir = TestDir::new();
    let path = dir.path().join("config.toml");
    let original = "# existing config\n";
    std::fs::write(&path, original).unwrap();

    save_config_toml_to_path(&path, "auto_approve = \"unsupported\"\n")
        .expect_err("invalid config should not be written");

    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn save_follows_relative_symlink_without_replacing_it() {
    use std::os::unix::fs::symlink;

    let dir = TestDir::new();
    let target = dir.path().join("managed-config.toml");
    let link = dir.path().join("config.toml");
    std::fs::write(&target, "# old\n").unwrap();
    symlink("managed-config.toml", &link).unwrap();

    save_config_toml_to_path(&link, "# new\n").expect("save through symlink");

    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        Path::new("managed-config.toml")
    );
    assert_eq!(std::fs::read_to_string(target).unwrap(), "# new\n");
}

#[cfg(unix)]
#[test]
fn newly_created_config_has_private_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TestDir::new();
    let path = dir.path().join("config.toml");

    save_config_toml_to_path(&path, "# valid config\n").expect("save config");

    let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
