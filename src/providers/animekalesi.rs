use async_trait::async_trait;
use scraper::{Html, Selector};
use std::path::PathBuf;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const ANIMEKALESI_BASE_URL: &str = "https://www.animekalesi.com";
const ANIMEKALESI_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub struct AnimekalesiProvider {
    staging_root: PathBuf,
}

impl AnimekalesiProvider {
    pub fn new() -> Self {
        Self {
            staging_root: std::env::temp_dir(),
        }
    }

    pub fn with_staging_root(mut self, staging_root: impl Into<PathBuf>) -> Self {
        self.staging_root = staging_root.into();
        self
    }

    fn build_client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .user_agent(ANIMEKALESI_USER_AGENT)
            .build()
            .map_err(|e| format!("Failed to build AnimeKalesi HTTP client: {e}"))
    }

    fn normalize_title(title: &str) -> String {
        let tr_chars = [
            ('İ', 'i'), ('I', 'i'), ('Ğ', 'g'), ('Ü', 'u'), ('Ş', 's'), ('Ö', 'o'), ('Ç', 'c'),
            ('ı', 'i'), ('ğ', 'g'), ('ü', 'u'), ('ş', 's'), ('ö', 'o'), ('ç', 'c'),
        ];
        let mut s = title.to_lowercase();
        for (tr, en) in &tr_chars {
            s = s.replace(*tr, &en.to_string());
        }
        s.trim().to_string()
    }

    fn parse_season_episode(title: &str) -> (Option<u32>, Option<u32>) {
        let season = regex::Regex::new(r"(\d+)\.\s*Sezon")
            .ok()
            .and_then(|re| re.captures(title))
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(1);

        let episode = regex::Regex::new(r"(\d+)\.\s*B[oö]l[uü]m")
            .ok()
            .and_then(|re| re.captures(title))
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse::<u32>().ok());

        (Some(season), episode)
    }
}

#[async_trait]
impl SubtitleProvider for AnimekalesiProvider {
    fn name(&self) -> &str {
        "animekalesi"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request
            .query
            .as_deref()
            .filter(|q| !q.trim().is_empty())
            .ok_or("AnimeKalesi search requires a query (series name)")?;

        let client = Self::build_client()?;

        // Fetch the full anime list page
        let list_url = format!("{ANIMEKALESI_BASE_URL}/tum-anime-serileri.html");
        let list_resp = client
            .get(&list_url)
            .header("Referer", ANIMEKALESI_BASE_URL)
            .send()
            .await
            .map_err(|e| format!("AnimeKalesi anime list fetch failed: {e}"))?;

        if !list_resp.status().is_success() {
            return Err(format!(
                "AnimeKalesi anime list failed: HTTP {}",
                list_resp.status().as_u16()
            ));
        }

        let html = list_resp
            .text()
            .await
            .map_err(|e| format!("Failed to read AnimeKalesi anime list: {e}"))?;

        let series_url: Option<(String, String)> = {
            let document = Html::parse_document(&html);
            let td_sel = Selector::parse("td#bolumler")
                .map_err(|e| format!("AnimeKalesi selector error: {e}"))?;
            let a_sel = Selector::parse("a")
                .map_err(|e| format!("AnimeKalesi selector error: {e}"))?;

            let normalized_query = Self::normalize_title(query.trim());
            let mut found: Option<(String, String)> = None;

            for td in document.select(&td_sel) {
                let Some(link) = td.select(&a_sel).next() else {
                    continue;
                };
                let title = link.text().collect::<String>().trim().to_string();
                let href = link.value().attr("href").unwrap_or_default().to_string();

                if href.is_empty() || !href.contains("bolumler-") {
                    continue;
                }

                let normalized_title = Self::normalize_title(&title);
                if normalized_title == normalized_query
                    || normalized_title.contains(&normalized_query)
                    || normalized_query.contains(&normalized_title)
                {
                    let exact = normalized_title == normalized_query;
                    found = Some((title, href));
                    if exact {
                        break;
                    }
                }
            }
            found
        };

        let Some((series_title, series_href)) = series_url else {
            return Ok(Vec::new());
        };

        // Get the subtitle listing for this series
        let subtitle_page_href = series_href.replace("bolumler-", "altyazib-");
        let subtitle_page_url = format!("{ANIMEKALESI_BASE_URL}/{subtitle_page_href}");

        let subs_resp = client
            .get(&subtitle_page_url)
            .header("Referer", ANIMEKALESI_BASE_URL)
            .send()
            .await
            .map_err(|e| format!("AnimeKalesi subtitle page fetch failed: {e}"))?;

        if !subs_resp.status().is_success() {
            return Ok(Vec::new());
        }

        let subs_html = subs_resp
            .text()
            .await
            .map_err(|e| format!("Failed to read AnimeKalesi subtitle page: {e}"))?;

        let episode_entries: Vec<(String, String)> = {
            let subs_doc = Html::parse_document(&subs_html);
            let td_indir_sel = Selector::parse("td#ayazi_indir")
                .map_err(|e| format!("AnimeKalesi selector error: {e}"))?;
            let a_sel2 = Selector::parse("a[href]")
                .map_err(|e| format!("AnimeKalesi selector error: {e}"))?;

            let mut entries = Vec::new();
            for td in subs_doc.select(&td_indir_sel) {
                let Some(link) = td.select(&a_sel2).next() else {
                    continue;
                };
                let href = link.value().attr("href").unwrap_or_default().to_string();
                let title = link
                    .value()
                    .attr("title")
                    .unwrap_or_default()
                    .to_string();
                if href.contains("indir_bolum-")
                    && title.contains("B\u{00f6}l\u{00fc}m T\u{00fc}rk\u{00e7}e Altyaz")
                {
                    entries.push((href, title));
                }
            }
            entries
        };

        let mut results = Vec::new();

        for (href, title) in episode_entries {
            let episode_page_url = format!("{ANIMEKALESI_BASE_URL}/{href}");
            let (season, episode) = Self::parse_season_episode(&title);

            let version = match (season, episode) {
                (Some(s), Some(e)) => format!("{series_title} - S{s:02}E{e:02}"),
                _ => title.clone(),
            };

            let id = episode_page_url.clone();
            results.push(SubtitleSearchResult {
                id,
                name: version,
                language: "tr".into(),
                language_name: "Turkish".into(),
                format: "srt".into(),
                provider: "animekalesi".into(),
                detail_path: Some(episode_page_url),
                download_path: None,
                download_count: None,
                rating: None,
                movie_name: Some(series_title.clone()),
                release_group: None,
            });
        }

        Ok(results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let detail_url = request
            .detail_path
            .as_deref()
            .or(request.download_path.as_deref())
            .ok_or("AnimeKalesi download requires detail_path")?;

        let client = Self::build_client()?;

        let page_resp = client
            .get(detail_url)
            .header("Referer", ANIMEKALESI_BASE_URL)
            .send()
            .await
            .map_err(|e| format!("AnimeKalesi episode page fetch failed: {e}"))?;

        if !page_resp.status().is_success() {
            return Err(format!(
                "AnimeKalesi episode page failed: HTTP {}",
                page_resp.status().as_u16()
            ));
        }

        let html = page_resp
            .text()
            .await
            .map_err(|e| format!("Failed to read AnimeKalesi episode page: {e}"))?;

        let download_url: String = {
            let document = Html::parse_document(&html);
            let dl_sel = Selector::parse("div#altyazi_indir a[href]")
                .map_err(|e| format!("AnimeKalesi selector error: {e}"))?;
            let href = document
                .select(&dl_sel)
                .next()
                .and_then(|a| a.value().attr("href"))
                .ok_or_else(|| {
                    "AnimeKalesi: no download link found on episode page".to_string()
                })?
                .to_string();
            if href.starts_with("http") {
                href
            } else {
                format!("{ANIMEKALESI_BASE_URL}/{href}")
            }
        };

        let content = client
            .get(&download_url)
            .header("Referer", detail_url)
            .send()
            .await
            .map_err(|e| format!("AnimeKalesi download failed: {e}"))?
            .bytes()
            .await
            .map_err(|e| format!("Failed to read AnimeKalesi download: {e}"))?;

        let archive_name = download_url
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("subtitle.zip")
            .to_string();

        extract_archive(&content, &archive_name, &request.language, &self.staging_root).await
    }
}
