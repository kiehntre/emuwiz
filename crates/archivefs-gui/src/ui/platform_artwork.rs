//! Platform artwork registry - see docs/PLATFORM_ARTWORK.md.
//!
//! Entirely offline: every mapping here is a static, compile-time table
//! (no network access, no download, no runtime fetch of any kind - see
//! docs/PLATFORM_ARTWORK.md's "no-network guarantee"). Built-in artwork
//! includes exact hardware PNGs compiled into the executable plus the
//! original EmuWiz SVG/native-glyph fallback set. An explicitly
//! configured local PNG remains highest priority. SVG remains an
//! inspectable source asset for fallbacks, but is never parsed at runtime.
//!
//! Extracted verbatim from `main.rs` (2026-08-22, GUI extraction Phase A):
//! shared by Library row rendering, Gamer View, and the platform shelf -
//! never a Gamer-View-only concern, hence its own module rather than living
//! under `gamer_view`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui;

/// The category fallback set (docs/PLATFORM_ARTWORK.md): used whenever a
/// platform has no dedicated asset of its own. Deliberately small and
/// closed - "do not block the feature on creating a unique image for
/// every obscure platform."
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlatformAssetCategory {
    Console,
    Handheld,
    Computer,
    Arcade,
    OpticalDisc,
    Cartridge,
    Unknown,
}

impl PlatformAssetCategory {
    pub(crate) fn asset_id(self) -> &'static str {
        match self {
            Self::Console => "console",
            Self::Handheld => "handheld",
            Self::Computer => "computer",
            Self::Arcade => "arcade",
            Self::OpticalDisc => "optical-disc",
            Self::Cartridge => "cartridge",
            Self::Unknown => "unknown",
        }
    }

    /// Plain-language name, for the accessible label a screen reader
    /// announces alongside (never instead of) the visual glyph - "platform
    /// artwork must never be the only way to identify a platform."
    pub(crate) fn accessible_label(self) -> &'static str {
        match self {
            Self::Console => "console",
            Self::Handheld => "handheld device",
            Self::Computer => "computer",
            Self::Arcade => "arcade cabinet",
            Self::OpticalDisc => "optical-disc system",
            Self::Cartridge => "cartridge system",
            Self::Unknown => "unrecognised platform",
        }
    }
}

/// Every canonical platform name this build knows a *category* for -
/// deliberately not exhaustive (an unmapped platform simply falls back to
/// `Unknown`, which is a correct, honest answer, not a bug). Matches
/// against `canonical_platform_names()`'s exact spellings
/// (archivefs-core's `platform` registry) case-insensitively,
/// so a filter/search-derived platform string with different casing still
/// resolves correctly.
pub(crate) fn platform_asset_category(platform: &str) -> PlatformAssetCategory {
    let Some(platform) = canonical_platform_for_artwork(platform) else {
        return PlatformAssetCategory::Unknown;
    };
    match platform.id {
        "3DO" | "ColecoVision" | "Dreamcast" | "GameCube" | "Intellivision" | "MasterSystem"
        | "MegaDrive" | "N64" | "NeoGeo" | "NeoGeo64" | "NES" | "PSX" | "PS2" | "PS3"
        | "Saturn" | "Sega 32X" | "SNES" | "Switch" | "Vectrex" | "Wii" | "WiiU" | "Xbox"
        | "Xbox360" => PlatformAssetCategory::Console,
        "Atari Lynx"
        | "Game Boy"
        | "Game Boy Advance"
        | "Game Boy Color"
        | "GameGear"
        | "NGage"
        | "Neo Geo Pocket"
        | "Neo Geo Pocket Color"
        | "Nintendo 3DS"
        | "Nintendo DS"
        | "PlayStation Vita"
        | "PSP"
        | "Virtual Boy"
        | "WonderSwan"
        | "WonderSwan Color" => PlatformAssetCategory::Handheld,
        "Acorn Archimedes" | "Acorn Electron" | "Amiga" | "Amstrad CPC" | "Apple II"
        | "Atari 8-bit" | "AtariST" | "BBC Micro" | "Commodore 128" | "Commodore 64" | "DOS"
        | "FM Towns" | "Macintosh" | "MSX" | "MSX2" | "NEC PC-8801" | "NEC PC-9801" | "PC"
        | "PC-98" | "ScummVM" | "Sharp X68000" | "VIC-20" | "ZX Spectrum" => {
            PlatformAssetCategory::Computer
        }
        "Arcade" => PlatformAssetCategory::Arcade,
        "AmigaCD32" | "Commodore CDTV" | "Neo Geo CD" | "PC Engine CD" | "Philips CD-i"
        | "Sega CD" => PlatformAssetCategory::OpticalDisc,
        "Atari2600" | "Atari5200" | "Atari7800" | "Atari Jaguar" | "PC Engine"
        | "TurboGrafx-16" => PlatformAssetCategory::Cartridge,
        _ => PlatformAssetCategory::Unknown,
    }
}

/// Resolves through the one canonical platform registry. Exact persisted IDs,
/// exact display names and exact registered aliases are accepted; filenames
/// are never guessed from a display label and substring matching is forbidden.
pub(crate) fn canonical_platform_for_artwork(
    platform: &str,
) -> Option<&'static archivefs_core::platform::Platform> {
    archivefs_core::platform::platform_by_id(platform)
        .or_else(|| {
            archivefs_core::platform::PLATFORMS
                .iter()
                .find(|candidate| candidate.display_name.eq_ignore_ascii_case(platform))
        })
        .or_else(|| archivefs_core::platform::platform_for_alias(platform))
}

/// The stable filename stem for a persisted canonical platform identifier:
/// lowercase ASCII alphanumerics only. Display-name changes therefore cannot
/// rename artwork. Registry tests prove that all 74 current IDs remain unique
/// under this convention.
pub(crate) fn canonical_platform_asset_id(platform_id: &str) -> String {
    platform_id
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect()
}

/// The exact canonical artwork key. Known platforms keep their own key even
/// when no PNG is bundled, allowing `<canonical-id>.png` custom overrides;
/// rendering then falls back deterministically to the platform category.
/// The category glyph a platform falls back to when it has no artwork of its own.
///
/// One function on purpose. A game row and the featured panel beside it both need
/// this, and deriving it separately in each is how the two drift into showing a
/// different glyph for the same game.
pub(crate) fn platform_fallback_asset_id(platform: &str, unknown_platform: bool) -> &'static str {
    if unknown_platform {
        return PlatformAssetCategory::Unknown.asset_id();
    }
    platform_asset_category(platform).asset_id()
}

pub(crate) fn platform_asset_id(platform: &str, unknown_platform: bool) -> String {
    if unknown_platform {
        return PlatformAssetCategory::Unknown.asset_id().to_owned();
    }
    canonical_platform_for_artwork(platform)
        .map(|platform| canonical_platform_asset_id(platform.id))
        .unwrap_or_else(|| PlatformAssetCategory::Unknown.asset_id().to_owned())
}

#[derive(Clone, Copy)]
pub(crate) struct BundledPlatformArtwork {
    pub(crate) asset_id: &'static str,
    pub(crate) png: &'static [u8],
}

pub(crate) const BUNDLED_PLATFORM_ARTWORK: &[BundledPlatformArtwork] = &[
    BundledPlatformArtwork {
        asset_id: "3do",
        png: include_bytes!("../../assets/platforms/3do.png"),
    },
    BundledPlatformArtwork {
        asset_id: "acornarchimedes",
        png: include_bytes!("../../assets/platforms/acornarchimedes.png"),
    },
    BundledPlatformArtwork {
        asset_id: "acornelectron",
        png: include_bytes!("../../assets/platforms/acornelectron.png"),
    },
    BundledPlatformArtwork {
        asset_id: "amiga",
        png: include_bytes!("../../assets/platforms/amiga.png"),
    },
    BundledPlatformArtwork {
        asset_id: "amigacd32",
        png: include_bytes!("../../assets/platforms/amigacd32.png"),
    },
    BundledPlatformArtwork {
        asset_id: "amstradcpc",
        png: include_bytes!("../../assets/platforms/amstradcpc.png"),
    },
    BundledPlatformArtwork {
        asset_id: "appleii",
        png: include_bytes!("../../assets/platforms/appleii.png"),
    },
    BundledPlatformArtwork {
        asset_id: "arcade",
        png: include_bytes!("../../assets/platforms/arcade.png"),
    },
    BundledPlatformArtwork {
        asset_id: "atari2600",
        png: include_bytes!("../../assets/platforms/atari2600.png"),
    },
    BundledPlatformArtwork {
        asset_id: "atari5200",
        png: include_bytes!("../../assets/platforms/atari5200.png"),
    },
    BundledPlatformArtwork {
        asset_id: "atari7800",
        png: include_bytes!("../../assets/platforms/atari7800.png"),
    },
    BundledPlatformArtwork {
        asset_id: "atarijaguar",
        png: include_bytes!("../../assets/platforms/atarijaguar.png"),
    },
    BundledPlatformArtwork {
        asset_id: "atarilynx",
        png: include_bytes!("../../assets/platforms/atarilynx.png"),
    },
    BundledPlatformArtwork {
        asset_id: "atarist",
        png: include_bytes!("../../assets/platforms/atarist.png"),
    },
    BundledPlatformArtwork {
        asset_id: "bbcmicro",
        png: include_bytes!("../../assets/platforms/bbcmicro.png"),
    },
    BundledPlatformArtwork {
        asset_id: "colecovision",
        png: include_bytes!("../../assets/platforms/colecovision.png"),
    },
    BundledPlatformArtwork {
        asset_id: "commodore64",
        png: include_bytes!("../../assets/platforms/commodore64.png"),
    },
    BundledPlatformArtwork {
        asset_id: "dreamcast",
        png: include_bytes!("../../assets/platforms/dreamcast.png"),
    },
    BundledPlatformArtwork {
        asset_id: "gameboy",
        png: include_bytes!("../../assets/platforms/gameboy.png"),
    },
    BundledPlatformArtwork {
        asset_id: "gameboyadvance",
        png: include_bytes!("../../assets/platforms/gameboyadvance.png"),
    },
    BundledPlatformArtwork {
        asset_id: "gameboycolor",
        png: include_bytes!("../../assets/platforms/gameboycolor.png"),
    },
    BundledPlatformArtwork {
        asset_id: "gamecube",
        png: include_bytes!("../../assets/platforms/gamecube.png"),
    },
    BundledPlatformArtwork {
        asset_id: "gamegear",
        png: include_bytes!("../../assets/platforms/gamegear.png"),
    },
    BundledPlatformArtwork {
        asset_id: "megadrive",
        png: include_bytes!("../../assets/platforms/megadrive.png"),
    },
    BundledPlatformArtwork {
        asset_id: "n64",
        png: include_bytes!("../../assets/platforms/n64.png"),
    },
    BundledPlatformArtwork {
        asset_id: "neogeo",
        png: include_bytes!("../../assets/platforms/neogeo.png"),
    },
    BundledPlatformArtwork {
        asset_id: "neogeopocket",
        png: include_bytes!("../../assets/platforms/neogeopocket.png"),
    },
    BundledPlatformArtwork {
        asset_id: "nes",
        png: include_bytes!("../../assets/platforms/nes.png"),
    },
    BundledPlatformArtwork {
        asset_id: "nintendo3ds",
        png: include_bytes!("../../assets/platforms/nintendo3ds.png"),
    },
    BundledPlatformArtwork {
        asset_id: "philipscdi",
        png: include_bytes!("../../assets/platforms/philipscdi.png"),
    },
    BundledPlatformArtwork {
        asset_id: "playstationvita",
        png: include_bytes!("../../assets/platforms/playstationvita.png"),
    },
    BundledPlatformArtwork {
        asset_id: "ps2",
        png: include_bytes!("../../assets/platforms/ps2.png"),
    },
    BundledPlatformArtwork {
        asset_id: "ps3",
        png: include_bytes!("../../assets/platforms/ps3.png"),
    },
    BundledPlatformArtwork {
        asset_id: "psp",
        png: include_bytes!("../../assets/platforms/psp.png"),
    },
    BundledPlatformArtwork {
        asset_id: "psx",
        png: include_bytes!("../../assets/platforms/psx.png"),
    },
    BundledPlatformArtwork {
        asset_id: "saturn",
        png: include_bytes!("../../assets/platforms/saturn.png"),
    },
    BundledPlatformArtwork {
        asset_id: "scummvm",
        png: include_bytes!("../../assets/platforms/scummvm.png"),
    },
    BundledPlatformArtwork {
        asset_id: "sega32x",
        png: include_bytes!("../../assets/platforms/sega32x.png"),
    },
    BundledPlatformArtwork {
        asset_id: "sharpx68000",
        png: include_bytes!("../../assets/platforms/sharpx68000.png"),
    },
    BundledPlatformArtwork {
        asset_id: "snes",
        png: include_bytes!("../../assets/platforms/snes.png"),
    },
    BundledPlatformArtwork {
        asset_id: "switch",
        png: include_bytes!("../../assets/platforms/switch.png"),
    },
    BundledPlatformArtwork {
        asset_id: "turbografx16",
        png: include_bytes!("../../assets/platforms/turbografx16.png"),
    },
    BundledPlatformArtwork {
        asset_id: "vic20",
        png: include_bytes!("../../assets/platforms/vic20.png"),
    },
    BundledPlatformArtwork {
        asset_id: "virtualboy",
        png: include_bytes!("../../assets/platforms/virtualboy.png"),
    },
    BundledPlatformArtwork {
        asset_id: "wii",
        png: include_bytes!("../../assets/platforms/wii.png"),
    },
    BundledPlatformArtwork {
        asset_id: "wiiu",
        png: include_bytes!("../../assets/platforms/wiiu.png"),
    },
    BundledPlatformArtwork {
        asset_id: "wonderswancolor",
        png: include_bytes!("../../assets/platforms/wonderswancolor.png"),
    },
    BundledPlatformArtwork {
        asset_id: "xbox",
        png: include_bytes!("../../assets/platforms/xbox.png"),
    },
    BundledPlatformArtwork {
        asset_id: "xbox360",
        png: include_bytes!("../../assets/platforms/xbox360.png"),
    },
    BundledPlatformArtwork {
        asset_id: "zxspectrum",
        png: include_bytes!("../../assets/platforms/zxspectrum.png"),
    },
];

pub(crate) fn bundled_platform_artwork(asset_id: &str) -> Option<BundledPlatformArtwork> {
    BUNDLED_PLATFORM_ARTWORK
        .iter()
        .copied()
        .find(|artwork| artwork.asset_id == asset_id)
}

pub(crate) const MAX_CUSTOM_ARTWORK_FILE_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_CUSTOM_ARTWORK_DIMENSION: u32 = 1024;
pub(crate) const MAX_CUSTOM_ARTWORK_DECODE_BYTES: u64 =
    MAX_CUSTOM_ARTWORK_DIMENSION as u64 * MAX_CUSTOM_ARTWORK_DIMENSION as u64 * 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlatformArtworkFingerprint {
    pub(crate) path: PathBuf,
    pub(crate) length: u64,
    pub(crate) modified: SystemTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CustomArtworkLoadError {
    Missing,
    UnsupportedPath,
    Metadata,
    Oversized,
    Malformed,
}

pub(crate) struct CachedPlatformArtwork {
    pub(crate) fingerprint: PlatformArtworkFingerprint,
    pub(crate) texture: Option<egui::TextureHandle>,
}

/// Session-local decoded texture cache. It holds no bytes from remote
/// sources and performs no discovery: only the explicitly configured
/// directory and the closed set of registry asset ids are considered.
#[derive(Default)]
pub(crate) struct PlatformArtworkCache {
    pub(crate) directory: Option<PathBuf>,
    pub(crate) entries: HashMap<String, CachedPlatformArtwork>,
    pub(crate) bundled_entries: HashMap<&'static str, Option<egui::TextureHandle>>,
}

impl PlatformArtworkCache {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.bundled_entries.clear();
        self.directory = None;
    }

    pub(crate) fn custom_texture(
        &mut self,
        context: &egui::Context,
        directory: Option<&Path>,
        asset_id: &str,
    ) -> Option<PlatformArtworkTexture> {
        if self.directory.as_deref() != directory {
            self.entries.clear();
            self.directory = directory.map(Path::to_path_buf);
        }

        let fingerprint = match custom_platform_artwork_fingerprint(directory, asset_id) {
            Ok(fingerprint) => fingerprint,
            Err(_) => {
                self.entries.remove(asset_id);
                return None;
            }
        };
        if let Some(cached) = self.entries.get(asset_id)
            && cached.fingerprint == fingerprint
        {
            return cached.texture.as_ref().map(PlatformArtworkTexture::from);
        }

        let texture = decode_custom_platform_artwork(&fingerprint.path)
            .ok()
            .map(|image| {
                context.load_texture(
                    format!("archivefs-custom-platform-{asset_id}"),
                    image,
                    egui::TextureOptions::LINEAR,
                )
            });
        let texture_info = texture.as_ref().map(PlatformArtworkTexture::from);
        self.entries.insert(
            asset_id.to_string(),
            CachedPlatformArtwork {
                fingerprint,
                texture,
            },
        );
        texture_info
    }

    fn bundled_texture(
        &mut self,
        context: &egui::Context,
        asset_id: &str,
    ) -> Option<PlatformArtworkTexture> {
        let bundled = bundled_platform_artwork(asset_id)?;
        let texture = self
            .bundled_entries
            .entry(bundled.asset_id)
            .or_insert_with(|| {
                decode_bundled_platform_artwork(bundled.png)
                    .ok()
                    .map(|image| {
                        context.load_texture(
                            format!("archivefs-bundled-platform-{}", bundled.asset_id),
                            image,
                            egui::TextureOptions::LINEAR,
                        )
                    })
            });
        texture.as_ref().map(PlatformArtworkTexture::from)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlatformArtworkTexture {
    id: egui::TextureId,
    size: egui::Vec2,
}

impl From<&egui::TextureHandle> for PlatformArtworkTexture {
    fn from(texture: &egui::TextureHandle) -> Self {
        Self {
            id: texture.id(),
            size: texture.size_vec2(),
        }
    }
}

pub(crate) fn valid_platform_asset_id(asset_id: &str) -> bool {
    !asset_id.is_empty()
        && asset_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Resolves one supported custom PNG by its exact registry asset id. SVG
/// files are intentionally not accepted: rendering them safely would add
/// an independent XML/vector parser stack, while the existing `image`
/// dependency already provides bounded PNG decoding. Symlinks and other
/// non-regular files are rejected so the configured directory is the only
/// filesystem location this feature reads.
pub(crate) fn custom_platform_artwork_path(
    directory: Option<&Path>,
    asset_id: &str,
) -> Option<PathBuf> {
    if !valid_platform_asset_id(asset_id) {
        return None;
    }
    let directory = directory?;
    let candidate = directory.join(format!("{asset_id}.png"));
    let metadata = candidate.symlink_metadata().ok()?;
    (metadata.file_type().is_file() && !metadata.file_type().is_symlink()).then_some(candidate)
}

pub(crate) fn custom_platform_artwork_fingerprint(
    directory: Option<&Path>,
    asset_id: &str,
) -> Result<PlatformArtworkFingerprint, CustomArtworkLoadError> {
    let path =
        custom_platform_artwork_path(directory, asset_id).ok_or(CustomArtworkLoadError::Missing)?;
    let metadata = path
        .metadata()
        .map_err(|_| CustomArtworkLoadError::Metadata)?;
    if metadata.len() > MAX_CUSTOM_ARTWORK_FILE_BYTES {
        return Err(CustomArtworkLoadError::Oversized);
    }
    let modified = metadata
        .modified()
        .map_err(|_| CustomArtworkLoadError::Metadata)?;
    Ok(PlatformArtworkFingerprint {
        path,
        length: metadata.len(),
        modified,
    })
}

pub(crate) fn decode_custom_platform_artwork(
    path: &Path,
) -> Result<egui::ColorImage, CustomArtworkLoadError> {
    use image::ImageDecoder as _;

    if path.extension().and_then(|extension| extension.to_str()) != Some("png") {
        return Err(CustomArtworkLoadError::UnsupportedPath);
    }
    let metadata = path
        .metadata()
        .map_err(|_| CustomArtworkLoadError::Metadata)?;
    if metadata.len() > MAX_CUSTOM_ARTWORK_FILE_BYTES {
        return Err(CustomArtworkLoadError::Oversized);
    }
    let file = std::fs::File::open(path).map_err(|_| CustomArtworkLoadError::Metadata)?;
    let mut decoder = image::codecs::png::PngDecoder::new(std::io::BufReader::new(file))
        .map_err(|_| CustomArtworkLoadError::Malformed)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_CUSTOM_ARTWORK_DIMENSION);
    limits.max_image_height = Some(MAX_CUSTOM_ARTWORK_DIMENSION);
    limits.max_alloc = Some(MAX_CUSTOM_ARTWORK_DECODE_BYTES);
    decoder
        .set_limits(limits)
        .map_err(|_| CustomArtworkLoadError::Oversized)?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(CustomArtworkLoadError::Malformed);
    }
    let rgba = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| CustomArtworkLoadError::Malformed)?
        .into_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        rgba.as_raw(),
    ))
}

/// Decode immutable bytes compiled into the executable. This deliberately
/// accepts no path, so installed builds cannot depend on the source tree.
/// A corrupt embedded image is cached as a failure and the caller paints
/// the category glyph instead.
pub(crate) fn decode_bundled_platform_artwork(
    png: &'static [u8],
) -> Result<egui::ColorImage, CustomArtworkLoadError> {
    use image::ImageDecoder as _;

    let mut decoder = image::codecs::png::PngDecoder::new(std::io::Cursor::new(png))
        .map_err(|_| CustomArtworkLoadError::Malformed)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_CUSTOM_ARTWORK_DIMENSION);
    limits.max_image_height = Some(MAX_CUSTOM_ARTWORK_DIMENSION);
    limits.max_alloc = Some(MAX_CUSTOM_ARTWORK_DECODE_BYTES);
    decoder
        .set_limits(limits)
        .map_err(|_| CustomArtworkLoadError::Oversized)?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(CustomArtworkLoadError::Malformed);
    }
    let rgba = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| CustomArtworkLoadError::Malformed)?
        .into_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        rgba.as_raw(),
    ))
}

/// The allocation-free core of `paint_platform_glyph`, so a caller that
/// already owns a rect (e.g. drawn inside an existing `Button`'s bounds,
/// as `show_platform_shelf_item` does) never has to allocate a second,
/// unwanted region just to get a glyph drawn.
pub(crate) fn paint_platform_glyph_at(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    color: egui::Color32,
    asset_id: &str,
) {
    let stroke = egui::Stroke::new((size * 0.06).max(1.5), color);
    let r = size * 0.42;
    match asset_id {
        "gamecube" => {
            painter.circle_stroke(center, r, stroke);
            painter.line_segment(
                [center - egui::vec2(0.0, r), center + egui::vec2(0.0, r)],
                stroke,
            );
        }
        "playstation2" => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(size * 0.4, size * 0.8)),
                egui::CornerRadius::same((size * 0.08) as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        "xbox" => {
            painter.circle_stroke(center, r, stroke);
            let d = r * 0.7;
            painter.line_segment(
                [center - egui::vec2(d, d), center + egui::vec2(d, d)],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(-d, d), center + egui::vec2(d, -d)],
                stroke,
            );
        }
        "handheld" => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(size * 0.55, size * 0.85)),
                egui::CornerRadius::same((size * 0.1) as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.circle_filled(
                center + egui::vec2(0.0, size * 0.2),
                size * 0.05,
                stroke.color,
            );
        }
        "computer" => {
            painter.rect_stroke(
                egui::Rect::from_center_size(
                    center - egui::vec2(0.0, size * 0.08),
                    egui::vec2(size * 0.75, size * 0.5),
                ),
                egui::CornerRadius::same((size * 0.05) as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-size * 0.2, size * 0.42),
                    center + egui::vec2(size * 0.2, size * 0.42),
                ],
                stroke,
            );
        }
        "arcade" => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(size * 0.55, size * 0.8)),
                egui::CornerRadius::same((size * 0.04) as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.circle_filled(
                center + egui::vec2(0.0, size * 0.15),
                size * 0.05,
                stroke.color,
            );
        }
        "optical-disc" => {
            painter.circle_stroke(center, r, stroke);
            painter.circle_stroke(center, r * 0.3, stroke);
        }
        "cartridge" => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(size * 0.5, size * 0.7)),
                egui::CornerRadius::same((size * 0.05) as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        _ => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(size * 0.75, size * 0.5)),
                egui::CornerRadius::same((size * 0.1) as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
            if asset_id == "unknown" {
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    "?",
                    egui::FontId::proportional(size * 0.4),
                    stroke.color,
                );
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlatformArtworkSource {
    Custom,
    Bundled,
    Glyph,
}

pub(crate) fn fitted_artwork_rect(
    center: egui::Pos2,
    size: f32,
    texture_size: egui::Vec2,
) -> egui::Rect {
    let scale = size / texture_size.x.max(texture_size.y).max(1.0);
    egui::Rect::from_center_size(center, texture_size * scale)
}

pub(crate) fn paint_texture(
    ui: &egui::Ui,
    texture: PlatformArtworkTexture,
    center: egui::Pos2,
    size: f32,
) {
    ui.painter().image(
        texture.id,
        fitted_artwork_rect(center, size, texture.size),
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

/// Draws a RomM cover inside the row's artwork slot.
///
/// Reuses [`fitted_artwork_rect`], so a 2:3 cover and a square platform icon
/// occupy the same box and a row's height never depends on which one it got.
pub(crate) fn paint_cover_fitted(
    ui: &egui::Ui,
    texture: &egui::TextureHandle,
    center: egui::Pos2,
    size: f32,
) {
    ui.painter().image(
        texture.id(),
        fitted_artwork_rect(center, size, texture.size_vec2()),
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

pub(crate) struct PlatformArtworkPaint<'a> {
    pub(crate) center: egui::Pos2,
    pub(crate) size: f32,
    pub(crate) color: egui::Color32,
    pub(crate) asset_id: &'a str,
    pub(crate) fallback_asset_id: &'a str,
}

pub(crate) fn paint_platform_artwork_at(
    ui: &egui::Ui,
    artwork_cache: &mut PlatformArtworkCache,
    artwork_directory: Option<&Path>,
    paint: PlatformArtworkPaint<'_>,
) -> PlatformArtworkSource {
    if let Some(texture) = artwork_cache.custom_texture(ui.ctx(), artwork_directory, paint.asset_id)
    {
        paint_texture(ui, texture, paint.center, paint.size);
        return PlatformArtworkSource::Custom;
    }
    if let Some(texture) = artwork_cache.bundled_texture(ui.ctx(), paint.asset_id) {
        paint_texture(ui, texture, paint.center, paint.size);
        return PlatformArtworkSource::Bundled;
    }
    // A category-level custom image remains a supported intentional fallback
    // for canonical platforms without dedicated artwork. It is consulted only
    // after the exact canonical filename and can therefore never mask it.
    if paint.fallback_asset_id != paint.asset_id
        && let Some(texture) =
            artwork_cache.custom_texture(ui.ctx(), artwork_directory, paint.fallback_asset_id)
    {
        paint_texture(ui, texture, paint.center, paint.size);
        return PlatformArtworkSource::Custom;
    }
    paint_platform_glyph_at(
        ui.painter(),
        paint.center,
        paint.size,
        paint.color,
        paint.fallback_asset_id,
    );
    PlatformArtworkSource::Glyph
}

/// Stable, path-safe local artwork key for a game. A user can place
/// `game-<normalised-title>.png` beside custom platform overrides; it is
/// decoded through the same bounded PNG-only cache and safety checks.
pub(crate) fn game_artwork_asset_id(title: &str) -> String {
    let mut id = String::from("game-");
    let mut separator_pending = false;
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if separator_pending && !id.ends_with('-') {
                id.push('-');
            }
            id.push(character);
            separator_pending = false;
        } else {
            separator_pending = true;
        }
        if id.len() >= 85 {
            break;
        }
    }
    while id.ends_with('-') {
        id.pop();
    }
    if id == "game" {
        id.push_str("-unknown");
    }
    id
}

pub(crate) struct GameRowArtworkPaint<'a> {
    pub(crate) center: egui::Pos2,
    pub(crate) size: f32,
    pub(crate) title: &'a str,
    pub(crate) platform_asset: &'a str,
    pub(crate) platform_fallback: &'a str,
}

/// Game rows prefer a safe local per-game PNG, then the same exact
/// platform artwork used by the shelf, then the generic Unknown glyph.
/// Both successful decodes and failures are session-cached.
pub(crate) fn paint_game_row_artwork(
    ui: &egui::Ui,
    artwork_cache: &mut PlatformArtworkCache,
    artwork_directory: Option<&Path>,
    paint: GameRowArtworkPaint<'_>,
) -> PlatformArtworkSource {
    let game_asset_id = game_artwork_asset_id(paint.title);
    if let Some(texture) = artwork_cache.custom_texture(ui.ctx(), artwork_directory, &game_asset_id)
    {
        paint_texture(ui, texture, paint.center, paint.size);
        return PlatformArtworkSource::Custom;
    }
    paint_platform_artwork_at(
        ui,
        artwork_cache,
        artwork_directory,
        PlatformArtworkPaint {
            center: paint.center,
            size: paint.size,
            color: ui.visuals().text_color().gamma_multiply(0.8),
            asset_id: paint.platform_asset,
            fallback_asset_id: paint.platform_fallback,
        },
    )
}
