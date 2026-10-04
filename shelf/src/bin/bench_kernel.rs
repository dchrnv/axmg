use axmg::death::DeathLog;
use axmg::merge::{Sequence, TieBreak};
use axmg::recall::{recall, RecallConfig};
use axmg::recognizer::StreamingRecognizer;
use axmg::store::{Store, TokenId};
use axmg::wheel::WheelState;
use std::path::Path;
use std::time::Instant;

fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * pct).round() as usize;
    sorted[idx]
}

fn print_stats(name: &str, mut latencies_us: Vec<f64>, unit: &str) {
    latencies_us.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = latencies_us.len();
    let sum: f64 = latencies_us.iter().sum();
    let avg = if n > 0 { sum / n as f64 } else { 0.0 };
    let min = latencies_us.first().copied().unwrap_or(0.0);
    let p50 = percentile(&latencies_us, 0.50);
    let p95 = percentile(&latencies_us, 0.95);
    let p99 = percentile(&latencies_us, 0.99);
    let max = latencies_us.last().copied().unwrap_or(0.0);

    println!(
        "{:<32} | N={:<5} | Min: {:>7.2} | Avg: {:>7.2} | p50: {:>7.2} | p95: {:>7.2} | p99: {:>7.2} | Max: {:>7.2} {}",
        name, n, min, avg, p50, p95, p99, max, unit
    );
}

fn main() {
    println!("=========================================================================================");
    println!("                       AXMG KERNEL BENCHMARK (Release Mode)                              ");
    println!("=========================================================================================\n");

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let gita_path = Path::new(manifest_dir).join("../docs/corpus/bhagavad_gita.txt");
    let gita_bytes = match std::fs::read(&gita_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Не удалось прочитать {}: {}", gita_path.display(), e);
            return;
        }
    };

    println!("Загружен тестовый корпус: {} ({} байт / {:.2} МБ)", gita_path.display(), gita_bytes.len(), gita_bytes.len() as f64 / (1024.0 * 1024.0));

    // ---------------------------------------------------------------------------------------------
    // БЕНЧ 1: Пропускная способность StreamingRecognizer (MB/s) при разных размерах чанков
    // ---------------------------------------------------------------------------------------------
    println!("\n[1] ПРОПУСКНАЯ СПОСОБНОСТЬ STREAMING RECOGNIZER");
    println!("-----------------------------------------------------------------------------------------");

    // Инициализируем прогретый стор
    let mut store = Store::new();
    store.init_factory();

    // Заполняем стор базовыми токенами из части корпуса для реалистичной нагрузки
    let train_slice = &gita_bytes[..gita_bytes.len().min(100_000)];
    let ids: Vec<TokenId> = train_slice.iter().map(|&b| store.intern(0, &[b as u32])).collect();
    let seq = Sequence::from_ids(&store, ids);
    let (out_seq, _) = axmg::merge::run_traced(&mut store, seq, 2, TieBreak::BirthOrder);
    println!("Стор инициализирован: {} Merkle-токенов, обучающая свёртка: {} -> {} токенов", store.len(), train_slice.len(), out_seq.len());

    let chunk_sizes = [64, 512, 4096, 65536, gita_bytes.len()];
    for &chunk_size in &chunk_sizes {
        let label = if chunk_size == gita_bytes.len() {
            "Монолит (2.7 МБ)".to_string()
        } else if chunk_size >= 1024 {
            format!("Чанки {} КБ", chunk_size / 1024)
        } else {
            format!("Чанки {} байт", chunk_size)
        };

        // Прогон
        let start = Instant::now();
        let mut recognizer = StreamingRecognizer::new(&store);
        let total_tokens;

        for chunk in gita_bytes.chunks(chunk_size) {
            recognizer.feed_bytes(chunk);
        }
        total_tokens = recognizer.finish().len();
        let elapsed = start.elapsed();

        let total_mb = gita_bytes.len() as f64 / (1024.0 * 1024.0);
        let throughput = total_mb / elapsed.as_secs_f64();

        println!(
            "{:<25} | Время: {:>7.2} мс | Скорость: {:>7.2} МБ/с | Токенов на выходе: {}",
            label,
            elapsed.as_secs_f64() * 1000.0,
            throughput,
            total_tokens
        );
    }

    // ---------------------------------------------------------------------------------------------
    // БЕНЧ 2: Задержка Wheel::turn + Online Surprise в горячем цикле (микросекунды)
    // ---------------------------------------------------------------------------------------------
    println!("\n[2] ЗАДЕРЖКА WHEEL::TURN + ONLINE SURPRISE (Горячий цикл)");
    println!("-----------------------------------------------------------------------------------------");

    let mut wheel = WheelState::with_store(store.clone());
    let mut death_log = DeathLog::new();
    let sample_chunks: Vec<&[u8]> = gita_bytes.chunks(256).take(200).collect();
    let mut turn_latencies_us = Vec::with_capacity(sample_chunks.len());

    for chunk in &sample_chunks {
        let t0 = Instant::now();
        let _report = wheel.turn(chunk, 2, TieBreak::BirthOrder);
        let _ = death_log.mark(&wheel);
        let _ = death_log.weight_cut(&wheel);
        let dt = t0.elapsed();
        turn_latencies_us.push(dt.as_secs_f64() * 1_000_000.0);
    }

    print_stats("Wheel::turn + Surprise + Death", turn_latencies_us, "µs");

    // ---------------------------------------------------------------------------------------------
    // БЕНЧ 3: Задержка ассоциативного вспоминания (Recall) по графу PPMI
    // ---------------------------------------------------------------------------------------------
    println!("\n[3] ЗАДЕРЖКА АССОЦИАТИВНОГО ОБХОДА (RECALL)");
    println!("-----------------------------------------------------------------------------------------");

    let queries = [
        "кришна",
        "йога знание действие",
        "arjuna warrior battlefield",
        "дхарма закон истина путь",
        "душа бессмертна огонь не жжет",
    ];

    let recall_cfg = RecallConfig {
        limit: 10,
        frequency_floor: 1,
        ppmi_threshold: 0.0,
        min_level: 1,
        window_min: 2,
        window_max: 5,
    };

    let mut recall_latencies_us = Vec::with_capacity(queries.len() * 100);
    for _ in 0..100 {
        for q in &queries {
            let t0 = Instant::now();
            let _res = recall(&wheel.store, &wheel.sequence.ids, q.as_bytes(), None, &recall_cfg);
            let dt = t0.elapsed();
            recall_latencies_us.push(dt.as_secs_f64() * 1_000_000.0);
        }
    }

    print_stats("recall (10 ассоциаций)", recall_latencies_us, "µs");

    // ---------------------------------------------------------------------------------------------
    // БЕНЧ 4: Lock-Free RCU снапшот FocusSet (наносекунды)
    // ---------------------------------------------------------------------------------------------
    println!("\n[4] LOCK-FREE SNAPSHOT FOCUSSET (Чтение активного фокуса)");
    println!("-----------------------------------------------------------------------------------------");

    let snapshot = death_log.snapshot(&wheel, 20);
    let arc_snap = std::sync::Arc::new(snapshot);
    let rcu_cell = std::sync::RwLock::new(arc_snap);

    let iterations = 100_000;
    let t0 = Instant::now();
    for _ in 0..iterations {
        let snap_clone = rcu_cell.read().unwrap().clone();
        std::hint::black_box(&snap_clone);
    }
    let total_snap_ns = t0.elapsed().as_nanos() as f64;
    let per_snap_ns = total_snap_ns / iterations as f64;

    println!(
        "{:<32} | N={:<5} | Среднее время снапшота: {:>7.2} ns ({:.2} млн опс/сек)",
        "FocusSet Arc-clone",
        iterations,
        per_snap_ns,
        1_000.0 / per_snap_ns
    );

    // ---------------------------------------------------------------------------------------------
    // БЕНЧ 5: REDB ACID Commit и Cold Start Recovery (миллисекунды)
    // ---------------------------------------------------------------------------------------------
    println!("\n[5] ТРАНЗАКЦИОННАЯ ПЕРСИСТЕНТНОСТЬ REDB (Запись и Холодный Старт)");
    println!("-----------------------------------------------------------------------------------------");

    let temp_dir = std::env::temp_dir();
    let test_db = temp_dir.join(format!("bench_redb_{}.redb", std::process::id()));
    if test_db.exists() {
        let _ = std::fs::remove_file(&test_db);
    }

    let t_save = Instant::now();
    axmg::persist::save_all(&wheel.store, &death_log, &test_db).expect("save_all");
    let save_ms = t_save.elapsed().as_secs_f64() * 1000.0;

    let t_load = Instant::now();
    let (loaded_store, loaded_dl) = axmg::persist::load_all(&test_db).expect("load_all");
    let load_ms = t_load.elapsed().as_secs_f64() * 1000.0;

    let db_size_kb = std::fs::metadata(&test_db).map(|m| m.len() as f64 / 1024.0).unwrap_or(0.0);

    println!("{:<32} | Время записи: {:>7.2} мс | Токенов: {} | Размер БД: {:.1} КБ", "redb::save_all (ACID Commit)", save_ms, wheel.store.len(), db_size_kb);
    println!("{:<32} | Время загрузки: {:>6.2} мс | Восстановлено: {} токенов, {} событий", "redb::load_all (Cold Start)", load_ms, loaded_store.len(), loaded_dl.events().len());

    let _ = std::fs::remove_file(&test_db);

    println!("\n=========================================================================================");
    println!("                        БЕНЧМАРК ЯДРА УСПЕШНО ЗАВЕРШЕН                                   ");
    println!("=========================================================================================\n");
}
