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
    done_reason: DoneReason,
    #[serde(default)]
    total_duration: u64,
    #[serde(default)]
    eval_duration: u64,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(from = "String")]
enum DoneReason {
    Stop,
    Length,
    Other(String),
}

impl From<String> for DoneReason {
    fn from(reason: String) -> Self {
        match reason.as_str() {
            "stop" => Self::Stop,
            "length" => Self::Length,
            _ => Self::Other(reason),
        }
    }
}

#[derive(Debug)]
enum Verdict {
    Accept(String),
    Retry,
    Fail(SummaryError),
}

/// Decide what to do with one generation attempt. `retry_allowance` is the
/// larger output allowance available for a retry, if any.
fn classify(
    generated: &GenerateResponse,
    num_ctx: u32,
    allowance: u32,
    retry_allowance: Option<u32>,
) -> Verdict {
    let prompt_tokens = generated.prompt_eval_count;
    if prompt_tokens.saturating_add(generated.eval_count) > num_ctx
        || prompt_tokens.saturating_add(allowance) > num_ctx
    {
        return Verdict::Fail(SummaryError::ContextOverflow);
    }
    match &generated.done_reason {
        DoneReason::Length
            if retry_allowance
                .is_some_and(|retry| prompt_tokens.saturating_add(retry) <= num_ctx) =>
        {
            Verdict::Retry
        }
        DoneReason::Length => Verdict::Fail(SummaryError::Truncated),
        DoneReason::Other(_) => Verdict::Fail(SummaryError::Incomplete),
        DoneReason::Stop => match generated.response.trim() {
            "" => Verdict::Fail(SummaryError::EmptyResponse),
            text => Verdict::Accept(text.to_string()),
        },
    }
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
        let retry_allowance = num_predict.saturating_mul(2);
        let retry_fits = budget.fits(&prompt, retry_allowance);
        for (attempt, allowance) in [num_predict, retry_allowance].into_iter().enumerate() {
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
                duration_ms, eval_ms, done_reason = ?generated.done_reason, "summary token usage");
            let retry = (attempt == 0 && retry_fits).then_some(retry_allowance);
            match classify(&generated, budget.num_ctx, allowance, retry) {
                Verdict::Accept(summary) => return Ok(summary),
                Verdict::Retry => {
                    warn!(
                        model,
                        output_tokens = generated.eval_count,
                        num_predict = allowance,
                        retry_num_predict = retry_allowance,
                        "summary generation truncated, retrying"
                    );
                }
                Verdict::Fail(error) => {
                    error!(model, prompt_tokens = generated.prompt_eval_count,
                        output_tokens = generated.eval_count, num_predict = allowance,
                        num_ctx = budget.num_ctx, done_reason = ?generated.done_reason,
                        %error, "summary generation failed");
                    return Err(error);
                }
            }
        }
        Err(SummaryError::Truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(text: &str, prompt: u32, output: u32, done_reason: &str) -> GenerateResponse {
        serde_json::from_value(serde_json::json!({
            "response": text,
            "prompt_eval_count": prompt,
            "eval_count": output,
            "done_reason": done_reason,
        }))
        .unwrap()
    }

    #[test]
    fn done_reason_parses_known_and_unknown_values() {
        assert_eq!(response("", 0, 0, "stop").done_reason, DoneReason::Stop);
        assert_eq!(response("", 0, 0, "length").done_reason, DoneReason::Length);
        assert_eq!(
            response("", 0, 0, "unload").done_reason,
            DoneReason::Other("unload".into())
        );
    }

    #[test]
    fn completed_response_is_accepted_trimmed() {
        let verdict = classify(&response("  summary \n", 100, 50, "stop"), 1000, 200, None);
        assert!(
            matches!(verdict, Verdict::Accept(ref text) if text == "summary"),
            "{verdict:?}"
        );
    }

    #[test]
    fn blank_response_is_empty() {
        let verdict = classify(&response(" \n", 100, 1, "stop"), 1000, 200, None);
        assert!(
            matches!(verdict, Verdict::Fail(SummaryError::EmptyResponse)),
            "{verdict:?}"
        );
    }

    #[test]
    fn tokens_beyond_context_overflow() {
        let used = classify(&response("x", 900, 200, "stop"), 1000, 50, None);
        assert!(
            matches!(used, Verdict::Fail(SummaryError::ContextOverflow)),
            "{used:?}"
        );
        let reserved = classify(&response("x", 900, 10, "stop"), 1000, 200, None);
        assert!(
            matches!(reserved, Verdict::Fail(SummaryError::ContextOverflow)),
            "{reserved:?}"
        );
    }

    #[test]
    fn truncated_response_retries_when_larger_allowance_fits() {
        let verdict = classify(&response("x", 100, 200, "length"), 1000, 200, Some(400));
        assert!(matches!(verdict, Verdict::Retry), "{verdict:?}");
    }

    #[test]
    fn truncated_response_fails_without_retry_or_when_retry_overflows() {
        let no_retry = classify(&response("x", 100, 200, "length"), 1000, 200, None);
        assert!(
            matches!(no_retry, Verdict::Fail(SummaryError::Truncated)),
            "{no_retry:?}"
        );
        let too_big = classify(&response("x", 700, 200, "length"), 1000, 200, Some(400));
        assert!(
            matches!(too_big, Verdict::Fail(SummaryError::Truncated)),
            "{too_big:?}"
        );
    }

    #[test]
    fn unknown_done_reason_is_incomplete() {
        let verdict = classify(&response("x", 100, 50, "unload"), 1000, 200, None);
        assert!(
            matches!(verdict, Verdict::Fail(SummaryError::Incomplete)),
            "{verdict:?}"
        );
    }
}
