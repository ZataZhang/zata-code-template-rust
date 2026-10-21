# PRD: Add widget

> ✅ **交付前置**：无，可立即开工。

> 🧍 **验收状态**：待人工验收。

# Part A · 人审层 (Review Layer)

## 1. Introduction & Goals

### Problem Statement

Users cannot do X.

### Interpretation (解读回显)

| 验证方式 | 输入 / 操作 | 期望观察到的结果 |
|---|---|---|
| 👀 人审 + 自动验证 | 用户点击 A | 看到 B |
| 🤖 自动验证 | 用户点击 C | 看到 D |
| 🤖 自动验证 | 空输入 | 提示 E |

**我默默定了这些**

- 歧义点 → 答案

**我理解为不做**

- 可能被期待但排除的能力

## 2. Human Review Map (介入与风险地图)

### 决策一：是否改默认值

**请确认：** 默认值改成 5？

## 3. Usage And Impact After Implementation

Users see B.

## 4. Requirement Shape

- Actor: user

# Part B · 执行器层 (Build Layer)

## 5. Repository Context And Architecture Fit

- Existing path: x

## 6. Recommendation

Do the smallest thing.

## 7. Implementation Guide

### 7.6 Realistic Validation Plan (Oracle 块)

```yaml
- id: rv-1
  behavior: 用户点击 A 看到 B
  reviewer: verifier
  real_entry: "just run"
  expected: "看到 B"
  mock_boundary: "无 mock"
  tier: R1
  test_layer: e2e
  required_for_acceptance: true
```

## 8. Delivery Dependencies

- Depends on tasks/issues:
  - none
- Gate type: none
- Sequence: via-main

## 9. Acceptance Checklist

### Execution

- [x] built

### Human-Confirmed (来自 Part A 风险地图)

- [ ] 默认值决策已确认

## 10. Functional Requirements

- FR-1: do it
- FR-2: do it well

## 11. Non-Goals

- none

## 12. Risks And Follow-Ups

- none

## 13. Decision Log

| # | 决策 | 选择 |
|---|---|---|
| D-01 | x | y |

### Final Reconciliation (Archive Only)

- Interpretation: confirmed — matches the examples
- Public behavior and contracts: confirmed — unchanged
- Related PRD status: confirmed — none related
- Requirements and risks: confirmed — no new risks
