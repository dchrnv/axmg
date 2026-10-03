use crate::protocol::*;
use crate::sensory::collector::SensoryCollector;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tracing::{error, info};

pub struct SensoryMcpServer {
    collector: Arc<SensoryCollector>,
}

impl SensoryMcpServer {
    pub fn new(collector: Arc<SensoryCollector>) -> Self {
        Self { collector }
    }

    pub async fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = req.id.clone().unwrap_or(serde_json::Value::Null);
        let method = req.method.clone();
        let has_id = req.id.is_some();

        let result = match method.as_str() {
            "initialize" => self.handle_initialize(req),
            "ping" => Ok(json!({})),
            "tools/list" => self.handle_list_tools(req),
            "tools/call" => self.handle_call_tool(req).await,
            "resources/list" => self.handle_list_resources(req),
            "resources/read" => self.handle_read_resource(req).await,
            "notifications/initialized" => {
                info!("Client initialized for axmg-sensory-mcp");
                return None;
            }
            _ => {
                error!("Method not found: {}", method);
                Err(JsonRpcError {
                    code: -32601,
                    message: "Method not found".to_string(),
                })
            }
        };

        let response = match result {
            Ok(res) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(res),
                error: None,
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(err),
            },
        };

        if !has_id {
            return None;
        }

        Some(response)
    }

    fn handle_initialize(&self, req: JsonRpcRequest) -> Result<serde_json::Value, JsonRpcError> {
        let _params: InitializeParams = serde_json::from_value(req.params.unwrap_or(json!({})))
            .map_err(|e| JsonRpcError {
                code: -32602,
                message: format!("Invalid params: {}", e),
            })?;

        let result = InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: json!({
                "tools": {},
                "resources": {}
            }),
            server_info: ServerInfo {
                name: "axmg-sensory-mcp".to_string(),
                version: "0.1.0".to_string(),
            },
        };

        Ok(serde_json::to_value(result).unwrap())
    }

    fn handle_list_tools(&self, _req: JsonRpcRequest) -> Result<serde_json::Value, JsonRpcError> {
        let result = ListToolsResult {
            tools: vec![
                ToolInfo {
                    name: "ingest_terminal_log".to_string(),
                    description: "Stream terminal output or system logs into axmg memory with automatic ANSI stripping.".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "log_text": { "type": "string", "description": "Raw terminal output or log text" },
                            "strip_ansi": { "type": "boolean", "description": "Whether to strip ANSI escapes (default: true)" },
                            "source_tag": { "type": "string", "description": "Optional tag for the log source (e.g. 'cargo_build', 'daemon')" }
                        },
                        "required": ["log_text"]
                    }),
                },
                ToolInfo {
                    name: "ingest_git_events".to_string(),
                    description: "Stream recent Git commits and working tree diffs into axmg memory.".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "repo_path": { "type": "string", "description": "Path to Git repository (default: '.')" },
                            "max_commits": { "type": "integer", "description": "Max recent commits to ingest (default: 5, range: 1..50)" },
                            "include_diff": { "type": "boolean", "description": "Whether to ingest working tree status and diff (default: true)" }
                        }
                    }),
                },
                ToolInfo {
                    name: "ingest_file_change".to_string(),
                    description: "Stream file modification or diff content into axmg memory.".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "file_path": { "type": "string", "description": "Path to modified file" },
                            "content": { "type": "string", "description": "Optional content or diff (reads file from disk if omitted)" },
                            "change_type": { "type": "string", "description": "Change type (e.g. 'modified', 'created', 'diff', default: 'modified')" }
                        },
                        "required": ["file_path"]
                    }),
                },
                ToolInfo {
                    name: "ingest_raw_bytes".to_string(),
                    description: "Stream raw binary or telemetry payload (hex, base64, or raw) into axmg memory without JSON overhead.".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "data": { "type": "string", "description": "Byte payload (hex, base64, or utf8 string)" },
                            "format": { "type": "string", "description": "Payload format: 'auto', 'hex', 'base64', 'utf8' (default: 'auto')" },
                            "source_tag": { "type": "string", "description": "Optional tag (e.g. 'can_bus', 'imu', 'telemetry', default: 'telemetry')" }
                        },
                        "required": ["data"]
                    }),
                },
                ToolInfo {
                    name: "check_sensory_surprise".to_string(),
                    description: "Check how surprising a sensory event is without saving it into memory.".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "text": { "type": "string", "description": "Sensory text to evaluate" }
                        },
                        "required": ["text"]
                    }),
                },
            ],
        };

        Ok(serde_json::to_value(result).unwrap())
    }

    async fn handle_call_tool(&self, req: JsonRpcRequest) -> Result<serde_json::Value, JsonRpcError> {
        let params: CallToolParams = serde_json::from_value(req.params.unwrap_or(json!({})))
            .map_err(|e| JsonRpcError {
                code: -32602,
                message: format!("Invalid params: {}", e),
            })?;

        let tool_outcome: Result<serde_json::Value, String> = match params.name.as_str() {
            "ingest_terminal_log" => {
                let log_text = match params.arguments.get("log_text").and_then(|v| v.as_str()) {
                    Some(t) if !t.trim().is_empty() => t,
                    _ => {
                        return Ok(serde_json::to_value(CallToolResult {
                            content: vec![ToolContent::Text {
                                text: "Error: 'log_text' parameter is required and cannot be empty.".to_string(),
                            }],
                            is_error: true,
                        }).unwrap());
                    }
                };
                let strip_ansi = params.arguments.get("strip_ansi").and_then(|v| v.as_bool()).unwrap_or(true);
                let source_tag = params.arguments.get("source_tag").and_then(|v| v.as_str());

                self.collector.ingest_terminal(log_text, strip_ansi, source_tag).await
            }
            "ingest_git_events" => {
                let repo_path_str = params.arguments.get("repo_path").and_then(|v| v.as_str()).unwrap_or(".");
                let repo_path = Path::new(repo_path_str);
                let max_commits = params.arguments.get("max_commits").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
                let include_diff = params.arguments.get("include_diff").and_then(|v| v.as_bool()).unwrap_or(true);

                self.collector.ingest_git(repo_path, max_commits, include_diff).await
            }
            "ingest_file_change" => {
                let file_path = match params.arguments.get("file_path").and_then(|v| v.as_str()) {
                    Some(p) if !p.trim().is_empty() => p,
                    _ => {
                        return Ok(serde_json::to_value(CallToolResult {
                            content: vec![ToolContent::Text {
                                text: "Error: 'file_path' parameter is required and cannot be empty.".to_string(),
                            }],
                            is_error: true,
                        }).unwrap());
                    }
                };
                let content = params.arguments.get("content").and_then(|v| v.as_str());
                let change_type = params.arguments.get("change_type").and_then(|v| v.as_str()).unwrap_or("modified");

                self.collector.ingest_file(file_path, content, change_type).await
            }
            "ingest_raw_bytes" => {
                let data = match params.arguments.get("data").and_then(|v| v.as_str()) {
                    Some(d) if !d.trim().is_empty() => d,
                    _ => {
                        return Ok(serde_json::to_value(CallToolResult {
                            content: vec![ToolContent::Text {
                                text: "Error: 'data' parameter is required and cannot be empty.".to_string(),
                            }],
                            is_error: true,
                        }).unwrap());
                    }
                };
                let format_hint = params.arguments.get("format").and_then(|v| v.as_str()).unwrap_or("auto");
                let source_tag = params.arguments.get("source_tag").and_then(|v| v.as_str());

                self.collector.ingest_raw_bytes(data, format_hint, source_tag).await
            }
            "check_sensory_surprise" => {
                let text = match params.arguments.get("text").and_then(|v| v.as_str()) {
                    Some(t) if !t.trim().is_empty() => t,
                    _ => {
                        return Ok(serde_json::to_value(CallToolResult {
                            content: vec![ToolContent::Text {
                                text: "Error: 'text' parameter is required and cannot be empty.".to_string(),
                            }],
                            is_error: true,
                        }).unwrap());
                    }
                };
                Ok(self.collector.check_surprise(text).await)
            }
            _ => {
                return Err(JsonRpcError {
                    code: -32601,
                    message: format!("Tool not found: {}", params.name),
                });
            }
        };

        let tool_res = match tool_outcome {
            Ok(result) => CallToolResult {
                content: vec![ToolContent::Text {
                    text: serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string()),
                }],
                is_error: false,
            },
            Err(err) => CallToolResult {
                content: vec![ToolContent::Text {
                    text: format!("Error: {}", err),
                }],
                is_error: true,
            },
        };

        Ok(serde_json::to_value(tool_res).unwrap())
    }

    fn handle_list_resources(&self, _req: JsonRpcRequest) -> Result<serde_json::Value, JsonRpcError> {
        let result = ListResourcesResult {
            resources: vec![
                ResourceInfo {
                    uri: "sensory://stats".to_string(),
                    name: "Sensory Streamer Statistics".to_string(),
                    description: "Counters of ingested terminal logs, git events, file diffs, and total streamed bytes.".to_string(),
                    mime_type: "application/json".to_string(),
                },
                ResourceInfo {
                    uri: "sensory://recent".to_string(),
                    name: "Recent Sensory Events".to_string(),
                    description: "Chronological log of recent sensory stream events with Surprise scores.".to_string(),
                    mime_type: "application/json".to_string(),
                },
            ],
        };

        Ok(serde_json::to_value(result).unwrap())
    }

    async fn handle_read_resource(&self, req: JsonRpcRequest) -> Result<serde_json::Value, JsonRpcError> {
        let params: ReadResourceParams = serde_json::from_value(req.params.unwrap_or(json!({})))
            .map_err(|e| JsonRpcError {
                code: -32602,
                message: format!("Invalid params: {}", e),
            })?;

        let content = match params.uri.as_str() {
            "sensory://stats" => self.collector.get_stats().await,
            "sensory://recent" => self.collector.get_recent_events(50).await,
            _ => {
                return Err(JsonRpcError {
                    code: -32602,
                    message: format!("Resource not found: {}", params.uri),
                });
            }
        };

        let result = ReadResourceResult {
            contents: vec![ResourceContent {
                uri: params.uri,
                mime_type: "application/json".to_string(),
                text: serde_json::to_string_pretty(&content).unwrap_or_else(|_| "{}".to_string()),
            }],
        };

        Ok(serde_json::to_value(result).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::KernelAdapter;

    fn create_test_server() -> SensoryMcpServer {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let mut db_path = std::env::temp_dir();
        db_path.push(format!("axmg_test_sensory_server_{}.redb", ts));
        let mut tl_path = std::env::temp_dir();
        tl_path.push(format!("axmg_test_sensory_server_{}.jsonl", ts));

        let adapter = Arc::new(KernelAdapter::with_paths(db_path, tl_path));
        let collector = Arc::new(SensoryCollector::new(adapter));
        SensoryMcpServer::new(collector)
    }

    #[tokio::test]
    async fn test_sensory_tools_and_resources_list() {
        let server = create_test_server();

        // 1. tools/list
        let req_tools = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/list".to_string(),
            params: None,
        };
        let resp = server.handle_request(req_tools).await.unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 5);
        let names: Vec<_> = tools.iter().filter_map(|t| t.get("name").and_then(|n| n.as_str())).collect();
        assert!(names.contains(&"ingest_raw_bytes"));

        // 2. resources/list
        let req_res = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "resources/list".to_string(),
            params: None,
        };
        let resp_res = server.handle_request(req_res).await.unwrap();
        let resources = resp_res.result.unwrap()["resources"].as_array().unwrap().clone();
        assert_eq!(resources.len(), 2);
    }

    #[tokio::test]
    async fn test_ingest_raw_bytes_tool() {
        let server = create_test_server();

        // 1. Ingest hex payload
        let req_hex = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(20)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "ingest_raw_bytes",
                "arguments": {
                    "data": "0x43414e5f4255535f5041434b4554", // CAN_BUS_PACKET
                    "format": "hex",
                    "source_tag": "can_bus"
                }
            })),
        };
        let resp_hex = server.handle_request(req_hex).await.unwrap();
        assert!(resp_hex.error.is_none());
        let res_hex = resp_hex.result.unwrap();
        assert_eq!(res_hex.get("isError").and_then(|v| v.as_bool()), None);
    }

    #[tokio::test]
    async fn test_ingest_terminal_and_read_stats() {
        let server = create_test_server();

        let req_ingest = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(10)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "ingest_terminal_log",
                "arguments": {
                    "log_text": "\x1b[32m[SERVER]\x1b[0m Listening on http://127.0.0.1:3000\n[REQUEST] GET /api/v1/health"
                }
            })),
        };
        let resp_ingest = server.handle_request(req_ingest).await.unwrap();
        assert!(resp_ingest.error.is_none());
        let res = resp_ingest.result.unwrap();
        assert_eq!(res.get("isError").and_then(|v| v.as_bool()), None);

        // Read sensory://stats
        let req_stats = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(11)),
            method: "resources/read".to_string(),
            params: Some(json!({ "uri": "sensory://stats" })),
        };
        let resp_stats = server.handle_request(req_stats).await.unwrap();
        let text_stats = resp_stats.result.unwrap()["contents"][0]["text"].as_str().unwrap().to_string();
        assert!(text_stats.contains("\"terminal_events\": 1"));
    }
}
