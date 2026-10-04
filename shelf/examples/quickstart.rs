//! # Quickstart: Использование axmg (shelf) как встраиваемой библиотеки памяти
//!
//! Запуск: `cargo run -p shelf --example quickstart`

use shelf::Axmg;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("============================================================");
    println!("  axmg (shelf) — Демонстрация высокоуровневой библиотеки");
    println!("============================================================\n");

    // 1. Создаем легковесный in-memory экземпляр движка
    let mut memory = Axmg::in_memory();
    println!("1. Инициализировано ядро памяти в RAM (токенов: {})", memory.stats().total_tokens);

    // 2. Скармливаем факты и обучающие последовательности
    let facts = [
        "Rust is a systems programming language focused on safety and speed.",
        "Merkle DAG stores immutable cryptographic history of composite tokens.",
        "axmg combines Merkle DAG and associative PPMI graph for LLM memory.",
        "Rust and Merkle DAG allow deterministic compression and instant recall.",
    ];

    println!("\n2. Интеграция текстового потока в память:");
    for fact in &facts {
        let report = memory.ingest(fact)?;
        println!(
            "   [Оборот {:2}] Байты: {:2} | Удивление: {:.3} | Вердикт: {:?} | Новых токенов: {}",
            report.revolution,
            report.bytes_ingested,
            report.surprise_score,
            report.verdict,
            report.new_tokens_count
        );
    }

    // 3. Активный фокус внимания (K=3 эпохи корней + срез по весу)
    println!("\n3. Активный фокус внимания (Prompt Context для LLM):");
    println!("   {}", memory.prompt_context());

    // 4. Ассоциативный поиск (Recall)
    println!("\n4. Ассоциативное вспоминание (Recall):");
    let queries = ["Rust", "Merkle memory", "PPMI"];
    for query in &queries {
        let result = memory.recall(query, Some(3));
        println!("   Запрос: '{}'", query);
        println!("   -> Найденные ассоциации: {}", result.context_string);
    }

    // 5. Оценка гипотез на удивление (check_surprise) БЕЗ мутации памяти
    println!("\n5. Оценка удивления (Surprise check):");
    let familiar = memory.check_surprise("Rust systems programming language");
    println!(
        "   Знакомая фраза -> S-static: {:.3} | Вердикт: {:?}",
        familiar.s_static, familiar.verdict
    );

    let noise = memory.check_surprise("xq98234 *&^% non_sense_random");
    println!(
        "   Случайный шум  -> S-static: {:.3} | Вердикт: {:?}",
        noise.s_static, noise.verdict
    );

    // 6. Системная статистика
    let stats = memory.stats();
    println!("\n6. Финальная статистика:");
    println!("   - Всего токенов в Merkle DAG: {}", stats.total_tokens);
    println!("   - Оборотов колеса:            {}", stats.current_revolution);
    println!("   - Коэффициент сжатия:         {:.2}x", stats.compression_ratio);
    println!("   - Токенов в активном фокусе:  {}", stats.active_focus_tokens);

    println!("\n✓ Работа завершена успешно.");
    Ok(())
}
