use async_trait::async_trait;
use regex::Regex;
use reqwest::header::REFERER;
use scraper::{Html, Selector};
use tokio::sync::Mutex;

use super::SubtitleProvider;
use crate::models::{
    matches_preferred_language, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const BASE: &str = "https://www.addic7ed.com";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                  Chrome/122.0.0.0 Safari/537.36";

struct LoginState {
    logged_in: bool,
}

pub struct Addic7edProvider {
    username: Option<String>,
    password: Option<String>,
    client: reqwest::Client,
    login_state: Mutex<LoginState>,
}

impl Addic7edProvider {
    pub fn new(username: Option<String>, password: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .cookie_store(true)
            .build()
            .expect("Failed to build Addic7ed HTTP client");
        Self {
            username,
            password,
            client,
            login_state: Mutex::new(LoginState { logged_in: false }),
        }
    }

    /// Login and set session cookies. Must not hold the mutex while calling.
    async fn ensure_logged_in(&self) -> Result<(), String> {
        let is_logged_in = {
            let guard = self.login_state.lock().await;
            guard.logged_in
        };
        if is_logged_in {
            return Ok(());
        }

        let (username, password) = match (&self.username, &self.password) {
            (Some(u), Some(p)) => (u.clone(), p.clone()),
            _ => {
                // No credentials — skip login, searches may still work
                return Ok(());
            }
        };

        let params = [
            ("username", username.as_str()),
            ("password", password.as_str()),
            ("Submit", "Log in"),
            ("url", ""),
            ("remember", "true"),
        ];

        let response = self
            .client
            .post(format!("{BASE}/dologin.php"))
            .header(REFERER, format!("{BASE}/login.php"))
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("Addic7ed login request failed: {e}"))?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if body.contains("Wrong password") || body.contains("doesn't exist") {
            return Err(format!(
                "Addic7ed: wrong username or password for '{username}'"
            ));
        }

        if body.contains("relax, slow down") {
            return Err("Addic7ed: rate limited during login".into());
        }

        // Successful login redirects (302) or returns the panel page
        if status.is_success() || status.as_u16() == 302 {
            let mut guard = self.login_state.lock().await;
            guard.logged_in = true;
        } else {
            return Err(format!("Addic7ed: login failed with status {status}"));
        }

        Ok(())
    }

    /// Fetch the full shows list from shows.php and return Vec<(name_lower, show_id)>.
    async fn fetch_show_ids(&self) -> Result<Vec<(String, u64)>, String> {
        let html = self
            .client
            .get(format!("{BASE}/shows.php"))
            .send()
            .await
            .map_err(|e| format!("Addic7ed shows.php fetch failed: {e}"))?
            .text()
            .await
            .map_err(|e| format!("Addic7ed shows.php read failed: {e}"))?;

        let document = Html::parse_document(&html);
        let sel = Selector::parse(r#"td > h3 > a[href]"#)
            .map_err(|e| format!("Addic7ed selector error: {e}"))?;

        let mut result = Vec::new();
        for anchor in document.select(&sel) {
            let href = match anchor.value().attr("href") {
                Some(h) if h.starts_with("/show/") => h,
                _ => continue,
            };
            let id_str = href.trim_start_matches("/show/");
            let Ok(id) = id_str.parse::<u64>() else {
                continue;
            };
            let name = anchor.text().collect::<String>().trim().to_lowercase();
            if !name.is_empty() {
                result.push((name, id));
            }
        }
        Ok(result)
    }

    /// Resolve a show title to a show ID by searching the shows list.
    async fn resolve_show_id(&self, title: &str) -> Option<u64> {
        let ids = self.fetch_show_ids().await.ok()?;
        let query = title.to_lowercase();
        // Exact match first
        if let Some((_, id)) = ids.iter().find(|(n, _)| n == &query) {
            return Some(*id);
        }
        // Prefix / contains
        if let Some((_, id)) = ids.iter().find(|(n, _)| n.starts_with(&query)) {
            return Some(*id);
        }
        if let Some((_, id)) = ids.iter().find(|(n, _)| n.contains(&query)) {
            return Some(*id);
        }
        None
    }

    /// Fetch episode subtitles for a show + season via ajax_loadShow.php.
    async fn fetch_show_subtitles(
        &self,
        show_id: u64,
        season: u32,
        query: &str,
        languages: Option<&[String]>,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let url = format!("{BASE}/ajax_loadShow.php");
        let html = self
            .client
            .get(&url)
            .query(&[
                ("show", show_id.to_string().as_str()),
                ("season", season.to_string().as_str()),
                ("langs", "|"),
            ])
            .header(REFERER, format!("{BASE}/show/{show_id}"))
            .header("X-Requested-With", "XMLHttpRequest")
            .send()
            .await
            .map_err(|e| format!("Addic7ed ajax_loadShow failed: {e}"))?
            .text()
            .await
            .map_err(|e| format!("Addic7ed ajax_loadShow read failed: {e}"))?;

        parse_episode_rows(&html, query, languages)
    }

    /// Search movies via moviesearch?q= and return subtitle results.
    async fn search_movies(
        &self,
        title: &str,
        languages: Option<&[String]>,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let encoded = url::form_urlencoded::byte_serialize(title.as_bytes()).collect::<String>();
        let html = self
            .client
            .get(format!("{BASE}/moviesearch?q={encoded}"))
            .header(REFERER, BASE)
            .send()
            .await
            .map_err(|e| format!("Addic7ed moviesearch failed: {e}"))?
            .text()
            .await
            .map_err(|e| format!("Addic7ed moviesearch read failed: {e}"))?;

        // Parse HTML in a block so `Html` (not Send) is dropped before any `.await`.
        let movie_ids: Vec<(String, String)> = {
            let document = Html::parse_document(&html);
            let link_sel = Selector::parse(r#"a[href]"#)
                .map_err(|e| format!("Addic7ed selector error: {e}"))?;
            let mut ids = Vec::new();
            for anchor in document.select(&link_sel) {
                let href = match anchor.value().attr("href") {
                    Some(h) if h.starts_with("movie/") => h.to_string(),
                    _ => continue,
                };
                let parts: Vec<&str> = href.splitn(2, '/').collect();
                if parts.len() == 2 && !parts[1].is_empty() {
                    let text = anchor.text().collect::<String>().trim().to_string();
                    ids.push((parts[1].to_string(), text));
                }
            }
            ids
        };

        let mut results = Vec::new();
        for (movie_id, movie_title) in movie_ids.into_iter().take(3) {
            let movie_results = self
                .fetch_movie_subtitles(&movie_id, &movie_title, languages)
                .await
                .unwrap_or_default();
            results.extend(movie_results);
        }
        Ok(results)
    }

    /// Fetch subtitles for a movie page.
    async fn fetch_movie_subtitles(
        &self,
        movie_id: &str,
        movie_title: &str,
        languages: Option<&[String]>,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let url = format!("{BASE}/movie/{movie_id}");
        let html = self
            .client
            .get(&url)
            .header(REFERER, BASE)
            .send()
            .await
            .map_err(|e| format!("Addic7ed movie page fetch failed: {e}"))?
            .text()
            .await
            .map_err(|e| format!("Addic7ed movie page read failed: {e}"))?;

        parse_movie_rows(&html, movie_id, movie_title, languages)
    }
}

// ── HTML parsers ─────────────────────────────────────────────────────────────

/// Parse `tr.epeven` rows from ajax_loadShow.php response.
fn parse_episode_rows(
    html: &str,
    show_name: &str,
    languages: Option<&[String]>,
) -> Result<Vec<SubtitleSearchResult>, String> {
    let document = Html::parse_document(html);
    let row_sel =
        Selector::parse("tr.epeven").map_err(|e| format!("Addic7ed row selector error: {e}"))?;
    let cell_sel =
        Selector::parse("td").map_err(|e| format!("Addic7ed cell selector error: {e}"))?;
    let link_sel =
        Selector::parse("a[href]").map_err(|e| format!("Addic7ed link selector error: {e}"))?;

    let mut results = Vec::new();

    for row in document.select(&row_sel) {
        let cells: Vec<_> = row.select(&cell_sel).collect();
        if cells.len() < 10 {
            continue;
        }

        let status = cells[5].text().collect::<String>();
        if status.contains('%') {
            continue; // incomplete subtitle
        }

        let season_text = cells[0].text().collect::<String>().trim().to_string();
        let episode_text = cells[1].text().collect::<String>().trim().to_string();
        let title_text = cells[2].text().collect::<String>().trim().to_string();
        let lang_text = cells[3].text().collect::<String>().trim().to_string();
        let version_text = cells[4].text().collect::<String>().trim().to_string();

        let page_link = cells[2]
            .select(&link_sel)
            .next()
            .and_then(|a| a.value().attr("href"))
            .map(|h| {
                if h.starts_with('/') {
                    format!("{BASE}{h}")
                } else {
                    h.to_string()
                }
            });

        let download_link = cells[9]
            .select(&link_sel)
            .next()
            .and_then(|a| a.value().attr("href"))
            .map(|h| h.trim_start_matches('/').to_string());

        let language = map_language(&lang_text);
        if !matches_preferred_language(&language, languages) {
            continue;
        }

        let id = download_link
            .as_deref()
            .unwrap_or("")
            .replace('/', "-")
            .trim_matches('-')
            .to_string();
        let id = if id.is_empty() {
            format!("a7-{show_name}-s{season_text}e{episode_text}")
        } else {
            format!("a7-{id}")
        };

        let name = if !title_text.is_empty() {
            format!("{show_name} - S{season_text:0>2}E{episode_text:0>2} - {title_text}")
        } else {
            format!("{show_name} - S{season_text:0>2}E{episode_text:0>2}")
        };

        results.push(SubtitleSearchResult {
            id,
            name,
            language,
            language_name: lang_text,
            format: "srt".into(),
            provider: "addic7ed".into(),
            detail_path: page_link,
            download_path: download_link,
            download_count: None,
            rating: None,
            movie_name: Some(show_name.to_string()),
            release_group: (!version_text.is_empty()).then_some(version_text),
        });
    }

    Ok(results)
}

/// Parse subtitle tables from a movie/{id} page.
fn parse_movie_rows(
    html: &str,
    movie_id: &str,
    movie_title: &str,
    languages: Option<&[String]>,
) -> Result<Vec<SubtitleSearchResult>, String> {
    let document = Html::parse_document(html);

    // Tables with class "tabel95" and width="100%"
    let table_sel = Selector::parse(r#"table[class="tabel95"]"#)
        .map_err(|e| format!("Addic7ed table selector error: {e}"))?;
    let link_sel =
        Selector::parse("a[href]").map_err(|e| format!("Addic7ed link selector error: {e}"))?;
    let td_sel = Selector::parse("td").map_err(|e| format!("Addic7ed td selector error: {e}"))?;

    let mut results = Vec::new();
    let page_link = format!("{BASE}/movie/{movie_id}");

    for table in document.select(&table_sel) {
        let rows: Vec<_> = table.select(&td_sel).collect();
        // Find language td: look for text that resembles a language name
        // The structure has rows: version row, subtitle row, download row
        // We look for the download link in any anchor with href containing "updated/"
        let download_link = table
            .select(&link_sel)
            .find(|a| {
                a.value()
                    .attr("href")
                    .map(|h| h.contains("updated/") || h.contains("original/"))
                    .unwrap_or(false)
            })
            .and_then(|a| a.value().attr("href"))
            .map(|h| h.trim_start_matches('/').to_string());

        let Some(dl_path) = download_link else {
            continue;
        };

        // Find language text from cells
        let lang_text = rows
            .iter()
            .find(|td| {
                let t = td.text().collect::<String>().trim().to_string();
                !t.is_empty() && is_known_language(&t)
            })
            .map(|td| td.text().collect::<String>().trim().to_string())
            .unwrap_or_else(|| "English".to_string());

        let language = map_language(&lang_text);
        if !matches_preferred_language(&language, languages) {
            continue;
        }

        // Extract version from the table (first meaningful text in a bold/version cell)
        let version_sel = Selector::parse("b").unwrap();
        let version_text = table
            .select(&version_sel)
            .next()
            .map(|b| {
                let t = b.text().collect::<String>().trim().to_string();
                // Strip leading word (e.g. "Version", "Versión")
                t.split_once(' ')
                    .map(|x| x.1)
                    .map(str::trim)
                    .unwrap_or("")
                    .to_string()
            })
            .filter(|v| !v.is_empty());

        let id = format!("a7-movie-{movie_id}-{}", dl_path.replace('/', "-"));
        results.push(SubtitleSearchResult {
            id,
            name: movie_title.to_string(),
            language,
            language_name: lang_text,
            format: "srt".into(),
            provider: "addic7ed".into(),
            detail_path: Some(page_link.clone()),
            download_path: Some(dl_path),
            download_count: None,
            rating: None,
            movie_name: Some(movie_title.to_string()),
            release_group: version_text,
        });
    }

    Ok(results)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn map_language(lang: &str) -> String {
    match lang.trim() {
        "English" => "en",
        "French" => "fr",
        "Spanish" | "Spanish (Spain)" | "Spanish (Latin America)" => "es",
        "German" => "de",
        "Italian" => "it",
        "Portuguese" => "pt",
        "Portuguese (Brazilian)" | "Portuguese-Brasil" | "Portuguese (Brasil)" => "pt-BR",
        "Dutch" => "nl",
        "Polish" => "pl",
        "Russian" => "ru",
        "Japanese" => "ja",
        "Chinese (Simplified)" | "Chinese (Traditional)" | "Chinese" => "zh",
        "Korean" => "ko",
        "Arabic" => "ar",
        "Turkish" => "tr",
        "Hungarian" => "hu",
        "Czech" => "cs",
        "Slovak" => "sk",
        "Romanian" => "ro",
        "Swedish" => "sv",
        "Norwegian" => "no",
        "Danish" => "da",
        "Finnish" => "fi",
        "Greek" => "el",
        "Bulgarian" => "bg",
        "Croatian" => "hr",
        "Serbian" | "Serbian (Cyrillic)" | "Serbian (Latin)" => "sr",
        "Ukrainian" => "uk",
        "Hebrew" => "he",
        "Persian" => "fa",
        "Indonesian" => "id",
        "Malay" => "ms",
        "Thai" => "th",
        "Vietnamese" => "vi",
        "Catalan" => "ca",
        "Armenian" => "hy",
        "Azerbaijani" => "az",
        "Bengali" => "bn",
        "Albanian" => "sq",
        "Slovenian" => "sl",
        "Macedonian" => "mk",
        "Bosnian" => "bs",
        "Basque" => "eu",
        "Galician" => "gl",
        _ => "und",
    }
    .to_string()
}

fn is_known_language(s: &str) -> bool {
    matches!(
        s.trim(),
        "English"
            | "French"
            | "Spanish"
            | "Spanish (Spain)"
            | "Spanish (Latin America)"
            | "German"
            | "Italian"
            | "Portuguese"
            | "Portuguese (Brazilian)"
            | "Portuguese-Brasil"
            | "Portuguese (Brasil)"
            | "Dutch"
            | "Polish"
            | "Russian"
            | "Japanese"
            | "Chinese"
            | "Chinese (Simplified)"
            | "Chinese (Traditional)"
            | "Korean"
            | "Arabic"
            | "Turkish"
            | "Hungarian"
            | "Czech"
            | "Slovak"
            | "Romanian"
            | "Swedish"
            | "Norwegian"
            | "Danish"
            | "Finnish"
            | "Greek"
            | "Bulgarian"
            | "Croatian"
            | "Serbian"
            | "Serbian (Cyrillic)"
            | "Serbian (Latin)"
            | "Ukrainian"
            | "Hebrew"
            | "Persian"
            | "Indonesian"
            | "Malay"
            | "Thai"
            | "Vietnamese"
            | "Catalan"
            | "Armenian"
            | "Azerbaijani"
            | "Bengali"
            | "Albanian"
            | "Slovenian"
            | "Macedonian"
            | "Bosnian"
            | "Basque"
            | "Galician"
    )
}

/// Try to extract a season number from the query string (e.g., "S01", "season 1").
fn extract_season(query: &str) -> Option<u32> {
    let re = Regex::new(r"(?i)[Ss](?:eason\s*)?(\d{1,2})").ok()?;
    re.captures(query)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
}

/// Strip season/episode markers from a title to get just the show/movie name.
fn strip_season_episode(query: &str) -> String {
    if let Ok(re) = Regex::new(r"(?i)\s+[Ss]\d{1,2}(?:[Ee]\d{1,2})?.*$") {
        re.replace(query, "").trim().to_string()
    } else {
        query.trim().to_string()
    }
}

/// Extract the subtitle filename from a Content-Disposition header, if present.
fn filename_from_disposition(header: Option<&reqwest::header::HeaderValue>) -> Option<String> {
    let value = header?.to_str().ok()?;
    let re = Regex::new(r#"filename="?([^";]+)"?"#).ok()?;
    re.captures(value)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

// ── Trait impl ────────────────────────────────────────────────────────────────

#[async_trait]
impl SubtitleProvider for Addic7edProvider {
    fn name(&self) -> &str {
        "addic7ed"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| "Addic7ed: search query is required".to_string())?;

        let langs = request.languages.as_deref();

        // Extract title (without S01E02 markers) and season number
        let title = strip_season_episode(query);
        let season = extract_season(query).unwrap_or(1);

        let mut results = Vec::new();

        // --- TV show search ---
        if let Some(show_id) = self.resolve_show_id(&title).await {
            let show_results = self
                .fetch_show_subtitles(show_id, season, &title, langs)
                .await
                .unwrap_or_default();
            results.extend(show_results);
        }

        // --- Movie search (only if show search was empty or query looks like a movie) ---
        if results.is_empty() {
            let movie_results = self.search_movies(&title, langs).await.unwrap_or_default();
            results.extend(movie_results);
        }

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let download_path = request
            .download_path
            .as_deref()
            .ok_or_else(|| "Addic7ed: download_path is required".to_string())?;

        // Warn if no credentials, but still attempt (public subtitles may not need login)
        if self.username.is_none() || self.password.is_none() {
            tracing::warn!("Addic7ed: no credentials provided; download may fail");
        } else {
            self.ensure_logged_in().await?;
        }

        let download_url =
            if download_path.starts_with("http://") || download_path.starts_with("https://") {
                download_path.to_string()
            } else {
                format!("{BASE}/{}", download_path.trim_start_matches('/'))
            };

        let referer = request.detail_path.as_deref().unwrap_or(BASE).to_string();

        let response = self
            .client
            .get(&download_url)
            .header(REFERER, &referer)
            .send()
            .await
            .map_err(|e| format!("Addic7ed download request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("Addic7ed download failed with status {status}"));
        }

        // If the response is HTML, the download limit was exceeded or auth failed
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        if content_type.contains("text/html") {
            return Err(
                "Addic7ed: got HTML instead of subtitle file — possibly download limit exceeded or login required".into(),
            );
        }

        // Try to get filename from Content-Disposition
        let disposition_name =
            filename_from_disposition(response.headers().get(reqwest::header::CONTENT_DISPOSITION));

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Addic7ed reading download body failed: {e}"))?
            .to_vec();

        let filename = disposition_name
            .or_else(|| {
                // Derive from download path: last segment
                download_path
                    .rsplit('/')
                    .next()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| request.name.clone())
            .unwrap_or_else(|| format!("{}.srt", request.subtitle_id));

        // Determine format from filename extension
        let format = {
            let ext = filename.rsplit('.').next().unwrap_or("srt").to_lowercase();
            match ext.as_str() {
                "srt" | "ass" | "ssa" | "vtt" => ext,
                _ => request.format.clone(),
            }
        };

        Ok(DownloadedSubtitle {
            name: filename,
            format,
            content,
        })
    }
}
