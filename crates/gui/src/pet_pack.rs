use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

use image::{ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};

const MANIFEST_FILE_NAME: &str = "pet.json";
const MAX_MANIFEST_SIZE: u64 = 64 * 1024;
const MAX_SPRITESHEET_SIZE: u64 = 32 * 1024 * 1024;
const SPRITESHEET_WIDTH: u32 = 1536;
const V1_SPRITESHEET_HEIGHT: u32 = 1872;
const V2_SPRITESHEET_HEIGHT: u32 = 2288;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PetPack {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub kind: Option<String>,
    pub sprite_version_number: u32,
}

#[derive(Debug, Deserialize)]
struct ExternalPetManifest {
    id: String,
    #[serde(rename = "displayName")]
    display_name: String,
    description: String,
    #[serde(rename = "spritesheetPath")]
    spritesheet_path: String,
    kind: Option<String>,
    #[serde(rename = "spriteVersionNumber")]
    sprite_version_number: Option<u32>,
}

#[derive(Debug)]
pub(crate) struct ValidatedPetPack {
    pub pack: PetPack,
    spritesheet_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum PetPackError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
}

pub fn pet_pack_root() -> PathBuf {
    kernel::expand_tilde(kernel::DEFAULT_DATA_DIR).join("pets")
}

pub fn discover_pet_packs() -> Result<Vec<PetPack>, PetPackError> {
    discover_pet_packs_at(&pet_pack_root())
}

pub(crate) fn discover_pet_packs_at(root: &Path) -> Result<Vec<PetPack>, PetPackError> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let canonical_root = root.canonicalize()?;
    if !canonical_root.is_dir() {
        return Err(PetPackError::Invalid(format!(
            "pet pack root is not a directory: {}",
            root.display()
        )));
    }

    let mut packs = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!("Skipping unreadable pet pack directory entry: {error}");
                continue;
            }
        };
        if !entry.path().is_dir() {
            continue;
        }
        let Some(directory_id) = entry.file_name().to_str().map(str::to_owned) else {
            tracing::warn!(path = %entry.path().display(), "Skipping pet pack with non-UTF-8 directory name");
            continue;
        };

        match validate_pet_pack_in_root(root, &canonical_root, &directory_id) {
            Ok(validated) => packs.push(validated.pack),
            Err(error) => tracing::warn!(
                pet_pack = %entry.path().display(),
                "Skipping invalid pet pack: {error}"
            ),
        }
    }

    packs.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(packs)
}

pub(crate) fn validate_pet_pack(id: &str) -> Result<ValidatedPetPack, PetPackError> {
    let root = pet_pack_root();
    let canonical_root = root.canonicalize()?;
    validate_pet_pack_in_root(&root, &canonical_root, id)
}

pub(crate) fn read_pet_spritesheet(
    id: &str,
    expected_sprite_version_number: u32,
) -> Result<Vec<u8>, PetPackError> {
    let validated = validate_pet_pack(id)?;
    if validated.pack.sprite_version_number != expected_sprite_version_number {
        return Err(PetPackError::Invalid(format!(
            "spriteVersionNumber changed while loading: expected {expected_sprite_version_number}, got {}",
            validated.pack.sprite_version_number
        )));
    }
    let bytes = read_limited(
        &validated.spritesheet_path,
        MAX_SPRITESHEET_SIZE,
        "spritesheet",
    )?;
    validate_webp_dimensions(
        std::io::Cursor::new(&bytes),
        validated.pack.sprite_version_number,
    )?;
    Ok(bytes)
}

fn validate_pet_pack_in_root(
    root: &Path,
    canonical_root: &Path,
    directory_id: &str,
) -> Result<ValidatedPetPack, PetPackError> {
    validate_single_component(directory_id, "pet pack id")?;

    let pack_path = root.join(directory_id);
    let canonical_pack =
        contained_canonical_path(&pack_path, canonical_root, "pet pack directory")?;
    if !canonical_pack.is_dir() {
        return Err(PetPackError::Invalid(format!(
            "pet pack is not a directory: {}",
            pack_path.display()
        )));
    }

    let manifest_path = pack_path.join(MANIFEST_FILE_NAME);
    let canonical_manifest = contained_canonical_path(&manifest_path, &canonical_pack, "manifest")?;
    let manifest_bytes = read_limited(&canonical_manifest, MAX_MANIFEST_SIZE, "manifest")?;
    let manifest: ExternalPetManifest = serde_json::from_slice(&manifest_bytes)?;

    validate_single_component(&manifest.id, "manifest id")?;
    if manifest.id != directory_id {
        return Err(PetPackError::Invalid(format!(
            "manifest id {:?} does not match directory {:?}",
            manifest.id, directory_id
        )));
    }
    let sprite_version_number = manifest.sprite_version_number.unwrap_or(1);
    if !matches!(sprite_version_number, 1 | 2) {
        return Err(PetPackError::Invalid(
            "spriteVersionNumber must be 1, 2, or omitted".into(),
        ));
    }

    let relative_spritesheet = Path::new(&manifest.spritesheet_path);
    validate_relative_path(relative_spritesheet, "spritesheetPath")?;
    if relative_spritesheet
        .extension()
        .and_then(|value| value.to_str())
        != Some("webp")
    {
        return Err(PetPackError::Invalid(
            "spritesheetPath must reference a .webp file".into(),
        ));
    }

    let spritesheet_path = contained_canonical_path(
        &pack_path.join(relative_spritesheet),
        &canonical_pack,
        "spritesheet",
    )?;
    let metadata = spritesheet_path.metadata()?;
    if !metadata.is_file() {
        return Err(PetPackError::Invalid(format!(
            "spritesheet is not a file: {}",
            spritesheet_path.display()
        )));
    }
    if metadata.len() > MAX_SPRITESHEET_SIZE {
        return Err(PetPackError::Invalid(format!(
            "spritesheet exceeds {MAX_SPRITESHEET_SIZE} bytes"
        )));
    }
    let reader = BufReader::new(File::open(&spritesheet_path)?);
    validate_webp_dimensions(reader, sprite_version_number)?;

    Ok(ValidatedPetPack {
        pack: PetPack {
            id: manifest.id,
            display_name: manifest.display_name,
            description: manifest.description,
            kind: manifest.kind,
            sprite_version_number,
        },
        spritesheet_path,
    })
}

fn validate_single_component(value: &str, field: &str) -> Result<(), PetPackError> {
    let mut components = Path::new(value).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(PetPackError::Invalid(format!(
            "{field} must be one safe path component"
        )));
    }
    Ok(())
}

fn validate_relative_path(path: &Path, field: &str) -> Result<(), PetPackError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PetPackError::Invalid(format!(
            "{field} must be a safe relative path"
        )));
    }
    Ok(())
}

fn contained_canonical_path(
    path: &Path,
    canonical_parent: &Path,
    description: &str,
) -> Result<PathBuf, PetPackError> {
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(canonical_parent) {
        return Err(PetPackError::Invalid(format!(
            "{description} escapes its pet pack directory"
        )));
    }
    Ok(canonical)
}

fn read_limited(path: &Path, limit: u64, description: &str) -> Result<Vec<u8>, PetPackError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() {
        return Err(PetPackError::Invalid(format!(
            "{description} is not a file: {}",
            path.display()
        )));
    }
    if metadata.len() > limit {
        return Err(PetPackError::Invalid(format!(
            "{description} exceeds {limit} bytes"
        )));
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(PetPackError::Invalid(format!(
            "{description} exceeds {limit} bytes"
        )));
    }
    Ok(bytes)
}

fn validate_webp_dimensions(
    reader: impl std::io::BufRead + std::io::Seek,
    sprite_version_number: u32,
) -> Result<(), PetPackError> {
    let expected_height = match sprite_version_number {
        1 => V1_SPRITESHEET_HEIGHT,
        2 => V2_SPRITESHEET_HEIGHT,
        _ => {
            return Err(PetPackError::Invalid(format!(
                "unsupported spriteVersionNumber: {sprite_version_number}"
            )));
        }
    };
    let dimensions = ImageReader::with_format(reader, ImageFormat::WebP).into_dimensions()?;
    if dimensions != (SPRITESHEET_WIDTH, expected_height) {
        return Err(PetPackError::Invalid(format!(
            "spritesheet dimensions for V{sprite_version_number} must be {SPRITESHEET_WIDTH}x{expected_height}, got {}x{}",
            dimensions.0, dimensions.1
        )));
    }
    Ok(())
}
