# 对象存储

工作区共享 crate `tpl-platform-oss` 将 MinIO 与阿里云 OSS 收敛到同一套异步读写 API。它是构建期复用的 Cargo package；每个业务服务独立连接对象存储，不经过其他微服务转发。

## 本地开发：zata-ops MinIO

zata-ops 的本机测试栈将 MinIO API 发布在 `http://127.0.0.1:9000`，Console 在 `http://127.0.0.1:9001`。在 Console 里先创建 bucket，例如 `rust-study-dev`。本地栈使用 `minioadmin` / `minioadmin` 测试凭据，只能用于本机开发。

供服务的 TOML 配置可使用：

```toml
[object_storage]
backend = "minio"
endpoint = "http://127.0.0.1:9000"
region = "us-east-1"
bucket = "rust-study-dev"
prefix = "accounts"
operation_timeout_secs = 5
io_timeout_secs = 60
max_buffered_bytes = 8388608
max_chunk_bytes = 1048576
```

把本地凭据放在服务自己的 `.env.local`，不要写入 TOML 或提交到 Git：

```env
APP_OBJECT_STORAGE__ACCESS_KEY_ID=minioadmin
APP_OBJECT_STORAGE__ACCESS_KEY_SECRET=minioadmin
```

Rust 进程在宿主机运行时用 `127.0.0.1`。如果容器运行在和 MinIO 共享的 Docker 网络里，则将 endpoint 改成该网络中的 MinIO 服务名；不要假设另一份 Compose 自动能解析 `minio`。

## 线上：阿里云 OSS

在服务自己的配置中选择 `aliyun_oss`，endpoint 使用 bucket 所在地域的 HTTPS 地址：

```toml
[object_storage]
backend = "aliyun_oss"
endpoint = "https://oss-cn-hangzhou.aliyuncs.com"
bucket = "your-production-bucket"
prefix = "accounts"
operation_timeout_secs = 5
io_timeout_secs = 60
max_buffered_bytes = 8388608
max_chunk_bytes = 1048576
```

通过部署平台的密钥管理注入 `APP_OBJECT_STORAGE__ACCESS_KEY_ID` 和 `APP_OBJECT_STORAGE__ACCESS_KEY_SECRET`。每个服务使用独立 RAM 子账号或最小权限凭据，并按服务配置不同的 bucket 或 `prefix`。下载签名 URL 是临时访问凭据，不要写入日志。

## 在微服务中接入

把 `tpl-platform-oss.workspace = true` 加到需要存储能力的服务 `infrastructure` crate。由该服务自己的配置结构读取 `ObjectStorageConfig`，再在 composition root 创建客户端；core 定义本服务所需的存储端口，infrastructure adapter 调用共享客户端实现端口。

```rust
use serde::Deserialize;
use tpl_platform_oss::{ObjectStorageClient, ObjectStorageConfig, ObjectStorageError};

#[derive(Deserialize, Default)]
#[serde(default)]
struct Settings {
    object_storage: ObjectStorageConfig,
}

async fn store_avatar(settings: Settings, image_bytes: Vec<u8>) -> Result<(), ObjectStorageError> {
    let storage_client = ObjectStorageClient::new(settings.object_storage)?;
    storage_client
        .write("profiles/user-123/avatar.png", image_bytes)
        .await
}
```

`read` 与 `write` 的整块缓冲大小默认上限为 8 MiB。大对象使用 `open_reader` / `open_writer` 分块处理；分块默认上限为 1 MiB。配置最多可将缓冲操作提高到 64 MiB、分块提高到 8 MiB。所有操作都受超时配置约束。

跨后端对象 key（含服务 `prefix`）最多 1,023 字节，采用阿里云 OSS 与 S3 兼容后端共同支持的上限。[阿里云 OSS 对象命名限制](https://help.aliyun.com/en/oss/user-guide/object-overview)、[Amazon S3 对象 key 限制](https://docs.aws.amazon.com/AmazonS3/latest/userguide/object-keys.html)。

服务应把 `ObjectStorageError` 映射到自己的错误码，不要把底层 SDK 错误详情直接返回给 HTTP 客户端。

当前 accounts 与 agent-tracing 尚无对象存储业务用例，因此共享客户端暂未装配到两个服务的运行状态。新增文件存储功能时，应只在实际使用该能力的服务配置与 composition root 中接入。
