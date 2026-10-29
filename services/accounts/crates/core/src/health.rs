//! 健康检查用例：汇总各依赖项的就绪探针，得出整体是否就绪。

use std::sync::Arc;

use async_trait::async_trait;

/// 就绪探针端口：检查某一个外部依赖（数据库、缓存……）是否可用。
///
/// 实现方负责给探测加超时，避免依赖卡死拖住整个 `/ready` 请求。
#[async_trait]
pub trait ReadinessProbe: Send + Sync {
    /// 依赖项名称，会出现在就绪报告里（如 `"postgres"`）。
    fn dependency_name(&self) -> &'static str;

    /// 执行一次探测。失败时返回原因，仅用于服务端日志，不应原样返回给外部调用方。
    async fn check(&self) -> Result<(), String>;
}

/// 单个依赖项的探测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyOutcome {
    /// 依赖项名称。
    pub dependency_name: &'static str,
    /// 探测失败的原因；`None` 表示探测通过。
    pub failure_reason: Option<String>,
}

/// 整体就绪报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessReport {
    /// 每个依赖项的结果，顺序与注册顺序一致。
    pub outcomes: Vec<DependencyOutcome>,
}

impl ReadinessReport {
    /// 所有依赖都通过探测才算就绪；没有任何依赖时视为就绪。
    pub fn is_ready(&self) -> bool {
        self.outcomes
            .iter()
            .all(|outcome| outcome.failure_reason.is_none())
    }
}

/// 健康检查用例。
pub struct HealthService {
    readiness_probes: Vec<Arc<dyn ReadinessProbe>>,
}

impl HealthService {
    /// 用一组就绪探针创建服务。
    pub fn new(readiness_probes: Vec<Arc<dyn ReadinessProbe>>) -> Self {
        Self { readiness_probes }
    }

    /// 依次执行所有探针并汇总结果。
    pub async fn check_readiness(&self) -> ReadinessReport {
        let mut outcomes = Vec::with_capacity(self.readiness_probes.len());
        for probe in &self.readiness_probes {
            let failure_reason = probe.check().await.err();
            outcomes.push(DependencyOutcome {
                dependency_name: probe.dependency_name(),
                failure_reason,
            });
        }
        ReadinessReport { outcomes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用探针：按构造时给定的结果返回。
    struct FixedProbe {
        dependency_name: &'static str,
        fixed_result: Result<(), String>,
    }

    #[async_trait]
    impl ReadinessProbe for FixedProbe {
        fn dependency_name(&self) -> &'static str {
            self.dependency_name
        }

        async fn check(&self) -> Result<(), String> {
            self.fixed_result.clone()
        }
    }

    fn probe(
        dependency_name: &'static str,
        fixed_result: Result<(), String>,
    ) -> Arc<dyn ReadinessProbe> {
        Arc::new(FixedProbe {
            dependency_name,
            fixed_result,
        })
    }

    #[tokio::test]
    async fn ready_when_there_are_no_dependencies() {
        let report = HealthService::new(vec![]).check_readiness().await;
        assert!(report.is_ready());
        assert!(report.outcomes.is_empty());
    }

    #[tokio::test]
    async fn ready_when_every_probe_passes() {
        let service = HealthService::new(vec![probe("postgres", Ok(())), probe("redis", Ok(()))]);
        assert!(service.check_readiness().await.is_ready());
    }

    #[tokio::test]
    async fn not_ready_when_any_probe_fails_and_order_is_kept() {
        let service = HealthService::new(vec![
            probe("postgres", Ok(())),
            probe("redis", Err("连接被拒绝".to_string())),
        ]);
        let report = service.check_readiness().await;

        assert!(!report.is_ready());
        assert_eq!(report.outcomes[0].dependency_name, "postgres");
        assert_eq!(report.outcomes[0].failure_reason, None);
        assert_eq!(report.outcomes[1].dependency_name, "redis");
        assert_eq!(
            report.outcomes[1].failure_reason.as_deref(),
            Some("连接被拒绝")
        );
    }
}
