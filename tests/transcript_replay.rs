use qmp::Client;
use qmp::mock::{ReplayServer, Transcript};

#[tokio::test]
async fn transcript_replay_smoke_test() -> qmp::Result<()> {
    // Deterministic transcript (client IDs are 0 for qmp_capabilities and start
    // at 1 for subsequent commands).
    let jsonl = r#"
{"dir":"server","msg":{"QMP":{"version":{"qemu":{"major":8,"minor":2,"micro":0},"package":"mock"},"capabilities":[]}}}
{"dir":"client","msg":{"execute":"qmp_capabilities","id":0}}
{"dir":"server","msg":{"return":{},"id":0}}
{"dir":"client","msg":{"execute":"query-version","id":1}}
{"dir":"server","msg":{"return":{"qemu":{"major":8,"minor":2,"micro":0},"package":"mock"},"id":1}}
"#;

    let transcript = Transcript::from_jsonl_str(jsonl)?;
    let server = ReplayServer::start_tcp(transcript).await?;

    let client = Client::connect(server.endpoint()).await?;

    let v: serde_json::Value = client.execute("query-version", Option::<()>::None).await?;
    assert_eq!(v.get("package").and_then(|v| v.as_str()), Some("mock"));

    server.shutdown().await;
    Ok(())
}
