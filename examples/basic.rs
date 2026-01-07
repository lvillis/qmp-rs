use qmp::{Client, Endpoint};

#[tokio::main(flavor = "current_thread")]
async fn main() -> qmp::Result<()> {
    // Adjust the path to your environment.
    let endpoint = Endpoint::unix("/var/run/qemu-server/100.qmp");

    let client = Client::connect(endpoint).await?;

    let status: serde_json::Value = client.execute("query-status", Option::<()>::None).await?;
    println!("status = {status}");

    Ok(())
}
