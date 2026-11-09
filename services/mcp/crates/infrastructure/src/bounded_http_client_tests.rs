use std::sync::Arc;

use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn configured_json_limit_keeps_the_rmcp_sixteen_mib_hard_cap() {
    let http_client = reqwest::Client::new();
    let clamped_client =
        BoundedReqwestClient::new(http_client.clone(), MAX_JSON_RESPONSE_HARD_LIMIT_BYTES + 1);
    let configured_client = BoundedReqwestClient::new(http_client, 1024);

    assert_eq!(
        clamped_client.max_json_response_bytes,
        MAX_JSON_RESPONSE_HARD_LIMIT_BYTES
    );
    assert_eq!(configured_client.max_json_response_bytes, 1024);
}

#[tokio::test]
async fn rejects_oversized_json_before_deserializing_the_response() {
    let response_limit = 1024;
    let response_body = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"tools\":[]}}}}{}",
        " ".repeat(response_limit),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定测试 HTTP server");
    let server_address = listener.local_addr().expect("读取测试 HTTP 地址");
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("接受测试请求");
        let mut request_headers = Vec::new();
        let mut request_chunk = [0_u8; 1024];
        while !request_headers
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            let read_length = stream.read(&mut request_chunk).await.expect("读取请求头");
            if read_length == 0 {
                return;
            }
            request_headers.extend_from_slice(&request_chunk[..read_length]);
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await
            .expect("写入响应头");
        stream
            .write_all(format!("{:X}\r\n", response_body.len()).as_bytes())
            .await
            .expect("写入 chunk 长度");
        stream
            .write_all(response_body.as_bytes())
            .await
            .expect("写入响应体");
        let _ = stream.write_all(b"\r\n0\r\n\r\n").await;
    });

    let http_client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("创建测试 reqwest client");
    let bounded_client = BoundedReqwestClient::new(http_client, response_limit);
    let request_message = serde_json::from_str::<ClientJsonRpcMessage>(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    )
    .expect("构造 tools/list 请求");
    let error = bounded_client
        .post_message(
            Arc::from(format!("http://{server_address}/mcp")),
            request_message,
            None,
            None,
            HashMap::new(),
        )
        .await
        .expect_err("超限 JSON 必须在反序列化前拒绝");

    assert!(matches!(
        error,
        StreamableHttpError::ResponseBodyTooLarge { max_size: 1024 }
    ));
    server_task.await.expect("测试 HTTP server 正常结束");
}

#[tokio::test]
async fn does_not_treat_json_rpc_error_as_success_for_unauthorized_http_response() {
    let response_body =
        r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32001,"message":"unauthorized"}}"#;
    let response_length = response_body.len();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定测试 HTTP server");
    let server_address = listener.local_addr().expect("读取测试 HTTP 地址");
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("接受测试请求");
        let mut request_headers = Vec::new();
        let mut request_chunk = [0_u8; 1024];
        while !request_headers
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            let read_length = stream.read(&mut request_chunk).await.expect("读取请求头");
            if read_length == 0 {
                return;
            }
            request_headers.extend_from_slice(&request_chunk[..read_length]);
        }
        stream
            .write_all(
                format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {response_length}\r\nConnection: close\r\n\r\n{response_body}"
                )
                .as_bytes(),
            )
            .await
            .expect("写入未授权响应");
    });

    let http_client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("创建测试 reqwest client");
    let bounded_client = BoundedReqwestClient::new(http_client, 1024);
    let request_message = serde_json::from_str::<ClientJsonRpcMessage>(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    )
    .expect("构造 tools/list 请求");
    let error = bounded_client
        .post_message(
            Arc::from(format!("http://{server_address}/mcp")),
            request_message,
            None,
            None,
            HashMap::new(),
        )
        .await
        .expect_err("HTTP 401 不得伪装成普通 JSON-RPC 响应");

    assert!(matches!(
        error,
        StreamableHttpError::UnexpectedServerResponse(_)
    ));
    server_task.await.expect("测试 HTTP server 正常结束");
}
