//! # Axmg — Высокоуровневый фасад и универсальный движок памяти
//!
//! Инкапсулирует Merkle DAG Store, Колесо времени, вытеснение памяти (K=3 + weight cut),
//! расчет Surprise и персистентность в redb в единый, безопасный и эргономичный интерфейс.

use std::fmt;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::death::{DeathLog, FocusConcept, FocusSet};
use crate::merge::TieBreak;
use crate::persist::{load_all, save_all, LoadError};
use crate::recall::{recall, RecallCandidate, RecallConfig};
use crate::recognizer::StreamingRecognizer;
use crate::store::Store;
use crate::surprise::{surprise, SurpriseReport, SurpriseVerdict};
use crate::wheel::WheelState;
use crate::BIRTH_THRESHOLD;

use serde::{Deserialize, Serialize};

/// Ошибки движка памяти axmg.
#[derive(Debug)]
pub enum AxmgError {
    /// Ошибка загрузки/верификации Merkle DAG или DeathLog из redb.
    Persist(LoadError),
    /// Ошибка транзакционной базы данных redb.
    Redb(redb::Error),
    /// Ошибка ввода-вывода (файлы, диск).
    Io(std::io::Error),
    /// Ошибка сериализации/десериализации JSON.
    Json(serde_json::Error),
}

impl fmt::Display for AxmgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AxmgError::Persist(e) => write!(f, "ошибка персистентности axmg: {e}"),
            AxmgError::Redb(e) => write!(f, "ошибка redb: {e}"),
            AxmgError::Io(e) => write!(f, "ошибка ввода-вывода: {e}"),
            AxmgError::Json(e) => write!(f, "ошибка json: {e}"),
        }
    }
}

impl std::error::Error for AxmgError {}

impl From<LoadError> for AxmgError {
    fn from(e: LoadError) -> Self {
        AxmgError::Persist(e)
    }
}

impl From<redb::Error> for AxmgError {
    fn from(e: redb::Error) -> Self {
        AxmgError::Redb(e)
    }
}

impl From<std::io::Error> for AxmgError {
    fn from(e: std::io::Error) -> Self {
        AxmgError::Io(e)
    }
}

impl From<serde_json::Error> for AxmgError {
    fn from(e: serde_json::Error) -> Self {
        AxmgError::Json(e)
    }
}

/// Конфигурация параметров работы движка Axmg.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxmgConfig {
    /// Путь к базе данных redb (None = работа только в оперативной памяти).
    pub db_path: Option<PathBuf>,
    /// Путь к файлу лога событий таймлайна в формате JSONL.
    pub timeline_path: Option<PathBuf>,
    /// Порог рождения пар (T в спеке, по умолчанию 2).
    pub birth_threshold: u64,
    /// Стратегия разрешения конфликтов при равной частоте пар.
    pub tie_break: TieBreak,
    /// Автоматическое сохранение в redb при каждом вызове ingest.
    pub auto_save: bool,
    /// Размер окна активных концептов в снапшоте фокуса (по умолчанию 20).
    pub focus_window: usize,
    /// Конфигурация поиска ассоциаций по умолчанию.
    pub recall_config: RecallConfig,
}

impl Default for AxmgConfig {
    fn default() -> Self {
        Self {
            db_path: None,
            timeline_path: None,
            birth_threshold: BIRTH_THRESHOLD,
            tie_break: TieBreak::BirthOrder,
            auto_save: true,
            focus_window: 20,
            recall_config: RecallConfig::default(),
        }
    }
}

/// Строитель (Builder) для гибкой настройки экземпляра Axmg.
#[derive(Debug, Default)]
pub struct AxmgBuilder {
    config: AxmgConfig,
}

impl AxmgBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn db_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.db_path = Some(path.into());
        self
    }

    pub fn timeline_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.timeline_path = Some(path.into());
        self
    }

    pub fn birth_threshold(mut self, threshold: u64) -> Self {
        self.config.birth_threshold = threshold;
        self
    }

    pub fn tie_break(mut self, tie_break: TieBreak) -> Self {
        self.config.tie_break = tie_break;
        self
    }

    pub fn auto_save(mut self, auto_save: bool) -> Self {
        self.config.auto_save = auto_save;
        self
    }

    pub fn focus_window(mut self, focus_window: usize) -> Self {
        self.config.focus_window = focus_window;
        self
    }

    pub fn recall_config(mut self, recall_config: RecallConfig) -> Self {
        self.config.recall_config = recall_config;
        self
    }

    pub fn recall_limit(mut self, limit: usize) -> Self {
        self.config.recall_config.limit = limit;
        self
    }

    pub fn recall_ppmi_threshold(mut self, threshold: f64) -> Self {
        self.config.recall_config.ppmi_threshold = threshold;
        self
    }

    pub fn build(self) -> Result<Axmg, AxmgError> {
        Axmg::with_config(self.config)
    }
}

/// Отчёт об интеграции порции данных в память.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestReport {
    /// Номер оборота Колеса после интеграции.
    pub revolution: u32,
    /// Число поступивших байт.
    pub bytes_ingested: usize,
    /// Метрика удивления S-static (сжимаемость замороженным словарем).
    pub surprise_static: f64,
    /// Метрика удивления S-growth (прирост словаря).
    pub surprise_growth: f64,
    /// Нормализованный общий скор удивления.
    pub surprise_score: f64,
    /// Вердикт классификатора (Familiar / NovelGrowth / Noise).
    pub verdict: SurpriseVerdict,
    /// Число реально рождённых новых токенов Merkle DAG на этом обороте.
    pub new_tokens_count: usize,
    /// Общий размер активного множества фокуса внимания.
    pub active_focus_count: usize,
    /// Число концептов, впервые или повторно вошедших в фокус.
    pub newly_focused: usize,
    /// Число концептов, вышедших из фокуса (устаревание).
    pub newly_unfocused: usize,
    /// Число концептов, срезанных квантилем по весу (weight cut 25%).
    pub weight_cut_count: usize,
    /// Является ли событие новым смысловым сигналом (S-growth > 0).
    pub is_novel: bool,
}

/// Запись в историческом журнале событий (таймлайне).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub revolution: u32,
    pub timestamp: u64,
    pub context_tag: String,
    pub text: String,
    pub surprise_score: f64,
    pub verdict: String,
    pub new_tokens: usize,
}

/// Статистика текущего состояния ядра памяти.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineStats {
    /// Общее число токенов в Merkle DAG (включая фабричные 256 байт).
    pub total_tokens: usize,
    /// Текущий оборот Колеса времени.
    pub current_revolution: u32,
    /// Длина рабочей сжатой последовательности токенов.
    pub sequence_length: usize,
    /// Общий суммарный объём сырых байт в потоке.
    pub raw_stream_bytes: usize,
    /// Коэффициент сжатия последовательности относительно сырых байт.
    pub compression_ratio: f64,
    /// Число токенов в активном фокусе внимания.
    pub active_focus_tokens: usize,
    /// Доля активного фокуса от общего размера графа памяти.
    pub active_focus_ratio: f64,
    /// Общее число событий в таймлайне.
    pub total_timeline_events: usize,
}

/// Результат ассоциативного поиска с метаданными.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResponse {
    pub query: String,
    pub parsed_tokens_count: usize,
    pub associations: Vec<RecallCandidate>,
    pub context_string: String,
}

/// Универсальный движок ассоциативной памяти axmg.
pub struct Axmg {
    wheel: WheelState,
    death_log: DeathLog,
    focus_snapshot: Arc<FocusSet>,
    config: AxmgConfig,
    timeline: Vec<TimelineEntry>,
}

impl Axmg {
    /// Создает изолированный in-memory экземпляр движка без записи на диск.
    pub fn in_memory() -> Self {
        let mut store = Store::new();
        store.init_factory();

        let wheel = WheelState::with_store(store);
        let mut death_log = DeathLog::new();
        death_log.mark(&wheel);
        let focus_snapshot = Arc::new(death_log.snapshot(&wheel, 20));

        Self {
            wheel,
            death_log,
            focus_snapshot,
            config: AxmgConfig::default(),
            timeline: Vec::new(),
        }
    }

    /// Открывает существующую базу redb по пути или создаёт новую с автоматическим ведением таймлайна.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AxmgError> {
        let db_path = path.as_ref().to_path_buf();
        let tl_path = {
            let file_stem = db_path.file_stem().and_then(|s| s.to_str()).unwrap_or("axmg");
            let mut p = db_path.clone();
            p.set_file_name(format!("{file_stem}_timeline.jsonl"));
            p
        };

        AxmgBuilder::new()
            .db_path(db_path)
            .timeline_path(tl_path)
            .build()
    }

    /// Создает экземпляр с заданной конфигурацией.
    pub fn with_config(config: AxmgConfig) -> Result<Self, AxmgError> {
        let (store, mut death_log) = match &config.db_path {
            Some(path) if path.exists() => load_all(path)?,
            _ => {
                let mut s = Store::new();
                s.init_factory();
                let dl = DeathLog::new();
                (s, dl)
            }
        };

        let mut timeline = Vec::new();
        if let Some(ref tl_path) = config.timeline_path {
            if tl_path.exists() {
                if let Ok(file) = OpenOptions::new().read(true).open(tl_path) {
                    let reader = BufReader::new(file);
                    for line in reader.lines().flatten() {
                        if let Ok(entry) = serde_json::from_str::<TimelineEntry>(&line) {
                            timeline.push(entry);
                        }
                    }
                }
            }
        }

        let mut wheel = WheelState::with_store(store);

        // Восстановление последовательности и оборота из сохранённого таймлайна
        for entry in &timeline {
            let ids = StreamingRecognizer::recognize(&wheel.store, entry.text.as_bytes());
            wheel.sequence.ids.extend(ids);
        }
        if let Some(last) = timeline.last() {
            wheel.revolution = last.revolution;
        }

        if death_log.events().is_empty() {
            death_log.mark(&wheel);
        }
        let focus_snapshot = Arc::new(death_log.snapshot(&wheel, config.focus_window));

        Ok(Self {
            wheel,
            death_log,
            focus_snapshot,
            config,
            timeline,
        })
    }

    /// Создает построитель параметров AxmgBuilder.
    pub fn builder() -> AxmgBuilder {
        AxmgBuilder::new()
    }

    /// Интегрирует строку текста в память (тег контекста "general").
    pub fn ingest(&mut self, text: &str) -> Result<IngestReport, AxmgError> {
        self.ingest_tagged(text, "general")
    }

    /// Интегрирует строку текста с заданным тегом контекста.
    pub fn ingest_tagged(&mut self, text: &str, tag: &str) -> Result<IngestReport, AxmgError> {
        self.ingest_bytes_tagged(text.as_bytes(), tag)
    }

    /// Интегрирует сырой срез байтов (тег контекста "raw").
    pub fn ingest_bytes(&mut self, bytes: &[u8]) -> Result<IngestReport, AxmgError> {
        self.ingest_bytes_tagged(bytes, "raw")
    }

    /// Основной горячий цикл интеграции байтов: вращение колеса, расчет Surprise,
    /// вытеснение шума K=3, срез весов 25%, обновление фокуса и опциональный ACID-сброс.
    pub fn ingest_bytes_tagged(&mut self, bytes: &[u8], tag: &str) -> Result<IngestReport, AxmgError> {
        // 1. Вращаем колесо времени (потоковая свертка + расчет Surprise без клонирования стора)
        let report = self.wheel.turn(bytes, self.config.birth_threshold, self.config.tie_break);

        // 2. Обновляем вытеснение шума (достижимость корней K=3 и срез по весу)
        let mark_outcome = self.death_log.mark(&self.wheel);
        let cut_outcome = self.death_log.weight_cut(&self.wheel);

        // 3. Формируем снимок активного фокуса
        let new_focus = self.death_log.snapshot(&self.wheel, self.config.focus_window);
        self.focus_snapshot = Arc::new(new_focus);

        // 4. Опциональный автоматический ACID-сброс в redb
        if self.config.auto_save {
            if let Some(ref db_path) = self.config.db_path {
                save_all(&self.wheel.store, &self.death_log, db_path)?;
            }
        }

        // 5. Запись в таймлайн
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let display_text = match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(_) => format!("<binary data: {} bytes>", bytes.len()),
        };

        let verdict_str = match report.verdict {
            SurpriseVerdict::Familiar => "FAMILIAR",
            SurpriseVerdict::NovelGrowth => "NOVEL_GROWTH",
            SurpriseVerdict::Noise => "NOISE",
        };

        let entry = TimelineEntry {
            revolution: report.revolution,
            timestamp,
            context_tag: tag.to_string(),
            text: display_text,
            surprise_score: (report.surprise_score * 1000.0).round() / 1000.0,
            verdict: verdict_str.to_string(),
            new_tokens: report.new_tokens_count,
        };

        if let Some(ref tl_path) = self.config.timeline_path {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(tl_path) {
                if let Ok(json_line) = serde_json::to_string(&entry) {
                    let _ = writeln!(file, "{json_line}");
                }
            }
        }
        self.timeline.push(entry);

        let is_novel = report.s_growth > 0.0;

        Ok(IngestReport {
            revolution: report.revolution,
            bytes_ingested: bytes.len(),
            surprise_static: report.s_static,
            surprise_growth: report.s_growth,
            surprise_score: report.surprise_score,
            verdict: report.verdict,
            new_tokens_count: report.new_tokens_count,
            active_focus_count: self.focus_snapshot.len(),
            newly_focused: mark_outcome.newly_focused,
            newly_unfocused: mark_outcome.newly_unfocused,
            weight_cut_count: cut_outcome.cut_count,
            is_novel,
        })
    }

    /// Ассоциативный поиск релевантных концептов по текстовому запросу.
    pub fn recall(&self, query: &str, limit: Option<usize>) -> RecallResponse {
        self.recall_bytes(query.as_bytes(), limit)
    }

    /// Ассоциативный поиск по сырому байтовому запросу.
    pub fn recall_bytes(&self, query: &[u8], limit: Option<usize>) -> RecallResponse {
        let mut cfg = self.config.recall_config.clone();
        if let Some(l) = limit {
            cfg.limit = l;
        }

        let focus_ref = if self.focus_snapshot.is_empty() {
            None
        } else {
            Some(self.focus_snapshot.tokens())
        };

        let result = recall(&self.wheel.store, &self.wheel.sequence.ids, query, focus_ref, &cfg);
        let context_string = result.to_context_string();
        let query_str = String::from_utf8_lossy(query).into_owned();

        RecallResponse {
            query: query_str,
            parsed_tokens_count: result.query_tokens.len(),
            associations: result.candidates,
            context_string,
        }
    }

    /// Проверяет степень удивления (Surprise) текста относительно замороженного стора БЕЗ мутаций.
    pub fn check_surprise(&self, hypothesis: &str) -> SurpriseReport {
        self.check_surprise_bytes(hypothesis.as_bytes())
    }

    /// Проверяет степень удивления (Surprise) байтов относительно замороженного стора БЕЗ мутаций.
    pub fn check_surprise_bytes(&self, bytes: &[u8]) -> SurpriseReport {
        surprise(&self.wheel.store, bytes)
    }

    /// Возвращает потокобезопасный `Arc`-снимок активного множества фокуса внимания.
    pub fn focus(&self) -> Arc<FocusSet> {
        self.focus_snapshot.clone()
    }

    /// Возвращает срез топ-концептов текущего фокуса.
    pub fn focused_concepts(&self) -> &[FocusConcept] {
        self.focus_snapshot.top_concepts()
    }

    /// Формирует компактную строку активного фокуса, готовую для внедрения в системный промпт LLM.
    pub fn prompt_context(&self) -> String {
        let concepts = self.focus_snapshot.top_concepts();
        if concepts.is_empty() {
            return String::new();
        }
        let items: Vec<String> = concepts
            .iter()
            .map(|c| {
                let trimmed = c.text.trim();
                let clean = if trimmed.is_empty() { "<ws>" } else { trimmed };
                format!("{clean} (lvl {}, w {:.2})", c.level, c.ppmi_weight)
            })
            .collect();
        format!("Active Focus [rev {}]: {}", self.focus_snapshot.revolution(), items.join(", "))
    }

    /// Вычисляет текущую системную статистику движка.
    pub fn stats(&self) -> EngineStats {
        let total_tokens = self.wheel.store.len();
        let focus_tokens = self.focus_snapshot.len();
        let active_focus_ratio = if total_tokens > 0 {
            focus_tokens as f64 / total_tokens as f64
        } else {
            0.0
        };

        let seq_len = self.wheel.sequence.ids.len();
        let (raw_bytes, compression_ratio) = if seq_len > 0 && total_tokens > 1 {
            let total_raw: usize = self
                .wheel
                .sequence
                .ids
                .iter()
                .map(|&id| self.wheel.store.bytes_of(id).len())
                .sum();
            let bits_per_token = (total_tokens as f64).log2();
            let compressed_bytes = (seq_len as f64) * bits_per_token / 8.0;
            let ratio = if compressed_bytes > 0.0 {
                total_raw as f64 / compressed_bytes
            } else {
                1.0
            };
            (total_raw, (ratio * 100.0).round() / 100.0)
        } else {
            (0, 1.0)
        };

        EngineStats {
            total_tokens,
            current_revolution: self.wheel.revolution,
            sequence_length: seq_len,
            raw_stream_bytes: raw_bytes,
            compression_ratio,
            active_focus_tokens: focus_tokens,
            active_focus_ratio: (active_focus_ratio * 10000.0).round() / 10000.0,
            total_timeline_events: self.timeline.len(),
        }
    }

    /// Принудительно сохраняет состояние (Store и DeathLog) в redb.
    pub fn save(&self) -> Result<(), AxmgError> {
        if let Some(ref db_path) = self.config.db_path {
            save_all(&self.wheel.store, &self.death_log, db_path)?;
        }
        Ok(())
    }

    /// Возвращает журнал событий таймлайна (до `limit` последних, от свежих к старым).
    pub fn timeline(&self, limit: Option<usize>) -> Vec<TimelineEntry> {
        let count = limit.unwrap_or(self.timeline.len());
        self.timeline.iter().rev().take(count).cloned().collect()
    }

    /// Доступ к неизменному низкоуровневому Store.
    pub fn store(&self) -> &Store {
        &self.wheel.store
    }

    /// Доступ к состоянию Колеса времени.
    pub fn wheel(&self) -> &WheelState {
        &self.wheel
    }

    /// Доступ к логу смерти/вытеснения.
    pub fn death_log(&self) -> &DeathLog {
        &self.death_log
    }

    /// Доступ к конфигурации.
    pub fn config(&self) -> &AxmgConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_memory_lifecycle() {
        let mut engine = Axmg::in_memory();

        // 1. Изначальное состояние
        let stats = engine.stats();
        assert_eq!(stats.total_tokens, 256);
        assert_eq!(stats.current_revolution, 0);

        // 2. Интеграция повторяющегося текста для формирования паттернов
        let report1 = engine.ingest("cat sat on the mat. cat sat on the mat.").unwrap();
        assert_eq!(report1.revolution, 1);
        assert!(report1.new_tokens_count > 0);
        assert!(report1.is_novel);

        // 3. Проверка фокуса
        let focus = engine.focus();
        assert!(focus.len() > 0);
        let prompt_ctx = engine.prompt_context();
        assert!(prompt_ctx.contains("Active Focus"));

        // 4. Проверка ассоциативного вспоминания (recall)
        let recall_resp = engine.recall("cat", Some(3));
        assert_eq!(recall_resp.query, "cat");

        // 5. Проверка Surprise
        let surprise_report = engine.check_surprise("cat sat");
        assert!(surprise_report.s_static > 1.0);

        // 6. Таймлайн
        let tl = engine.timeline(Some(10));
        assert_eq!(tl.len(), 1);
        assert_eq!(tl[0].revolution, 1);
    }

    #[test]
    fn test_disk_persistence_roundtrip() {
        let mut tmp_db = std::env::temp_dir();
        tmp_db.push(format!("axmg_engine_test_{}.redb", std::process::id()));
        let mut tmp_tl = std::env::temp_dir();
        tmp_tl.push(format!("axmg_engine_test_{}_tl.jsonl", std::process::id()));

        let _ = std::fs::remove_file(&tmp_db);
        let _ = std::fs::remove_file(&tmp_tl);

        // Шаг 1: запись данных
        {
            let mut engine = AxmgBuilder::new()
                .db_path(&tmp_db)
                .timeline_path(&tmp_tl)
                .build()
                .unwrap();

            engine.ingest("apple banana orange apple banana orange").unwrap();
            engine.ingest("red green blue red green blue").unwrap();
            assert_eq!(engine.stats().current_revolution, 2);
        }

        // Шаг 2: холодный старт и проверка восстановления
        {
            let engine = AxmgBuilder::new()
                .db_path(&tmp_db)
                .timeline_path(&tmp_tl)
                .build()
                .unwrap();

            let stats = engine.stats();
            assert!(stats.total_tokens > 256);
            assert_eq!(stats.current_revolution, 2);
            assert_eq!(stats.total_timeline_events, 2);

            let recall_resp = engine.recall("apple", Some(5));
            assert_eq!(recall_resp.query, "apple");
        }

        let _ = std::fs::remove_file(&tmp_db);
        let _ = std::fs::remove_file(&tmp_tl);
    }
}
