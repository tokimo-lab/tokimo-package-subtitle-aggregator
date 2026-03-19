/// LegendasDivx provider — Portuguese subtitle site (legendasdivx.pt)
/// HTML scraping with optional login via LEGENDASDIVX_USER / LEGENDASDIVX_PASS env vars
use async_trait::async_trait;
use scraper::{Html, Selector, Element};

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult};

const SITE: &str = "https://www.legendasdivx.pt";
const UA: &str = "Sub-Zero/2";

fn build_client(cookies: bool) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .cookie_store(cookies)
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/webp,*/*;q=0.8".parse().unwrap());
            h.insert("Origin", SITE.parse().unwrap());
            h.insert("Referer", SITE.parse().unwrap());
            h
        })
        .build()
        .map_err(|e| format!("legendasdivx: failed to build client: {e}"))
}

fn get_credentials() -> (Option<String>, Option<String>) {
    let user = std::env::var("LEGENDASDIVX_USER").ok();
    let pass = std::env::var("LEGENDASDIVX_PASS").ok();
    (user, pass)
}

pub struct LegendasDivxProvider {
    staging_root: std::path::PathBuf,
}

impl LegendasDivxProvider {
    pub fn new(staging_root: impl Into<std::path::PathBuf>) -> Self {
        Self { staging_root: staging_root.into() }
    }

    /// Attempt login and return authenticated client. Falls back to unauthenticated on failure.
    async fn login(&self, user: &str, pass: &str) -> Result<reqwest::Client, String> {
        let client = build_client(true)?;
        let login_url = format!("{SITE}/forum/ucp.php?mode=login");

        let resp = client.get(&login_url).send().await
            .map_err(|e| format!("legendasdivx: login page GET failed: {e}"))?;
        let html = resp.text().await.map_err(|e| format!("legendasdivx: read login page: {e}"))?;

        // Extract all form inputs — keep Html in a scoped block so it's dropped before .await
        let mut form_data: Vec<(String, String)> = Vec::new();
        {
            let doc = Html::parse_document(&html);
            let input_sel = Selector::parse("input").map_err(|_| "legendasdivx: selector error")?;
            for input in doc.select(&input_sel) {
                let name = input.value().attr("name").unwrap_or("").to_string();
                let value = input.value().attr("value").unwrap_or("").to_string();
                if !name.is_empty() {
                    form_data.push((name, value));
                }
            }
            for pair in &mut form_data {
                if pair.0 == "username" { pair.1 = user.to_string(); }
                if pair.0 == "password" { pair.1 = pass.to_string(); }
            }
        } // doc dropped here, before .await

        let _resp = client.post(&login_url)
            .form(&form_data)
            .send()
            .await
            .map_err(|e| format!("legendasdivx: login POST failed: {e}"))?;

        Ok(client)
    }
}

#[async_trait]
impl SubtitleProvider for LegendasDivxProvider {
    fn name(&self) -> &str {
        "legendasdivx"
    }

    async fn search(&self, request: &SubtitleSearchRequest) -> Result<Vec<SubtitleSearchResult>, String> {
        let query = request.query.clone().unwrap_or_default();
        if query.trim().is_empty() {
            return Err("legendasdivx: search requires a query".into());
        }

        let (user, pass) = get_credentials();
        let client = if let (Some(u), Some(p)) = (user.as_deref(), pass.as_deref()) {
            match self.login(u, p).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("legendasdivx: login failed ({}), continuing unauthenticated", e);
                    build_client(false)?
                }
            }
        } else {
            build_client(false)?
        };

        // Determine language filter: pt-BR = 29, pt = 28
        let req_langs = request.languages.as_deref().unwrap_or(&[]);
        let lang_filter = if req_langs.iter().any(|l| l == "pt-BR" || l == "pt-br") {
            "&form_cat=29"
        } else {
            "&form_cat=28"
        };

        let encoded_query = url::form_urlencoded::byte_serialize(
            format!("\"{}\"", query).as_bytes()
        ).collect::<String>();

        let search_url = format!(
            "{SITE}/modules.php?name=Downloads&file=jz&d_op=search&op=_jz00&query={encoded_query}&temporada=&episodio=&imdb={lang_filter}"
        );

        let resp = client.get(&search_url)
            .header("Referer", format!("{SITE}/index.php"))
            .send()
            .await
            .map_err(|e| format!("legendasdivx: search request failed: {e}"))?;

        let status = resp.status().as_u16();
        if status == 302 {
            return Err("legendasdivx: redirected to login page; credentials may be required or expired".into());
        }
        if !resp.status().is_success() {
            return Err(format!("legendasdivx: search HTTP {status}"));
        }

        let html = resp.text().await.map_err(|e| format!("legendasdivx: read search response: {e}"))?;

        if html.contains("A legenda não foi encontrada") {
            return Ok(vec![]);
        }

        parse_search_results(&html)
    }

    async fn download(&self, request: &SubtitleDownloadRequest) -> Result<DownloadedSubtitle, String> {
        let url = request.download_path.as_deref()
            .or(request.detail_path.as_deref())
            .ok_or("legendasdivx: download requires download_path")?;

        let (user, pass) = get_credentials();
        let client = if let (Some(u), Some(p)) = (user.as_deref(), pass.as_deref()) {
            match self.login(u, p).await {
                Ok(c) => c,
                Err(_) => build_client(false)?,
            }
        } else {
            build_client(false)?
        };

        let resp = client.get(url).send().await
            .map_err(|e| format!("legendasdivx: download request: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("legendasdivx: download HTTP {}", resp.status().as_u16()));
        }

        let bytes = resp.bytes().await.map_err(|e| format!("legendasdivx: read bytes: {e}"))?;
        let filename = url.rsplit('/').next().unwrap_or("subtitle.zip").to_string();
        extract_archive(&bytes, &filename, &request.language, &self.staging_root).await
    }
}

fn parse_search_results(html: &str) -> Result<Vec<SubtitleSearchResult>, String> {
    let doc = Html::parse_document(html);
    let mut results = Vec::new();

    let sub_box_sel = Selector::parse("div.sub_box")
        .map_err(|_| "legendasdivx: selector error")?;
    let th_sel = Selector::parse("th").map_err(|_| "legendasdivx: th selector error")?;
    let _td_sel = Selector::parse("td").map_err(|_| "legendasdivx: td selector error")?;
    let desc_sel = Selector::parse("td.td_desc").map_err(|_| "legendasdivx: desc selector error")?;
    let footer_sel = Selector::parse("div.sub_footer").map_err(|_| "legendasdivx: footer selector error")?;
    let download_sel = Selector::parse("a.sub_download").map_err(|_| "legendasdivx: download selector error")?;
    let header_sel = Selector::parse("div.sub_header").map_err(|_| "legendasdivx: header selector error")?;
    let a_sel = Selector::parse("a").map_err(|_| "legendasdivx: a selector error")?;
    let img_sel = Selector::parse("img").map_err(|_| "legendasdivx: img selector error")?;

    for (idx, sub_box) in doc.select(&sub_box_sel).enumerate() {
        let mut hits: u64 = 0;
        let mut language = "pt".to_string();
        let mut description = String::new();

        // Parse table headers
        for th in sub_box.select(&th_sel) {
            let th_text = th.text().collect::<String>();
            if let Some(next_td) = th.next_sibling_element() {
                if th_text.contains("Hits") {
                    hits = next_td.text().collect::<String>().trim().parse().unwrap_or(0);
                } else if th_text.contains("Idioma") {
                    if let Some(img) = next_td.select(&img_sel).next() {
                        let src = img.value().attr("src").unwrap_or("").to_lowercase();
                        if src.contains("brazil") {
                            language = "pt-BR".into();
                        } else if src.contains("portugal") {
                            language = "pt".into();
                        }
                    }
                }
            }
        }

        // Description
        if let Some(desc_el) = sub_box.select(&desc_sel).next() {
            description = desc_el.text().collect::<String>().trim().to_string();
        }

        // Download link from footer
        let Some(footer) = sub_box.select(&footer_sel).next() else { continue };
        let Some(dl_anchor) = footer.select(&download_sel).next() else { continue };
        let href = dl_anchor.value().attr("href").unwrap_or("");
        if href.is_empty() { continue }

        let download_url = if href.starts_with("http") {
            href.to_string()
        } else {
            format!("{SITE}/modules.php{href}")
        };

        // Uploader from header
        let uploader = sub_box.select(&header_sel).next()
            .and_then(|h| h.select(&a_sel).next())
            .map(|a| a.text().collect::<String>())
            .unwrap_or_else(|| "anonymous".into());

        let lang_name = if language == "pt-BR" { "Portuguese (Brazilian)" } else { "Portuguese" };

        results.push(SubtitleSearchResult {
            id: format!("legendasdivx_{idx}"),
            name: description.lines().next().unwrap_or("").trim().to_string(),
            language: language.clone(),
            language_name: lang_name.into(),
            format: "srt".into(),
            provider: "legendasdivx".into(),
            detail_path: Some(download_url.clone()),
            download_path: Some(download_url),
            download_count: Some(hits),
            rating: None,
            movie_name: None,
            release_group: Some(uploader),
        });
    }

    tracing::info!("legendasdivx: parsed {} results", results.len());
    Ok(results)
}
