use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use super::{
    ContextBudget, InputTooLarge, SUMMARY_CONTEXT_TOKENS, SUMMARY_SYSTEM_PROMPT, SummaryError,
    SummaryGenerator, SummaryUsage,
};

pub(super) const OLLAMA_REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Clone)]
pub(crate) struct OllamaSummaryGenerator {
    http: reqwest::Client,
    ollama_url: String,
}

impl OllamaSummaryGenerator {
    pub(crate) fn new(http: reqwest::Client, ollama_url: String) -> Self {
        Self { http, ollama_url }
    }
}

#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    system: &'static str,
    prompt: &'a str,
    stream: bool,
    think: bool,
    options: GenerateOptions,
}

#[derive(Serialize)]
struct GenerateOptions {
    num_ctx: u32,
    num_predict: u32,
    temperature: f32,
}

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
    prompt_eval_count: u32,
    eval_count: u32,
    done_reason: String,
    #[serde(default)]
    total_duration: u64,
    #[serde(default)]
    eval_duration: u64,
}

#[derive(Deserialize)]
struct ShowResponse {
    model_info: serde_json::Map<String, serde_json::Value>,
}

impl SummaryGenerator for OllamaSummaryGenerator {
    async fn context_budget(&self, model: &str) -> Result<ContextBudget, SummaryError> {
        let response = self
            .http
            .post(format!(
                "{}/api/show",
                self.ollama_url.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(30))
            .json(&serde_json::json!({"model": model}))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|err| {
                error!(model, error = ?err, "failed to fetch Ollama model metadata");
                SummaryError::ModelUnavailable
            })?;
        let metadata: ShowResponse = response.json().await.map_err(|err| {
            error!(%err, "invalid Ollama model metadata");
            SummaryError::InvalidModelMetadata
        })?;
        let model_context = metadata
            .model_info
            .iter()
            .filter(|(key, _)| key.ends_with(".context_length"))
            .filter_map(|(_, value)| value.as_u64())
            .min()
            .filter(|limit| *limit > 0)
            .ok_or(SummaryError::MissingContextLimit)?;
        let num_ctx = model_context.min(u64::from(SUMMARY_CONTEXT_TOKENS)) as u32;
        info!(
            model,
            model_context, num_ctx, "summary model context budget"
        );
        Ok(ContextBudget { num_ctx })
    }

    async fn generate(
        &self,
        model: &str,
        budget: ContextBudget,
        prompt: String,
        num_predict: u32,
        usage: &mut SummaryUsage,
    ) -> Result<String, SummaryError> {
        if !budget.fits(&prompt, num_predict) {
            return Err(InputTooLarge.into());
        }
        for (attempt, allowance) in [num_predict, num_predict.saturating_mul(2)]
            .into_iter()
            .enumerate()
        {
            info!(
                model,
                estimated_input_tokens = budget.estimated_input_tokens(&prompt),
                num_ctx = budget.num_ctx,
                num_predict = allowance,
                attempt = attempt + 1,
                "generating summary within context budget"
            );
            let request = GenerateRequest {
                model,
                system: SUMMARY_SYSTEM_PROMPT,
                prompt: &prompt,
                stream: false,
                think: false,
                options: GenerateOptions {
                    num_ctx: budget.num_ctx,
                    num_predict: allowance,
                    temperature: 0.0,
                },
            };
            let response = self.http
                .post(format!("{}/api/generate", self.ollama_url.trim_end_matches('/')))
                .timeout(OLLAMA_REQUEST_TIMEOUT)
                .json(&request)
                .send().await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|err| {
                    error!(model, error = ?err, timeout = err.is_timeout(), timeout_seconds = OLLAMA_REQUEST_TIMEOUT.as_secs(), "Ollama request failed");
                    SummaryError::ServiceUnavailable
                })?;
            let generated: GenerateResponse = response.json().await.map_err(|err| {
                error!(error = ?err, timeout = err.is_timeout(), "invalid Ollama response");
                SummaryError::InvalidResponse
            })?;
            let duration_ms = (generated.total_duration / 1_000_000) as i64;
            let eval_ms = (generated.eval_duration / 1_000_000) as i64;
            usage.llm_calls += 1;
            usage.prompt_tokens += i64::from(generated.prompt_eval_count);
            usage.output_tokens += i64::from(generated.eval_count);
            usage.llm_ms += duration_ms;
            usage.eval_ms += eval_ms;
            info!(model, prompt_tokens = generated.prompt_eval_count,
                output_tokens = generated.eval_count, num_ctx = budget.num_ctx,
                duration_ms, eval_ms, done_reason = %generated.done_reason, "summary token usage");
            if generated
                .prompt_eval_count
                .saturating_add(generated.eval_count)
                > budget.num_ctx
                || generated.prompt_eval_count.saturating_add(allowance) > budget.num_ctx
            {
                error!(
                    model,
                    prompt_tokens = generated.prompt_eval_count,
                    output_tokens = generated.eval_count,
                    num_predict = allowance,
                    num_ctx = budget.num_ctx,
                    "summary generation exceeded context budget"
                );
                return Err(SummaryError::ContextOverflow);
            }
            if generated.done_reason == "length" {
                let retry_allowance = num_predict.saturating_mul(2);
                let retry = attempt == 0
                    && budget.fits(&prompt, retry_allowance)
                    && generated.prompt_eval_count.saturating_add(retry_allowance)
                        <= budget.num_ctx;
                warn!(
                    model,
                    output_tokens = generated.eval_count,
                    num_predict = allowance,
                    retry,
                    retry_num_predict = retry_allowance,
                    "summary generation truncated"
                );
                if retry {
                    continue;
                }
                return Err(SummaryError::Truncated);
            }
            if generated.done_reason != "stop" {
                error!(model, done_reason = %generated.done_reason, "summary generation incomplete");
                return Err(SummaryError::Incomplete);
            }
            if generated.response.trim().is_empty() {
                return Err(SummaryError::EmptyResponse);
            }
            return Ok(generated.response.trim().to_string());
        }
        Err(SummaryError::Truncated)
    }
}
