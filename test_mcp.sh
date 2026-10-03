#!/bin/bash
# test_mcp.sh
source $HOME/.cargo/env

# Start the MCP server and feed it some commands
(
echo '{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "remember", "arguments": {"text": "В недрах тундры выдры в гетрах тырят в ведра ядра кедров"}}}'
sleep 1
echo '{"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "remember", "arguments": {"text": "Выдры очень любят кедровые орехи и часто их тырят"}}}'
sleep 1
echo '{"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "check_surprise", "arguments": {"candidate_text": "выдры тырят кедры"}}}'
sleep 1
echo '{"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "recall", "arguments": {"query": "выдры", "limit": 5}}}'
sleep 1
) | cargo run -p mcp 2>/dev/null
