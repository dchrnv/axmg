import json
import subprocess
import time

mcp_process = subprocess.Popen(
    ["cargo", "run", "-p", "mcp"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    text=True
)

def call(method, args):
    req = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": method, "arguments": args}}
    mcp_process.stdin.write(json.dumps(req) + "\n")
    mcp_process.stdin.flush()
    while True:
        line = mcp_process.stdout.readline()
        if not line: break
        try:
            resp = json.loads(line)
            if "result" in resp:
                content = resp["result"]["content"][0]["text"]
                return json.loads(content)
        except: pass
    return {}

call("remember", {"text": "axmg is a test project."})
call("remember", {"text": "axmg uses MCP for context."})
res = call("recall", {"query": "axmg MCP", "limit": 5})
print(json.dumps(res, indent=2))

mcp_process.terminate()
