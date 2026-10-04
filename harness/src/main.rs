//! # axmg Harness — Внешняя обвязка и адаптер ядра axmg
//!
//! Модуль внешней обвязки, который подключает библиотеку ядра `shelf` (axmg-kernel)
//! как независимую внешнюю зависимость через высокоуровневый фасад `Axmg`.

use serde::Deserialize;
use shelf::Axmg;

/// JSON-запрос от внешнего мира к ядру.
#[derive(Debug, Deserialize)]
pub struct SensoryInputPayload {
    pub text: String,
    pub tag: Option<String>,
}

fn main() {
    println!("=== axmg (Axiom Gemini) — Outer Harness v0.1.0 ===");

    // 1. Инициализация универсального движка памяти Axmg из библиотеки
    let mut engine = Axmg::in_memory();

    // 2. Имитация входящего внешнего JSON-запроса
    let raw_json = r#"{"text": "hello world memory engine", "tag": "sensor"}"#;
    let payload: SensoryInputPayload = serde_json::from_str(raw_json).expect("некорректный JSON");

    println!("Получен внешний запрос: '{}'", payload.text);

    // 3. Интеграция данных через единый метод библиотеки
    let report = engine
        .ingest_tagged(&payload.text, payload.tag.as_deref().unwrap_or("general"))
        .expect("ошибка интеграции данных");

    println!("\n=== Отчёт интеграции (JSON) ===");
    println!("{}", serde_json::to_string_pretty(&report).unwrap());

    println!("\n=== Активный фокус внимания (Промпт) ===");
    println!("{}", engine.prompt_context());

    println!("\n=== Системная статистика ядра (JSON) ===");
    println!("{}", serde_json::to_string_pretty(&engine.stats()).unwrap());
}

