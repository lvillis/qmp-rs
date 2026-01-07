use qmp::Client;
use qmp::mock::{MockScript, MockServer};
use qmp::ops::{OpsClient, Profile};

#[tokio::test]
async fn client_can_execute_and_receive_events() -> qmp::Result<()> {
    let script = MockScript::new()
        .reply_return(
            "query-status",
            serde_json::json!({"running": true, "singlestep": false, "status": "running"}),
        )
        .post_event(serde_json::json!({
            "event": "STOP",
            "data": {"reason": "pause"},
            "timestamp": {"seconds": 0, "microseconds": 0}
        }));

    let server = MockServer::start_tcp(script).await?;
    let client = Client::connect(server.endpoint()).await?;

    let mut events = client.events();
    let status: serde_json::Value = client.execute("query-status", Option::<()>::None).await?;
    assert_eq!(
        status.get("status").and_then(|v| v.as_str()),
        Some("running")
    );

    let ev = events.recv().await?;
    assert_eq!(ev.name, "STOP");

    server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn ops_policy_blocks_disallowed_commands() -> qmp::Result<()> {
    let script = MockScript::new().reply_return(
        "query-version",
        serde_json::json!({"qemu": {"major": 8, "minor": 2, "micro": 0}, "package": "mock"}),
    );

    let server = MockServer::start_tcp(script).await?;
    let client = Client::connect(server.endpoint()).await?;

    let ops = OpsClient::from_profile(client, Profile::ReadOnly);
    let _v = ops.query_version().await?;

    let err = ops
        .call_json::<()>("stop", None, Default::default())
        .await
        .err()
        .expect("must error");
    assert_eq!(err.kind(), qmp::error::ErrorKind::Policy);

    server.shutdown().await;
    Ok(())
}
