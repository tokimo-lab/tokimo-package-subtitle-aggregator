use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use super::SubtitleProvider;
use crate::archive::extract_archive;
use crate::models::{
    normalize_format, DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest,
    SubtitleSearchResult,
};

const SUBSOURCE_API_BASE: &str = "https://api.subsource.net/api/v1/";
const SUBSOURCE_USER_AGENT: &str = "subtitle-aggregator v0.1.0";

/// Map ISO 639-1 / ISO 639-2 language codes to SubSource full language names.
fn to_subsource_language(lang: &str) -> Option<&'static str> {
    match lang {
        "en" | "eng" => Some("English"),
        "fa" | "fas" | "per" => Some("Farsi_persian"),
        "af" | "afr" => Some("Afrikaans"),
        "sq" | "sqi" | "alb" => Some("Albanian"),
        "am" | "amh" => Some("Amharic"),
        "ar" | "ara" => Some("Arabic"),
        "hy" | "hye" | "arm" => Some("Armenian"),
        "as" | "asm" => Some("Assamese"),
        "az" | "aze" => Some("Azerbaijani"),
        "eu" | "eus" | "baq" => Some("Basque"),
        "be" | "bel" => Some("Belarusian"),
        "bn" | "ben" => Some("Bengali"),
        "bs" | "bos" => Some("Bosnian"),
        "pt-BR" | "pt-br" => Some("Brazillian Portuguese"),
        "br" | "bre" => Some("Breton"),
        "bg" | "bul" => Some("Bulgarian"),
        "my" | "mya" | "bur" => Some("Burmese"),
        "ca" | "cat" => Some("Catalan"),
        "zh" | "zho" | "chi" | "zh-CN" | "zh-cn" | "zh-TW" | "zh-tw" => Some("Chinese BG code"),
        "hr" | "hrv" => Some("Croatian"),
        "cs" | "ces" | "cze" => Some("Czech"),
        "da" | "dan" => Some("Danish"),
        "nl" | "nld" | "dut" => Some("Dutch"),
        "eo" | "epo" => Some("Espranto"),
        "et" | "est" => Some("Estonian"),
        "fi" | "fin" => Some("Finnish"),
        "fr" | "fra" | "fre" => Some("French"),
        "gd" | "gla" => Some("Gaelic"),
        "ka" | "kat" | "geo" => Some("Georgian"),
        "de" | "deu" | "ger" => Some("German"),
        "el" | "ell" | "gre" => Some("Greek"),
        "he" | "heb" => Some("Hebrew"),
        "hi" | "hin" => Some("Hindi"),
        "hu" | "hun" => Some("Hungarian"),
        "is" | "isl" | "ice" => Some("Icelandic"),
        "ig" | "ibo" => Some("Igbo"),
        "id" | "ind" => Some("Indonesian"),
        "ia" | "ina" => Some("Interlingua"),
        "ga" | "gle" => Some("Irish"),
        "it" | "ita" => Some("Italian"),
        "ja" | "jpn" => Some("Japanese"),
        "kn" | "kan" => Some("Kannada"),
        "kk" | "kaz" => Some("Kazakh"),
        "km" | "khm" => Some("Khmer"),
        "ko" | "kor" => Some("Korean"),
        "ku" | "kur" => Some("Kurdish"),
        "lv" | "lav" => Some("Latvian"),
        "lt" | "lit" => Some("Lithuanian"),
        "lb" | "ltz" => Some("Luxembourgish"),
        "mk" | "mkd" | "mac" => Some("Macedonian"),
        "ms" | "msa" | "may" => Some("Malay"),
        "ml" | "mal" => Some("Malayalam"),
        "mr" | "mar" => Some("Marathi"),
        "mn" | "mon" => Some("Mongolian"),
        "nv" | "nav" => Some("Navajo"),
        "ne" | "nep" => Some("Nepali"),
        "se" | "sme" => Some("Northen Sami"),
        "no" | "nor" => Some("Norwegian"),
        "oc" | "oci" => Some("Occitan"),
        "pl" | "pol" => Some("Polish"),
        "pt" | "por" => Some("Portuguese"),
        "ps" | "pus" => Some("Pushto"),
        "ro" | "ron" | "rum" => Some("Romanian"),
        "ru" | "rus" => Some("Russian"),
        "sr" | "srp" => Some("Serbian"),
        "sd" | "snd" => Some("Sindhi"),
        "si" | "sin" => Some("Sinhala"),
        "sk" | "slk" | "slo" => Some("Slovak"),
        "sl" | "slv" => Some("Slovenian"),
        "so" | "som" => Some("Somali"),
        "es" | "spa" => Some("Spanish"),
        "sw" | "swa" => Some("Swahili"),
        "sv" | "swe" => Some("Swedish"),
        "tl" | "tgl" => Some("Tagalog"),
        "ta" | "tam" => Some("Tamil"),
        "tt" | "tat" => Some("Tatar"),
        "te" | "tel" => Some("Telugu"),
        "th" | "tha" => Some("Thai"),
        "tr" | "tur" => Some("Turkish"),
        "tk" | "tuk" => Some("Turkmen"),
        "uk" | "ukr" => Some("Ukrainian"),
        "ur" | "urd" => Some("Urdu"),
        "uz" | "uzb" => Some("Uzbek"),
        "vi" | "vie" => Some("Vietnamese"),
        "cy" | "cym" | "wel" => Some("Welsh"),
        _ => None,
    }
}

/// Map SubSource full language names back to ISO 639-1 codes.
fn from_subsource_language(name: &str) -> String {
    match name {
        "English" => "en".into(),
        "Farsi_persian" => "fa".into(),
        "Afrikaans" => "af".into(),
        "Albanian" => "sq".into(),
        "Amharic" => "am".into(),
        "Arabic" => "ar".into(),
        "Armenian" => "hy".into(),
        "Assamese" => "as".into(),
        "Azerbaijani" => "az".into(),
        "Basque" => "eu".into(),
        "Belarusian" => "be".into(),
        "Bengali" => "bn".into(),
        "Bosnian" => "bs".into(),
        "Brazillian Portuguese" => "pt-BR".into(),
        "Breton" => "br".into(),
        "Bulgarian" => "bg".into(),
        "Burmese" => "my".into(),
        "Catalan" => "ca".into(),
        "Chinese BG code" => "zh".into(),
        "Croatian" => "hr".into(),
        "Czech" => "cs".into(),
        "Danish" => "da".into(),
        "Dutch" => "nl".into(),
        "Espranto" => "eo".into(),
        "Estonian" => "et".into(),
        "Finnish" => "fi".into(),
        "French" => "fr".into(),
        "Gaelic" => "gd".into(),
        "Georgian" => "ka".into(),
        "German" => "de".into(),
        "Greek" => "el".into(),
        "Hebrew" => "he".into(),
        "Hindi" => "hi".into(),
        "Hungarian" => "hu".into(),
        "Icelandic" => "is".into(),
        "Igbo" => "ig".into(),
        "Indonesian" => "id".into(),
        "Interlingua" => "ia".into(),
        "Irish" => "ga".into(),
        "Italian" => "it".into(),
        "Japanese" => "ja".into(),
        "Kannada" => "kn".into(),
        "Kazakh" => "kk".into(),
        "Khmer" => "km".into(),
        "Korean" => "ko".into(),
        "Kurdish" => "ku".into(),
        "Latvian" => "lv".into(),
        "Lithuanian" => "lt".into(),
        "Luxembourgish" => "lb".into(),
        "Macedonian" => "mk".into(),
        "Malay" => "ms".into(),
        "Malayalam" => "ml".into(),
        "Marathi" => "mr".into(),
        "Mongolian" => "mn".into(),
        "Navajo" => "nv".into(),
        "Nepali" => "ne".into(),
        "Northen Sami" => "se".into(),
        "Norwegian" => "no".into(),
        "Occitan" => "oc".into(),
        "Polish" => "pl".into(),
        "Portuguese" => "pt".into(),
        "Pushto" => "ps".into(),
        "Romanian" => "ro".into(),
        "Russian" => "ru".into(),
        "Serbian" => "sr".into(),
        "Sindhi" => "sd".into(),
        "Sinhala" => "si".into(),
        "Slovak" => "sk".into(),
        "Slovenian" => "sl".into(),
        "Somali" => "so".into(),
        "Spanish" => "es".into(),
        "Swahili" => "sw".into(),
        "Swedish" => "sv".into(),
        "Tagalog" => "tl".into(),
        "Tamil" => "ta".into(),
        "Tatar" => "tt".into(),
        "Telugu" => "te".into(),
        "Thai" => "th".into(),
        "Turkish" => "tr".into(),
        "Turkmen" => "tk".into(),
        "Ukrainian" => "uk".into(),
        "Urdu" => "ur".into(),
        "Uzbek" => "uz".into(),
        "Vietnamese" => "vi".into(),
        "Welsh" => "cy".into(),
        other => other.to_ascii_lowercase(),
    }
}

fn from_subsource_language_name(name: &str) -> String {
    match name {
        "English" => "English".into(),
        "Farsi_persian" => "فارسی".into(),
        "Arabic" => "العربية".into(),
        "Chinese BG code" => "中文".into(),
        "French" => "Français".into(),
        "German" => "Deutsch".into(),
        "Italian" => "Italiano".into(),
        "Japanese" => "日本語".into(),
        "Korean" => "한국어".into(),
        "Portuguese" => "Português".into(),
        "Brazillian Portuguese" => "Português (BR)".into(),
        "Russian" => "Русский".into(),
        "Spanish" => "Español".into(),
        other => other.into(),
    }
}

// ── SubSource API response types ──

#[derive(Debug, Deserialize)]
struct SubsourceSearchResponse {
    #[serde(default)]
    data: Vec<SubsourceMovieItem>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SubsourceMovieItem {
    #[serde(rename = "movieId")]
    movie_id: serde_json::Value,
    title: Option<String>,
    #[serde(rename = "releaseYear", default)]
    release_year: Option<serde_json::Value>,
    #[serde(rename = "alternateTitle", default)]
    alternate_title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SubsourceSubtitlesResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    data: Vec<SubsourceSubtitleItem>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SubsourceSubtitleItem {
    #[serde(rename = "subtitleId")]
    subtitle_id: serde_json::Value,
    language: String,
    link: Option<String>,
    #[serde(rename = "releaseInfo", default)]
    release_info: Vec<String>,
    #[serde(rename = "hearingImpaired", default)]
    hearing_impaired: Option<bool>,
    #[serde(rename = "foreignParts", default)]
    foreign_parts: Option<bool>,
    #[serde(default)]
    commentary: Option<String>,
    #[serde(default)]
    contributors: Vec<SubsourceContributor>,
    #[serde(rename = "uploaderId", default)]
    uploader_id: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SubsourceContributor {
    id: Option<serde_json::Value>,
    displayname: Option<String>,
}

pub struct SubSourceProvider {
    api_key: Option<String>,
    staging_root: PathBuf,
}

impl SubSourceProvider {
    pub fn new(api_key: Option<String>) -> Self {
        let api_key = api_key.or_else(|| std::env::var("SUBSOURCE_API_KEY").ok());
        Self {
            api_key,
            staging_root: std::env::temp_dir(),
        }
    }

    pub fn with_staging_root(mut self, staging_root: impl Into<PathBuf>) -> Self {
        self.staging_root = staging_root.into();
        self
    }

    fn build_client(&self) -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .user_agent(SUBSOURCE_USER_AGENT)
            .build()
            .map_err(|e| format!("创建 SubSource HTTP 客户端失败: {e}"))
    }

    fn api_key_param(&self) -> Vec<(&'static str, String)> {
        if let Some(key) = &self.api_key {
            vec![("api_key", key.clone())]
        } else {
            vec![]
        }
    }

    /// Search for a movie/show by title or IMDB ID and return its internal movie_id.
    async fn search_movie_id(
        &self,
        client: &reqwest::Client,
        title: &str,
        imdb_id: Option<&str>,
    ) -> Result<Option<String>, String> {
        let mut params = self.api_key_param();

        if let Some(imdb) = imdb_id {
            params.push(("searchType", "imdb".into()));
            params.push(("imdb", imdb.to_string()));
        } else {
            params.push(("searchType", "text".into()));
            params.push(("q", title.to_ascii_lowercase()));
        }

        let url = format!("{SUBSOURCE_API_BASE}movies/search");
        let resp = client
            .get(&url)
            .query(&params)
            .send()
            .await
            .map_err(|e| format!("SubSource 搜索请求失败: {e}"))?;

        self.check_status(&resp)?;

        let search_resp: SubsourceSearchResponse = resp
            .json()
            .await
            .map_err(|e| format!("解析 SubSource 搜索结果失败: {e}"))?;

        // If IMDB search returned nothing, fall back to text search
        let results = if imdb_id.is_some() && search_resp.data.is_empty() {
            tracing::debug!("SubSource: IMDB 搜索无结果，尝试文本搜索: {title}");
            let mut fallback_params = self.api_key_param();
            fallback_params.push(("searchType", "text".into()));
            fallback_params.push(("q", title.to_ascii_lowercase()));

            let resp2 = client
                .get(&url)
                .query(&fallback_params)
                .send()
                .await
                .map_err(|e| format!("SubSource 文本搜索失败: {e}"))?;
            self.check_status(&resp2)?;
            resp2
                .json::<SubsourceSearchResponse>()
                .await
                .map_err(|e| format!("解析 SubSource 文本搜索结果失败: {e}"))?
                .data
        } else {
            search_resp.data
        };

        let title_lower = title.to_ascii_lowercase();
        for result in &results {
            let Some(result_title) = &result.title else {
                continue;
            };
            let mut titles_to_check = vec![result_title.to_ascii_lowercase()];
            if let Some(alt) = &result.alternate_title {
                titles_to_check.push(alt.to_ascii_lowercase());
            }

            let matched = titles_to_check.iter().any(|t| t.contains(&title_lower));
            if matched {
                let movie_id = match &result.movie_id {
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                return Ok(Some(movie_id));
            }
        }

        // Looser match: take first result if available
        if let Some(first) = results.first() {
            let movie_id = match &first.movie_id {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            return Ok(Some(movie_id));
        }

        Ok(None)
    }

    fn check_status(&self, resp: &reqwest::Response) -> Result<(), String> {
        match resp.status().as_u16() {
            200 => Ok(()),
            400 => Err("SubSource: 无效请求参数 (400)".into()),
            401 => Err("SubSource: 认证失败 (401)".into()),
            403 => Err("SubSource: 访问被拒绝 (403)".into()),
            429 => Err("SubSource: 请求过于频繁 (429)".into()),
            code => Err(format!("SubSource: HTTP 错误 ({code})")),
        }
    }
}

#[async_trait]
impl SubtitleProvider for SubSourceProvider {
    fn name(&self) -> &str {
        "subsource"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let client = self.build_client()?;

        let title = request
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("");

        let imdb_id = request.imdb_id.as_deref();

        if title.is_empty() && imdb_id.is_none() {
            return Err("SubSource 搜索需要提供 query 或 imdb_id".into());
        }

        let movie_id = self
            .search_movie_id(&client, title, imdb_id)
            .await?;

        let Some(movie_id) = movie_id else {
            tracing::debug!("SubSource: 未找到匹配的影片");
            return Ok(vec![]);
        };

        tracing::debug!("SubSource: 找到影片 ID {movie_id}");

        // Determine languages to query
        let languages_to_query: Vec<String> = request
            .languages
            .as_deref()
            .map(|langs| {
                langs
                    .iter()
                    .filter_map(|l| to_subsource_language(l).map(|n| n.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // If no recognizable languages, query without language filter
        let lang_list = if languages_to_query.is_empty() {
            vec![None]
        } else {
            languages_to_query.iter().map(|l| Some(l.clone())).collect()
        };

        let mut all_results: Vec<SubtitleSearchResult> = Vec::new();

        for lang_name in lang_list {
            let mut params = self.api_key_param();
            params.push(("limit", "100".into()));
            params.push(("movieId", movie_id.clone()));
            if let Some(ref lang) = lang_name {
                params.push(("language", lang.to_ascii_lowercase()));
            }

            let url = format!("{SUBSOURCE_API_BASE}subtitles");
            let resp = client
                .get(&url)
                .query(&params)
                .send()
                .await
                .map_err(|e| format!("SubSource 字幕列表请求失败: {e}"))?;

            self.check_status(&resp)?;

            let subs_resp: SubsourceSubtitlesResponse = resp
                .json()
                .await
                .map_err(|e| format!("解析 SubSource 字幕列表失败: {e}"))?;

            if subs_resp.success == Some(false) {
                continue;
            }

            tracing::info!(
                "SubSource 语言 {:?} 返回 {} 条字幕",
                lang_name,
                subs_resp.data.len()
            );

            for item in subs_resp.data {
                let subtitle_id = match &item.subtitle_id {
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };

                let page_link = item.link.as_deref().map(|l| {
                    if l.starts_with("http") {
                        l.to_string()
                    } else {
                        format!("https://subsource.net{l}")
                    }
                });

                let language_name_str = item.language.clone();
                let language_code = from_subsource_language(&language_name_str);
                let language_display = from_subsource_language_name(&language_name_str);

                let release_group = if item.release_info.is_empty() {
                    None
                } else {
                    Some(item.release_info.join(", "))
                };

                // Derive a name from release info or subtitle_id
                let name = if !item.release_info.is_empty() {
                    item.release_info[0].clone()
                } else {
                    format!("subsource_{subtitle_id}")
                };

                let format = normalize_format(&name).unwrap_or_else(|| "srt".into());

                let download_path = format!(
                    "{SUBSOURCE_API_BASE}subtitles/{subtitle_id}/download"
                );

                all_results.push(SubtitleSearchResult {
                    id: subtitle_id,
                    name,
                    language: language_code,
                    language_name: language_display,
                    format,
                    provider: "subsource".into(),
                    detail_path: page_link,
                    download_path: Some(download_path),
                    download_count: None,
                    rating: None,
                    movie_name: None,
                    release_group,
                });
            }
        }

        Ok(all_results)
    }

    async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<DownloadedSubtitle, String> {
        let download_url = request
            .download_path
            .as_deref()
            .ok_or("SubSource 下载缺少 download_path")?;

        let params = self.api_key_param();
        let client = self.build_client()?;

        let resp = client
            .get(download_url)
            .query(&params)
            .send()
            .await
            .map_err(|e| format!("SubSource 下载请求失败: {e}"))?;

        match resp.status().as_u16() {
            429 => return Err("SubSource 请求过于频繁 (429)".into()),
            403 => return Err("SubSource 访问被拒绝 (403)".into()),
            401 => return Err("SubSource 认证失败 (401)".into()),
            200 => {}
            code => {
                let body = resp.text().await.unwrap_or_default();
                return Err(format!("SubSource 下载失败 ({code}): {body}"));
            }
        }

        let content = resp
            .bytes()
            .await
            .map_err(|e| format!("读取 SubSource 下载内容失败: {e}"))?;

        let archive_name = download_url
            .rsplit('/')
            .find(|s| !s.is_empty())
            .map(|s| {
                // Strip query params if any
                s.split('?').next().unwrap_or(s)
            })
            .unwrap_or("subtitle.zip");

        let archive_name = if archive_name.contains('.') {
            archive_name.to_string()
        } else {
            format!("{archive_name}.zip")
        };

        extract_archive(&content, &archive_name, &request.language, &self.staging_root).await
    }
}
