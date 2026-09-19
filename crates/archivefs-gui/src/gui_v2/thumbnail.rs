//! Bounded worker-only decode and immutable, no-clobber persistent thumbnails.
use eframe::egui;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};

pub(super) const WIDTH: u32 = 240;
pub(super) const HEIGHT: u32 = 320;
const MAX_SOURCE: u64 = 32 * 1024 * 1024;
const MAX_CACHE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub(super) struct Timings {
    pub metadata: Duration,
    pub lookup: Duration,
    pub network: Duration,
    pub decode: Duration,
    pub resize: Duration,
    pub provider_processing: Duration,
    pub delivery: Duration,
    pub cache_hit: bool,
}

pub(super) struct Pixels {
    pub image: egui::ColorImage,
    pub timings: Timings,
}

pub(super) fn cache_root() -> Result<PathBuf, String> {
    archivefs_core::app_dirs::data_path("gui-v2-thumbnails-v1").map_err(|error| error.to_string())
}

fn fingerprint(path: &Path, metadata: &fs::Metadata) -> String {
    let mut hash = Sha256::new();
    hash.update(path.as_os_str().as_encoded_bytes());
    hash.update(metadata.len().to_le_bytes());
    if let Ok(time) = metadata.modified().and_then(|time| {
        time.duration_since(UNIX_EPOCH)
            .map_err(std::io::Error::other)
    }) {
        hash.update(time.as_secs().to_le_bytes());
        hash.update(time.subsec_nanos().to_le_bytes());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash.update(metadata.ino().to_le_bytes());
        hash.update(metadata.ctime().to_le_bytes());
        hash.update(metadata.ctime_nsec().to_le_bytes());
    }
    hash.update(b"240x320-v1");
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn load_local(path: &Path, root: &Path) -> Result<Pixels, String> {
    let start = Instant::now();
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err("The artwork location is not safe to use.".into());
    }
    let canonical = fs::canonicalize(path).map_err(|_| "The picture is no longer available.")?;
    let metadata = fs::metadata(&canonical).map_err(|_| "The picture could not be read.")?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE {
        return Err("The picture is too large or is not a regular image file.".into());
    }
    let key = fingerprint(&canonical, &metadata);
    let mut timings = Timings {
        lookup: start.elapsed(),
        ..Timings::default()
    };
    prepare_cache(root)?;
    let cached = root.join(format!("{key}.png"));
    if fs::symlink_metadata(&cached).is_ok_and(|metadata| metadata.file_type().is_file())
        && let Ok(image) = decode(&cached, true, &mut timings)
    {
        timings.cache_hit = true;
        return Ok(Pixels { image, timings });
    }
    let image = decode(&canonical, false, &mut timings)?;
    if fingerprint(
        &canonical,
        &fs::metadata(&canonical).map_err(|_| "The picture changed while loading.")?,
    ) != key
    {
        return Err("The picture changed while loading. Retry it.".into());
    }
    // Cache failure does not prevent a valid picture being shown.
    if let Err(error) = publish(root, &cached, &image) {
        log::debug!("gui_v2 thumbnail not cached: {error}");
    }
    Ok(Pixels { image, timings })
}

pub(super) fn decode(
    path: &Path,
    thumbnail: bool,
    timings: &mut Timings,
) -> Result<egui::ColorImage, String> {
    let start = Instant::now();
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "The image could not be opened.")?
        .take(MAX_SOURCE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The image could not be read.")?;
    if bytes.len() as u64 > MAX_SOURCE {
        return Err("The image exceeds the safe size limit.".into());
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "The image format is not recognised.")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(if thumbnail { WIDTH } else { 8192 });
    limits.max_image_height = Some(if thumbnail { HEIGHT } else { 8192 });
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| "The picture is broken, unsupported, or too large to decode safely.")?;
    timings.decode += start.elapsed();
    let start = Instant::now();
    let pixels = if thumbnail {
        decoded.to_rgba8()
    } else {
        decoded.thumbnail(WIDTH, HEIGHT).to_rgba8()
    };
    timings.resize += start.elapsed();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [pixels.width() as usize, pixels.height() as usize],
        pixels.as_raw(),
    ))
}

fn prepare_cache(root: &Path) -> Result<(), String> {
    if fs::symlink_metadata(root).is_ok_and(|meta| !meta.file_type().is_dir()) {
        return Err("The thumbnail cache is not a regular directory.".into());
    }
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    Ok(())
}

fn publish(root: &Path, destination: &Path, image: &egui::ColorImage) -> Result<(), String> {
    // Immutable keys: neither unrelated files nor another worker's result is overwritten.
    let pixels: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_srgba_unmultiplied())
        .collect();
    let buffer = image::RgbaImage::from_raw(image.size[0] as u32, image.size[1] as u32, pixels)
        .ok_or("Invalid thumbnail dimensions.")?;
    let mut encoded = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buffer)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(root).map_err(|error| error.to_string())?;
    temporary
        .write_all(encoded.get_ref())
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary
        .persist_noclobber(destination)
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Run once at index refresh, never once per image or frame. Only our exact
/// immutable key format is eligible; user files/symlinks are left untouched.
pub(super) fn trim_cache(root: &Path) {
    if !fs::symlink_metadata(root).is_ok_and(|meta| meta.file_type().is_dir()) {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".png")?;
            if stem.len() != 64
                || !stem.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !entry.file_type().ok()?.is_file()
            {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some((metadata.modified().ok(), metadata.len(), entry.path()))
        })
        .collect();
    files.sort_by_key(|entry| entry.0);
    let mut total: u64 = files.iter().map(|entry| entry.1).sum();
    for (_, size, path) in files {
        if total <= MAX_CACHE_BYTES {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}
