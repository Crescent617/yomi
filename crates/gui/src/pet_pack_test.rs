use std::path::Path;
use std::sync::OnceLock;

use image::codecs::webp::WebPEncoder;
use tempfile::TempDir;

use crate::pet_pack::discover_pet_packs_at;

const WIDTH: u32 = 1536;
const V1_HEIGHT: u32 = 1872;
const V2_HEIGHT: u32 = 2288;

fn valid_webp() -> &'static [u8] {
    static WEBP: OnceLock<Vec<u8>> = OnceLock::new();
    WEBP.get_or_init(|| {
        let pixels = vec![0_u8; (WIDTH * V1_HEIGHT) as usize];
        let mut bytes = Vec::new();
        WebPEncoder::new_lossless(&mut bytes)
            .encode(&pixels, WIDTH, V1_HEIGHT, image::ExtendedColorType::L8)
            .expect("encode WebP fixture");
        bytes
    })
}

fn webp_with_dimensions(width: u32, height: u32) -> Vec<u8> {
    let pixels = vec![0_u8; (width * height) as usize];
    let mut bytes = Vec::new();
    WebPEncoder::new_lossless(&mut bytes)
        .encode(&pixels, width, height, image::ExtendedColorType::L8)
        .expect("encode WebP fixture");
    bytes
}

#[allow(clippy::needless_pass_by_value)]
fn write_pack(root: &Path, directory: &str, manifest: serde_json::Value, webp: &[u8]) {
    let directory = root.join(directory);
    std::fs::create_dir_all(&directory).expect("create pack directory");
    std::fs::write(
        directory.join("pet.json"),
        serde_json::to_vec(&manifest).expect("serialize manifest"),
    )
    .expect("write manifest");
    std::fs::write(directory.join("spritesheet.webp"), webp).expect("write spritesheet");
}

fn manifest(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "displayName": format!("Display {id}"),
        "description": format!("Description {id}"),
        "spritesheetPath": "spritesheet.webp",
        "spriteVersionNumber": 1,
        "unknownV2Field": true
    })
}

#[test]
fn discovers_valid_packs_in_deterministic_id_order() {
    let root = TempDir::new().expect("temp directory");
    let mut second = manifest("zebra");
    second["kind"] = serde_json::json!("cat");
    write_pack(root.path(), "zebra", second, valid_webp());
    let mut first = manifest("alpha");
    first
        .as_object_mut()
        .expect("manifest object")
        .remove("spriteVersionNumber");
    write_pack(root.path(), "alpha", first, valid_webp());

    let packs = discover_pet_packs_at(root.path()).expect("discover packs");

    assert_eq!(packs.len(), 2);
    assert_eq!(packs[0].id, "alpha");
    assert_eq!(packs[0].display_name, "Display alpha");
    assert_eq!(packs[0].description, "Description alpha");
    assert_eq!(packs[0].kind, None);
    assert_eq!(packs[0].sprite_version_number, 1);
    assert_eq!(packs[1].id, "zebra");
    assert_eq!(packs[1].kind.as_deref(), Some("cat"));
}

#[test]
fn missing_root_is_empty_and_is_not_created() {
    let parent = TempDir::new().expect("temp directory");
    let root = parent.path().join("missing");

    assert!(discover_pet_packs_at(&root)
        .expect("discover missing root")
        .is_empty());
    assert!(!root.exists());
}

#[test]
fn malformed_mismatched_and_unsupported_version_packs_are_skipped() {
    let root = TempDir::new().expect("temp directory");
    write_pack(root.path(), "valid", manifest("valid"), valid_webp());

    let malformed = root.path().join("malformed");
    std::fs::create_dir(&malformed).expect("create malformed pack");
    std::fs::write(malformed.join("pet.json"), b"{").expect("write malformed manifest");

    write_pack(root.path(), "mismatch", manifest("different"), valid_webp());
    let mut unsupported = manifest("unsupported");
    unsupported["spriteVersionNumber"] = serde_json::json!(3);
    write_pack(root.path(), "unsupported", unsupported, valid_webp());

    let packs = discover_pet_packs_at(root.path()).expect("discover packs");
    assert_eq!(
        packs
            .iter()
            .map(|pack| pack.id.as_str())
            .collect::<Vec<_>>(),
        ["valid"]
    );
}

#[test]
fn discovers_v2_pack_and_rejects_version_dimension_mismatches() {
    let root = TempDir::new().expect("temp directory");
    let mut v2 = manifest("v2");
    v2["spriteVersionNumber"] = serde_json::json!(2);
    write_pack(
        root.path(),
        "v2",
        v2,
        &webp_with_dimensions(WIDTH, V2_HEIGHT),
    );

    let mut v2_with_v1_sheet = manifest("v2-wrong-height");
    v2_with_v1_sheet["spriteVersionNumber"] = serde_json::json!(2);
    write_pack(
        root.path(),
        "v2-wrong-height",
        v2_with_v1_sheet,
        valid_webp(),
    );

    write_pack(
        root.path(),
        "v1-wrong-height",
        manifest("v1-wrong-height"),
        &webp_with_dimensions(WIDTH, V2_HEIGHT),
    );

    let packs = discover_pet_packs_at(root.path()).expect("discover packs");
    assert_eq!(packs.len(), 1);
    assert_eq!(packs[0].id, "v2");
    assert_eq!(packs[0].sprite_version_number, 2);
}

#[test]
fn wrong_dimensions_and_non_webp_are_skipped() {
    let root = TempDir::new().expect("temp directory");
    write_pack(
        root.path(),
        "wrong-size",
        manifest("wrong-size"),
        &webp_with_dimensions(1, 1),
    );
    write_pack(
        root.path(),
        "not-webp",
        manifest("not-webp"),
        b"not a WebP image",
    );

    assert!(discover_pet_packs_at(root.path())
        .expect("discover packs")
        .is_empty());
}

#[test]
fn traversal_and_oversized_manifest_are_skipped() {
    let root = TempDir::new().expect("temp directory");
    let traversal = root.path().join("traversal");
    std::fs::create_dir(&traversal).expect("create traversal pack");
    let mut traversal_manifest = manifest("traversal");
    traversal_manifest["spritesheetPath"] = serde_json::json!("../outside.webp");
    std::fs::write(
        traversal.join("pet.json"),
        serde_json::to_vec(&traversal_manifest).expect("serialize manifest"),
    )
    .expect("write manifest");
    std::fs::write(root.path().join("outside.webp"), valid_webp()).expect("write outside WebP");

    let oversized = root.path().join("oversized");
    std::fs::create_dir(&oversized).expect("create oversized pack");
    std::fs::write(oversized.join("pet.json"), vec![b' '; 64 * 1024 + 1])
        .expect("write oversized manifest");

    assert!(discover_pet_packs_at(root.path())
        .expect("discover packs")
        .is_empty());
}

#[cfg(unix)]
#[test]
fn symlink_escapes_are_skipped() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("temp directory");
    let outside = TempDir::new().expect("outside temp directory");

    write_pack(
        outside.path(),
        "escaped-pack",
        manifest("escaped-pack"),
        valid_webp(),
    );
    symlink(
        outside.path().join("escaped-pack"),
        root.path().join("escaped-pack"),
    )
    .expect("symlink escaped pack");

    let sheet_escape = root.path().join("sheet-escape");
    std::fs::create_dir(&sheet_escape).expect("create sheet escape pack");
    std::fs::write(
        sheet_escape.join("pet.json"),
        serde_json::to_vec(&manifest("sheet-escape")).expect("serialize manifest"),
    )
    .expect("write manifest");
    std::fs::write(outside.path().join("outside.webp"), valid_webp())
        .expect("write outside spritesheet");
    symlink(
        outside.path().join("outside.webp"),
        sheet_escape.join("spritesheet.webp"),
    )
    .expect("symlink escaped spritesheet");

    assert!(discover_pet_packs_at(root.path())
        .expect("discover packs")
        .is_empty());
}
