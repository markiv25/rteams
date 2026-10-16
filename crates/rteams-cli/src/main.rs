#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    // `rteams` / `rteams serve`: MCP over stdio. Logs must never go to stdout.
    rteams_server::Server::new().serve_stdio().await
}
