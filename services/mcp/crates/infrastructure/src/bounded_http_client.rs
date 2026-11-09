//! 在 rmcp 反序列化前限制远端 JSON 响应大小。

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use bytes::Bytes;
use futures_util::{Stream, StreamExt, stream};
use reqwest::header::{
    ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, RETRY_AFTER, WWW_AUTHENTICATE,
};
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, HttpStatusError, InsufficientScopeError, SseError, StreamableHttpClient,
    StreamableHttpError, StreamableHttpPostResponse,
};
use sse_stream::{Sse, SseStream};

const MAX_ERROR_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_RETRY_AFTER_BYTES: usize = 128;
const SESSION_ID_HEADER: &str = "mcp-session-id";
const LAST_EVENT_ID_HEADER: &str = "last-event-id";

/// 给 rmcp reqwest transport 增加可配置的 JSON 响应体上限。
#[derive(Clone)]
pub(crate) struct BoundedReqwestClient {
    http_client: reqwest::Client,
    max_json_response_bytes: usize,
}

impl BoundedReqwestClient {
    pub(crate) fn new(http_client: reqwest::Client, max_json_response_bytes: usize) -> Self {
        Self {
            http_client,
            max_json_response_bytes,
        }
    }

    async fn post_message_bounded(
        &self,
        post_request: PostRequest,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<reqwest::Error>> {
        let session_was_attached = post_request.session_id.is_some();
        let mut request_builder = self
            .http_client
            .post(post_request.uri.as_ref())
            .header(ACCEPT, "text/event-stream, application/json");
        if let Some(auth_header) = post_request.auth_header {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        if let Some(session_id) = post_request.session_id {
            request_builder = request_builder.header(SESSION_ID_HEADER, session_id.as_ref());
        }
        for (header_name, header_value) in post_request.custom_headers {
            if is_reserved_header(&header_name) {
                return Err(StreamableHttpError::ReservedHeaderConflict(
                    header_name.to_string(),
                ));
            }
            request_builder = request_builder.header(header_name, header_value);
        }

        let response = request_builder
            .json(&post_request.message)
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;
        let response_status = response.status();
        let session_id = response
            .headers()
            .get(SESSION_ID_HEADER)
            .and_then(|header_value| header_value.to_str().ok())
            .map(str::to_owned);
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|header_value| header_value.to_str().ok())
            .map(str::to_owned);
        let is_json_response = content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("application/json"));
        let status_error = http_status_error(response_status, response.headers());

        if response_status == reqwest::StatusCode::UNAUTHORIZED
            && let Some(challenge) = response.headers().get(WWW_AUTHENTICATE)
        {
            let challenge = header_text(challenge)?;
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                challenge,
            )));
        }
        if response_status == reqwest::StatusCode::FORBIDDEN
            && let Some(challenge) = response.headers().get(WWW_AUTHENTICATE)
        {
            let challenge = header_text(challenge)?;
            let scope = scope_from_challenge(&challenge);
            return Err(StreamableHttpError::InsufficientScope(
                InsufficientScopeError::new(challenge, scope),
            ));
        }
        if matches!(
            response_status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if response_status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }

        if !response_status.is_success() {
            if !is_json_response || response_status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(status_error
                    .map(StreamableHttpError::from)
                    .unwrap_or_else(|| {
                        StreamableHttpError::UnexpectedServerResponse(
                            format!("HTTP {response_status}").into(),
                        )
                    }));
            }

            let error_body_limit = self.max_json_response_bytes.min(MAX_ERROR_RESPONSE_BYTES);
            let response_body = match read_bounded_json_body(response, error_body_limit).await {
                Ok(response_body) => response_body,
                Err(StreamableHttpError::ResponseBodyTooLarge { .. }) => {
                    return Err(status_error
                        .map(StreamableHttpError::from)
                        .unwrap_or_else(|| StreamableHttpError::ResponseBodyTooLarge {
                            max_size: error_body_limit,
                        }));
                }
                Err(error) => {
                    if let Some(status_error) = status_error {
                        return Err(StreamableHttpError::from(status_error));
                    }
                    return Err(error);
                }
            };
            if let Ok(ServerJsonRpcMessage::Error(error)) =
                serde_json::from_slice::<ServerJsonRpcMessage>(&response_body)
            {
                return Ok(StreamableHttpPostResponse::Json(
                    ServerJsonRpcMessage::Error(error),
                    session_id,
                ));
            }
            return Err(status_error
                .map(StreamableHttpError::from)
                .unwrap_or_else(|| {
                    StreamableHttpError::UnexpectedServerResponse(
                        format!("HTTP {response_status}").into(),
                    )
                }));
        }

        match content_type.as_deref() {
            Some(value) if value.starts_with("text/event-stream") => {
                let sse_bytes =
                    limit_sse_event_bytes(response.bytes_stream(), post_request.max_sse_event_size);
                let sse_stream = SseStream::from_bytes_stream(sse_bytes).boxed();
                Ok(StreamableHttpPostResponse::Sse(sse_stream, session_id))
            }
            Some(value) if value.starts_with("application/json") => {
                let response_body =
                    read_bounded_json_body(response, self.max_json_response_bytes).await?;
                match serde_json::from_slice::<ServerJsonRpcMessage>(&response_body) {
                    Ok(message) => Ok(StreamableHttpPostResponse::Json(message, session_id)),
                    Err(_error)
                        if !matches!(post_request.message, ClientJsonRpcMessage::Request(_)) =>
                    {
                        Ok(StreamableHttpPostResponse::Accepted)
                    }
                    Err(error) => Err(StreamableHttpError::Deserialize(error)),
                }
            }
            _ => Err(StreamableHttpError::UnexpectedContentType(content_type)),
        }
    }
}

impl StreamableHttpClient for BoundedReqwestClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.post_message_bounded(PostRequest {
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
            max_sse_event_size: self.max_json_response_bytes,
        })
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.post_message_bounded(PostRequest {
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
            max_sse_event_size,
        })
        .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        <reqwest::Client as StreamableHttpClient>::delete_session(
            &self.http_client,
            uri,
            session_id,
            auth_header,
            custom_headers,
        )
        .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<
        futures_util::stream::BoxStream<'static, Result<Sse, SseError>>,
        StreamableHttpError<Self::Error>,
    > {
        <reqwest::Client as StreamableHttpClient>::get_stream_with_max_sse_event_size(
            &self.http_client,
            uri,
            session_id,
            last_event_id,
            auth_header,
            custom_headers,
            self.max_json_response_bytes,
        )
        .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<
        futures_util::stream::BoxStream<'static, Result<Sse, SseError>>,
        StreamableHttpError<Self::Error>,
    > {
        <reqwest::Client as StreamableHttpClient>::get_stream_with_max_sse_event_size(
            &self.http_client,
            uri,
            session_id,
            last_event_id,
            auth_header,
            custom_headers,
            max_sse_event_size,
        )
        .await
    }
}

struct PostRequest {
    uri: Arc<str>,
    message: ClientJsonRpcMessage,
    session_id: Option<Arc<str>>,
    auth_header: Option<String>,
    custom_headers: HashMap<HeaderName, HeaderValue>,
    max_sse_event_size: usize,
}

async fn read_bounded_json_body(
    response: reqwest::Response,
    max_size: usize,
) -> Result<Vec<u8>, StreamableHttpError<reqwest::Error>> {
    if response
        .content_length()
        .is_some_and(|content_length| content_length > max_size as u64)
    {
        return Err(StreamableHttpError::ResponseBodyTooLarge { max_size });
    }

    let mut response_bytes = Vec::new();
    let mut response_stream = response.bytes_stream();
    while let Some(next_chunk) = response_stream.next().await {
        let next_chunk = next_chunk.map_err(StreamableHttpError::Client)?;
        if next_chunk.len() > max_size.saturating_sub(response_bytes.len()) {
            return Err(StreamableHttpError::ResponseBodyTooLarge { max_size });
        }
        response_bytes.extend_from_slice(&next_chunk);
    }
    Ok(response_bytes)
}

fn http_status_error(status: reqwest::StatusCode, headers: &HeaderMap) -> Option<HttpStatusError> {
    if status != reqwest::StatusCode::TOO_MANY_REQUESTS && !status.is_server_error() {
        return None;
    }
    let retry_after = headers
        .get(RETRY_AFTER)
        .filter(|header_value| header_value.as_bytes().len() <= MAX_RETRY_AFTER_BYTES)
        .and_then(|header_value| header_value.to_str().ok())
        .map(str::to_owned);
    Some(HttpStatusError::new(status.as_u16(), retry_after))
}

fn header_text(header_value: &HeaderValue) -> Result<String, StreamableHttpError<reqwest::Error>> {
    header_value.to_str().map(str::to_owned).map_err(|_| {
        StreamableHttpError::UnexpectedServerResponse(
            "invalid www-authenticate header value".into(),
        )
    })
}

fn scope_from_challenge(challenge: &str) -> Option<String> {
    let lowercase_challenge = challenge.to_ascii_lowercase();
    let scope_start = lowercase_challenge.find("scope=")? + "scope=".len();
    let scope_value = challenge[scope_start..].trim_start();
    if let Some(quoted_scope) = scope_value.strip_prefix('"') {
        return quoted_scope
            .split_once('"')
            .map(|(scope, _remainder)| scope.to_owned());
    }
    let scope_end = scope_value
        .find(|character: char| character == ',' || character == ';' || character.is_whitespace())
        .unwrap_or(scope_value.len());
    (scope_end > 0).then(|| scope_value[..scope_end].to_owned())
}

fn is_reserved_header(header_name: &HeaderName) -> bool {
    matches!(
        header_name.as_str(),
        "accept" | SESSION_ID_HEADER | LAST_EVENT_ID_HEADER
    )
}

fn limit_sse_event_bytes<S>(
    response_stream: S,
    max_event_size: usize,
) -> impl Stream<Item = Result<Bytes, io::Error>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin + Send + 'static,
{
    stream::unfold(
        (
            response_stream,
            SseEventSizeCounter::new(max_event_size),
            false,
        ),
        |(mut response_stream, mut event_size_counter, failed)| async move {
            if failed {
                return None;
            }
            let next_chunk = response_stream.next().await?;
            match next_chunk {
                Ok(chunk) if event_size_counter.observe(&chunk).is_ok() => {
                    Some((Ok(chunk), (response_stream, event_size_counter, false)))
                }
                Ok(_chunk) => Some((
                    Err(io::Error::other(
                        "SSE event exceeded the configured response limit",
                    )),
                    (response_stream, event_size_counter, true),
                )),
                Err(error) => Some((
                    Err(io::Error::other(error)),
                    (response_stream, event_size_counter, true),
                )),
            }
        },
    )
}

struct SseEventSizeCounter {
    max_size: usize,
    retained_size: usize,
    line_size: usize,
    line_is_comment: bool,
    previous_was_cr: bool,
}

impl SseEventSizeCounter {
    fn new(max_size: usize) -> Self {
        Self {
            max_size,
            retained_size: 0,
            line_size: 0,
            line_is_comment: false,
            previous_was_cr: false,
        }
    }

    fn observe(&mut self, chunk: &[u8]) -> Result<(), ()> {
        for &byte in chunk {
            if self.previous_was_cr {
                self.previous_was_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => {
                    self.finish_line()?;
                    self.previous_was_cr = true;
                }
                b'\n' => self.finish_line()?,
                _ => {
                    if self.line_size == 0 {
                        self.line_is_comment = byte == b':';
                    }
                    self.line_size = self.line_size.saturating_add(1);
                    self.check_limit()?;
                }
            }
        }
        Ok(())
    }

    fn finish_line(&mut self) -> Result<(), ()> {
        if self.line_size == 0 {
            self.retained_size = 0;
        } else if !self.line_is_comment {
            self.retained_size = self
                .retained_size
                .saturating_add(self.line_size)
                .saturating_add(1);
        }
        self.line_size = 0;
        self.line_is_comment = false;
        self.check_limit()
    }

    fn check_limit(&self) -> Result<(), ()> {
        if self.retained_size.saturating_add(self.line_size) > self.max_size {
            Err(())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "bounded_http_client_tests.rs"]
mod tests;
