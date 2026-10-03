import subprocess
import json
import urllib.request
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parent


def call_mcp(mcp_process, method, args, req_id):
    req = {
        "jsonrpc": "2.0",
        "id": req_id,
        "method": "tools/call",
        "params": {
            "name": method,
            "arguments": args
        }
    }
    mcp_process.stdin.write(json.dumps(req) + "\n")
    mcp_process.stdin.flush()
    
    while True:
        line = mcp_process.stdout.readline()
        if not line: return {}
        try:
            resp = json.loads(line)
            if resp.get("id") == req_id:
                content = resp.get("result", {}).get("content", [])
                if content:
                    text = content[0].get("text", "{}")
                    return json.loads(text)
        except json.JSONDecodeError:
            pass

def ollama_chat(query: str, memory_context: str) -> str:
    url = "http://127.0.0.1:1234/v1/chat/completions"
    
    if memory_context:
        prompt = f"Контекст из памяти:\n{memory_context}\n\nВопрос пользователя: {query}"
    else:
        prompt = query

    system_prompt = (
        "Ты помощник, который использует предоставленный 'Контекст из памяти' для ответов на вопросы. "
        "Если контекст пустой, отвечай как обычно. Отвечай кратко и только по делу."
    )
    payload = {
        "model": "dolphin3-cyber-8b",
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": prompt}
        ],
        "temperature": 0.7,
        "max_tokens": 500,
        "stream": False
    }

    try:
        req = urllib.request.Request(
            url,
            data=json.dumps(payload).encode("utf-8"),
            headers={
                "Content-Type": "application/json",
                "Authorization": "Bearer lm-studio"
            }
        )
        response = urllib.request.urlopen(req)
        result = json.loads(response.read().decode("utf-8"))
        return result.get("choices", [{}])[0].get("message", {}).get("content", "[Пустой ответ]")
    except Exception as e:
        return f"[Ошибка связи с LM Studio: {e}]"

def main():
    report = {
        "ingestion": [],
        "queries": []
    }
    
    binary = PROJECT_ROOT / "target" / "debug" / "mcp"
    mcp_process = subprocess.Popen(
        [str(binary)],
        shell=False,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        cwd=PROJECT_ROOT,
    )

    req_id = 1

    try:
        test_queries = [
            "Как работает WheelState и для чего нужен sliding window на 100000 токенов?",
            "Что делает функция check_surprise?",
            "Как испечь яблочный пирог?"
        ]

        print("\n🧠 Тестируем память...")
        for query in test_queries:
            surp_res = call_mcp(mcp_process, "check_surprise", {"candidate_text": query}, req_id)
            req_id += 1
            recall_res = call_mcp(mcp_process, "recall", {"query": query, "limit": 10, "frequency_floor": 1, "ppmi_threshold": 0.0}, req_id)
            req_id += 1

            associations = recall_res.get("associations", [])
            print(f"DEBUG: raw associations = {associations}")
            texts = []
            for association in associations:
                text = association.get("associated_text", "").strip()
                if text not in texts:
                    texts.append(text)
            memory_context = ", ".join(texts)
            llm_response = ollama_chat(query, memory_context)

            report["queries"].append({
                "query": query,
                "s_growth": surp_res.get("surprise_growth", 0.0),
                "s_static": surp_res.get("surprise_static", 0.0),
                "verdict": surp_res.get("verdict", ""),
                "associations_count": len(texts),
                "memory_context": memory_context,
                "ollama_response": llm_response
            })
            
            print(f"Вопрос: {query}\nПамять: {memory_context}\nОтвет: {llm_response}\n")
    finally:
        mcp_process.terminate()
        mcp_process.wait(timeout=5)

    with (PROJECT_ROOT / "experiment_results.json").open("w", encoding="utf-8") as report_file:
        json.dump(report, report_file, indent=2, ensure_ascii=False)
    print("✅ Отчет сохранен в experiment_results.json")

if __name__ == "__main__":
    main()
