//! PostgreSQL migration 目录变动时重新触发基础设施 crate 构建。

fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
