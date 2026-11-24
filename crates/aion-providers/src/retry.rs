use std::future::Future;
use std::time::Duration;

use reqwest::header::HeaderMap;
use serde_json::Value;

use crate::error::ProviderError;

/// Retry a fallible async operation with exponential backoff
pub async fn with_retry<F, Fut, T>(max_retries: u32, f: F) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut backoff = Duration::from_secs(1);
    for attempt in 0..=max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) if e.is_retryable() && attempt < max_retries => {
                tracing::warn!(attempt = attempt + 1, max_retries, error = %e, "retrying request");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

pub const MAX_STREAM_RETRIES: u32 = 2;
pub const MAX_INITIAL_CONNECT_RETRIES: u32 = 2;
const INITIAL_HTTP_5XX_RETRY_BACKOFFS: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
];
const MAX_BACKOFF: Duration = Duration::from_secs(15);
const INITIAL_CONNECT_BACKOFF: Duration = Duration::from_millis(300);
const MAX_INITIAL_CONNECT_BACKOFF: Duration = Duration::from_secs(2);

/// Retry initial request failures that occur before an HTTP response exists.
/// HTTP status errors and rate limits are intentionally not retried here.
pub async fn with_initial_connect_retry<F, Fut, T>(f: F) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut backoff = INITIAL_CONNECT_BACKOFF;
    for attempt in 0..=MAX_INITIAL_CONNECT_RETRIES {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) if is_initial_connect_error(&e) && attempt < MAX_INITIAL_CONNECT_RETRIES => {
                tracing::warn!(
                    attempt = attempt + 1,
                    max_retries = MAX_INITIAL_CONNECT_RETRIES,
                    error = %e,
                    "retrying initial provider request after connect failure"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_INITIAL_CONNECT_BACKOFF);
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

fn is_initial_connect_error(error: &ProviderError) -> bool {
    match error {
        ProviderError::Http(err) => err.is_connect(),
        ProviderError::Connection(_) => true,
        _ => false,
    }
}

/// Waits before retrying a rate-limited request. Several agents sharing one provider account
/// (a team on one relay) exceed a requests-per-minute limit together; the limit clears within
/// a minute or two, so failing the turn after the first 429 stops the whole team.
const INITIAL_RATE_LIMIT_RETRY_BACKOFFS: [Duration; 6] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(60),
];
const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(120);

/// Retry transient provider-side failures before stream consumption starts: HTTP 5xx
/// responses, and rate limits other than an exhausted quota.
pub(crate) async fn with_initial_status_retry<F, Fut, T>(f: F) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut server_errors = 0;
    let mut rate_limits = 0;
    loop {
        let error = match f().await {
            Ok(val) => return Ok(val),
            Err(error) => error,
        };
        if let Some(status) = initial_http_5xx_status(&error)
            && let Some(backoff) = INITIAL_HTTP_5XX_RETRY_BACKOFFS.get(server_errors)
        {
            server_errors += 1;
            tracing::warn!(
                attempt = server_errors,
                max_retries = INITIAL_HTTP_5XX_RETRY_BACKOFFS.len(),
                status,
                "retrying initial provider request after server error"
            );
            tokio::time::sleep(*backoff).await;
            continue;
        }
        if let Some(wait) = initial_rate_limit_wait(&error, rate_limits) {
            rate_limits += 1;
            tracing::warn!(
                attempt = rate_limits,
                max_retries = INITIAL_RATE_LIMIT_RETRY_BACKOFFS.len(),
                wait_ms = wait.as_millis() as u64,
                "retrying initial provider request after rate limit"
            );
            tokio::time::sleep(wait).await;
            continue;
        }
        return Err(error);
    }
}

/// How long to wait before retry `attempt` of a rate-limited request, or `None` when it
/// should fail now: the retries are used up, or the provider says the quota is exhausted.
fn initial_rate_limit_wait(error: &ProviderError, attempt: usize) -> Option<Duration> {
    let ProviderError::RateLimited { retry_after_ms, body } = error else {
        return None;
    };
    if body.as_deref().is_some_and(|body| body.contains("insufficient_quota")) {
        return None;
    }
    let backoff = INITIAL_RATE_LIMIT_RETRY_BACKOFFS.get(attempt)?;
    let wait = (*backoff)
        .max(Duration::from_millis(*retry_after_ms))
        .min(MAX_RATE_LIMIT_WAIT);
    Some(wait + retry_jitter())
}

/// Up to two seconds, so agents limited together do not retry in the same instant.
fn retry_jitter() -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    Duration::from_millis(u64::from(nanos % 2000))
}

fn initial_http_5xx_status(error: &ProviderError) -> Option<u16> {
    match error {
        ProviderError::Api { status, .. } if (500..=599).contains(status) => Some(*status),
        _ => None,
    }
}

/// Send an HTTP request and check status, returning the response on success.
/// Used by provider-specific retry loops to avoid duplicating request logic.
pub async fn send_and_check(
    client: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    body: &Value,
) -> Result<reqwest::Response, ProviderError> {
    let response = client
        .post(url)
        .headers(headers.clone())
        .json(body)
        .send()
        .await
        .map_err(|e| ProviderError::Connection(e.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        return Err(ProviderError::Api {
            status: status.as_u16(),
            message: body_text,
        });
    }

    Ok(response)
}

/// Sleep with exponential backoff and log the retry attempt.
/// Returns the next backoff duration.
pub async fn backoff_sleep(attempt: u32, current_backoff: Duration) -> Duration {
    tracing::warn!(
        attempt,
        max = MAX_STREAM_RETRIES,
        "retrying stream after mid-stream disconnect"
    );
    tokio::time::sleep(current_backoff).await;
    (current_backoff * 2).min(MAX_BACKOFF)
}

#[cfg(test)]
#[path = "retry_test.rs"]
mod retry_test;
