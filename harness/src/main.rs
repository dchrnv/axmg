//! # axmg Harness — Внешняя обвязка и адаптер ядра axmg
//!
//! Модуль внешней обвязки, который подключает библиотеку ядра `shelf` (axmg-kernel)
//! как независимую внешнюю зависимость и предоставляет JSON/CLI адаптеры.

use serde::{Deserialize, Serialize};
use shelf::store::Store;
use shelf::wheel::WheelState;
use shelf::merge::TieBreak;

/// JSON-запрос от внешнего мира к ядру.
#[derive(Debug, Deserialize)]
pub struct SensoryInputPayload {
    pub text: String,
    pub revolution: Option<u64>,
}

/// JSON-ответ внешней обвязки.
#[derive(Debug, Serialize)]
pub struct MemoryResponse {
    pub status: String,
    pub revolution: u32,
    pub surprise_static: f64,
    pub surprise_growth: f64,
    pub surprise_score: f64,
    pub verdict: String,
    pub total_tokens: usize,
    pub sequence_len: usize,
}

fn main() {
    println!("=== axmg (Axiom Gemini) — Outer Harness v0.1.0 ===");

    // 1. Инициализация изолированного ядра shelf из библиотеки
    let mut store = Store::new();
    store.init_factory();

    let mut wheel = WheelState::with_store(store);

    // 2. Имитация входящего внешнего JSON-запроса
    let raw_json = r#"{"text": "hello world", "revolution": 1}"#;
    let payload: SensoryInputPayload = serde_json::from_str(raw_json).expect("некорректный JSON");

    println!("Получен внешний запрос: '{}'", payload.text);

    // 3. Обновление колеса времени и памяти с онлайн-вычислением Surprise в горячем цикле
    let report = wheel.turn(payload.text.as_bytes(), 2, TieBreak::BirthOrder);

    let verdict_str = match report.verdict {
        shelf::SurpriseVerdict::Familiar => "FAMILIAR",
        shelf::SurpriseVerdict::NovelGrowth => "NOVEL_GROWTH",
        shelf::SurpriseVerdict::Noise => "NOISE",
    };

    // 4. Формирование структурированного ответа внешней обвязки
    let response = MemoryResponse {
        status: "EVENT_INTEGRATED".to_string(),
        revolution: report.revolution,
        surprise_static: report.s_static,
        surprise_growth: report.s_growth,
        surprise_score: report.surprise_score,
        verdict: verdict_str.to_string(),
        total_tokens: wheel.store.len(),
        sequence_len: wheel.sequence.ids.len(),
    };

    println!("\n=== Ответ ядра (JSON) ===");
    println!("{}", serde_json::to_string_pretty(&response).unwrap());
}

