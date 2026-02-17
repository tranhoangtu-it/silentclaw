use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::warn;

/// Abstraction for text → vector embedding providers.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
}

/// OpenAI embedding provider using text-embedding-3-small (1536 dims).
pub struct OpenAIEmbedding {
    client: reqwest::Client,
    api_key: String,
    model: String,
    dims: usize,
}

impl OpenAIEmbedding {
    pub fn new(api_key: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.to_string(),
            model: "text-embedding-3-small".to_string(),
            dims: 1536,
        }
    }

    pub fn with_model(mut self, model: &str, dims: usize) -> Self {
        self.model = model.to_string();
        self.dims = dims;
        self
    }
}

#[derive(Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbeddingProvider for OpenAIEmbedding {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let results = self.embed_batch(&[text.to_string()]).await?;
        results
            .into_iter()
            .next()
            .context("Empty embedding response")
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let max_retries = 3u32;
        let mut attempt = 0;

        loop {
            let body = EmbeddingRequest {
                model: self.model.clone(),
                input: texts.to_vec(),
            };

            let resp = self
                .client
                .post("https://api.openai.com/v1/embeddings")
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await;

            match resp {
                Ok(r) if r.status().is_success() => {
                    let data: EmbeddingResponse =
                        r.json().await.context("Failed to parse embedding response")?;
                    return Ok(data.data.into_iter().map(|d| d.embedding).collect());
                }
                Ok(r) => {
                    let status = r.status();
                    let text = r.text().await.unwrap_or_default();
                    if attempt < max_retries && (status.is_server_error() || status.as_u16() == 429) {
                        let delay = Duration::from_secs(2u64.pow(attempt));
                        warn!(attempt, %status, "Embedding API error, retrying in {:?}", delay);
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                    } else {
                        anyhow::bail!("Embedding API error {}: {}", status, text);
                    }
                }
                Err(e) => {
                    if attempt < max_retries {
                        let delay = Duration::from_secs(2u64.pow(attempt));
                        warn!(attempt, error = %e, "Embedding request failed, retrying in {:?}", delay);
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                    } else {
                        return Err(e).context("Embedding API request failed after retries");
                    }
                }
            }
        }
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

/// Mock embedding provider for testing — returns deterministic vectors.
#[cfg(test)]
pub struct MockEmbedding {
    dims: usize,
}

#[cfg(test)]
impl MockEmbedding {
    pub fn new(dims: usize) -> Self {
        Self { dims }
    }
}

#[cfg(test)]
#[async_trait]
impl EmbeddingProvider for MockEmbedding {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        // Deterministic hash-based vector for testing
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(text.as_bytes());
        let vec: Vec<f32> = (0..self.dims)
            .map(|i| {
                let byte = hash[i % 32] as f32;
                (byte / 255.0) * 2.0 - 1.0 // normalize to [-1, 1]
            })
            .collect();
        Ok(vec)
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}
