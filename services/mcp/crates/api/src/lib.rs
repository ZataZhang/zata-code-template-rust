//! MCP 服务 HTTP 接入层：REST 管理接口、调用方认证与 Streamable HTTP MCP 网关。

pub mod dto;
pub mod error;
pub mod extract;
pub mod mcp_gateway;
pub mod routes;
pub mod server;
