use async_trait::async_trait;

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const NAPIPROJEKT_API: &str = "https://napiprojekt.pl/unit_napisy/dl.php";
const STAGING_ROOT: &str = "/tmp/subtitle-aggregator";

/// Compute NapiProjekt subhash from MD5 hash string.
/// Python reference:
///   idx = [0xe, 0x3, 0x6, 0x8, 0x2]
///   mul = [2, 2, 5, 4, 3]
///   add = [0, 0xd, 0x10, 0xb, 0x5]
fn get_subhash(hash: &str) -> String {
    let idx: [usize; 5] = [0xe, 0x3, 0x6, 0x8, 0x2];
    let mul: [u32; 5] = [2, 2, 5, 4, 3];
    let add: [u32; 5] = [0, 0xd, 0x10, 0xb, 0x5];

    let chars: Vec<char> = hash.chars().collect();
    let mut result = String::new();
    for i in 0..5 {
        let c = chars[idx[i]];
        let digit = u32::from_str_radix(&c.to_string(), 16).unwrap_or(0);
        let val = ((digit + add[i]) * mul[i]) % 0x10;
        result.push_str(&format!("{val:x}"));
    }
    result
}

pub struct NapiprojektProvider;

impl NapiprojektProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SubtitleProvider for NapiprojektProvider {
    fn name(&self) -> &str {
        "napiprojekt"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let hash = request
            .file_hash
            .as_deref()
            .ok_or("napiprojekt: file_hash is required")?;

        let subhash = get_subhash(hash);

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("napiprojekt: failed to build client: {e}"))?;

        let response = client
            .get(NAPIPROJEKT_API)
            .query(&[
                ("v", "dreambox"),
                ("kolejka", "false"),
                ("nick", ""),
                ("pass", ""),
                ("napios", "Linux"),
                ("l", "PL"),
                ("f", hash),
                ("t", subhash.as_str()),
            ])
            .send()
            .await
            .map_err(|e| format!("napiprojekt: request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "napiprojekt: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("napiprojekt: failed to read response: {e}"))?;

        if content.starts_with(b"NPc0") {
            tracing::info!("napiprojekt: no subtitle found for hash {hash}");
            return Ok(vec![]);
        }

        let name = request
            .query
            .clone()
            .unwrap_or_else(|| format!("napiprojekt_{hash}"));

        let result = SubtitleSearchResult {
            id: hash.to_string(),
            name: format!("{name}.zip"),
            language: "pl".into(),
            language_name: "Polish".into(),
            format: "srt".into(),
            provider: "napiprojekt".into(),
            detail_path: None,
            download_path: Some(hash.to_string()),
            download_count: None,
            rating: None,
            movie_name: Some(name),
            release_group: None,
        };

        Ok(vec![result])
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let hash = request
            .download_path
            .as_deref()
            .or(Some(request.subtitle_id.as_str()))
            .ok_or("napiprojekt: download_path (hash) is required")?;

        let subhash = get_subhash(hash);

        let client = reqwest::Client::builder()
            .user_agent("subtitle-aggregator/0.1")
            .build()
            .map_err(|e| format!("napiprojekt: failed to build client: {e}"))?;

        let response = client
            .get(NAPIPROJEKT_API)
            .query(&[
                ("v", "dreambox"),
                ("kolejka", "false"),
                ("nick", ""),
                ("pass", ""),
                ("napios", "Linux"),
                ("l", "PL"),
                ("f", hash),
                ("t", subhash.as_str()),
            ])
            .send()
            .await
            .map_err(|e| format!("napiprojekt: download request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "napiprojekt: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("napiprojekt: failed to read content: {e}"))?;

        if content.starts_with(b"NPc0") {
            return Err("napiprojekt: no subtitle found".into());
        }

        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("napiprojekt_{}.srt", hash));

        // Check if content looks like a zip archive
        if content.starts_with(b"PK") {
            let staging = std::path::Path::new(STAGING_ROOT);
            return crate::archive::extract_archive(
                &content,
                &format!("{name}.zip"),
                &request.language,
                staging,
            )
            .await;
        }

        Ok(DownloadedSubtitle {
            name,
            format: "srt".into(),
            content: content.to_vec(),
        })
    }
}
