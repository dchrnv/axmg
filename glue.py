import subprocess
import json
import urllib.request
import os
import shutil
from pathlib import Path
from typing import Any


PROJECT_ROOT = Path(__file__).resolve().parent

def main():
    print("🚀 Стартуем Ядро Памяти (MCP)...")

    binary = PROJECT_ROOT / "target" / "debug" / "mcp"
    if binary.exists():
        command = [str(binary)]
    else:
        cargo = shutil.which("cargo") or os.path.expanduser("~/.cargo/bin/cargo")
        command = [cargo, "run", "-p", "mcp"]

    mcp_process = subprocess.Popen(
        command,
        shell=False,
        stdin=subprocess.PIPE, 
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        cwd=PROJECT_ROOT,
    )
    assert mcp_process.stdin is not None
    assert mcp_process.stdout is not None
    mcp_stdin = mcp_process.stdin
    mcp_stdout = mcp_process.stdout

    req_id = 1

    def call_mcp(method: str, args: dict[str, Any]) -> dict[str, Any]:
        nonlocal req_id
        req: dict[str, Any] = {
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tools/call",
            "params": {
                "name": method,
                "arguments": args
            }
        }
        req_id += 1
        try:
            mcp_stdin.write(json.dumps(req) + "\n")
            mcp_stdin.flush()
        except Exception:
            return {}
        
        # Читаем ответ. Поскольку сервер может спамить логами или мы можем читать мусор, 
        # парсим построчно пока не найдем наш ID.
        while True:
            line = mcp_stdout.readline()
            if not line: return {}
            try:
                resp = json.loads(line)
                if resp.get("id") == req_id - 1:
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
            "Используй эти ассоциации, если они полезны, чтобы показать, что ты помнишь контекст. Отвечай кратко на русском."
        )
        payload: dict[str, Any] = {
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
        except urllib.error.URLError as e:
            return f"[Ошибка связи с LM Studio: {e}]"
        except Exception as e:
            return f"[Ошибка: {e}]"

    def ingest_project():
        print("🔄 Сканирование и загрузка исходного кода проекта в Ядро...")
        root_dir = Path(__file__).parent
        allowed_exts = {".rs", ".py", ".md", ".toml"}
        ignored_dirs = {".git", "target", "__pycache__", "venv", "env"}
        
        files_to_read = []
        for path in root_dir.rglob("*"):
            if path.is_file() and path.suffix in allowed_exts:
                if not any(ignored in path.parts for ignored in ignored_dirs):
                    files_to_read.append(path)
                    
        print(f"📦 Найдено {len(files_to_read)} файлов для загрузки. Интегрируем...")
        for i, file_path in enumerate(files_to_read, 1):
            try:
                content = file_path.read_text(encoding="utf-8")
                # Send file name as context tag to help with associations
                call_mcp("remember", {"text": f"Файл {file_path.name}:\n{content}", "context_tag": file_path.name})
                if i % 10 == 0:
                    print(f"  ... загружено {i}/{len(files_to_read)} файлов")
            except Exception as e:
                print(f"⚠️ Ошибка чтения {file_path.name}: {e}")
                
        print("✅ База знаний Ядра актуализирована!")

    ingest_project()
    print("✅ Система готова! Напишите что-нибудь (или 'exit' для выхода).")
    
    try:
        while True:
            try:
                user_input = input("\nВы: ")
                if user_input.strip().lower() in ["exit", "quit", "выход"]:
                    break
                if not user_input.strip():
                    continue

                recall_res = call_mcp("recall", {"query": user_input, "limit": 5})
                associations = recall_res.get("associations", [])
                texts: list[str] = []
                for association in associations:
                    text = str(association.get("associated_text", "")).strip()
                    if len(text) > 2 and text not in texts:
                        texts.append(text)
                memory_context = ", ".join(texts)
                if memory_context:
                    print(f"🧠 Ядро вспомнило: {memory_context}")
                else:
                    print("🧠 Ядро пока ничего не ассоциирует.")

                print("🤖 Ollama думает...")
                response = ollama_chat(user_input, memory_context)
                print(f"Ollama: {response}")

                call_mcp("remember", {"text": user_input})
                call_mcp("remember", {"text": response})
            except KeyboardInterrupt:
                break
            except Exception as error:
                print(f"Ошибка: {error}")
                break
    finally:
        mcp_process.terminate()
        mcp_process.wait(timeout=5)
        print("Выход.")

if __name__ == "__main__":
    main()
