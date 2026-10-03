use mcp::adapter::KernelAdapter;
use mcp::protocol::JsonRpcRequest;
use mcp::sensory::{SensoryCollector, SensoryMcpServer};
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mcp=info".parse()?))
        .with_writer(std::io::stderr)
        .init();

    let db_path = std::env::var("AXMG_SENSORY_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("axmg_sensory_store.redb"));

    let tl_path = std::env::var("AXMG_SENSORY_TIMELINE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("axmg_sensory_timeline.jsonl"));

    let adapter = Arc::new(KernelAdapter::with_paths(db_path, tl_path));

    let args: Vec<String> = std::env::args().collect();
    let is_raw_mode = args.iter().any(|a| a == "--raw" || a == "-r" || a == "--stream");

    // 1. Нетекстовый сырой режим: прямой прием байтового потока без JSON
    if is_raw_mode {
        info!("Starting axmg-sensory-mcp in RAW BYTE STREAM mode (no JSON overhead)");

        let chunk_size = args
            .iter()
            .position(|a| a == "--chunk-size" || a == "-c")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(4096);

        let source_tag = args
            .iter()
            .position(|a| a == "--tag" || a == "-t")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| "telemetry_stream".to_string());

        let config = mcp::sensory::raw_bytes::ByteStreamConfig {
            chunk_size,
            source_tag,
            anomaly_surprise_threshold: 1.0,
        };

        let stdin = tokio::io::stdin();
        let report = mcp::sensory::raw_bytes::stream_from_async_read(stdin, adapter, config).await?;
        eprintln!(
            "[AXMG RAW STREAM] Finished. Chunks: {}, Total bytes: {}, Anomalies: {}, Revolutions: {}",
            report.total_chunks, report.total_bytes, report.anomalies_detected, report.last_revolution
        );
        return Ok(());
    }

    // 2. Стандартный режим Model Context Protocol (JSON-RPC stdio)
    info!("Starting axmg-sensory-mcp server (MCP JSON-RPC)");
    let collector = Arc::new(SensoryCollector::new(adapter));
    let server = SensoryMcpServer::new(collector);

    let stdin = io::stdin();
    let mut handle = stdin.lock();
    let mut line = String::new();

    loop {
        line.clear();
        match handle.read_line(&mut line) {
            Ok(0) => break,
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

    info!("axmg-sensory-mcp server stopped");
    Ok(())
}
