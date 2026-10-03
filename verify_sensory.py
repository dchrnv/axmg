#!/usr/bin/env python3
import json
import subprocess
import sys
import os

def main():
    print("=" * 65)
    print("  AXMG — Комплексная проверка Сенсорного стримера (Фаза 3)")
    print("=" * 65)

    env = os.environ.copy()
    env["AXMG_SENSORY_DB_PATH"] = "/tmp/axmg_test_sensory_e2e.redb"
    env["AXMG_SENSORY_TIMELINE_PATH"] = "/tmp/axmg_test_sensory_e2e.jsonl"

    # [A] Проверка прямого байтового стриминга БЕЗ JSON (--raw stdin)
    print("\n[1/6] Проверка прямого байтового стриминга БЕЗ JSON (axmg-sensory-mcp --raw)...")
    sample_telemetry = b"".join([f"CAN_FRAME_{i:03d}:ACCEL={i*0.5:.2f};GYRO={i*0.1:.2f};\n".encode("ascii") for i in range(10)])
    p_raw = subprocess.Popen(
        ["cargo", "run", "-q", "-p", "mcp", "--bin", "axmg-sensory-mcp", "--", "--raw", "--chunk-size", "128", "--tag", "can_telemetry"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env
    )
    _, raw_err = p_raw.communicate(input=sample_telemetry)
    assert p_raw.returncode == 0, f"Raw streaming failed: {raw_err.decode()}"
    raw_err_text = raw_err.decode()
    assert "[AXMG RAW STREAM] Finished" in raw_err_text
    print("      ✓ Сырой байтовый поток телеметрии успешно обработан напрямую в Merkle DAG!")

    # [B] Проверка MCP JSON-RPC сервера
    print("\n[2/6] Запуск бинарника axmg-sensory-mcp через stdio JSON-RPC...")
    p = subprocess.Popen(
        ["cargo", "run", "-q", "-p", "mcp", "--bin", "axmg-sensory-mcp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env
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

    print("[3/6] Проверка handshake и регистрации сенсорных инструментов/ресурсов...")
    init_res = send_rpc("initialize", {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {"name": "test-sensory-client", "version": "1.0"}
    })
    server_info = init_res["result"]["serverInfo"]
    print(f"      ✓ Подключен к {server_info['name']} v{server_info['version']}")

    tools_res = send_rpc("tools/list")
    tools = [t["name"] for t in tools_res["result"]["tools"]]
    print(f"      ✓ Доступные сенсорные инструменты: {', '.join(tools)}")
    assert "ingest_terminal_log" in tools
    assert "ingest_git_events" in tools
    assert "ingest_file_change" in tools
    assert "ingest_raw_bytes" in tools

    res_list = send_rpc("resources/list")
    resources = [r["uri"] for r in res_list["result"]["resources"]]
    print(f"      ✓ Доступные ресурсы: {', '.join(resources)}")
    assert "sensory://stats" in resources
    assert "sensory://recent" in resources

    # 4. Ingest Terminal Log & File
    print("\n[4/6] Проверка стриминга консольных логов и файлов...")
    sample_log = "\x1b[1;32m[BUILD SUCCESS]\x1b[0m Compiled shelf in 0.42s\r\n\x1b[34m[INFO]\x1b[0m Running test harness\n"
    res_term = send_rpc("tools/call", {
        "name": "ingest_terminal_log",
        "arguments": {
            "log_text": sample_log,
            "strip_ansi": True,
            "source_tag": "compiler"
        }
    })
    content = json.loads(res_term["result"]["content"][0]["text"])
    print(f"      ✓ Терминальный лог: {content['lines_count']} строк, байт: {content['bytes_ingested']}")

    res_file = send_rpc("tools/call", {
        "name": "ingest_file_change",
        "arguments": {
            "file_path": "src/sensory/raw_bytes.rs",
            "content": "pub struct ByteStreamConfig { pub chunk_size: usize }",
            "change_type": "modified"
        }
    })
    file_content = json.loads(res_file["result"]["content"][0]["text"])
    print(f"      ✓ Файл {file_content['file_path']} зафиксирован, байт: {file_content['bytes_ingested']}")

    # 5. Ingest Raw Bytes via MCP tool (Hex and Base64)
    print("\n[5/6] Проверка универсального байтового адаптера (ingest_raw_bytes)...")
    res_hex = send_rpc("tools/call", {
        "name": "ingest_raw_bytes",
        "arguments": {
            "data": "0x4865785f54656c656d657472795f5061636b6574",
            "format": "hex",
            "source_tag": "hex_sensor"
        }
    })
    hex_content = json.loads(res_hex["result"]["content"][0]["text"])
    print(f"      ✓ Hex полезная нагрузка принята: {hex_content['bytes_ingested']} байт")

    res_b64 = send_rpc("tools/call", {
        "name": "ingest_raw_bytes",
        "arguments": {
            "data": "QmFzZTY0X1RlbGVtZXRyeV9GcmFtZV9EYXRh",
            "format": "base64",
            "source_tag": "b64_sensor"
        }
    })
    b64_content = json.loads(res_b64["result"]["content"][0]["text"])
    print(f"      ✓ Base64 полезная нагрузка принята: {b64_content['bytes_ingested']} байт")

    # 6. Stats & Recent resources
    print("\n[6/6] Проверка сенсорных ресурсов (sensory://stats и sensory://recent)...")
    res_stats = send_rpc("resources/read", {"uri": "sensory://stats"})
    stats_data = json.loads(res_stats["result"]["contents"][0]["text"])
    sensory_st = stats_data["sensory"]
    print(f"      ✓ sensory://stats: всего событий {sensory_st['total_events']} (терминал: {sensory_st['terminal_events']}, файлов: {sensory_st['file_events']}, сырых байт: {sensory_st['raw_bytes_events']})")
    print(f"      ✓ Суммарный объем: {sensory_st['total_bytes_streamed']} байт")

    res_recent = send_rpc("resources/read", {"uri": "sensory://recent"})
    recent_data = json.loads(res_recent["result"]["contents"][0]["text"])
    print(f"      ✓ sensory://recent: событий в кольцевом буфере: {recent_data['count']}")

    p.terminate()
    print("\n" + "=" * 65)
    print("  ✓ ВСЕ ПРОВЕРКИ ФАЗЫ 3 (3.1 и 3.2) УСПЕШНО ПРОЙДЕНЫ!")
    print("=" * 65)

if __name__ == "__main__":
    main()
