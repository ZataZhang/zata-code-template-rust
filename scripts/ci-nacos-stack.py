#!/usr/bin/env python3
"""用一次性 Nacos 和隔离 Compose 项目验证远端部署配置入口。"""

from __future__ import annotations

import base64
from dataclasses import dataclass, replace
import http.server
import json
import os
import secrets
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
NACOS_IMAGE = "nacos/nacos-server:v3.2.4"
NACOS_TIMEOUT_SECONDS = 240
SPECIAL_ADMIN_PASSWORD = 'literal = ${SHOULD_NOT_EXPAND} "quoted" with spaces'


@dataclass(frozen=True)
class NacosLocation:
    server_url: str
    namespace: str
    group: str
    app_data_id: str
    compose_data_id: str


@dataclass(frozen=True)
class CiFixture:
    nacos: NacosLocation
    project_name: str
    nacos_container: str
    nacos_port: int
    web_port: int
    admin_port: int
    application_port: int


@dataclass(frozen=True)
class TestSecrets:
    postgres_password: str
    redis_password: str
    admin_password: str
    nacos_token: str
    nacos_identity: str


@dataclass(frozen=True)
class RunOptions:
    redacted_values: tuple[str, ...] = ()
    include_failure_output: bool = False


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def free_distinct_ports(count: int) -> list[int]:
    ports: list[int] = []
    while len(ports) < count:
        candidate_port = free_port()
        if candidate_port not in ports:
            ports.append(candidate_port)
    return ports


def run_checked(
    arguments: list[str],
    *,
    environment: dict[str, str] | None = None,
    options: RunOptions = RunOptions(),
) -> str:
    completed = subprocess.run(
        arguments,
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        failure_output = ""
        if options.include_failure_output:
            failure_output = "\n".join(
                (completed.stdout + completed.stderr).splitlines()[-35:]
            )
            for secret_value in options.redacted_values:
                if secret_value:
                    failure_output = failure_output.replace(secret_value, "<redacted>")
        details = f"\n{failure_output}" if failure_output else ""
        raise RuntimeError(
            f"命令失败（退出码 {completed.returncode}）：{Path(arguments[0]).name}{details}"
        )
    return completed.stdout.strip()


def nacos_url(location: NacosLocation, endpoint: str) -> str:
    return f"{location.server_url.rstrip('/')}/nacos/{endpoint}"


def wait_for_nacos(location: NacosLocation) -> None:
    state_url = nacos_url(
        location,
        f"v3/admin/core/namespace?namespaceId={urllib.parse.quote(location.namespace)}",
    )
    deadline = time.monotonic() + NACOS_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(state_url, timeout=2) as response:
                if response.status == 200:
                    return
        except (OSError, urllib.error.URLError):
            time.sleep(2)
    raise RuntimeError("隔离 Nacos 未在时限内就绪")


def publish_config(location: NacosLocation, data_id: str, content: str) -> None:
    endpoint = nacos_url(location, "v3/admin/cs/config")
    body = urllib.parse.urlencode(
        {
            "namespaceId": location.namespace,
            "groupName": location.group,
            "dataId": data_id,
            "content": content,
        }
    ).encode()
    request = urllib.request.Request(
        endpoint,
        data=body,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        payload = json.load(response)
    if payload.get("code") != 0 or payload.get("data") is not True:
        raise RuntimeError("隔离 Nacos 未接受测试配置")


def wait_for_client_config(
    location: NacosLocation, data_id: str, expected_content: str
) -> None:
    endpoint = nacos_url(location, "v3/client/cs/config")
    query = urllib.parse.urlencode(
        {
            "namespaceId": location.namespace,
            "groupName": location.group,
            "dataId": data_id,
        }
    )
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(f"{endpoint}?{query}", timeout=5) as response:
                payload = json.load(response)
            if (
                payload.get("code") == 0
                and isinstance(payload.get("data"), dict)
                and payload["data"].get("content") == expected_content
            ):
                return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(1)
    raise RuntimeError("隔离 Nacos 未通过 Client API 返回刚发布的测试配置")


def compose_environment(
    fixture: CiFixture,
    locator_file: Path,
) -> dict[str, str]:
    environment = os.environ.copy()
    environment.update(
        {
            "NACOS_SERVER_ADDR": fixture.nacos.server_url,
            # 空值阻止 dotenvy 从本机 .env.local 回填真实认证凭据。
            "NACOS_USERNAME": "",
            "NACOS_PASSWORD": "",
            "NACOS_ACCESS_TOKEN": "",
            "APP_CONFIG_FILE": str(locator_file),
            "COMPOSE_PROJECT_NAME": fixture.project_name,
            "COMPOSE_WEB_PORT": str(fixture.web_port),
            "COMPOSE_ADMIN_PORT": str(fixture.admin_port),
        }
    )
    environment.pop("COMPOSE_ACCOUNTS_PORT", None)
    return environment


def write_locator(file_path: Path, location: NacosLocation) -> None:
    file_path.write_text(
        "[nacos]\n"
        f'namespace_id = "{location.namespace}"\n'
        f'group = "{location.group}"\n'
        f'app_data_id = "{location.app_data_id}"\n'
        f'compose_data_id = "{location.compose_data_id}"\n',
        encoding="utf-8",
    )


def properties_content(secrets_bundle: TestSecrets) -> str:
    return "\n".join(
        (
            "# isolated CI values",
            f"POSTGRES_PASSWORD={secrets_bundle.postgres_password}",
            f"REDIS_PASSWORD={secrets_bundle.redis_password}",
            "ADMIN_BOOTSTRAP_USERNAME=ci-admin",
            f"ADMIN_BOOTSTRAP_PASSWORD={secrets_bundle.admin_password}",
            "COOKIE_SECURE=false",
            "TRUSTED_PROXY_CIDR=127.0.0.1",
            "WEB_PORT=3000",
            "ADMIN_PORT=8081",
            "LOG_FORMAT=json",
            "",
        )
    )


def run_fail_closed_case(label: str, environment: dict[str, str]) -> None:
    with tempfile.TemporaryDirectory(prefix="compose-config-docker-probe-") as temporary:
        probe_root = Path(temporary)
        docker_probe = probe_root / "docker"
        marker_path = probe_root / "docker-was-called"
        docker_probe.write_text(
            "#!/bin/sh\n"
            f"touch {shlex_quote(str(marker_path))}\n"
            "exit 0\n",
            encoding="utf-8",
        )
        docker_probe.chmod(0o755)
        probe_environment = environment.copy()
        probe_environment["PATH"] = f"{probe_root}{os.pathsep}{environment['PATH']}"
        result = subprocess.run(
            ["./scripts/compose-config.sh", "compose", "full", "ps"],
            cwd=ROOT,
            env=probe_environment,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        if result.returncode == 0 or marker_path.exists():
            raise RuntimeError(f"Nacos 负控未 fail-closed：{label}")
    print(f"PASS  compose-config fail-closed：{label}")


def shlex_quote(value: str) -> str:
    return "'" + value.replace("'", "'\\''") + "'"


class UnauthorizedNacosHandler(http.server.BaseHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802
        self.send_response(401)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, _format: str, *_arguments: object) -> None:
        return


def test_auth_rejection(environment: dict[str, str]) -> None:
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), UnauthorizedNacosHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    auth_environment = environment.copy()
    auth_environment["NACOS_SERVER_ADDR"] = f"http://127.0.0.1:{server.server_port}"
    auth_environment["NACOS_USERNAME"] = "ci-probe"
    auth_environment["NACOS_PASSWORD"] = "ci-probe-secret"
    try:
        run_fail_closed_case("认证 HTTP 401 传输边界 stub", auth_environment)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


def env_map(container_id: str) -> dict[str, str]:
    encoded_environment = run_checked(
        ["docker", "inspect", "--format", "{{json .Config.Env}}", container_id]
    )
    return dict(item.split("=", 1) for item in json.loads(encoded_environment))


def find_service_container(project_name: str, service_name: str) -> str:
    return run_checked(
        [
            "docker",
            "ps",
            "--filter",
            f"label=com.docker.compose.project={project_name}",
            "--filter",
            f"label=com.docker.compose.service={service_name}",
            "--format",
            "{{.ID}}",
        ]
    ).splitlines()[0]


def verify_container_configuration(fixture: CiFixture, secrets_bundle: TestSecrets) -> None:
    postgres_environment = env_map(
        find_service_container(fixture.project_name, "postgres")
    )
    redis_environment = env_map(find_service_container(fixture.project_name, "redis"))
    accounts_environment = env_map(
        find_service_container(fixture.project_name, "accounts")
    )

    matches = (
        postgres_environment.get("POSTGRES_PASSWORD")
        == secrets_bundle.postgres_password
        and redis_environment.get("REDIS_PASSWORD")
        == secrets_bundle.redis_password
        and accounts_environment.get("ADMIN_BOOTSTRAP_USERNAME") == "ci-admin"
        and accounts_environment.get("ADMIN_BOOTSTRAP_PASSWORD")
        == secrets_bundle.admin_password
        and accounts_environment.get("DATABASE_URL")
        == f"postgres://app:{secrets_bundle.postgres_password}@postgres:5432/app"
        and accounts_environment.get("REDIS_URL")
        == f"redis://:{secrets_bundle.redis_password}@redis:6379/0"
        and not any(
            key.startswith("NACOS_")
            for service_environment in (
                postgres_environment,
                redis_environment,
                accounts_environment,
            )
            for key in service_environment
        )
    )
    if not matches:
        raise RuntimeError("容器环境与 Nacos 值不一致或 Nacos 引导参数泄漏")
    print("PASS  Nacos 值原样到达 PostgreSQL、Redis 与 accounts 容器；NACOS_* 未传入容器")


def http_status(url: str) -> int:
    try:
        with urllib.request.urlopen(url, timeout=2) as response:
            return response.status
    except urllib.error.HTTPError as response:
        return response.code
    except (OSError, urllib.error.URLError):
        return 0


def verify_native_application_config(
    environment: dict[str, str],
    working_directory: Path,
    fixture: CiFixture,
) -> None:
    app_environment = environment.copy()
    for variable_name in list(app_environment):
        if variable_name.startswith("APP_") or variable_name in {
            "DATABASE_URL",
            "REDIS_URL",
            "ADMIN_BOOTSTRAP_USERNAME",
            "ADMIN_BOOTSTRAP_PASSWORD",
        }:
            app_environment.pop(variable_name, None)
    app_environment.update(
        {
            "APP_CONFIG_FILE": str(working_directory / "locator.toml"),
            "NACOS_USERNAME": "",
            "NACOS_PASSWORD": "",
            "NACOS_ACCESS_TOKEN": "",
            "SQLX_OFFLINE": "true",
            "RUST_LOG": "error",
        }
    )
    build_result = subprocess.run(
        [
            "cargo",
            "build",
            "--locked",
            "--quiet",
            "-p",
            "tpl-accounts",
            "--bin",
            "accounts-service",
        ],
        cwd=ROOT,
        env=app_environment,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if build_result.returncode != 0:
        raise RuntimeError("native accounts-service 构建失败")

    service_process = subprocess.Popen(
        [str(ROOT / "target/debug/accounts-service")],
        cwd=working_directory,
        env=app_environment,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    service_url = f"http://127.0.0.1:{fixture.application_port}"
    try:
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if service_process.poll() is not None:
                raise RuntimeError("native accounts-service 在 /live 就绪前退出")
            if http_status(f"{service_url}/live") == 200:
                break
            time.sleep(0.5)
        else:
            raise RuntimeError("native accounts-service 未在远端配置端口就绪")

        if http_status(f"{service_url}/auth/me") != 503:
            raise RuntimeError("未配置数据库/Redis 时 /auth/me 应返回 503")
        print("PASS  native accounts-service 从应用 TOML Data ID 读取端口；无 DB 时 /auth/me 返回 503")
    finally:
        service_process.terminate()
        try:
            service_process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            service_process.kill()
            service_process.wait(timeout=5)


def cleanup(
    fixture: CiFixture,
    environment: dict[str, str],
    secrets_bundle: TestSecrets,
) -> None:
    cleanup_environment = environment.copy()
    cleanup_environment["POSTGRES_PASSWORD"] = secrets_bundle.postgres_password
    cleanup_environment["REDIS_PASSWORD"] = secrets_bundle.redis_password
    subprocess.run(
        [
            "docker",
            "compose",
            "--env-file",
            "/dev/null",
            "--project-name",
            fixture.project_name,
            "--file",
            "docker-compose.yml",
            "down",
            "--volumes",
            "--remove-orphans",
        ],
        cwd=ROOT,
        env=cleanup_environment,
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    subprocess.run(
        ["docker", "rm", "--force", fixture.nacos_container],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def main() -> int:
    if (ROOT / ".env.compose").exists():
        raise RuntimeError("CI 验证要求干净 checkout 中不存在旧 .env.compose 文件")

    suffix = secrets.token_hex(4)
    project_name = f"tpl-ci-{suffix}"
    nacos_port, web_port, admin_port, application_port = free_distinct_ports(4)
    namespace = "public"
    group = f"CI-{suffix.upper()}"
    location = NacosLocation(
        server_url=f"http://127.0.0.1:{nacos_port}",
        namespace=namespace,
        group=group,
        app_data_id=f"rust-study-ci-{suffix}.toml",
        compose_data_id=f"rust-study-ci-{suffix}.properties",
    )
    fixture = CiFixture(
        nacos=location,
        project_name=project_name,
        nacos_container=f"{project_name}-nacos",
        nacos_port=nacos_port,
        web_port=web_port,
        admin_port=admin_port,
        application_port=application_port,
    )
    secrets_bundle = TestSecrets(
        postgres_password=f"pg-{secrets.token_hex(16)}",
        redis_password=f"redis-{secrets.token_hex(16)}",
        admin_password=SPECIAL_ADMIN_PASSWORD,
        nacos_token=base64.b64encode(secrets.token_bytes(32)).decode("ascii"),
        nacos_identity=secrets.token_hex(16),
    )

    print("== 启动一次性 Nacos 配置源")
    with tempfile.TemporaryDirectory(prefix="nacos-compose-ci-") as temporary:
        configuration_directory = Path(temporary)
        locator_file = configuration_directory / "locator.toml"
        write_locator(locator_file, location)
        environment = compose_environment(fixture, locator_file)
        environment.pop("ENV_FILE", None)
        nacos_environment = environment.copy()
        nacos_environment["NACOS_AUTH_TOKEN"] = secrets_bundle.nacos_token
        nacos_environment["NACOS_AUTH_IDENTITY_VALUE"] = secrets_bundle.nacos_identity

        container_result = subprocess.run(
            [
                "docker",
                "run",
                "--detach",
                "--rm",
                "--name",
                fixture.nacos_container,
                "--publish",
                f"127.0.0.1:{fixture.nacos_port}:8848",
                "--memory",
                "1g",
                "--env",
                "MODE=standalone",
                "--env",
                "JVM_XMS=256m",
                "--env",
                "JVM_XMX=512m",
                "--env",
                "JVM_XMN=128m",
                "--env",
                "JVM_MS=96m",
                "--env",
                "JVM_MMS=256m",
                "--env",
                "NACOS_AUTH_ENABLE=false",
                "--env",
                "NACOS_AUTH_ADMIN_ENABLE=false",
                "--env",
                "NACOS_AUTH_CONSOLE_ENABLE=false",
                "--env",
                "NACOS_AUTH_TOKEN",
                "--env",
                "NACOS_AUTH_IDENTITY_KEY=ciIdentity",
                "--env",
                "NACOS_AUTH_IDENTITY_VALUE",
                NACOS_IMAGE,
            ],
            cwd=ROOT,
            env=nacos_environment,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        if container_result.returncode != 0:
            raise RuntimeError("无法启动一次性 Nacos 容器")

        try:
            wait_for_nacos(location)
            valid_content = properties_content(secrets_bundle)
            publish_config(location, location.compose_data_id, valid_content)
            wait_for_client_config(location, location.compose_data_id, valid_content)
            print("PASS  独立 Nacos Client API 返回远端 Compose Properties")

            app_toml_content = (
                "[server]\n"
                'host = "127.0.0.1"\n'
                f"port = {application_port}\n"
            )
            publish_config(location, location.app_data_id, app_toml_content)
            wait_for_client_config(location, location.app_data_id, app_toml_content)
            verify_native_application_config(
                environment, configuration_directory, fixture
            )

            missing_data_id = f"missing-{suffix}.properties"
            missing_locator = configuration_directory / "missing-locator.toml"
            missing_location = replace(location, compose_data_id=missing_data_id)
            write_locator(missing_locator, missing_location)
            missing_environment = environment.copy()
            missing_environment["APP_CONFIG_FILE"] = str(missing_locator)
            run_fail_closed_case("配置不存在", missing_environment)

            malformed_data_id = f"malformed-{suffix}.properties"
            malformed_locator = configuration_directory / "malformed-locator.toml"
            malformed_location = replace(location, compose_data_id=malformed_data_id)
            write_locator(malformed_locator, malformed_location)
            publish_config(
                location,
                malformed_data_id,
                "POSTGRES_PASSWORD=unused\nREDIS_PASSWORD=unused\nUNKNOWN_KEY=must-fail\n",
            )
            wait_for_client_config(
                location,
                malformed_data_id,
                "POSTGRES_PASSWORD=unused\nREDIS_PASSWORD=unused\nUNKNOWN_KEY=must-fail\n",
            )
            malformed_environment = environment.copy()
            malformed_environment["APP_CONFIG_FILE"] = str(malformed_locator)
            run_fail_closed_case("属性格式不受支持", malformed_environment)

            duplicate_data_id = f"duplicate-{suffix}.properties"
            duplicate_locator = configuration_directory / "duplicate-locator.toml"
            duplicate_location = replace(location, compose_data_id=duplicate_data_id)
            write_locator(duplicate_locator, duplicate_location)
            duplicate_content = (
                "POSTGRES_PASSWORD=first\n"
                "POSTGRES_PASSWORD=second\n"
                "REDIS_PASSWORD=unused\n"
            )
            publish_config(location, duplicate_data_id, duplicate_content)
            wait_for_client_config(location, duplicate_data_id, duplicate_content)
            duplicate_environment = environment.copy()
            duplicate_environment["APP_CONFIG_FILE"] = str(duplicate_locator)
            run_fail_closed_case("重复属性键", duplicate_environment)

            invalid_type_data_id = f"invalid-type-{suffix}.properties"
            invalid_type_locator = configuration_directory / "invalid-type-locator.toml"
            invalid_type_location = replace(
                location, compose_data_id=invalid_type_data_id
            )
            write_locator(invalid_type_locator, invalid_type_location)
            invalid_type_content = properties_content(secrets_bundle).replace(
                "WEB_PORT=3000\n", "WEB_PORT=65536\n"
            )
            publish_config(location, invalid_type_data_id, invalid_type_content)
            wait_for_client_config(
                location, invalid_type_data_id, invalid_type_content
            )
            invalid_type_environment = environment.copy()
            invalid_type_environment["APP_CONFIG_FILE"] = str(invalid_type_locator)
            run_fail_closed_case("非有效端口类型", invalid_type_environment)

            unreachable_environment = environment.copy()
            unreachable_environment["NACOS_SERVER_ADDR"] = f"http://127.0.0.1:{free_port()}"
            run_fail_closed_case("Nacos 不可达", unreachable_environment)
            test_auth_rejection(environment)

            legacy_environment = environment.copy()
            legacy_environment["ENV_FILE"] = "/tmp/legacy-compose.env"
            run_fail_closed_case("ENV_FILE 已停用", legacy_environment)

            run_checked(
                ["./scripts/compose-config.sh", "compose", "full", "config", "--quiet"],
                environment=environment,
            )
            run_checked(
                ["./scripts/compose-config.sh", "compose", "full", "config", "--services"],
                environment=environment,
            )
            print("PASS  Compose 仅执行静默或服务名配置验证")

            print("== 启动隔离 Compose stack")
            redacted_values = (
                secrets_bundle.postgres_password,
                secrets_bundle.redis_password,
                secrets_bundle.admin_password,
                secrets_bundle.nacos_token,
                secrets_bundle.nacos_identity,
            )
            run_checked(
                ["just", "stack-up"],
                environment=environment,
                options=RunOptions(redacted_values=redacted_values),
            )
            verify_container_configuration(fixture, secrets_bundle)
            run_checked(
                ["just", "stack-smoke"],
                environment=environment,
                options=RunOptions(
                    redacted_values=redacted_values,
                    include_failure_output=True,
                ),
            )
            print("PASS  完整 stack smoke（入口、认证隔离、限流、备份与恢复）")

            if (ROOT / ".env.compose").exists():
                raise RuntimeError("Compose 运维入口生成了旧式本地部署文件")
            print("PASS  运维入口未生成 .env.compose")
        finally:
            cleanup(fixture, environment, secrets_bundle)

    print("PASS  仅清理本次创建的 Compose 项目、数据卷与 Nacos 容器")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:  # 只输出自定义脱敏状态，不回显子进程原始 stderr。
        print(f"FAIL  {error}", file=sys.stderr)
        raise SystemExit(1)
