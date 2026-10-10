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

#[test]
fn init_path_validation() {
    const BASE: &str = "[ext]\nname = \"demo\"\nversion = \"0.1.0\"\ndescription = \"t\"\n";

    // 正常：声明存在且常规。
    let dir = write_pkg(&[
        ("ext.toml", &format!("{BASE}init = \"scripts/init.sh\"\n")),
        ("scripts/init.sh", "#!/bin/sh\n"),
    ]);
    let m = parse_manifest(dir.path()).unwrap();
    assert_eq!(m.ext.init.as_deref(), Some("scripts/init.sh"));

    // 越界：指向包外真实存在的文件，拒。canonicalize 要求目标存在，
    // 所以在逃逸路径上真实落一个文件。
    let dir = write_pkg(&[("ext.toml", &format!("{BASE}init = \"../../evil.sh\"\n"))]);
    let escaped = dir.path().join("../../evil.sh");
    std::fs::create_dir_all(escaped.parent().unwrap()).unwrap();
    std::fs::write(&escaped, "#!/bin/sh\n").unwrap();
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("escapes the package"), "{err}");
    std::fs::remove_file(&escaped).ok();

    // 声明了但文件不存在：拒（防手误）。
    let dir = write_pkg(&[("ext.toml", &format!("{BASE}init = \"scripts/nope.sh\"\n"))]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("scripts/nope.sh"), "{err}");

    // 指向目录（非常规文件）：拒。
    let dir = write_pkg(&[
        ("ext.toml", &format!("{BASE}init = \"scripts\"\n")),
        ("scripts/.keep", ""),
    ]);
    std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("regular file"), "{err}");

    // 空白字符断词：拒。
    let dir = write_pkg(&[
        (
            "ext.toml",
            &format!("{BASE}init = \"scripts/my init.sh\"\n"),
        ),
        ("scripts/my init.sh", "#!/bin/sh\n"),
    ]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("only [a-zA-Z0-9._/-]"), "{err}");

    // shell 元字符（注入面）：拒，文件真实存在也拒。
    let dir = write_pkg(&[
        (
            "ext.toml",
            &format!("{BASE}init = \"scripts/$(evil).sh\"\n"),
        ),
        ("scripts/$(evil).sh", "#!/bin/sh\n"),
    ]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("only [a-zA-Z0-9._/-]"), "{err}");

    // 无斜杠：shell 走 PATH 查找会命中 <data_dir>/bin 里别的扩展的
    // 同名 bin，拒。
    let dir = write_pkg(&[
        ("ext.toml", &format!("{BASE}init = \"init.sh\"\n")),
        ("init.sh", "#!/bin/sh\n"),
    ]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("subdirectory path"), "{err}");

    // 绝对路径：join 会被整体替换、装到目标位置后必 127，拒。
    // 用当前平台认的绝对路径——Windows 不认 "/tmp/..." 为绝对；
    // Windows 侧用正斜杠写法（TOML 基本字符串里反斜杠是转义字符，
    // 且 Windows API 接受正斜杠）。
    let abs = if cfg!(windows) {
        "C:/evil.sh"
    } else {
        "/tmp/evil.sh"
    };
    let dir = write_pkg(&[("ext.toml", &format!("{BASE}init = \"{abs}\"\n"))]);
    let err = parse_manifest(dir.path()).unwrap_err();
    assert!(err.to_string().contains("package-relative"), "{err}");

    // 未声明：None，不报错。
    let dir = write_pkg(&[("ext.toml", BASE)]);
    assert!(parse_manifest(dir.path()).unwrap().ext.init.is_none());
}
