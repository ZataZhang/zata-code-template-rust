//! PRD Machine Contract 的解析原语 —— PRD 格式的**唯一实现**。
//!
//! `skills/prd/SKILL.md` 的 `Machine Contract` 章节定义了 PRD 里若干**被机器解析**的格式：
//! 验收清单的复选框、`Human-Confirmed` 分组、验收状态横幅、Change Log 条目。
//!
//! 为什么要做成一个库：Python 版里同一份解析被抄了三份（skill 的校验器、`just` 的看板、
//! 提交钩子），靠一个守卫测试逐例比对来防漂移，而契约文本与实现仍然漂移过。这里只有这一份，
//! 校验器、看板、钩子都**调用**它，不再各自重写。
//!
//! 本模块只做**解析**，不做门禁裁决：哪些未勾项该拦、哪些该放行（例如 `Human-Confirmed`
//! 组内的空框属于人属项，不拦归档）由契约规定、由 [`super::check`] 按契约取用，这里只保证把结构如实报出来。

use std::sync::LazyLock;

use regex::Regex;

/// 本模块实现的 Machine Contract 版本；与 `SKILL.md` 的 `Machine-Contract-Version` 标记必须一致。
pub const CONTRACT_VERSION: u32 = 5;

/// `[~]` 的标记字符：表示「等 runner 门禁，不计未勾」，绝不是「已完成」。
pub const RESOLVED_MARK: char = '~';

/// 编译一个写死的正则；写错了是程序员的错，直接 panic。
fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|error| panic!("内置正则无效 {pattern:?}：{error}"))
}

/// `## Acceptance Checklist` / `## 验收清单`（允许编号前缀与双语后缀）。
pub static ACCEPTANCE_CHECKLIST_HEADING: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^##\s+(?:\d+\.\s+)?(?:Acceptance Checklist\b.*|验收清单.*)\s*$"));

/// 顶级标题（`## `）；清单章节止于下一个它。
pub static TOP_LEVEL_HEADING: LazyLock<Regex> = LazyLock::new(|| regex(r"^##\s+"));

/// 只认 `- [ ]` / `- [x]` / `- [X]`；`[~]` 刻意**不是**复选框。
static CHECKBOX: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^\s*[-*+]\s+\[(?P<mark>[ xX])\]\s*(?P<label>.*)$"));

static RESOLVED_ITEM: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s*[-*+]\s+\[~\]"));

static CODE_FENCE: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s*(?:```|~~~)"));

/// 分组标题：三级及以下标题。清单章节内用标题表达分组。
static GROUP_HEADING: LazyLock<Regex> = LazyLock::new(|| regex(r"^(#{3,6})\s+(.+?)\s*$"));

/// 整行加粗的分组标签（如 `**Human-Confirmed**`）。契约规定的分组形式是标题；整行加粗属于
/// 兼容存量 PRD 的宽容输入。要求整行只有加粗内容，因此行内强调（`**X：** 正文`）不会命中。
static BOLD_GROUP_LABEL: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^\s*(?:\*\*|__)\s*(?P<label>.+?)\s*(?:\*\*|__)\s*$"));

/// 粗体分组标签没有层级，按三级处理，复用「遇同级或更高级分组即关闭」的规则。
const BOLD_GROUP_DEPTH: usize = 3;

/// `Human-Confirmed` 分组标签的匹配前缀（大小写不敏感）。用前缀而非精确相等：允许
/// `Human-Confirmed (来自 Part A 风险地图)` 这类带说明后缀的写法。
const HUMAN_CONFIRMED_GROUP_PREFIX: &str = "human-confirmed";

/// 清单章节内是否**提到过** Human-Confirmed（不限书写形式），用于诊断「分组没被识别出来」。
static HUMAN_CONFIRMED_MENTION: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)human[-\s_]?confirmed"));

/// 验收状态横幅所在行：引用块（`>` 开头），且带可 grep 的字面量 `验收状态`（或 `Acceptance Status`）。
/// 只在引用块行里找，避免把正文或决策日志里对横幅的讨论误当成状态声明。
static ACCEPTANCE_STATUS_BANNER: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)^\s*>\s*.*?(?:验收状态|Acceptance Status)"));

static CHANGE_LOG_HEADING: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?i)^##\s+(?:\d+\.\s+)?(?:Change Log\b.*|变更记录.*)\s*$"));

/// 每条变更记录是一个 `###` 标题。
static CHANGE_ENTRY_HEADING: LazyLock<Regex> = LazyLock::new(|| regex(r"^###\s+.+"));

/// 契约 §1 规定的六个字段及其（中英文）行首写法；缺任一即视为不完整条目。
static CHANGE_LOG_FIELDS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        ("类型", "类型|Type"),
        ("原文", "原文|Before"),
        ("变更后", "变更后|After"),
        ("原因", "原因|Reason"),
        ("影响", "影响|Impact"),
        ("审核", "审核|Review"),
    ]
    .into_iter()
    .map(|(name, alternatives)| {
        (
            name,
            regex(&format!(r"(?i)^\s*[-*+]\s+(?:{alternatives})\s*[:：]")),
        )
    })
    .collect()
});

/// 验收清单里的一条复选框。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecklistItem {
    /// 1 起始的行号。
    pub line: usize,
    /// 原始行文本（右侧空白已去除）。
    pub text: String,
    /// `' '` 未勾 / `'x'` 已勾 / `'~'` runner-owned 豁免。
    pub mark: char,
    /// 最近一层分组标签；不在任何分组内时为 `None`。
    pub group: Option<String>,
    /// 本条是否处于 Human-Confirmed 分组的**作用范围内**。分组可以嵌套，因此这里记录的是
    /// 「包含它的任一层分组是 Human-Confirmed」，而不是「它的直接分组标签叫 Human-Confirmed」。
    pub in_human_group: bool,
}

/// 验收清单章节的解析结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChecklistState {
    /// 是否找到清单章节。
    pub section_found: bool,
    /// 章节内全部复选框条目（含已勾与 `[~]`）。
    pub items: Vec<ChecklistItem>,
    /// 是否识别出 Human-Confirmed 分组。
    pub human_group_found: bool,
    /// 章节内是否出现过 `Human-Confirmed` 字样。`mentioned` 为真而 `group_found` 为假，说明分组
    /// 写法没被识别——调用方据此给出可行动的诊断，而不是让 agent 自己猜。
    pub human_confirmed_mentioned: bool,
}

impl ChecklistState {
    fn matching(&self, predicate: impl Fn(&ChecklistItem) -> bool) -> Vec<&ChecklistItem> {
        self.items.iter().filter(|item| predicate(item)).collect()
    }

    /// 全部未勾条目（`- [ ]`），**不排除**人属分组。
    pub fn unchecked_items(&self) -> Vec<&ChecklistItem> {
        self.matching(|item| item.mark == ' ')
    }

    /// 已勾条目（`- [x]`）。
    pub fn checked_items(&self) -> Vec<&ChecklistItem> {
        self.matching(|item| item.mark == 'x')
    }

    /// `[~]` 条目。
    pub fn resolved_items(&self) -> Vec<&ChecklistItem> {
        self.matching(|item| item.mark == RESOLVED_MARK)
    }

    /// 位于 Human-Confirmed 分组作用范围内的条目（含 `[~]` 后置门禁）。
    pub fn human_items(&self) -> Vec<&ChecklistItem> {
        self.matching(|item| item.in_human_group)
    }

    /// 未勾且**不在**人属分组内的条目——执行侧真正要处理的那些。
    ///
    /// 契约规定 Human-Confirmed 组内的空框由人填写，执行工具不得代勾，因此它们从「执行项」里
    /// 排除；`[~]` 本就不算未勾。归档门禁只看这一组：执行侧交付完整即可归档，人属项留着等人填。
    pub fn execution_unchecked_items(&self) -> Vec<&ChecklistItem> {
        self.matching(|item| item.mark == ' ' && !item.in_human_group)
    }

    /// 未勾且**位于**人属分组内的条目——还在等人确认的那些。与 [`Self::execution_unchecked_items`]
    /// 互补：两者合起来恰好是 [`Self::unchecked_items`]。横幅是 `待人工验收` 还是 `已验收`，
    /// 取决于这一组是否为空。
    pub fn human_unchecked_items(&self) -> Vec<&ChecklistItem> {
        self.matching(|item| item.mark == ' ' && item.in_human_group)
    }

    /// `(已勾数, 总数)`：看板的进度口径。`[~]` 门禁项不计入；分组（含 Human-Confirmed）不影响计数。
    pub fn progress(&self) -> (usize, usize) {
        let counted = self.matching(|item| item.mark == ' ' || item.mark == 'x');
        let checked = counted.iter().filter(|item| item.mark == 'x').count();
        (checked, counted.len())
    }
}

/// 验收状态横幅的三态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptanceStatus {
    /// `⬜ 未开工`。
    NotStarted,
    /// `🧍 待人工验收`：执行侧完成，等人确认。
    AwaitingHuman,
    /// `✅ 已验收`（旧称 `✅ 可归档` 按已验收读）。
    Accepted,
}

/// 状态词到状态值的映射。`可归档` 是 v4 及以前 `已验收` 的旧称：存量 PRD 里的 `✅ 可归档`
/// 一律按已验收读，不要求回头改文件。
const STATUS_TOKENS: [(AcceptanceStatus, &str); 4] = [
    (AcceptanceStatus::NotStarted, "未开工"),
    (AcceptanceStatus::AwaitingHuman, "待人工验收"),
    (AcceptanceStatus::Accepted, "已验收"),
    (AcceptanceStatus::Accepted, "可归档"),
];

/// 验收状态横幅的解析结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptanceBanner {
    /// 横幅首行的 1 起始行号。
    pub line: usize,
    /// 状态；横幅在但状态词无法识别时为 `None`。
    pub status: Option<AcceptanceStatus>,
}

/// 一条 Change Log 记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeLogEntry {
    /// `###` 标题所在行号（1 起始）。
    pub line: usize,
    /// 标题文本。
    pub title: String,
    /// 缺失的字段名。
    pub missing_fields: Vec<&'static str>,
}

/// Change Log 章节的解析结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeLogState {
    /// 是否存在 `## Change Log` / `## 变更记录` 章节。
    pub section_found: bool,
    /// 按出现顺序的条目。
    pub entries: Vec<ChangeLogEntry>,
}

impl ChangeLogState {
    /// 章节存在、至少一条记录，且没有缺字段的条目。
    pub fn is_complete(&self) -> bool {
        self.section_found
            && !self.entries.is_empty()
            && self
                .entries
                .iter()
                .all(|entry| entry.missing_fields.is_empty())
    }
}

/// Acceptance Checklist 章节的半开区间 `(start, end)`（0 起始）。`start` 指向标题的**下一行**；
/// 找不到章节时返回 `None`。
pub fn acceptance_section_bounds(lines: &[&str]) -> Option<(usize, usize)> {
    let heading_index = lines
        .iter()
        .position(|line| ACCEPTANCE_CHECKLIST_HEADING.is_match(line))?;
    let end_index = (heading_index + 1..lines.len())
        .find(|&index| TOP_LEVEL_HEADING.is_match(lines[index]))
        .unwrap_or(lines.len());
    Some((heading_index + 1, end_index))
}

fn is_human_confirmed_label(label: &str) -> bool {
    label
        .trim()
        .to_lowercase()
        .starts_with(HUMAN_CONFIRMED_GROUP_PREFIX)
}

/// 一层分组：`(层级, 标签, 是否人属组)`。
type Group = (usize, String, bool);

/// 解析 PRD 的 Acceptance Checklist 章节。
///
/// 只统计章节内的复选框；围栏代码块内的内容忽略。分组由三级及以下标题表达（整行加粗标签作为
/// 兼容输入一并接受）；遇到同级或更高级的分组即关闭当前分组。条目归属「包含它的所有分组」，
/// 因此人属判定看整条分组栈，而不是只看最近一层——嵌套子标题不该把人属项踢出去。
pub fn parse_checklist(content: &str) -> ChecklistState {
    let lines: Vec<&str> = content.lines().collect();
    let Some((start, end)) = acceptance_section_bounds(&lines) else {
        return ChecklistState::default();
    };

    let mut items = Vec::new();
    let mut in_code_block = false;
    let mut group_stack: Vec<Group> = Vec::new();
    let mut human_group_found = false;
    let mut human_confirmed_mentioned = false;

    let mut enter_group = |stack: &mut Vec<Group>, depth: usize, label: &str| {
        while stack
            .last()
            .is_some_and(|(top_depth, _, _)| depth <= *top_depth)
        {
            stack.pop();
        }
        let is_human = is_human_confirmed_label(label);
        stack.push((depth, label.to_string(), is_human));
        human_group_found |= is_human;
    };

    for (offset, line) in lines[start..end].iter().enumerate() {
        let line_number = start + offset + 1;
        if CODE_FENCE.is_match(line) {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }
        if HUMAN_CONFIRMED_MENTION.is_match(line) {
            human_confirmed_mentioned = true;
        }
        if let Some(heading) = GROUP_HEADING.captures(line) {
            enter_group(&mut group_stack, heading[1].len(), heading[2].trim());
            continue;
        }
        if let Some(bold) = BOLD_GROUP_LABEL.captures(line) {
            enter_group(&mut group_stack, BOLD_GROUP_DEPTH, bold["label"].trim());
            continue;
        }

        let mark = if let Some(checkbox) = CHECKBOX.captures(line) {
            checkbox["mark"].to_lowercase().chars().next()
        } else if RESOLVED_ITEM.is_match(line) {
            Some(RESOLVED_MARK)
        } else {
            None
        };
        if let Some(mark) = mark {
            items.push(ChecklistItem {
                line: line_number,
                text: line.trim_end().to_string(),
                mark,
                group: group_stack.last().map(|(_, label, _)| label.clone()),
                in_human_group: group_stack.iter().any(|(_, _, is_human)| *is_human),
            });
        }
    }

    ChecklistState {
        section_found: true,
        items,
        human_group_found,
        human_confirmed_mentioned,
    }
}

/// 定位 PRD 头部的验收状态横幅并解析其三态（Machine Contract §6 / §8）。
///
/// 横幅是 §9 验收清单的投影，只有 `⬜ 未开工` / `🧍 待人工验收` / `✅ 已验收` 三个状态。命中首行
/// 横幅后取标记之后**最先出现**的状态词，模板自带的括号说明（`未开工（…改为 🧍 待人工验收…）`）
/// 因此不会把默认态误判成待人工。只认这一处显式声明，不按 §9 的未勾项结构反推——结构推断会把
/// 尚未完工的 PRD 误报成「等你验收」。
pub fn find_acceptance_banner(content: &str) -> Option<AcceptanceBanner> {
    for (index, line) in content.lines().enumerate() {
        let Some(banner_match) = ACCEPTANCE_STATUS_BANNER.find(line) else {
            continue;
        };
        let remainder = &line[banner_match.end()..];
        let status = STATUS_TOKENS
            .iter()
            .filter_map(|(status, token)| remainder.find(token).map(|offset| (offset, *status)))
            .min_by_key(|(offset, _)| *offset)
            .map(|(_, status)| status);
        return Some(AcceptanceBanner {
            line: index + 1,
            status,
        });
    }
    None
}

/// 验收状态横幅的状态值；没有横幅或状态词无法识别时为 `None`。
pub fn parse_acceptance_status(content: &str) -> Option<AcceptanceStatus> {
    find_acceptance_banner(content).and_then(|banner| banner.status)
}

/// 解析 PRD 的 Change Log 章节（Machine Contract §1）。
///
/// 章节止于下一个 `##` 标题；每条记录是一个 `###` 标题加六个 `- 字段：` 行。缺字段的条目照样
/// 返回（在 `missing_fields` 里点名），由调用方决定怎么用——「改了 PRD 没写完整 Change Log」
/// 是门禁问题，不是解析问题。
pub fn parse_change_log(content: &str) -> ChangeLogState {
    let lines: Vec<&str> = content.lines().collect();
    let Some(heading_index) = lines
        .iter()
        .position(|line| CHANGE_LOG_HEADING.is_match(line))
    else {
        return ChangeLogState::default();
    };
    let end_index = (heading_index + 1..lines.len())
        .find(|&index| TOP_LEVEL_HEADING.is_match(lines[index]))
        .unwrap_or(lines.len());

    let mut entries = Vec::new();
    let mut current: Option<(usize, String, Vec<&'static str>)> = None;
    let flush = |current: &mut Option<(usize, String, Vec<&'static str>)>,
                 entries: &mut Vec<ChangeLogEntry>| {
        if let Some((line, title, found)) = current.take() {
            let missing_fields = CHANGE_LOG_FIELDS
                .iter()
                .map(|(name, _)| *name)
                .filter(|name| !found.contains(name))
                .collect();
            entries.push(ChangeLogEntry {
                line,
                title,
                missing_fields,
            });
        }
    };

    for (index, line) in lines
        .iter()
        .enumerate()
        .take(end_index)
        .skip(heading_index + 1)
    {
        if CHANGE_ENTRY_HEADING.is_match(line) {
            flush(&mut current, &mut entries);
            current = Some((index + 1, line[4..].trim().to_string(), Vec::new()));
            continue;
        }
        let Some((_, _, found)) = current.as_mut() else {
            continue;
        };
        if let Some((name, _)) = CHANGE_LOG_FIELDS
            .iter()
            .find(|(_, pattern)| pattern.is_match(line))
        {
            found.push(name);
        }
    }
    flush(&mut current, &mut entries);

    ChangeLogState {
        section_found: true,
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRD: &str = "\
# Title

> 验收状态：🧍 待人工验收

## 9. Acceptance Checklist

### Execution
- [x] built
- [ ] documented
- [~] gated — runner-owned gate: x

### Human-Confirmed (来自 Part A)
- [ ] looks right
#### nested
- [ ] nested item stays human

### Other
- [x] done

```
- [ ] inside fence is ignored
```

## 10. Functional Requirements
- [ ] not part of the checklist
";

    #[test]
    fn contract_version_matches_the_marker_in_the_skill() {
        // 契约文本（SKILL.md）与实现必须同版本：契约章节任何改动都要同步 bump 两处。
        let skill = include_str!("../../../../skills/prd/SKILL.md");
        let marker = Regex::new(r"(?m)^Machine-Contract-Version:\s*(\d+)\s*$").unwrap();
        let declared: u32 = marker
            .captures(skill)
            .expect("SKILL.md 缺少 Machine-Contract-Version 标记")[1]
            .parse()
            .unwrap();
        assert_eq!(
            declared, CONTRACT_VERSION,
            "SKILL.md 与 contract.rs 的契约版本不一致"
        );
    }

    #[test]
    fn checklist_counts_the_three_mark_kinds_and_ignores_fences_and_other_sections() {
        let state = parse_checklist(PRD);

        assert!(state.section_found);
        assert_eq!(state.items.len(), 6);
        assert_eq!(state.checked_items().len(), 2);
        assert_eq!(state.resolved_items().len(), 1);
        assert_eq!(state.unchecked_items().len(), 3);
        assert_eq!(state.progress(), (2, 5), "[~] 不计入进度");
    }

    #[test]
    fn human_group_scope_includes_nested_subheadings() {
        let state = parse_checklist(PRD);

        assert!(state.human_group_found);
        let human_texts: Vec<_> = state
            .human_items()
            .iter()
            .map(|item| item.text.clone())
            .collect();
        assert_eq!(
            human_texts,
            ["- [ ] looks right", "- [ ] nested item stays human"],
            "嵌套的子标题不该把人属项踢出分组"
        );
        assert_eq!(state.human_unchecked_items().len(), 2);
        assert_eq!(state.execution_unchecked_items().len(), 1);
        assert_eq!(
            state.execution_unchecked_items().len() + state.human_unchecked_items().len(),
            state.unchecked_items().len(),
            "执行项与人属项互补，合起来恰好是全部未勾项"
        );
    }

    #[test]
    fn group_label_prefix_match_is_case_insensitive_and_bold_label_is_accepted() {
        for heading in [
            "### human-confirmed",
            "### HUMAN-CONFIRMED（说明）",
            "**Human-Confirmed**",
        ] {
            let content = format!("## Acceptance Checklist\n{heading}\n- [ ] item\n");
            let state = parse_checklist(&content);
            assert!(state.human_group_found, "应识别：{heading}");
            assert_eq!(state.human_unchecked_items().len(), 1, "{heading}");
        }
    }

    #[test]
    fn inline_bold_emphasis_is_not_mistaken_for_a_group_label() {
        let content = "## 验收清单\n**Human-Confirmed：** 这只是行内强调\n- [ ] item\n";
        let state = parse_checklist(content);
        assert!(!state.human_group_found);
        assert!(
            state.human_confirmed_mentioned,
            "但应当提示「提到过却没识别」，让作者知道写法有问题"
        );
    }

    #[test]
    fn missing_checklist_section_is_reported() {
        let state = parse_checklist("# Title\n\n## 1. Intro\n- [ ] x\n");
        assert!(!state.section_found);
        assert!(state.items.is_empty());
    }

    #[test]
    fn banner_takes_the_first_status_word_so_template_hints_do_not_flip_the_state() {
        let template =
            "> **验收状态**：⬜ 未开工（完成后改为 🧍 待人工验收，全部确认后改为 ✅ 已验收）";
        assert_eq!(
            parse_acceptance_status(template),
            Some(AcceptanceStatus::NotStarted)
        );
        assert_eq!(
            parse_acceptance_status("> 验收状态：🧍 待人工验收"),
            Some(AcceptanceStatus::AwaitingHuman)
        );
        assert_eq!(
            parse_acceptance_status("> Acceptance Status: ✅ 已验收"),
            Some(AcceptanceStatus::Accepted)
        );
        assert_eq!(
            parse_acceptance_status("> 验收状态：✅ 可归档"),
            Some(AcceptanceStatus::Accepted),
            "旧称按已验收读"
        );
    }

    #[test]
    fn banner_only_counts_inside_blockquotes() {
        assert_eq!(
            find_acceptance_banner("验收状态：已验收（正文里的讨论，不是横幅）"),
            None
        );
        let unknown = find_acceptance_banner("> 验收状态：进行中").unwrap();
        assert_eq!(
            (unknown.line, unknown.status),
            (1, None),
            "横幅在但状态词不认识"
        );
    }

    #[test]
    fn change_log_names_the_missing_fields() {
        let content = "\
## 14. Change Log

### 2026-10-16 first
- 类型: 澄清
- 原文: a
- 变更后: b
- 原因: c
- 影响: d
- 审核: e

### 2026-10-17 incomplete
- Type: x
- Reason: y
";
        let state = parse_change_log(content);
        assert!(state.section_found);
        assert_eq!(state.entries.len(), 2);
        assert!(state.entries[0].missing_fields.is_empty());
        assert_eq!(
            state.entries[1].missing_fields,
            ["原文", "变更后", "影响", "审核"]
        );
        assert!(!state.is_complete());
        assert!(!parse_change_log("# no log").section_found);
    }
}
