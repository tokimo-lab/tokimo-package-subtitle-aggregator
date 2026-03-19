use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};

use crate::models::{
    SubtitleDownloadRequest, SubtitleDownloadResponse, SubtitleSearchRequest, SubtitleSearchResult,
};
use crate::providers::SubtitleProvider;

/// Per-provider search timeout. Prevents a slow provider from blocking all results.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(30);

pub struct SubtitleAggregator {
    providers: Vec<Arc<dyn SubtitleProvider>>,
}

impl SubtitleAggregator {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn add_provider(&mut self, provider: Arc<dyn SubtitleProvider>) {
        self.providers.push(provider);
    }

    /// Concurrently search all providers, merge results.
    /// Each provider is given PROVIDER_TIMEOUT seconds before being cancelled.
    pub async fn search(
        &self,
        request: &SubtitleSearchRequest,
    ) -> Result<Vec<SubtitleSearchResult>, String> {
        // Wrap in Arc so each spawned task increments a ref-count instead of cloning all strings.
        let request = Arc::new(request.clone());
        let mut handles = Vec::with_capacity(self.providers.len());

        for provider in &self.providers {
            let provider = Arc::clone(provider);
            let request = Arc::clone(&request);
            let handle = tokio::spawn(async move {
                let name = provider.name();
                let fut = provider.search(&request);
                match tokio::time::timeout(PROVIDER_TIMEOUT, fut).await {
                    Ok(Ok(results)) => {
                        tracing::info!("[{name}] 搜索到 {} 条字幕", results.len());
                        results
                    }
                    Ok(Err(error)) => {
                        tracing::warn!("[{name}] 搜索失败: {error}");
                        Vec::new()
                    }
                    Err(_) => {
                        tracing::warn!("[{name}] 搜索超时 ({}s)", PROVIDER_TIMEOUT.as_secs());
                        Vec::new()
                    }
                }
            });
            handles.push(handle);
        }

        let mut all_results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(results) => all_results.extend(results),
                Err(error) => tracing::warn!("Provider 任务崩溃: {error}"),
            }
        }

        Ok(all_results)
    }

    /// Download from the specific provider
    pub async fn download(
        &self,
        request: &SubtitleDownloadRequest,
    ) -> Result<SubtitleDownloadResponse, String> {
        let provider = self
            .providers
            .iter()
            .find(|p| p.name() == request.provider)
            .ok_or_else(|| format!("未找到 provider: {}", request.provider))?;

        let downloaded = provider.download(request).await?;

        Ok(SubtitleDownloadResponse {
            name: downloaded.name,
            format: downloaded.format,
            content_base64: BASE64.encode(&downloaded.content),
        })
    }

    /// List available provider names
    pub fn provider_names(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.name()).collect()
    }
}

impl Default for SubtitleAggregator {
    fn default() -> Self {
        Self::new()
    }
}
