//! Optional, read-only RetroAchievements game metadata.
//!
//! This module deliberately has no effect on identity, launch, or DAT
//! decisions.  A caller supplies an explicitly resolved RetroAchievements
//! game id (normally from a future hash-matching seam), and this module only
//! parses and serves bounded public metadata.  The cache is provider-owned
//! derived data and is published atomically.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const CACHE_FORMAT_VERSION: u32 = 1;
pub const MAX_CACHE_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_ACHIEVEMENTS: usize = 2_000;
pub const MAX_TEXT_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RetroAchievementsGameId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetroAchievementsAchievement {
    pub id: u32,
    pub title: String,
    pub description: Option<String>,
    pub points: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetroAchievementsGame {
    pub game_id: RetroAchievementsGameId,
    pub title: String,
    pub console_id: u32,
    pub achievement_count: u32,
    pub total_points: u32,
    pub badge_url: Option<String>,
    #[serde(default)]
    pub achievements: Vec<RetroAchievementsAchievement>,
    pub fetched_at_unix_seconds: i64,
    pub provenance: String,
    /// Local EmuWiz path used only as a cache join key; it is not sent to RA.
    pub archivefs_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CacheFile {
    format_version: u32,
    entries: Vec<RetroAchievementsGame>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Malformed(String),
    TooLarge,
    TooManyAchievements,
    InvalidField(&'static str),
}

/// Parse the public `API_GetGame` response.  The endpoint requires an API key;
/// callers must obtain it through their configured secret store and never put
/// it in a cache or log.  Unknown fields are ignored for forward compatibility.
pub fn parse_game_response(
    bytes: &[u8],
    archivefs_path: &Path,
    fetched_at_unix_seconds: i64,
) -> Result<RetroAchievementsGame, ParseError> {
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return Err(ParseError::TooLarge);
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| ParseError::Malformed(e.to_string()))?;
    let object = value
        .as_object()
        .ok_or_else(|| ParseError::Malformed("game response is not an object".into()))?;
    let game_id = object
        .get("ID")
        .and_then(number)
        .ok_or(ParseError::InvalidField("ID"))?;
    let title = object
        .get("Title")
        .and_then(text)
        .ok_or(ParseError::InvalidField("Title"))?;
    let console_id = object
        .get("ConsoleID")
        .and_then(number)
        .ok_or(ParseError::InvalidField("ConsoleID"))?;
    let achievements: Vec<RetroAchievementsAchievement> = object
        .get("Achievements")
        .and_then(|v| v.as_object())
        .map(|map| {
            if map.len() > MAX_ACHIEVEMENTS {
                return Err(ParseError::TooManyAchievements);
            }
            map.values().map(parse_achievement).collect()
        })
        .transpose()?
        .unwrap_or_default();
    let achievement_count = object
        .get("NumAchievements")
        .and_then(number)
        .unwrap_or(achievements.len() as u32);
    let total_points = object
        .get(" points")
        .and_then(number)
        .or_else(|| object.get("Points").and_then(number))
        .unwrap_or_else(|| achievements.iter().map(|a| a.points).sum());
    Ok(RetroAchievementsGame {
        game_id: RetroAchievementsGameId(game_id),
        title,
        console_id,
        achievement_count,
        total_points,
        badge_url: object.get("ImageIcon").and_then(text),
        achievements,
        fetched_at_unix_seconds,
        provenance: "RetroAchievements".into(),
        archivefs_path: archivefs_path.display().to_string(),
    })
}

fn parse_achievement(
    value: &serde_json::Value,
) -> Result<RetroAchievementsAchievement, ParseError> {
    let o = value
        .as_object()
        .ok_or_else(|| ParseError::Malformed("achievement is not an object".into()))?;
    Ok(RetroAchievementsAchievement {
        id: o
            .get("ID")
            .and_then(number)
            .ok_or(ParseError::InvalidField("achievement ID"))?,
        title: o
            .get("Title")
            .and_then(text)
            .ok_or(ParseError::InvalidField("achievement Title"))?,
        description: o.get("Description").and_then(text),
        points: o
            .get("Points")
            .and_then(number)
            .ok_or(ParseError::InvalidField("achievement Points"))?,
    })
}

fn number(value: &serde_json::Value) -> Option<u32> {
    value.as_u64().and_then(|n| u32::try_from(n).ok())
}
fn text(value: &serde_json::Value) -> Option<String> {
    let text = value.as_str()?.trim();
    (!text.is_empty() && text.len() <= MAX_TEXT_BYTES).then(|| text.to_string())
}

pub fn cache_path(data_root: &Path) -> PathBuf {
    data_root.join("retroachievements").join("games.json")
}

/// Official public game-info endpoint. The API key is deliberately accepted
/// separately by the caller and is never interpolated into logs or cache data.
pub fn game_api_url(game_id: RetroAchievementsGameId) -> String {
    format!(
        "https://retroachievements.org/API/API_GetGame.php?i={}",
        game_id.0
    )
}

/// Optional explicit configuration seam. Environment lookup is kept here only
/// as a bridge for callers that already have a secret manager; no key is ever
/// returned in diagnostics or persisted by this module.
pub fn configured_api_key() -> Option<String> {
    std::env::var("RETROACHIEVEMENTS_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
}

/// Joins a cache entry only to the exact local path that was explicitly
/// associated with its provider game id. Titles are never used as a match.
pub fn cached_for_path<'a>(
    entries: &'a [RetroAchievementsGame],
    path: &Path,
) -> Option<&'a RetroAchievementsGame> {
    let wanted = path.display().to_string();
    entries.iter().find(|entry| entry.archivefs_path == wanted)
}

pub fn load_cache(path: &Path) -> Result<Vec<RetroAchievementsGame>, ParseError> {
    let meta = fs::metadata(path).map_err(|e| ParseError::Malformed(e.to_string()))?;
    if meta.len() > MAX_CACHE_BYTES {
        return Err(ParseError::TooLarge);
    }
    let bytes = fs::read(path).map_err(|e| ParseError::Malformed(e.to_string()))?;
    let cache: CacheFile =
        serde_json::from_slice(&bytes).map_err(|e| ParseError::Malformed(e.to_string()))?;
    if cache.format_version != CACHE_FORMAT_VERSION || cache.entries.len() > MAX_ACHIEVEMENTS {
        return Err(ParseError::Malformed(
            "unsupported cache format or size".into(),
        ));
    }
    Ok(cache.entries)
}

pub fn publish_cache(path: &Path, entries: &[RetroAchievementsGame]) -> Result<(), ParseError> {
    if entries.len() > MAX_ACHIEVEMENTS {
        return Err(ParseError::TooManyAchievements);
    }
    let bytes = serde_json::to_vec_pretty(&CacheFile {
        format_version: CACHE_FORMAT_VERSION,
        entries: entries.to_vec(),
    })
    .map_err(|e| ParseError::Malformed(e.to_string()))?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return Err(ParseError::TooLarge);
    }
    let parent = path
        .parent()
        .ok_or_else(|| ParseError::Malformed("cache has no parent".into()))?;
    fs::create_dir_all(parent).map_err(|e| ParseError::Malformed(e.to_string()))?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, bytes).map_err(|e| ParseError::Malformed(e.to_string()))?;
    fs::rename(&tmp, path).map_err(|e| ParseError::Malformed(e.to_string()))
}

pub fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_bounded_public_game_metadata() {
        let json = br#"{"ID":42,"Title":"Example","ConsoleID":7,"NumAchievements":1,"Points":10,"ImageIcon":"/icon.png","Achievements":{"1":{"ID":1,"Title":"Start","Description":"Win","Points":10}}}"#;
        let game = parse_game_response(json, Path::new("/roms/example.zip"), 1).unwrap();
        assert_eq!(game.game_id, RetroAchievementsGameId(42));
        assert_eq!(game.achievement_count, 1);
        assert_eq!(game.total_points, 10);
        assert_eq!(game.provenance, "RetroAchievements");
    }
    #[test]
    fn malformed_response_fails_closed() {
        assert!(parse_game_response(br#"[]"#, Path::new("x"), 0).is_err());
    }
    #[test]
    fn cache_round_trip_is_atomic_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path());
        let game = parse_game_response(br#"{"ID":1,"Title":"X","ConsoleID":1}"#, Path::new("x"), 0)
            .unwrap();
        publish_cache(&path, &[game.clone()]).unwrap();
        assert_eq!(load_cache(&path).unwrap(), vec![game]);
    }
}
