//! A2A Card、JSON-RPC 与流式事件解析的行为测试。

use super::*;

fn card(document: Value) -> A2aCard {
    serde_json::from_value(document).unwrap()
}

fn base_card() -> Value {
    json!({
        "name": "Remote Agent",
        "version": "1.0.0",
        "supportedInterfaces": [{
            "url": "https://agent.example.com/a2a",
            "protocolBinding": "JSONRPC",
            "protocolVersion": "1.0.0"
        }],
        "capabilities": {"streaming": true},
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain"],
        "skills": []
    })
}

#[test]
fn card_requires_text_input_and_output_modes() {
    let card_url = Url::parse("https://agent.example.com/card").unwrap();
    let mut document = base_card();
    document["defaultOutputModes"] = json!(["application/json"]);
    assert_eq!(
        card(document).into_loaded(&card_url, false),
        Err(request_failed())
    );
}

#[test]
fn card_selects_first_same_origin_jsonrpc_interface() {
    let card_url = Url::parse("https://agent.example.com/card").unwrap();
    let mut document = base_card();
    document["supportedInterfaces"] = json!([
        {"url": "https://other.example.com/a2a", "protocolBinding": "JSONRPC", "protocolVersion": "1.0"},
        {"url": "https://agent.example.com/a2a-http", "protocolBinding": "HTTP+JSON", "protocolVersion": "1.0"},
        {"url": "https://agent.example.com/a2a", "protocolBinding": "JSONRPC", "protocolVersion": "1.0.0"}
    ]);
    let loaded = card(document).into_loaded(&card_url, false).unwrap();
    assert_eq!(loaded.endpoint.as_str(), "https://agent.example.com/a2a");
    assert!(loaded.streaming);
}

#[test]
fn card_rejects_missing_jsonrpc_interface() {
    let card_url = Url::parse("https://agent.example.com/card").unwrap();
    let mut document = base_card();
    document["supportedInterfaces"] = json!([
        {"url": "https://agent.example.com/a2a", "protocolBinding": "HTTP+JSON", "protocolVersion": "1.0"}
    ]);
    assert_eq!(
        card(document).into_loaded(&card_url, false),
        Err(request_failed())
    );
}

#[test]
fn bearer_security_requires_a_token_and_bearer_scheme() {
    let card_url = Url::parse("https://agent.example.com/card").unwrap();
    let mut document = base_card();
    document["securitySchemes"] = json!({
        "bearer": {"httpAuthSecurityScheme": {"scheme": "bearer"}}
    });
    document["securityRequirements"] = json!([{"schemes": {"bearer": {"list": []}}}]);
    let secured = card(document.clone());
    assert_eq!(
        secured.into_loaded(&card_url, false),
        Err(authentication_required())
    );
    assert!(card(document).into_loaded(&card_url, true).is_ok());

    let mut unsupported = base_card();
    unsupported["securitySchemes"] = json!({
        "key": {"apiKeySecurityScheme": {"location": "header", "name": "X-Key"}}
    });
    unsupported["securityRequirements"] = json!([{"schemes": {"key": {"list": []}}}]);
    assert_eq!(
        card(unsupported).into_loaded(&card_url, true),
        Err(request_failed())
    );
}

#[test]
fn skill_security_requirements_are_enforced() {
    let card_url = Url::parse("https://agent.example.com/card").unwrap();
    let mut document = base_card();
    document["securitySchemes"] = json!({
        "bearer": {"httpAuthSecurityScheme": {"scheme": "bearer"}}
    });
    document["skills"] = json!([{
        "id": "s1",
        "name": "s1",
        "description": "s1",
        "tags": [],
        "securityRequirements": [{"schemes": {"bearer": {"list": []}}}]
    }]);
    assert_eq!(
        card(document).into_loaded(&card_url, false),
        Err(authentication_required())
    );
}

#[test]
fn message_text_uses_the_source_template() {
    assert_eq!(compose_message_text("", "hello"), "hello");
    assert_eq!(
        compose_message_text("Be brief.", "hello"),
        "Follow these instructions when responding to the user.\n\nBe brief.\n\nUser message:\nhello"
    );
}

#[test]
fn artifact_updates_append_to_the_same_artifact() {
    let mut artifacts = ArtifactAccumulator::default();
    artifacts
        .apply(
            &json!({"artifactId": "a1", "parts": [{"text": "hello"}]}),
            false,
        )
        .unwrap();
    artifacts
        .apply(
            &json!({"artifactId": "a1", "parts": [{"text": " world"}]}),
            true,
        )
        .unwrap();
    artifacts
        .apply(
            &json!({"artifactId": "a2", "parts": [{"text": "second"}]}),
            false,
        )
        .unwrap();
    assert_eq!(artifacts.render(), "hello world\n\nsecond");

    let mut unknown_append = ArtifactAccumulator::default();
    assert_eq!(
        unknown_append.apply(&json!({"artifactId": "a1", "parts": [{"text": "x"}]}), true),
        Err(request_failed())
    );
    assert_eq!(
        artifacts.apply(&json!({"parts": [{"text": "x"}]}), false),
        Err(request_failed())
    );
}

#[test]
fn non_text_parts_are_rejected() {
    assert_eq!(
        text_from_parts(&[json!({"raw": "aGVsbG8="})]),
        Err(request_failed())
    );
}

#[test]
fn direct_message_completes_even_when_the_message_carries_a_task_id() {
    let accumulator = RunAccumulator {
        task_id: Some("t1".to_owned()),
        direct_message_received: true,
        output: "complete answer".to_owned(),
        ..RunAccumulator::default()
    };
    assert_eq!(
        accumulator.completion_plan(),
        CompletionPlan::DirectMessage,
        "A2A 1.0 允许 SendMessage 直接返回 Message；其 taskId 只是关联标识，不能据此再轮询"
    );
}

#[test]
fn non_terminal_task_requires_polling() {
    let accumulator = RunAccumulator {
        task_id: Some("t1".to_owned()),
        state: Some(TASK_STATE_WORKING),
        ..RunAccumulator::default()
    };
    assert_eq!(accumulator.completion_plan(), CompletionPlan::PollTask);

    let terminal = RunAccumulator {
        state: Some(TASK_STATE_COMPLETED),
        ..RunAccumulator::default()
    };
    assert_eq!(terminal.completion_plan(), CompletionPlan::Complete);

    assert_eq!(
        RunAccumulator::default().completion_plan(),
        CompletionPlan::Incomplete
    );
}

#[test]
fn final_text_prefers_status_message_then_history_then_artifacts() {
    let mut accumulator = RunAccumulator {
        latest_status: Some(json!({
            "state": "TASK_STATE_COMPLETED",
            "message": {"role": "ROLE_AGENT", "parts": [{"text": "status answer"}]}
        })),
        latest_task: Some(json!({
            "id": "t1",
            "status": {"state": "TASK_STATE_COMPLETED"},
            "history": [{"role": "ROLE_AGENT", "parts": [{"text": "history answer"}]}],
            "artifacts": [{"artifactId": "a1", "parts": [{"text": "artifact answer"}]}]
        })),
        ..RunAccumulator::default()
    };
    assert_eq!(final_task_text(&accumulator).unwrap(), "status answer");

    accumulator.latest_status = Some(json!({"state": "TASK_STATE_COMPLETED"}));
    assert_eq!(final_task_text(&accumulator).unwrap(), "history answer");

    accumulator.latest_task = Some(json!({
        "id": "t1",
        "status": {"state": "TASK_STATE_COMPLETED"},
        "history": [],
        "artifacts": [
            {"artifactId": "a1", "parts": [{"text": "first"}]},
            {"artifactId": "a2", "parts": [{"text": "second"}]}
        ]
    }));
    assert_eq!(final_task_text(&accumulator).unwrap(), "first\n\nsecond");
}

#[test]
fn streamed_artifacts_are_used_when_the_task_has_no_final_text() {
    let mut accumulator = RunAccumulator::default();
    accumulator
        .artifacts
        .apply(
            &json!({"artifactId": "a1", "parts": [{"text": "streamed"}]}),
            false,
        )
        .unwrap();
    assert_eq!(final_task_text(&accumulator).unwrap(), "streamed");
}

#[test]
fn task_states_accept_names_and_numbers() {
    assert_eq!(
        task_state_name(Some(&json!("TASK_STATE_COMPLETED"))).unwrap(),
        TASK_STATE_COMPLETED
    );
    assert_eq!(
        task_state_name(Some(&json!(5))).unwrap(),
        TASK_STATE_CANCELED
    );
    assert_eq!(task_state_name(Some(&json!(0))), Err(request_failed()));
    assert_eq!(task_state_name(Some(&json!(99))), Err(request_failed()));
    assert_eq!(
        task_state_name(Some(&json!("TASK_STATE_BOGUS"))),
        Err(request_failed())
    );
    assert_eq!(task_state_name(None), Err(request_failed()));
}

#[test]
fn terminal_failure_codes_are_stable() {
    assert_eq!(
        terminal_failure_code(TASK_STATE_INPUT_REQUIRED),
        "a2a_input_required"
    );
    assert_eq!(
        terminal_failure_code(TASK_STATE_AUTH_REQUIRED),
        "a2a_auth_required"
    );
    assert_eq!(terminal_failure_code(TASK_STATE_CANCELED), "a2a_canceled");
    assert_eq!(terminal_failure_code(TASK_STATE_REJECTED), "a2a_rejected");
    assert_eq!(terminal_failure_code(TASK_STATE_FAILED), "a2a_failed");
    for state in [
        TASK_STATE_COMPLETED,
        TASK_STATE_FAILED,
        TASK_STATE_CANCELED,
        TASK_STATE_REJECTED,
    ] {
        assert!(TASK_STATES_WITHOUT_CONTINUATION.contains(&state));
    }
}

#[test]
fn rpc_envelopes_require_a_result_and_reject_errors() {
    assert_eq!(
        rpc_result(&json!({"jsonrpc": "2.0", "result": {"task": {"id": "t1"}}})),
        Ok(json!({"task": {"id": "t1"}}))
    );
    assert_eq!(
        rpc_result(&json!({"jsonrpc": "2.0", "error": {"code": -32603, "message": "boom"}})),
        Err(request_failed())
    );
    assert_eq!(
        rpc_result(&json!({"jsonrpc": "2.0"})),
        Err(request_failed())
    );
    assert_eq!(rpc_result(&json!([1, 2])), Err(request_failed()));
}

#[test]
fn sse_frames_are_extracted_with_multiline_data() {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(b": comment\nevent: message\ndata: {\"a\": 1}\n\n");
    assert_eq!(
        take_data_frame(&mut buffer).unwrap(),
        Some("{\"a\": 1}".to_owned())
    );
    assert_eq!(take_data_frame(&mut buffer).unwrap(), None);

    let mut crlf = Vec::new();
    crlf.extend_from_slice(b"data: first\r\ndata: second\r\n\r\n");
    assert_eq!(
        take_data_frame(&mut crlf).unwrap(),
        Some("first\nsecond".to_owned())
    );
}

#[test]
fn partial_sse_frames_wait_for_more_bytes() {
    let mut buffer = b"data: {\"task\":".to_vec();
    assert_eq!(take_data_frame(&mut buffer).unwrap(), None);
    buffer.extend_from_slice(b"{\"id\": \"t1\"}}\n\n");
    assert_eq!(
        take_data_frame(&mut buffer).unwrap(),
        Some("{\"task\":{\"id\": \"t1\"}}".to_owned())
    );
}
