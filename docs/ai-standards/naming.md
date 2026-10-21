# Naming Standards

本页给出 Rust 的命名约定。核心原则沿用本模板一贯的 **Fully Qualified Naming**：让数据来源、状态和职责一眼可见。

## Rust 基本约定（不可违背）

- 函数、变量、模块、文件：`snake_case`
- 类型、trait、枚举（及其变体）：`UpperCamelCase`
- 常量、静态量：`SCREAMING_SNAKE_CASE`
- crate 名用 `kebab-case`（引用时变成下划线），本仓库内部 crate 统一前缀 `tpl-`
- 转换方法：`as_`（廉价借用视图）、`to_`（昂贵复制）、`into_`（消耗 self）
- 返回 `bool` 的方法用 `is_` / `has_` / `can_` 开头

## 语义命名

- 拒绝 `data`、`item`、`res`、`obj`、`tmp`、`val` 这类泛型变量名
- 变量名应包含来源、类型或状态
- 让名称本身解释数据流，而不是依赖上下文猜测

好例子：`raw_request_body_text`、`parsed_database_settings`、`readiness_report`、`failure_reason`
差例子：`data`、`result`、`tmp`

> 约定俗成的短名可以保留：迭代器里的 `i`、闭包里的单字母参数、`self`、`err`（极短作用域）。判断标准是「脱离上下文还看得懂吗」。

## 逐步产生新名字（Rust 里的 SSA）

每一步产生新变量，让解析、校验、转换、调用和输出各有自己的名字。Rust 的**遮蔽（shadowing）**是惯用手法，但只在「同一数据换了形态」时用：

```rust
// 推荐：形态变化用遮蔽，语义变化用新名字
let port_text = std::env::var("PORT")?;
let port: u16 = port_text.parse()?;          // 同一个值：文本 → 数字，遮蔽或新名都可以
let bind_address = format!("0.0.0.0:{port}"); // 语义变了：新名字

// 不推荐：反复覆盖含义不同的变量
let mut data = read_input();
data = clean(data);
data = call_service(data);
```

优先不可变（`let`），确实需要变化才用 `let mut`。

## Types As Prompts

- 用类型表达约束：用 `enum` 代替字符串状态，用新类型（`struct UserId(i64)`）区分含义不同的同类值
- 公共函数的参数和返回值类型必须显式，不依赖推断来「解释」接口
- 当名字仍不足以表达约束时，用类型补齐，而不是加注释

## 错误与结果

- 错误类型以 `Error` 结尾（`ConfigError`、`AgeError`）；库层用 `thiserror` 定义
- 返回 `Result` 的公共函数在文档里写 `# Errors` 小节

## 测试命名

- 测试函数名写成一句完整的行为描述：`ready_is_503_and_hides_the_failure_reason_when_a_dependency_is_down`
- 集成测试文件按被测对象命名：`health_routes.rs`
