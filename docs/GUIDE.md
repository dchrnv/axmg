# axmg — Руководство по интеграции и использованию библиотеки (Library Guide)

**Версия:** 0.1.0 (LTS-фасад)  
**Крейт:** `shelf` (`axmg-kernel`)  
**Лицензия:** AGPLv3

---

## 1. Философия: Зачем библиотека вместо векторной БД?

Большинство современных AI-агентов используют векторные базы данных (Pinecone, Qdrant, Chroma). Это порождает четыре фундаментальные проблемы:
1. **Стохастичность и галлюцинации:** Один и тот же текст с разными эмбеддингами даёт разную выдачу.
2. **Тяжелые зависимости:** Требуется инференс тяжелых нейросетей-эмбеддеров на GPU/CPU.
3. **Бесконечное накопление мусора:** Векторные БД не умеют естественно забывать — контекст забивается устаревшим шумом.
4. **Потеря точной побитовой идентичности:** Вектор усредняет семантику и теряет точные идентификаторы (хеши коммитов, стектрейсы, код).

`axmg` решает это на уровне алгоритмических структур данных:
- **Детерминированный Merkle DAG (SHA-256):** Память строится снизу вверх из сырых байт без словарей.
- **Динамика вытеснения ($K=3$ + срез 25%):** Естественное забывание шума и удержание в фокусе только актуальных концептов.
- **Ассоциативный граф (PPMI):** Мгновенный поиск связанных концептов через совместную встречаемость без векторных эмбеддингов.
- **96.89% экономия контекста LLM:** Агент получает сжатый концентрат активного фокуса вместо мегабайтов сырых логов.

---

## 2. Подключение к проекту

### В Cargo-воркспейсе:
```toml
[dependencies]
shelf = { path = "../shelf" }
```

### Как внешняя зависимость через Git:
```toml
[dependencies]
axmg = { git = "https://github.com/dchrnv/axmg", package = "shelf", branch = "main" }
```

---

## 3. Рецепты интеграции (Integration Recipes)

### Рецепт 1: Подключение к агенту (LLM Agent Context Loop)

Задача: Агент выполняет задачи, память непрерывно впитывает происходящее и динамически формирует контекст для системного промпта.

```rust
use shelf::Axmg;

pub struct AgentSession {
    memory: Axmg,
}

impl AgentSession {
    pub fn new(storage_path: &str) -> Self {
        Self {
            memory: Axmg::open(storage_path).expect("Failed to open axmg storage"),
        }
    }

    /// Вызывается перед каждым обращением к LLM
    pub fn build_system_prompt(&self, base_prompt: &str, user_query: &str) -> String {
        // 1. Извлекаем активный сжатый фокус внимания
        let focus_context = self.memory.prompt_context();

        // 2. Делаем ассоциативный поиск (recall) под конкретный запрос пользователя
        let recall = self.memory.recall(user_query, Some(5));

        format!(
            "{base_prompt}\n\n[ПАМЯТЬ АГЕНТА]\nФокус: {focus_context}\nАссоциации: {}",
            recall.context_string
        )
    }

    /// Вызывается после получения ответа от пользователя или выполнения команды
    pub fn on_event(&mut self, text: &str, tag: &str) {
        let _ = self.memory.ingest_tagged(text, tag);
    }
}
```

---

### Рецепт 2: Сенсорный фоновый поток (Background Event Streamer)

Задача: Фоновый поток слушает терминал, системные логи или Git и непрерывно «кормит» память сырыми байтами.

```rust
use shelf::Axmg;
use std::sync::{Arc, Mutex};
use std::thread;

fn start_sensory_daemon(shared_memory: Arc<Mutex<Axmg>>) {
    thread::spawn(move || {
        // Имитация чтения потока (например, tail -f или сокет)
        loop {
            let chunk = b"cargo check finished with exit code 0\n";
            {
                let mut mem = shared_memory.lock().unwrap();
                let report = mem.ingest_bytes_tagged(chunk, "terminal").unwrap();
                if report.is_novel {
                    println!("Замечен новый паттерн поведения! Score: {:.3}", report.surprise_score);
                }
            }
            thread::sleep(std::time::Duration::from_secs(5));
        }
    });
}
```

---

### Рецепт 3: Голосовая «говорилка» (Vocalizer & Spoken Head)

Задача: Подключить синтезатор речи (TTS) к ядру памяти без раздувания ядра.

```rust
use shelf::Axmg;

pub struct VocalizerHead {
    memory: Axmg,
}

impl VocalizerHead {
    pub fn on_tick(&mut self) {
        // Забираем снимок фокуса
        let focus = self.memory.focus();
        
        // Передаем топ-концепты в аудио-стример
        for concept in focus.top_concepts().iter().take(3) {
            println!("Озвучиваем актуальный концепт: '{}' (вес: {:.2})", concept.text, concept.ppmi_weight);
            // audio_streamer.synthesize(&concept.text);
        }
    }
}
```

---

## 4. Гарантии надежности и ACID-персистентность

1. **Единая транзакция (`save_all`):**
   При сохранении на диск тела новых токенов Merkle DAG и события вытеснения `DeathLog` записываются в единой транзакции `redb`. Падение питания не может оставить базу в полу-сохраненном состоянии.
2. **Cold Start (Холодный старт):**
   При рестарте процесса вызов `Axmg::open("path.redb")`:
   - проверяет валидность всех Merkle-цепочек (детекция повреждений),
   - восстанавливает точный номер эпохи (оборота),
   - воссоздает множество активного фокуса $K=3$ бит-в-бит через replay-механизм.
3. **Безопасность параллельного чтения:**
   Метод `memory.focus()` возвращает `Arc<FocusSet>`. Это позволяет раздавать снапшот фокуса десяткам читающих потоков (веб-сервер, UI, голосовой генератор) без каких-либо блокировок (`lock-free`).

---

## 5. Документация и запуск тестов

- **Интерактивная HTML-вики:**
  ```bash
  cargo doc -p shelf --no-deps --open
  ```
- **Запуск примера:**
  ```bash
  cargo run -p shelf --example quickstart
  ```
- **Сквозная E2E-верификация:**
  ```bash
  python3 verify_phase4_e2e.py
  ```
