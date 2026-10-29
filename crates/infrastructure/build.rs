//! 让迁移文件变化时重新编译：`sqlx::migrate!` 在编译期把 SQL 嵌进二进制，
//! 但 cargo 默认不知道要监视账号服务的迁移目录。

fn main() {
    println!("cargo:rerun-if-changed=../../services/accounts/migrations");
}
