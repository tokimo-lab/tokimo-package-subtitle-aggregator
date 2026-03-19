use serde::{Deserialize, Serialize};

// ── Search Request ──
// 与 rust-online-media-ingest/models.rs 和 @acme/types SubtitleSearchInput 保持一致
// 额外新增 file_hash / file_size 字段支持射手网等基于哈希的 provider

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleSearchRequest {
    pub query: Option<String>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<String>,
    pub languages: Option<Vec<String>>,
    /// 文件哈希 (射手网等 hash-based provider 使用)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub file_hash: Option<String>,
    /// 文件大小 bytes
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub file_size: Option<u64>,
}

// ── Search Result ──
// 与 rust-online-media-ingest/models.rs SubtitleSearchResult 完全一致

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleSearchResult {
    pub id: String,
    pub name: String,
    pub language: String,
    pub language_name: String,
    pub format: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movie_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_group: Option<String>,
}

// ── Download Request ──
// 与 rust-online-media-ingest/models.rs SubtitleDownloadRequest 一致
// 额外新增 provider 字段用于路由到对应 provider, 默认 "assrt" 向后兼容

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleDownloadRequest {
    pub subtitle_id: String,
    pub detail_path: Option<String>,
    pub download_path: Option<String>,
    pub language: String,
    pub format: String,
    pub name: Option<String>,
    /// 下载时路由到哪个 provider, 默认 "assrt" 向后兼容
    #[serde(default = "default_provider")]
    pub provider: String,
}

fn default_provider() -> String {
    "assrt".into()
}

// ── Download Response ──
// 与 rust-online-media-ingest/models.rs SubtitleDownloadResponse 完全一致

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleDownloadResponse {
    pub name: String,
    pub format: String,
    pub content_base64: String,
}

// ── Internal downloaded subtitle ──

#[derive(Debug, Clone)]
pub struct DownloadedSubtitle {
    pub name: String,
    pub format: String,
    pub content: Vec<u8>,
}

// ── Helper functions ──

pub fn normalize_language(language_text: &str) -> String {
    let normalized = language_text.split_whitespace().collect::<String>();
    if normalized.contains('简') {
        return "zh-CN".into();
    }
    if normalized.contains('繁') {
        return "zh-TW".into();
    }
    if normalized.contains("双语") || normalized.contains("雙語") {
        return "zh".into();
    }
    if normalized.contains('英') || normalized.to_ascii_lowercase().contains("english") {
        return "en".into();
    }
    if normalized.contains('日') || normalized.to_ascii_lowercase().contains("japanese") {
        return "ja".into();
    }
    if normalized.contains('韩') || normalized.contains('韓') {
        return "ko".into();
    }
    if normalized.to_ascii_lowercase().contains("chinese") {
        return "zh".into();
    }
    "zh".into()
}

pub fn normalize_format(format_text: &str) -> Option<String> {
    let lowered = format_text.trim().to_ascii_lowercase();
    if lowered.is_empty() {
        return None;
    }
    if lowered.contains("subrip") || lowered == "srt" {
        return Some("srt".into());
    }
    if lowered.contains("advanced substation alpha") || lowered == "ass" {
        return Some("ass".into());
    }
    if lowered.contains("substation alpha") || lowered == "ssa" {
        return Some("ssa".into());
    }
    if lowered.contains("webvtt") || lowered == "vtt" {
        return Some("vtt".into());
    }

    match lowered.rsplit('.').next() {
        Some("srt") => Some("srt".into()),
        Some("ass") => Some("ass".into()),
        Some("ssa") => Some("ssa".into()),
        Some("vtt") => Some("vtt".into()),
        _ => None,
    }
}

pub fn matches_preferred_language(language: &str, preferred_languages: Option<&[String]>) -> bool {
    let Some(preferred_languages) = preferred_languages else {
        return true;
    };
    if preferred_languages.is_empty() {
        return true;
    }
    if preferred_languages
        .iter()
        .any(|preferred| preferred == language)
    {
        return true;
    }
    if language == "zh" {
        return preferred_languages
            .iter()
            .any(|preferred| preferred.starts_with("zh"));
    }
    if language.starts_with("zh-") {
        return preferred_languages
            .iter()
            .any(|preferred| preferred == "zh");
    }
    false
}

pub fn score_subtitle_name(name: &str, format: &str, preferred_language: &str) -> i32 {
    let lower = name.to_ascii_lowercase();
    let mut score = 0;

    if preferred_language == "zh-CN" {
        if lower.contains("chs") || name.contains('简') {
            score += 40;
        }
        if lower.contains("eng") || name.contains("双语") || name.contains("雙語") {
            score += 10;
        }
    } else if preferred_language == "zh-TW" {
        if lower.contains("cht") || name.contains('繁') {
            score += 40;
        }
        if lower.contains("eng") || name.contains("双语") || name.contains("雙語") {
            score += 10;
        }
    } else if preferred_language == "en" {
        if lower.contains("eng") || name.contains("英文") {
            score += 40;
        }
    } else if preferred_language == "ja" {
        if lower.contains("jpn") || name.contains('日') {
            score += 40;
        }
    } else if preferred_language == "ko" {
        if lower.contains("kor") || name.contains('韩') || name.contains('韓') {
            score += 40;
        }
    } else if lower.contains("chs")
        || lower.contains("cht")
        || name.contains("双语")
        || name.contains("雙語")
    {
        score += 20;
    }

    score
        + match format {
            "srt" => 8,
            "ass" => 6,
            "ssa" => 4,
            "vtt" => 2,
            _ => 0,
        }
}
