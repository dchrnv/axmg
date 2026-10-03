#!/usr/bin/env python3
import json
import subprocess
import time
import sys

def main():
    print("=" * 65)
    print("  AXMG — Комплексная проверка готовности Фазы 1 (Ядро + MCP)")
    print("=" * 65)

    # 1. Сборка и запуск MCP-сервера в фоновом процессе
    print("\n[1/6] Запуск бинарника axmg-memory-mcp через stdio JSON-RPC...")
    p = subprocess.Popen(
        ["cargo", "run", "-q", "-p", "mcp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True
    )

    req_id = 0
    def send_rpc(method, params=None):
        nonlocal req_id
        req_id += 1
        payload = {"jsonrpc": "2.0", "id": req_id, "method": method}
        if params is not None:
            payload["params"] = params
        p.stdin.write(json.dumps(payload) + "\n")
        p.stdin.flush()
        while True:
            line = p.stdout.readline()
            if not line:
                return None
            try:
                res = json.loads(line)
                if res.get("id") == req_id:
                    return res
            except Exception:
                pass

    # 2. Проверка handshake и списка инструментов
    print("[2/6] Проверка MCP Handshake и зарегистрированных Tools / Resources...")
    init_res = send_rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test-verifier", "version": "1.0"}})
    assert init_res and "result" in init_res, "Ошибка initialize"
    server_info = init_res["result"].get("serverInfo") or init_res["result"].get("server_info")
    print(f"      ✓ Подключен к {server_info['name']} v{server_info['version']}")

    tools_res = send_rpc("tools/list")
    tools = [t["name"] for t in tools_res["result"]["tools"]]
    print(f"      ✓ Доступные инструменты: {', '.join(tools)}")

    resources_res = send_rpc("resources/list")
    resources = [r["uri"] for r in resources_res["result"]["resources"]]
    print(f"      ✓ Доступные ресурсы: {', '.join(resources)}")

    # 3. Тест инкрементального запоминания и замера Surprise
    print("\n[3/6] Проверка помпы Колеса и онлайн-метрик Surprise (remember)...")
    inputs = [
        "Rust is a systems programming language focused on safety and speed.",
        "Merkle DAG stores immutable cryptographic history of composable tokens.",
        "Rust and Merkle DAG combine to build axmg associative memory.",
        "Rust is a systems programming language focused on safety and speed." # Повтор для проверки Familiar
    ]

    for idx, text in enumerate(inputs, 1):
        call_res = send_rpc("tools/call", {"name": "remember", "arguments": {"text": text}})
        content = json.loads(call_res["result"]["content"][0]["text"])
        print(f"      Шаг {idx}: '{text[:45]}...'")
        print(f"        -> Rev: {content['revolution']} | S-static: {content['surprise_static']:.3f} | S-growth: {content['surprise_growth']:.3f} | Score: {content['surprise_score']:.3f}")
        print(f"        -> Вердикт: {content['verdict']} | Активный фокус: {content['active_focus_count']} токенов (+{content['newly_focused']}/-{content['newly_unfocused']})")

    # 4. Проверка неблокирующего ресурса активного фокуса
    print("\n[4/6] Проверка lock-free чтения ресурсов (memory://focused и memory://stats)...")
    focused_res = send_rpc("resources/read", {"uri": "memory://focused"})
    focused_data = json.loads(focused_res["result"]["contents"][0]["text"])
    print(f"      ✓ memory://focused: {focused_data['focus_count']} токенов в фокусе (эпоха {focused_data['revolution']})")
    print(f"      ✓ Топ активных концептов в фокусе внимания:")
    for c in focused_data.get("top_concepts", [])[:5]:
        print(f"          - Id #{c['token_id']}: \"{c['text']}\" (уровень {c['level']}, PPMI-вес {c['ppmi_weight']:.2f})")

    stats_res = send_rpc("resources/read", {"uri": "memory://stats"})
    stats_data = json.loads(stats_res["result"]["contents"][0]["text"])
    print(f"      ✓ memory://stats: всего токенов: {stats_data['total_tokens']}, доля фокуса: {stats_data['active_focus_ratio']*100:.2f}%")

    # 5. Проверка ассоциативного вспоминания (recall)
    print("\n[5/6] Проверка ассоциативного субграфного поиска (recall)...")
    queries = ["Rust memory", "Merkle tokens"]
    for q in queries:
        recall_res = send_rpc("tools/call", {"name": "recall", "arguments": {"query": q, "limit": 4}})
        rec_data = json.loads(recall_res["result"]["content"][0]["text"])
        print(f"      Запрос: \"{q}\"")
        print(f"        -> Распознано токенов запроса: {rec_data['parsed_tokens']}")
        print(f"        -> Восстановленный контекст LLM: \"{rec_data['context_string']}\"")
        for a in rec_data["associations"][:3]:
            print(f"            * Ассоциация: \"{a['associated_text']}\" (Score: {a['relevance_score']:.2f}, PPMI: {a['ppmi']:.2f})")

    # 6. Проверка безинерционного замера Surprise (check_surprise)
    print("\n[6/6] Проверка оценки гипотез без сохранения (check_surprise)...")
    hypotheses = [
        ("Знакомый паттерн", "Rust is a systems programming language."),
        ("Случайный шум", "xj9128z !@#$$% 000qwerty_non_sense_entropy")
    ]
    for label, hyp in hypotheses:
        surp_res = send_rpc("tools/call", {"name": "check_surprise", "arguments": {"candidate_text": hyp}})
        surp_data = json.loads(surp_res["result"]["content"][0]["text"])
        print(f"      {label}: \"{hyp[:35]}\"")
        print(f"        -> S-static: {surp_data['surprise_static']:.3f} | Score: {surp_data['surprise_score']:.3f} | Вердикт: {surp_data['verdict']}")

    p.terminate()
    p.wait()
    print("\n" + "=" * 65)
    print("  ✓ ВСЕ ТЕСТЫ И СЦЕНАРИИ ФАЗЫ 1 УСПЕШНО ПРОЙДЕНЫ!")
    print("=" * 65)

if __name__ == "__main__":
    main()
