use qmp::ops::{OpsClient, Profile};
use qmp::{Client, Endpoint};

#[tokio::main(flavor = "current_thread")]
async fn main() -> qmp::Result<()> {
    // Adjust the path to your environment.
    let client = Client::connect(Endpoint::unix("/var/run/qemu-server/100.qmp")).await?;

    // Read-only allow-list by default.
    let ops = OpsClient::from_profile(client, Profile::ReadOnly);

    let v = ops.query_version().await?;
    println!(
        "qemu {}.{}.{} ({})",
        v.qemu.major, v.qemu.minor, v.qemu.micro, v.package
    );

    Ok(())
}
