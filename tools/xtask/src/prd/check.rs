//! PRD 门禁：交付物 PRD 的结构、验收清单与验证 oracle 是否合规。
//!
//! 三档严格度（[`Gate`]）：
//!
//! - [`Gate::Structure`]：只检查结构（必需章节、Part A/B 分层、解读回显、FR 编号、oracle 模式、交付依赖）。
//!   新写出来的 pending PRD 用它，因为 pending 的清单本来就可以是空框。
//! - [`Gate::Delivery`]：再加「执行侧欠的验收项必须都勾上」。
//! - [`Gate::ArchiveReady`]：再加最终对账（Final Reconciliation）与验收状态横幅和 §9 的一致性。
//!   归档只代表**执行侧**交付完成：`Human-Confirmed` 组内的空框是留给人的待答项，不拦归档，
//!   它们的去向由横幅承接——横幅必须与 §9 对得上，否则一条「已归档但仍欠人一个确认」的 PRD
//!   会从所有待办视图里消失（Machine Contract §8）。
//!
//! 解析复用 [`super::contract`]（PRD 格式的唯一实现）；本模块只负责「是否合规」的裁决。

use std::sync::LazyLock;

use regex::Regex;

use super::contract::{
    ACCEPTANCE_CHECKLIST_HEADING, AcceptanceStatus, TOP_LEVEL_HEADING, find_acceptance_banner,
    parse_checklist,
};

/// 一条问题：行号（1 起始；整体性问题没有行号）加说明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// 1 起始的行号；缺章节这类整体性问题为 `None`。
    pub line: Option<usize>,
    /// 问题说明。
    pub message: String,
}

impl Issue {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            message: message.into(),
        }
    }

    fn general(message: impl Into<String>) -> Self {
        Self {
            line: None,
            message: message.into(),
        }
    }
}

/// 校验的严格度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// 只查结构。
    Structure,
    /// 结构 + 执行侧欠的验收项。
    Delivery,
    /// 交付 + 最终对账 + 横幅一致性。
    ArchiveReady,
}

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|error| panic!("内置正则无效 {pattern:?}：{error}"))
}

static REALISTIC_VALIDATION_HEADING: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^###\s+(?:7\.6\s+)?Realistic Validation Plan\b"));
static THIRD_LEVEL_HEADING: LazyLock<Regex> = LazyLock::new(|| regex(r"^###\s+"));
static YAML_FENCE_START: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)^\s*```ya?ml\s*$"));
static YAML_FENCE_END: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s*```\s*$"));
static ORACLE_ENTRY: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s*-\s+id:\s*(?P<value>.*)$"));
static ORACLE_FIELD: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^\s+(?P<key>[a-z_]+):\s*(?P<value>.*)$"));
static PART_A_HEADING: LazyLock<Regex> = LazyLock::new(|| regex(r"^#\s+Part A\b"));
static PART_B_HEADING: LazyLock<Regex> = LazyLock::new(|| regex(r"^#\s+Part B\b"));
static PART_A_EXECUTOR_METADATA: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"\brv-\d+\b|critical_value_source:|must_cross:|forbidden_bypasses:|fresh_state_probe:|final_tree_evidence:",
    )
});
static DELIVERY_DEPENDENCIES_HEADING: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)^#{2,4}\s+(?:\d+\.\s+)?Delivery Dependencies\s*$"));
static DELIVERY_SCALAR_FIELD: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)^-\s*(?P<key>Gate type|Sequence)\s*:\s*(?P<value>.*)$"));
static DELIVERY_GATE_BANNER: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)^>\s*.*?(?:交付前置|Delivery Gate)"));
static FUNCTIONAL_REQUIREMENT_HEADING: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^##\s+(?:10\.\s+)?Functional Requirements\b"));
static FUNCTIONAL_REQUIREMENT: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^\s*[-*+]\s+(?:\*\*)?FR-(?P<number>\d+)(?:\*\*)?\s*[:：]"));
static FINAL_RECONCILIATION_HEADING: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^###\s+Final Reconciliation\b"));
static RECONCILIATION_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"^\s*[-*+]\s+(?P<key>Interpretation|Public behavior and contracts|Related PRD status|Requirements and risks|Validation blocker|Validation follow-up):\s*(?P<value>.*)$",
    )
});
static INTERPRETATION_HEADING: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^###\s+Interpretation\b"));
static TABLE_ROW: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s*\|.*\|\s*$"));
static TABLE_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s*\|[\s:|-]+\|\s*$"));

const NO_EXECUTABLE_BEHAVIOR_MARKER: &str = "No executable behavior changes; realistic validation is limited to documentation/build checks.";
const INTERPRETATION_REQUIRED_BLOCKS: [&str; 2] = ["我默默定了这些", "我理解为不做"];
const MINIMUM_BEHAVIOR_EXAMPLE_ROWS: usize = 3;

/// 每条 oracle 总是必填的字段。
const BASE_ORACLE_FIELDS: [&str; 8] = [
    "behavior",
    "reviewer",
    "real_entry",
    "expected",
    "mock_boundary",
    "tier",
    "test_layer",
    "required_for_acceptance",
];
const VALID_ORACLE_REVIEWERS: [&str; 2] = ["human", "verifier"];
/// `reviewer: human` 额外必填：要给人看的呈递物。
const HUMAN_REVIEWER_ORACLE_FIELDS: [&str; 1] = ["presentation"];
/// 证据链出处只在失败有真实爆炸半径的地方才值得付出撰写与收集成本：R0/R1 只需要一条能判别失败的断言。
const DEEP_CHAIN_ORACLE_FIELDS: [&str; 5] = [
    "critical_value_source",
    "must_cross",
    "forbidden_bypasses",
    "fresh_state_probe",
    "final_tree_evidence",
];
const NEGATIVE_CONTROL_ORACLE_FIELDS: [&str; 2] = ["negative_control", "expected_fail"];
const VALID_ORACLE_TIERS: [&str; 4] = ["R0", "R1", "R2", "R3"];
const DEEP_CHAIN_TIERS: [&str; 2] = ["R2", "R3"];
const NEGATIVE_CONTROL_TIERS: [&str; 1] = ["R3"];
/// 没写 tier 的 oracle 按 R3 处理：未分级的条目保留全套负担，降低要求得靠声明低风险来换。
const DEFAULT_ORACLE_TIER: &str = "R3";
const NEGATIVE_CONTROL_NOT_FEASIBLE_PREFIX: &str = "not feasible";
const REQUIRED_RECONCILIATION_FIELDS: [&str; 4] = [
    "Interpretation",
    "Public behavior and contracts",
    "Related PRD status",
    "Requirements and risks",
];
/// `⏸️ 验证阻塞` 归档时额外必需的字段：阻塞原因与被追踪的闭合方。
const BLOCKED_RECONCILIATION_FIELDS: [&str; 2] = ["Validation blocker", "Validation follow-up"];

/// 必需章节及其标题写法，按它们应有的先后顺序排列。
static REQUIRED_SECTIONS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    let numbered = |name: &'static str, number: &str, exact: bool| {
        let tail = if exact { r"\s*$" } else { r"\b" };
        (
            name,
            regex(&format!(
                r"^##\s+(?:{number}\.\s+)?{}{tail}",
                regex::escape(name)
            )),
        )
    };
    vec![
        numbered("Introduction & Goals", "1", true),
        numbered("Human Review Map", "2", false),
        numbered("Usage And Impact After Implementation", "3", true),
        numbered("Requirement Shape", "4", true),
        numbered("Repository Context And Architecture Fit", "5", true),
        numbered("Recommendation", "6", true),
        numbered("Implementation Guide", "7", true),
        numbered("Delivery Dependencies", "8", true),
        ("Acceptance Checklist", ACCEPTANCE_CHECKLIST_HEADING.clone()),
        numbered("Functional Requirements", "10", true),
        numbered("Non-Goals", "11", true),
        numbered("Risks And Follow-Ups", "12", true),
        numbered("Decision Log", "13", true),
    ]
});

/// 对一份 PRD 全文运行门禁，返回全部问题（空表示通过）。
pub fn validate(content: &str, gate: Gate) -> Vec<Issue> {
    let mut issues = required_section_issues(content);
    issues.extend(part_a_metadata_issues(content));
    issues.extend(interpretation_echo_issues(content));
    issues.extend(functional_requirement_issues(content));
    if gate != Gate::Structure {
        issues.extend(unchecked_item_issues(content));
    }
    issues.extend(oracle_schema_issues(content));
    issues.extend(delivery_field_value_issues(content));
    issues.extend(delivery_gate_banner_issues(content));
    if gate == Gate::ArchiveReady {
        issues.extend(archive_reconciliation_issues(content));
        issues.extend(acceptance_banner_issues(content));
    }
    issues
}

fn lines_of(content: &str) -> Vec<&str> {
    content.lines().collect()
}

fn heading_level(line: &str) -> usize {
    line.len() - line.trim_start_matches('#').len()
}

fn required_section_issues(content: &str) -> Vec<Issue> {
    let lines = lines_of(content);
    let mut positions: Vec<(&str, usize)> = Vec::new();
    let mut issues = Vec::new();

    for (name, pattern) in REQUIRED_SECTIONS.iter() {
        match lines.iter().position(|line| pattern.is_match(line)) {
            Some(index) => positions.push((name, index)),
            None => issues.push(Issue::general(format!(
                "Missing required PRD section: {name}"
            ))),
        }
    }
    for pair in positions.windows(2) {
        let ((previous_name, previous_index), (current_name, current_index)) = (pair[0], pair[1]);
        if current_index < previous_index {
            issues.push(Issue::at(
                current_index + 1,
                format!("Section {current_name:?} must appear after {previous_name:?}"),
            ));
        }
    }
    issues
}

fn part_a_metadata_issues(content: &str) -> Vec<Issue> {
    let lines = lines_of(content);
    let Some(part_a) = lines.iter().position(|line| PART_A_HEADING.is_match(line)) else {
        return vec![Issue::general("Missing Part A review-layer heading")];
    };
    let Some(part_b) = lines.iter().position(|line| PART_B_HEADING.is_match(line)) else {
        return vec![Issue::general("Missing Part B build-layer heading")];
    };
    if part_b <= part_a {
        return vec![Issue::at(part_b + 1, "Part B must appear after Part A")];
    }
    lines[part_a + 1..part_b]
        .iter()
        .enumerate()
        .filter_map(|(offset, line)| {
            PART_A_EXECUTOR_METADATA.find(line).map(|found| {
                Issue::at(
                    part_a + 2 + offset,
                    format!(
                        "Part A contains executor-only metadata: {:?}",
                        found.as_str()
                    ),
                )
            })
        })
        .collect()
}

fn functional_requirement_issues(content: &str) -> Vec<Issue> {
    let lines = lines_of(content);
    let Some(heading) = lines
        .iter()
        .position(|line| FUNCTIONAL_REQUIREMENT_HEADING.is_match(line))
    else {
        return vec![Issue::general("Missing Functional Requirements section")];
    };
    let end = (heading + 1..lines.len())
        .find(|&index| TOP_LEVEL_HEADING.is_match(lines[index]))
        .unwrap_or(lines.len());

    let entries: Vec<(usize, usize)> = (heading + 1..end)
        .filter_map(|index| {
            FUNCTIONAL_REQUIREMENT
                .captures(lines[index])
                .and_then(|captures| captures["number"].parse().ok())
                .map(|number| (index + 1, number))
        })
        .collect();
    if entries.is_empty() {
        return vec![Issue::at(
            heading + 1,
            "Functional Requirements must contain FR-1, FR-2, … items",
        )];
    }

    let actual: Vec<usize> = entries.iter().map(|(_, number)| *number).collect();
    let expected: Vec<usize> = (1..=actual.len()).collect();
    if actual == expected {
        return Vec::new();
    }
    vec![Issue::at(
        entries[0].0,
        format!(
            "Functional Requirement IDs must be unique, sequential, and ordered from FR-1; found {actual:?}"
        ),
    )]
}

fn archive_reconciliation_issues(content: &str) -> Vec<Issue> {
    let lines = lines_of(content);
    let Some(heading) = lines
        .iter()
        .position(|line| FINAL_RECONCILIATION_HEADING.is_match(line))
    else {
        return vec![Issue::general(
            "Missing Final Reconciliation section required for archive readiness",
        )];
    };
    let end = (heading + 1..lines.len())
        .find(|&index| THIRD_LEVEL_HEADING.is_match(lines[index]))
        .unwrap_or(lines.len());

    let mut fields = std::collections::HashMap::new();
    for (index, line) in lines.iter().enumerate().take(end).skip(heading + 1) {
        if let Some(captures) = RECONCILIATION_FIELD.captures(line) {
            fields.insert(
                captures["key"].to_string(),
                (index + 1, captures["value"].trim().to_string()),
            );
        }
    }

    // `⏸️ 验证阻塞` 额外要求交代阻塞原因与闭合方，否则空框就只是空框。
    let mut required: Vec<&str> = REQUIRED_RECONCILIATION_FIELDS.to_vec();
    if is_validation_blocked(content) {
        required.extend(BLOCKED_RECONCILIATION_FIELDS);
    }
    let mut issues = Vec::new();
    for field in required {
        let Some((line, value)) = fields.get(field) else {
            issues.push(Issue::at(
                heading + 1,
                format!("Final Reconciliation missing field: {field}"),
            ));
            continue;
        };
        let normalized = value.to_lowercase().replace(' ', "");
        let looks_like_placeholder = value.is_empty()
            || (value.contains('[') && value.contains(']'))
            || normalized == "confirmed/corrected"
            || normalized == "confirmed/corrected—summary";
        if looks_like_placeholder {
            issues.push(Issue::at(
                *line,
                format!("Final Reconciliation field {field:?} is incomplete"),
            ));
        }
    }
    issues
}

/// 执行侧还欠的未勾项。`Human-Confirmed` 组内的空框是人的待答项，执行工具不得代勾，也不拦归档——
/// 它们是否还欠着，交给 [`acceptance_banner_issues`] 核对横幅。
fn unchecked_item_issues(content: &str) -> Vec<Issue> {
    let checklist = parse_checklist(content);
    if !checklist.section_found {
        return vec![Issue::general("Missing Acceptance Checklist section")];
    }
    // `⏸️ 验证阻塞` 是唯一允许带着执行侧空框归档的状态；空框本身照旧保留，
    // 由 Final Reconciliation 的 Validation blocker / follow-up 交代清楚。
    if is_validation_blocked(content) {
        return Vec::new();
    }
    checklist
        .execution_unchecked_items()
        .into_iter()
        .map(|item| Issue::at(item.line, item.text.clone()))
        .collect()
}

/// 横幅是否声明了 `⏸️ 验证阻塞`。
fn is_validation_blocked(content: &str) -> bool {
    find_acceptance_banner(content).and_then(|banner| banner.status)
        == Some(AcceptanceStatus::ValidationBlocked)
}

/// 归档时验收状态横幅与 §9 不一致的问题。
///
/// 规则（契约 §8）：§9 里 `Human-Confirmed` 组还有空框 → 横幅必须是 `🧍 待人工验收`；一个都不剩
/// → 必须是 `✅ 已验收`；`⬜ 未开工` 永远不可归档。
fn acceptance_banner_issues(content: &str) -> Vec<Issue> {
    let checklist = parse_checklist(content);
    if !checklist.section_found {
        // 缺清单章节已由 unchecked_item_issues 报告，这里不重复。
        return Vec::new();
    }
    let Some(banner) = find_acceptance_banner(content) else {
        return vec![Issue::at(
            1,
            "Missing Acceptance Status Banner under the title: archive hands the human review off to this \
             banner, so it must state 🧍 待人工验收 (Human-Confirmed items still open) or ✅ 已验收 (none \
             open). See 'Acceptance Status Banner' in the PRD skill.",
        )];
    };
    let open_human_count = checklist.human_unchecked_items().len();
    let open_executor_count = checklist.execution_unchecked_items().len();
    match banner.status {
        None => vec![Issue::at(
            banner.line,
            "Acceptance Status Banner state is not recognised: use exactly one of ⬜ 未开工 / ⏸️ 验证阻塞 / \
             🧍 待人工验收 / ✅ 已验收.",
        )],
        Some(AcceptanceStatus::NotStarted) => vec![Issue::at(
            banner.line,
            "Acceptance Status Banner still says ⬜ 未开工 but the PRD is being archived: set ⏸️ 验证阻塞 while \
             executor items are blocked, 🧍 待人工验收 while Human-Confirmed items remain open, else ✅ 已验收.",
        )],
        Some(AcceptanceStatus::ValidationBlocked) if open_executor_count == 0 => vec![Issue::at(
            banner.line,
            "Acceptance Status Banner says ⏸️ 验证阻塞 but §9 has no open executor item: flip it to 🧍 待人工验收 \
             while Human-Confirmed items remain open, else ✅ 已验收.",
        )],
        Some(AcceptanceStatus::ValidationBlocked) => Vec::new(),
        Some(AcceptanceStatus::AwaitingHuman) if open_executor_count > 0 => vec![Issue::at(
            banner.line,
            format!(
                "Acceptance Status Banner says 🧍 待人工验收 but {open_executor_count} executor item(s) in §9 are \
                 still unchecked: executor validation cannot be handed to the human — set ⏸️ 验证阻塞 and record \
                 Validation blocker / Validation follow-up, or finish the validation first."
            ),
        )],
        Some(AcceptanceStatus::AwaitingHuman) if open_human_count == 0 => vec![Issue::at(
            banner.line,
            "Acceptance Status Banner says 🧍 待人工验收 but §9 has no open Human-Confirmed item: flip it to ✅ 已验收.",
        )],
        Some(AcceptanceStatus::Accepted) if open_executor_count > 0 => vec![Issue::at(
            banner.line,
            format!(
                "Acceptance Status Banner says ✅ 已验收 but {open_executor_count} executor item(s) in §9 are still \
                 unchecked: set ⏸️ 验证阻塞 (validation blocked) or finish the validation."
            ),
        )],
        Some(AcceptanceStatus::Accepted) if open_human_count > 0 => vec![Issue::at(
            banner.line,
            format!(
                "Acceptance Status Banner says ✅ 已验收 but {open_human_count} Human-Confirmed item(s) in §9 \
                 are still unchecked: set 🧍 待人工验收 (only the human may tick them)."
            ),
        )],
        Some(_) => Vec::new(),
    }
}

/// Section 1 的解读回显：只有这一道关卡能拦住「解读错了」——oracle 和实现都是同一份解读的产物，
/// 解读错了它们会互相印证，下游检查全绿在错的行为上。散文没法被浏览式的审阅逐条否证，所以回显
/// 必须带一张具体的行为样例表，外加「默默定的决定」与「读成不做的范围」。
fn interpretation_echo_issues(content: &str) -> Vec<Issue> {
    let lines = lines_of(content);
    let Some(heading) = lines
        .iter()
        .position(|line| INTERPRETATION_HEADING.is_match(line))
    else {
        return vec![Issue::general(
            "Missing Section 1 'Interpretation (解读回显)' subsection",
        )];
    };
    let end = (heading + 1..lines.len())
        .find(|&index| {
            THIRD_LEVEL_HEADING.is_match(lines[index]) || TOP_LEVEL_HEADING.is_match(lines[index])
        })
        .unwrap_or(lines.len());
    let section = &lines[heading + 1..end];

    let mut example_rows = 0;
    for (offset, line) in section.iter().enumerate() {
        if !TABLE_SEPARATOR.is_match(line) {
            continue;
        }
        let rows = section[offset + 1..]
            .iter()
            .take_while(|row| TABLE_ROW.is_match(row))
            .count();
        example_rows = example_rows.max(rows);
    }

    let mut issues = Vec::new();
    if example_rows < MINIMUM_BEHAVIOR_EXAMPLE_ROWS {
        issues.push(Issue::at(
            heading + 1,
            format!(
                "Interpretation echo needs a behavior-example table with at least {MINIMUM_BEHAVIOR_EXAMPLE_ROWS} rows; found {example_rows}"
            ),
        ));
    }
    let section_text = section.join("\n");
    for block in INTERPRETATION_REQUIRED_BLOCKS {
        if !section_text.contains(block) {
            issues.push(Issue::at(
                heading + 1,
                format!("Interpretation echo missing block: {block}"),
            ));
        }
    }
    issues
}

/// YAML 标量值：去掉行内注释与首尾引号。
fn meaningful_yaml_value(raw: &str) -> String {
    let without_comment = raw.split('#').next().unwrap_or_default().trim();
    without_comment
        .trim_matches(|c| c == '"' || c == '\'')
        .trim()
        .to_string()
}

/// oracle 块必须是顶层的 `- id:` 序列。外面套一层键（`realistic_validation:`）会让块变成映射，
/// 下游抽取器会直接丢掉整份 PRD 而不是报字段级问题——所以要在这里拒绝，那里才修得了。
fn oracle_block_shape_issues(fence_lines: &[&str], fence_start_line: usize) -> Vec<Issue> {
    let first_content_line = fence_lines
        .iter()
        .map(|line| line.trim())
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or_default();
    if first_content_line.starts_with("- ") {
        return Vec::new();
    }
    vec![Issue::at(
        fence_start_line,
        format!(
            "Realistic Validation Plan oracle block must be a top-level sequence starting with `- id:` \
             (got {first_content_line:?}); drop the wrapping key and move any global note such as a \
             `failure_triage:` reminder *beneath* the block, outside the fence"
        ),
    )]
}

type OracleEntry = (String, usize, std::collections::HashMap<String, String>);

fn oracle_schema_issues(content: &str) -> Vec<Issue> {
    let lines = lines_of(content);
    let Some(heading) = lines
        .iter()
        .position(|line| REALISTIC_VALIDATION_HEADING.is_match(line))
    else {
        return vec![Issue::general("Missing Realistic Validation Plan section")];
    };
    let start = heading + 1;
    let end = (start..lines.len())
        .find(|&index| THIRD_LEVEL_HEADING.is_match(lines[index]))
        .unwrap_or(lines.len());
    let section = &lines[start..end];
    let declares_no_executable_behavior = section
        .iter()
        .any(|line| line.contains(NO_EXECUTABLE_BEHAVIOR_MARKER));

    let mut entries: Vec<OracleEntry> = Vec::new();
    let mut current: Option<OracleEntry> = None;
    let mut in_yaml = false;
    let mut first_fence_lines: Vec<&str> = Vec::new();
    let mut first_fence_start = 0;
    let mut collecting_first_fence = false;

    for (offset, line) in section.iter().enumerate() {
        let line_number = start + offset + 1;
        if !in_yaml && YAML_FENCE_START.is_match(line) {
            in_yaml = true;
            if first_fence_start == 0 {
                first_fence_start = line_number;
                collecting_first_fence = true;
            }
            continue;
        }
        if in_yaml && YAML_FENCE_END.is_match(line) {
            collecting_first_fence = false;
            entries.extend(current.take());
            in_yaml = false;
            continue;
        }
        if !in_yaml {
            continue;
        }
        if collecting_first_fence {
            first_fence_lines.push(line);
        }
        if let Some(entry) = ORACLE_ENTRY.captures(line) {
            entries.extend(current.take());
            current = Some((
                meaningful_yaml_value(&entry["value"]),
                line_number,
                std::collections::HashMap::new(),
            ));
            continue;
        }
        if let (Some(field), Some((_, _, fields))) = (ORACLE_FIELD.captures(line), current.as_mut())
        {
            fields.insert(
                field["key"].to_string(),
                meaningful_yaml_value(&field["value"]),
            );
        }
    }
    entries.extend(current.take());

    // 「无可执行行为」的声明只在**确实没有 oracle 条目**时才豁免整个块。
    // 模板里「无可执行行为时本块写：……」那句提示本身就含有这个声明；只要看到这句话就放行的话，
    // 作者忘了删它，真实的 oracle 块就完全不被校验（连 `tier: R0|R1|R2|R3` 这样的占位符都能蒙混过关）。
    if declares_no_executable_behavior && entries.is_empty() {
        return Vec::new();
    }

    // 先查形状再查内容：被包了一层的块依然会把每个字段暴露给下面的逐行扫描，否则它会当成合法的蒙混过关。
    // 完全没有 YAML 围栏时跳过形状检查，让下面更清楚的「必须包含结构化 oracle 块」去报。
    if first_fence_start > 0 {
        let shape_issues = oracle_block_shape_issues(&first_fence_lines, first_fence_start);
        if !shape_issues.is_empty() {
            return shape_issues;
        }
    }
    if entries.is_empty() {
        return vec![Issue::at(
            start,
            "Realistic Validation Plan must contain a structured YAML oracle block",
        )];
    }

    let mut issues = Vec::new();
    for (id, line, fields) in &entries {
        let field = |name: &str| fields.get(name).map(String::as_str).unwrap_or_default();
        let declared_tier = field("tier").to_uppercase();
        if !declared_tier.is_empty() && !VALID_ORACLE_TIERS.contains(&declared_tier.as_str()) {
            issues.push(Issue::at(
                *line,
                format!(
                    "Oracle {id:?} has invalid tier {declared_tier:?}; expected one of {}",
                    VALID_ORACLE_TIERS.join(", ")
                ),
            ));
            continue;
        }
        let declared_reviewer = field("reviewer").to_lowercase();
        if !declared_reviewer.is_empty()
            && !VALID_ORACLE_REVIEWERS.contains(&declared_reviewer.as_str())
        {
            issues.push(Issue::at(
                *line,
                format!(
                    "Oracle {id:?} has invalid reviewer {declared_reviewer:?}; expected one of {}",
                    VALID_ORACLE_REVIEWERS.join(", ")
                ),
            ));
            continue;
        }

        let effective_tier = if declared_tier.is_empty() {
            DEFAULT_ORACLE_TIER
        } else {
            declared_tier.as_str()
        };
        let mut required: Vec<&str> = BASE_ORACLE_FIELDS.to_vec();
        if declared_reviewer == "human" {
            required.extend(HUMAN_REVIEWER_ORACLE_FIELDS);
        }
        if DEEP_CHAIN_TIERS.contains(&effective_tier) {
            required.extend(DEEP_CHAIN_ORACLE_FIELDS);
        }
        if NEGATIVE_CONTROL_TIERS.contains(&effective_tier) {
            required.extend(NEGATIVE_CONTROL_ORACLE_FIELDS);
        }
        // 写明「not feasible」的负控是被接受的结论，此时 expected_fail 无意义；强求它会逼作者编造一个失败模式，
        // 而不是如实记录那个真实的限制。
        if field("negative_control")
            .to_lowercase()
            .starts_with(NEGATIVE_CONTROL_NOT_FEASIBLE_PREFIX)
        {
            required.retain(|name| *name != "expected_fail");
        }

        let mut missing: Vec<&str> = Vec::new();
        for name in required {
            if !missing.contains(&name) && field(name).is_empty() {
                missing.push(name);
            }
        }
        if !missing.is_empty() {
            let tier_note = if declared_tier.is_empty() {
                format!(" (untiered, treated as {DEFAULT_ORACLE_TIER})")
            } else {
                String::new()
            };
            issues.push(Issue::at(
                *line,
                format!(
                    "Oracle {id:?}{tier_note} missing non-empty field(s): {}",
                    missing.join(", ")
                ),
            ));
        }
    }
    issues
}

/// §8 里标量字段的封闭词表。它们由 skill 声明、由本校验器掌管：仓库侧的解析器遇到未知值
/// 完全可以丢掉整份 PRD，于是手误会悄悄删掉一份工作，而不是大声失败。
fn delivery_vocabulary(field: &str) -> Option<&'static [&'static str]> {
    match field.to_lowercase().as_str() {
        "gate type" => Some(&["hard", "none", "soft"]),
        "sequence" => Some(&["stack", "via-main"]),
        _ => None,
    }
}

/// §8 章节内的 `(行号, 去空白后的行)`。规范形状在 `## 8.` 之下还会再套一层 `### Delivery Dependencies`，
/// 所以章节止于同级或更高级的标题，而不是第一个 `#` 行。
fn delivery_section_lines(content: &str) -> Vec<(usize, String)> {
    let lines = lines_of(content);
    let Some(start) = lines
        .iter()
        .position(|line| DELIVERY_DEPENDENCIES_HEADING.is_match(line))
    else {
        return Vec::new();
    };
    let section_level = heading_level(lines[start]);
    let mut result = Vec::new();
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if line.starts_with('#') {
            if heading_level(line) <= section_level {
                break;
            }
            continue;
        }
        let stripped = line.trim();
        if !stripped.is_empty() {
            result.push((index + 1, stripped.to_string()));
        }
    }
    result
}

fn delivery_field_value_issues(content: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    for (line, stripped) in delivery_section_lines(content) {
        let Some(captures) = DELIVERY_SCALAR_FIELD.captures(&stripped) else {
            continue;
        };
        let written_name = captures["key"].trim().to_string();
        let Some(vocabulary) = delivery_vocabulary(&written_name) else {
            continue;
        };
        let value = captures["value"].trim();
        // 值为空说明字段声明了但留空；契约规定省略的字段回落到文档化的默认值，没什么可校验的。
        if value.is_empty() || vocabulary.contains(&value.to_lowercase().as_str()) {
            continue;
        }
        issues.push(Issue::at(
            line,
            format!(
                "Invalid '{written_name}' value in Section 8: {value:?}. Expected one of: {}. See '8. Delivery Dependencies' in the PRD skill.",
                vocabulary.join(", ")
            ),
        ));
    }
    issues
}

/// §8 `Depends on tasks/issues` 下声明的上游引用。
fn declared_delivery_refs(content: &str) -> Vec<String> {
    let lines = lines_of(content);
    let Some(start) = lines
        .iter()
        .position(|line| DELIVERY_DEPENDENCIES_HEADING.is_match(line))
    else {
        return Vec::new();
    };
    let section_level = heading_level(lines[start]);
    let depends_on = regex(r"(?i)^-\s*Depends on tasks/issues\s*:");
    let other_field = regex(r"(?i)^-\s*(Gate type|Sequence|Notes)\s*:");

    let mut refs = Vec::new();
    let mut inside_depends_field = false;
    for line in &lines[start + 1..] {
        if line.starts_with('#') {
            if heading_level(line) <= section_level {
                break;
            }
            continue;
        }
        let stripped = line.trim();
        if depends_on.is_match(stripped) {
            inside_depends_field = true;
            let inline = stripped
                .split_once(':')
                .map(|(_, value)| value.trim())
                .unwrap_or_default();
            if !inline.is_empty() {
                refs.extend(delivery_ref_token(inline));
            }
        } else if other_field.is_match(stripped) {
            inside_depends_field = false;
        } else if inside_depends_field && stripped.starts_with('-') {
            refs.extend(delivery_ref_token(
                stripped.trim_start_matches(['-', ' ']).trim(),
            ));
        }
    }
    refs
}

/// 一条声明值里有意义的上游引用（取文件名，横幅里可以写路径也可以只写短标识）。
fn delivery_ref_token(raw: &str) -> Option<String> {
    let value = raw.trim().trim_matches('`').trim();
    // 作者常写 `none（本 PRD 是善后，无未完成上游）`：开头的 token 才声明依赖，后面是说明文字。
    // 不能按 `-` 拆分：PRD 的 slug（如 `P1-FEAT-...`）本身就含 `-`。
    let leading = value
        .split(['（', '(', ',', '，', ';', '；', ' ', '\t'])
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches('`');
    if leading.is_empty() || ["none", "n/a", "无"].contains(&leading.to_lowercase().as_str()) {
        return None;
    }
    std::path::Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
}

/// 顶部的交付前置横幅是 §8 的投影：只需检查它存在、在 Part A 之上，并且与 §8 在唯一重要的一点上一致
/// ——是否被挡住、被谁挡住。
fn delivery_gate_banner_issues(content: &str) -> Vec<Issue> {
    let lines = lines_of(content);
    let Some(part_a) = lines.iter().position(|line| PART_A_HEADING.is_match(line)) else {
        // 缺 Part A 已由 Part A 元数据检查报告。
        return Vec::new();
    };
    let Some(banner_index) = (0..part_a).find(|&index| DELIVERY_GATE_BANNER.is_match(lines[index]))
    else {
        return vec![Issue::at(
            1,
            "Missing Delivery Gate Banner under the title: every PRD must state 上游依赖 up front, including \
             when there is none. See 'Delivery Gate Banner' in the PRD skill.",
        )];
    };

    // 横幅可以折成连续的多行引用块。
    let mut banner_end = banner_index;
    while banner_end + 1 < part_a && lines[banner_end + 1].trim_start().starts_with('>') {
        banner_end += 1;
    }
    let banner_text = lines[banner_index..=banner_end].join(" ");

    let declared = declared_delivery_refs(content);
    if declared.is_empty() {
        let says_none = ["无", "none", "no upstream"]
            .iter()
            .any(|token| banner_text.to_lowercase().contains(token) || banner_text.contains(token));
        if !says_none {
            return vec![Issue::at(
                banner_index + 1,
                "Delivery Gate Banner must say there is no dependency (e.g. 无 / none) because Section 8 declares none.",
            )];
        }
        return Vec::new();
    }
    let mut missing: Vec<&String> = declared
        .iter()
        .filter(|reference| !banner_text.contains(reference.as_str()))
        .collect();
    if missing.is_empty() {
        return Vec::new();
    }
    missing.sort();
    vec![Issue::at(
        banner_index + 1,
        format!(
            "Delivery Gate Banner does not name every upstream declared in Section 8: missing {}. Section 8 \
             is the source of truth; update the banner to match it.",
            missing
                .iter()
                .map(|reference| reference.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )]
}

#[cfg(test)]
mod tests;
