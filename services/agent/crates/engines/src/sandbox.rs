//! 内置 Sandbox 执行器：受限平台工具、步骤预算、协作取消与受治理委派。
//!
//! 本适配器不取得容器、宿主进程或网络执行权限。`text.summarize`、
//! `artifact.write` 与 `delegate_task` 都是本进程内的受限平台工具：
//!
//! - 普通文本由 `text.summarize` 生成确定性摘要；
//! - ` ```artifact filename="..." media_type="..." ` 代码块触发 `artifact.write`；
//! - `@delegate <child-agent>: <task>` 行触发受治理的 `delegate_task`。
//!
//! 工具白名单与步骤上限来自 Run 冻结的 Runtime 快照；任一工具不在白名单、
//! 指令格式非法或超出步骤预算时 fail closed，不执行部分副作用。取消在工具
//! 调用之间检查，已发出的委派副作用不保证回滚。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tpl_agent_core::delegation::{DelegationChildRef, DelegationSnapshot};
use tpl_agent_core::error::AgentError;
use tpl_agent_core::ports::{
    DelegationRequest, ExecutionObserver, ExecutionOutcome, ExecutionRequest, RunExecutor,
};
use tpl_agent_core::resources::{NewArtifact, ResourceContent, sanitize_filename};
use tpl_agent_core::run::RuntimeSnapshot;
use tpl_agent_core::run_events::{
    message_delta, tool_call_completed, tool_call_failed, tool_call_started,
};
use tpl_agent_core::time_utils::now;

use crate::skills::{self, SkillContext};

/// 文本摘要工具名。
pub const TOOL_SUMMARIZE: &str = "text.summarize";
/// 产物写出工具名。
pub const TOOL_ARTIFACT: &str = "artifact.write";
/// 受治理委派工具名。
pub const TOOL_DELEGATE: &str = "delegate_task";

const MAX_SUMMARY_CHARS: usize = 2000;
const MAX_RESOURCE_EXCERPT_CHARS: usize = 600;
const MAX_DELEGATION_NOTE_CHARS: usize = 1000;
const MAX_ARTIFACT_HEADER_BYTES: usize = 512;

/// 内置 Sandbox 执行器。
#[derive(Clone, Copy, Debug, Default)]
pub struct SandboxExecutor;

impl SandboxExecutor {
    /// 创建执行器；工具集合完全来自每次 Run 的 Runtime 快照。
    pub fn new() -> Self {
        Self
    }
}

/// 输入中的一条显式平台工具指令。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Directive {
    Artifact {
        filename: String,
        media_type: String,
        content: String,
    },
    Delegate {
        child_name: String,
        task: String,
    },
}

/// 解析后的输入：工具指令与剩余文本。
#[derive(Clone, Debug, PartialEq, Eq)]
struct ParsedInput {
    directives: Vec<Directive>,
    context: String,
}

#[async_trait]
impl RunExecutor for SandboxExecutor {
    async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionOutcome, AgentError> {
        let snapshot = &request.run.snapshot;
        let runtime = snapshot.runtime();
        if runtime.max_steps == 0 {
            return Err(AgentError::Execution("sandbox_step_limit_exceeded"));
        }
        let parsed = parse_input(&snapshot.input.content)?;
        validate_directives(runtime, &parsed.directives, snapshot.delegation.as_ref())?;
        let summarize_enabled = runtime.allows_tool(TOOL_SUMMARIZE);
        let planned_steps = usize::from(summarize_enabled) + parsed.directives.len();
        if planned_steps > usize::try_from(runtime.max_steps).unwrap_or(usize::MAX) {
            return Err(AgentError::Execution("sandbox_step_limit_exceeded"));
        }
        let skill_context = skills::assemble_skill_context(
            &snapshot.agent.prompt,
            &snapshot.skills,
            &request.skill_assets,
        )?;

        let mut step = 0usize;
        let mut artifacts = Vec::new();
        let mut artifact_names = Vec::new();
        let mut delegation_notes = Vec::new();
        let mut tool_failures = Vec::new();
        let mut summary = None;

        if summarize_enabled {
            step += 1;
            let call_id = format!("sandbox-{step}-summarize");
            observe(
                &request.observer,
                tool_call_started(now(), &call_id, TOOL_SUMMARIZE),
            )
            .await?;
            if request.cancellation.is_cancel_requested().await? {
                return Ok(cancelled_outcome());
            }
            let text = summarize_input(&parsed.context, &request.resources);
            let result = json!({"characters": text.chars().count()});
            observe(
                &request.observer,
                tool_call_completed(now(), &call_id, &result)?,
            )
            .await?;
            summary = Some(text);
        }

        for directive in parsed.directives {
            if request.cancellation.is_cancel_requested().await? {
                return Ok(cancelled_outcome());
            }
            step += 1;
            match directive {
                Directive::Artifact {
                    filename,
                    media_type,
                    content,
                } => {
                    let call_id = format!("sandbox-{step}-artifact");
                    observe(
                        &request.observer,
                        tool_call_started(now(), &call_id, TOOL_ARTIFACT),
                    )
                    .await?;
                    let bytes = content.into_bytes();
                    if bytes.is_empty() {
                        observe(
                            &request.observer,
                            tool_call_failed(now(), &call_id, "sandbox_artifact_invalid"),
                        )
                        .await?;
                        return Err(AgentError::Execution("sandbox_artifact_invalid"));
                    }
                    let result = json!({"bytes": bytes.len(), "media_type": media_type});
                    observe(
                        &request.observer,
                        tool_call_completed(now(), &call_id, &result)?,
                    )
                    .await?;
                    artifact_names.push(filename.clone());
                    artifacts.push(NewArtifact {
                        filename,
                        media_type,
                        bytes,
                    });
                }
                Directive::Delegate { child_name, task } => {
                    let call_id = format!("sandbox-{step}-delegate");
                    observe(
                        &request.observer,
                        tool_call_started(now(), &call_id, TOOL_DELEGATE),
                    )
                    .await?;
                    let child = child_by_name(snapshot.delegation.as_ref(), &child_name)
                        .ok_or(AgentError::Execution("sandbox_invalid_directive"))?;
                    let outcome = request
                        .delegation
                        .delegate(DelegationRequest {
                            parent_run_id: request.run.run_id,
                            tool_call_id: call_id.clone(),
                            child_agent_id: child.agent_id,
                            input: task,
                        })
                        .await;
                    match outcome {
                        Ok(result) => {
                            let result_value = json!({
                                "child_run_id": result.child_run_id.to_string(),
                                "status": result.status.as_str(),
                            });
                            observe(
                                &request.observer,
                                tool_call_completed(now(), &call_id, &result_value)?,
                            )
                            .await?;
                            if result.status == tpl_agent_core::run::RunStatus::Cancelled {
                                if request.cancellation.is_cancel_requested().await? {
                                    return Ok(cancelled_outcome());
                                }
                                tool_failures.push(format!("委派 {} 已取消", child.display_name));
                            } else if let Some(error_code) = result.error_code {
                                tool_failures.push(format!(
                                    "委派 {} 失败：{error_code}",
                                    child.display_name
                                ));
                            } else {
                                delegation_notes.push((
                                    child.display_name.clone(),
                                    result.content.unwrap_or_default(),
                                ));
                            }
                        }
                        Err(AgentError::RunCancelled) => {
                            observe(
                                &request.observer,
                                tool_call_failed(now(), &call_id, "run_cancelled"),
                            )
                            .await?;
                            return Ok(cancelled_outcome());
                        }
                        Err(error) => {
                            let code = error.code();
                            observe(&request.observer, tool_call_failed(now(), &call_id, code))
                                .await?;
                            tool_failures.push(format!("委派 {} 失败：{code}", child.display_name));
                        }
                    }
                }
            }
        }

        let assistant_content = compose_answer(
            summarize_enabled,
            summary.as_deref(),
            &skill_context,
            &delegation_notes,
            &tool_failures,
            &artifact_names,
        );
        observe(
            &request.observer,
            message_delta(now(), request.assistant_message_id, 0),
        )
        .await?;
        Ok(ExecutionOutcome {
            assistant_content,
            artifacts,
            external_stop_confirmed: false,
        })
    }
}

async fn observe(
    observer: &Arc<dyn ExecutionObserver>,
    draft: tpl_agent_core::run::RunEventDraft,
) -> Result<(), AgentError> {
    observer.observe(draft).await
}

fn cancelled_outcome() -> ExecutionOutcome {
    ExecutionOutcome {
        assistant_content: String::new(),
        artifacts: Vec::new(),
        external_stop_confirmed: false,
    }
}

fn validate_directives(
    runtime: &RuntimeSnapshot,
    directives: &[Directive],
    delegation: Option<&DelegationSnapshot>,
) -> Result<(), AgentError> {
    for directive in directives {
        match directive {
            Directive::Artifact {
                filename,
                media_type,
                content,
            } => {
                if !runtime.allows_tool(TOOL_ARTIFACT) {
                    return Err(AgentError::Execution("sandbox_tool_not_allowed"));
                }
                if content.is_empty()
                    || media_type.len() > 128
                    || media_type.chars().any(char::is_control)
                    || sanitize_filename(filename).is_err()
                {
                    return Err(AgentError::Execution("sandbox_invalid_directive"));
                }
            }
            Directive::Delegate { child_name, .. } => {
                if !runtime.allows_tool(TOOL_DELEGATE) {
                    return Err(AgentError::Execution("sandbox_tool_not_allowed"));
                }
                if child_by_name(delegation, child_name).is_none() {
                    return Err(AgentError::Execution("sandbox_invalid_directive"));
                }
            }
        }
    }
    Ok(())
}

fn child_by_name<'a>(
    delegation: Option<&'a DelegationSnapshot>,
    name: &str,
) -> Option<&'a DelegationChildRef> {
    delegation?.child_agents.iter().find(|child| {
        child.name.eq_ignore_ascii_case(name) || child.display_name.eq_ignore_ascii_case(name)
    })
}

fn parse_input(input: &str) -> Result<ParsedInput, AgentError> {
    let mut directives = Vec::new();
    let mut context_lines = Vec::new();
    let lines = input.lines().collect::<Vec<_>>();
    let mut index = 0usize;
    while index < lines.len() {
        let trimmed = lines[index].trim_start();
        if let Some(header) = trimmed.strip_prefix("```artifact") {
            if header.len() > MAX_ARTIFACT_HEADER_BYTES {
                return Err(AgentError::Execution("sandbox_invalid_directive"));
            }
            let (filename, media_type) = parse_artifact_header(header)?;
            index += 1;
            let mut content_lines = Vec::new();
            let mut closed = false;
            while index < lines.len() {
                if lines[index].trim_end() == "```" {
                    closed = true;
                    index += 1;
                    break;
                }
                content_lines.push(lines[index]);
                index += 1;
            }
            if !closed {
                return Err(AgentError::Execution("sandbox_invalid_directive"));
            }
            directives.push(Directive::Artifact {
                filename,
                media_type,
                content: content_lines.join("\n"),
            });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("@delegate ") {
            let Some((child_name, task)) = rest.split_once(':') else {
                return Err(AgentError::Execution("sandbox_invalid_directive"));
            };
            let child_name = child_name.trim();
            let task = task.trim();
            if child_name.is_empty() || task.is_empty() {
                return Err(AgentError::Execution("sandbox_invalid_directive"));
            }
            directives.push(Directive::Delegate {
                child_name: child_name.to_owned(),
                task: task.to_owned(),
            });
            index += 1;
            continue;
        }
        context_lines.push(lines[index]);
        index += 1;
    }
    Ok(ParsedInput {
        directives,
        context: context_lines.join("\n").trim().to_owned(),
    })
}

fn parse_artifact_header(header: &str) -> Result<(String, String), AgentError> {
    let mut filename = None;
    let mut media_type = None;
    for token in header.split_whitespace() {
        let Some((key, raw_value)) = token.split_once('=') else {
            return Err(AgentError::Execution("sandbox_invalid_directive"));
        };
        let value = raw_value.trim_matches('"');
        if value.is_empty() {
            return Err(AgentError::Execution("sandbox_invalid_directive"));
        }
        match key {
            "filename" if filename.is_none() => filename = Some(value.to_owned()),
            "media_type" if media_type.is_none() => media_type = Some(value.to_owned()),
            _ => return Err(AgentError::Execution("sandbox_invalid_directive")),
        }
    }
    let filename =
        sanitize_filename(&filename.ok_or(AgentError::Execution("sandbox_invalid_directive"))?)
            .map_err(|_| AgentError::Execution("sandbox_invalid_directive"))?;
    let media_type = media_type.unwrap_or_else(|| infer_media_type(&filename));
    Ok((filename, media_type))
}

fn infer_media_type(filename: &str) -> String {
    let extension = filename
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("md") | Some("markdown") => "text/markdown".to_owned(),
        Some("json") => "application/json".to_owned(),
        _ => "text/plain".to_owned(),
    }
}

fn summarize_input(context: &str, resources: &[ResourceContent]) -> String {
    let mut sections = Vec::new();
    let normalized = collapse_whitespace(context);
    if !normalized.is_empty() {
        sections.push(truncate_chars(&normalized, MAX_SUMMARY_CHARS));
    }
    for resource in resources {
        let snapshot = &resource.snapshot;
        if skills::is_text_media_type(&snapshot.media_type) {
            match std::str::from_utf8(&resource.bytes) {
                Ok(text) => {
                    let text = collapse_whitespace(text);
                    if !text.is_empty() {
                        sections.push(format!(
                            "[资源 {}]\n{}",
                            snapshot.filename,
                            truncate_chars(&text, MAX_RESOURCE_EXCERPT_CHARS)
                        ));
                    }
                }
                Err(_) => sections.push(format!(
                    "[资源 {}]（{}，非 UTF-8 文本，未内联）",
                    snapshot.filename, snapshot.media_type
                )),
            }
        } else {
            sections.push(format!(
                "[附件 {}]（{}，{} 字节，未内联）",
                snapshot.filename, snapshot.media_type, snapshot.size_bytes
            ));
        }
    }
    if sections.is_empty() {
        "已接收输入，但没有可摘要的文本内容。".to_owned()
    } else {
        sections.join("\n\n")
    }
}

fn compose_answer(
    summarize_enabled: bool,
    summary: Option<&str>,
    skill_context: &SkillContext,
    delegation_notes: &[(String, String)],
    tool_failures: &[String],
    artifact_names: &[String],
) -> String {
    let mut sections = Vec::new();
    match summary {
        Some(summary) if !summary.is_empty() => sections.push(summary.to_owned()),
        _ if !summarize_enabled => {
            sections.push("平台 Runtime 未启用 text.summarize；输入已记录。".to_owned());
        }
        _ => {}
    }
    if !skill_context.skill_names.is_empty() {
        sections.push(format!(
            "使用的 Skill：{}",
            skill_context.skill_names.join("、")
        ));
    }
    if !delegation_notes.is_empty() {
        let mut lines = vec!["委派结果：".to_owned()];
        for (name, content) in delegation_notes {
            lines.push(format!(
                "- {name}：{}",
                truncate_chars(content, MAX_DELEGATION_NOTE_CHARS)
            ));
        }
        sections.push(lines.join("\n"));
    }
    if !tool_failures.is_empty() {
        let mut lines = vec!["工具失败：".to_owned()];
        for failure in tool_failures {
            lines.push(format!("- {failure}"));
        }
        sections.push(lines.join("\n"));
    }
    if !artifact_names.is_empty() {
        let mut lines = vec!["产物：".to_owned()];
        for name in artifact_names {
            lines.push(format!("- {name}"));
        }
        sections.push(lines.join("\n"));
    }
    sections.join("\n\n")
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(text: &str, maximum: usize) -> String {
    if text.chars().count() <= maximum {
        return text.to_owned();
    }
    let mut truncated = text.chars().take(maximum).collect::<String>();
    truncated.push('…');
    truncated
}

/// 检查取消后是否仍有待执行的工具指令；仅在测试中用于确认执行顺序。
#[cfg(test)]
fn remaining_steps(planned: usize, executed: usize) -> usize {
    planned.saturating_sub(executed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpl_agent_core::resources::ResourceSnapshot;
    use uuid::Uuid;

    #[test]
    fn artifact_blocks_are_parsed_out_of_context() {
        let parsed = parse_input(
            "Please produce a report.\n\
             ```artifact filename=\"report.md\" media_type=\"text/markdown\"\n\
             # Report\n\
             body\n\
             ```\n\
             after",
        )
        .unwrap();
        assert_eq!(parsed.context, "Please produce a report.\nafter");
        assert_eq!(parsed.directives.len(), 1);
        match &parsed.directives[0] {
            Directive::Artifact {
                filename,
                media_type,
                content,
            } => {
                assert_eq!(filename, "report.md");
                assert_eq!(media_type, "text/markdown");
                assert_eq!(content, "# Report\nbody");
            }
            other => panic!("unexpected directive: {other:?}"),
        }
    }

    #[test]
    fn delegation_lines_are_parsed() {
        let parsed = parse_input("intro\n@delegate Researcher: find the numbers\noutro").unwrap();
        assert_eq!(parsed.context, "intro\noutro");
        assert_eq!(
            parsed.directives,
            vec![Directive::Delegate {
                child_name: "Researcher".to_owned(),
                task: "find the numbers".to_owned(),
            }]
        );
    }

    #[test]
    fn malformed_directives_fail_closed() {
        assert!(parse_input("```artifact filename=\"x.md\"\nunterminated").is_err());
        assert!(parse_input("```artifact filename=\"\"\n```").is_err());
        assert!(parse_input("@delegate no-colon").is_err());
        assert!(parse_input("@delegate : task").is_err());
    }

    #[test]
    fn media_type_defaults_by_extension() {
        assert_eq!(infer_media_type("a.md"), "text/markdown");
        assert_eq!(infer_media_type("a.json"), "application/json");
        assert_eq!(infer_media_type("a.bin"), "text/plain");
    }

    #[test]
    fn summary_is_bounded_and_reports_binary_attachments() {
        let resources = vec![
            ResourceContent {
                snapshot: ResourceSnapshot {
                    resource_id: Uuid::nil(),
                    filename: "notes.txt".to_owned(),
                    media_type: "text/plain".to_owned(),
                    size_bytes: 5,
                    sha256: "a".repeat(64),
                },
                bytes: b"hello".to_vec(),
            },
            ResourceContent {
                snapshot: ResourceSnapshot {
                    resource_id: Uuid::nil(),
                    filename: "chart.png".to_owned(),
                    media_type: "image/png".to_owned(),
                    size_bytes: 12,
                    sha256: "b".repeat(64),
                },
                bytes: vec![0x89, 0x50, 0x4e, 0x47],
            },
        ];
        let summary = summarize_input("  one   two ", &resources);
        assert!(summary.contains("one two"));
        assert!(summary.contains("[资源 notes.txt]"));
        assert!(summary.contains("[附件 chart.png]（image/png，12 字节，未内联）"));
    }

    #[test]
    fn step_budget_is_not_exceeded_by_planned_work() {
        assert_eq!(remaining_steps(4, 1), 3);
        assert_eq!(remaining_steps(1, 1), 0);
    }
}
