//! 工具发现并发回归测试。

use super::*;

#[tokio::test]
async fn tool_listing_polls_eight_connections_concurrently() {
    let rig = TestRig::new();
    *rig.remote.listing_barrier.lock().expect("测试同步锁可用") =
        Some(Arc::new(tokio::sync::Barrier::new(8)));
    // 两批连接必须各自全部开始才能返回，串行实现会在第一条等待至超时。
    for index in 0..16 {
        let connection =
            rig.seed_connection(&format!("parallel-{index}"), ConnectionStatus::Active);
        rig.authorize(
            connection.id,
            AssignmentSubject::User(user_identity(&[]).subject_id),
        );
        rig.remote.set_tools(connection.id, &["search"]);
    }
    let tools = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        rig.service.list_authorized_tools(&user_identity(&[])),
    )
    .await
    .expect("健康连接必须并发开始")
    .expect("列出工具");
    assert_eq!(tools.len(), 16);
    assert!(
        tools
            .windows(2)
            .all(|pair| pair[0].exposed_name < pair[1].exposed_name)
    );
}
