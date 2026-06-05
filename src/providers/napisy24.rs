use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const NAPISY24_API: &str = "http://napisy24.pl/run/CheckSubAgent.php";
const STAGING_ROOT: &str = "/tmp/subtitle-aggregator";

pub struct Napisy24Provider {
    username: String,
    password: String,
}

impl Default for Napisy24Provider {
    fn default() -> Self {
        Self::new()
    }
}

impl Napisy24Provider {
    pub fn new() -> Self {
        let username = std::env::var("NAPISY24_USER").unwrap_or_else(|_| "subliminal".into());
        let password = std::env::var("NAPISY24_PASS").unwrap_or_else(|_| "lanimilbus".into());
        Self { username, password }
    }
}

#[async_trait]
impl SubtitleProvider for Napisy24Provider {
    fn name(&self) -> &str {
        "napisy24"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let file_hash = request
            .file_hash
            .as_deref()
            .ok_or("napisy24: file_hash is required")?;
        let file_size = request.file_size.ok_or("napisy24: file_size is required")?;
        let filename = request
            .query
            .clone()
            .unwrap_or_else(|| "video.mkv".to_string());

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("napisy24: failed to build client: {e}"))?;

        let file_size_str = file_size.to_string();
        let params = [
            ("postAction", "CheckSub"),
            ("ua", self.username.as_str()),
            ("ap", self.password.as_str()),
            ("fs", file_size_str.as_str()),
            ("fh", file_hash),
            ("fn", filename.as_str()),
            ("n24pref", "1"),
        ];

        let response = client
            .post(NAPISY24_API)
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("napisy24: request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!("napisy24: HTTP {}", response.status().as_u16()));
        }

        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| format!("napisy24: failed to read response: {e}"))?;

        // Response is split by `||`: first part is status/info, second is zip content
        let separator = b"||";
        let split_pos = body_bytes
            .windows(2)
            .position(|w| w == separator)
            .ok_or("napisy24: unexpected response format (missing ||)")?;

        let status_part = &body_bytes[..split_pos];
        let zip_part = &body_bytes[split_pos + 2..];

        let status_str = String::from_utf8_lossy(status_part);
        let status_str = status_str.trim();

        tracing::info!("napisy24: status = {status_str}");

        // Status codes: OK-0 = not found, OK-1 = video info only, OK-2 = found, OK-3 = not from db
        if status_str.starts_with("OK-0") {
            return Ok(vec![]);
        }
        if !status_str.starts_with("OK-2") {
            tracing::info!("napisy24: no subtitle in response (status: {status_str})");
            return Ok(vec![]);
        }

        if zip_part.is_empty() {
            return Ok(vec![]);
        }

        // Parse subtitle info from `|` separated key:value pairs
        let mut napis_id = String::new();
        let mut imdb_id = String::new();
        for part in status_str.split('|') {
            if let Some(val) = part.strip_prefix("napisId:") {
                napis_id = val.trim().to_string();
            } else if let Some(val) = part.strip_prefix("imdb:") {
                imdb_id = val.trim().to_string();
            }
        }

        let sub_id = if !napis_id.is_empty() {
            napis_id.clone()
        } else {
            file_hash.to_string()
        };

        // Encode zip bytes as base64 for storage in download_path
        let zip_b64 = general_purpose::STANDARD.encode(zip_part);

        let result = SubtitleSearchResult {
            id: sub_id.clone(),
            name: format!("napisy24_{sub_id}.zip"),
            language: "pl".into(),
            language_name: "Polish".into(),
            format: "srt".into(),
            provider: "napisy24".into(),
            detail_path: if !imdb_id.is_empty() {
                Some(imdb_id)
            } else {
                None
            },
            download_path: Some(zip_b64),
            download_count: None,
            rating: None,
            movie_name: Some(filename),
            release_group: None,
        };

        Ok(vec![result])
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let zip_b64 = request
            .download_path
            .as_deref()
            .ok_or("napisy24: download_path (base64 zip) is required")?;

        let zip_bytes = general_purpose::STANDARD
            .decode(zip_b64)
            .map_err(|e| format!("napisy24: failed to decode base64: {e}"))?;

        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("napisy24_{}.srt", request.subtitle_id));

        let staging = std::path::Path::new(STAGING_ROOT);
        crate::archive::extract_archive(
            &zip_bytes,
            &format!("{name}.zip"),
            &request.language,
            staging,
        )
        .await
    }
}
