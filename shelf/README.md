# axmg-kernel (shelf) — Движок ассоциативной памяти на базе Merkle DAG

[![License: AGPL v3](https://img.shields.io/badge/License-AGPLv3-blue.svg)](file:///home/assis/axmg/LICENSE)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024-orange.svg)](https://www.rust-lang.org)
[![Status: Frozen v0.1.0](https://img.shields.io/badge/Status-Frozen%20v0.1.0-green.svg)](file:///home/assis/axmg/STATUS.md)

`shelf` — это встраиваемая, независимая и детерминированная Rust-библиотека ассоциативной памяти без использования векторных баз данных, эмбеддингов и нейросетевых словарей.

Она строит композитную память непосредственно из сырых байтовых потоков, используя криптографический **Merkle DAG (SHA-256)**, динамику времени (**Wheel**), механизм фильтрации шума (**Eviction $K=3$**) и ассоциативный граф (**PPMI + Recall**).

---

## 🚀 Быстрый старт (Quickstart)

Добавьте библиотеку в ваш `Cargo.toml`:

```toml
[dependencies]
shelf = { path = "../shelf" }
```

### 1. Минимальный пример (In-Memory)

```rust
use axmg::Axmg;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Создаем движок в оперативной памяти
    let mut memory = Axmg::in_memory();

    // 2. Интегрируем поток фактов (горячий цикл: свертка, Surprise, вытеснение)
    let report = memory.ingest("Rust and Merkle DAG combine to build axmg")?;
    println!("Оборот: {}, Удивление: {:.3}, Вердикт: {:?}", 
        report.revolution, report.surprise_score, report.verdict);

    // 3. Извлекаем контекст активного фокуса внимания для системного промпта LLM
    let prompt_context = memory.prompt_context();
    println!("Prompt Context: {prompt_context}");

    // 4. Ассоциативный поиск (Recall)
    let recall = memory.recall("Merkle", Some(5));
    println!("Ассоциации: {}", recall.context_string);

    // 5. Оценка гипотезы без изменения памяти (Read-Only)
    let surprise = memory.check_surprise("Rust systems programming");
    println!("S-static: {:.3} | Вердикт: {:?}", surprise.s_static, surprise.verdict);

    Ok(())
}
```

Запустить готовый пример из репозитория:
```bash
cargo run -p shelf --example quickstart
```

---

## 💾 Режимы хранения

### Режим 1: Полная персистентность (ACID на `redb`)

```rust
use axmg::Axmg;

// Открывает существующую базу или создает новую по указанному пути.
// Таймлайн автоматически пишется рядом в "agent_memory_timeline.jsonl".
let mut memory = Axmg::open("agent_memory.redb")?;

memory.ingest("Персистентное событие")?;
// Данные атомарно закоммичены на диск!
```

### Режим 2: Гибкая конфигурация через `AxmgBuilder`

```rust
use axmg::{Axmg, AxmgBuilder, TieBreak};

let memory = AxmgBuilder::new()
    .db_path("custom_store.redb")
    .timeline_path("events.jsonl")
    .birth_threshold(2)               // Минимальная частота для рождения составного токена
    .tie_break(TieBreak::BirthOrder)   // Детерминированный порядок сортировки волн
    .focus_window(25)                 // Число топ-концептов в фокусе
    .auto_save(true)                  // Транзакционный сброс на каждом шаге
    .build()?;
```

---

## 🧠 Ключевые архитектурные концепции

| Концепт | Описание | Модуль |
|---|---|---|
| **Store (Merkle DAG)** | Неизменяемая SoA-арена токенов. Детерминированные SHA-256 хеши. Фабричные 256 байт на нулевом уровне. | [`store.rs`](file:///home/assis/axmg/shelf/src/store.rs) |
| **StreamingRecognizer** | Потоковая свёртка чанков произвольного размера за $O(N)$ без словаря. | [`recognizer.rs`](file:///home/assis/axmg/shelf/src/recognizer.rs) |
| **WheelState (Колесо времени)** | Дискретные эпохи (обороты). Трекинг времени последнего касания (`last_touched`). | [`wheel.rs`](file:///home/assis/axmg/shelf/src/wheel.rs) |
| **Eviction & FocusSet ($K=3$)** | Generational GC (mark без sweep). Корневое окно $K=3$ оборотов + срез 25% пограничных по PPMI-весу. | [`death.rs`](file:///home/assis/axmg/shelf/src/death.rs) |
| **Surprise ($S_{\text{static}}, S_{\text{growth}}$)** | Потоковый расчет удивления в горячем цикле без клонирования стора. Классификация: Familiar / NovelGrowth / Noise. | [`surprise.rs`](file:///home/assis/axmg/shelf/src/surprise.rs) |
| **PPMI Recall** | Субграфный ассоциативный поиск по матрице совместной встречаемости. | [`recall.rs`](file:///home/assis/axmg/shelf/src/recall.rs) |

---

## 🛠 Справочник API (`Axmg`)

### Методы интеграции и записи
- `engine.ingest(text: &str) -> Result<IngestReport, AxmgError>` — интеграция строки с тегом `"general"`.
- `engine.ingest_tagged(text: &str, tag: &str) -> Result<IngestReport, AxmgError>` — интеграция строки с пользовательским тегом.
- `engine.ingest_bytes(bytes: &[u8]) -> Result<IngestReport, AxmgError>` — интеграция сырых байт (логи, бинарные потоки).

### Методы извлечения и поиска
- `engine.recall(query: &str, limit: Option<usize>) -> RecallResponse` — ассоциативный субграфный поиск концептов вокруг запроса.
- `engine.focus() -> Arc<FocusSet>` — мгновенный lock-free доступ к снапшоту активных токенов.
- `engine.focused_concepts() -> &[FocusConcept]` — срез топ-концептов текущего фокуса.
- `engine.prompt_context() -> String` — готовая строка активного контекста для внедрения в системный промпт LLM (экономия токенов **96.89%**).

### Диагностика и статистика
- `engine.check_surprise(hypothesis: &str) -> SurpriseReport` — проверка удивления без мутации хранилища.
- `engine.stats() -> EngineStats` — статистика: число токенов, коэффициент сжатия, доля фокуса.
- `engine.timeline(limit: Option<usize>) -> Vec<TimelineEntry>` — выборка последних событий таймлайна.
- `engine.save() -> Result<(), AxmgError>` — принудительный сброс на диск.

---

## 🛡 Фундаментальные инварианты библиотеки

1. **Строгий детерминизм:** Одинаковый поток байтов всегда порождает идентичные SHA-256 хеши и структуру Merkle DAG.
2. **Смерть — это прекращение обращения, а не удаление:** Тела токенов в `Store` неизменны (Append-only). Вытеснение фиксируется как лог событий `Unfocus`.
3. **Безопасность типов:** Никаких паник в библиотечном коде; все операции возвращают `Result<T, AxmgError>`.
4. **Lock-Free чтение:** Чтение фокуса внимания не блокирует входящий поток интеграции событий.

---

## 📖 Локальная документация (Rustdoc)

Сгенерировать и открыть интерактивную HTML-вики документацию с перекрёстными ссылками и сигнатурами:

```bash
cargo doc -p shelf --no-deps --open
```
