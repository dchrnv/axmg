# 🏗️ Архитектура AXMG (Axiom Gemini)

Это 100% реальная, физически существующая архитектура, которая прямо сейчас лежит в проекте `axmg`. Ниже приведена прямая параллель между логическими слоями системы и физическим кодом.

## Диаграмма слоев

```mermaid
graph TD
    subgraph Layer 3: Application / UI
        UI[Tauri Desktop App / Path of Heroes]
        GLUE[Python / Rust Glue Script]
    end

    subgraph Layer 2: LLM & Dispatch
        LLM[Local LLM - LM Studio / Ollama]
        MCP_SERVER[axmg-memory-mcp Server]
    end

    subgraph Layer 1: The Core (Rust)
        ADAPTER[Kernel Adapter]
        WHEEL[WheelState - Sliding Window]
        STORE[Merkle DAG Store - redb]
        PPMI[PPMI Filter - Associations]
        MERGE[Generational Merge Engine]
    end

    UI -->|User Input| GLUE
    GLUE -->|1. recall| MCP_SERVER
    MCP_SERVER -->|Context| GLUE
    GLUE -->|2. Prompt + Context| LLM
    LLM -->|Response| GLUE
    GLUE -->|3. remember| MCP_SERVER
    
    MCP_SERVER <--> ADAPTER
    ADAPTER <--> WHEEL
    WHEEL <--> STORE
    WHEEL <--> MERGE
    ADAPTER <--> PPMI
```

## 🧠 Слой 1: Топологическое Ядро (The Core)
**Где лежит:** папка `axmg/shelf/`
Математическое сердце системы, написанное на Rust. Оно ничего не знает о человеческом языке, векторных эмбеддингах или нейросетях.
* **Store (`redb`):** Дисковая NoSQL база, хранящая Merkle DAG токенов (база `axmg_store.redb`).
* **WheelState (`wheel.rs`):** Скользящее окно оперативной памяти. Поддерживает лимит в 100,000 токенов для защиты от утечек памяти.
* **Generational Merge Engine (`merge.rs`):** Ищет повторяющиеся пары и склеивает их "волнами" (поколениями), собирая слоги из букв, а слова из слогов.
* **PPMI Filter (`ppmi.rs`):** Оценивает статистическую значимость связей, чтобы отличать реальный контекст от случайного шума. Поддерживает динамическое изменение порогов (строгий "академический" режим или чуткий "диалоговый" режим).

## 🌉 Слой 2: Протокол связи (LLM & Dispatch)
**Где лежит:** папка `axmg/mcp/`
Стандартизированный мост Model Context Protocol (JSON-RPC через stdio). Позволяет любой LLM или клиенту общаться с Ядром.
* **axmg-memory-mcp Server:** Скомпилированный бинарник на Rust, который слушает команды.
* **Kernel Adapter (`adapter.rs`):** Обёртка над Ядром, экспонирующая три главных инструмента:
  * `remember(text)`: Загрузить текст в графовую базу.
  * `recall(query, frequency_floor, ppmi_threshold)`: Вытянуть ассоциации из памяти (Третье состояние - Dispatch). 
  * `check_surprise(text)`: Узнать, насколько этот текст несет структурную новизну ($S_{growth}$).

## 🐍 Слой 3: Приложение и Нейросеть (Application / UI)
**Где лежит:** файлы `glue.py`, `run_experiment.py` и внешнее ПО (LM Studio/Ollama).
Связывает пользователя, Ядро и "речевой аппарат" (LLM).
* **GLUE:** Питоновские скрипты, которые управляют потоком данных. Они перехватывают ввод, дергают Ядро (через MCP) за ассоциациями, формируют промпт и отправляют его в нейросеть.
* **LLM:** Локальная модель (например, Dolphin или Qwen), запущенная в LM Studio (`127.0.0.1:1234`). Она используется исключительно для формулирования ответов на основе жестких фактов, выданных Ядром.

## 🔮 Слой 4: Будущее (Tauri / UI)
**Статус:** В планах.
Десктопное приложение "Путь Героев". Графическая оболочка на Tauri для визуализации графа памяти в реальном времени, карточки профилей и бесшовная интеграция с ОС.
