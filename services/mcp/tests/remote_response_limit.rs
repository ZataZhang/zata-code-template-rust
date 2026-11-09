//! 远端 MCP 响应上限的真实 HTTP 入口测试。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use rmcp::model::{
    Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::json;
use tpl_mcp_core::{RemoteMcpClient, RemoteMcpError, RemoteTarget};
use tpl_mcp_infrastructure::config::OutboundSettings;
use tpl_mcp_infrastructure::remote_mcp::RmcpRemoteClient;
use url::Url;
use uuid::Uuid;

struct PagedToolsServer {
    list_page_requests: Arc<AtomicUsize>,
}

impl ServerHandler for PagedToolsServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("response-limit-test", "0.1.0"))
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let requested_cursor = request.and_then(|params| params.cursor);
        let page_number = match requested_cursor.as_deref() {
            None => 0,
            Some("page-2") => 1,
            Some("page-3") => 2,
            _ => return Err(ErrorData::invalid_params("unexpected page cursor", None)),
        };
        self.list_page_requests.fetch_add(1, Ordering::SeqCst);

        let input_schema = json!({"type": "object"})
            .as_object()
            .cloned()
            .expect("测试输入 schema 是对象");
        let mut page = ListToolsResult::with_all_items(vec![Tool::new(
            format!("tool-{page_number}"),
            "x".repeat(600),
            input_schema,
        )]);
        page.next_cursor = match page_number {
            0 => Some("page-2".to_owned()),
            1 => Some("page-3".to_owned()),
            _ => None,
        };
        Ok(page)
    }
}

#[tokio::test]
async fn tools_list_stops_fetching_pages_when_cumulative_size_exceeds_configured_limit() {
    let list_page_requests = Arc::new(AtomicUsize::new(0));
    let server_list_page_requests = list_page_requests.clone();
    let upstream_service = StreamableHttpService::new(
        move || {
            Ok::<PagedToolsServer, std::io::Error>(PagedToolsServer {
                list_page_requests: server_list_page_requests.clone(),
            })
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let upstream_router = Router::new().nest_service("/mcp", upstream_service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地测试上游");
    let upstream_address = listener.local_addr().expect("读取本地测试上游地址");
    let upstream_task = tokio::spawn(async move {
        axum::serve(listener, upstream_router)
            .await
            .expect("运行本地测试上游");
    });

    let settings = OutboundSettings {
        max_response_bytes: 1024,
        ..OutboundSettings::default()
    };
    let remote_client = RmcpRemoteClient::new(&settings);
    let target = RemoteTarget {
        connection_id: Uuid::new_v4(),
        endpoint_url: Url::parse(&format!("http://{upstream_address}/mcp"))
            .expect("测试 endpoint 有效"),
        addresses: vec![SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            upstream_address.port(),
        )],
        credential: None,
    };

    let error = remote_client
        .list_tools(&target)
        .await
        .expect_err("超过配置累计上限的工具发现必须失败");
    assert_eq!(error, RemoteMcpError::Protocol);
    assert_eq!(
        list_page_requests.load(Ordering::SeqCst),
        2,
        "累计超限的第二页后应停止，不请求第三页"
    );

    upstream_task.abort();
}
