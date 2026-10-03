use crate::adapter::KernelAdapter;
use crate::protocol::*;
use serde_json::json;
use tracing::{error, info};

pub struct McpServer {
    adapter: KernelAdapter,
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl McpServer {
    pub fn new() -> Self {
        Self {
            adapter: KernelAdapter::new(),
        }
    }

    #[allow(dead_code)]
    pub fn with_adapter(adapter: KernelAdapter) -> Self {
        Self { adapter }
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
                info!("Client initialized");
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

        // Do not respond to notifications (requests without an ID per JSON-RPC 2.0 / MCP spec)
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
                name: "axmg-memory-mcp".to_string(),
                version: "0.1.0".to_string(),
            },
        };

        Ok(serde_json::to_value(result).unwrap())
    }

    fn handle_list_tools(&self, _req: JsonRpcRequest) -> Result<serde_json::Value, JsonRpcError> {
        let result = ListToolsResult {
            tools: vec![
                ToolInfo {
                    name: "remember".to_string(),
                    description: "Save a fact, event, or context text into memory with online Surprise calculation.".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "text": { "type": "string", "description": "The text or event data to remember" },
                            "context_tag": { "type": "string", "description": "Optional source tag (e.g. 'user_prompt', 'git_commit', 'docs')" }
                        },
                        "required": ["text"]
                    }),
                },
                ToolInfo {
                    name: "recall".to_string(),
                    description: "Recall associative memories based on a query string.".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "The search query or topic" },
                            "limit": { "type": "integer", "description": "Max results to return (default: 5, range: 1..100)" },
                            "frequency_floor": { "type": "integer", "description": "Minimum pair frequency to consider (default: 1 for chat mode)" },
                            "ppmi_threshold": { "type": "number", "description": "Minimum PPMI score to consider valid (default: 0.0 for chat mode)" }
                        },
                        "required": ["query"]
                    }),
                },
                ToolInfo {
                    name: "check_surprise".to_string(),
                    description: "Check how surprising a hypothesis or candidate text is without saving it into memory.".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "candidate_text": { "type": "string", "description": "Text or hypothesis to evaluate" },
                            "hypothesis": { "type": "string", "description": "Alias for candidate_text" }
                        },
                        "required": ["candidate_text"]
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
            "remember" => {
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
                let context_tag = params
                    .arguments
                    .get("context_tag")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty());
                Ok(self.adapter.remember(text, context_tag).await)
            }
            "recall" => {
                let query = match params.arguments.get("query").and_then(|v| v.as_str()) {
                    Some(q) if !q.trim().is_empty() => q,
                    _ => {
                        return Ok(serde_json::to_value(CallToolResult {
                            content: vec![ToolContent::Text {
                                text: "Error: 'query' parameter is required and cannot be empty.".to_string(),
                            }],
                            is_error: true,
                        }).unwrap());
                    }
                };
                let raw_limit = params.arguments.get("limit").and_then(|v| v.as_u64()).unwrap_or(5);
                let limit = (raw_limit as usize).clamp(1, 100);

                let frequency_floor = params.arguments.get("frequency_floor").and_then(|v| v.as_u64()).unwrap_or(1);
                let ppmi_threshold = params.arguments.get("ppmi_threshold").and_then(|v| v.as_f64()).unwrap_or(0.0);

                Ok(self.adapter.recall(query, limit, frequency_floor, ppmi_threshold).await)
            }
            "check_surprise" => {
                let text = match params
                    .arguments
                    .get("candidate_text")
                    .or_else(|| params.arguments.get("hypothesis"))
                    .and_then(|v| v.as_str())
                {
                    Some(t) if !t.trim().is_empty() => t,
                    _ => {
                        return Ok(serde_json::to_value(CallToolResult {
                            content: vec![ToolContent::Text {
                                text: "Error: 'candidate_text' or 'hypothesis' parameter is required and cannot be empty.".to_string(),
                            }],
                            is_error: true,
                        }).unwrap());
                    }
                };
                Ok(self.adapter.check_surprise(text).await)
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
                    uri: "memory://focused".to_string(),
                    name: "Focused Memory Concepts".to_string(),
                    description: "Current snapshot of active concepts and associations in memory focus.".to_string(),
                    mime_type: "application/json".to_string(),
                },
                ResourceInfo {
                    uri: "memory://stats".to_string(),
                    name: "Memory Engine Stats".to_string(),
                    description: "Overall statistics: token count, revolutions, compression ratio, active focus ratio.".to_string(),
                    mime_type: "application/json".to_string(),
                },
                ResourceInfo {
                    uri: "memory://timeline".to_string(),
                    name: "Memory Timeline".to_string(),
                    description: "Chronological timeline of recent absorbed events with surprise scores and tags.".to_string(),
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
            "memory://focused" => self.adapter.read_focused().await,
            "memory://stats" => self.adapter.read_stats().await,
            "memory://timeline" => self.adapter.read_timeline().await,
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

    fn create_test_server() -> McpServer {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut db_path = std::env::temp_dir();
        db_path.push(format!("axmg_test_server_{}.redb", ts));
        let mut tl_path = std::env::temp_dir();
        tl_path.push(format!("axmg_test_server_{}.jsonl", ts));

        let adapter = KernelAdapter::with_paths(db_path, tl_path);
        McpServer::with_adapter(adapter)
    }

    #[tokio::test]
    async fn test_tools_list() {
        let server = create_test_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/list".to_string(),
            params: None,
        };
        let resp = server.handle_request(req).await.expect("Expected response");
        assert!(resp.error.is_none());
        let res = resp.result.unwrap();
        let tools = res.get("tools").and_then(|v| v.as_array()).unwrap();
        assert_eq!(tools.len(), 3);
        let names: Vec<_> = tools.iter().filter_map(|t| t.get("name").and_then(|n| n.as_str())).collect();
        assert!(names.contains(&"remember"));
        assert!(names.contains(&"recall"));
        assert!(names.contains(&"check_surprise"));
    }

    #[tokio::test]
    async fn test_remember_validation() {
        let server = create_test_server();

        // 1. Missing text parameter
        let req_missing = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "remember",
                "arguments": {}
            })),
        };
        let resp_missing = server.handle_request(req_missing).await.unwrap();
        let res = resp_missing.result.unwrap();
        let is_error = res.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
        assert!(is_error);

        // 2. Empty/whitespace text parameter
        let req_empty = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "remember",
                "arguments": { "text": "   " }
            })),
        };
        let resp_empty = server.handle_request(req_empty).await.unwrap();
        let res = resp_empty.result.unwrap();
        let is_error = res.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
        assert!(is_error);

        // 3. Valid text
        let req_valid = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "remember",
                "arguments": { "text": "Systems programming in Rust with Merkle DAG" }
            })),
        };
        let resp_valid = server.handle_request(req_valid).await.unwrap();
        assert!(resp_valid.error.is_none());
        let res = resp_valid.result.unwrap();
        assert_eq!(res.get("isError").and_then(|v| v.as_bool()), None);
        let content_text = res["content"][0]["text"].as_str().unwrap();
        assert!(content_text.contains("STORED"));
    }

    #[tokio::test]
    async fn test_recall_validation() {
        let server = create_test_server();

        // Empty query
        let req_empty = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "recall",
                "arguments": { "query": "" }
            })),
        };
        let resp_empty = server.handle_request(req_empty).await.unwrap();
        let res = resp_empty.result.unwrap();
        assert!(res.get("isError").and_then(|v| v.as_bool()).unwrap_or(false));

        // Valid query with limit clamping
        let req_valid = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "recall",
                "arguments": { "query": "Rust", "limit": 200 }
            })),
        };
        let resp_valid = server.handle_request(req_valid).await.unwrap();
        assert!(resp_valid.error.is_none());
        let res = resp_valid.result.unwrap();
        assert_eq!(res.get("isError").and_then(|v| v.as_bool()), None);
    }

    #[tokio::test]
    async fn test_check_surprise_aliases() {
        let server = create_test_server();

        // 1. Using candidate_text
        let req1 = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "check_surprise",
                "arguments": { "candidate_text": "Hypothesis about Merkle trees" }
            })),
        };
        let resp1 = server.handle_request(req1).await.unwrap();
        assert!(resp1.error.is_none());
        let res1 = resp1.result.unwrap();
        assert_eq!(res1.get("isError").and_then(|v| v.as_bool()), None);
        let text1 = res1["content"][0]["text"].as_str().unwrap();
        assert!(text1.contains("surprise_score"));

        // 2. Using hypothesis alias
        let req2 = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "check_surprise",
                "arguments": { "hypothesis": "Hypothesis about Merkle trees" }
            })),
        };
        let resp2 = server.handle_request(req2).await.unwrap();
        assert!(resp2.error.is_none());
        let res2 = resp2.result.unwrap();
        assert_eq!(res2.get("isError").and_then(|v| v.as_bool()), None);

        // 3. Empty argument
        let req3 = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "check_surprise",
                "arguments": {}
            })),
        };
        let resp3 = server.handle_request(req3).await.unwrap();
        let res3 = resp3.result.unwrap();
        assert!(res3.get("isError").and_then(|v| v.as_bool()).unwrap_or(false));
    }

    #[tokio::test]
    async fn test_unknown_tool() {
        let server = create_test_server();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(99)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "non_existent_tool",
                "arguments": {}
            })),
        };
        let resp = server.handle_request(req).await.unwrap();
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32601);
    }
}
