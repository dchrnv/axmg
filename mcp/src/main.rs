use std::io::{self, BufRead};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use mcp::protocol::JsonRpcRequest;
use mcp::server::McpServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // We must output logs to stderr, because stdout is reserved for MCP JSON-RPC.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mcp=info".parse()?))
        .with_writer(std::io::stderr)
        .init();

    info!("Starting axmg-memory-mcp server");

    let server = McpServer::new();
    let stdin = io::stdin();
    let mut handle = stdin.lock();
    let mut line = String::new();

    loop {
        line.clear();
        match handle.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                let req_line = line.trim();
                if req_line.is_empty() {
                    continue;
                }
                
                match serde_json::from_str::<JsonRpcRequest>(req_line) {
                    Ok(req) => {
                        if let Some(resp) = server.handle_request(req).await {
                            let mut resp_str = serde_json::to_string(&resp)?;
                            resp_str.push('\n');
                            // Write directly to stdout as required by MCP
                            use std::io::Write;
                            io::stdout().write_all(resp_str.as_bytes())?;
                            io::stdout().flush()?;
                        }
                    }
                    Err(e) => {
                        error!("Failed to parse JSON-RPC request: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("Error reading from stdin: {}", e);
                break;
            }
        }
    }

    info!("axmg-memory-mcp server stopped");
    Ok(())
}
