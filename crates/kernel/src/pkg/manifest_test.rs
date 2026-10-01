//! manifest 解析与校验测试。

use super::{parse_manifest, valid_entry_name, valid_ext_name};

fn write_pkg(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, content) in files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    dir
}

const VALID_TOML: &str = r#"
[ext]
name = "demo"
version = "0.1.0"
description = "test package"

[[cron]]
name = "dream"
schedule = "0 4 * * *"
message_file = "prompts/dream.txt"
"#;

#[test]
fn valid_manifest_parses() {
    let dir = write_pkg(&[("ext.toml", VALID_TOML), ("prompts/dream.txt", "dream now")]);
    let m = parse_manifest(dir.path()).unwrap();
    assert_eq!(m.ext.name, "demo");
    assert_eq!(m.cron.len(), 1);
    assert_eq!(m.cron[0].resolve_message(dir.path()).unwrap(), "dream now");
}

#[test]
fn missing_manifest_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("ext.toml"), "{err}");
}

#[test]
fn bad_ext_name_rejected() {
    for bad in ["Demo", "-demo", "demo_x", "a".repeat(33).as_str()] {
        let toml = VALID_TOML.replace("name = \"demo\"", &format!("name = \"{bad}\""));
        let dir = write_pkg(&[("ext.toml", &toml), ("prompts/dream.txt", "x")]);
        assert!(
            parse_manifest(dir.path()).is_err(),
            "{bad} should be rejected"
        );
    }
}

#[test]
fn message_xor_message_file() {
    let both = r#"
[ext]
name = "demo"
version = "0.1.0"
description = "t"
[[cron]]
name = "c1"
schedule = "0 4 * * *"
message = "inline"
message_file = "prompts/dream.txt"
"#;
    let dir = write_pkg(&[("ext.toml", both), ("prompts/dream.txt", "x")]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("both"), "{err}");

    let neither = r#"
[ext]
name = "demo"
version = "0.1.0"
description = "t"
[[cron]]
name = "c1"
schedule = "0 4 * * *"
"#;
    let dir = write_pkg(&[("ext.toml", neither)]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("neither"), "{err}");
}

#[test]
fn message_file_escape_rejected() {
    // 包在 <root>/pkg，../outside.txt 指向 <root>/outside.txt（包外但存在）。
    let root = tempfile::tempdir().unwrap();
    let pkg = root.path().join("pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(root.path().join("outside.txt"), "x").unwrap();
    let escape = VALID_TOML.replace("prompts/dream.txt", "../outside.txt");
    std::fs::write(pkg.join("ext.toml"), &escape).unwrap();
    let err = parse_manifest(&pkg).unwrap_err();
    assert!(err.to_string().contains("escapes"), "{err}");
}

#[test]
fn bad_schedule_rejected() {
    let bad = VALID_TOML.replace("0 4 * * *", "not a cron");
    let dir = write_pkg(&[("ext.toml", &bad), ("prompts/dream.txt", "x")]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("schedule"), "{err}");
}

#[test]
fn name_rules() {
    assert!(valid_ext_name("memory-system"));
    assert!(valid_ext_name("a1"));
    assert!(!valid_ext_name("1a"));
    assert!(!valid_ext_name("A"));
    assert!(!valid_ext_name("a_b"));
    assert!(valid_entry_name("dream_1"));
    assert!(!valid_entry_name("1dream"));
}

#[test]
fn duplicate_cron_entry_names_rejected() {
    let dup = r#"
[ext]
name = "demo"
version = "0.1.0"
description = "t"
[[cron]]
name = "dream"
schedule = "0 4 * * *"
message = "a"
[[cron]]
name = "dream"
schedule = "0 5 * * *"
message = "b"
"#;
    let dir = write_pkg(&[("ext.toml", dup)]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("duplicate"), "{err}");
}

#[test]
fn message_and_field_caps_enforced() {
    // 内联 message 超长。
    let big = "x".repeat(64 * 1024 + 1);
    let toml = format!(
        r#"
[ext]
name = "demo"
version = "0.1.0"
description = "t"

[[cron]]
name = "dream"
schedule = "0 4 * * *"
message = "{big}"
"#
    );
    let dir = write_pkg(&[("ext.toml", &toml)]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("too long"), "{err}");

    // message_file 超大小上限。
    let dir = write_pkg(&[
        ("ext.toml", VALID_TOML),
        ("prompts/dream.txt", &"y".repeat(70 * 1024)),
    ]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("too large"), "{err}");

    // message_file 不是常规文件（目录；FIFO 同走这道 is_file 闸）：拒，
    // 绝不阻塞读。
    {
        let dir = write_pkg(&[("ext.toml", VALID_TOML), ("prompts/dream.txt", "x")]);
        let toml = VALID_TOML.replace("prompts/dream.txt", "prompts");
        std::fs::write(dir.path().join("ext.toml"), toml).unwrap();
        let err = parse_manifest(dir.path()).unwrap_err();
        assert!(err.to_string().contains("regular file"), "{err}");
    }

    // description 超长。
    let toml = VALID_TOML.replace(
        "description = \"test package\"",
        &format!("description = \"{}\"", "d".repeat(257)),
    );
    let dir = write_pkg(&[("ext.toml", &toml), ("prompts/dream.txt", "x")]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("description"), "{err}");
}

#[test]
fn too_many_cron_entries_rejected() {
    use std::fmt::Write as _;
    let mut toml =
        String::from("[ext]\nname = \"demo\"\nversion = \"0.1.0\"\ndescription = \"t\"\n");
    for i in 0..257 {
        let _ = write!(
            toml,
            "[[cron]]\nname = \"c{i}\"\nschedule = \"0 4 * * *\"\nmessage = \"m\"\n"
        );
    }
    let dir = write_pkg(&[("ext.toml", &toml)]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("too many"), "{err}");
}
