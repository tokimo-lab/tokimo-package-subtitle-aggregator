/// Titlovi provider — Croatian, Serbian, Bosnian subtitles
/// API: https://kodi.titlovi.com/api/subtitles
/// Requires TITLOVI_USER / TITLOVI_PASS env vars
use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::Mutex;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    normalize_format, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const API_BASE: &str = "https://kodi.titlovi.com/api/subtitles";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                  Chrome/122.0.0.0 Safari/537.36";

struct TokenState {
    token: Option<String>,
    user_id: Option<u64>,
}

pub struct TitloviProvider {
    username: String,
    password: String,
    client: reqwest::Client,
    token_state: Mutex<TokenState>,
    staging_root: std::path::PathBuf,
}

impl TitloviProvider {
    pub fn new(
        username: impl Into<String>,
        password: impl Into<String>,
        staging_root: impl Into<std::path::PathBuf>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .build()
            .expect("Failed to build Titlovi HTTP client");
        Self {
            username: username.into(),
            password: password.into(),
            client,
            token_state: Mutex::new(TokenState {
                token: None,
                user_id: None,
            }),
            staging_root: staging_root.into(),
        }
    }

    async fn ensure_token(&self) -> Result<(String, u64), String> {
        {
            let guard = self.token_state.lock().await;
            if let (Some(token), Some(user_id)) = (&guard.token, &guard.user_id) {
                return Ok((token.clone(), *user_id));
            }
        }
        self.do_login().await
    }

    async fn do_login(&self) -> Result<(String, u64), String> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct TokenResponse {
            token: String,
            user_id: u64,
        }

        let resp = self
            .client
            .post(format!("{API_BASE}/gettoken"))
            .query(&[
                ("username", self.username.as_str()),
                ("password", self.password.as_str()),
                ("json", "true"),
            ])
            .send()
            .await
            .map_err(|e| format!("Titlovi: login request failed: {e}"))?;

        if resp.status().as_u16() == 401 {
            return Err(
                "Titlovi: authentication failed — check TITLOVI_USER / TITLOVI_PASS".into(),
            );
        }
        if !resp.status().is_success() {
            return Err(format!(
                "Titlovi: login failed with HTTP {}",
                resp.status().as_u16()
            ));
        }

        let data: TokenResponse = resp
            .json()
            .await
            .map_err(|e| format!("Titlovi: failed to parse token response: {e}"))?;

        let mut guard = self.token_state.lock().await;
        guard.token = Some(data.token.clone());
        guard.user_id = Some(data.user_id);
        Ok((data.token, data.user_id))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TitloviSearchResponse {
    subtitle_results: Option<Vec<TitloviResult>>,
    #[allow(dead_code)]
    pages_available: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TitloviResult {
    id: Option<serde_json::Value>,
    title: Option<String>,
    link: Option<String>,
    lang: Option<String>,
    #[allow(dead_code)]
    season: Option<u32>,
    #[allow(dead_code)]
    episode: Option<u32>,
    #[allow(dead_code)]
    year: Option<u32>,
    rating: Option<f64>,
    download_count: Option<u64>,
    release: Option<String>,
}

fn titlovi_lang_to_code(lang: &str) -> &'static str {
    match lang.to_lowercase().as_str() {
        "hr" | "hrv" | "croatian" => "hr",
        "sr" | "srp" | "serbian" | "sr-latn" | "sr-cyrl" => "sr",
        "bs" | "bos" | "bosnian" => "bs",
        "sl" | "slv" | "slovenian" => "sl",
        _ => "hr",
    }
}

fn titlovi_lang_name(code: &str) -> &'static str {
    match code {
        "hr" => "Croatian",
        "sr" => "Serbian",
        "bs" => "Bosnian",
        "sl" => "Slovenian",
        _ => "Croatian",
    }
}

#[async_trait]
impl SubtitleProvider for TitloviProvider {
    fn name(&self) -> &str {
        "titlovi"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let (token, user_id) = self.ensure_token().await?;

        let query = request.query.clone().unwrap_or_default();
        let langs = match &request.languages {
            Some(l) if !l.is_empty() => l.join("|"),
            _ => "hr|sr|bs".to_string(),
        };

        let mut params: Vec<(&str, String)> = vec![
            ("query", query.clone()),
            ("lang", langs),
            ("token", token),
            ("userid", user_id.to_string()),
            ("json", "true".to_string()),
        ];

        if let Some(imdb_id) = &request.imdb_id {
            params.push(("imdbID", imdb_id.clone()));
        }

        let resp = self
            .client
            .get(format!("{API_BASE}/search"))
            .query(&params)
            .send()
            .await
            .map_err(|e| format!("Titlovi: search request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "Titlovi: search failed with HTTP {}",
                resp.status().as_u16()
            ));
        }

        let data: TitloviSearchResponse = resp
            .json()
            .await
            .map_err(|e| format!("Titlovi: failed to parse search response: {e}"))?;

        let mut results = Vec::new();
        for sub in data.subtitle_results.unwrap_or_default() {
            let id = match &sub.id {
                Some(serde_json::Value::Number(n)) => n.to_string(),
                Some(serde_json::Value::String(s)) => s.clone(),
                _ => continue,
            };
            let link = match sub.link {
                Some(l) if !l.is_empty() => l,
                _ => continue,
            };

            let lang_raw = sub.lang.as_deref().unwrap_or("hr");
            let language = titlovi_lang_to_code(lang_raw).to_string();
            let language_name = titlovi_lang_name(&language).to_string();
            let name = sub.title.clone().unwrap_or_else(|| query.clone());
            let release = sub.release.clone().unwrap_or_default();

            results.push(SubtitleSearchResult {
                id,
                name,
                language,
                language_name,
                format: "srt".into(),
                provider: "titlovi".into(),
                detail_path: None,
                download_path: Some(link),
                download_count: sub.download_count,
                rating: sub.rating,
                movie_name: sub.title,
                release_group: if release.is_empty() {
                    None
                } else {
                    Some(release)
                },
            });
        }

        tracing::info!("Titlovi: found {} results", results.len());
        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let url = request
            .download_path
            .as_deref()
            .ok_or("Titlovi: missing download_path")?;

        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("Titlovi: download request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "Titlovi: download failed with HTTP {}",
                resp.status().as_u16()
            ));
        }

        let file_name = resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                let re =
                    regex::Regex::new(r#"filename[^;=\n]*=(?:(['"])(.+?)\1|([^;\n]*))"#).ok()?;
                re.captures(v)
                    .and_then(|c| c.get(2).or_else(|| c.get(3)))
                    .map(|m| m.as_str().trim().to_string())
            })
            .unwrap_or_else(|| format!("titlovi_{}.zip", request.subtitle_id));

        let content = resp
            .bytes()
            .await
            .map_err(|e| format!("Titlovi: failed to read download content: {e}"))?;

        if let Some(format) = normalize_format(&file_name) {
            return Ok(DownloadedSubtitle {
                name: file_name,
                format,
                content: content.to_vec(),
            });
        }

        extract_archive(&content, &file_name, &request.language, &self.staging_root).await
    }
}
