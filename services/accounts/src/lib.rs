//! 装配层（composition root）的库部分：把各层的具体实现接到用例上。
//!
//! 拆成库加薄薄的 `main.rs`，是为了让集成测试能复用**真实的装配代码**，
//! 而不是在测试里另写一份装配逻辑。

pub mod composition;
pub mod server;
pub mod shutdown;
