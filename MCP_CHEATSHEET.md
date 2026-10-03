# Быстрая справка по MCP (cheatsheet)

## Три инструмента

### `remember(text, context_tag?)`
Запомнить событие/факт. Вернёт новизну.
```
Пример: "Rust гарантирует безопасность памяти на уровне компилятора"
Ответ: { status: "STORED", is_novel_event: true, surprise: 1.8 }
```

### `recall(query, limit=5)`
Найти ассоциированные воспоминания.
```
Пример: "что я знаю о безопасности программирования?"
Ответ: [{ text: "...", relevance_score: 0.89 }, ...]
```

### `check_surprise(candidate_text)`
Оценить согласованность утверждения.
```
Пример: "Python — интерпретируемый язык"
Ответ: { verdict: "FAMILIAR", surprise_ratio: 0.15 }
```

## Три ресурса памяти

### `memory://focused`
Что в фокусе сейчас? Какие концепты активны?
```json
{
  "revolution": 43,
  "focus_count": 331,
  "active_tokens": [{ "token_id": 14205, "level": 2, "ppmi_weight": 14.85 }]
}
```

### `memory://stats`
Статистика хранилища: всего токенов, сжатие, размер фокуса.
```json
{
  "total_tokens": 408,
  "current_revolution": 43,
  "compression_ratio": 3.85,
  "active_focus_ratio": 0.811
}
```

### `memory://timeline`
Хронологический срез недавних событий памяти:
```json
{
  "total_events": 12,
  "recent_events": [{ "revolution": 43, "context_tag": "architecture", "text_preview": "..." }]
}
```

## Сенсорный сервер `axmg-sensory-mcp`
- **Ресурсы:** `sensory://stats`, `sensory://recent`
- **Инструмент:** `ingest_raw_bytes(data, encoding, source_tag)`
- **CLI пайплайн:** `cat telemetry.bin | axmg-sensory-mcp --raw`

## Типичный цикл

1. **Новый факт** → `remember("факт")` → система открывает новую структуру
2. **Нужна идея** → `recall("тема")` → система находит ассоциации
3. **Проверь гипотезу** → `check_surprise("гипотеза")` → система оценивает согласованность
4. **Диагностика** → `memory://stats` и `memory://focused` → смотришь здоровье

## Ключевые числа

- `surprise_static >= 1.5` → это новое, неожиданное (`SURPRISING`)
- `surprise_static < 1.0` → это знакомое, типичное (`FAMILIAR`)
- `relevance_score > 0.8` → сильная ассоциация
- `compression_ratio > 3.0` → хорошее сжатие текста
- `active_focus_ratio > 0.2` → активная память в фокусе

## Когда что вызывать

| Задача | Инструмент / Ресурс |
|--------|---------------------|
| Сохранить факт, вывод, идею | `remember()` |
| Вспомнить релевантный контекст | `recall()` |
| Проверить логичность утверждения | `check_surprise()` |
| Увидеть активные концепты | `memory://focused` |
| Проверить здоровье памяти | `memory://stats` |
| Посмотреть историю событий | `memory://timeline` |
| Записать байты телеметрии | `ingest_raw_bytes()` / `--raw` |

## Текущее состояние (Фазы 1–4 Дорожной карты завершены)

- ✅ `remember()` работает, вычисляет surprise в горячем цикле
- ✅ `recall()` работает через субграфный PPMI-обход
- ✅ `check_surprise()` работает, оценивает согласованность без мутации стора
- ✅ `memory://focused`, `memory://stats`, `memory://timeline`
- ✅ `axmg-sensory-mcp` собирает логи, Git, файлы и сырые байты
- ✅ ACID-персистентность на `redb` и восстановление при холодном старте
- ✅ 96.89% экономия контекста LLM подтверждена E2E-тестами
