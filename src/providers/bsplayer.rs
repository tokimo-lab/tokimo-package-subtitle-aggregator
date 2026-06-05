use async_trait::async_trait;

use super::SubtitleProvider;
use crate::models::{
    DownloadedSubtitle, SubtitleDownloadRequest, SubtitleSearchRequest, SubtitleSearchResult,
};

const BSPLAYER_API_URL: &str = "http://api.bsplayer-subtitles.com/v1.php";
const BSPLAYER_USER_AGENT: &str = "BSPlayer/2.x (1022.12360)";

pub struct BsPlayerProvider;

impl Default for BsPlayerProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl BsPlayerProvider {
    pub fn new() -> Self {
        Self
    }

    fn build_client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .user_agent(BSPLAYER_USER_AGENT)
            .build()
            .map_err(|e| format!("Failed to build BSPlayer HTTP client: {e}"))
    }

    fn build_soap_envelope(search_url: &str, func_name: &str, params: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<SOAP-ENV:Envelope xmlns:SOAP-ENV="http://schemas.xmlsoap.org/soap/envelope/" xmlns:SOAP-ENC="http://schemas.xmlsoap.org/soap/encoding/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:ns1="{search_url}">
<SOAP-ENV:Body SOAP-ENV:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<ns1:{func_name}>{params}</ns1:{func_name}>
</SOAP-ENV:Body></SOAP-ENV:Envelope>"#,
        )
    }

    async fn api_request(
        client: &reqwest::Client,
        func_name: &str,
        params: &str,
    ) -> Result<String, String> {
        let envelope = Self::build_soap_envelope(BSPLAYER_API_URL, func_name, params);
        let soap_action = format!("\"http://api.bsplayer-subtitles.com/v1.php#{func_name}\"");

        let response = client
            .post(BSPLAYER_API_URL)
            .header("Content-Type", "text/xml; charset=utf-8")
            .header("SOAPAction", soap_action)
            .header("Connection", "close")
            .body(envelope)
            .send()
            .await
            .map_err(|e| format!("BSPlayer API request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "BSPlayer API returned HTTP {}",
                response.status().as_u16()
            ));
        }

        response
            .text()
            .await
            .map_err(|e| format!("Failed to read BSPlayer response: {e}"))
    }

    async fn login(client: &reqwest::Client) -> Result<String, String> {
        let params = "<username></username><password></password><AppID>BSPlayer v2.67</AppID>";
        let xml = Self::api_request(client, "logIn", params).await?;

        // Extract token from <data>TOKEN</data>
        let token = Self::extract_xml_text(&xml, "data")
            .ok_or_else(|| "BSPlayer login failed: no token in response".to_string())?;

        let status = Self::extract_xml_text(&xml, "status").unwrap_or_default();
        if status != "OK" {
            return Err(format!("BSPlayer login failed: status={status}"));
        }

        Ok(token)
    }

    async fn logout(client: &reqwest::Client, token: &str) {
        let params = format!("<handle>{token}</handle>");
        let _ = Self::api_request(client, "logOut", &params).await;
    }

    fn extract_xml_text(xml: &str, tag: &str) -> Option<String> {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        let start = xml.find(&open)? + open.len();
        let end = xml[start..].find(&close)?;
        Some(xml[start..start + end].trim().to_string())
    }

    fn map_language(opensubtitles_code: &str) -> (&'static str, &'static str) {
        match opensubtitles_code {
            "ara" => ("ar", "Arabic"),
            "bul" => ("bg", "Bulgarian"),
            "ces" => ("cs", "Czech"),
            "dan" => ("da", "Danish"),
            "deu" | "ger" => ("de", "German"),
            "ell" => ("el", "Greek"),
            "eng" => ("en", "English"),
            "fin" => ("fi", "Finnish"),
            "fra" | "fre" => ("fr", "French"),
            "hun" => ("hu", "Hungarian"),
            "ita" => ("it", "Italian"),
            "jpn" => ("ja", "Japanese"),
            "kor" => ("ko", "Korean"),
            "nld" | "dut" => ("nl", "Dutch"),
            "pol" => ("pl", "Polish"),
            "por" => ("pt", "Portuguese"),
            "pob" => ("pt-BR", "Portuguese (Brazil)"),
            "ron" | "rum" => ("ro", "Romanian"),
            "rus" => ("ru", "Russian"),
            "spa" => ("es", "Spanish"),
            "swe" => ("sv", "Swedish"),
            "tur" => ("tr", "Turkish"),
            "ukr" => ("uk", "Ukrainian"),
            "zho" | "chi" => ("zh", "Chinese"),
            _ => ("und", "Unknown"),
        }
    }
}

#[async_trait]
impl SubtitleProvider for BsPlayerProvider {
    fn name(&self) -> &str {
        "bsplayer"
    }

    async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        let file_hash = request
            .file_hash
            .as_deref()
            .ok_or("BSPlayer search requires file_hash")?;
        let file_size = request
            .file_size
            .ok_or("BSPlayer search requires file_size")?;

        let imdb_id = request.imdb_id.as_deref().unwrap_or("*");
        let language_ids = request
            .languages
            .as_deref()
            .map(|langs| {
                langs
                    .iter()
                    .map(|l| match l.as_str() {
                        "en" => "eng",
                        "de" => "deu",
                        "fr" => "fra",
                        "es" => "spa",
                        "it" => "ita",
                        "ru" => "rus",
                        "tr" => "tur",
                        "bg" => "bul",
                        "pl" => "pol",
                        "cs" => "ces",
                        "zh" => "zho",
                        "ja" => "jpn",
                        "ko" => "kor",
                        other => other,
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_else(|| "eng".to_string());

        let client = Self::build_client()?;
        let token = Self::login(&client).await?;

        let params = format!(
            "<handle>{token}</handle>\
             <movieHash>{file_hash}</movieHash>\
             <movieSize>{file_size}</movieSize>\
             <languageId>{language_ids}</languageId>\
             <imdbId>{imdb_id}</imdbId>"
        );

        let xml = Self::api_request(&client, "searchSubtitles", &params).await?;
        Self::logout(&client, &token).await;

        // Check status
        let status = Self::extract_xml_text(&xml, "status").unwrap_or_default();
        if status != "OK" {
            return Ok(Vec::new());
        }

        // Parse items
        let mut results = Vec::new();
        // Split by <item> blocks
        let item_open = "<item>";
        let item_close = "</item>";
        let mut pos = 0;
        let mut index = 0;

        while let Some(start) = xml[pos..].find(item_open) {
            let abs_start = pos + start + item_open.len();
            if let Some(end) = xml[abs_start..].find(item_close) {
                let item_xml = &xml[abs_start..abs_start + end];

                let sub_id = Self::extract_xml_text(item_xml, "subID")
                    .unwrap_or_else(|| format!("bsplayer-{index}"));
                let download_link =
                    Self::extract_xml_text(item_xml, "subDownloadLink").unwrap_or_default();
                let sub_lang = Self::extract_xml_text(item_xml, "subLang").unwrap_or_default();
                let sub_name = Self::extract_xml_text(item_xml, "subName")
                    .unwrap_or_else(|| format!("subtitle_{index}"));
                let sub_format = Self::extract_xml_text(item_xml, "subFormat")
                    .unwrap_or_else(|| "srt".to_string())
                    .to_ascii_lowercase();

                let (lang_code, lang_name) = Self::map_language(&sub_lang);

                if !download_link.is_empty() {
                    results.push(SubtitleSearchResult {
                        id: sub_id.clone(),
                        name: sub_name.clone(),
                        language: lang_code.to_string(),
                        language_name: lang_name.to_string(),
                        format: sub_format,
                        provider: "bsplayer".into(),
                        detail_path: None,
                        download_path: Some(download_link),
                        download_count: None,
                        rating: None,
                        movie_name: None,
                        release_group: Some(sub_name),
                    });
                }

                pos = abs_start + end + item_close.len();
                index += 1;
            } else {
                break;
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
            .ok_or("BSPlayer download requires download_path")?;

        let client = reqwest::Client::builder()
            .user_agent("Mozilla/4.0 (compatible; Synapse)")
            .build()
            .map_err(|e| format!("Failed to build BSPlayer download client: {e}"))?;

        let response = client
            .get(download_url)
            .send()
            .await
            .map_err(|e| format!("BSPlayer download failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "BSPlayer download failed: HTTP {}",
                response.status().as_u16()
            ));
        }

        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read BSPlayer download: {e}"))?;

        // BSPlayer serves gzip-compressed subtitle files
        let decompressed = Self::decompress_gzip(&content).unwrap_or_else(|_| content.to_vec());

        let format = request.format.clone();
        let name = request
            .name
            .clone()
            .unwrap_or_else(|| format!("subtitle.{format}"));

        Ok(DownloadedSubtitle {
            name,
            format,
            content: decompressed,
        })
    }
}

impl BsPlayerProvider {
    fn decompress_gzip(data: &[u8]) -> Result<Vec<u8>, String> {
        use std::io::Read;
        let mut decoder = flate2::read::GzDecoder::new(data);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|e| format!("Gzip decompress failed: {e}"))?;
        Ok(out)
    }
}
