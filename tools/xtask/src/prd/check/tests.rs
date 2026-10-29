//! `check` 的测试：每条规则一个正例、一个反例；负面用例都是在合规的基线夹具上只改一处。

use super::*;

/// 一份合规的最小 PRD。负面用例都是在它的基础上改一处，所以每条失败都只能归因于那一处改动。
fn valid_prd() -> String {
    include_str!("../../../tests/fixtures/valid-prd.md").to_string()
}

fn messages(issues: &[Issue]) -> Vec<String> {
    issues.iter().map(|issue| issue.message.clone()).collect()
}

fn assert_single_issue(content: &str, gate: Gate, expected_fragment: &str) {
    let issues = validate(content, gate);
    assert_eq!(
        issues.len(),
        1,
        "应该只有一条问题，实际：{:#?}",
        messages(&issues)
    );
    assert!(
        issues[0].message.contains(expected_fragment),
        "期望包含 {expected_fragment:?}，实际：{:?}",
        issues[0].message
    );
}

#[test]
fn the_baseline_fixture_passes_every_gate() {
    for gate in [Gate::Structure, Gate::Delivery, Gate::ArchiveReady] {
        assert_eq!(
            messages(&validate(&valid_prd(), gate)),
            Vec::<String>::new(),
            "{gate:?}"
        );
    }
}

#[test]
fn missing_and_out_of_order_sections_are_reported() {
    let without = valid_prd().replace("## 11. Non-Goals\n\n- none\n\n", "");
    assert_single_issue(
        &without,
        Gate::Structure,
        "Missing required PRD section: Non-Goals",
    );

    let swapped = valid_prd()
        .replace("## 6. Recommendation", "## 6X")
        .replace(
            "## 5. Repository Context And Architecture Fit",
            "## 6. Recommendation\n\nx\n\n## 5. Repository Context And Architecture Fit",
        );
    let issues = messages(&validate(&swapped, Gate::Structure));
    assert!(
        issues
            .iter()
            .any(|issue| issue.contains("must appear after")),
        "{issues:?}"
    );
}

#[test]
fn executor_metadata_leaking_into_part_a_is_rejected() {
    let leaked = valid_prd().replace("Users cannot do X.", "Users cannot do X (see rv-3).");
    assert_single_issue(&leaked, Gate::Structure, "executor-only metadata: \"rv-3\"");
}

#[test]
fn missing_part_headings_are_reported() {
    let no_part_b = valid_prd().replace("# Part B · 执行器层 (Build Layer)", "");
    assert_single_issue(
        &no_part_b,
        Gate::Structure,
        "Missing Part B build-layer heading",
    );
}

#[test]
fn interpretation_echo_needs_enough_example_rows_and_both_blocks() {
    let thin = valid_prd().replace("| 🤖 自动验证 | 空输入 | 提示 E |\n", "");
    assert_single_issue(&thin, Gate::Structure, "at least 3 rows; found 2");

    let no_blocks = valid_prd().replace("**我理解为不做**", "**其他**");
    assert_single_issue(&no_blocks, Gate::Structure, "missing block: 我理解为不做");

    let gone = valid_prd().replace("### Interpretation (解读回显)", "### Something else");
    assert_single_issue(&gone, Gate::Structure, "Missing Section 1 'Interpretation");
}

#[test]
fn functional_requirement_ids_must_be_sequential_from_one() {
    let skipped = valid_prd().replace("FR-2: do it well", "FR-3: do it well");
    assert_single_issue(&skipped, Gate::Structure, "found [1, 3]");
    let duplicated = valid_prd().replace("FR-2: do it well", "FR-1: do it well");
    assert_single_issue(&duplicated, Gate::Structure, "found [1, 1]");
    let none = valid_prd().replace("- FR-1: do it\n- FR-2: do it well\n", "");
    assert_single_issue(&none, Gate::Structure, "must contain FR-1");
}

#[test]
fn executor_owed_unchecked_items_block_delivery_but_not_structure() {
    let owed = valid_prd().replace("- [x] built", "- [ ] built");
    assert!(
        validate(&owed, Gate::Structure).is_empty(),
        "pending PRD 的空框不算结构问题"
    );
    assert_single_issue(&owed, Gate::Delivery, "- [ ] built");
}

#[test]
fn open_human_confirmed_items_never_block_delivery_or_archive() {
    // 基线里 Human-Confirmed 的空框一直开着，三档都通过——这正是「归档不等人」的契约。
    assert!(validate(&valid_prd(), Gate::ArchiveReady).is_empty());
}

#[test]
fn resolved_marker_is_not_counted_as_owed() {
    let gated = valid_prd().replace("- [x] built", "- [~] built — runner-owned gate: ci");
    assert!(validate(&gated, Gate::ArchiveReady).is_empty());
}

#[test]
fn archive_requires_a_banner_that_agrees_with_the_human_group() {
    let not_started =
        valid_prd().replace("🧍 **验收状态**：待人工验收。", "⬜ **验收状态**：未开工。");
    assert_single_issue(&not_started, Gate::ArchiveReady, "still says ⬜ 未开工");

    let no_banner = valid_prd().replace("> 🧍 **验收状态**：待人工验收。\n", "");
    assert_single_issue(
        &no_banner,
        Gate::ArchiveReady,
        "Missing Acceptance Status Banner",
    );

    let unknown = valid_prd().replace("待人工验收。", "进行中。");
    assert_single_issue(&unknown, Gate::ArchiveReady, "not recognised");

    let accepted_but_open =
        valid_prd().replace("🧍 **验收状态**：待人工验收。", "✅ **验收状态**：已验收。");
    assert_single_issue(
        &accepted_but_open,
        Gate::ArchiveReady,
        "1 Human-Confirmed item(s)",
    );

    let all_confirmed = valid_prd().replace("- [ ] 默认值决策已确认", "- [x] 默认值决策已确认");
    assert_single_issue(&all_confirmed, Gate::ArchiveReady, "flip it to ✅ 已验收");
    let consistent =
        all_confirmed.replace("🧍 **验收状态**：待人工验收。", "✅ **验收状态**：已验收。");
    assert!(validate(&consistent, Gate::ArchiveReady).is_empty());
}

#[test]
fn archive_requires_a_filled_in_final_reconciliation() {
    let missing = valid_prd().replace("- Requirements and risks: confirmed — no new risks\n", "");
    assert_single_issue(
        &missing,
        Gate::ArchiveReady,
        "missing field: Requirements and risks",
    );

    for placeholder in [
        "[confirmed / corrected — summary]",
        "confirmed / corrected",
        "confirmed/corrected — summary",
        "",
    ] {
        let content = valid_prd().replace("confirmed — matches the examples", placeholder);
        assert_single_issue(
            &content,
            Gate::ArchiveReady,
            "field \"Interpretation\" is incomplete",
        );
    }

    let none = valid_prd().replace("### Final Reconciliation (Archive Only)", "### Reconciled");
    assert_single_issue(
        &none,
        Gate::ArchiveReady,
        "Missing Final Reconciliation section",
    );
    assert!(
        validate(&none, Gate::Delivery).is_empty(),
        "对账只在归档档要求"
    );
}

#[test]
fn oracle_missing_base_fields_are_named() {
    let broken = valid_prd().replace("  mock_boundary: \"无 mock\"\n", "");
    assert_single_issue(
        &broken,
        Gate::Structure,
        "missing non-empty field(s): mock_boundary",
    );
}

#[test]
fn oracle_depth_scales_with_tier() {
    let r2 = valid_prd().replace("tier: R1", "tier: R2");
    assert_single_issue(
        &r2,
        Gate::Structure,
        "critical_value_source, must_cross, forbidden_bypasses, fresh_state_probe, final_tree_evidence",
    );

    let r3 = valid_prd().replace("tier: R1", "tier: R3").replace(
        "  required_for_acceptance: true\n",
        "  required_for_acceptance: true\n  critical_value_source: a\n  must_cross: b\n  forbidden_bypasses: c\n  fresh_state_probe: d\n  final_tree_evidence: e\n",
    );
    assert_single_issue(&r3, Gate::Structure, "negative_control, expected_fail");

    let infeasible = r3.replace(
        "  final_tree_evidence: e\n",
        "  final_tree_evidence: e\n  negative_control: not feasible — 无法在不改生产代码的前提下变红\n",
    );
    assert!(
        validate(&infeasible, Gate::Structure).is_empty(),
        "写明 not feasible 即可省略 expected_fail：{:?}",
        messages(&validate(&infeasible, Gate::Structure))
    );
}

#[test]
fn untiered_oracle_is_treated_as_the_strictest_tier() {
    let untiered = valid_prd().replace("  tier: R1\n", "");
    let issues = messages(&validate(&untiered, Gate::Structure));
    assert_eq!(issues.len(), 1);
    assert!(issues[0].contains("untiered, treated as R3"), "{issues:?}");
    assert!(issues[0].contains("tier"), "{issues:?}");
}

#[test]
fn human_reviewer_oracle_needs_a_presentation() {
    let human = valid_prd().replace("reviewer: verifier", "reviewer: human");
    assert_single_issue(
        &human,
        Gate::Structure,
        "missing non-empty field(s): presentation",
    );
    let ok = human.replace(
        "  tier: R1\n",
        "  tier: R1\n  presentation: \"截图 + open 命令\"\n",
    );
    assert!(validate(&ok, Gate::Structure).is_empty());
}

#[test]
fn invalid_tier_and_reviewer_values_are_rejected() {
    let tier = valid_prd().replace("tier: R1", "tier: R9");
    assert_single_issue(&tier, Gate::Structure, "invalid tier \"R9\"");
    let reviewer = valid_prd().replace("reviewer: verifier", "reviewer: robot");
    assert_single_issue(&reviewer, Gate::Structure, "invalid reviewer \"robot\"");
}

#[test]
fn a_wrapped_oracle_block_is_rejected_by_shape_before_content() {
    let wrapped = valid_prd().replace(
        "```yaml\n- id: rv-1\n",
        "```yaml\nrealistic_validation:\n- id: rv-1\n",
    );
    assert_single_issue(&wrapped, Gate::Structure, "must be a top-level sequence");
}

#[test]
fn no_executable_behavior_marker_waives_the_oracle_block() {
    let docs_only = valid_prd().replace(
        "```yaml\n- id: rv-1\n  behavior: 用户点击 A 看到 B\n  reviewer: verifier\n  real_entry: \"just run\"\n  expected: \"看到 B\"\n  mock_boundary: \"无 mock\"\n  tier: R1\n  test_layer: e2e\n  required_for_acceptance: true\n```\n",
        "- `No executable behavior changes; realistic validation is limited to documentation/build checks.`\n",
    );
    assert!(validate(&docs_only, Gate::ArchiveReady).is_empty());
    let no_block = valid_prd().replace("```yaml", "```text");
    assert_single_issue(
        &no_block,
        Gate::Structure,
        "must contain a structured YAML oracle block",
    );
}

#[test]
fn keeping_the_no_executable_behavior_sentence_does_not_waive_a_real_oracle_block() {
    // 模板的提示句含有豁免声明；作者忘了删它时，真实的 oracle 块仍必须被校验。
    let sloppy = valid_prd()
        .replace("tier: R1", "tier: R9")
        .replace(
            "```

## 8.",
            "```

- `No executable behavior changes; realistic validation is limited to documentation/build checks.`\n\n## 8.",
        );
    assert_single_issue(&sloppy, Gate::Structure, "invalid tier \"R9\"");
}

#[test]
fn delivery_scalar_fields_use_closed_vocabularies() {
    let bad_gate = valid_prd().replace("Gate type: none", "Gate type: maybe");
    assert_single_issue(
        &bad_gate,
        Gate::Structure,
        "Invalid 'Gate type' value in Section 8: \"maybe\"",
    );
    let bad_sequence = valid_prd().replace("Sequence: via-main", "Sequence: parallel");
    assert_single_issue(&bad_sequence, Gate::Structure, "Invalid 'Sequence'");
    let blank = valid_prd().replace("Sequence: via-main", "Sequence:");
    assert!(
        validate(&blank, Gate::Structure).is_empty(),
        "留空 = 回落到默认值"
    );
}

#[test]
fn delivery_banner_must_exist_and_agree_with_section_8() {
    let missing = valid_prd().replace("> ✅ **交付前置**：无，可立即开工。\n", "");
    assert_single_issue(&missing, Gate::Structure, "Missing Delivery Gate Banner");

    let lies = valid_prd().replace("交付前置**：无，可立即开工。", "交付前置**：被阻塞。");
    assert_single_issue(&lies, Gate::Structure, "must say there is no dependency");

    let with_upstream = valid_prd().replace(
        "  - none\n- Gate type: none",
        "  - `tasks/pending/P1-FEAT-20260601-000000-upstream.md`\n- Gate type: hard",
    );
    assert_single_issue(
        &with_upstream,
        Gate::Structure,
        "missing P1-FEAT-20260601-000000-upstream.md",
    );
    let named = with_upstream.replace(
        "交付前置**：无，可立即开工。",
        "交付前置**：⛔ 依赖 P1-FEAT-20260601-000000-upstream.md。",
    );
    assert!(validate(&named, Gate::Structure).is_empty());
}

#[test]
fn none_with_a_parenthetical_rationale_still_means_no_dependency() {
    let content = valid_prd().replace("  - none\n", "  - none（本 PRD 是善后，无未完成上游）\n");
    assert!(validate(&content, Gate::Structure).is_empty());
}
