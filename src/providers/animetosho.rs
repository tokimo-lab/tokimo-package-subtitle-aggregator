use async_trait::async_trait;
use serde::Deserialize;
use std::io::Read;
use xz2::read::XzDecoder;

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const FEED_API_URL: &str = "https://feed.animetosho.org/json";
const STORAGE_DOWNLOAD_BASE: &str = "https://animetosho.org/storage/attach/";

/// Maximum number of entries to inspect per search to avoid excessive API calls.
const SEARCH_ENTRY_LIMIT: usize = 10;

// ── AnimeTosho feed API response types ──

#[derive(Debug, Deserialize)]
struct EntryItem {
    id: u64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct TorrentDetails {
    #[serde(default)]
    files: Vec<TorrentFile>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TorrentFile {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    attachments: Vec<Attachment>,
}

#[derive(Debug, Deserialize)]
struct Attachment {
    id: u64,
    #[serde(rename = "type")]
    attachment_type: String,
    info: AttachmentInfo,
}

#[derive(Debug, Deserialize)]
struct AttachmentInfo {
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

// ── Language helpers ──

/// Convert an ISO 639-2/B (alpha-3) language code to ISO 639-1 (alpha-2).
fn alpha3b_to_alpha2(code: &str) -> &str {
    match code {
        "ara" => "ar",
        "deu" | "ger" => "de",
        "eng" => "en",
        "fin" => "fi",
        "fra" | "fre" => "fr",
        "heb" => "he",
        "ind" => "id",
        "ita" => "it",
        "jpn" => "ja",
        "pol" => "pl",
        "por" => "pt",
        "rus" => "ru",
        "spa" => "es",
        "swe" => "sv",
        "tha" => "th",
        "tur" => "tr",
        "vie" => "vi",
        "zho" | "chi" => "zh",
        other => other,
    }
}

fn language_name(code: &str) -> &str {
    match code {
        "ar" => "Arabic",
        "de" => "German",
        "en" => "English",
        "fi" => "Finnish",
        "fr" => "French",
        "he" => "Hebrew",
        "id" => "Indonesian",
        "it" => "Italian",
        "ja" => "Japanese",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "pt-BR" => "Portuguese (Brazil)",
        "ru" => "Russian",
        "es" => "Spanish",
        "sv" => "Swedish",
        "th" => "Thai",
        "tr" => "Turkish",
        "vi" => "Vietnamese",
        "zh" => "Chinese",
        _ => "Unknown",
    }
}

/// Derive the subtitle format from a filename extension.
fn format_from_filename(name: &str) -> String {
    match name
        .rsplit('.')
        .next()
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("srt") => "srt".into(),
        Some("ass") => "ass".into(),
        Some("ssa") => "ssa".into(),
        Some("vtt") => "vtt".into(),
        Some("sub") => "sub".into(),
        _ => "srt".into(),
    }
}

// ── Provider ──

pub struct AnimeToshoProvider;

impl AnimeToshoProvider {
    pub fn new() -> Self {
        Self
    }

    fn build_client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {e}"))
    }

    /// Fetch the list of NZB/torrent entries matching a title query.
    async fn fetch_entries(
        client: &reqwest::Client,
        query: &str,
    ) -> Result<Vec<EntryItem>, String> {
        let response = client
            .get(FEED_API_URL)
            .query(&[("q", query)])
            .send()
            .await
            .map_err(|e| format!("AnimeTosho search request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "AnimeTosho search failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        let entries: Vec<EntryItem> = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse AnimeTosho search response: {e}"))?;

        Ok(entries)
    }

    /// Fetch subtitle attachments for a single entry (torrent/NZB) by its AnimeTosho ID.
    async fn fetch_torrent_details(
        client: &reqwest::Client,
        entry_id: u64,
    ) -> Result<TorrentDetails, String> {
        let id_str = entry_id.to_string();
        let response = client
            .get(FEED_API_URL)
            .query(&[("show", "torrent"), ("id", id_str.as_str())])
            .send()
            .await
            .map_err(|e| format!("AnimeTosho torrent detail request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "AnimeTosho torrent detail failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        let details: TorrentDetails = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse AnimeTosho torrent details: {e}"))?;

        Ok(details)
    }

    /// Construct the download URL for an attachment stored as an xz-compressed file.
    fn attachment_download_url(attachment_id: u64) -> String {
        // AnimeTosho stores attachments at:
        //   /storage/attach/{hex_id_zero_padded_8}/{attachment_id}.xz
        let hex_id = format!("{:08x}", attachment_id);
        format!("{}{}/{}.xz", STORAGE_DOWNLOAD_BASE, hex_id, attachment_id)
    }

    /// Decompress an xz-encoded byte slice.
    fn decompress_xz(data: &[u8]) -> Result<Vec<u8>, String> {
        let mut decoder = XzDecoder::new(data);
        let mut output = Vec::new();
        decoder
            .read_to_end(&mut output)
            .map_err(|e| format!("Failed to decompress xz subtitle: {e}"))?;
        Ok(output)
    }
}

#[async_trait]
impl SubtitleProvider for AnimeToshoProvider {
    fn name(&self) -> &str {
        "animetosho"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .ok_or("AnimeTosho search requires a title query")?;

        let client = Self::build_client()?;

        let entries = Self::fetch_entries(&client, query).await?;

        // Only consider complete entries, most recent first, up to the limit.
        let mut complete_entries: Vec<EntryItem> = entries
            .into_iter()
            .filter(|e| e.status.as_deref().map_or(true, |s| s == "complete"))
            .collect();
        complete_entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        let entries_to_process = complete_entries
            .into_iter()
            .take(SEARCH_ENTRY_LIMIT)
            .collect::<Vec<_>>();

        let mut results = Vec::new();

        for entry in entries_to_process {
            let entry_title = entry.title.clone().unwrap_or_default();

            let details = match Self::fetch_torrent_details(&client, entry.id).await {
                Ok(d) => d,
                Err(_) => continue,
            };

            for file in details.files {
                let file_name = file.name.clone().unwrap_or_default();

                for attachment in file.attachments {
                    if attachment.attachment_type != "subtitle" {
                        continue;
                    }

                    let alpha3 = attachment.info.lang.as_deref().unwrap_or("eng");
                    let mut lang_code = alpha3b_to_alpha2(alpha3).to_string();

                    // Detect Brazilian Portuguese from the attachment name
                    if lang_code == "pt" {
                        if let Some(ref att_name) = attachment.info.name {
                            if att_name.to_ascii_lowercase().contains("brazil")
                                || att_name.to_ascii_lowercase().contains("br")
                            {
                                lang_code = "pt-BR".into();
                            }
                        }
                    }

                    // Filter by requested languages if specified
                    if let Some(ref langs) = request.languages {
                        if !langs.is_empty() && !langs.contains(&lang_code) {
                            continue;
                        }
                    }

                    let att_name = attachment
                        .info
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("subtitle_{}", attachment.id));

                    let format = format_from_filename(&att_name);
                    let display_name = if file_name.is_empty() {
                        att_name.clone()
                    } else {
                        format!("{} - {}", file_name, att_name)
                    };

                    let download_url = Self::attachment_download_url(attachment.id);

                    results.push(SubtitleSearchResult {
                        id: format!("animetosho_{}", attachment.id),
                        name: display_name,
                        language: lang_code.clone(),
                        language_name: language_name(&lang_code).into(),
                        format,
                        provider: "animetosho".into(),
                        detail_path: None,
                        download_path: Some(download_url),
                        download_count: None,
                        rating: None,
                        movie_name: Some(entry_title.clone()),
                        release_group: None,
                    });
                }
            }
        }

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let download_url = request
            .download_path
            .as_deref()
            .ok_or("AnimeTosho download requires a download_path")?;

        let client = Self::build_client()?;

        let response = client
            .get(download_url)
            .send()
            .await
            .map_err(|e| format!("AnimeTosho download request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "AnimeTosho download failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read AnimeTosho download response: {e}"))?;

        // AnimeTosho stores subtitle attachments as xz-compressed files.
        let xz_magic: &[u8] = b"\xFD\x37\x7A\x58\x5A\x00";
        let content = if bytes.starts_with(xz_magic) {
            Self::decompress_xz(&bytes)?
        } else {
            bytes.to_vec()
        };

        let format = request.format.clone();
        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("subtitle.{format}"));

        Ok(DownloadedSubtitle {
            name,
            format,
            content,
        })
    }
}
