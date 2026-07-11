#[tokio::main]
async fn main() -> std::io::Result<()> {
    mobailmux::run().await
}
