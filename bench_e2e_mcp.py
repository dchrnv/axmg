#!/usr/bin/env python3
"""
E2E Benchmark for axmg MCP Servers (axmg-mcp & axmg-sensory-mcp)
Measures JSON-RPC stdio roundtrip latencies, RPS, and raw sensory pipe throughput.
"""

import json
import os
import subprocess
import sys
import time
from typing import List, Dict, Any

def percentile(data: List[float], pct: float) -> float:
    if not data:
        return 0.0
    k = (len(data) - 1) * pct
    f = int(k)
    c = min(f + 1, len(data) - 1)
    d = k - f
    return data[f] + d * (data[c] - data[f])

def print_row(name: str, latencies_ms: List[float], rps: float):
    latencies_ms.sort()
    n = len(latencies_ms)
    min_v = latencies_ms[0] if n else 0.0
    avg_v = sum(latencies_ms) / n if n else 0.0
    p50_v = percentile(latencies_ms, 0.50)
    p95_v = percentile(latencies_ms, 0.95)
    p99_v = percentile(latencies_ms, 0.99)
    max_v = latencies_ms[-1] if n else 0.0

    print(f"{name:<34} | N={n:<4} | Min: {min_v:>6.2f} | Avg: {avg_v:>6.2f} | p50: {p50_v:>6.2f} | p95: {p95_v:>6.2f} | p99: {p99_v:>6.2f} | Max: {max_v:>6.2f} ms | {rps:>7.1f} req/s")

class McpClient:
    def __init__(self, binary_path: str, env=None):
        self.proc = subprocess.Popen(
            [binary_path],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
            env=env or os.environ.copy()
        )
        self.req_id = 0

    def send_req(self, method: str, params: Dict[str, Any] = None) -> (Dict[str, Any], float):
        self.req_id += 1
        msg = {
            "jsonrpc": "2.0",
            "id": self.req_id,
            "method": method,
            "params": params or {}
        }
        raw = json.dumps(msg)
        t0 = time.perf_counter()
        self.proc.stdin.write(raw + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        dt = (time.perf_counter() - t0) * 1000.0 # ms
        if not line:
            raise RuntimeError("MCP process died or closed stdout")
        return json.loads(line), dt

    def close(self):
        try:
            self.proc.terminate()
            self.proc.wait(timeout=2)
        except Exception:
            self.proc.kill()

def main():
    print("=========================================================================================================")
    print("                           AXMG MCP PROTOCOL BENCHMARK (Release Mode)                                    ")
    print("=========================================================================================================\n")

    release_bin = "./target/release/axmg-mcp"
    sensory_bin = "./target/release/axmg-sensory-mcp"

    if not os.path.exists(release_bin):
        print("Сборка axmg-mcp в release-режиме...")
        subprocess.run(["cargo", "build", "--release", "-p", "mcp"], check=True)

    tmp_db = f"/tmp/bench_mcp_{os.getpid()}.redb"
    tmp_tl = f"/tmp/bench_mcp_{os.getpid()}.jsonl"
    env = os.environ.copy()
    env["AXMG_DB_PATH"] = tmp_db
    env["AXMG_TIMELINE_PATH"] = tmp_tl

    client = McpClient(release_bin, env=env)

    # 1. Initialize
    init_res, init_dt = client.send_req("initialize", {
        "protocolVersion": "2024-11-05",
        "clientInfo": {"name": "bench-client", "version": "1.0"}
    })
    print(f"Сервер инициализирован за: {init_dt:.2f} ms | Протокол: {init_res.get('result', {}).get('protocolVersion')}")
    print("---------------------------------------------------------------------------------------------------------")

    N = 100

    # 2. Tool: remember
    rem_latencies = []
    t_start = time.perf_counter()
    for i in range(N):
        text = f"Событие {i}: алгоритм Merkle DAG обеспечивает хеширование композиций и быструю дедупликацию блоков"
        _, dt = client.send_req("tools/call", {
            "name": "remember",
            "arguments": {"text": text, "context_tag": "benchmark"}
        })
        rem_latencies.append(dt)
    rem_rps = N / (time.perf_counter() - t_start)
    print_row("Tool: remember", rem_latencies, rem_rps)

    # 3. Tool: check_surprise
    sur_latencies = []
    t_start = time.perf_counter()
    for i in range(N):
        cand = f"Гипотеза {i}: дедупликация блоков Merkle DAG"
        _, dt = client.send_req("tools/call", {
            "name": "check_surprise",
            "arguments": {"hypothesis": cand}
        })
        sur_latencies.append(dt)
    sur_rps = N / (time.perf_counter() - t_start)
    print_row("Tool: check_surprise", sur_latencies, sur_rps)

    # 4. Tool: recall
    rec_latencies = []
    t_start = time.perf_counter()
    for i in range(N):
        query = "Merkle DAG хеширование блоков"
        _, dt = client.send_req("tools/call", {
            "name": "recall",
            "arguments": {"query": query, "limit": 5}
        })
        rec_latencies.append(dt)
    rec_rps = N / (time.perf_counter() - t_start)
    print_row("Tool: recall", rec_latencies, rec_rps)

    # 5. Resource: memory://focused
    foc_latencies = []
    t_start = time.perf_counter()
    for _ in range(N):
        _, dt = client.send_req("resources/read", {"uri": "memory://focused"})
        foc_latencies.append(dt)
    foc_rps = N / (time.perf_counter() - t_start)
    print_row("Resource: memory://focused", foc_latencies, foc_rps)

    # 6. Resource: memory://stats
    stat_latencies = []
    t_start = time.perf_counter()
    for _ in range(N):
        _, dt = client.send_req("resources/read", {"uri": "memory://stats"})
        stat_latencies.append(dt)
    stat_rps = N / (time.perf_counter() - t_start)
    print_row("Resource: memory://stats", stat_latencies, stat_rps)

    # 7. Resource: memory://timeline
    tl_latencies = []
    t_start = time.perf_counter()
    for _ in range(N):
        _, dt = client.send_req("resources/read", {"uri": "memory://timeline"})
        tl_latencies.append(dt)
    tl_rps = N / (time.perf_counter() - t_start)
    print_row("Resource: memory://timeline", tl_latencies, tl_rps)

    client.close()

    # Очистка
    for p in [tmp_db, tmp_tl]:
        if os.path.exists(p):
            os.remove(p)

    # ---------------------------------------------------------------------------------------------------------
    # Сенсорный тест прямого пайплайна: axmg-sensory-mcp --raw
    # ---------------------------------------------------------------------------------------------------------
    print("\n---------------------------------------------------------------------------------------------------------")
    print("[СЕНСОРНЫЙ СТРИМЕР] Прямой пайплайн без JSON (axmg-sensory-mcp --raw):")
    sensory_tmp_db = f"/tmp/bench_sensory_{os.getpid()}.redb"
    sensory_tmp_tl = f"/tmp/bench_sensory_{os.getpid()}.jsonl"
    s_env = os.environ.copy()
    s_env["AXMG_SENSORY_DB_PATH"] = sensory_tmp_db
    s_env["AXMG_SENSORY_TIMELINE_PATH"] = sensory_tmp_tl

    # Генерируем 512 КБ тестовой телеметрии
    test_kb = 512
    payload = (b"TelemetryFrame: sensor_id=42, temp=36.6, pressure=101.3, status=OK\n" * (test_kb * 1024 // 68))
    actual_bytes = len(payload)

    t0 = time.perf_counter()
    p = subprocess.Popen(
        [sensory_bin, "--raw", "--chunk-size", "65536"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=s_env
    )
    _, stderr_out = p.communicate(input=payload, timeout=10)
    elapsed = time.perf_counter() - t0

    throughput_mb = (actual_bytes / (1024.0 * 1024.0)) / elapsed
    print(f"Пайплайн --raw: передано {actual_bytes / 1024.0:.1f} КБ за {elapsed*1000.0:.1f} мс | Скорость обработки и записи: {throughput_mb:.2f} МБ/с")

    for p_path in [sensory_tmp_db, sensory_tmp_tl]:
        if os.path.exists(p_path):
            os.remove(p_path)

    print("\n=========================================================================================================")
    print("                             БЕНЧМАРК MCP УСПЕШНО ЗАВЕРШЕН                                               ")
    print("=========================================================================================================\n")

if __name__ == "__main__":
    main()
