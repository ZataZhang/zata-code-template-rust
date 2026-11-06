//! 守卫测试（guard test）：守护 Agent 服务层间依赖方向，并阻止跨服务私有 crate 引用。
//!
//! 编译器只能拦住「没声明的依赖」；有人把违规依赖写进 `Cargo.toml` 后照样能编译通过。
//! 所以这里读取 `cargo metadata`，断言 Agent 各层只依赖本服务允许的内部层。账号与
//! agent-tracing 的私有层都属于其他服务，同样不允许出现。
//!
//! 失败时：修复触发它的 `Cargo.toml`，不要改本测试让它通过。只有当架构约定本身变更时才改
//! 下面的名单，并同步更新 `docs/ai-standards/architecture.md`。
//!
//! 内部层按服务名和目录名识别（`services/agent/crates/api` → `agent-api`），
//! 服务根按目录名识别（`services/agent` → `agent`），所以给 package 改名不影响本测试。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// Agent 每一层允许依赖的内部层。`core` 是最内层；`agent` 是 composition root。
const ALLOWED_LAYER_DEPENDENCIES: &[(&str, &[&str])] = &[
    ("agent-core", &[]),
    ("agent-api", &["agent-core"]),
    ("agent-engines", &["agent-core"]),
    ("agent-infrastructure", &["agent-core"]),
    (
        "agent",
        &[
            "agent-api",
            "agent-core",
            "agent-engines",
            "agent-infrastructure",
        ],
    ),
];

/// 工作区里的一个内部层及其所属服务。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Layer {
    service: String,
    layer: String,
}

/// 某一层实际声明的内部依赖（含 dev / build 依赖）。
struct LayerDependencies {
    layer: String,
    internal_layers: BTreeSet<String>,
}

/// 对照允许名单，返回所有违规描述；空表示全部合规。
fn find_violations(declared: &[LayerDependencies]) -> Vec<String> {
    let allowed: BTreeMap<&str, BTreeSet<&str>> = ALLOWED_LAYER_DEPENDENCIES
        .iter()
        .map(|(layer, deps)| (*layer, deps.iter().copied().collect()))
        .collect();

    let mut violations = Vec::new();

    for layer_dependencies in declared {
        let Some(allowed_layers) = allowed.get(layer_dependencies.layer.as_str()) else {
            violations.push(format!(
                "未登记的层 `{}`：新增 crate 时必须同时在本测试的名单里声明它能依赖谁",
                layer_dependencies.layer
            ));
            continue;
        };
        for depended_layer in &layer_dependencies.internal_layers {
            if !allowed_layers.contains(depended_layer.as_str()) {
                violations.push(format!(
                    "`{}` 不允许依赖 `{depended_layer}`（Agent 服务内部允许：{allowed_layers:?}）",
                    layer_dependencies.layer
                ));
            }
        }
    }

    for expected_layer in allowed.keys() {
        if !declared.iter().any(|d| d.layer == *expected_layer) {
            violations.push(format!(
                "名单里的层 `{expected_layer}` 在工作区里不存在：是被删除或改名了吗？"
            ));
        }
    }
    violations
}

/// 用 `cargo metadata` 读出工作区各层声明的内部依赖，只返回 Agent 服务的层。
fn read_declared_dependencies() -> Vec<LayerDependencies> {
    let cargo_binary = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo_binary)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("无法执行 cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata 输出不是 JSON");
    // 只看 `services/*/crates/` 下的内部层与 `services/` 下的业务服务。`tools/` 下的开发期工具
    // （xtask）不进生产服务，也不参与依赖方向检查。其他服务的层也要建映射，才能发现跨服务引用。
    let packages: Vec<&Value> = metadata["packages"]
        .as_array()
        .expect("缺少 packages")
        .iter()
        .filter(|package| {
            package["manifest_path"]
                .as_str()
                .is_some_and(|manifest_path| {
                    Path::new(manifest_path)
                        .parent()
                        .and_then(Path::parent)
                        .and_then(Path::file_name)
                        .is_some_and(|name| matches!(name.to_str(), Some("crates" | "services")))
                })
        })
        .collect();

    // 包名 → 层（含所属服务）。
    let layer_by_package_name: BTreeMap<&str, Layer> = packages
        .iter()
        .map(|package| {
            let manifest_path = package["manifest_path"]
                .as_str()
                .expect("缺少 manifest_path");
            let package_directory = Path::new(manifest_path)
                .parent()
                .expect("无法从 manifest_path 得到 package 目录");
            let parent_name = package_directory
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .expect("无法从 manifest_path 得到 package 父目录名");
            let layer = match parent_name {
                "crates" => {
                    let service_name = package_directory
                        .parent()
                        .and_then(Path::parent)
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                        .expect("无法从 manifest_path 得到服务目录名");
                    let crate_name = package_directory
                        .file_name()
                        .and_then(|name| name.to_str())
                        .expect("无法从 manifest_path 得到 crate 目录名");
                    Layer {
                        service: service_name.to_string(),
                        layer: format!("{service_name}-{crate_name}"),
                    }
                }
                "services" => Layer {
                    service: package_directory
                        .file_name()
                        .and_then(|name| name.to_str())
                        .expect("无法从 manifest_path 得到服务目录名")
                        .to_string(),
                    layer: package_directory
                        .file_name()
                        .and_then(|name| name.to_str())
                        .expect("无法从 manifest_path 得到服务目录名")
                        .to_string(),
                },
                unexpected_parent => panic!("未预期的服务 package 父目录 `{unexpected_parent}`"),
            };
            (package["name"].as_str().expect("缺少 name"), layer)
        })
        .collect();

    packages
        .iter()
        .filter(|package| {
            let package_name = package["name"].as_str().expect("缺少 name");
            layer_by_package_name[package_name].service == "agent"
        })
        .map(|package| {
            let package_name = package["name"].as_str().expect("缺少 name");
            let internal_layers = package["dependencies"]
                .as_array()
                .expect("缺少 dependencies")
                .iter()
                .filter_map(|dependency| dependency["name"].as_str())
                .filter_map(|dependency_name| layer_by_package_name.get(dependency_name))
                .map(|layer| layer.layer.clone())
                .collect();
            LayerDependencies {
                layer: layer_by_package_name[package_name].layer.clone(),
                internal_layers,
            }
        })
        .collect()
}

#[test]
fn agent_layer_dependencies_follow_the_allowed_direction() {
    let violations = find_violations(&read_declared_dependencies());
    assert!(
        violations.is_empty(),
        "Agent 层间依赖方向被破坏（见 docs/ai-standards/architecture.md）：\n- {}",
        violations.join("\n- ")
    );
}

/// 负控：确认检查器真的会报错，而不是永远通过。
#[test]
fn checker_rejects_forbidden_and_cross_service_dependencies() {
    let layer = |name: &str, deps: &[&str]| LayerDependencies {
        layer: name.to_string(),
        internal_layers: deps.iter().map(|dep| (*dep).to_string()).collect(),
    };
    let declared = vec![
        layer("agent-core", &["agent-engines"]), // 内层反向依赖外层
        layer(
            "agent-api",
            &["agent-core", "agent-infrastructure", "accounts-core"],
        ), // 接入层越过 core，且引用其他服务私有层
        layer("agent-engines", &["agent-core"]),
        layer("agent-infrastructure", &["agent-core"]),
        layer(
            "agent",
            &[
                "agent-api",
                "agent-core",
                "agent-engines",
                "agent-infrastructure",
            ],
        ),
        layer("agent-extra", &[]), // 未登记的新层
    ];

    let violations = find_violations(&declared);

    assert_eq!(violations.len(), 4, "实际：{violations:#?}");
    assert!(
        violations
            .iter()
            .any(|v| v.starts_with("`agent-core` 不允许依赖 `agent-engines`"))
    );
    assert!(
        violations
            .iter()
            .any(|v| v.starts_with("`agent-api` 不允许依赖 `agent-infrastructure`"))
    );
    assert!(
        violations
            .iter()
            .any(|v| v.starts_with("`agent-api` 不允许依赖 `accounts-core`"))
    );
    assert!(
        violations
            .iter()
            .any(|v| v.contains("未登记的层 `agent-extra`"))
    );
}
